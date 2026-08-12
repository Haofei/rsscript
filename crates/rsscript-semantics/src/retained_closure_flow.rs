//! Flow-sensitive facts for locals captured by retained closures.

use crate::hir::{CallResolution, HirBlock, HirExpr, HirStmt};
use crate::{LocalFlowState, RetainedClosureCapture};
use rsscript_syntax::{
    Span,
    ast::{Callee, Expr},
};
use std::collections::HashMap;

/// Collect local captures that would escape through a retaining closure call.
pub fn retained_closure_captures_from_flow(
    block: &HirBlock,
    entry_states: &HashMap<Span, LocalFlowState>,
) -> Vec<RetainedClosureCapture> {
    let mut captures = Vec::new();
    collect_retained_closure_captures_from_block(block, entry_states, &mut captures);
    captures
}

fn collect_retained_closure_captures_from_block(
    block: &HirBlock,
    entry_states: &HashMap<Span, LocalFlowState>,
    captures: &mut Vec<RetainedClosureCapture>,
) {
    for statement in &block.statements {
        collect_retained_closure_captures_from_stmt(statement, entry_states, captures);
    }
}

fn collect_retained_closure_captures_from_stmt(
    statement: &HirStmt,
    entry_states: &HashMap<Span, LocalFlowState>,
    captures: &mut Vec<RetainedClosureCapture>,
) {
    let entry_state = entry_states.get(crate::local_flow_statement_span(statement));
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(value, state, captures);
            }
        }
        HirStmt::Assign { target, value, .. } => {
            if let Some(state) = entry_state {
                for read in crate::hir::assign_target_reads(target) {
                    collect_retained_closure_captures_from_expr(read, state, captures);
                }
                collect_retained_closure_captures_from_expr(value, state, captures);
            }
        }
        HirStmt::With { resource, body, .. } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(resource, state, captures);
            }
            collect_retained_closure_captures_from_block(body, entry_states, captures);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(condition, state, captures);
            }
            collect_retained_closure_captures_from_block(then_body, entry_states, captures);
            if let Some(else_body) = else_body {
                collect_retained_closure_captures_from_block(else_body, entry_states, captures);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let (Some(condition), Some(state)) = (condition, entry_state) {
                collect_retained_closure_captures_from_expr(condition, state, captures);
            }
            collect_retained_closure_captures_from_block(body, entry_states, captures);
        }
        HirStmt::For { iterable, body, .. } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(iterable, state, captures);
            }
            collect_retained_closure_captures_from_block(body, entry_states, captures);
        }
        HirStmt::Match { value, arms, .. } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(value, state, captures);
            }
            for arm in arms {
                collect_retained_closure_captures_from_block(&arm.body, entry_states, captures);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                if let Some(state) = entry_state {
                    collect_retained_closure_captures_from_expr(&arm.operation, state, captures);
                }
                collect_retained_closure_captures_from_block(&arm.body, entry_states, captures);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_retained_closure_captures_from_expr(
    expr: &HirExpr,
    state: &LocalFlowState,
    captures: &mut Vec<RetainedClosureCapture>,
) {
    match expr {
        HirExpr::Call {
            callee,
            args,
            resolution,
            ..
        } => {
            if let CallResolution::Resolved { signature, .. } = resolution {
                for arg in args {
                    let Some(name) = arg.name.as_ref() else {
                        continue;
                    };
                    if !signature.retained_params.contains(name) {
                        continue;
                    }
                    let Some((body, closure_span)) = crate::retained_closure_argument(&arg.value)
                    else {
                        continue;
                    };
                    for (used_name, capture_span) in crate::hir_block_inline_capture_uses(body) {
                        if state.is_local(&used_name) {
                            push_retained_closure_capture(
                                captures,
                                RetainedClosureCapture {
                                    name: used_name,
                                    callee: callee_display(callee),
                                    param: name.clone(),
                                    capture_span,
                                    closure_span: closure_span.clone(),
                                },
                            );
                        }
                    }
                }
            }
            for arg in args {
                collect_retained_closure_captures_from_expr(&arg.value, state, captures);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_retained_closure_captures_from_expr(value, state, captures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_retained_closure_captures_from_expr(left, state, captures);
            collect_retained_closure_captures_from_expr(right, state, captures);
        }
        HirExpr::Field { base, .. } => {
            collect_retained_closure_captures_from_expr(base, state, captures);
        }
        HirExpr::Index { base, index, .. } => {
            collect_retained_closure_captures_from_expr(base, state, captures);
            collect_retained_closure_captures_from_expr(index, state, captures);
        }
        HirExpr::Closure { body, .. } => {
            collect_retained_closure_captures_from_block(body, &HashMap::new(), captures);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_retained_closure_captures_from_expr(value, state, captures);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_retained_closure_captures_from_expr(guard, state, captures);
                }
                collect_retained_closure_captures_from_block(&arm.body, &HashMap::new(), captures);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_retained_closure_captures_from_expr(&entry.key, state, captures);
                collect_retained_closure_captures_from_expr(&entry.value, state, captures);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_retained_closure_captures_from_expr(&field.value, state, captures);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_retained_closure_captures_from_expr(item, state, captures);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn push_retained_closure_capture(
    captures: &mut Vec<RetainedClosureCapture>,
    capture: RetainedClosureCapture,
) {
    if !captures.contains(&capture) {
        captures.push(capture);
    }
}

fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => format!(
            "{} {}.{method}",
            (*effect).map(|e| e.as_str()).unwrap_or("read"),
            local_expr_label(receiver)
        ),
    }
}

fn local_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::CharLiteral(value, _) | Expr::MultilineString(value, _) => {
            format!("{value:?}")
        }
        Expr::Field { base, name, .. } => format!("{}.{}", local_expr_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", local_expr_label(base)),
        Expr::Call { callee, .. } => format!("{}()", callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            local_expr_label(value)
        }
        _ => "<expr>".to_string(),
    }
}
