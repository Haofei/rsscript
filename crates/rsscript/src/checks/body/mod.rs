use crate::text_util::{
    split_top_level_type_args, strip_fresh_type, substitute_type_args, type_arg_names,
    type_root_name,
};
use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, FixEdit, Span, code};
use crate::hir::{
    CallResolution, FieldInfo, HirBindingKind, HirBlock, HirCallArg, HirExpr, HirMatchArm, HirStmt,
    HirTypeKind, ParamEffect, ResolvedCalleeKind,
};
use crate::syntax::ast::{
    Block as SyntaxBlock, Callee, DataEffect, Expr, FileFeature, FunctionDecl, Item,
    MatchFieldPattern, MatchLiteral, MatchPattern, Stmt as SyntaxStmt, TypeRef,
};

use super::local::{
    BodyState, FreshReturnIssue, FreshReturnIssueKind, LocalAnalysis, ManagedToLocalUse, MovedUse,
    ResourceEscapeKind, RetainedClosureCapture, RetainedLocalUse, TakeHandleField,
    is_copy_type_name, merge_if_state, merge_loop_state,
};

mod async_checks;
mod binding;
mod closure_captures;
mod effects;
mod fresh;
mod place;
mod resource_pool;
mod semantics;
mod try_checks;

use async_checks::*;
use binding::*;
use closure_captures::*;
use effects::*;
use fresh::*;
use place::*;
use resource_pool::*;
use semantics::*;
use try_checks::*;

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    let functions: Vec<FunctionDecl> = analyzer
        .syntax_program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function.clone()),
            Item::Type(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect();

    for function in functions {
        check_explicit_closure_capture_effects_syntax(analyzer, &function);
        analyzer.async_let_names.clear();
        let hir_body = analyzer.hir.function_body(&function.name).cloned();
        let local_analysis = LocalAnalysis::new(hir_body.as_ref());
        check_managed_to_local_uses(analyzer, &local_analysis);
        check_moved_uses(analyzer, &local_analysis);
        check_retained_local_uses(analyzer, &local_analysis);
        check_retained_closure_captures(analyzer, &local_analysis);
        check_take_handle_fields(analyzer, &local_analysis);
        check_fresh_returns(analyzer, &local_analysis, &function);
        if let Some(body) = &hir_body {
            if let Some(block) = body.block.as_ref() {
                check_await_placement(analyzer, block, function.is_async);
            }
            check_resource_pool_bindings(analyzer, body);
            check_local_class_bindings(analyzer, body);
            check_uninferable_unused_bindings(analyzer, body);
            if let Some(block) = body.block.as_ref() {
                check_resource_pool_discards(analyzer, block, &mut Vec::new());
                let bindings: std::collections::HashMap<String, HirBindingKind> = body
                    .bindings
                    .iter()
                    .map(|binding| (binding.name.clone(), binding.kind))
                    .collect();
                check_lazy_factory_captures_block(analyzer, block, &bindings);
                let binding_names = bindings.keys().cloned().collect::<HashSet<_>>();
                check_explicit_closure_captures_block(analyzer, block, &binding_names);
            }
        }
        let mut state = local_analysis.initial_state();
        if let Some(block) = hir_body.as_ref().and_then(|body| body.block.as_ref()) {
            let return_error_type = function
                .return_ty
                .as_ref()
                .and_then(result_error_type_ref_name);
            check_try_error_types(analyzer, block, return_error_type.as_deref());
            check_block(
                analyzer,
                &local_analysis,
                block,
                &mut state,
                true,
                &HashSet::new(),
            );
        }
    }
}


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Fallthrough,
    Return,
    Break,
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CallPlaceAccess {
    pub(super) effect: ParamEffect,
    pub(super) path: PlacePath,
    pub(super) moves_path: bool,
    pub(super) base_is_local: bool,
    pub(super) span: crate::diagnostic::Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlacePath {
    pub(super) base: String,
    pub(super) components: Vec<String>,
    pub(super) has_index: bool,
    pub(super) crosses_handle: bool,
}
