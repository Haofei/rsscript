//! Semantic control-flow diagnostics over resolved HIR.

use crate::hir::{Hir, HirBlock, HirExpr, HirStmt, number_literal_type_name};
use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{FunctionDecl, Item, Program, TypeRef};
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
}
