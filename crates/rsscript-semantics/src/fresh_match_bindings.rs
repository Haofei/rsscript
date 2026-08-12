//! HIR facts for the single-payload fresh `Some`/`Ok` match binding sugar.

use crate::hir::{CallResolution, HirExpr, HirMatchArm};
use rsscript_syntax::ast::MatchPattern;

/// A local binding introduced by matching a fresh `Option`/`Result` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshMatchBinding {
    pub name: String,
    pub payload_type_name: String,
    pub source_ident: Option<String>,
    pub fresh_from_scrutinee: bool,
}

/// Derive the fresh-payload binding fact for one match arm, if the resolved HIR
/// establishes the narrow single-payload `Some`/`Ok` contract.
pub fn fresh_match_binding(value: &HirExpr, arm: &HirMatchArm) -> Option<FreshMatchBinding> {
    let value_type = hir_expr_type_name(value)?;
    let source_ident = hir_expr_ident_name(value).map(str::to_owned);
    let fresh_from_scrutinee = is_fresh_match_scrutinee(value);
    if source_ident.is_none() && !fresh_from_scrutinee {
        return None;
    }
    let MatchPattern::Variant { name, bindings, .. } = &arm.pattern else {
        return None;
    };
    let [MatchPattern::Binding { name: binding, .. }] = bindings.as_slice() else {
        return None;
    };
    let payload_type_name = fresh_payload_type_for_variant(value_type, name)?.to_owned();
    Some(FreshMatchBinding {
        name: binding.clone(),
        payload_type_name,
        source_ident,
        fresh_from_scrutinee,
    })
}

fn fresh_payload_type_for_variant<'a>(value_type: &'a str, variant: &str) -> Option<&'a str> {
    let inner = value_type
        .trim()
        .strip_prefix("Option<")
        .and_then(|rest| rest.strip_suffix('>'));
    if variant == "Some" {
        return inner?.trim().strip_prefix("fresh ").map(str::trim);
    }
    let inner = value_type
        .trim()
        .strip_prefix("Result<")
        .and_then(|rest| rest.strip_suffix('>'))?;
    let payload = split_top_level_type_args(inner).first()?.trim();
    (variant == "Ok")
        .then(|| payload.strip_prefix("fresh ").map(str::trim))
        .flatten()
}

fn split_top_level_type_args(value: &str) -> Vec<&str> {
    let mut args = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in value.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                args.push(value[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    args.push(value[start..].trim());
    args
}

fn is_fresh_match_scrutinee(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Call { resolution, .. } => {
            matches!(resolution, CallResolution::Resolved { signature, .. } if signature.returns_fresh)
        }
        HirExpr::Try { value, .. } => is_fresh_match_scrutinee(value),
        _ => false,
    }
}

fn hir_expr_ident_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => hir_expr_ident_name(value),
        _ => None,
    }
}

fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { type_name, .. }
        | HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}
