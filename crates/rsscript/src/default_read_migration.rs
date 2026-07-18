//! Conservative source simplifications enabled by RSScript's default `read`.
//!
//! This intentionally plans only edits whose target is proved by the resolved
//! HIR signature. Constructors, unresolved calls, closure values, and all
//! non-`read` effects remain untouched.

use crate::diagnostic::{FixEdit, Span};
use crate::hir::{CallResolution, Hir, HirBlock, HirExpr, HirStmt, ResolvedCalleeKind};
use crate::lexer::{Token, lex};
use crate::syntax::ast::{BinaryOp, Callee, DataEffect, Item, Program};
use crate::syntax::{isolate_module_namespaces, parse_source};

/// Plan conservative default-`read` simplifications.
///
/// The caller supplies the same interface sources used for analysis, so named
/// calls are only changed when their resolved target parameter is `read`.
pub fn default_read_migration_edits(
    file: &str,
    source: &str,
    interfaces: &[(&str, &str)],
) -> Vec<FixEdit> {
    let tokens = lex(file, source);
    let mut program = parse_source(file, source);
    isolate_module_namespaces(&mut program);
    let mut combined_interfaces = crate::standard_package_interfaces().to_vec();
    combined_interfaces.extend(interfaces.iter().copied());
    let interface_programs = combined_interfaces
        .iter()
        .map(|(interface_file, interface_source)| parse_source(interface_file, interface_source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&program, &interface_programs);

    let mut edits = declaration_edits(&program, &tokens, source);
    for (_, body) in hir.function_bodies() {
        if let Some(block) = &body.block {
            collect_call_edits(block, &tokens, source, &mut edits);
        }
    }
    edits.sort_by(|left, right| {
        left.span
            .line
            .cmp(&right.span.line)
            .then(left.span.column.cmp(&right.span.column))
    });
    edits.dedup_by(|left, right| {
        left.span.line == right.span.line
            && left.span.column == right.span.column
            && left.span.length == right.span.length
    });
    edits
}

fn declaration_edits(program: &Program, tokens: &[Token], source: &str) -> Vec<FixEdit> {
    let mut edits = Vec::new();
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        for param in &function.params {
            if param.effect != Some(DataEffect::Read)
                || param.effective_effect() != Some(DataEffect::Read)
            {
                continue;
            }
            if let Some(token) = explicit_read_after(tokens, &param.span) {
                edits.push(remove_read_edit(token, source));
            }
        }
    }
    edits
}

fn collect_call_edits(block: &HirBlock, tokens: &[Token], source: &str, edits: &mut Vec<FixEdit>) {
    for statement in &block.statements {
        collect_stmt_edits(statement, tokens, source, edits);
    }
}

fn collect_stmt_edits(
    statement: &HirStmt,
    tokens: &[Token],
    source: &str,
    edits: &mut Vec<FixEdit>,
) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_expr_edits(value, tokens, source, edits);
            }
        }
        HirStmt::With { resource, body, .. } => {
            collect_expr_edits(resource, tokens, source, edits);
            collect_call_edits(body, tokens, source, edits);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_edits(condition, tokens, source, edits);
            collect_call_edits(then_body, tokens, source, edits);
            if let Some(else_body) = else_body {
                collect_call_edits(else_body, tokens, source, edits);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_edits(condition, tokens, source, edits);
            }
            collect_call_edits(body, tokens, source, edits);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expr_edits(iterable, tokens, source, edits);
            collect_call_edits(body, tokens, source, edits);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expr_edits(value, tokens, source, edits);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_edits(guard, tokens, source, edits);
                }
                collect_call_edits(&arm.body, tokens, source, edits);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_edits(&arm.operation, tokens, source, edits);
                collect_call_edits(&arm.body, tokens, source, edits);
            }
        }
        HirStmt::Assign { target, value, .. } => {
            collect_expr_edits(target, tokens, source, edits);
            collect_expr_edits(value, tokens, source, edits);
        }
        HirStmt::Expr(expr) => collect_expr_edits(expr, tokens, source, edits),
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_edits(expr: &HirExpr, tokens: &[Token], source: &str, edits: &mut Vec<FixEdit>) {
    match expr {
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_edits(&field.value, tokens, source, edits);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_edits(&entry.key, tokens, source, edits);
                collect_expr_edits(&entry.value, tokens, source, edits);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_edits(item, tokens, source, edits);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            if let Some((insert, remove)) =
                boolean_negation_edits(expr, left, right, tokens, source)
            {
                edits.push(insert);
                edits.push(remove);
            }
            collect_expr_edits(left, tokens, source, edits);
            collect_expr_edits(right, tokens, source, edits);
        }
        HirExpr::Field { base, .. } => collect_expr_edits(base, tokens, source, edits),
        HirExpr::Index { base, index, .. } => {
            collect_expr_edits(base, tokens, source, edits);
            collect_expr_edits(index, tokens, source, edits);
        }
        HirExpr::Call {
            callee,
            receiver,
            args,
            resolution,
            span,
            ..
        } => {
            let is_constructor = matches!(
                resolution,
                CallResolution::Resolved {
                    kind: ResolvedCalleeKind::Constructor { .. },
                    ..
                } | CallResolution::EnumVariant
            );
            if !is_constructor {
                for arg in args {
                    if let Some(token) = explicit_read_after(tokens, &arg.span) {
                        edits.push(remove_read_edit(token, source));
                    }
                }
                if let CallResolution::Resolved { signature, .. } = resolution {
                    for arg in args {
                        let Some(name) = arg.name.as_deref() else {
                            continue;
                        };
                        // Named arguments are deliberately reorderable. Look up
                        // the resolved parameter by label rather than assuming
                        // its source position matches the signature position.
                        let Some(param) = signature.params.iter().find(|param| param.name == name)
                        else {
                            continue;
                        };
                        if param.effect != Some(crate::hir::ParamEffect::Read)
                            || explicit_read_after(tokens, &arg.span).is_some()
                            || hir_arg_is_not_same_ident(&arg.value, name)
                        {
                            continue;
                        }
                        if let Some(edit) = remove_same_name_label_edit(tokens, &arg.span, source) {
                            edits.push(edit);
                        }
                    }
                }
                if matches!(
                    callee,
                    Callee::ReceiverCall {
                        effect: Some(DataEffect::Read),
                        ..
                    }
                ) && let Some(token) = explicit_read_before(tokens, span)
                {
                    edits.push(remove_read_edit(token, source));
                }
            }
            if let Some(receiver) = receiver {
                collect_expr_edits(&receiver.value, tokens, source, edits);
            }
            for arg in args {
                collect_expr_edits(&arg.value, tokens, source, edits);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_expr_edits(value, tokens, source, edits),
        HirExpr::Closure { body, .. } => collect_call_edits(body, tokens, source, edits),
        HirExpr::Match { value, arms, .. } => {
            collect_expr_edits(value, tokens, source, edits);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_edits(guard, tokens, source, edits);
                }
                collect_call_edits(&arm.body, tokens, source, edits);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn boolean_negation_edits(
    expr: &HirExpr,
    left: &HirExpr,
    right: &HirExpr,
    tokens: &[Token],
    source: &str,
) -> Option<(FixEdit, FixEdit)> {
    let HirExpr::Binary { op, .. } = expr else {
        return None;
    };
    let HirExpr::Ident {
        name,
        span: right_span,
        ..
    } = right
    else {
        return None;
    };
    let left_span = hir_atomic_expr_span(left)?;
    if !((op == &BinaryOp::Equal && name == "false")
        || (op == &BinaryOp::NotEqual && name == "true"))
        || left_span.line != right_span.line
    {
        return None;
    }
    let operator = tokens.windows(2).find_map(|pair| {
        let first = &pair[0];
        let second = &pair[1];
        let matches_operator = (op == &BinaryOp::Equal && first.symbol("=") && second.symbol("="))
            || (op == &BinaryOp::NotEqual && first.symbol("!") && second.symbol("="));
        (matches_operator
            && first.span.line == left_span.line
            && first.span.column >= left_span.column + left_span.length
            && second.span.column < right_span.column)
            .then_some(first)
    })?;
    let operator_index = tokens.iter().position(|token| {
        token.span.file == operator.span.file
            && token.span.line == operator.span.line
            && token.span.column == operator.span.column
    })?;
    // AST spans may include formatting after a postfix expression. Anchor the
    // closing parenthesis to the final lexical token before the comparison.
    let left_end = tokens
        .get(operator_index.checked_sub(1)?)
        .filter(|token| token.span.line == left_span.line)
        .map(|token| token.span.column + token.span.length)
        .filter(|end| *end >= left_span.column)?;
    let whitespace_start = source
        .lines()
        .nth(left_span.line.checked_sub(1)?)
        .and_then(|line| {
            let gap = line
                .chars()
                .skip(left_end.checked_sub(1)?)
                .take(operator.span.column.checked_sub(left_end)?);
            gap.clone().all(char::is_whitespace).then_some(left_end)
        })
        .unwrap_or(operator.span.column);
    Some((
        // A bare name needs no grouping. Every other expression is wrapped so
        // the replacement preserves the original comparison's precedence.
        FixEdit::insert_before(
            left_span,
            if matches!(left, HirExpr::Ident { .. }) {
                "!"
            } else {
                "!("
            },
        ),
        FixEdit::replace(
            &Span {
                file: operator.span.file.clone(),
                line: operator.span.line,
                column: whitespace_start,
                length: right_span.column + right_span.length - whitespace_start,
            },
            if matches!(left, HirExpr::Ident { .. }) {
                ""
            } else {
                ")"
            },
        ),
    ))
}

/// HIR binary-expression spans deliberately anchor their operator rather than
/// cover the whole expression. Restrict source rewrites to variants whose span
/// is a complete expression range; otherwise a negation could be inserted in
/// the middle of a nested expression.
fn hir_atomic_expr_span(expr: &HirExpr) -> Option<&Span> {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::Char { span, .. }
        | HirExpr::ObjectLiteral { span, .. }
        | HirExpr::MapLiteral { span, .. }
        | HirExpr::ArrayLiteral { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Match { span, .. } => Some(span),
        HirExpr::Binary { .. }
        | HirExpr::Effect { .. }
        | HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Try { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn explicit_read_after<'a>(tokens: &'a [Token], anchor: &Span) -> Option<&'a Token> {
    let index = tokens
        .iter()
        .position(|token| same_start(&token.span, anchor))?;
    let colon = tokens.get(index + 1)?;
    let read = tokens.get(index + 2)?;
    (colon.symbol(":") && read.is_ident_text("read")).then_some(read)
}

fn explicit_read_before<'a>(tokens: &'a [Token], anchor: &Span) -> Option<&'a Token> {
    let index = tokens
        .iter()
        .position(|token| same_start(&token.span, anchor))?;
    let read = tokens.get(index.checked_sub(1)?)?;
    read.is_ident_text("read").then_some(read)
}

fn hir_arg_is_not_same_ident(expr: &HirExpr, name: &str) -> bool {
    match expr {
        HirExpr::Ident { name: ident, .. } => ident != name,
        HirExpr::Effect { value, .. } => hir_arg_is_not_same_ident(value, name),
        _ => true,
    }
}

fn remove_same_name_label_edit(tokens: &[Token], anchor: &Span, source: &str) -> Option<FixEdit> {
    let index = tokens
        .iter()
        .position(|token| same_start(&token.span, anchor))?;
    let label = tokens.get(index)?;
    let colon = tokens.get(index + 1)?;
    if !colon.symbol(":") {
        return None;
    }
    let trailing_horizontal_whitespace = source
        .lines()
        .nth(colon.span.line.saturating_sub(1))
        .map(|line| {
            line.chars()
                .skip(colon.span.column.saturating_sub(1) + colon.span.length)
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .count()
        })
        .unwrap_or(0);
    Some(FixEdit::replace(
        &Span {
            length: label.span.length + colon.span.length + trailing_horizontal_whitespace,
            ..label.span.clone()
        },
        "",
    ))
}

fn same_start(left: &Span, right: &Span) -> bool {
    left.file == right.file && left.line == right.line && left.column == right.column
}

fn remove_read_edit(token: &Token, source: &str) -> FixEdit {
    let trailing_horizontal_whitespace = source
        .lines()
        .nth(token.span.line.saturating_sub(1))
        .map(|line| {
            line.chars()
                .skip(token.span.column.saturating_sub(1) + token.span.length)
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .count()
        })
        .unwrap_or(0);
    FixEdit::replace(
        &Span {
            length: token.span.length + trailing_horizontal_whitespace,
            ..token.span.clone()
        },
        "",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze_source;

    fn apply(source: &str, edits: &[FixEdit]) -> String {
        let mut lines = source
            .lines()
            .map(|line| line.chars().collect::<Vec<_>>())
            .collect::<Vec<_>>();
        let mut ordered = edits.to_vec();
        ordered.sort_by(|left, right| {
            right
                .span
                .line
                .cmp(&left.span.line)
                .then(right.span.column.cmp(&left.span.column))
        });
        for edit in ordered {
            let line = &mut lines[edit.span.line - 1];
            let start = edit.span.column - 1;
            line.splice(start..start + edit.span.length, edit.replacement.chars());
        }
        let mut result = lines
            .into_iter()
            .map(|line| line.into_iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        if source.ends_with('\n') {
            result.push('\n');
        }
        result
    }

    #[test]
    fn migrates_only_resolved_default_read_sites() {
        let source = r#"
struct Boxed { item: String }

fn inspect(value: read String) -> Unit {}
fn rewrite(value: mut String) -> Unit {}

fn use(value: read String) -> Unit {
    inspect(value: read value)
    rewrite(value: mut value)
    let boxed = Boxed(item: read value)
}
"#;
        let edits = default_read_migration_edits("test.rss", source, &[]);
        let rewritten = apply(source, &edits);

        assert_eq!(edits.len(), 3);
        assert!(rewritten.contains("fn inspect(value: String)"));
        assert!(rewritten.contains("fn use(value: String)"));
        assert!(rewritten.contains("inspect(value: value)"));
        assert!(rewritten.contains("rewrite(value: mut value)"));
        assert!(rewritten.contains("Boxed(item: read value)"));
        assert!(analyze_source("test.rss", &rewritten).is_empty());
    }

    #[test]
    fn migrates_explicit_read_receiver_when_the_method_is_resolved() {
        let source = r#"
struct Viewer {}

fn Viewer.show(self: read Viewer, value: read String) -> Unit {}

fn use(viewer: read Viewer, value: read String) -> Unit {
    read viewer.show(value: read value)
}
"#;
        let edits = default_read_migration_edits("test.rss", source, &[]);
        let rewritten = apply(source, &edits);

        assert_eq!(edits.len(), 6);
        assert!(rewritten.contains("self: Viewer"));
        assert!(rewritten.contains("viewer: Viewer"));
        assert!(rewritten.contains("value: String"));
        assert!(rewritten.contains("viewer.show(value: value)"));
        assert!(!rewritten.contains("read viewer.show"));
        assert!(analyze_source("test.rss", &rewritten).is_empty());
    }

    #[test]
    fn migrates_same_name_read_argument_labels() {
        let source = r#"
fn inspect(value: String) -> Unit {}
fn use(value: String) -> Unit { inspect(value: value) }
"#;
        let edits = default_read_migration_edits("test.rss", source, &[]);
        let rewritten = apply(source, &edits);

        assert_eq!(edits.len(), 1);
        assert!(rewritten.contains("inspect(value)"));
        assert!(analyze_source("test.rss", &rewritten).is_empty());
    }

    #[test]
    fn migrates_reordered_same_name_read_argument_labels() {
        let source = r#"
fn combine(left: String, right: String) -> Unit {}
fn use(left: String, right: String) -> Unit { combine(right: right, left: left) }
"#;
        let edits = default_read_migration_edits("test.rss", source, &[]);
        let rewritten = apply(source, &edits);

        assert_eq!(edits.len(), 2, "{rewritten}");
        assert!(rewritten.contains("combine(right, left)"), "{rewritten}");
        assert!(analyze_source("test.rss", &rewritten).is_empty());
    }

    #[test]
    fn migrates_boolean_false_comparisons_to_not() {
        let source = r#"
fn invert(ready: Bool) -> Bool { return ready == false }
fn keep(ready: Bool) -> Bool { return ready != false }
"#;
        let edits = default_read_migration_edits("test.rss", source, &[]);
        let rewritten = apply(source, &edits);

        assert!(rewritten.contains("return !ready"), "{rewritten}");
        assert!(
            rewritten.lines().all(|line| !line.ends_with(' ')),
            "{rewritten}"
        );
        assert!(rewritten.contains("ready != false"));
        assert!(analyze_source("test.rss", &rewritten).is_empty());
    }

    #[test]
    fn migrates_postfix_boolean_false_comparisons_with_grouping() {
        let source = r#"
struct State { ready: Bool }
fn invert(state: State, left: Bool, right: Bool) -> Bool {
    let field = state.ready == false
    let combined = (left && right) != true
    return field || combined
}
"#;
        let edits = default_read_migration_edits("test.rss", source, &[]);
        let rewritten = apply(source, &edits);

        assert!(
            rewritten.contains("let field = !(state.ready)"),
            "{rewritten}"
        );
        assert!(
            rewritten.contains("let combined = (left && right) != true"),
            "{rewritten}"
        );
        assert!(analyze_source("test.rss", &rewritten).is_empty());
    }
}
