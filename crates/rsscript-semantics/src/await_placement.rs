//! Semantic validation of where source `await` expressions may occur.

use crate::hir::{CallResolution, HirBlock, HirExpr, HirStmt, assign_target_reads};
use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{
    Block as AstBlock, Callee, Expr as AstExpr, FunctionDecl, Stmt as AstStmt,
};

/// Diagnose `await` expressions outside an async function or structured task
/// group. Operand type and lifetime validation are separate semantic passes.
pub fn await_placement_diagnostics(block: &HirBlock, function_is_async: bool) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    collect_block(block, function_is_async, &mut diagnostics);
    diagnostics
}

/// Diagnose an await operand and consume a matching structured async-let name.
/// Returns `None` when the operand is a resolved async call or an outstanding
/// async-let binding from the current task group.
pub fn await_operand_diagnostic(
    value: &HirExpr,
    await_expr: &HirExpr,
    async_let_names: &mut Vec<String>,
) -> Option<Diagnostic> {
    if await_expr_targets_async_call(value) {
        return None;
    }
    if let Some(async_let_name) =
        await_targets_async_let_binding(value, async_let_names).map(str::to_owned)
    {
        async_let_names.retain(|name| name != &async_let_name);
        return None;
    }
    Some(
        Diagnostic::error(
            code::AWAIT_NON_ASYNC,
            "`await` must consume an async call.",
            expr_span(await_expr).clone(),
            "await non-async expression",
        )
        .with_cause("RSScript does not expose Future or Task values in source; the executable async MVP only awaits direct async calls.")
        .with_fix("await_async_call", "Await an `async fn` call directly.", "manual"),
    )
}

/// Diagnose an async call that is evaluated without an `await` boundary.
pub fn async_call_consumption_diagnostic(
    callee_display: &str,
    span: &rsscript_diagnostics::Span,
    is_async: bool,
    consumed: bool,
) -> Option<Diagnostic> {
    if !is_async || consumed {
        return None;
    }
    Some(
        Diagnostic::error(
            code::ASYNC_CALL_NOT_CONSUMED,
            format!("async call `{callee_display}` must be awaited."),
            span.clone(),
            "async call must be awaited",
        )
        .with_cause(
            "Async calls introduce suspension boundaries that must be visible in source; `spawn` is reserved but not executable in v0.7.",
        )
        .with_fix(
            "await_async_call",
            format!("Write `await {callee_display}(...)`."),
            "manual",
        ),
    )
}

/// Construct the canonical diagnostic for an `await` expression whose source
/// shape cannot be lowered by RSScript's structured async model.
pub fn async_fn_lowering_diagnostic(
    span: rsscript_diagnostics::Span,
    label: impl Into<String>,
    cause: impl Into<String>,
) -> Diagnostic {
    Diagnostic::error(
        code::ASYNC_FN_NOT_LOWERABLE,
        "async function is not lowerable in this version.",
        span,
        label,
    )
    .with_cause(cause)
    .with_fix(
        "restructure_async_fn",
        "Make every `await` a top-level statement, or move a `task_group` into a synchronous function.",
        "manual",
    )
}

/// Construct the canonical diagnostic for a cancellation-token request that
/// has no lexically owning task group.
pub fn cancellation_token_outside_task_group_diagnostic(
    span: rsscript_diagnostics::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::CANCELLATION_TOKEN_OUTSIDE_TASK_GROUP,
        "`Task.cancellation_token()` is not allowed inside an `async fn`.",
        span,
        "this would observe a never-cancelled token, not the task_group's",
    )
    .with_cause(
        "An `async fn` has no lexically enclosing `task_group`, so this call cannot inherit the group's cancellation token and would silently never cancel.",
    )
    .with_fix(
        "pass_cancellation_token",
        "Call `Task.cancellation_token()` inside the `task_group` block and pass the token into this function as a `read CancellationToken` parameter.",
        "manual",
    )
}

/// Validate cancellation-token ownership for one source async function.
///
/// `Task.cancellation_token()` is meaningful only in the lexical body of a
/// task group. A nested task group owns an independent token and is therefore
/// intentionally excluded from this traversal.
pub fn async_function_cancellation_diagnostics(function: &FunctionDecl) -> Vec<Diagnostic> {
    if !function.is_async {
        return Vec::new();
    }
    first_cancellation_token_in_block(&function.body)
        .map(cancellation_token_outside_task_group_diagnostic)
        .into_iter()
        .collect()
}

fn first_cancellation_token_in_block(block: &AstBlock) -> Option<rsscript_diagnostics::Span> {
    block
        .statements
        .iter()
        .find_map(first_cancellation_token_in_statement)
}

fn first_cancellation_token_in_statement(
    statement: &AstStmt,
) -> Option<rsscript_diagnostics::Span> {
    match statement {
        AstStmt::Let(statement) => statement
            .value
            .as_ref()
            .and_then(first_cancellation_token_in_expr),
        AstStmt::Return(statement) => statement
            .value
            .as_ref()
            .and_then(first_cancellation_token_in_expr),
        AstStmt::Expr(expr) => first_cancellation_token_in_expr(expr),
        AstStmt::With(statement) => first_cancellation_token_in_expr(&statement.resource)
            .or_else(|| first_cancellation_token_in_block(&statement.body)),
        AstStmt::If(statement) => first_cancellation_token_in_expr(&statement.condition)
            .or_else(|| first_cancellation_token_in_block(&statement.then_body))
            .or_else(|| {
                statement
                    .else_body
                    .as_ref()
                    .and_then(first_cancellation_token_in_block)
            }),
        AstStmt::Loop(statement) => statement
            .condition
            .as_ref()
            .and_then(first_cancellation_token_in_expr)
            .or_else(|| first_cancellation_token_in_block(&statement.body)),
        AstStmt::For(statement) => first_cancellation_token_in_expr(&statement.iterable)
            .or_else(|| first_cancellation_token_in_block(&statement.body)),
        AstStmt::Match(statement) => {
            first_cancellation_token_in_expr(&statement.value).or_else(|| {
                statement
                    .arms
                    .iter()
                    .find_map(|arm| first_cancellation_token_in_block(&arm.body))
            })
        }
        AstStmt::TaskGroup(_) => None,
        AstStmt::LetElse(statement) => first_cancellation_token_in_expr(&statement.value)
            .or_else(|| first_cancellation_token_in_block(&statement.else_body)),
        _ => None,
    }
}

fn first_cancellation_token_in_expr(expr: &AstExpr) -> Option<rsscript_diagnostics::Span> {
    match expr {
        AstExpr::Call { callee, args, span } => {
            if let Callee::Qualified { namespace, name } = callee
                && namespace == "Task"
                && name == "cancellation_token"
            {
                return Some(span.clone());
            }
            args.iter()
                .find_map(|argument| first_cancellation_token_in_expr(&argument.value))
        }
        AstExpr::Binary { left, right, .. } => first_cancellation_token_in_expr(left)
            .or_else(|| first_cancellation_token_in_expr(right)),
        AstExpr::Field { base, .. } => first_cancellation_token_in_expr(base),
        AstExpr::Index { base, index, .. } => first_cancellation_token_in_expr(base)
            .or_else(|| first_cancellation_token_in_expr(index)),
        AstExpr::Effect { value, .. }
        | AstExpr::Manage { value, .. }
        | AstExpr::Spawn { value, .. }
        | AstExpr::Await { value, .. }
        | AstExpr::Try { value, .. } => first_cancellation_token_in_expr(value),
        AstExpr::Closure { body, .. } => first_cancellation_token_in_block(body),
        AstExpr::Match { value, arms, .. } => {
            first_cancellation_token_in_expr(value).or_else(|| {
                arms.iter()
                    .find_map(|arm| first_cancellation_token_in_block(&arm.body))
            })
        }
        AstExpr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| first_cancellation_token_in_expr(&entry.key))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| first_cancellation_token_in_expr(&entry.value))
            }),
        AstExpr::ObjectLiteral { .. }
        | AstExpr::ArrayLiteral { .. }
        | AstExpr::Ident(..)
        | AstExpr::Number(..)
        | AstExpr::String(..)
        | AstExpr::CharLiteral(..)
        | AstExpr::MultilineString(..)
        | AstExpr::Unknown(_) => None,
    }
}

/// A value whose current flow state would retain it across an `await`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwaitLiveValueFact {
    pub kind: &'static str,
    pub name: String,
}

/// Diagnose flow facts that are invalid across a suspension boundary.
pub fn await_live_value_diagnostics(
    span: &rsscript_diagnostics::Span,
    facts: &[AwaitLiveValueFact],
) -> Vec<Diagnostic> {
    facts
        .iter()
        .map(|fact| {
            Diagnostic::error(
                code::AWAIT_LIVE_LOCAL,
                format!("{} `{}` cannot live across `await`.", fact.kind, fact.name),
                span.clone(),
                "value live across await",
            )
            .with_cause("Suspending an RSScript async frame may keep managed handles and Copy snapshots, but local values, resources, and runtime guards must not be retained across suspension.")
            .with_fix("drop_before_await", format!("End the lifetime of `{}` before this `await`.", fact.name), "manual")
        })
        .collect()
}

fn await_targets_async_let_binding<'a>(
    expr: &'a HirExpr,
    async_let_names: &'a [String],
) -> Option<&'a str> {
    match expr {
        HirExpr::Ident { name, .. } if async_let_names.contains(name) => Some(name.as_str()),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            await_targets_async_let_binding(value, async_let_names)
        }
        _ => None,
    }
}

fn await_expr_targets_async_call(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Call { resolution, .. } => {
            matches!(resolution, CallResolution::Resolved { signature, .. } if signature.is_async)
        }
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            await_expr_targets_async_call(value)
        }
        _ => false,
    }
}

fn expr_span(expr: &HirExpr) -> &rsscript_diagnostics::Span {
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

fn collect_block(block: &HirBlock, function_is_async: bool, diagnostics: &mut Vec<Diagnostic>) {
    // A task-group body is flattened into its parent block. Its async `let`
    // bindings identify the structured-concurrency boundary where awaits are
    // valid even within a synchronous enclosing function.
    let in_task_group = block
        .statements
        .iter()
        .any(|statement| matches!(statement, HirStmt::Let { is_async: true, .. }));
    let async_context = function_is_async || in_task_group;
    for statement in &block.statements {
        collect_statement(statement, async_context, diagnostics);
    }
}

fn collect_statement(
    statement: &HirStmt,
    function_is_async: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match statement {
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_expression(value, function_is_async, diagnostics);
            }
        }
        HirStmt::With { resource, body, .. } => {
            collect_expression(resource, function_is_async, diagnostics);
            collect_block(body, function_is_async, diagnostics);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expression(condition, function_is_async, diagnostics);
            collect_block(then_body, function_is_async, diagnostics);
            if let Some(else_body) = else_body {
                collect_block(else_body, function_is_async, diagnostics);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expression(condition, function_is_async, diagnostics);
            }
            collect_block(body, function_is_async, diagnostics);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expression(iterable, function_is_async, diagnostics);
            collect_block(body, function_is_async, diagnostics);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expression(value, function_is_async, diagnostics);
            for arm in arms {
                collect_block(&arm.body, function_is_async, diagnostics);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                // Select operations run at the structured await boundary, while
                // their bodies remain ordinary code in the enclosing context.
                collect_expression(&arm.operation, true, diagnostics);
                collect_block(&arm.body, function_is_async, diagnostics);
            }
        }
        HirStmt::Expr(value) => collect_expression(value, function_is_async, diagnostics),
        HirStmt::Assign { target, value, .. } => {
            collect_expression(value, function_is_async, diagnostics);
            for read in assign_target_reads(target) {
                collect_expression(read, function_is_async, diagnostics);
            }
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn collect_expression(expr: &HirExpr, function_is_async: bool, diagnostics: &mut Vec<Diagnostic>) {
    match expr {
        HirExpr::Await { value, span, .. } => {
            if !function_is_async {
                diagnostics.push(
                    Diagnostic::error(
                        code::AWAIT_OUTSIDE_ASYNC,
                        "`await` is only valid inside an async function.",
                        span.clone(),
                        "await outside async fn",
                    )
                    .with_cause("Suspension points are part of the async function frame and cannot appear in ordinary synchronous functions.")
                    .with_fix("move_to_async_fn", "Move this await into an `async fn`, or call a synchronous API.", "manual"),
                );
            }
            collect_expression(value, function_is_async, diagnostics);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expression(left, function_is_async, diagnostics);
            collect_expression(right, function_is_async, diagnostics);
        }
        HirExpr::Field { base, .. } => collect_expression(base, function_is_async, diagnostics),
        HirExpr::Index { base, index, .. } => {
            collect_expression(base, function_is_async, diagnostics);
            collect_expression(index, function_is_async, diagnostics);
        }
        HirExpr::Call { args, .. } => {
            for argument in args {
                collect_expression(&argument.value, function_is_async, diagnostics);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Try { value, .. } => collect_expression(value, function_is_async, diagnostics),
        HirExpr::Closure { body, .. } => collect_block(body, false, diagnostics),
        HirExpr::Match { value, arms, .. } => {
            collect_expression(value, function_is_async, diagnostics);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expression(guard, function_is_async, diagnostics);
                }
                collect_block(&arm.body, function_is_async, diagnostics);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expression(&entry.key, function_is_async, diagnostics);
                collect_expression(&entry.value, function_is_async, diagnostics);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expression(&field.value, function_is_async, diagnostics);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expression(item, function_is_async, diagnostics);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_diagnostics::Span;

    fn span() -> Span {
        Span {
            file: "async.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    fn await_expression() -> HirExpr {
        HirExpr::Await {
            value: Box::new(HirExpr::Unknown(span())),
            type_name: None,
            span: span(),
        }
    }

    #[test]
    fn rejects_await_in_synchronous_hir_but_allows_it_in_async_hir() {
        let block = HirBlock {
            statements: vec![HirStmt::Expr(await_expression())],
            span: span(),
        };

        let diagnostics = await_placement_diagnostics(&block, false);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::AWAIT_OUTSIDE_ASYNC);
        assert!(await_placement_diagnostics(&block, true).is_empty());
    }

    #[test]
    fn await_operand_consumes_a_structured_async_let_once() {
        let value = HirExpr::Ident {
            name: "pending".to_owned(),
            type_name: None,
            span: span(),
        };
        let await_expr = HirExpr::Await {
            value: Box::new(value.clone()),
            type_name: None,
            span: span(),
        };
        let mut async_let_names = vec!["pending".to_owned()];

        assert!(await_operand_diagnostic(&value, &await_expr, &mut async_let_names).is_none());
        assert!(async_let_names.is_empty());
        let diagnostic = await_operand_diagnostic(&value, &await_expr, &mut async_let_names)
            .expect("a consumed async let cannot be awaited twice");
        assert_eq!(diagnostic.code, code::AWAIT_NON_ASYNC);
    }

    #[test]
    fn async_call_consumption_requires_an_explicit_boundary() {
        let diagnostic = async_call_consumption_diagnostic("fetch", &span(), true, false)
            .expect("an unconsumed async call is invalid");
        assert_eq!(diagnostic.code, code::ASYNC_CALL_NOT_CONSUMED);
        assert!(async_call_consumption_diagnostic("fetch", &span(), true, true).is_none());
        assert!(async_call_consumption_diagnostic("fetch", &span(), false, false).is_none());
    }

    #[test]
    fn async_lowering_and_cancellation_contracts_are_canonical() {
        let lowering = async_fn_lowering_diagnostic(
            span(),
            "await is nested in an expression",
            "the source shape needs an explicit suspension boundary.",
        );
        assert_eq!(lowering.code, code::ASYNC_FN_NOT_LOWERABLE);
        assert_eq!(lowering.fixes[0].kind, "restructure_async_fn");

        let cancellation = cancellation_token_outside_task_group_diagnostic(span());
        assert_eq!(
            cancellation.code,
            code::CANCELLATION_TOKEN_OUTSIDE_TASK_GROUP
        );
        assert_eq!(cancellation.fixes[0].kind, "pass_cancellation_token");
    }

    #[test]
    fn async_function_cancellation_rule_stops_at_task_group_boundaries() {
        let program = rsscript_syntax::parse_source(
            "async.rss",
            "async fn invalid() { Task.cancellation_token() }\nasync fn valid() { task_group { Task.cancellation_token() } }",
        );
        let functions = program
            .items
            .iter()
            .filter_map(|item| match item {
                rsscript_syntax::ast::Item::Function(function) => Some(function),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            async_function_cancellation_diagnostics(functions[0]).len(),
            1
        );
        assert!(async_function_cancellation_diagnostics(functions[1]).is_empty());
    }

    #[test]
    fn live_value_facts_preserve_resource_and_local_diagnostics() {
        let facts = [
            AwaitLiveValueFact {
                kind: "resource",
                name: "file".to_owned(),
            },
            AwaitLiveValueFact {
                kind: "local value",
                name: "payload".to_owned(),
            },
        ];
        let diagnostics = await_live_value_diagnostics(&span(), &facts);
        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code == code::AWAIT_LIVE_LOCAL)
        );
        assert!(diagnostics[0].summary.contains("resource `file`"));
        assert!(diagnostics[1].summary.contains("local value `payload`"));
    }
}
