//! Semantic control-flow diagnostics over resolved HIR.

use crate::hir::{Hir, HirBlock, HirStmt};
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
}
