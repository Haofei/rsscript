//! Semantic control-flow diagnostics over resolved HIR.

use crate::hir::{Hir, HirBlock, HirExpr, HirMatchArm, HirStmt, number_literal_type_name};
use rsscript_diagnostics::{Diagnostic, Span, code};
use rsscript_syntax::ast::{
    Block, DataEffect, FunctionDecl, Item, MatchLiteral, MatchPattern, Program, Stmt, TypeRef,
};
use std::collections::HashSet;

/// Construct the canonical diagnostic for a resolved non-exhaustive `match`.
pub fn non_exhaustive_match_diagnostic(expression: bool, span: Span) -> Diagnostic {
    let subject = if expression {
        "match expression"
    } else {
        "match statement"
    };
    Diagnostic::error(
        code::NON_EXHAUSTIVE_MATCH,
        format!("{subject} is not exhaustive."),
        span,
        "non-exhaustive match",
    )
    .with_cause(
        "Supported match statements must cover `Some`/`None`, `Ok`/`Err`, all sum type variants, or include `_`.",
    )
    .with_fix(
        "add_missing_arm",
        "Add the missing variant arm or a final `_` fallback.",
        "manual",
    )
}

pub fn function_fallthrough_diagnostics(program: &Program, hir: &Hir) -> Vec<Diagnostic> {
    program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => fallthrough_diagnostic(function, hir),
            _ => None,
        })
        .collect()
}

/// Diagnose explicit bare returns from a concrete non-`Unit` function.
pub fn missing_return_value_diagnostics(program: &Program, hir: &Hir) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &program.items {
        let Item::Function(function) = item else {
            continue;
        };
        let Some(return_ty) = &function.return_ty else {
            continue;
        };
        if !function.has_body || (return_ty.name == "Unit" && return_ty.args.is_empty()) {
            continue;
        }
        let generics = function
            .type_params
            .iter()
            .map(|param| param.name.as_str())
            .collect();
        if type_mentions_generic(return_ty, &generics) {
            continue;
        }
        let Some(body) = hir
            .function_body(&function.name)
            .and_then(|body| body.block.as_ref())
        else {
            continue;
        };
        collect_bare_returns(
            body,
            function,
            &render_type_ref(return_ty),
            &mut diagnostics,
        );
    }
    diagnostics
}

/// Diagnose `break`/`continue` written outside any enclosing loop.
///
/// The rule is purely lexical: a `break` or `continue` binds to the innermost
/// `loop`/`while`/`for` that encloses it *in the same control-flow region*. A
/// closure body is its own region, so a loop surrounding the closure is not a
/// target for a `break` inside it. Every other nesting construct (`if`, `match`
/// arms, `select` arms, `with` bodies, plain blocks) is transparent.
pub fn loop_control_flow_diagnostics(block: &HirBlock) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    collect_loop_control_flow(block, false, &mut diagnostics);
    diagnostics
}

fn loop_control_outside_loop_diagnostic(keyword: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::LOOP_CONTROL_OUTSIDE_LOOP,
        format!("`{keyword}` is not inside a loop."),
        span,
        "no enclosing loop",
    )
    .with_cause(
        "`break` and `continue` target the innermost enclosing `loop`, `while`, or `for` of the same control-flow region; a closure body starts a new region.",
    )
    .with_fix(
        "remove_loop_control",
        format!("Remove the `{keyword}`, or move it inside the loop it was meant to control."),
        "manual",
    )
}

fn collect_loop_control_flow(block: &HirBlock, in_loop: bool, diagnostics: &mut Vec<Diagnostic>) {
    for statement in &block.statements {
        match statement {
            HirStmt::Break(span) if !in_loop => {
                diagnostics.push(loop_control_outside_loop_diagnostic("break", span.clone()));
            }
            HirStmt::Continue(span) if !in_loop => {
                diagnostics.push(loop_control_outside_loop_diagnostic(
                    "continue",
                    span.clone(),
                ));
            }
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_loop_control_flow_expr(condition, in_loop, diagnostics);
                }
                collect_loop_control_flow(body, true, diagnostics);
            }
            HirStmt::For { iterable, body, .. } => {
                collect_loop_control_flow_expr(iterable, in_loop, diagnostics);
                collect_loop_control_flow(body, true, diagnostics);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_loop_control_flow_expr(condition, in_loop, diagnostics);
                collect_loop_control_flow(then_body, in_loop, diagnostics);
                if let Some(else_body) = else_body {
                    collect_loop_control_flow(else_body, in_loop, diagnostics);
                }
            }
            HirStmt::With { resource, body, .. } => {
                collect_loop_control_flow_expr(resource, in_loop, diagnostics);
                collect_loop_control_flow(body, in_loop, diagnostics);
            }
            HirStmt::Match { value, arms, .. } => {
                collect_loop_control_flow_expr(value, in_loop, diagnostics);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        collect_loop_control_flow_expr(guard, in_loop, diagnostics);
                    }
                    collect_loop_control_flow(&arm.body, in_loop, diagnostics);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_loop_control_flow_expr(&arm.operation, in_loop, diagnostics);
                    collect_loop_control_flow(&arm.body, in_loop, diagnostics);
                }
            }
            HirStmt::Let { value, .. } => {
                if let Some(value) = value {
                    collect_loop_control_flow_expr(value, in_loop, diagnostics);
                }
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    collect_loop_control_flow_expr(value, in_loop, diagnostics);
                }
            }
            HirStmt::Assign { target, value, .. } => {
                collect_loop_control_flow_expr(target, in_loop, diagnostics);
                collect_loop_control_flow_expr(value, in_loop, diagnostics);
            }
            HirStmt::Expr(expr) => collect_loop_control_flow_expr(expr, in_loop, diagnostics),
        }
    }
}

/// Walk into the blocks an expression can carry. A closure body is a new
/// control-flow region, so it restarts with no enclosing loop.
fn collect_loop_control_flow_expr(
    expr: &HirExpr,
    in_loop: bool,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        HirExpr::Closure { body, .. } => collect_loop_control_flow(body, false, diagnostics),
        HirExpr::Match { value, arms, .. } => {
            collect_loop_control_flow_expr(value, in_loop, diagnostics);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_loop_control_flow_expr(guard, in_loop, diagnostics);
                }
                collect_loop_control_flow(&arm.body, in_loop, diagnostics);
            }
        }
        HirExpr::Call { args, receiver, .. } => {
            if let Some(receiver) = receiver {
                collect_loop_control_flow_expr(&receiver.value, in_loop, diagnostics);
            }
            for arg in args {
                collect_loop_control_flow_expr(&arg.value, in_loop, diagnostics);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            collect_loop_control_flow_expr(left, in_loop, diagnostics);
            collect_loop_control_flow_expr(right, in_loop, diagnostics);
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. }
        | HirExpr::Field { base: value, .. } => {
            collect_loop_control_flow_expr(value, in_loop, diagnostics);
        }
        HirExpr::Index { base, index, .. } => {
            collect_loop_control_flow_expr(base, in_loop, diagnostics);
            collect_loop_control_flow_expr(index, in_loop, diagnostics);
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_loop_control_flow_expr(item, in_loop, diagnostics);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_loop_control_flow_expr(&entry.key, in_loop, diagnostics);
                collect_loop_control_flow_expr(&entry.value, in_loop, diagnostics);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_loop_control_flow_expr(&field.value, in_loop, diagnostics);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

/// Diagnose a read of a local that no earlier statement assigns.
///
/// The rule, stated precisely, is deliberately weaker than full
/// definite-assignment analysis:
///
/// * A `let` declared with a type but no initializer starts *unassigned*.
/// * The body is walked in source order. Within one statement, the reads in its
///   expressions are seen before that statement's own assignment takes effect,
///   so `x = x + 1` on an unassigned `x` is a read of an unassigned binding.
/// * **Any** assignment to the name, anywhere earlier in the walk — including
///   inside one arm of an `if`, one `match` arm, or a loop body that may run
///   zero times — marks it assigned from then on. The analysis is therefore
///   optimistic about paths and only reports a read that *no* path assigns.
/// * A later `let` of the same name with an initializer also marks it assigned.
/// * Closure bodies are not walked at all: a closure runs at a time this check
///   does not model.
///
/// Consequence: every read this reports is wrong on every path, so there are no
/// false positives from branch merging — at the cost of missing reads that are
/// unassigned on only some paths.
pub fn definite_assignment_diagnostics(body: &Block, block: &HirBlock) -> Vec<Diagnostic> {
    let mut deferred = HashSet::new();
    collect_deferred_let_spans(body, &mut deferred);
    if deferred.is_empty() {
        return Vec::new();
    }
    let mut state = DefiniteAssignment {
        deferred,
        unassigned: HashSet::new(),
    };
    let mut diagnostics = Vec::new();
    collect_definite_assignment(block, &mut state, &mut diagnostics);
    diagnostics
}

/// The deferred declarations of one function body, and which of them have not
/// been assigned yet at the current point of the walk.
struct DefiniteAssignment {
    deferred: HashSet<Span>,
    unassigned: HashSet<String>,
}

/// Collect the spans of `let name: T` declarations that carry no initializer.
///
/// This is read off the *syntax* tree on purpose. In HIR a `let … else` also
/// lowers to a `Let` with no value (the value is bound by the preceding
/// `match`), and that binding is always assigned; keying on the syntax
/// declaration keeps the two apart exactly rather than by heuristic.
fn collect_deferred_let_spans(block: &Block, spans: &mut HashSet<Span>) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) if stmt.value.is_none() => {
                spans.insert(stmt.span.clone());
            }
            Stmt::If(stmt) => {
                collect_deferred_let_spans(&stmt.then_body, spans);
                if let Some(else_body) = &stmt.else_body {
                    collect_deferred_let_spans(else_body, spans);
                }
            }
            Stmt::Loop(stmt) => collect_deferred_let_spans(&stmt.body, spans),
            Stmt::For(stmt) => collect_deferred_let_spans(&stmt.body, spans),
            Stmt::With(stmt) => collect_deferred_let_spans(&stmt.body, spans),
            Stmt::TaskGroup(stmt) => collect_deferred_let_spans(&stmt.body, spans),
            Stmt::LetElse(stmt) => collect_deferred_let_spans(&stmt.else_body, spans),
            Stmt::Match(stmt) => {
                for arm in &stmt.arms {
                    collect_deferred_let_spans(&arm.body, spans);
                }
            }
            Stmt::Select(stmt) => {
                for arm in &stmt.arms {
                    collect_deferred_let_spans(&arm.body, spans);
                }
            }
            Stmt::Let(_)
            | Stmt::Return(_)
            | Stmt::Assign(_)
            | Stmt::Expr(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => {}
        }
    }
}

fn read_before_assignment_diagnostic(name: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::READ_BEFORE_ASSIGNMENT,
        format!("`{name}` is read before it is assigned."),
        span,
        "binding not assigned",
    )
    .with_cause(
        "The binding was declared with a type but no initializer, and no statement before this one assigns it on any path.",
    )
    .with_fix(
        "initialize_binding",
        format!("Give `{name}` an initializer, or assign it before this read."),
        "manual",
    )
}

fn collect_definite_assignment(
    block: &HirBlock,
    state: &mut DefiniteAssignment,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                name, value, span, ..
            } => match value {
                Some(value) => {
                    collect_assignment_reads(value, state, diagnostics);
                    state.unassigned.remove(name);
                }
                None => {
                    if state.deferred.contains(span) {
                        // A deferred declaration: the binding holds no value yet.
                        state.unassigned.insert(name.clone());
                    } else {
                        // A `let … else` binding, already bound by the match.
                        state.unassigned.remove(name);
                    }
                }
            },
            HirStmt::Assign { target, value, .. } => {
                collect_assignment_reads(value, state, diagnostics);
                match target {
                    // `x = e` initializes the whole binding.
                    HirExpr::Ident { name, .. } => {
                        state.unassigned.remove(name);
                    }
                    // `x.f = e` / `x[i] = e` read the existing place first.
                    target => collect_assignment_reads(target, state, diagnostics),
                }
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    collect_assignment_reads(value, state, diagnostics);
                }
            }
            HirStmt::Expr(expr) => collect_assignment_reads(expr, state, diagnostics),
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                collect_assignment_reads(resource, state, diagnostics);
                state.unassigned.remove(binding);
                collect_definite_assignment(body, state, diagnostics);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_assignment_reads(condition, state, diagnostics);
                collect_definite_assignment(then_body, state, diagnostics);
                if let Some(else_body) = else_body {
                    collect_definite_assignment(else_body, state, diagnostics);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_assignment_reads(condition, state, diagnostics);
                }
                collect_definite_assignment(body, state, diagnostics);
            }
            HirStmt::For {
                binding,
                iterable,
                body,
                ..
            } => {
                collect_assignment_reads(iterable, state, diagnostics);
                state.unassigned.remove(binding);
                collect_definite_assignment(body, state, diagnostics);
            }
            HirStmt::Match { value, arms, .. } => {
                collect_assignment_reads(value, state, diagnostics);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        collect_assignment_reads(guard, state, diagnostics);
                    }
                    collect_definite_assignment(&arm.body, state, diagnostics);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_assignment_reads(&arm.operation, state, diagnostics);
                    state.unassigned.remove(&arm.binding);
                    collect_definite_assignment(&arm.body, state, diagnostics);
                }
            }
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
        }
    }
}

fn collect_assignment_reads(
    expr: &HirExpr,
    state: &mut DefiniteAssignment,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match expr {
        HirExpr::Ident { name, span, .. } => {
            if state.unassigned.contains(name) {
                diagnostics.push(read_before_assignment_diagnostic(name, span.clone()));
                // Report each deferred binding once per function body.
                state.unassigned.remove(name);
            }
        }
        // A closure runs at a time this check does not model.
        HirExpr::Closure { .. } => {}
        HirExpr::Call { args, receiver, .. } => {
            if let Some(receiver) = receiver {
                collect_assignment_reads(&receiver.value, state, diagnostics);
            }
            for arg in args {
                collect_assignment_reads(&arg.value, state, diagnostics);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            collect_assignment_reads(left, state, diagnostics);
            collect_assignment_reads(right, state, diagnostics);
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. }
        | HirExpr::Field { base: value, .. } => {
            collect_assignment_reads(value, state, diagnostics);
        }
        HirExpr::Index { base, index, .. } => {
            collect_assignment_reads(base, state, diagnostics);
            collect_assignment_reads(index, state, diagnostics);
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_assignment_reads(item, state, diagnostics);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_assignment_reads(&entry.key, state, diagnostics);
                collect_assignment_reads(&entry.value, state, diagnostics);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_assignment_reads(&field.value, state, diagnostics);
            }
        }
        HirExpr::Match { value, arms, .. } => {
            collect_assignment_reads(value, state, diagnostics);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_assignment_reads(guard, state, diagnostics);
                }
                collect_definite_assignment(&arm.body, state, diagnostics);
            }
        }
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

/// Diagnose a control-flow condition whose checked HIR type is not `Bool`.
/// Unknown expression types are left to the resolving/type-checking passes.
pub fn bool_condition_diagnostic(expr: &HirExpr, construct: &str) -> Option<Diagnostic> {
    let type_name = expr_type_name(expr)?;
    if type_name == "Bool" {
        return None;
    }
    Some(
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("{construct} condition has type `{type_name}`, expected `Bool`."),
            expr_span(expr).clone(),
            "control-flow type mismatch",
        )
        .with_cause("RSScript control-flow conditions are explicit `Bool` values; non-empty strings, numbers, and managed handles do not coerce to truthy or falsey values.")
        .with_fix(
            "use_bool_condition",
            "Compare explicitly or call a function that returns `Bool`.",
            "manual",
        ),
    )
}

/// Diagnose a `for` iterable whose resolved type does not match the loop mode.
/// An unresolved iterable type remains the responsibility of earlier passes.
pub fn for_iterable_diagnostic(
    expr: &HirExpr,
    type_name: Option<&str>,
    is_async: bool,
) -> Option<Diagnostic> {
    let type_name = type_name?;
    let bare_type_name = strip_fresh_type(type_name);
    if (!is_async && generic_item_type(bare_type_name, "List").is_some())
        || (is_async && generic_item_type(bare_type_name, "Stream").is_some())
    {
        return None;
    }
    let expected = if is_async { "Stream<T>" } else { "List<T>" };
    let cause = if is_async {
        "RSScript `await for` iterates `Stream<T>` values by repeatedly awaiting `Stream.next`."
    } else {
        "RSScript v0.7 `for` iteration is limited to `List<T>` so loop ownership and review metadata stay explicit."
    };
    let fix_id = if is_async {
        "iterate_stream"
    } else {
        "iterate_list"
    };
    let fix = if is_async {
        "Iterate a `Stream<T>` value or convert the input to a Stream before the loop."
    } else {
        "Iterate a `List<T>` value or convert the input to a List before the loop."
    };
    Some(
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("for iterable has type `{type_name}`, expected `{expected}`."),
            expr_span(expr).clone(),
            "control-flow type mismatch",
        )
        .with_cause(cause)
        .with_fix(fix_id, fix, "manual"),
    )
}

/// Diagnose `match` expression arms that produce a value incompatible with the
/// resolved expression result type.
pub fn match_expression_arm_type_diagnostics(
    arms: &[HirMatchArm],
    expected_type: Option<&str>,
) -> Vec<Diagnostic> {
    let Some(expected_type) = expected_type else {
        return Vec::new();
    };
    arms.iter()
        .filter_map(|arm| {
            let arm_type = match_arm_value_type(&arm.body)?;
            (arm_type != expected_type).then(|| {
                Diagnostic::error(
                    code::CONTROL_FLOW_TYPE_MISMATCH,
                    format!(
                        "match arm has type `{arm_type}`, expected `{expected_type}` from the first produced arm."
                    ),
                    arm.span.clone(),
                    "match arm type mismatch",
                )
                .with_cause("A match expression must produce one compatible value type across every arm.")
                .with_fix(
                    "align_match_arm_types",
                    "Return the same value type from every match expression arm.",
                    "manual",
                )
            })
        })
        .collect()
}

/// Diagnose a `match` scrutinee outside the set of values with a review-visible
/// pattern model. Alias expansion and declared-type classification are supplied
/// by the semantic database caller.
pub fn match_scrutinee_diagnostic(
    expr: &HirExpr,
    type_name: Option<&str>,
    is_declared_pattern_type: bool,
) -> Option<Diagnostic> {
    let type_name = type_name?;
    let supported_root = matches!(type_root_name(type_name), "Option" | "Result" | "List");
    let supported_scalar = matches!(type_name, "Int" | "String" | "Char" | "Bool");
    if supported_root || supported_scalar || is_declared_pattern_type {
        return None;
    }
    Some(
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("match scrutinee has type `{type_name}`, expected `Option<T>`, `Result<T, E>`, `List<T>`, a declared sum/struct/class type, or an `Int`/`String`/`Char`/`Bool` literal match."),
            expr_span(expr).clone(),
            "control-flow type mismatch",
        )
        .with_cause("RSScript v0.7 `match` is limited to review-visible `Option`, `Result`, declared sum/struct/class patterns, and simple scalar literal dispatch.")
        .with_fix(
            "match_option_or_result",
            "Match an `Option<T>`, `Result<T, E>`, declared sum value, or scalar literal value; otherwise rewrite this branch as `if`.",
            "manual",
        ),
    )
}

/// Diagnose a literal pattern that cannot match the resolved scrutinee type.
pub fn match_literal_type_diagnostic(
    literal: &MatchLiteral,
    span: &rsscript_diagnostics::Span,
    scrutinee_type: &str,
) -> Option<Diagnostic> {
    let literal_type = match literal {
        MatchLiteral::Int(_) => "Int",
        MatchLiteral::String(_) => "String",
        MatchLiteral::Char(_) => "Char",
        MatchLiteral::Bool(_) => "Bool",
    };
    (literal_type != scrutinee_type).then(|| {
        Diagnostic::error(
            code::CONTROL_FLOW_TYPE_MISMATCH,
            format!("literal match pattern cannot match scrutinee type `{scrutinee_type}`."),
            span.clone(),
            "match literal type mismatch",
        )
        .with_cause(
            "Literal patterns are only allowed when matching `Int`, `String`, or `Bool` values.",
        )
    })
}

/// Diagnose a variant or structured pattern that does not belong to the
/// resolved scrutinee type.
pub fn match_pattern_type_diagnostic(
    pattern_name: &str,
    scrutinee_type: &str,
    span: &rsscript_diagnostics::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::CONTROL_FLOW_TYPE_MISMATCH,
        format!("match pattern `{pattern_name}` cannot match scrutinee type `{scrutinee_type}`."),
        span.clone(),
        "match pattern type mismatch",
    )
    .with_cause("RSScript match patterns must belong to the scrutinee's type.")
}

/// Diagnose a variant name outside the available family for a scrutinee type.
pub fn match_variant_family_diagnostic(
    variant_name: &str,
    scrutinee_type: &str,
    allowed: &[String],
    span: &rsscript_diagnostics::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::CONTROL_FLOW_TYPE_MISMATCH,
        format!("match pattern `{variant_name}` cannot match scrutinee type `{scrutinee_type}`."),
        span.clone(),
        "match variant type mismatch",
    )
    .with_cause("RSScript match patterns must belong to the scrutinee's type.")
    .with_fix(
        "match_matching_variant_family",
        format!(
            "Use variants of `{}`: {}.",
            type_root_name(scrutinee_type),
            allowed.join(", ")
        ),
        "manual",
    )
}

/// Diagnose positional variant bindings that do not match the declared arity.
pub fn variant_pattern_arity_diagnostic(
    variant_name: &str,
    expected: usize,
    found: usize,
    span: &rsscript_diagnostics::Span,
) -> Diagnostic {
    let field_word = if expected == 1 { "field" } else { "fields" };
    Diagnostic::error(
        code::VARIANT_PATTERN_ARITY_MISMATCH,
        format!(
            "variant pattern `{variant_name}` binds {found} sub-pattern(s) but `{variant_name}` declares {expected} {field_word}."
        ),
        span.clone(),
        "variant pattern arity mismatch",
    )
    .with_cause(
        "A positional variant pattern must bind exactly as many sub-patterns as the variant declares fields, in declared order.",
    )
}

/// Diagnose a structured match pattern that omits its explicit scrutinee data
/// effect. Literal and variant patterns do not project a field place and are
/// therefore outside this rule.
pub fn structured_match_effect_diagnostic(
    pattern: &MatchPattern,
    scrutinee_effect: Option<DataEffect>,
    arm_span: &rsscript_diagnostics::Span,
) -> Option<Diagnostic> {
    let is_structured = matches!(
        pattern,
        MatchPattern::Struct { .. } | MatchPattern::List { .. }
    );
    (is_structured && scrutinee_effect.is_none()).then(|| {
        Diagnostic::error(
            code::MISSING_DATA_EFFECT,
            "structured match patterns require an explicit scrutinee effect.",
            arm_span.clone(),
            "missing match scrutinee effect",
        )
        .with_cause(
            "A structured pattern projects fields from the scrutinee, so the source must spell `match read`, `match mut`, or `match take`.",
        )
        .with_fix(
            "spell_match_effect",
            "Add `read`, `mut`, or `take` after `match`.",
            "manual",
        )
    })
}

/// Diagnose a mutating effect inside a `match` guard. The caller supplies the
/// first effect fact and source span discovered during checked-HIR traversal.
pub fn match_guard_mutation_diagnostic(
    effect: DataEffect,
    span: &rsscript_diagnostics::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::READ_VIEW_MUTATION,
        format!("match guard cannot use `{}`.", effect.as_str()),
        span.clone(),
        "guard mutation is not allowed",
    )
    .with_cause("A guard runs before the arm is selected and may only read pattern bindings.")
    .with_fix(
        "make_guard_read_only",
        "Move mutation into the selected arm body or rewrite the guard as a read-only predicate.",
        "manual",
    )
}

/// Diagnose a structured pattern field that requests mutable/taking access to
/// a managed class value.
pub fn managed_pattern_field_effect_diagnostic(
    field_name: &str,
    effect: DataEffect,
    span: &rsscript_diagnostics::Span,
) -> Option<Diagnostic> {
    matches!(effect, DataEffect::Mut | DataEffect::Take).then(|| {
        Diagnostic::error(
            code::READ_VIEW_MUTATION,
            format!(
                "managed pattern field `{field_name}` cannot request `{}`.",
                effect.as_str()
            ),
            span.clone(),
            "managed pattern field is read-only",
        )
        .with_cause("Managed class values are shared runtime objects; structured patterns expose only read views of their fields.")
        .with_fix(
            "use_read_pattern",
            "Use a read field binding and perform managed mutation through an explicit method.",
            "manual",
        )
    })
}

/// Diagnose a child field effect that exceeds its match scrutinee effect.
pub fn weakened_pattern_field_effect_diagnostic(
    field_name: &str,
    effect: DataEffect,
    span: &rsscript_diagnostics::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::READ_VIEW_MUTATION,
        format!(
            "field pattern `{field_name}` requests `{}` from a weaker match scrutinee.",
            effect.as_str()
        ),
        span.clone(),
        "pattern effect is not allowed",
    )
    .with_cause("Pattern binding effects are monotonic: a child field cannot request more authority than the scrutinee effect provides.")
    .with_fix(
        "weaken_pattern_effect",
        "Use `read` for this field or strengthen the match scrutinee effect when the value is local and mutable.",
        "manual",
    )
}

/// Diagnose a repeated structured pattern field when either projection requests
/// mutable or taking access.
pub fn conflicting_pattern_field_effect_diagnostic(
    field_name: &str,
    span: &rsscript_diagnostics::Span,
    previous_span: &rsscript_diagnostics::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::FIELD_PARTIAL_ACCESS_CONFLICT,
        format!(
            "pattern field `{field_name}` is bound more than once with mutable or taking access."
        ),
        span.clone(),
        "pattern field conflict",
    )
    .with_cause(format!(
        "The previous binding for `{field_name}` was at {}:{}.",
        previous_span.line, previous_span.column
    ))
    .with_fix(
        "remove_overlapping_pattern_binding",
        "Bind each mutable or taking field place at most once in a pattern.",
        "manual",
    )
}

/// Diagnose a structured pattern field that is listed more than once.
pub fn duplicate_pattern_field_diagnostic(
    field_name: &str,
    span: &rsscript_diagnostics::Span,
    previous_span: &rsscript_diagnostics::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::FIELD_PARTIAL_ACCESS_CONFLICT,
        format!("pattern field `{field_name}` is listed more than once."),
        span.clone(),
        "duplicate pattern field",
    )
    .with_cause(format!(
        "The previous projection of `{field_name}` was at {}:{}.",
        previous_span.line, previous_span.column
    ))
    .with_fix(
        "remove_duplicate_pattern_field",
        "List each field at most once in a structured pattern.",
        "manual",
    )
}

/// Diagnose a structured pattern field absent from the resolved declaration.
pub fn unknown_pattern_field_diagnostic(
    field_name: &str,
    pattern_name: &str,
    span: &rsscript_diagnostics::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::UNKNOWN_FIELD,
        format!("unknown field `{field_name}` on type `{pattern_name}`."),
        span.clone(),
        "unknown field",
    )
    .with_cause("Structured match patterns may only project declared fields.")
    .with_fix(
        "use_declared_pattern_field",
        format!("Use a field declared on `{pattern_name}` or update the pattern."),
        "manual",
    )
}

/// Diagnose a structured pattern that omits declared fields without `..`.
pub fn omitted_pattern_fields_diagnostic(
    pattern_name: &str,
    span: &rsscript_diagnostics::Span,
) -> Diagnostic {
    Diagnostic::error(
        code::CONTROL_FLOW_TYPE_MISMATCH,
        format!("pattern `{pattern_name} {{ ... }}` omits fields without `..`."),
        span.clone(),
        "pattern omits fields",
    )
    .with_cause("Omitted fields must be visible in review; write `..` when intentionally ignoring the rest.")
    .with_fix(
        "add_pattern_rest",
        format!("Write `{pattern_name} {{ ..., .. }}` when omitting fields."),
        "manual",
    )
}

fn collect_bare_returns(
    block: &HirBlock,
    function: &FunctionDecl,
    expected: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Return {
                value: None, span, ..
            } => diagnostics.push(return_mismatch(function, expected, span)),
            HirStmt::With { body, .. } | HirStmt::Loop { body, .. } | HirStmt::For { body, .. } => {
                collect_bare_returns(body, function, expected, diagnostics)
            }
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_bare_returns(then_body, function, expected, diagnostics);
                if let Some(body) = else_body {
                    collect_bare_returns(body, function, expected, diagnostics);
                }
            }
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_bare_returns(&arm.body, function, expected, diagnostics);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_bare_returns(&arm.body, function, expected, diagnostics);
                }
            }
            _ => {}
        }
    }
}

fn return_mismatch(
    function: &FunctionDecl,
    expected: &str,
    span: &rsscript_syntax::Span,
) -> Diagnostic {
    Diagnostic::error(code::RETURN_TYPE_MISMATCH, format!("return in `{}` has type `Unit`, expected `{expected}`.", function.name), span.clone(), "return type mismatch")
        .with_cause("RSScript return types are part of the review contract and must be checked before Rust lowering.")
        .with_fix("match_return_type", format!("Return a value of type `{expected}` here."), "manual")
}

fn fallthrough_diagnostic(function: &FunctionDecl, hir: &Hir) -> Option<Diagnostic> {
    let return_ty = function.return_ty.as_ref()?;
    if !function.has_body || (return_ty.name == "Unit" && return_ty.args.is_empty()) {
        return None;
    }
    let generics = function
        .type_params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<HashSet<_>>();
    if type_mentions_generic(return_ty, &generics) {
        return None;
    }
    let body = hir.function_body(&function.name)?.block.as_ref()?;
    block_may_fall_through(body).then(|| {
        let expected = render_type_ref(return_ty);
        Diagnostic::error(code::RETURN_TYPE_MISMATCH, format!("return in `{}` has type `Unit`, expected `{expected}`.", function.name), function.span.clone(), "return type mismatch")
            .with_cause("RSScript return types are part of the review contract and must be checked before Rust lowering.")
            .with_fix("match_return_type", format!("Return a value of type `{expected}` here."), "manual")
    })
}

fn block_may_fall_through(block: &HirBlock) -> bool {
    block.statements.iter().all(statement_may_fall_through)
}
fn statement_may_fall_through(statement: &HirStmt) -> bool {
    match statement {
        HirStmt::Return { .. } | HirStmt::Break(_) | HirStmt::Continue(_) => false,
        HirStmt::If {
            then_body,
            else_body: Some(else_body),
            ..
        } => block_may_fall_through(then_body) || block_may_fall_through(else_body),
        HirStmt::Match { arms, .. } if !arms.is_empty() => {
            arms.iter().any(|arm| block_may_fall_through(&arm.body))
        }
        HirStmt::Select { arms, .. } if !arms.is_empty() => {
            arms.iter().any(|arm| block_may_fall_through(&arm.body))
        }
        HirStmt::With { body, .. } => block_may_fall_through(body),
        // `loop { … }` with no `break` targeting it never completes, so the
        // statements after it are unreachable and the function needs no
        // trailing `return` (spec §4.7, §6.3). `while`/`for` always may fall
        // through: their condition or iterator can be false/empty on entry.
        HirStmt::Loop {
            condition: None,
            body,
            ..
        } => block_breaks_enclosing_loop(body),
        _ => true,
    }
}

/// Whether `block` contains a `break` that targets the loop whose body it is.
///
/// `break` is unlabelled, so it binds to the innermost enclosing loop: a
/// `break` inside a nested `loop`/`while`/`for` does not target the outer one,
/// and a `break` inside a closure body cannot escape the closure at all.
fn block_breaks_enclosing_loop(block: &HirBlock) -> bool {
    block.statements.iter().any(statement_breaks_enclosing_loop)
}

fn statement_breaks_enclosing_loop(statement: &HirStmt) -> bool {
    match statement {
        HirStmt::Break(_) => true,
        // A `break` below this point targets the nested loop, not ours.
        HirStmt::Loop { .. } | HirStmt::For { .. } => false,
        HirStmt::Continue(_) | HirStmt::Unknown(_) => false,
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            value.as_ref().is_some_and(expr_breaks_enclosing_loop)
        }
        HirStmt::Expr(value) => expr_breaks_enclosing_loop(value),
        HirStmt::Assign { target, value, .. } => {
            expr_breaks_enclosing_loop(target) || expr_breaks_enclosing_loop(value)
        }
        HirStmt::With { resource, body, .. } => {
            expr_breaks_enclosing_loop(resource) || block_breaks_enclosing_loop(body)
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            expr_breaks_enclosing_loop(condition)
                || block_breaks_enclosing_loop(then_body)
                || else_body.as_ref().is_some_and(block_breaks_enclosing_loop)
        }
        HirStmt::Match { value, arms, .. } => {
            expr_breaks_enclosing_loop(value)
                || arms.iter().any(|arm| {
                    arm.guard.as_ref().is_some_and(expr_breaks_enclosing_loop)
                        || block_breaks_enclosing_loop(&arm.body)
                })
        }
        HirStmt::Select { arms, .. } => arms.iter().any(|arm| {
            expr_breaks_enclosing_loop(&arm.operation) || block_breaks_enclosing_loop(&arm.body)
        }),
    }
}

/// `break` is a statement, so an expression can only hold one inside a block it
/// carries: a `match` arm (which belongs to the enclosing loop) or a closure
/// body (which does not).
fn expr_breaks_enclosing_loop(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => false,
        HirExpr::ObjectLiteral { fields, .. } => fields
            .iter()
            .any(|field| expr_breaks_enclosing_loop(&field.value)),
        HirExpr::MapLiteral { entries, .. } => entries.iter().any(|entry| {
            expr_breaks_enclosing_loop(&entry.key) || expr_breaks_enclosing_loop(&entry.value)
        }),
        HirExpr::ArrayLiteral { items, .. } => items.iter().any(expr_breaks_enclosing_loop),
        HirExpr::Binary { left, right, .. } => {
            expr_breaks_enclosing_loop(left) || expr_breaks_enclosing_loop(right)
        }
        HirExpr::Field { base, .. } => expr_breaks_enclosing_loop(base),
        HirExpr::Index { base, index, .. } => {
            expr_breaks_enclosing_loop(base) || expr_breaks_enclosing_loop(index)
        }
        HirExpr::Call { receiver, args, .. } => {
            receiver
                .as_ref()
                .is_some_and(|receiver| expr_breaks_enclosing_loop(&receiver.value))
                || args
                    .iter()
                    .any(|arg| expr_breaks_enclosing_loop(&arg.value))
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => expr_breaks_enclosing_loop(value),
        HirExpr::Match { value, arms, .. } => {
            expr_breaks_enclosing_loop(value)
                || arms.iter().any(|arm| {
                    arm.guard.as_ref().is_some_and(expr_breaks_enclosing_loop)
                        || block_breaks_enclosing_loop(&arm.body)
                })
        }
    }
}
fn type_mentions_generic(ty: &TypeRef, generics: &HashSet<&str>) -> bool {
    generics.contains(ty.name.as_str())
        || ty
            .args
            .iter()
            .any(|arg| type_mentions_generic(arg, generics))
        || ty
            .fn_params
            .iter()
            .any(|arg| type_mentions_generic(arg, generics))
        || ty
            .fn_return
            .as_deref()
            .is_some_and(|ty| type_mentions_generic(ty, generics))
}
fn render_type_ref(ty: &TypeRef) -> String {
    if ty.args.is_empty() {
        ty.name.clone()
    } else {
        format!(
            "{}<{}>",
            ty.name,
            ty.args
                .iter()
                .map(render_type_ref)
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident {
            name, type_name, ..
        } => type_name.as_deref().or(match name.as_str() {
            "true" | "false" => Some("Bool"),
            "null" => Some("JsonLiteral"),
            "Unit" => Some("Unit"),
            "None" => Some("Option<?>"),
            _ => None,
        }),
        HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Number { value, .. } => Some(number_literal_type_name(value)),
        HirExpr::String { .. } => Some("String"),
        HirExpr::Char { .. } => Some("Char"),
        HirExpr::Binary { .. }
        | HirExpr::Index { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
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

fn strip_fresh_type(type_name: &str) -> &str {
    type_name.strip_prefix("fresh ").unwrap_or(type_name)
}

fn generic_item_type<'a>(type_name: &'a str, root: &str) -> Option<&'a str> {
    let inner = type_name
        .strip_prefix(&format!("{root}<"))
        .and_then(|type_name| type_name.strip_suffix('>'))?;
    (!inner.is_empty()).then_some(inner)
}

fn type_root_name(type_name: &str) -> &str {
    let type_name = type_name.trim();
    let type_name = type_name.strip_prefix("fresh ").unwrap_or(type_name);
    type_name
        .split_once('<')
        .map_or(type_name, |(root, _)| root)
}

fn match_arm_value_type(block: &HirBlock) -> Option<&str> {
    match block.statements.iter().next_back()? {
        HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => expr_type_name(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_syntax::parse_source;

    fn loop_control_codes(source: &str) -> Vec<String> {
        let program = parse_source("loop-control.rss", source);
        let hir = Hir::from_syntax(&program);
        let block = hir
            .function_body("check")
            .and_then(|body| body.block.as_ref())
            .expect("function body")
            .clone();
        loop_control_flow_diagnostics(&block)
            .into_iter()
            .map(|diagnostic| diagnostic.code.clone())
            .collect()
    }

    #[test]
    fn rejects_break_and_continue_with_no_enclosing_loop() {
        assert_eq!(
            loop_control_codes("fn check() -> Unit { if true { break } continue }"),
            [
                code::LOOP_CONTROL_OUTSIDE_LOOP,
                code::LOOP_CONTROL_OUTSIDE_LOOP
            ]
        );
    }

    #[test]
    fn accepts_break_and_continue_inside_loops() {
        assert!(
            loop_control_codes(
                "fn check(items: read List<Int>) -> Unit { while true { if true { break } } for item in items { continue } }"
            )
            .is_empty()
        );
    }

    #[test]
    fn a_closure_body_does_not_inherit_the_enclosing_loop() {
        assert_eq!(
            loop_control_codes("fn check() -> Unit { while true { let f = || { break } } }"),
            [code::LOOP_CONTROL_OUTSIDE_LOOP]
        );
    }

    fn definite_assignment_codes(source: &str) -> Vec<String> {
        let program = parse_source("definite-assignment.rss", source);
        let hir = Hir::from_syntax(&program);
        let Some(Item::Function(function)) = program
            .items
            .iter()
            .find(|item| matches!(item, Item::Function(function) if function.name == "check"))
        else {
            panic!("a `check` function");
        };
        let block = hir
            .function_body("check")
            .and_then(|body| body.block.as_ref())
            .expect("function body");
        definite_assignment_diagnostics(&function.body, block)
            .into_iter()
            .map(|diagnostic| diagnostic.code.clone())
            .collect()
    }

    #[test]
    fn rejects_a_read_of_a_binding_nothing_assigns() {
        assert_eq!(
            definite_assignment_codes(
                "fn check(out: mut List<Int>) -> Unit {\n    let x: Int\n    List.push(self: mut out, value: x)\n}"
            ),
            [code::READ_BEFORE_ASSIGNMENT]
        );
    }

    #[test]
    fn accepts_a_read_once_any_path_assigns() {
        // Assignment on one arm only is enough: the analysis reports a read
        // only when no path assigns at all.
        assert!(
            definite_assignment_codes(
                "fn check(flag: Bool, out: mut List<Int>) -> Unit {\n    let mut x: Int\n    if flag {\n        x = 1\n    }\n    List.push(self: mut out, value: x)\n}"
            )
            .is_empty()
        );
    }

    #[test]
    fn a_self_referential_assignment_reads_before_it_writes() {
        assert_eq!(
            definite_assignment_codes("fn check() -> Unit {\n    let mut x: Int\n    x = x + 1\n}"),
            [code::READ_BEFORE_ASSIGNMENT]
        );
    }

    #[test]
    fn a_let_else_binding_is_not_a_deferred_declaration() {
        assert!(
            definite_assignment_codes(
                "fn check(value: Option<String>) -> String {\n    let Some(inner) = value else {\n        return \"default\"\n    }\n    return inner\n}"
            )
            .is_empty()
        );
    }

    #[test]
    fn derives_non_exhaustive_match_diagnostics_from_resolved_facts() {
        let span = Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        assert_eq!(
            non_exhaustive_match_diagnostic(false, span.clone()).code,
            code::NON_EXHAUSTIVE_MATCH
        );
        assert_eq!(
            non_exhaustive_match_diagnostic(true, span).code,
            code::NON_EXHAUSTIVE_MATCH
        );
    }

    #[test]
    fn detects_non_unit_fallthrough() {
        let program = parse_source(
            "fallthrough.rss",
            "fn value() -> Int { let answer: Int = 1 }",
        );
        let hir = Hir::from_syntax(&program);
        let diagnostics = function_fallthrough_diagnostics(&program, &hir);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::RETURN_TYPE_MISMATCH);
    }

    #[test]
    fn infinite_loop_without_break_is_diverging() {
        let program = parse_source(
            "diverging-loop.rss",
            "fn serve(start: Int) -> Int {\n    let mut count = start\n    loop {\n        count = count + 1\n    }\n}",
        );
        let hir = Hir::from_syntax(&program);
        assert!(function_fallthrough_diagnostics(&program, &hir).is_empty());
    }

    #[test]
    fn infinite_loop_with_break_still_falls_through() {
        let program = parse_source(
            "breaking-loop.rss",
            "fn serve(start: Int) -> Int {\n    let mut count = start\n    loop {\n        if count > 1 {\n            break\n        }\n        count = count + 1\n    }\n}",
        );
        let hir = Hir::from_syntax(&program);
        let diagnostics = function_fallthrough_diagnostics(&program, &hir);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::RETURN_TYPE_MISMATCH);
    }

    #[test]
    fn break_in_a_nested_loop_does_not_target_the_outer_loop() {
        let program = parse_source(
            "nested-break.rss",
            "fn serve(start: Int) -> Int {\n    let mut count = start\n    loop {\n        loop {\n            break\n        }\n        count = count + 1\n    }\n}",
        );
        let hir = Hir::from_syntax(&program);
        assert!(function_fallthrough_diagnostics(&program, &hir).is_empty());
    }

    #[test]
    fn break_in_a_closure_does_not_target_the_enclosing_loop() {
        let program = parse_source(
            "closure-break.rss",
            "fn serve(start: Int) -> Int {\n    let mut count = start\n    loop {\n        let step = || {\n            break\n        }\n        count = count + 1\n    }\n}",
        );
        let hir = Hir::from_syntax(&program);
        assert!(function_fallthrough_diagnostics(&program, &hir).is_empty());
    }

    #[test]
    fn while_loop_still_falls_through() {
        let program = parse_source(
            "while-loop.rss",
            "fn serve(start: Int) -> Int {\n    let mut count = start\n    while count < 10 {\n        count = count + 1\n    }\n}",
        );
        let hir = Hir::from_syntax(&program);
        let diagnostics = function_fallthrough_diagnostics(&program, &hir);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::RETURN_TYPE_MISMATCH);
    }

    #[test]
    fn detects_nested_bare_return() {
        let program = parse_source(
            "bare-return.rss",
            "fn value() -> Int { if true { return } else { return 1 } }",
        );
        let hir = Hir::from_syntax(&program);
        let diagnostics = missing_return_value_diagnostics(&program, &hir);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::RETURN_TYPE_MISMATCH);
    }

    #[test]
    fn requires_explicit_boolean_control_flow_conditions() {
        let program = parse_source("condition.rss", "fn value() { if 1 {} }");
        let hir = Hir::from_syntax(&program);
        let body = hir
            .function_body("value")
            .and_then(|body| body.block.as_ref())
            .expect("function body");
        let HirStmt::If { condition, .. } = &body.statements[0] else {
            panic!("if statement")
        };
        let diagnostic = bool_condition_diagnostic(condition, "if").expect("must reject Int");
        assert_eq!(diagnostic.code, code::CONTROL_FLOW_TYPE_MISMATCH);

        let program = parse_source("condition.rss", "fn value() { if true {} }");
        let hir = Hir::from_syntax(&program);
        let body = hir
            .function_body("value")
            .and_then(|body| body.block.as_ref())
            .expect("function body");
        let HirStmt::If { condition, .. } = &body.statements[0] else {
            panic!("if statement")
        };
        assert!(bool_condition_diagnostic(condition, "if").is_none());
    }

    #[test]
    fn validates_sync_and_async_for_iterables() {
        let span = rsscript_diagnostics::Span {
            file: "loop.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        let value = HirExpr::Ident {
            name: "items".to_owned(),
            type_name: Some("Map<String, Int>".to_owned()),
            span: span.clone(),
        };
        let diagnostic = for_iterable_diagnostic(&value, Some("Map<String, Int>"), false)
            .expect("must reject a non-list sync iterable");
        assert_eq!(diagnostic.code, code::CONTROL_FLOW_TYPE_MISMATCH);
        assert!(for_iterable_diagnostic(&value, Some("fresh List<Int>"), false).is_none());
        assert!(for_iterable_diagnostic(&value, Some("Stream<Int>"), true).is_none());
    }

    #[test]
    fn validates_match_expression_arm_value_types() {
        let program = parse_source(
            "match.rss",
            r#"fn value(flag: Bool) -> Int {
                return match flag {
                    true => { 1 }
                    false => { "no" }
                }
            }"#,
        );
        let hir = Hir::from_syntax(&program);
        let body = hir
            .function_body("value")
            .and_then(|body| body.block.as_ref())
            .expect("function body");
        let HirStmt::Return {
            value: Some(HirExpr::Match {
                arms, type_name, ..
            }),
            ..
        } = &body.statements[0]
        else {
            panic!("match return")
        };
        let diagnostics = match_expression_arm_type_diagnostics(arms, type_name.as_deref());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::CONTROL_FLOW_TYPE_MISMATCH);
    }

    #[test]
    fn validates_match_scrutinee_types_from_resolved_facts() {
        let span = rsscript_diagnostics::Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        let value = HirExpr::Ident {
            name: "value".to_owned(),
            type_name: Some("Map<String, Int>".to_owned()),
            span,
        };
        let diagnostic = match_scrutinee_diagnostic(&value, Some("Map<String, Int>"), false)
            .expect("must reject Map patterns");
        assert_eq!(diagnostic.code, code::CONTROL_FLOW_TYPE_MISMATCH);
        assert!(match_scrutinee_diagnostic(&value, Some("Result<Int, Error>"), false).is_none());
        assert!(match_scrutinee_diagnostic(&value, Some("Token"), true).is_none());
    }

    #[test]
    fn validates_match_literal_types() {
        let span = rsscript_diagnostics::Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        let diagnostic =
            match_literal_type_diagnostic(&MatchLiteral::Int("1".to_owned()), &span, "String")
                .expect("must reject integer pattern for string");
        assert_eq!(diagnostic.code, code::CONTROL_FLOW_TYPE_MISMATCH);
        assert!(match_literal_type_diagnostic(&MatchLiteral::Bool(true), &span, "Bool").is_none());
    }

    #[test]
    fn reports_variant_pattern_family_and_arity_diagnostics() {
        let span = rsscript_diagnostics::Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        let family = match_variant_family_diagnostic(
            "Other",
            "Option<Int>",
            &["Some".to_owned(), "None".to_owned()],
            &span,
        );
        assert_eq!(family.code, code::CONTROL_FLOW_TYPE_MISMATCH);
        let arity = variant_pattern_arity_diagnostic("Some", 1, 2, &span);
        assert_eq!(arity.code, code::VARIANT_PATTERN_ARITY_MISMATCH);
        let pattern = match_pattern_type_diagnostic("[..]", "Int", &span);
        assert_eq!(pattern.code, code::CONTROL_FLOW_TYPE_MISMATCH);
    }

    #[test]
    fn requires_explicit_effect_for_structured_match_patterns() {
        let span = rsscript_diagnostics::Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        let pattern = MatchPattern::List {
            prefix: Vec::new(),
            rest: None,
            suffix: Vec::new(),
            span: span.clone(),
        };
        let diagnostic = structured_match_effect_diagnostic(&pattern, None, &span)
            .expect("structured patterns need an effect");
        assert_eq!(diagnostic.code, code::MISSING_DATA_EFFECT);
        assert!(
            structured_match_effect_diagnostic(&pattern, Some(DataEffect::Read), &span).is_none()
        );
    }

    #[test]
    fn rejects_mutating_match_guards() {
        let span = rsscript_diagnostics::Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        let diagnostic = match_guard_mutation_diagnostic(DataEffect::Take, &span);
        assert_eq!(diagnostic.code, code::READ_VIEW_MUTATION);
        assert!(diagnostic.summary.contains("take"));
    }

    #[test]
    fn validates_structured_pattern_field_effects() {
        let span = rsscript_diagnostics::Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        let managed = managed_pattern_field_effect_diagnostic("value", DataEffect::Mut, &span)
            .expect("managed fields are read-only");
        assert_eq!(managed.code, code::READ_VIEW_MUTATION);
        assert!(
            managed_pattern_field_effect_diagnostic("value", DataEffect::Read, &span).is_none()
        );
        let weaker = weakened_pattern_field_effect_diagnostic("value", DataEffect::Take, &span);
        assert_eq!(weaker.code, code::READ_VIEW_MUTATION);
    }

    #[test]
    fn reports_duplicate_and_conflicting_pattern_fields() {
        let previous = rsscript_diagnostics::Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 4,
            length: 1,
        };
        let current = rsscript_diagnostics::Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 12,
            length: 1,
        };
        let duplicate = duplicate_pattern_field_diagnostic("value", &current, &previous);
        assert_eq!(duplicate.code, code::FIELD_PARTIAL_ACCESS_CONFLICT);
        let conflict = conflicting_pattern_field_effect_diagnostic("value", &current, &previous);
        assert_eq!(conflict.code, code::FIELD_PARTIAL_ACCESS_CONFLICT);
    }

    #[test]
    fn reports_unknown_and_omitted_pattern_fields() {
        let span = rsscript_diagnostics::Span {
            file: "match.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        };
        let unknown = unknown_pattern_field_diagnostic("missing", "Record", &span);
        assert_eq!(unknown.code, code::UNKNOWN_FIELD);
        let omitted = omitted_pattern_fields_diagnostic("Record", &span);
        assert_eq!(omitted.code, code::CONTROL_FLOW_TYPE_MISMATCH);
    }
}
