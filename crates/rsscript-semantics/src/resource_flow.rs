//! Backend-neutral HIR facts for lexical resource escape and capture checks.

use crate::hir::{HirBlock, HirCallArg, HirEffectEventKind, HirExpr, HirStmt, ParamEffect};
use crate::{ResourceEscape, ResourceEscapeKind, hir::HirBindingKind, hir_block_identifier_uses};
use rsscript_syntax::{Span, ast::Callee};
use std::collections::HashMap;

/// Index resource escape and capture facts by the enclosing `with` statement.
pub fn resource_escapes_by_with_statement(block: &HirBlock) -> HashMap<Span, Vec<ResourceEscape>> {
    let mut escapes = HashMap::new();
    collect_block_resource_escapes(block, &mut escapes);
    escapes
}

fn collect_block_resource_escapes(
    block: &HirBlock,
    escapes_by_with_span: &mut HashMap<Span, Vec<ResourceEscape>>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::With {
                binding,
                body,
                span,
                ..
            } => {
                let mut escapes = Vec::new();
                collect_resource_escapes_in_block(binding, body, &mut escapes);
                escapes_by_with_span.insert(span.clone(), escapes);
                collect_block_resource_escapes(body, escapes_by_with_span);
            }
            HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                collect_expr_resource_escapes(value, escapes_by_with_span)
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_expr_resource_escapes(condition, escapes_by_with_span);
                collect_block_resource_escapes(then_body, escapes_by_with_span);
                if let Some(else_body) = else_body {
                    collect_block_resource_escapes(else_body, escapes_by_with_span);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_expr_resource_escapes(condition, escapes_by_with_span);
                }
                collect_block_resource_escapes(body, escapes_by_with_span);
            }
            HirStmt::For { iterable, body, .. } => {
                collect_expr_resource_escapes(iterable, escapes_by_with_span);
                collect_block_resource_escapes(body, escapes_by_with_span);
            }
            HirStmt::Match { value, arms, .. } => {
                collect_expr_resource_escapes(value, escapes_by_with_span);
                for arm in arms {
                    collect_block_resource_escapes(&arm.body, escapes_by_with_span);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_expr_resource_escapes(&arm.operation, escapes_by_with_span);
                    collect_block_resource_escapes(&arm.body, escapes_by_with_span);
                }
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn collect_expr_resource_escapes(
    expr: &HirExpr,
    escapes_by_with_span: &mut HashMap<Span, Vec<ResourceEscape>>,
) {
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_resource_escapes(&arg.value, escapes_by_with_span);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_expr_resource_escapes(value, escapes_by_with_span),
        HirExpr::Binary { left, right, .. } => {
            collect_expr_resource_escapes(left, escapes_by_with_span);
            collect_expr_resource_escapes(right, escapes_by_with_span);
        }
        HirExpr::Field { base, .. } => collect_expr_resource_escapes(base, escapes_by_with_span),
        HirExpr::Index { base, index, .. } => {
            collect_expr_resource_escapes(base, escapes_by_with_span);
            collect_expr_resource_escapes(index, escapes_by_with_span);
        }
        HirExpr::Closure { body, .. } => collect_block_resource_escapes(body, escapes_by_with_span),
        HirExpr::Match { value, arms, .. } => {
            collect_expr_resource_escapes(value, escapes_by_with_span);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_resource_escapes(guard, escapes_by_with_span);
                }
                collect_block_resource_escapes(&arm.body, escapes_by_with_span);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_resource_escapes(&entry.key, escapes_by_with_span);
                collect_expr_resource_escapes(&entry.value, escapes_by_with_span);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_resource_escapes(&field.value, escapes_by_with_span);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_resource_escapes(item, escapes_by_with_span);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn collect_resource_escapes_in_block(
    binding: &str,
    block: &HirBlock,
    escapes: &mut Vec<ResourceEscape>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Return {
                value: Some(value), ..
            } if let Some(span) = resource_escape_operand_span(value, binding) => {
                push_resource_escape(escapes, binding, ResourceEscapeKind::Escape, span);
            }
            HirStmt::Let {
                kind: HirBindingKind::ManagedLet,
                value: Some(value),
                ..
            } if let Some(span) = resource_escape_operand_span(value, binding) => {
                push_resource_escape(escapes, binding, ResourceEscapeKind::Escape, span);
            }
            HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                collect_resource_escapes_in_expr(binding, value, escapes)
            }
            HirStmt::With { body, .. } => collect_resource_escapes_in_block(binding, body, escapes),
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_resource_escapes_in_expr(binding, condition, escapes);
                collect_resource_escapes_in_block(binding, then_body, escapes);
                if let Some(else_body) = else_body {
                    collect_resource_escapes_in_block(binding, else_body, escapes);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_resource_escapes_in_expr(binding, condition, escapes);
                }
                collect_resource_escapes_in_block(binding, body, escapes);
            }
            HirStmt::For { iterable, body, .. } => {
                collect_resource_escapes_in_expr(binding, iterable, escapes);
                collect_resource_escapes_in_block(binding, body, escapes);
            }
            HirStmt::Match { value, arms, .. } => {
                collect_resource_escapes_in_expr(binding, value, escapes);
                for arm in arms {
                    collect_resource_escapes_in_block(binding, &arm.body, escapes);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_resource_escapes_in_expr(binding, &arm.operation, escapes);
                    collect_resource_escapes_in_block(binding, &arm.body, escapes);
                }
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }

        if let HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            value: Some(value),
            ..
        } = statement
            && let Some(span) = managed_binding_resource_capture_span(value, binding)
        {
            push_resource_escape(escapes, binding, ResourceEscapeKind::Capture, span);
        }
    }
}

fn managed_binding_resource_capture_span(expr: &HirExpr, binding: &str) -> Option<Span> {
    match expr {
        HirExpr::Closure { body, span, .. }
            if hir_block_identifier_uses(body)
                .iter()
                .any(|(name, _)| name == binding) =>
        {
            Some(span.clone())
        }
        HirExpr::Effect {
            effect: ParamEffect::Read | ParamEffect::Mut,
            value,
            ..
        } => managed_binding_resource_capture_span(value, binding),
        HirExpr::Call { callee, args, .. } if resource_escape_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| managed_binding_resource_capture_span(&arg.value, binding)),
        _ => None,
    }
}

fn collect_resource_escapes_in_expr(
    binding: &str,
    expr: &HirExpr,
    escapes: &mut Vec<ResourceEscape>,
) {
    match expr {
        HirExpr::Manage { value, span, .. } => {
            if resource_escape_operand_span(value, binding).is_some() {
                push_resource_escape(escapes, binding, ResourceEscapeKind::Escape, span.clone());
            }
            collect_resource_escapes_in_expr(binding, value, escapes);
        }
        HirExpr::Call {
            callee,
            args,
            events,
            ..
        } if tempdir_keep_consumes_binding(callee, args, binding) => {
            for arg in args {
                if !take_ident_effect_expr(&arg.value, binding) {
                    collect_resource_escapes_in_expr(binding, &arg.value, escapes);
                }
            }
        }
        HirExpr::Call { args, events, .. } => {
            for event in events {
                if matches!(event.kind, HirEffectEventKind::Retain { .. })
                    && event.binding_name == binding
                {
                    push_resource_escape(
                        escapes,
                        binding,
                        ResourceEscapeKind::Escape,
                        event.value_span.clone(),
                    );
                }
            }
            for arg in args {
                collect_resource_escapes_in_expr(binding, &arg.value, escapes);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_resource_escapes_in_expr(binding, value, escapes),
        HirExpr::Binary { left, right, .. } => {
            collect_resource_escapes_in_expr(binding, left, escapes);
            collect_resource_escapes_in_expr(binding, right, escapes);
        }
        HirExpr::Field { base, .. } => collect_resource_escapes_in_expr(binding, base, escapes),
        HirExpr::Index { base, index, .. } => {
            collect_resource_escapes_in_expr(binding, base, escapes);
            collect_resource_escapes_in_expr(binding, index, escapes);
        }
        HirExpr::Closure { body, .. } => collect_resource_escapes_in_block(binding, body, escapes),
        HirExpr::Match { value, arms, .. } => {
            collect_resource_escapes_in_expr(binding, value, escapes);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_resource_escapes_in_expr(binding, guard, escapes);
                }
                collect_resource_escapes_in_block(binding, &arm.body, escapes);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_resource_escapes_in_expr(binding, &entry.key, escapes);
                collect_resource_escapes_in_expr(binding, &entry.value, escapes);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_resource_escapes_in_expr(binding, &field.value, escapes);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_resource_escapes_in_expr(binding, item, escapes);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn resource_escape_operand_span(expr: &HirExpr, binding: &str) -> Option<Span> {
    match expr {
        HirExpr::Ident { name, span, .. } if name == binding => Some(span.clone()),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            resource_escape_operand_span(value, binding)
        }
        HirExpr::Call { callee, args, .. } if resource_escape_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| resource_escape_operand_span(&arg.value, binding)),
        _ => None,
    }
}

fn tempdir_keep_consumes_binding(callee: &Callee, args: &[HirCallArg], binding: &str) -> bool {
    (matches!(callee, Callee::Qualified { namespace, name } if namespace == "TempDir" && name == "keep")
        || matches!(callee, Callee::Name(name) if name == "TempDir.keep"))
        && args.iter().any(|arg| {
            arg.name.as_deref().unwrap_or("dir") == "dir"
                && take_ident_effect_expr(&arg.value, binding)
        })
}

fn take_ident_effect_expr(expr: &HirExpr, binding: &str) -> bool {
    matches!(
        expr,
        HirExpr::Effect {
            effect: ParamEffect::Take,
            value,
            ..
        } if matches!(value.as_ref(), HirExpr::Ident { name, .. } if name == binding)
    )
}

fn resource_escape_wrapper_callee(callee: &Callee) -> bool {
    matches!(callee, Callee::Name(name) if matches!(name.as_str(), "Ok" | "Err" | "Some"))
}

fn push_resource_escape(
    escapes: &mut Vec<ResourceEscape>,
    binding: &str,
    kind: ResourceEscapeKind,
    span: Span,
) {
    let escape = ResourceEscape {
        binding: binding.to_string(),
        kind,
        span,
    };
    if !escapes.contains(&escape) {
        escapes.push(escape);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::Hir;
    use rsscript_syntax::parse_source;

    #[test]
    fn indexes_returned_resource_escape_by_with_span() {
        let program = parse_source(
            "resource-flow.rss",
            r#"
resource File { fd: Int }
fn File.open() -> File
fn main() -> File {
    with File.open() as file {
        return file
    }
}
"#,
        );
        let hir = Hir::from_syntax(&program);
        let body = hir
            .function_body("main")
            .and_then(|body| body.block.as_ref())
            .unwrap();
        let HirStmt::With { span, .. } = &body.statements[0] else {
            panic!("expected a with statement");
        };

        assert!(matches!(
            resource_escapes_by_with_statement(body).get(span).unwrap().as_slice(),
            [ResourceEscape { binding, kind: ResourceEscapeKind::Escape, .. }]
                if binding == "file"
        ));
    }
}
