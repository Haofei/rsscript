//! HIR projections used when a local binding enters ownership flow.

use crate::hir::{CallResolution, HirExpr, HirTypeKind, ParamEffect, ResolvedCalleeKind};
use rsscript_syntax::{Span, ast::Callee};

/// Ownership-relevant facts about a `let` initializer, independent of CFG
/// state and therefore reusable by every semantic consumer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalBindingValueFacts {
    pub source_ident: Option<(String, Span)>,
    pub handle_field_source: Option<(String, Span)>,
    pub is_fresh_value: bool,
}

/// Extract ownership-relevant facts from a local binding initializer.
pub fn local_binding_value_facts(value: &HirExpr) -> LocalBindingValueFacts {
    LocalBindingValueFacts {
        source_ident: local_binding_source_ident(value),
        handle_field_source: local_binding_handle_field_source(value),
        is_fresh_value: hir_expr_is_fresh_value(value),
    }
}

fn hir_expr_is_fresh_value(value: &HirExpr) -> bool {
    match value {
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ArrayLiteral { .. } => true,
        HirExpr::Call { resolution, .. } => match resolution {
            CallResolution::EnumVariant => true,
            CallResolution::Resolved {
                kind:
                    ResolvedCalleeKind::Constructor {
                        type_kind: HirTypeKind::Struct,
                    },
                ..
            } => true,
            CallResolution::Resolved { signature, .. } => signature.returns_fresh,
            _ => false,
        },
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => {
            hir_expr_is_fresh_value(value)
        }
        _ => false,
    }
}

fn local_binding_source_ident(value: &HirExpr) -> Option<(String, Span)> {
    match value {
        HirExpr::Ident { name, span, .. } => Some((name.clone(), span.clone())),
        HirExpr::Effect {
            effect: ParamEffect::Read | ParamEffect::Mut,
            value,
            ..
        } => local_binding_source_ident(value),
        HirExpr::Call { callee, args, .. } if local_binding_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| local_binding_source_ident(&arg.value)),
        _ => None,
    }
}

fn local_binding_handle_field_source(value: &HirExpr) -> Option<(String, Span)> {
    match value {
        HirExpr::Field { base, access, .. } if access.is_handle => {
            crate::hir_expr_path(base).map(|(mut path, _)| {
                path.push('.');
                path.push_str(&access.name);
                (path, access.span.clone())
            })
        }
        HirExpr::Field { base, .. } => local_binding_handle_field_source(base),
        HirExpr::Effect {
            effect: ParamEffect::Read | ParamEffect::Mut,
            value,
            ..
        } => local_binding_handle_field_source(value),
        HirExpr::Call { callee, args, .. } if local_binding_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| local_binding_handle_field_source(&arg.value)),
        _ => None,
    }
}

fn local_binding_wrapper_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Name(name) if matches!(name.as_str(), "Ok" | "Err" | "Some"))
}
