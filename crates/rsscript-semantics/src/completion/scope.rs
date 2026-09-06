//! Cursor-local binding and local-flow facts used by semantic completion.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rsscript_syntax::ast::{Block, Item, LetStmt, Program, Stmt};

use super::*;
use crate::hir::HirEffectEventKind;

#[derive(Debug, Clone)]
pub(super) struct ScopeBinding {
    pub(super) name: String,
    pub(super) kind: SemanticCompletionKind,
    pub(super) ty: Option<ResolvedType>,
    pub(super) depth: usize,
}

#[derive(Default)]
pub(super) struct ScopeFacts {
    pub(super) bindings: BTreeMap<String, ScopeBinding>,
    pub(super) function_name: Option<String>,
    pub(super) partial: bool,
}

pub(super) fn scope_at(source: &str, cursor: usize, program: &Program, hir: &Hir) -> ScopeFacts {
    let mut facts = ScopeFacts::default();
    let Some(function) = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) if function_contains(source, function, cursor) => {
                Some(function)
            }
            _ => None,
        })
        .min_by_key(|function| span_byte_length(source, &function.span))
    else {
        return facts;
    };
    facts.function_name = Some(function.name.clone());

    let mut types = HashMap::new();
    for parameter in &function.params {
        let ty = ResolvedType::from_type_ref(&parameter.ty);
        types.insert(parameter.name.clone(), ty.clone());
        facts.bindings.insert(
            parameter.name.clone(),
            ScopeBinding {
                name: parameter.name.clone(),
                kind: SemanticCompletionKind::Param,
                ty: Some(ty),
                depth: 1,
            },
        );
    }
    collect_block_scope(
        source,
        cursor,
        &function.body,
        hir,
        1,
        &mut types,
        &mut facts,
    );
    facts
}

#[derive(Default)]
pub(super) struct LocalAvailability {
    pub(super) unavailable: BTreeSet<String>,
    pub(super) partial: bool,
}

/// Project ownership availability from the checked HIR local-flow graph.
///
/// We deliberately do not search source text for `take`: effects inside
/// comments, strings, unrelated scopes, and malformed token sequences are not
/// ownership facts. A move on any path before the cursor makes availability
/// unprovable, so the binding is omitted and the enclosing result is partial.
pub(super) fn local_availability_before_cursor(
    source: &str,
    cursor: usize,
    hir: &Hir,
    scope: &ScopeFacts,
) -> LocalAvailability {
    let mut facts = LocalAvailability::default();
    let Some(function_name) = scope.function_name.as_deref() else {
        return facts;
    };
    let Some(body) = hir.function_body(function_name) else {
        facts.partial = true;
        return facts;
    };
    let Some(block) = body.block.as_ref() else {
        facts.partial = true;
        return facts;
    };
    for step in crate::local_flow_graph(block) {
        for event in step.events {
            if event.kind != HirEffectEventKind::Take {
                continue;
            }
            let Some(end) = span_end_byte(source, &event.value_span) else {
                facts.partial = true;
                continue;
            };
            if end <= cursor {
                if let Some(root) = crate::path_root(&event.binding_name) {
                    facts.unavailable.insert(root.to_string());
                    facts.partial = true;
                } else {
                    facts.partial = true;
                }
            }
        }
    }
    facts
}

fn collect_block_scope(
    source: &str,
    cursor: usize,
    block: &Block,
    hir: &Hir,
    depth: usize,
    types: &mut HashMap<String, ResolvedType>,
    facts: &mut ScopeFacts,
) {
    if !block_contains(source, block, cursor) {
        facts.partial = true;
        return;
    }
    for statement in &block.statements {
        if span_start_byte(source, stmt_span(statement)).is_none_or(|start| start > cursor) {
            break;
        }
        match statement {
            Stmt::Let(let_stmt) => add_let(source, cursor, let_stmt, hir, depth, types, facts),
            Stmt::With(stmt) if block_contains(source, &stmt.body, cursor) => {
                let mut child = types.clone();
                if let Some(ty) = crate::hir::infer_hir_expr_type(hir, &stmt.resource, types) {
                    child.insert(stmt.binding.clone(), ty.clone());
                    facts
                        .bindings
                        .insert(stmt.binding.clone(), binding(&stmt.binding, ty, depth + 1));
                } else {
                    facts.partial = true;
                }
                collect_block_scope(
                    source,
                    cursor,
                    &stmt.body,
                    hir,
                    depth + 1,
                    &mut child,
                    facts,
                );
                return;
            }
            Stmt::If(stmt) => {
                if block_contains(source, &stmt.then_body, cursor) {
                    let mut child = types.clone();
                    collect_block_scope(
                        source,
                        cursor,
                        &stmt.then_body,
                        hir,
                        depth + 1,
                        &mut child,
                        facts,
                    );
                    return;
                }
                if let Some(body) = &stmt.else_body
                    && block_contains(source, body, cursor)
                {
                    let mut child = types.clone();
                    collect_block_scope(source, cursor, body, hir, depth + 1, &mut child, facts);
                    return;
                }
            }
            Stmt::Loop(stmt) if block_contains(source, &stmt.body, cursor) => {
                let mut child = types.clone();
                collect_block_scope(
                    source,
                    cursor,
                    &stmt.body,
                    hir,
                    depth + 1,
                    &mut child,
                    facts,
                );
                return;
            }
            Stmt::For(stmt) if block_contains(source, &stmt.body, cursor) => {
                let mut child = types.clone();
                let item = crate::hir::infer_hir_expr_type(hir, &stmt.iterable, types)
                    .and_then(|ty| ty.named_argument("List", 0).cloned());
                if let Some(ty) = item {
                    child.insert(stmt.binding.clone(), ty.clone());
                    facts
                        .bindings
                        .insert(stmt.binding.clone(), binding(&stmt.binding, ty, depth + 1));
                } else {
                    facts.partial = true;
                }
                collect_block_scope(
                    source,
                    cursor,
                    &stmt.body,
                    hir,
                    depth + 1,
                    &mut child,
                    facts,
                );
                return;
            }
            Stmt::TaskGroup(stmt) if block_contains(source, &stmt.body, cursor) => {
                let mut child = types.clone();
                collect_block_scope(
                    source,
                    cursor,
                    &stmt.body,
                    hir,
                    depth + 1,
                    &mut child,
                    facts,
                );
                return;
            }
            Stmt::Match(stmt) => {
                for arm in &stmt.arms {
                    if block_contains(source, &arm.body, cursor) {
                        facts.partial = true;
                        let mut child = types.clone();
                        collect_block_scope(
                            source,
                            cursor,
                            &arm.body,
                            hir,
                            depth + 1,
                            &mut child,
                            facts,
                        );
                        return;
                    }
                }
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    if block_contains(source, &arm.body, cursor) {
                        facts.partial = true;
                        let mut child = types.clone();
                        collect_block_scope(
                            source,
                            cursor,
                            &arm.body,
                            hir,
                            depth + 1,
                            &mut child,
                            facts,
                        );
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}

fn binding(name: &str, ty: ResolvedType, depth: usize) -> ScopeBinding {
    ScopeBinding {
        name: name.to_string(),
        kind: SemanticCompletionKind::Local,
        ty: Some(ty),
        depth,
    }
}

fn add_let(
    source: &str,
    cursor: usize,
    stmt: &LetStmt,
    hir: &Hir,
    depth: usize,
    types: &mut HashMap<String, ResolvedType>,
    facts: &mut ScopeFacts,
) {
    let Some(name_start) = find_name_in_span(source, &stmt.span, &stmt.name) else {
        facts.partial = true;
        return;
    };
    if name_start > cursor {
        return;
    }
    let ty = stmt
        .type_annotation
        .as_ref()
        .map(ResolvedType::from_type_ref)
        .or_else(|| {
            stmt.value
                .as_ref()
                .and_then(|value| crate::hir::infer_hir_expr_type(hir, value, types))
        });
    if let Some(ty) = &ty {
        types.insert(stmt.name.clone(), ty.clone());
    } else {
        facts.partial = true;
    }
    facts.bindings.insert(
        stmt.name.clone(),
        ScopeBinding {
            name: stmt.name.clone(),
            kind: SemanticCompletionKind::Local,
            ty,
            depth,
        },
    );
}

fn stmt_span(statement: &Stmt) -> &Span {
    match statement {
        Stmt::Let(stmt) => &stmt.span,
        Stmt::Return(stmt) => &stmt.span,
        Stmt::With(stmt) => &stmt.span,
        Stmt::MalformedWith(span) => span,
        Stmt::If(stmt) => &stmt.span,
        Stmt::MalformedIf(span) => span,
        Stmt::Loop(stmt) => &stmt.span,
        Stmt::MalformedLoop(span) => span,
        Stmt::For(stmt) => &stmt.span,
        Stmt::MalformedFor(span) => span,
        Stmt::Match(stmt) => &stmt.span,
        Stmt::MalformedMatch(span) => span,
        Stmt::TaskGroup(stmt) => &stmt.span,
        Stmt::Select(stmt) => &stmt.span,
        Stmt::Break(span) | Stmt::Continue(span) | Stmt::Unknown(span) => span,
        Stmt::LetElse(stmt) => &stmt.span,
        Stmt::Assign(stmt) => &stmt.span,
        Stmt::Expr(expr) => expr.span(),
    }
}
