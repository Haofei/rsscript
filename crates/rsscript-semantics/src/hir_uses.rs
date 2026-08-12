//! Backend-neutral HIR identifier-use queries.

use crate::hir::{
    HirBlock, HirEffectEvent, HirEffectEventKind, HirExpr, HirStmt, assign_target_reads,
};
use rsscript_syntax::Span;
use rsscript_syntax::ast::DataEffect;

/// Return the identifier reads reachable from a HIR block in source order.
/// This deliberately follows nested statement bodies but leaves a `match`
/// expression's arms to its enclosing statement traversal, matching the HIR
/// control-flow boundary.
pub fn hir_block_identifier_uses(block: &HirBlock) -> Vec<(String, Span)> {
    let mut uses = Vec::new();
    collect_block_identifier_uses(block, &mut uses);
    uses
}

/// Return the identifier reads directly represented by one statement.
pub fn hir_stmt_identifier_uses(statement: &HirStmt) -> Vec<(String, Span)> {
    let mut uses = Vec::new();
    collect_stmt_identifier_uses(statement, &mut uses);
    uses
}

/// Return the resolved effect events directly evaluated by one statement.
/// Nested statement bodies remain CFG edges; `match` expression arms are
/// included because they are part of the expression's evaluation shape.
pub fn hir_stmt_effect_events(statement: &HirStmt) -> Vec<HirEffectEvent> {
    let mut events = Vec::new();
    collect_stmt_effect_events(statement, &mut events);
    events
}

/// Return the canonical HIR place path for an identifier or field chain.
pub fn hir_expr_path(expr: &HirExpr) -> Option<(String, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } => Some((name.clone(), span.clone())),
        HirExpr::Field {
            base, name, span, ..
        } => {
            let (mut base_path, _) = hir_expr_path(base)?;
            base_path.push('.');
            base_path.push_str(name);
            Some((base_path, span.clone()))
        }
        _ => None,
    }
}

fn collect_block_identifier_uses(block: &HirBlock, uses: &mut Vec<(String, Span)>) {
    for statement in &block.statements {
        collect_stmt_identifier_uses(statement, uses);
        match statement {
            HirStmt::With { body, .. } => collect_block_identifier_uses(body, uses),
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_block_identifier_uses(then_body, uses);
                if let Some(else_body) = else_body {
                    collect_block_identifier_uses(else_body, uses);
                }
            }
            HirStmt::Loop { body, .. } => collect_block_identifier_uses(body, uses),
            HirStmt::For { body, .. } => collect_block_identifier_uses(body, uses),
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_block_identifier_uses(&arm.body, uses);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_block_identifier_uses(&arm.body, uses);
                }
            }
            HirStmt::Let { .. }
            | HirStmt::Return { .. }
            | HirStmt::Expr(_)
            | HirStmt::Assign { .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn collect_stmt_identifier_uses(statement: &HirStmt, uses: &mut Vec<(String, Span)>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_expr_identifier_uses(value, uses),
        HirStmt::Assign { target, value, .. } => {
            for read in assign_target_reads(target) {
                collect_expr_identifier_uses(read, uses);
            }
            collect_expr_identifier_uses(value, uses);
        }
        HirStmt::With { resource, .. } => collect_expr_identifier_uses(resource, uses),
        HirStmt::If { condition, .. } => collect_expr_identifier_uses(condition, uses),
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => collect_expr_identifier_uses(condition, uses),
        HirStmt::For { iterable, .. } => collect_expr_identifier_uses(iterable, uses),
        HirStmt::Match { value, .. } => collect_expr_identifier_uses(value, uses),
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_identifier_uses(&arm.operation, uses);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Loop {
            condition: None, ..
        }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_identifier_uses(expr: &HirExpr, uses: &mut Vec<(String, Span)>) {
    match expr {
        HirExpr::Ident { name, span, .. } => uses.push((name.clone(), span.clone())),
        HirExpr::Field { base, .. } => collect_expr_identifier_uses(base, uses),
        HirExpr::Index { base, index, .. } => {
            collect_expr_identifier_uses(base, uses);
            collect_expr_identifier_uses(index, uses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_identifier_uses(&arg.value, uses);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_identifier_uses(&field.value, uses);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_identifier_uses(&entry.key, uses);
                collect_expr_identifier_uses(&entry.value, uses);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_identifier_uses(item, uses);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_expr_identifier_uses(value, uses),
        HirExpr::Binary { left, right, .. } => {
            collect_expr_identifier_uses(left, uses);
            collect_expr_identifier_uses(right, uses);
        }
        HirExpr::Closure { body, .. } => collect_block_identifier_uses(body, uses),
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Match { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn collect_stmt_effect_events(statement: &HirStmt, events: &mut Vec<HirEffectEvent>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_expr_effect_events(value, events),
        HirStmt::Assign { target, value, .. } => {
            for read in assign_target_reads(target) {
                collect_expr_effect_events(read, events);
            }
            collect_expr_effect_events(value, events);
        }
        HirStmt::With { resource, .. } => collect_expr_effect_events(resource, events),
        HirStmt::If { condition, .. } => collect_expr_effect_events(condition, events),
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => collect_expr_effect_events(condition, events),
        HirStmt::For { iterable, .. } => collect_expr_effect_events(iterable, events),
        HirStmt::Match {
            value,
            scrutinee_effect,
            ..
        } => {
            collect_expr_effect_events(value, events);
            push_match_take_event(*scrutinee_effect, value, events);
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_effect_events(&arm.operation, events);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Loop {
            condition: None, ..
        }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_effect_events(expr: &HirExpr, events: &mut Vec<HirEffectEvent>) {
    match expr {
        HirExpr::Call {
            args,
            events: expr_events,
            ..
        } => {
            events.extend(expr_events.iter().cloned());
            for arg in args {
                collect_expr_effect_events(&arg.value, events);
            }
        }
        HirExpr::Effect {
            value,
            events: expr_events,
            ..
        }
        | HirExpr::Manage {
            value,
            events: expr_events,
            ..
        } => {
            events.extend(expr_events.iter().cloned());
            collect_expr_effect_events(value, events);
        }
        HirExpr::Spawn { value, .. } | HirExpr::Await { value, .. } => {
            collect_expr_effect_events(value, events)
        }
        HirExpr::Try { value, .. } => collect_expr_effect_events(value, events),
        HirExpr::Binary { left, right, .. } => {
            collect_expr_effect_events(left, events);
            collect_expr_effect_events(right, events);
        }
        HirExpr::Field { base, .. } => collect_expr_effect_events(base, events),
        HirExpr::Index { base, index, .. } => {
            collect_expr_effect_events(base, events);
            collect_expr_effect_events(index, events);
        }
        HirExpr::Match {
            value,
            scrutinee_effect,
            arms,
            ..
        } => {
            collect_expr_effect_events(value, events);
            push_match_take_event(*scrutinee_effect, value, events);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_effect_events(guard, events);
                }
                for statement in &arm.body.statements {
                    collect_stmt_effect_events(statement, events);
                }
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_effect_events(&entry.key, events);
                collect_expr_effect_events(&entry.value, events);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_effect_events(&field.value, events);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_effect_events(item, events);
            }
        }
        HirExpr::Closure { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn push_match_take_event(
    scrutinee_effect: Option<DataEffect>,
    value: &HirExpr,
    events: &mut Vec<HirEffectEvent>,
) {
    if scrutinee_effect == Some(DataEffect::Take)
        && let Some((binding_name, span)) = hir_expr_path(value)
    {
        events.push(HirEffectEvent {
            function_name: String::new(),
            kind: HirEffectEventKind::Take,
            binding_name,
            span: span.clone(),
            value_span: span,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::Hir;
    use rsscript_syntax::parse_source;

    #[test]
    fn collects_nested_statement_and_assignment_reads_in_source_order() {
        let program = parse_source(
            "uses.rss",
            r#"
fn main(a: Int, b: Int, c: Int) -> Unit {
    let x = a
    if b { x = c }
}
"#,
        );
        let hir = Hir::from_syntax(&program);
        let block = hir
            .function_body("main")
            .and_then(|body| body.block.as_ref())
            .unwrap();

        assert_eq!(
            hir_block_identifier_uses(block)
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>(),
            ["a", "b", "c"]
        );
    }

    #[test]
    fn collects_resolved_effect_events_from_a_statement() {
        let program = parse_source(
            "effects.rss",
            r#"
fn consume(value: take Int) -> Unit
fn main(value: Int) -> Unit {
    consume(value: take value)
}
"#,
        );
        let hir = Hir::from_syntax(&program);
        let statement = &hir
            .function_body("main")
            .and_then(|body| body.block.as_ref())
            .unwrap()
            .statements[0];

        assert!(matches!(
            hir_stmt_effect_events(statement).as_slice(),
            [HirEffectEvent {
                kind: HirEffectEventKind::Take,
                binding_name,
                ..
            }] if binding_name == "value"
        ));
    }
}
