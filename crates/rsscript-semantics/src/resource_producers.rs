//! HIR resource-producer classification and boundary diagnostics.

use crate::{
    hir::{Hir, HirExpr, HirTypeKind},
    resource_producer_escape_diagnostic, resource_producer_missing_try_diagnostic,
};
use rsscript_diagnostics::{Diagnostic, Span};

/// The resolved shape of a resource-producing expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceProducerKind {
    Resource,
    ResultResource { ok_type: String },
}

/// Classify an expression which creates a resource or a `Result` whose `Ok`
/// value is a resource. This is a semantic HIR query; callers merely provide
/// the expression's lexical context.
pub fn resource_producer_kind(hir: &Hir, expr: &HirExpr) -> Option<ResourceProducerKind> {
    match expr {
        HirExpr::Call { .. } => {
            if expression_type_is_resource(hir, expr) {
                Some(ResourceProducerKind::Resource)
            } else {
                result_resource_ok_type(hir, expr)
                    .map(|ok_type| ResourceProducerKind::ResultResource { ok_type })
            }
        }
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. }
            if expression_type_is_resource(hir, expr) =>
        {
            resource_producer_kind(hir, value)
        }
        _ => None,
    }
}

/// Report a resource producer used outside a resource-owning context.
pub fn resource_producer_context_diagnostic(
    hir: &Hir,
    expr: &HirExpr,
    allowed_resource_context: bool,
) -> Option<Diagnostic> {
    (!allowed_resource_context && resource_producer_kind(hir, expr).is_some()).then(|| {
        resource_producer_escape_diagnostic(
            hir_expr_type_name(expr).unwrap_or("resource"),
            hir_expr_span(expr).clone(),
        )
    })
}

/// Report the missing `?` at a `with` boundary for `Result<Resource, E>`.
pub fn result_resource_with_try_diagnostic(hir: &Hir, expr: &HirExpr) -> Option<Diagnostic> {
    if matches!(expr, HirExpr::Try { .. }) {
        return None;
    }
    let ResourceProducerKind::ResultResource { ok_type } = resource_producer_kind(hir, expr)?
    else {
        return None;
    };
    Some(resource_producer_missing_try_diagnostic(
        &ok_type,
        hir_expr_span(expr).clone(),
    ))
}

fn expression_type_is_resource(hir: &Hir, expr: &HirExpr) -> bool {
    hir_expr_type_name(expr)
        .is_some_and(|type_name| hir.type_kind(type_name) == Some(HirTypeKind::Resource))
}

fn result_resource_ok_type(hir: &Hir, expr: &HirExpr) -> Option<String> {
    let type_name = hir_expr_type_name(expr)?;
    let inner = type_name.strip_prefix("Result<")?.strip_suffix('>')?;
    let ok_type = split_top_level_type_args(inner).first()?.to_string();
    (hir.type_kind(&ok_type) == Some(HirTypeKind::Resource)).then_some(ok_type)
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

fn hir_expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::Char { span, .. }
        | HirExpr::ObjectLiteral { span, .. }
        | HirExpr::MapLiteral { span, .. }
        | HirExpr::ArrayLiteral { span, .. }
        | HirExpr::Binary { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Effect { span, .. }
        | HirExpr::Manage { span, .. }
        | HirExpr::Spawn { span, .. }
        | HirExpr::Await { span, .. }
        | HirExpr::Try { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Match { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}
