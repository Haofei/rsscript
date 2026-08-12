//! Flow-sensitive validation facts for `fresh` return proofs.

use crate::hir::{HirBlock, HirExpr, HirReturnProof, HirStmt};
use crate::{
    FreshReturnIssue, FreshReturnIssueKind, LocalFlowState, fresh_field_access_base,
    fresh_handle_or_weak_field_path, fresh_return_value_span,
};
use rsscript_syntax::Span;
use std::collections::HashMap;

/// Collect failed `fresh` return proofs using ownership states at HIR statement
/// entry. Diagnostics remain a compiler boundary concern; this exposes only
/// language facts.
pub fn fresh_return_issues_from_flow(
    block: &HirBlock,
    entry_states: &HashMap<Span, LocalFlowState>,
) -> Vec<FreshReturnIssue> {
    let mut issues = Vec::new();
    collect_block(block, entry_states, &mut issues);
    issues
}

fn collect_block(
    block: &HirBlock,
    entry_states: &HashMap<Span, LocalFlowState>,
    issues: &mut Vec<FreshReturnIssue>,
) {
    for statement in &block.statements {
        collect_stmt(statement, entry_states, issues);
    }
}

fn collect_stmt(
    statement: &HirStmt,
    entry_states: &HashMap<Span, LocalFlowState>,
    issues: &mut Vec<FreshReturnIssue>,
) {
    match statement {
        HirStmt::Return { value, proof, span } => {
            collect_issue(value.as_ref(), proof, span, entry_states, issues);
        }
        HirStmt::With { body, .. } | HirStmt::Loop { body, .. } | HirStmt::For { body, .. } => {
            collect_block(body, entry_states, issues);
        }
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => {
            collect_block(then_body, entry_states, issues);
            if let Some(else_body) = else_body {
                collect_block(else_body, entry_states, issues);
            }
        }
        HirStmt::Match { arms, .. } => {
            for arm in arms {
                collect_block(&arm.body, entry_states, issues);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_block(&arm.body, entry_states, issues);
            }
        }
        HirStmt::Let { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Expr(_)
        | HirStmt::Assign { .. }
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_issue(
    value: Option<&HirExpr>,
    proof: &HirReturnProof,
    return_span: &Span,
    entry_states: &HashMap<Span, LocalFlowState>,
    issues: &mut Vec<FreshReturnIssue>,
) {
    match proof {
        HirReturnProof::Ident { name } => {
            let span = fresh_return_value_span(value)
                .unwrap_or(return_span)
                .clone();
            if let Some(state) = entry_states.get(return_span) {
                let returns_fresh =
                    state.is_clean_local(name) && state.is_fresh_returnable_local(name);
                if state.is_managed(name) || state.is_local(name) {
                    if !returns_fresh {
                        push(
                            issues,
                            FreshReturnIssueKind::NotClean { name: name.clone() },
                            span,
                        );
                    }
                    return;
                }
            }
            push(
                issues,
                FreshReturnIssueKind::UnknownIdent { name: name.clone() },
                span,
            );
        }
        HirReturnProof::Unknown => {
            if let Some(value) = value
                && let Some(path) = fresh_handle_or_weak_field_path(value)
            {
                push(
                    issues,
                    FreshReturnIssueKind::NotClean { name: path },
                    fresh_return_value_span(Some(value))
                        .unwrap_or(return_span)
                        .clone(),
                );
                return;
            }
            if let Some(value) = value
                && fresh_field_access_base(value).is_some_and(|name| {
                    entry_states.get(return_span).is_some_and(|state| {
                        state.is_local(name)
                            && state.is_clean_local(name)
                            && state.is_fresh_returnable_local(name)
                    })
                })
            {
                return;
            }
            push(
                issues,
                FreshReturnIssueKind::Unknown,
                fresh_return_value_span(value)
                    .unwrap_or(return_span)
                    .clone(),
            );
        }
        HirReturnProof::NoValue
        | HirReturnProof::StructConstructor
        | HirReturnProof::FreshCall
        | HirReturnProof::Literal => {}
    }
}

fn push(issues: &mut Vec<FreshReturnIssue>, kind: FreshReturnIssueKind, span: Span) {
    let issue = FreshReturnIssue { kind, span };
    if !issues.contains(&issue) {
        issues.push(issue);
    }
}
