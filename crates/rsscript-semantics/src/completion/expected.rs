//! Expected-type projections for semantic completion.

use std::collections::HashMap;

use rsscript_syntax::ast::{Block, Item, Program, Stmt};

use super::*;

/// A match arm has an expected sum type only when the checked scrutinee can be
/// inferred from the lexical bindings at the cursor.
pub(super) fn match_pattern_expected_type(
    source: &str,
    cursor: usize,
    program: &Program,
    hir: &Hir,
    scope: &ScopeFacts,
) -> Option<ResolvedType> {
    let mut value_types = HashMap::new();
    for binding in scope.bindings.values() {
        if let Some(ty) = &binding.ty {
            value_types.insert(binding.name.clone(), ty.clone());
        }
    }
    fn visit_block(
        source: &str,
        cursor: usize,
        block: &Block,
        hir: &Hir,
        value_types: &HashMap<String, ResolvedType>,
    ) -> Option<ResolvedType> {
        for statement in &block.statements {
            match statement {
                Stmt::Match(match_stmt)
                    if span_start_byte(source, &match_stmt.span)
                        .is_some_and(|start| brace_contains_after(source, start, cursor))
                        && !match_stmt
                            .arms
                            .iter()
                            .any(|arm| block_contains(source, &arm.body, cursor)) =>
                {
                    return crate::hir::infer_hir_expr_type(hir, &match_stmt.value, value_types);
                }
                Stmt::If(statement) => {
                    if let Some(found) =
                        visit_block(source, cursor, &statement.then_body, hir, value_types)
                    {
                        return Some(found);
                    }
                    if let Some(else_body) = &statement.else_body
                        && let Some(found) =
                            visit_block(source, cursor, else_body, hir, value_types)
                    {
                        return Some(found);
                    }
                }
                Stmt::With(statement) => {
                    if let Some(found) =
                        visit_block(source, cursor, &statement.body, hir, value_types)
                    {
                        return Some(found);
                    }
                }
                Stmt::Loop(statement) => {
                    if let Some(found) =
                        visit_block(source, cursor, &statement.body, hir, value_types)
                    {
                        return Some(found);
                    }
                }
                Stmt::For(statement) => {
                    if let Some(found) =
                        visit_block(source, cursor, &statement.body, hir, value_types)
                    {
                        return Some(found);
                    }
                }
                Stmt::TaskGroup(statement) => {
                    if let Some(found) =
                        visit_block(source, cursor, &statement.body, hir, value_types)
                    {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
    for item in &program.items {
        if let Item::Function(function) = item
            && function_contains(source, function, cursor)
            && let Some(found) = visit_block(source, cursor, &function.body, hir, &value_types)
        {
            return Some(found);
        }
    }
    None
}

pub(super) fn typed_let_rhs<'a>(
    source: &str,
    cursor: usize,
    block: &'a Block,
) -> Option<&'a rsscript_syntax::ast::TypeRef> {
    for statement in &block.statements {
        match statement {
            Stmt::Let(let_stmt) => {
                let start = span_start_byte(source, &let_stmt.span)?;
                if start <= cursor && current_line_after_equals(&source[start..cursor]) {
                    return let_stmt.type_annotation.as_ref();
                }
            }
            Stmt::If(if_stmt) => {
                if let Some(found) = typed_let_rhs(source, cursor, &if_stmt.then_body) {
                    return Some(found);
                }
                if let Some(else_body) = &if_stmt.else_body
                    && let Some(found) = typed_let_rhs(source, cursor, else_body)
                {
                    return Some(found);
                }
            }
            Stmt::With(with_stmt) => {
                if let Some(found) = typed_let_rhs(source, cursor, &with_stmt.body) {
                    return Some(found);
                }
            }
            Stmt::Loop(loop_stmt) => {
                if let Some(found) = typed_let_rhs(source, cursor, &loop_stmt.body) {
                    return Some(found);
                }
            }
            Stmt::For(for_stmt) => {
                if let Some(found) = typed_let_rhs(source, cursor, &for_stmt.body) {
                    return Some(found);
                }
            }
            Stmt::TaskGroup(group) => {
                if let Some(found) = typed_let_rhs(source, cursor, &group.body) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn assignment_target_before_cursor(source: &str, cursor: usize) -> Option<&str> {
    let line = source[..cursor]
        .rsplit_once('\n')
        .map_or(&source[..cursor], |(_, line)| line);
    let (left, _) = line.rsplit_once('=')?;
    let target = left.trim();
    is_identifier(target).then_some(target)
}

fn current_line_after_equals(source: &str) -> bool {
    source
        .rsplit_once('\n')
        .map_or(source, |(_, line)| line)
        .contains('=')
}

pub(super) fn bool_condition_position(source: &str, cursor: usize) -> bool {
    let line = source[..cursor]
        .rsplit_once('\n')
        .map_or(&source[..cursor], |(_, line)| line);
    ["if", "while"].into_iter().any(|keyword| {
        line.trim_start().strip_prefix(keyword).is_some_and(|rest| {
            !rest.starts_with(|ch: char| ch.is_ascii_alphanumeric() || ch == '_')
        }) && !line.contains('{')
    })
}

pub(super) fn after_keyword_on_line(source: &str, cursor: usize, keyword: &str) -> bool {
    let line = source[..cursor]
        .rsplit_once('\n')
        .map_or(&source[..cursor], |(_, line)| line)
        .trim_start();
    line.strip_prefix(keyword)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}
