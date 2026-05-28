use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::code;
use crate::syntax::ast::{
    Block, CallArg, DataEffect, EffectDecl, Expr, FieldDecl, FileMode, FunctionDecl, Item, LetKind,
    Param, Stmt, TypeDecl, TypeKind, TypeRef,
};
use crate::syntax::parse_source;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFinding {
    pub code: String,
    pub summary: String,
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
        findings.push(ReviewFinding {
            code: code::REVIEW_MODE_CHANGED.to_string(),
            summary: format!(
                "file mode changed from {} to {}.",
                file_mode_label(old_program.mode),
                file_mode_label(new_program.mode)
            ),
        });
    }

    let old_types = collect_type_sigs(&old_program.items);
    let new_types = collect_type_sigs(&new_program.items);
    let type_names: BTreeSet<_> = old_types.keys().chain(new_types.keys()).cloned().collect();

    for name in type_names {
        match (old_types.get(&name), new_types.get(&name)) {
            (Some(_), None) => findings.push(ReviewFinding {
                code: code::REVIEW_TYPE_REMOVED.to_string(),
                summary: format!("type `{name}` was removed."),
            }),
            (None, Some(_)) => findings.push(ReviewFinding {
                code: code::REVIEW_TYPE_ADDED.to_string(),
                summary: format!("type `{name}` was added."),
            }),
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
            (Some(_), None) => findings.push(ReviewFinding {
                code: code::REVIEW_FUNCTION_REMOVED.to_string(),
                summary: format!("function `{name}` was removed."),
            }),
            (None, Some(_)) => findings.push(ReviewFinding {
                code: code::REVIEW_FUNCTION_ADDED.to_string(),
                summary: format!("function `{name}` was added."),
            }),
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
        output.push_str(&format!("{}: {}\n", finding.code, finding.summary));
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionSig {
    name: String,
    params: Vec<ParamSig>,
    return_type: Option<String>,
    returns_fresh: bool,
    effects: BTreeSet<String>,
    boundary: BoundarySig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParamSig {
    name: String,
    effect: Option<&'static str>,
    type_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BoundarySig {
    local_bindings: BTreeSet<String>,
    manage_count: usize,
    take_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeSig {
    name: String,
    kind: TypeKind,
    fields: BTreeMap<String, FieldSig>,
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
        findings.push(ReviewFinding {
            code: code::REVIEW_PARAMS_CHANGED.to_string(),
            summary: format!("function `{}` parameters changed.", old.name),
        });
    }
    if old.return_type != new.return_type || old.returns_fresh != new.returns_fresh {
        findings.push(ReviewFinding {
            code: code::REVIEW_RETURN_CHANGED.to_string(),
            summary: format!("function `{}` return contract changed.", old.name),
        });
    }
    if old.effects != new.effects {
        findings.push(ReviewFinding {
            code: code::REVIEW_EFFECTS_CHANGED.to_string(),
            summary: format!("function `{}` effects changed.", old.name),
        });
    }
    if old.boundary != new.boundary {
        findings.push(ReviewFinding {
            code: code::REVIEW_BOUNDARY_CHANGED.to_string(),
            summary: format!("function `{}` local/manage boundary changed.", old.name),
        });
    }
}

fn compare_type(old: &TypeSig, new: &TypeSig, findings: &mut Vec<ReviewFinding>) {
    if old.kind != new.kind {
        findings.push(ReviewFinding {
            code: code::REVIEW_TYPE_KIND_CHANGED.to_string(),
            summary: format!(
                "type `{}` kind changed from {} to {}.",
                old.name,
                type_kind_label(old.kind),
                type_kind_label(new.kind)
            ),
        });
    }
    if old.fields != new.fields {
        findings.push(ReviewFinding {
            code: code::REVIEW_TYPE_FIELDS_CHANGED.to_string(),
            summary: format!("type `{}` field layout changed.", old.name),
        });
    }
}

fn file_mode_label(mode: Option<FileMode>) -> &'static str {
    match mode {
        Some(FileMode::Managed) => "managed",
        Some(FileMode::UsesLocal) => "uses-local",
        None => "<missing>",
    }
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
    collect_boundary_block(block, &mut boundary);
    boundary
}

fn collect_boundary_block(block: &Block, boundary: &mut BoundarySig) {
    for statement in &block.statements {
        collect_boundary_stmt(statement, boundary);
    }
}

fn collect_boundary_stmt(statement: &Stmt, boundary: &mut BoundarySig) {
    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local {
                boundary.local_bindings.insert(stmt.name.clone());
            }
            if let Some(value) = &stmt.value {
                collect_boundary_expr(value, boundary);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_boundary_expr(value, boundary);
            }
        }
        Stmt::With(stmt) => {
            collect_boundary_expr(&stmt.resource, boundary);
            collect_boundary_block(&stmt.body, boundary);
        }
        Stmt::If(stmt) => {
            collect_boundary_expr(&stmt.condition, boundary);
            collect_boundary_block(&stmt.then_body, boundary);
            if let Some(else_body) = &stmt.else_body {
                collect_boundary_block(else_body, boundary);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_boundary_expr(condition, boundary);
            }
            collect_boundary_block(&stmt.body, boundary);
        }
        Stmt::Expr(expr) => collect_boundary_expr(expr, boundary),
        Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
    }
}

fn collect_boundary_expr(expr: &Expr, boundary: &mut BoundarySig) {
    match expr {
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            ..
        } => {
            boundary.take_count += 1;
            collect_boundary_expr(value, boundary);
        }
        Expr::Effect { value, .. } => collect_boundary_expr(value, boundary),
        Expr::Manage { value, .. } => {
            boundary.manage_count += 1;
            collect_boundary_expr(value, boundary);
        }
        Expr::Call { args, .. } => {
            for CallArg { value, .. } in args {
                collect_boundary_expr(value, boundary);
            }
        }
        Expr::Field { base, .. } => collect_boundary_expr(base, boundary),
        Expr::Closure { body, .. } => collect_boundary_block(body, boundary),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}
