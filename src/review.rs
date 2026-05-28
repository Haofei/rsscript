use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::diagnostic::{Span, code};
use crate::hir::{CallResolution, Hir, ResolvedCalleeKind};
use crate::syntax::ast::{
    Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FileFeature, FunctionDecl,
    Item, LetKind, Param, Stmt, TypeDecl, TypeKind, TypeRef,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMap {
    pub summary: ReviewMapSummary,
    pub files: Vec<ReviewMapFile>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewMapSummary {
    pub total_functions: usize,
    pub total_lines: usize,
    pub must_review_lines: usize,
    pub safe_to_skip_lines: usize,
    pub unknown_lines: usize,
    pub suggested_review_lines: usize,
    pub review_ratio: ReviewRatio,
    #[serde(rename = "must_review")]
    pub review_required: ReviewMapCategorySummary,
    #[serde(rename = "safe_to_skip")]
    pub foldable: ReviewMapCategorySummary,
    pub unknown: ReviewMapCategorySummary,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReviewRatio {
    scaled: u32,
}

impl Serialize for ReviewRatio {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_f64(f64::from(self.scaled) / 1000.0)
    }
}

impl ReviewRatio {
    fn from_parts(numerator: usize, denominator: usize) -> Self {
        if denominator == 0 {
            return Self { scaled: 0 };
        }
        let scaled = ((numerator.saturating_mul(1000)) / denominator).min(1000) as u32;
        Self { scaled }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewMapCategorySummary {
    pub functions: usize,
    pub lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapFile {
    pub file: String,
    pub features: Vec<String>,
    pub risk: ReviewMapFileRisk,
    pub reasons: Vec<String>,
    pub regions: Vec<ReviewMapRegion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMapFileRisk {
    Low,
    Elevated,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapRegion {
    pub function: String,
    pub classification: ReviewMapClassification,
    pub line: usize,
    pub line_count: usize,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMapClassification {
    #[serde(rename = "must_review")]
    ReviewRequired,
    #[serde(rename = "safe_to_skip")]
    Foldable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewRisk {
    Feature,
    Api,
    TypeLayout,
    Effect,
    Boundary,
    Unsafe,
    Guarantee,
}

impl ReviewRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::Api => "api",
            Self::TypeLayout => "type-layout",
            Self::Effect => "effect",
            Self::Boundary => "boundary",
            Self::Unsafe => "unsafe",
            Self::Guarantee => "guarantee",
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

    if feature_label(&old_program.features) != feature_label(&new_program.features) {
        findings.push(review_finding(
            code::REVIEW_FEATURES_CHANGED,
            ReviewRisk::Feature,
            format!(
                "file features changed from {} to {}.",
                feature_label(&old_program.features),
                feature_label(&new_program.features)
            ),
            Vec::new(),
            Some(feature_label(&old_program.features)),
            Some(feature_label(&new_program.features)),
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

pub fn review_map_sources(sources: Vec<(&str, &str)>) -> ReviewMap {
    let files: Vec<ReviewMapFile> = sources
        .into_iter()
        .map(|(file, source)| review_map_file(file, source))
        .collect();
    ReviewMap {
        summary: review_map_summary(&files),
        files,
    }
}

pub fn format_review_map_human(map: &ReviewMap) -> String {
    if map
        .files
        .iter()
        .all(|file| file.regions.is_empty() && file.reasons.is_empty())
    {
        return "review map: no functions detected\n".to_string();
    }

    let mut output = String::new();
    output.push_str(&format!(
        "summary: must-review {} functions/{} lines; safe-to-skip {} functions/{} lines; unknown {} functions/{} lines; total {} functions/{} lines\n",
        map.summary.review_required.functions,
        map.summary.review_required.lines,
        map.summary.foldable.functions,
        map.summary.foldable.lines,
        map.summary.unknown.functions,
        map.summary.unknown.lines,
        map.summary.total_functions,
        map.summary.total_lines
    ));
    for file in &map.files {
        output.push_str(&format!("{}:", file.file));
        if !file.features.is_empty() {
            output.push_str(&format!(
                " features {}; risk {}",
                file.features.join(", "),
                review_map_file_risk_label(file.risk)
            ));
        }
        if !file.reasons.is_empty() {
            output.push_str(&format!("; {}", file.reasons.join("; ")));
        }
        output.push('\n');
        for region in &file.regions {
            output.push_str(&format!(
                "  {} [{}] line {} ({} lines): {}\n",
                region.function,
                review_map_classification_label(region.classification),
                region.line,
                region.line_count,
                region.reasons.join("; ")
            ));
        }
    }
    output
}

pub fn format_review_map_json(map: &ReviewMap) -> String {
    serde_json::to_string(map).expect("review map JSON serialization should not fail")
}

fn review_map_summary(files: &[ReviewMapFile]) -> ReviewMapSummary {
    let mut summary = ReviewMapSummary::default();
    for region in files.iter().flat_map(|file| file.regions.iter()) {
        summary.total_functions += 1;
        summary.total_lines += region.line_count;
        let category = match region.classification {
            ReviewMapClassification::ReviewRequired => &mut summary.review_required,
            ReviewMapClassification::Foldable => &mut summary.foldable,
            ReviewMapClassification::Unknown => &mut summary.unknown,
        };
        category.functions += 1;
        category.lines += region.line_count;
    }
    summary.must_review_lines = summary.review_required.lines;
    summary.safe_to_skip_lines = summary.foldable.lines;
    summary.unknown_lines = summary.unknown.lines;
    summary.suggested_review_lines = summary.review_required.lines + summary.unknown.lines;
    summary.review_ratio =
        ReviewRatio::from_parts(summary.suggested_review_lines, summary.total_lines);
    summary
}

fn review_map_file(file: &str, source: &str) -> ReviewMapFile {
    let program = parse_source(file, source);
    let hir = Hir::from_syntax(&program);
    let mut function_lines = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some((function.name.clone(), function.span.line)),
            Item::Type(_) => None,
        })
        .collect::<Vec<_>>();
    function_lines.sort_by_key(|(_, line)| *line);
    let total_lines = source.lines().count().max(1);

    let mut region_drafts = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(review_map_region_draft(
                function,
                &hir,
                &function_lines,
                total_lines,
            )),
            Item::Type(_) => None,
        })
        .collect::<Vec<_>>();
    propagate_review_map_call_classifications(&mut region_drafts);
    let regions = region_drafts
        .into_iter()
        .map(|draft| draft.region)
        .collect();

    let features = feature_names(&program.features);
    ReviewMapFile {
        file: file.to_string(),
        features,
        risk: review_map_file_risk(&program.features),
        reasons: review_map_file_reasons(&program.features),
        regions,
    }
}

fn review_map_region_draft(
    function: &FunctionDecl,
    hir: &Hir,
    function_lines: &[(String, usize)],
    total_lines: usize,
) -> ReviewMapRegionDraft {
    let mut facts = ReviewMapFacts::default();
    collect_review_map_facts_block(&function.body, hir, &mut facts);

    let mut reasons = Vec::new();
    if function.is_public {
        reasons.push("public entry point".to_string());
    }
    if is_entry_function(&function.name) {
        reasons.push("entry point".to_string());
    }
    for param in &function.params {
        if matches!(param.effect, Some(DataEffect::Mut | DataEffect::Take)) {
            reasons.push(format!(
                "{} parameter `{}`",
                effect_label(param.effect.expect("effect matched")),
                param.name
            ));
        }
        if type_ref_contains_name(&param.ty, "ResourcePool") {
            reasons.push(format!("ResourcePool parameter `{}`", param.name));
        }
    }
    if let Some(return_ty) = &function.return_ty
        && type_ref_contains_name(return_ty, "ResourcePool")
    {
        reasons.push("ResourcePool return type".to_string());
    }
    if function.returns_fresh {
        reasons.push("fresh guarantee boundary".to_string());
    }
    for effect in &function.effects {
        match effect {
            EffectDecl::Retains(param) => reasons.push(format!("retains `{param}`")),
            EffectDecl::Name(name) if matches!(name.as_str(), "native" | "unsafe") => {
                reasons.push(format!("{name} boundary"))
            }
            EffectDecl::Name(name) if is_runtime_guarantee_boundary(name) => {
                reasons.push(format!("guarantee `{name}`"))
            }
            _ => {}
        }
    }
    if facts.has_local {
        reasons.push("local binding".to_string());
    }
    if facts.has_manage {
        reasons.push("manage boundary".to_string());
    }
    if facts.has_with {
        reasons.push("resource with block".to_string());
    }
    if facts.has_take {
        reasons.push("take effect".to_string());
    }
    if facts.has_mut {
        reasons.push("mut call-site effect".to_string());
    }
    if facts.has_resource_pool {
        reasons.push("ResourcePool usage".to_string());
    }

    let had_review_reason = !reasons.is_empty();
    let classification = if !facts.unresolved_calls.is_empty() {
        let calls = facts
            .unresolved_calls
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        reasons.push(format!("unresolved call(s): {calls}"));
        if had_review_reason {
            ReviewMapClassification::ReviewRequired
        } else {
            ReviewMapClassification::Unknown
        }
    } else if reasons.is_empty() {
        reasons.push("private pure helper with no retention or resource boundary".to_string());
        ReviewMapClassification::Foldable
    } else {
        ReviewMapClassification::ReviewRequired
    };

    ReviewMapRegionDraft {
        region: ReviewMapRegion {
            function: function.name.clone(),
            classification,
            line: function.span.line,
            line_count: review_map_line_count(
                &function.name,
                function.span.line,
                function_lines,
                total_lines,
            ),
            reasons,
        },
        facts,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewMapRegionDraft {
    region: ReviewMapRegion,
    facts: ReviewMapFacts,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReviewMapFacts {
    has_local: bool,
    has_manage: bool,
    has_with: bool,
    has_mut: bool,
    has_take: bool,
    has_resource_pool: bool,
    user_calls: BTreeSet<String>,
    unresolved_calls: BTreeSet<String>,
}

fn propagate_review_map_call_classifications(drafts: &mut [ReviewMapRegionDraft]) {
    propagate_unknown_calls(drafts);
    propagate_review_required_calls(drafts);
}

fn propagate_unknown_calls(drafts: &mut [ReviewMapRegionDraft]) {
    loop {
        let classifications = drafts
            .iter()
            .map(|draft| (draft.region.function.clone(), draft.region.classification))
            .collect::<BTreeMap<_, _>>();
        let mut changed = false;

        for draft in drafts.iter_mut() {
            if draft.region.classification == ReviewMapClassification::Unknown {
                continue;
            }
            let Some(callee) = draft.facts.user_calls.iter().find(|callee| {
                classifications.get(*callee) == Some(&ReviewMapClassification::Unknown)
            }) else {
                continue;
            };
            draft.region.classification = ReviewMapClassification::Unknown;
            draft.region.reasons.retain(|reason| {
                reason != "private pure helper with no retention or resource boundary"
            });
            draft
                .region
                .reasons
                .push(format!("calls unknown `{callee}`"));
            changed = true;
        }

        if !changed {
            break;
        }
    }
}

fn propagate_review_required_calls(drafts: &mut [ReviewMapRegionDraft]) {
    loop {
        let classifications = drafts
            .iter()
            .map(|draft| (draft.region.function.clone(), draft.region.classification))
            .collect::<BTreeMap<_, _>>();
        let mut changed = false;

        for draft in drafts.iter_mut() {
            if draft.region.classification != ReviewMapClassification::Foldable {
                continue;
            }
            let Some(callee) = draft.facts.user_calls.iter().find(|callee| {
                classifications.get(*callee) == Some(&ReviewMapClassification::ReviewRequired)
            }) else {
                continue;
            };
            draft.region.classification = ReviewMapClassification::ReviewRequired;
            draft.region.reasons.retain(|reason| {
                reason != "private pure helper with no retention or resource boundary"
            });
            draft
                .region
                .reasons
                .push(format!("calls must-review `{callee}`"));
            changed = true;
        }

        if !changed {
            break;
        }
    }
}

fn collect_review_map_facts_block(block: &Block, hir: &Hir, facts: &mut ReviewMapFacts) {
    for statement in &block.statements {
        collect_review_map_facts_stmt(statement, hir, facts);
    }
}

fn collect_review_map_facts_stmt(statement: &Stmt, hir: &Hir, facts: &mut ReviewMapFacts) {
    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local {
                facts.has_local = true;
            }
            if let Some(value) = &stmt.value {
                collect_review_map_facts_expr(value, hir, facts);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_review_map_facts_expr(value, hir, facts);
            }
        }
        Stmt::With(stmt) => {
            facts.has_with = true;
            collect_review_map_facts_expr(&stmt.resource, hir, facts);
            collect_review_map_facts_block(&stmt.body, hir, facts);
        }
        Stmt::If(stmt) => {
            collect_review_map_facts_expr(&stmt.condition, hir, facts);
            collect_review_map_facts_block(&stmt.then_body, hir, facts);
            if let Some(else_body) = &stmt.else_body {
                collect_review_map_facts_block(else_body, hir, facts);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_review_map_facts_expr(condition, hir, facts);
            }
            collect_review_map_facts_block(&stmt.body, hir, facts);
        }
        Stmt::Expr(expr) => collect_review_map_facts_expr(expr, hir, facts),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
    }
}

fn collect_review_map_facts_expr(expr: &Expr, hir: &Hir, facts: &mut ReviewMapFacts) {
    match expr {
        Expr::Call { callee, args, .. } => {
            if is_resource_pool_callee(callee) {
                facts.has_resource_pool = true;
            }
            match hir.resolve_call(callee) {
                CallResolution::Resolved { signature, kind } => {
                    if kind == ResolvedCalleeKind::UserFunction {
                        facts.user_calls.insert(signature.name);
                    }
                }
                CallResolution::Unknown => {
                    facts.unresolved_calls.insert(review_callee_display(callee));
                }
                CallResolution::EnumVariant => {}
            }
            for arg in args {
                collect_review_map_facts_expr(&arg.value, hir, facts);
            }
        }
        Expr::Effect { effect, value, .. } => {
            match effect {
                DataEffect::Mut => facts.has_mut = true,
                DataEffect::Take => facts.has_take = true,
                DataEffect::Read => {}
            }
            collect_review_map_facts_expr(value, hir, facts);
        }
        Expr::Manage { value, .. } => {
            facts.has_manage = true;
            collect_review_map_facts_expr(value, hir, facts);
        }
        Expr::Try { value, .. } => collect_review_map_facts_expr(value, hir, facts),
        Expr::Binary { left, right, .. } => {
            collect_review_map_facts_expr(left, hir, facts);
            collect_review_map_facts_expr(right, hir, facts);
        }
        Expr::Field { base, .. } => collect_review_map_facts_expr(base, hir, facts),
        Expr::Index { base, index, .. } => {
            collect_review_map_facts_expr(base, hir, facts);
            collect_review_map_facts_expr(index, hir, facts);
        }
        Expr::Closure { body, .. } => collect_review_map_facts_block(body, hir, facts),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn review_callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
    }
}

fn is_resource_pool_callee(callee: &Callee) -> bool {
    match callee {
        Callee::Name(name) => name == "ResourcePool",
        Callee::Qualified { namespace, .. } => type_root_name(namespace) == "ResourcePool",
    }
}

fn type_ref_contains_name(ty: &TypeRef, name: &str) -> bool {
    ty.name == name || ty.args.iter().any(|arg| type_ref_contains_name(arg, name))
}

fn type_root_name(type_name: &str) -> &str {
    type_name
        .split_once('<')
        .map_or(type_name, |(root, _)| root)
}

fn review_map_line_count(
    function_name: &str,
    start_line: usize,
    function_lines: &[(String, usize)],
    total_lines: usize,
) -> usize {
    let next_line = function_lines
        .iter()
        .find(|(name, line)| name != function_name && *line > start_line)
        .map(|(_, line)| *line)
        .unwrap_or(total_lines + 1);
    next_line.saturating_sub(start_line).max(1)
}

fn review_map_classification_label(classification: ReviewMapClassification) -> &'static str {
    match classification {
        ReviewMapClassification::ReviewRequired => "must-review",
        ReviewMapClassification::Foldable => "safe-to-skip",
        ReviewMapClassification::Unknown => "unknown",
    }
}

fn review_map_file_risk_label(risk: ReviewMapFileRisk) -> &'static str {
    match risk {
        ReviewMapFileRisk::Low => "low",
        ReviewMapFileRisk::Elevated => "elevated",
        ReviewMapFileRisk::High => "high",
    }
}

fn is_entry_function(name: &str) -> bool {
    matches!(name, "main" | "run")
        || name.starts_with("run_")
        || name.starts_with("handle_")
        || name.ends_with("_handler")
}

fn is_runtime_guarantee_boundary(effect: &str) -> bool {
    matches!(effect, "no_panic" | "noalloc" | "no_block")
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
        code::REVIEW_FEATURES_CHANGED => (
            "review_file_features",
            "Review whether this file should enable the changed advanced capabilities.",
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
        code::REVIEW_GUARANTEE_REMOVED => (
            "review_removed_guarantee",
            "Review callers that relied on the removed runtime guarantee.",
        ),
        code::REVIEW_FUNCTION_KIND_CHANGED => (
            "review_function_kind",
            "Review callers and scheduling assumptions for the changed function kind.",
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
    is_async: bool,
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
    is_weak: bool,
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
        is_weak: field.is_weak,
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
        is_async: function.is_async,
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
    if old.is_async != new.is_async {
        findings.push(review_finding(
            code::REVIEW_FUNCTION_KIND_CHANGED,
            ReviewRisk::Api,
            format!("function `{}` kind changed.", old.name),
            paired_spans(
                &old.span,
                &new.span,
                "old function kind",
                "new function kind",
            ),
            Some(function_kind_contract(old)),
            Some(function_kind_contract(new)),
        ));
    }
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
    let old_guarantees = guarantee_effects(&old.effects);
    let new_guarantees = guarantee_effects(&new.effects);
    let removed_guarantees: BTreeSet<_> = old_guarantees
        .difference(&new_guarantees)
        .cloned()
        .collect();
    if !removed_guarantees.is_empty() {
        findings.push(review_finding(
            code::REVIEW_GUARANTEE_REMOVED,
            ReviewRisk::Guarantee,
            format!(
                "function `{}` removed guarantee(s): {}.",
                old.name,
                effects_contract(&removed_guarantees)
            ),
            paired_spans(&old.span, &new.span, "old guarantees", "new guarantees"),
            Some(effects_contract(&old_guarantees)),
            Some(effects_contract(&new_guarantees)),
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

fn feature_label(features: &[FileFeature]) -> String {
    let labels = feature_names(features);
    if labels.is_empty() {
        return "<none>".to_string();
    }
    labels.join(",")
}

fn feature_names(features: &[FileFeature]) -> Vec<String> {
    let mut labels = features
        .iter()
        .map(feature_name)
        .map(str::to_string)
        .collect::<Vec<_>>();
    labels.sort_unstable();
    labels.dedup();
    labels
}

fn feature_name(feature: &FileFeature) -> &'static str {
    match feature {
        FileFeature::Local => "local",
        FileFeature::Native => "native",
        FileFeature::Unsafe => "unsafe",
        FileFeature::Async => "async",
        FileFeature::Device => "device",
        FileFeature::Ffi => "ffi",
        FileFeature::Reflection => "reflection",
    }
}

fn review_map_file_risk(features: &[FileFeature]) -> ReviewMapFileRisk {
    if features.iter().any(|feature| {
        matches!(
            feature,
            FileFeature::Native | FileFeature::Unsafe | FileFeature::Device | FileFeature::Ffi
        )
    }) {
        ReviewMapFileRisk::High
    } else if features.iter().any(|feature| {
        matches!(
            feature,
            FileFeature::Local | FileFeature::Async | FileFeature::Reflection
        )
    }) {
        ReviewMapFileRisk::Elevated
    } else {
        ReviewMapFileRisk::Low
    }
}

fn review_map_file_reasons(features: &[FileFeature]) -> Vec<String> {
    feature_names(features)
        .into_iter()
        .filter_map(|feature| review_map_feature_reason(&feature).map(str::to_string))
        .collect()
}

fn review_map_feature_reason(feature: &str) -> Option<&'static str> {
    match feature {
        "local" => Some("local capability enabled"),
        "native" => Some("native boundary capability enabled"),
        "unsafe" => Some("unsafe capability enabled"),
        "async" => Some("async control-flow capability enabled"),
        "device" => Some("device capability enabled"),
        "ffi" => Some("ffi boundary capability enabled"),
        "reflection" => Some("reflection capability enabled"),
        _ => None,
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
        "{} {}({})",
        function_kind_contract(function),
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

fn function_kind_contract(function: &FunctionSig) -> String {
    if function.is_async {
        "async fn".to_string()
    } else {
        "fn".to_string()
    }
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

fn guarantee_effects(effects: &BTreeSet<String>) -> BTreeSet<String> {
    effects
        .iter()
        .filter(|effect| {
            matches!(
                effect.as_str(),
                "no_panic" | "noalloc" | "no_block" | "pure"
            )
        })
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
            if field.is_weak {
                format!("{name}: weak {}", field.type_name)
            } else if field.is_handle {
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
        Expr::Effect { value, .. } | Expr::Try { value, .. } => {
            collect_boundary_expr(value, path, boundary);
        }
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
        Expr::Index { base, index, .. } => {
            collect_boundary_expr(base, path, boundary);
            collect_boundary_expr(index, path, boundary);
        }
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
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            boundary_expr_subject(value)
        }
        Expr::Field { name, .. } => Some(format!(".{name}")),
        Expr::Index { .. } => None,
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
