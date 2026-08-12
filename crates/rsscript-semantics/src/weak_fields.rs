//! Weak-handle use rules over checked HIR.

use crate::hir::{HirBlock, HirExpr, HirFieldAccess, HirStmt};
use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::Callee;

/// Returns whether this call is the explicit weak-handle upgrade operation.
pub fn is_weak_upgrade_call(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if namespace == "Weak" && name.split_once('<').map_or(name.as_str(), |(root, _)| root) == "upgrade"
    )
}

/// Diagnose a weak field used without an explicit `Weak.upgrade` boundary.
pub fn weak_field_upgrade_diagnostic(value: &HirExpr) -> Option<Diagnostic> {
    let access = weak_field_access_requiring_upgrade(value)?;
    Some(
        Diagnostic::error(
            code::WEAK_FIELD_REQUIRES_UPGRADE,
            format!(
                "weak field `{}` must be upgraded before it is used as a value.",
                access.name
            ),
            access.span.clone(),
            "weak field requires upgrade",
        )
        .with_cause("A weak field is a non-owning handle and may no longer point to a live value.")
        .with_fix(
            "upgrade_weak_field",
            format!(
                "Use `Weak.upgrade(value: read {})` and handle `None`.",
                access.name
            ),
            "manual",
        ),
    )
}

fn weak_field_access_requiring_upgrade(expr: &HirExpr) -> Option<&HirFieldAccess> {
    match expr {
        HirExpr::Field { base, access, .. } => access
            .is_weak
            .then_some(access)
            .or_else(|| weak_field_access_requiring_upgrade(base)),
        HirExpr::Call { callee, args, .. } if is_weak_upgrade_call(callee) => {
            for argument in args {
                if let HirExpr::Effect { value, .. } = &argument.value
                    && weak_field_access_requiring_upgrade(value).is_some()
                {
                    return None;
                }
            }
            args.iter()
                .find_map(|argument| weak_field_access_requiring_upgrade(&argument.value))
        }
        HirExpr::Call { args, .. } => args
            .iter()
            .find_map(|argument| weak_field_access_requiring_upgrade(&argument.value)),
        HirExpr::Effect { value, .. }
        | HirExpr::Try { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. } => weak_field_access_requiring_upgrade(value),
        HirExpr::Index { base, index, .. } => weak_field_access_requiring_upgrade(base)
            .or_else(|| weak_field_access_requiring_upgrade(index)),
        HirExpr::Binary { left, right, .. } => weak_field_access_requiring_upgrade(left)
            .or_else(|| weak_field_access_requiring_upgrade(right)),
        HirExpr::Closure { body, .. } => weak_field_access_in_block(body),
        HirExpr::Match { value, arms, .. } => {
            weak_field_access_requiring_upgrade(value).or_else(|| {
                arms.iter()
                    .find_map(|arm| weak_field_access_in_block(&arm.body))
            })
        }
        HirExpr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| weak_field_access_requiring_upgrade(&entry.key))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| weak_field_access_requiring_upgrade(&entry.value))
            }),
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn weak_field_access_in_block(block: &HirBlock) -> Option<&HirFieldAccess> {
    block
        .statements
        .iter()
        .find_map(weak_field_access_requiring_upgrade_in_statement)
}

fn weak_field_access_requiring_upgrade_in_statement(
    statement: &HirStmt,
) -> Option<&HirFieldAccess> {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => weak_field_access_requiring_upgrade(value),
        HirStmt::With { resource, body, .. } => weak_field_access_requiring_upgrade(resource)
            .or_else(|| weak_field_access_in_block(body)),
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => weak_field_access_requiring_upgrade(condition)
            .or_else(|| weak_field_access_in_block(then_body))
            .or_else(|| else_body.as_ref().and_then(weak_field_access_in_block)),
        HirStmt::Loop {
            condition, body, ..
        } => condition
            .as_ref()
            .and_then(weak_field_access_requiring_upgrade)
            .or_else(|| weak_field_access_in_block(body)),
        HirStmt::For { iterable, body, .. } => weak_field_access_requiring_upgrade(iterable)
            .or_else(|| weak_field_access_in_block(body)),
        HirStmt::Match { value, arms, .. } => {
            weak_field_access_requiring_upgrade(value).or_else(|| {
                arms.iter()
                    .find_map(|arm| weak_field_access_in_block(&arm.body))
            })
        }
        HirStmt::Select { arms, .. } => arms.iter().find_map(|arm| {
            weak_field_access_requiring_upgrade(&arm.operation)
                .or_else(|| weak_field_access_in_block(&arm.body))
        }),
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_diagnostics::Span;

    fn span() -> Span {
        Span {
            file: "weak.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    fn weak_field() -> HirExpr {
        HirExpr::Field {
            base: Box::new(HirExpr::Ident {
                name: "owner".to_owned(),
                type_name: None,
                span: span(),
            }),
            name: "next".to_owned(),
            access: HirFieldAccess {
                function_name: "main".to_owned(),
                name: "next".to_owned(),
                span: span(),
                base_ty: None,
                ty: None,
                base_type: None,
                type_name: None,
                is_handle: true,
                is_weak: true,
            },
            span: span(),
        }
    }

    #[test]
    fn weak_field_use_requires_an_explicit_upgrade() {
        let diagnostic = weak_field_upgrade_diagnostic(&weak_field())
            .expect("a weak field cannot be used directly");
        assert_eq!(diagnostic.code, code::WEAK_FIELD_REQUIRES_UPGRADE);
        assert!(diagnostic.summary.contains("weak field `next`"));
    }

    #[test]
    fn recognizes_only_the_explicit_weak_upgrade_operation() {
        assert!(is_weak_upgrade_call(&Callee::Qualified {
            namespace: "Weak".to_owned(),
            name: "upgrade<T>".to_owned(),
        }));
        assert!(!is_weak_upgrade_call(&Callee::Qualified {
            namespace: "Other".to_owned(),
            name: "upgrade".to_owned(),
        }));
    }
}
