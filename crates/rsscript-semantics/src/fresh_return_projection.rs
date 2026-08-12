//! HIR projections shared by fresh-return ownership analysis.

use crate::hir::HirExpr;
use rsscript_syntax::{Span, ast::Callee};

/// Return the local binding at the base of a non-handle field projection that
/// can preserve a `fresh` return proof.
pub fn fresh_field_access_base(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Field { base, access, .. } if !access.is_handle && !access.is_weak => {
            fresh_field_access_base(base)
        }
        HirExpr::Ident { name, .. } => Some(name),
        HirExpr::Call { callee, args, .. } if fresh_wrapper_callee(callee) => args
            .first()
            .and_then(|arg| fresh_field_access_base(&arg.value)),
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => fresh_field_access_base(value),
        HirExpr::Manage { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Match { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

/// Return the display path of a handle or weak field nested in an expression
/// that invalidates a `fresh` return proof.
pub fn fresh_handle_or_weak_field_path(expr: &HirExpr) -> Option<String> {
    match expr {
        HirExpr::Field {
            base, name, access, ..
        } if access.is_handle || access.is_weak => {
            let base = fresh_expr_path(base).unwrap_or_else(|| "<expr>".to_string());
            Some(format!("{base}.{name}"))
        }
        HirExpr::Field { base, .. } => fresh_handle_or_weak_field_path(base),
        HirExpr::Call { callee, args, .. } if fresh_wrapper_callee(callee) => args
            .first()
            .and_then(|arg| fresh_handle_or_weak_field_path(&arg.value)),
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => fresh_handle_or_weak_field_path(value),
        HirExpr::Ident { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Match { .. }
        | HirExpr::Manage { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

/// Return the outermost value span after erasing `effect` and `manage`
/// wrappers, matching the user-facing operand for a fresh-return diagnostic.
pub fn fresh_return_value_span(value: Option<&HirExpr>) -> Option<&Span> {
    let mut value = value?;
    loop {
        match value {
            HirExpr::Effect { value: inner, .. } | HirExpr::Manage { value: inner, .. } => {
                value = inner;
            }
            _ => return Some(hir_expr_span(value)),
        }
    }
}

fn fresh_expr_path(expr: &HirExpr) -> Option<String> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name.clone()),
        HirExpr::Field { base, name, .. } => {
            fresh_expr_path(base).map(|base| format!("{base}.{name}"))
        }
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => fresh_expr_path(value),
        _ => None,
    }
}

fn fresh_wrapper_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Name(name) if matches!(name.as_str(), "Ok" | "Some"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::{HirFieldAccess, ParamEffect};

    fn span(column: usize) -> Span {
        Span {
            file: "fresh.rss".to_owned(),
            line: 1,
            column,
            length: 1,
        }
    }

    fn handle_field() -> HirExpr {
        HirExpr::Field {
            base: Box::new(HirExpr::Ident {
                name: "owner".to_owned(),
                type_name: None,
                span: span(1),
            }),
            name: "child".to_owned(),
            access: HirFieldAccess {
                function_name: "main".to_owned(),
                name: "child".to_owned(),
                span: span(1),
                base_ty: None,
                ty: None,
                base_type: None,
                type_name: None,
                is_handle: true,
                is_weak: false,
            },
            span: span(7),
        }
    }

    #[test]
    fn distinguishes_value_fields_from_handle_fields_and_erases_return_wrappers() {
        assert_eq!(fresh_field_access_base(&handle_field()), None);
        assert_eq!(
            fresh_handle_or_weak_field_path(&handle_field()).as_deref(),
            Some("owner.child")
        );

        let wrapped = HirExpr::Effect {
            effect: ParamEffect::Read,
            value: Box::new(HirExpr::Manage {
                value: Box::new(handle_field()),
                events: Vec::new(),
                type_name: None,
                span: span(6),
            }),
            events: Vec::new(),
            type_name: None,
            span: span(5),
        };
        assert_eq!(fresh_return_value_span(Some(&wrapped)), Some(&span(7)));
    }
}
