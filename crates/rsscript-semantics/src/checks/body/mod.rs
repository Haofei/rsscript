use crate::text_util::{substitute_type_args, type_arg_names, type_root_name};
use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, Span, code};
use crate::hir::{
    CallResolution, FieldInfo, HirBindingKind, HirBlock, HirCallArg, HirExpr, HirMatchArm, HirStmt,
    HirTypeKind, ParamEffect, ResolvedCalleeKind,
};
use crate::syntax::ast::{
    Block as SyntaxBlock, Callee, DataEffect, Expr, FunctionDecl, Item, MatchFieldPattern,
    MatchLiteral, MatchPattern, Stmt as SyntaxStmt, TypeRef,
};

use super::local::{
    BodyState, FreshReturnIssueKind, LocalAnalysis, ResourceEscapeKind, merge_if_state,
    merge_loop_state,
};
pub(crate) use rsscript_semantics::Flow;
use rsscript_semantics::is_copy_type_name;

mod binding;
mod closure_captures;
mod effects;
mod fresh;
mod place;
mod resources;
mod semantics;
mod try_checks;

use binding::*;
use closure_captures::*;
use effects::*;
use fresh::*;
use place::*;
use resources::*;
use semantics::*;
use try_checks::*;

use crate::checks::shared::hir_expr_span;

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
                analyzer
                    .diagnostics
                    .extend(rsscript_semantics::await_placement_diagnostics(
                        block,
                        function.is_async,
                    ));
            }
            check_local_class_bindings(analyzer, body);
            check_uninferable_unused_bindings(analyzer, body);
            if let Some(block) = body.block.as_ref() {
                let bindings: std::collections::HashMap<String, HirBindingKind> = body
                    .bindings
                    .iter()
                    .map(|binding| (binding.name.clone(), binding.kind))
                    .collect();
                let binding_names = bindings.keys().cloned().collect::<HashSet<_>>();
                check_explicit_closure_captures_block(analyzer, block, &binding_names);
            }
        }
        let mut state = local_analysis.initial_state();
        if let Some(block) = hir_body.as_ref().and_then(|body| body.block.as_ref()) {
            // `?` needs a return type that can carry the failure case. Classify
            // the declared return type with aliases expanded; a return type that
            // names one of the function's own type parameters stays unclassified
            // so a generic helper is never wrongly rejected.
            let return_type = function.return_ty.as_ref().map(|return_ty| {
                let rendered = type_ref_name(return_ty);
                if function
                    .type_params
                    .iter()
                    .any(|param| param.name == rendered)
                {
                    String::new()
                } else {
                    analyzer.expand_type_alias(&rendered)
                }
            });
            analyzer
                .diagnostics
                .extend(rsscript_semantics::try_error_type_diagnostics(
                    block,
                    rsscript_semantics::TryContext::from_return_type(return_type.as_deref()),
                ));
            analyzer
                .diagnostics
                .extend(rsscript_semantics::loop_control_flow_diagnostics(block));
            analyzer
                .diagnostics
                .extend(rsscript_semantics::definite_assignment_diagnostics(
                    &function.body,
                    block,
                ));
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
