use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::diagnostic::{Span, code};
use crate::syntax::ast::{
    Block, CallArg, DataEffect, EffectDecl, Expr, FieldDecl, FileMode, FunctionDecl, Item, LetKind,
    Param, Stmt, TypeDecl, TypeKind, TypeRef,
};
use crate::syntax::parse_source;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewFinding {
    pub code: String,
    pub risk: ReviewRisk,
    pub summary: String,
    pub spans: Vec<ReviewSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    pub fixes: Vec<ReviewFix>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewSpan {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewFix {
    pub kind: String,
    pub title: String,
    pub applicability: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewRisk {
    Mode,
    Api,
    TypeLayout,
    Effect,
    Boundary,
    Unsafe,
}

impl ReviewRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mode => "mode",
            Self::Api => "api",
            Self::TypeLayout => "type-layout",
            Self::Effect => "effect",
            Self::Boundary => "boundary",
            Self::Unsafe => "unsafe",
        }
    }
}

pub fn review_sources(
    old_file: &str,
    old_source: &str,
    new_file: &str,
    new_source: &str,
) -> Vec<ReviewFinding> {
    let old_program = parse_source(old_file, old_source);
    let new_program = parse_source(new_file, new_source);
    let mut findings = Vec::new();

    if old_program.mode != new_program.mode {
        findings.push(review_finding(
            code::REVIEW_MODE_CHANGED,
            ReviewRisk::Mode,
            format!(
                "file mode changed from {} to {}.",
                file_mode_label(old_program.mode),
                file_mode_label(new_program.mode)
            ),
            Vec::new(),
            Some(file_mode_label(old_program.mode).to_string()),
            Some(file_mode_label(new_program.mode).to_string()),
        ));
    }

    let old_types = collect_type_sigs(&old_program.items);
    let new_types = collect_type_sigs(&new_program.items);
    let type_names: BTreeSet<_> = old_types.keys().chain(new_types.keys()).cloned().collect();

    for name in type_names {
        match (old_types.get(&name), new_types.get(&name)) {
            (Some(old), None) => findings.push(review_finding(
                code::REVIEW_TYPE_REMOVED,
                ReviewRisk::TypeLayout,
                format!("type `{name}` was removed."),
                vec![review_span(&old.span, "removed type")],
                Some(type_contract(old)),
                None,
            )),
            (None, Some(new)) => findings.push(review_finding(
                code::REVIEW_TYPE_ADDED,
                ReviewRisk::TypeLayout,
                format!("type `{name}` was added."),
                vec![review_span(&new.span, "added type")],
                None,
                Some(type_contract(new)),
            )),
            (Some(old), Some(new)) => compare_type(old, new, &mut findings),
            (None, None) => {}
        }
    }

    let old_functions = collect_function_sigs(&old_program.items);
    let new_functions = collect_function_sigs(&new_program.items);
    let function_names: BTreeSet<_> = old_functions
        .keys()
        .chain(new_functions.keys())
        .cloned()
        .collect();

    for name in function_names {
        match (old_functions.get(&name), new_functions.get(&name)) {
            (Some(old), None) => findings.push(review_finding(
                code::REVIEW_FUNCTION_REMOVED,
                ReviewRisk::Api,
                format!("function `{name}` was removed."),
                vec![review_span(&old.span, "removed function")],
                Some(function_contract(old)),
                None,
            )),
            (None, Some(new)) => findings.push(review_finding(
                code::REVIEW_FUNCTION_ADDED,
                ReviewRisk::Api,
                format!("function `{name}` was added."),
                vec![review_span(&new.span, "added function")],
                None,
                Some(function_contract(new)),
            )),
            (Some(old), Some(new)) => compare_function(old, new, &mut findings),
            (None, None) => {}
        }
    }

    findings
}

pub fn format_review_human(findings: &[ReviewFinding]) -> String {
    if findings.is_empty() {
        return "review: no API changes detected\n".to_string();
    }

    let mut output = String::new();
    for finding in findings {
        output.push_str(&format!(
            "{}[{}]: {}\n",
            finding.code,
            finding.risk.as_str(),
            finding.summary
        ));
    }
    output
}

pub fn format_review_json(findings: &[ReviewFinding]) -> String {
    serde_json::to_string(findings).expect("review JSON serialization should not fail")
}

fn review_finding(
    code: &str,
    risk: ReviewRisk,
    summary: impl Into<String>,
    spans: Vec<ReviewSpan>,
    before: Option<String>,
    after: Option<String>,
) -> ReviewFinding {
    ReviewFinding {
        code: code.to_string(),
        risk,
        summary: summary.into(),
        spans,
        before,
        after,
        fixes: review_fixes(code),
    }
}

fn review_fixes(code: &str) -> Vec<ReviewFix> {
    let (kind, title) = match code {
        code::REVIEW_MODE_CHANGED => (
            "review_file_mode",
            "Review whether this file should allow local ownership features.",
        ),
        code::REVIEW_FUNCTION_REMOVED => (
            "restore_or_migrate_function",
            "Restore the function or migrate all call sites.",
        ),
        code::REVIEW_FUNCTION_ADDED => (
            "review_new_api",
            "Review the new API surface and ownership contract.",
        ),
        code::REVIEW_PARAMS_CHANGED => (
            "update_call_sites",
            "Update call sites for the changed parameter contract.",
        ),
        code::REVIEW_RETURN_CHANGED => (
            "review_return_contract",
            "Review callers that depend on the old return and freshness contract.",
        ),
        code::REVIEW_EFFECTS_CHANGED => (
            "review_effect_contract",
            "Review added or removed effects and update callers.",
        ),
        code::REVIEW_TYPE_REMOVED => (
            "restore_or_migrate_type",
            "Restore the type or migrate all consumers.",
        ),
        code::REVIEW_TYPE_ADDED => (
            "review_type_exposure",
            "Review the new type's ownership and resource fields.",
        ),
        code::REVIEW_TYPE_KIND_CHANGED => (
            "review_type_kind",
            "Review class, struct, or resource lifetime semantics for this type.",
        ),
        code::REVIEW_TYPE_FIELDS_CHANGED => (
            "review_type_layout",
            "Review field ownership, handle markers, and resource containment.",
        ),
        code::REVIEW_BOUNDARY_CHANGED => (
            "review_local_manage_boundary",
            "Review the changed local ownership and manage boundary.",
        ),
        code::REVIEW_UNSAFE_NATIVE_ADDED => (
            "review_unsafe_native_boundary",
            "Review the new unsafe or native boundary and require explicit justification.",
        ),
        _ => ("review_change", "Review this source-level contract change."),
    };
    vec![ReviewFix {
        kind: kind.to_string(),
        title: title.to_string(),
        applicability: "manual".to_string(),
    }]
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionSig {
    name: String,
    params: Vec<ParamSig>,
    return_type: Option<String>,
    returns_fresh: bool,
    effects: BTreeSet<String>,
    boundary: BoundarySig,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParamSig {
    name: String,
    effect: Option<&'static str>,
    type_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BoundarySig {
    events: BTreeSet<BoundaryEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BoundaryEvent {
    kind: BoundaryEventKind,
    subject: Option<String>,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BoundaryEventKind {
    LocalBinding,
    Manage,
    Take,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeSig {
    name: String,
    kind: TypeKind,
    fields: BTreeMap<String, FieldSig>,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldSig {
    type_name: String,
    is_handle: bool,
}

fn collect_type_sigs(items: &[Item]) -> BTreeMap<String, TypeSig> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Type(type_decl) => Some((type_decl.name.clone(), type_sig(type_decl))),
            Item::Function(_) => None,
        })
        .collect()
}

fn type_sig(type_decl: &TypeDecl) -> TypeSig {
    TypeSig {
        name: type_decl.name.clone(),
        kind: type_decl.kind,
        fields: type_decl
            .fields
            .iter()
            .map(|field| (field.name.clone(), field_sig(field)))
            .collect(),
        span: type_decl.span.clone(),
    }
}

fn field_sig(field: &FieldDecl) -> FieldSig {
    FieldSig {
        type_name: type_name(&field.ty),
        is_handle: field.is_handle,
    }
}

fn collect_function_sigs(items: &[Item]) -> BTreeMap<String, FunctionSig> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some((function.name.clone(), function_sig(function))),
            Item::Type(_) => None,
        })
        .collect()
}

fn function_sig(function: &FunctionDecl) -> FunctionSig {
    FunctionSig {
        name: function.name.clone(),
        params: function.params.iter().map(param_sig).collect(),
        return_type: function.return_ty.as_ref().map(type_name),
        returns_fresh: function.returns_fresh,
        effects: function.effects.iter().map(effect_name).collect(),
        boundary: boundary_sig(&function.body),
        span: function.span.clone(),
    }
}

fn param_sig(param: &Param) -> ParamSig {
    ParamSig {
        name: param.name.clone(),
        effect: param.effect.map(effect_label),
        type_name: type_name(&param.ty),
    }
}

fn compare_function(old: &FunctionSig, new: &FunctionSig, findings: &mut Vec<ReviewFinding>) {
    if old.params != new.params {
        findings.push(review_finding(
            code::REVIEW_PARAMS_CHANGED,
            ReviewRisk::Api,
            format!("function `{}` parameters changed.", old.name),
            paired_spans(&old.span, &new.span, "old function", "new function"),
            Some(params_contract(&old.params)),
            Some(params_contract(&new.params)),
        ));
    }
    if old.return_type != new.return_type || old.returns_fresh != new.returns_fresh {
        findings.push(review_finding(
            code::REVIEW_RETURN_CHANGED,
            ReviewRisk::Api,
            format!("function `{}` return contract changed.", old.name),
            paired_spans(&old.span, &new.span, "old function", "new function"),
            Some(return_contract(old)),
            Some(return_contract(new)),
        ));
    }
    if old.effects != new.effects {
        findings.push(review_finding(
            code::REVIEW_EFFECTS_CHANGED,
            ReviewRisk::Effect,
            format!("function `{}` effects changed.", old.name),
            paired_spans(&old.span, &new.span, "old function", "new function"),
            Some(effects_contract(&old.effects)),
            Some(effects_contract(&new.effects)),
        ));
    }
    let old_unsafe_native = unsafe_native_effects(&old.effects);
    let new_unsafe_native = unsafe_native_effects(&new.effects);
    let added_unsafe_native: BTreeSet<_> = new_unsafe_native
        .difference(&old_unsafe_native)
        .cloned()
        .collect();
    if !added_unsafe_native.is_empty() {
        findings.push(review_finding(
            code::REVIEW_UNSAFE_NATIVE_ADDED,
            ReviewRisk::Unsafe,
            format!(
                "function `{}` added unsafe/native boundary: {}.",
                old.name,
                effects_contract(&added_unsafe_native)
            ),
            paired_spans(
                &old.span,
                &new.span,
                "old function",
                "new unsafe/native boundary",
            ),
            Some(effects_contract(&old_unsafe_native)),
            Some(effects_contract(&new_unsafe_native)),
        ));
    }
    if old.boundary != new.boundary {
        findings.push(review_finding(
            code::REVIEW_BOUNDARY_CHANGED,
            ReviewRisk::Boundary,
            format!(
                "function `{}` local/manage boundary changed: {}.",
                old.name,
                boundary_change_summary(&old.boundary, &new.boundary)
            ),
            paired_spans(&old.span, &new.span, "old function", "new function"),
            Some(boundary_contract(&old.boundary)),
            Some(boundary_contract(&new.boundary)),
        ));
    }
}

fn compare_type(old: &TypeSig, new: &TypeSig, findings: &mut Vec<ReviewFinding>) {
    if old.kind != new.kind {
        findings.push(review_finding(
            code::REVIEW_TYPE_KIND_CHANGED,
            ReviewRisk::TypeLayout,
            format!(
                "type `{}` kind changed from {} to {}.",
                old.name,
                type_kind_label(old.kind),
                type_kind_label(new.kind)
            ),
            paired_spans(&old.span, &new.span, "old type", "new type"),
            Some(type_kind_label(old.kind).to_string()),
            Some(type_kind_label(new.kind).to_string()),
        ));
    }
    if old.fields != new.fields {
        findings.push(review_finding(
            code::REVIEW_TYPE_FIELDS_CHANGED,
            ReviewRisk::TypeLayout,
            format!("type `{}` field layout changed.", old.name),
            paired_spans(&old.span, &new.span, "old type", "new type"),
            Some(fields_contract(&old.fields)),
            Some(fields_contract(&new.fields)),
        ));
    }
}

fn file_mode_label(mode: Option<FileMode>) -> &'static str {
    match mode {
        Some(FileMode::Managed) => "managed",
        Some(FileMode::UsesLocal) => "uses-local",
        None => "<missing>",
    }
}

fn review_span(span: &Span, label: &str) -> ReviewSpan {
    ReviewSpan {
        file: span.file.clone(),
        line: span.line,
        column: span.column,
        length: span.length,
        label: label.to_string(),
    }
}

fn paired_spans(old: &Span, new: &Span, old_label: &str, new_label: &str) -> Vec<ReviewSpan> {
    vec![review_span(old, old_label), review_span(new, new_label)]
}

fn type_kind_label(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Class => "class",
        TypeKind::Struct => "struct",
        TypeKind::Resource => "resource",
    }
}

fn effect_label(effect: DataEffect) -> &'static str {
    match effect {
        DataEffect::Read => "read",
        DataEffect::Mut => "mut",
        DataEffect::Take => "take",
    }
}

fn function_contract(function: &FunctionSig) -> String {
    let mut contract = format!(
        "fn {}({})",
        function.name,
        params_contract(&function.params)
    );
    if let Some(return_type) = &function.return_type {
        if function.returns_fresh {
            contract.push_str(&format!(" -> fresh {return_type}"));
        } else {
            contract.push_str(&format!(" -> {return_type}"));
        }
    }
    if !function.effects.is_empty() {
        contract.push_str(&format!(
            " effects({})",
            effects_contract(&function.effects)
        ));
    }
    contract
}

fn params_contract(params: &[ParamSig]) -> String {
    params
        .iter()
        .map(|param| match param.effect {
            Some(effect) => format!("{}: {} {}", param.name, effect, param.type_name),
            None => format!("{}: {}", param.name, param.type_name),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn return_contract(function: &FunctionSig) -> String {
    match (&function.return_type, function.returns_fresh) {
        (Some(return_type), true) => format!("fresh {return_type}"),
        (Some(return_type), false) => return_type.clone(),
        (None, _) => "<missing>".to_string(),
    }
}

fn effects_contract(effects: &BTreeSet<String>) -> String {
    if effects.is_empty() {
        return "<none>".to_string();
    }
    effects.iter().cloned().collect::<Vec<_>>().join(", ")
}

fn unsafe_native_effects(effects: &BTreeSet<String>) -> BTreeSet<String> {
    effects
        .iter()
        .filter(|effect| matches!(effect.as_str(), "unsafe" | "native"))
        .cloned()
        .collect()
}

fn type_contract(ty: &TypeSig) -> String {
    format!(
        "{} {} {{ {} }}",
        type_kind_label(ty.kind),
        ty.name,
        fields_contract(&ty.fields)
    )
}

fn fields_contract(fields: &BTreeMap<String, FieldSig>) -> String {
    if fields.is_empty() {
        return "<none>".to_string();
    }
    fields
        .iter()
        .map(|(name, field)| {
            if field.is_handle {
                format!("{name}: handle {}", field.type_name)
            } else {
                format!("{name}: {}", field.type_name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn boundary_contract(boundary: &BoundarySig) -> String {
    if boundary.events.is_empty() {
        return "<none>".to_string();
    }
    boundary
        .events
        .iter()
        .map(boundary_event_label)
        .collect::<Vec<_>>()
        .join("; ")
}

fn type_name(ty: &TypeRef) -> String {
    if ty.args.is_empty() {
        return ty.name.clone();
    }

    let args = ty.args.iter().map(type_name).collect::<Vec<_>>().join(", ");
    format!("{}<{args}>", ty.name)
}

fn effect_name(effect: &EffectDecl) -> String {
    match effect {
        EffectDecl::Name(name) => name.clone(),
        EffectDecl::Retains(param) => format!("retains({param})"),
    }
}

fn boundary_sig(block: &Block) -> BoundarySig {
    let mut boundary = BoundarySig::default();
    collect_boundary_block(block, "body", &mut boundary);
    boundary
}

fn collect_boundary_block(block: &Block, path: &str, boundary: &mut BoundarySig) {
    for (index, statement) in block.statements.iter().enumerate() {
        collect_boundary_stmt(statement, &format!("{path}[{}]", index + 1), boundary);
    }
}

fn collect_boundary_stmt(statement: &Stmt, path: &str, boundary: &mut BoundarySig) {
    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local {
                push_boundary_event(
                    boundary,
                    BoundaryEventKind::LocalBinding,
                    Some(stmt.name.clone()),
                    path,
                );
            }
            if let Some(value) = &stmt.value {
                collect_boundary_expr(value, &format!("{path}.value"), boundary);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_boundary_expr(value, &format!("{path}.return"), boundary);
            }
        }
        Stmt::With(stmt) => {
            collect_boundary_expr(&stmt.resource, &format!("{path}.resource"), boundary);
            collect_boundary_block(&stmt.body, &format!("{path}.body"), boundary);
        }
        Stmt::If(stmt) => {
            collect_boundary_expr(&stmt.condition, &format!("{path}.condition"), boundary);
            collect_boundary_block(&stmt.then_body, &format!("{path}.then"), boundary);
            if let Some(else_body) = &stmt.else_body {
                collect_boundary_block(else_body, &format!("{path}.else"), boundary);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_boundary_expr(condition, &format!("{path}.condition"), boundary);
            }
            collect_boundary_block(&stmt.body, &format!("{path}.loop"), boundary);
        }
        Stmt::Expr(expr) => collect_boundary_expr(expr, path, boundary),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
    }
}

fn collect_boundary_expr(expr: &Expr, path: &str, boundary: &mut BoundarySig) {
    match expr {
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            ..
        } => {
            push_boundary_event(
                boundary,
                BoundaryEventKind::Take,
                boundary_expr_subject(value),
                path,
            );
            collect_boundary_expr(value, &format!("{path}.take"), boundary);
        }
        Expr::Effect { value, .. } => collect_boundary_expr(value, path, boundary),
        Expr::Manage { value, .. } => {
            push_boundary_event(
                boundary,
                BoundaryEventKind::Manage,
                boundary_expr_subject(value),
                path,
            );
            collect_boundary_expr(value, &format!("{path}.manage"), boundary);
        }
        Expr::Call { args, .. } => {
            for (index, CallArg { name, value, .. }) in args.iter().enumerate() {
                let arg_path = name.as_ref().map_or_else(
                    || format!("{path}.arg{}", index + 1),
                    |name| format!("{path}.arg({name})"),
                );
                collect_boundary_expr(value, &arg_path, boundary);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_boundary_expr(left, &format!("{path}.left"), boundary);
            collect_boundary_expr(right, &format!("{path}.right"), boundary);
        }
        Expr::Field { base, .. } => collect_boundary_expr(base, path, boundary),
        Expr::Closure { body, .. } => {
            collect_boundary_block(body, &format!("{path}.closure"), boundary)
        }
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn push_boundary_event(
    boundary: &mut BoundarySig,
    kind: BoundaryEventKind,
    subject: Option<String>,
    path: &str,
) {
    boundary.events.insert(BoundaryEvent {
        kind,
        subject,
        path: path.to_string(),
    });
}

fn boundary_expr_subject(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => Some(name.clone()),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => boundary_expr_subject(value),
        Expr::Field { name, .. } => Some(format!(".{name}")),
        Expr::Call { .. }
        | Expr::Binary { .. }
        | Expr::Closure { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::Unknown(_) => None,
    }
}

fn boundary_change_summary(old: &BoundarySig, new: &BoundarySig) -> String {
    let added = new
        .events
        .difference(&old.events)
        .map(|event| format!("added {}", boundary_event_label(event)))
        .collect::<Vec<_>>();
    let removed = old
        .events
        .difference(&new.events)
        .map(|event| format!("removed {}", boundary_event_label(event)))
        .collect::<Vec<_>>();

    let mut parts = added;
    parts.extend(removed);
    if parts.is_empty() {
        "boundary event paths changed".to_string()
    } else {
        parts.join("; ")
    }
}

fn boundary_event_label(event: &BoundaryEvent) -> String {
    let subject = event
        .subject
        .as_ref()
        .map_or(String::new(), |subject| format!(" `{subject}`"));
    format!(
        "{}{} at {}",
        boundary_event_kind_label(event.kind),
        subject,
        event.path
    )
}

fn boundary_event_kind_label(kind: BoundaryEventKind) -> &'static str {
    match kind {
        BoundaryEventKind::LocalBinding => "local binding",
        BoundaryEventKind::Manage => "manage",
        BoundaryEventKind::Take => "take",
    }
}
