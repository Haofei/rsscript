//! Semantic control-flow diagnostics over resolved HIR.

use crate::hir::{Hir, HirBlock, HirExpr, HirMatchArm, HirStmt, number_literal_type_name};
use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{
    DataEffect, FunctionDecl, Item, MatchLiteral, MatchPattern, Program, TypeRef,
};
use std::collections::HashSet;

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
        _ => true,
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
}
