use crate::text_util::{
    builtin_generic_type_params, split_top_level_type_args, strip_fresh_type, type_arg_names,
    type_root_name,
};
use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::checks::diagnostic_helpers::error_cause_manual_fix;
use crate::checks::shared::builtin_value_type_name;
use crate::diagnostic::{Diagnostic, Span, code};
use crate::hir::{
    CallResolution, FunctionSig, HirBindingKind, HirBlock, HirCallArg, HirExpr, HirStmt, ParamSig,
    ResolvedCalleeKind,
};
use crate::syntax::ast::{
    BinaryOp, Callee, DataEffect, Expr, FunctionDecl, GenericBound, Item, TypeRef,
};

mod closure_contracts;
mod generic_constraints;
mod type_compatibility;

use closure_contracts::*;
use generic_constraints::*;
pub(crate) use type_compatibility::*;

#[derive(Debug, Clone)]
struct CallbackBinding {
    type_name: String,
    span: Span,
}

struct CallCheckContext<'a> {
    noescape_bindings: &'a HashMap<String, CallbackBinding>,
    callback_bindings: &'a HashMap<String, CallbackBinding>,
    callable_closure_bindings: &'a HashMap<String, Span>,
    local_closure_bindings: &'a HashMap<String, Span>,
}

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    analyzer
        .diagnostics
        .extend(rsscript_semantics::function_fallthrough_diagnostics(
            &analyzer.syntax_program,
            &analyzer.hir,
        ));
    analyzer
        .diagnostics
        .extend(rsscript_semantics::missing_return_value_diagnostics(
            &analyzer.syntax_program,
            &analyzer.hir,
        ));
    let items = analyzer.syntax_program.items.clone();
    for item in &items {
        if let Item::Function(function) = item {
            let Some(body) = analyzer.hir.function_body(&function.name).cloned() else {
                continue;
            };
            if !function.has_body {
                continue;
            }
            let callback_bindings = body
                .bindings
                .iter()
                .filter_map(|binding| {
                    binding.ty.as_ref().and_then(|ty| {
                        let type_name = ty.to_string();
                        is_fn_type(&type_name).then_some((
                            binding.name.clone(),
                            CallbackBinding {
                                type_name,
                                span: binding.span.clone(),
                            },
                        ))
                    })
                })
                .collect::<HashMap<_, _>>();
            let noescape_bindings = callback_bindings
                .iter()
                .filter_map(|(name, binding)| {
                    is_noescape_fn_type(&binding.type_name)
                        .then_some((name.clone(), binding.clone()))
                })
                .collect::<HashMap<_, _>>();
            if let Some(block) = &body.block {
                let mut local_closure_bindings = HashMap::new();
                let mut callable_closure_bindings = HashMap::new();
                collect_closure_bindings(
                    block,
                    &mut callable_closure_bindings,
                    &mut local_closure_bindings,
                );
                let context = CallCheckContext {
                    noescape_bindings: &noescape_bindings,
                    callback_bindings: &callback_bindings,
                    callable_closure_bindings: &callable_closure_bindings,
                    local_closure_bindings: &local_closure_bindings,
                };
                check_block(analyzer, function, block, &context);
            }
        }
    }
}

fn collect_closure_bindings(
    block: &HirBlock,
    callable_bindings: &mut HashMap<String, Span>,
    local_bindings: &mut HashMap<String, Span>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                kind: HirBindingKind::LocalLet,
                name,
                value: Some(HirExpr::Closure { body, explicit, .. }),
                span,
                ..
            } => {
                callable_bindings.insert(name.clone(), span.clone());
                if !explicit {
                    local_bindings.insert(name.clone(), span.clone());
                }
                collect_closure_bindings(body, callable_bindings, local_bindings);
            }
            HirStmt::Let {
                value: Some(HirExpr::Closure { body, .. }),
                ..
            } => collect_closure_bindings(body, callable_bindings, local_bindings),
            HirStmt::Let { .. }
            | HirStmt::Return { .. }
            | HirStmt::Expr(_)
            | HirStmt::Assign { .. } => {}
            HirStmt::With { body, .. } => {
                collect_closure_bindings(body, callable_bindings, local_bindings)
            }
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_closure_bindings(then_body, callable_bindings, local_bindings);
                if let Some(else_body) = else_body {
                    collect_closure_bindings(else_body, callable_bindings, local_bindings);
                }
            }
            HirStmt::Loop { body, .. } => {
                collect_closure_bindings(body, callable_bindings, local_bindings)
            }
            HirStmt::For { body, .. } => {
                collect_closure_bindings(body, callable_bindings, local_bindings)
            }
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_closure_bindings(&arm.body, callable_bindings, local_bindings);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_closure_bindings(&arm.body, callable_bindings, local_bindings);
                }
            }
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
        }
    }
}

fn check_block(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    block: &HirBlock,
    context: &CallCheckContext<'_>,
) {
    let Some(_recursion) = analyzer.budget.enter_recursion() else {
        return;
    };
    if !analyzer.budget.consume_nodes(block.statements.len().max(1)) {
        return;
    }
    let noescape_bindings = context.noescape_bindings;
    let local_closure_bindings = context.local_closure_bindings;
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                value: Some(value),
                ty,
                value_ty,
                name,
                span,
                ..
            } => {
                check_binding_type(
                    analyzer,
                    name,
                    &ty.as_ref().map(ToString::to_string),
                    &value_ty.as_ref().map(ToString::to_string),
                    value,
                );
                check_noescape_escape(
                    analyzer,
                    value,
                    span,
                    noescape_bindings,
                    NoescapeEscapeContext::Store,
                );
                check_local_closure_escape(
                    analyzer,
                    value,
                    span,
                    local_closure_bindings,
                    LocalClosureEscapeContext::Store,
                );
                check_expr(analyzer, function, value, context);
            }
            HirStmt::Return {
                value: Some(value), ..
            } => {
                check_return_type(analyzer, function, Some(value), hir_expr_span(value));
                check_noescape_escape(
                    analyzer,
                    value,
                    hir_expr_span(value),
                    noescape_bindings,
                    NoescapeEscapeContext::Return,
                );
                check_local_closure_escape(
                    analyzer,
                    value,
                    hir_expr_span(value),
                    local_closure_bindings,
                    LocalClosureEscapeContext::Return,
                );
                check_expr(analyzer, function, value, context);
            }
            HirStmt::Return { value: None, .. } => {}
            HirStmt::Expr(value) | HirStmt::Assign { value, .. } => {
                check_noescape_escape(
                    analyzer,
                    value,
                    hir_expr_span(value),
                    noescape_bindings,
                    NoescapeEscapeContext::UseAsValue,
                );
                check_local_closure_escape(
                    analyzer,
                    value,
                    hir_expr_span(value),
                    local_closure_bindings,
                    LocalClosureEscapeContext::UseAsValue,
                );
                check_expr(analyzer, function, value, context);
            }
            HirStmt::With { resource, body, .. } => {
                check_expr(analyzer, function, resource, context);
                check_block(analyzer, function, body, context);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                check_expr(analyzer, function, condition, context);
                check_block(analyzer, function, then_body, context);
                if let Some(else_body) = else_body {
                    check_block(analyzer, function, else_body, context);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    check_expr(analyzer, function, condition, context);
                }
                check_block(analyzer, function, body, context);
            }
            HirStmt::For { iterable, body, .. } => {
                check_expr(analyzer, function, iterable, context);
                check_block(analyzer, function, body, context);
            }
            HirStmt::Match { value, arms, .. } => {
                check_expr(analyzer, function, value, context);
                for arm in arms {
                    check_block(analyzer, function, &arm.body, context);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    check_expr(analyzer, function, &arm.operation, context);
                    check_block(analyzer, function, &arm.body, context);
                }
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn check_binding_type(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    expected: &Option<String>,
    actual: &Option<String>,
    value: &HirExpr,
) {
    let Some(expected) = expected.as_deref() else {
        return;
    };
    if unresolved_generic_type(analyzer, expected) {
        return;
    }
    if check_variant_payload_type(
        analyzer,
        &PayloadCheckSite::Binding { name },
        expected,
        value,
    ) {
        return;
    }
    let Some(actual) = actual.as_deref() else {
        return;
    };
    if unresolved_generic_type(analyzer, actual) {
        return;
    }
    let resolved_expected = analyzer.expand_type_alias(expected);
    let resolved_actual = analyzer.expand_type_alias(actual);
    if json_value_accepts_literal(&resolved_expected, value) {
        return;
    }
    if check_map_literal_type(analyzer, &resolved_expected, value, "binding initializer") {
        return;
    }
    if check_list_literal_type(analyzer, &resolved_expected, value, "binding initializer") {
        return;
    }
    if argument_type_matches(&resolved_expected, &resolved_actual) {
        return;
    }
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::ARGUMENT_TYPE_MISMATCH,
        format!("binding `{name}` has initializer type `{actual}`, expected `{expected}`."),
        hir_expr_span(value).clone(),
        "binding type mismatch",
        "Explicit `let` and `local` type annotations are source-level contracts and must match the initializer before Rust lowering.",
        "match_binding_type",
        format!("Initialize `{name}` with a `{expected}` value, or change the binding annotation."),
    ));
}

/// Identifies which call site is performing the Result/Option variant
/// payload-type check, so the shared logic can emit site-specific diagnostics
/// and literal-context labels while keeping identical structure.
enum PayloadCheckSite<'a> {
    Binding {
        name: &'a str,
    },
    Argument {
        call_name: &'a str,
        arg_name: &'a str,
    },
}

impl PayloadCheckSite<'_> {
    fn map_literal_label(&self) -> &'static str {
        match self {
            PayloadCheckSite::Binding { .. } => "binding payload",
            PayloadCheckSite::Argument { .. } => "argument payload",
        }
    }

    fn list_literal_label(&self) -> &'static str {
        self.map_literal_label()
    }

    fn push_mismatch(
        &self,
        analyzer: &mut Analyzer<'_>,
        actual: &str,
        expected: &str,
        span: &Span,
    ) {
        match self {
            PayloadCheckSite::Binding { name } => {
                analyzer.diagnostics.push(error_cause_manual_fix(
                    code::ARGUMENT_TYPE_MISMATCH,
                    format!(
                        "binding `{name}` has initializer payload type `{actual}`, expected `{expected}`."
                    ),
                    span.clone(),
                    "binding type mismatch",
                    "Result and Option binding initializers are checked against explicit binding payload types before Rust lowering.",
                    "match_binding_payload_type",
                    format!("Initialize `{name}` with a `{expected}` payload, or change the binding annotation."),
                ));
            }
            PayloadCheckSite::Argument {
                call_name,
                arg_name,
            } => {
                analyzer.diagnostics.push(error_cause_manual_fix(
                    code::ARGUMENT_TYPE_MISMATCH,
                    format!(
                        "argument `{arg_name}` for `{call_name}` has payload type `{actual}`, expected `{expected}`."
                    ),
                    span.clone(),
                    "argument type mismatch",
                    "Result and Option argument constructors are checked against the resolved parameter payload before Rust lowering.",
                    "match_argument_payload_type",
                    format!("Pass a `{expected}` payload for `{arg_name}`."),
                ));
            }
        }
    }
}

/// Shared dispatcher for the Result/Option variant payload-type check used by
/// both binding initializers and call arguments. Returns `true` when `value` is
/// a Result/Option variant constructor whose payload was checked (so the caller
/// should skip its ordinary type check), `false` otherwise. Behavior is
/// identical across sites; only the emitted diagnostics differ (see
/// `PayloadCheckSite`).
fn check_variant_payload_type(
    analyzer: &mut Analyzer<'_>,
    site: &PayloadCheckSite<'_>,
    expected_type: &str,
    value: &HirExpr,
) -> bool {
    let Some((variant, payload)) = enum_variant_payload(value) else {
        return false;
    };
    match (type_root_name(expected_type), variant) {
        ("Option", "None") => true,
        ("Option", "Some") => {
            let Some(expected) = type_arg_names(expected_type)
                .and_then(|args| args.first().map(|arg| arg.trim().to_string()))
            else {
                return false;
            };
            if let Some(payload) = payload {
                check_payload_type(analyzer, site, payload, &expected);
            } else if expected != "Unit" {
                site.push_mismatch(analyzer, "Unit", &expected, hir_expr_span(value));
            }
            true
        }
        ("Result", "Ok" | "Err") => {
            let Some(args) = type_arg_names(expected_type) else {
                return false;
            };
            let expected = match variant {
                "Ok" => args.first().copied(),
                "Err" => args.get(1).copied(),
                _ => None,
            };
            let Some(expected) = expected else {
                return false;
            };
            let expected = expected.trim();
            if let Some(payload) = payload {
                check_payload_type(analyzer, site, payload, expected);
            } else if expected != "Unit" {
                site.push_mismatch(analyzer, "Unit", expected, hir_expr_span(value));
            }
            true
        }
        _ => false,
    }
}

/// Shared recursive payload-type check for the binding and argument sites:
/// unwraps nested Result/Option constructors, accepts literals, and otherwise
/// compares the payload type against `expected`.
fn check_payload_type(
    analyzer: &mut Analyzer<'_>,
    site: &PayloadCheckSite<'_>,
    payload: &HirExpr,
    expected: &str,
) {
    if let Some((variant, nested_payload)) = enum_variant_payload(payload)
        && let Some(expected_payload) = expected_variant_payload_type(expected, variant)
    {
        if let Some(nested_payload) = nested_payload {
            check_payload_type(analyzer, site, nested_payload, &expected_payload);
        } else if expected_payload != "Unit" {
            site.push_mismatch(analyzer, "Unit", &expected_payload, hir_expr_span(payload));
        }
        return;
    }
    if json_value_accepts_literal(expected, payload) {
        return;
    }
    if check_map_literal_type(analyzer, expected, payload, site.map_literal_label()) {
        return;
    }
    if check_list_literal_type(analyzer, expected, payload, site.list_literal_label()) {
        return;
    }
    let Some(actual) = hir_expr_type_name(payload) else {
        return;
    };
    if unresolved_generic_type(analyzer, actual) {
        return;
    }
    if !argument_type_matches(expected, actual) {
        site.push_mismatch(analyzer, actual, expected, hir_expr_span(payload));
    }
}

fn check_expr(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    expr: &HirExpr,
    context: &CallCheckContext<'_>,
) {
    let Some(_recursion) = analyzer.budget.enter_recursion() else {
        return;
    };
    if !analyzer.budget.consume_nodes(1) {
        return;
    }
    match expr {
        HirExpr::Call {
            callee,
            args,
            span,
            resolution,
            ..
        } => {
            check_call_args(analyzer, function, callee, args, span, resolution, context);
            for arg in args {
                check_expr(analyzer, function, &arg.value, context);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_expr(analyzer, function, value, context);
        }
        HirExpr::Binary { left, right, .. } => {
            check_expr(analyzer, function, left, context);
            check_expr(analyzer, function, right, context);
        }
        HirExpr::Field { base, .. } => check_expr(analyzer, function, base, context),
        HirExpr::Index { base, index, .. } => {
            check_expr(analyzer, function, base, context);
            check_expr(analyzer, function, index, context);
        }
        HirExpr::Closure { body, .. } => {
            // Closure bodies use the enclosing function's return contract only when they
            // are lowered as ordinary statements. noescape callback return contracts are
            // checked at their call/parameter boundary.
            check_expr_block_without_return_contract(analyzer, function, body, context)
        }
        HirExpr::Match { value, arms, .. } => {
            check_expr(analyzer, function, value, context);
            for arm in arms {
                check_expr_block_without_return_contract(analyzer, function, &arm.body, context);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_expr(analyzer, function, &entry.key, context);
                check_expr(analyzer, function, &entry.value, context);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn check_expr_block_without_return_contract(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    block: &HirBlock,
    context: &CallCheckContext<'_>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                check_expr(analyzer, function, value, context);
            }
            HirStmt::With { resource, body, .. } => {
                check_expr(analyzer, function, resource, context);
                check_expr_block_without_return_contract(analyzer, function, body, context);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                check_expr(analyzer, function, condition, context);
                check_expr_block_without_return_contract(analyzer, function, then_body, context);
                if let Some(else_body) = else_body {
                    check_expr_block_without_return_contract(
                        analyzer, function, else_body, context,
                    );
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    check_expr(analyzer, function, condition, context);
                }
                check_expr_block_without_return_contract(analyzer, function, body, context);
            }
            HirStmt::For { iterable, body, .. } => {
                check_expr(analyzer, function, iterable, context);
                check_expr_block_without_return_contract(analyzer, function, body, context);
            }
            HirStmt::Match { value, arms, .. } => {
                check_expr(analyzer, function, value, context);
                for arm in arms {
                    check_expr_block_without_return_contract(
                        analyzer, function, &arm.body, context,
                    );
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    check_expr(analyzer, function, &arm.operation, context);
                    check_expr_block_without_return_contract(
                        analyzer, function, &arm.body, context,
                    );
                }
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn check_return_type(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    value: Option<&HirExpr>,
    span: &Span,
) {
    let Some(return_ty) = &function.return_ty else {
        return;
    };
    let function_type_params = function
        .type_params
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    if type_contains_unresolved_generic(&type_ref_name(return_ty), &function_type_params) {
        return;
    }

    match value {
        None => {
            if return_ty.name != "Unit" {
                return_type_mismatch_diagnostic(
                    analyzer,
                    &function.name,
                    "Unit",
                    &type_ref_name(return_ty),
                    span,
                );
            }
        }
        Some(value) => {
            check_return_expr_type(analyzer, function, return_ty, value, hir_expr_span(value));
        }
    }
}

fn check_return_expr_type(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    return_ty: &TypeRef,
    value: &HirExpr,
    span: &Span,
) {
    if return_ty.name == "Result" && return_ty.args.len() == 2 {
        check_result_return_expr_type(analyzer, function, return_ty, value, span);
        return;
    }
    if return_ty.name == "Option" && return_ty.args.len() == 1 {
        check_option_return_expr_type(analyzer, function, return_ty, value, span);
        return;
    }

    let expected = type_ref_name(return_ty);
    let Some(actual) = hir_expr_type_name(value) else {
        return;
    };
    if unresolved_generic_type(analyzer, actual) {
        return;
    }
    if json_value_accepts_literal(&expected, value) {
        return;
    }
    if check_map_literal_type(analyzer, &expected, value, "return value") {
        return;
    }
    if check_list_literal_type(analyzer, &expected, value, "return value") {
        return;
    }
    if !argument_type_matches(
        &analyzer.expand_type_alias(&expected),
        &analyzer.expand_type_alias(actual),
    ) {
        return_type_mismatch_diagnostic(analyzer, &function.name, actual, &expected, span);
    }
}

fn check_result_return_expr_type(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    return_ty: &TypeRef,
    value: &HirExpr,
    span: &Span,
) {
    let ok_ty = type_ref_name(&return_ty.args[0]);
    let err_ty = type_ref_name(&return_ty.args[1]);
    match enum_variant_payload(value) {
        Some(("Ok", Some(payload))) => {
            check_return_payload_type(analyzer, function, payload, &ok_ty, "Ok payload");
        }
        Some(("Err", Some(payload))) => {
            check_return_payload_type(analyzer, function, payload, &err_ty, "Err payload");
        }
        Some(("Ok", None)) => {
            if ok_ty != "Unit" {
                return_type_mismatch_diagnostic(analyzer, &function.name, "Unit", &ok_ty, span);
            }
        }
        Some(("Err", None)) => {
            if err_ty != "Unit" {
                return_type_mismatch_diagnostic(analyzer, &function.name, "Unit", &err_ty, span);
            }
        }
        _ => {
            let Some(actual) = hir_expr_type_name(value) else {
                return;
            };
            if unresolved_generic_type(analyzer, actual) {
                return;
            }
            let expected_result = type_ref_name(return_ty);
            if is_result_type_name(actual) {
                if !argument_type_matches(&expected_result, actual) {
                    return_type_mismatch_diagnostic(
                        analyzer,
                        &function.name,
                        actual,
                        &expected_result,
                        span,
                    );
                }
            } else if !argument_type_matches(&ok_ty, actual) {
                return_type_mismatch_diagnostic(analyzer, &function.name, actual, &ok_ty, span);
            }
        }
    }
}

fn check_option_return_expr_type(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    return_ty: &TypeRef,
    value: &HirExpr,
    span: &Span,
) {
    let some_ty = type_ref_name(&return_ty.args[0]);
    match enum_variant_payload(value) {
        Some(("Some", Some(payload))) => {
            check_return_payload_type(analyzer, function, payload, &some_ty, "Some payload");
        }
        Some(("Some", None)) => {
            if some_ty != "Unit" {
                return_type_mismatch_diagnostic(analyzer, &function.name, "Unit", &some_ty, span);
            }
        }
        Some(("None", _)) => {}
        _ => {
            let Some(actual) = hir_expr_type_name(value) else {
                return;
            };
            if unresolved_generic_type(analyzer, actual) {
                return;
            }
            let expected_option = type_ref_name(return_ty);
            if is_option_type_name(actual) {
                if !argument_type_matches(&expected_option, actual) {
                    return_type_mismatch_diagnostic(
                        analyzer,
                        &function.name,
                        actual,
                        &expected_option,
                        span,
                    );
                }
            } else {
                return_type_mismatch_diagnostic(
                    analyzer,
                    &function.name,
                    actual,
                    &expected_option,
                    span,
                );
            }
        }
    }
}

fn check_return_payload_type(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    payload: &HirExpr,
    expected: &str,
    label: &str,
) {
    if let Some((variant, nested_payload)) = enum_variant_payload(payload)
        && let Some(expected_payload) = expected_variant_payload_type(expected, variant)
    {
        if let Some(nested_payload) = nested_payload {
            check_return_payload_type(
                analyzer,
                function,
                nested_payload,
                &expected_payload,
                nested_payload_label(variant),
            );
        } else if expected_payload != "Unit" {
            return_payload_type_mismatch_diagnostic(
                analyzer,
                function,
                "Unit",
                &expected_payload,
                nested_payload_label(variant),
                hir_expr_span(payload),
            );
        }
        return;
    }
    if json_value_accepts_literal(expected, payload) {
        return;
    }
    if check_map_literal_type(analyzer, expected, payload, "return payload") {
        return;
    }
    if check_list_literal_type(analyzer, expected, payload, "return payload") {
        return;
    }
    let Some(actual) = hir_expr_type_name(payload) else {
        return;
    };
    if unresolved_generic_type(analyzer, actual) {
        return;
    }
    if !argument_type_matches(expected, actual) {
        return_payload_type_mismatch_diagnostic(
            analyzer,
            function,
            actual,
            expected,
            label,
            hir_expr_span(payload),
        );
    }
}

fn return_payload_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    actual: &str,
    expected: &str,
    label: &str,
    span: &Span,
) {
    analyzer
        .diagnostics
        .push(rsscript_semantics::return_payload_type_mismatch_diagnostic(
            &function.name,
            actual,
            expected,
            label,
            span.clone(),
        ));
}

fn enum_variant_payload(expr: &HirExpr) -> Option<(&'static str, Option<&HirExpr>)> {
    if let HirExpr::Effect { value, .. }
    | HirExpr::Manage { value, .. }
    | HirExpr::Try { value, .. } = expr
    {
        return enum_variant_payload(value);
    }
    let HirExpr::Call { callee, args, .. } = expr else {
        return None;
    };
    let variant = match callee_name(callee).as_str() {
        "Ok" => "Ok",
        "Err" => "Err",
        "Some" => "Some",
        "None" => "None",
        _ => return None,
    };
    match variant {
        "Ok" | "Err" | "Some" if args.len() == 1 && args[0].name.is_none() => {
            Some((variant, Some(&args[0].value)))
        }
        _ => None,
    }
}

fn expected_variant_payload_type(expected_type: &str, variant: &str) -> Option<String> {
    match (type_root_name(expected_type), variant) {
        ("Option", "None") => Some("Unit".to_string()),
        ("Option", "Some") => type_arg_names(expected_type)
            .and_then(|args| args.first().map(|arg| arg.trim().to_string())),
        ("Result", "Ok" | "Err") => {
            let args = type_arg_names(expected_type)?;
            let expected = match variant {
                "Ok" => args.first().copied(),
                "Err" => args.get(1).copied(),
                _ => None,
            }?;
            Some(expected.trim().to_string())
        }
        _ => None,
    }
}

fn fresh_type_target(type_name: &str) -> Option<&str> {
    type_name.trim().strip_prefix("fresh ").map(str::trim)
}

fn nested_payload_label(variant: &str) -> &'static str {
    match variant {
        "Ok" => "Ok payload",
        "Err" => "Err payload",
        "Some" => "Some payload",
        "None" => "None payload",
        _ => "payload",
    }
}

fn return_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    function_name: &str,
    actual: &str,
    expected: &str,
    span: &Span,
) {
    analyzer
        .diagnostics
        .push(rsscript_semantics::return_type_mismatch_diagnostic(
            function_name,
            actual,
            expected,
            span.clone(),
        ));
}

fn check_call_args(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    callee: &Callee,
    args: &[HirCallArg],
    call_span: &Span,
    resolution: &CallResolution,
    context: &CallCheckContext<'_>,
) {
    let noescape_bindings = context.noescape_bindings;
    let callback_bindings = context.callback_bindings;
    let local_closure_bindings = context.local_closure_bindings;
    let callable_closure_bindings = context.callable_closure_bindings;
    let call_name = callee_display(callee);
    if is_closure_binding_call(
        callee,
        args,
        resolution,
        callback_bindings,
        callable_closure_bindings,
    ) {
        check_callback_call_args(analyzer, callee, args, callback_bindings);
        return;
    }
    if matches!(resolution, CallResolution::EnumVariant) {
        check_enum_variant_form(analyzer, callee, args, call_span);
        return;
    }
    check_dyn_from_call(analyzer, function, callee, args, call_span);

    let is_receiver_call = matches!(callee, Callee::ReceiverCall { .. });
    let signature = match resolution {
        CallResolution::Resolved { signature, .. } => signature,
        CallResolution::Unknown => {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_CALLEE,
                    format!("call to `{}` does not resolve.", callee_display(callee)),
                    call_span.clone(),
                    "unknown callee",
                )
                .with_cause(
                    "The callee is not a user function, known type constructor, enum variant, or builtin signature.",
                )
                .with_fix(
                    "declare_or_import_callee",
                    "Declare the function or add a builtin signature for this API.",
                    "manual",
                ),
            );
            return;
        }
        CallResolution::Ambiguous { candidates } => {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_CALLEE,
                    format!(
                        "receiver-call `{}` is ambiguous between {}.",
                        callee_display(callee),
                        candidates.join(", ")
                    ),
                    call_span.clone(),
                    "ambiguous receiver call",
                )
                .with_cause(
                    "Receiver-call shorthand is only allowed when exactly one inherent or protocol method candidate is visible.",
                )
                .with_fix(
                    "use_canonical_call",
                    "Write the canonical qualified call explicitly.",
                    "manual",
                ),
            );
            return;
        }
        CallResolution::EnumVariant => return,
    };
    let allow_positional_args = is_receiver_call
        || matches!(
            resolution,
            CallResolution::Resolved {
                signature,
                kind: ResolvedCalleeKind::UserFunction,
            } if !signature.is_public
        );
    let allow_constructor_field_shorthand = matches!(
        resolution,
        CallResolution::Resolved {
            kind: ResolvedCalleeKind::Constructor { .. },
            ..
        }
    );
    if let Callee::ReceiverCall {
        receiver,
        method,
        effect,
    } = callee
    {
        let receiver_fact = rsscript_semantics::ReceiverCallEffectFact {
            callee_display: callee_display(callee),
            method: method.clone(),
            receiver_label: call_expr_label(receiver),
            supplied_effect: (*effect).unwrap_or(DataEffect::Read).as_str(),
            receiver_parameter_declared: !signature.params.is_empty(),
            expected_effect: signature
                .params
                .first()
                .and_then(|parameter| parameter.effect.map(|effect| effect.as_str())),
            span: call_span.clone(),
        };
        analyzer
            .diagnostics
            .extend(rsscript_semantics::receiver_call_effect_diagnostics(
                &receiver_fact,
            ));
    }
    // For receiver-call shorthand, the receiver slot is provided implicitly.
    // Protocol methods conventionally name it `self`; core namespace functions
    // may keep their canonical parameter name, e.g. `List.push(list: mut List<T>, ...)`.
    let signature_params: Vec<_> = if is_receiver_call {
        signature.params.iter().skip(1).cloned().collect()
    } else {
        signature.params.clone()
    };
    check_protocol_receiver_satisfaction(analyzer, function, callee, args, call_span);
    let param_names: HashSet<String> = signature_params
        .iter()
        .map(|param| param.name.clone())
        .collect();

    // Resolve each argument's target parameter name exactly once and reuse it
    // across every per-arg validation phase below. The resolution depends only on
    // the argument, its position, and these fixed inputs, so it is identical in
    // each phase — sharing it does not change which diagnostics fire or in what
    // order.
    let resolved_names: Vec<Option<&str>> = args
        .iter()
        .enumerate()
        .map(|(index, arg)| {
            let shorthand = constructor_field_shorthand_name(
                allow_constructor_field_shorthand,
                arg,
                &param_names,
            );
            let binding_is_applicable =
                arg.name.is_some() || allow_positional_args || shorthand.is_some();
            arg.parameter_index
                .filter(|_| binding_is_applicable)
                .and_then(|parameter_index| signature.params.get(parameter_index))
                .map(|parameter| parameter.name.as_str())
                .or_else(|| {
                    resolved_arg_param_name(
                        arg,
                        index,
                        allow_constructor_field_shorthand,
                        allow_positional_args,
                        &param_names,
                        &signature_params,
                    )
                })
        })
        .collect();

    let parameter_facts = signature
        .params
        .iter()
        .enumerate()
        .map(|(index, parameter)| rsscript_semantics::CallParameterFact {
            accepts_argument: !is_receiver_call || index != 0,
            required: !is_receiver_call || index != 0,
            name: parameter.name.clone(),
            effect: parameter.effect.map(|effect| effect.as_str()),
        })
        .collect::<Vec<_>>();
    let argument_facts = args
        .iter()
        .zip(&resolved_names)
        .map(
            |(argument, resolved_name)| rsscript_semantics::CallArgumentFact {
                explicit_name: argument.name.is_some(),
                resolved_name: resolved_name.map(str::to_owned),
                span: argument.span.clone(),
                value_span: hir_expr_span(&argument.value).clone(),
                constructor_shorthand: constructor_field_shorthand_name(
                    allow_constructor_field_shorthand,
                    argument,
                    &param_names,
                )
                .is_some(),
                effect: expr_data_effect(&argument.value),
            },
        )
        .collect::<Vec<_>>();
    analyzer
        .diagnostics
        .extend(rsscript_semantics::call_argument_diagnostics(
            &call_name,
            call_span,
            allow_positional_args,
            &parameter_facts,
            &argument_facts,
        ));

    let Some(type_param_substitutions) =
        call_type_param_substitutions(analyzer, Some(function), callee, args, signature)
    else {
        return;
    };
    check_generic_call_bounds(
        analyzer,
        function,
        callee,
        &call_name,
        signature,
        &type_param_substitutions,
        call_span,
    );
    check_message_channel_payload(
        analyzer,
        function,
        &call_name,
        signature,
        &type_param_substitutions,
        call_span,
    );

    check_argument_types(
        analyzer,
        args,
        &call_name,
        signature,
        &type_param_substitutions,
        &resolved_names,
    );
    check_argument_escaping(
        analyzer,
        args,
        &call_name,
        signature,
        noescape_bindings,
        local_closure_bindings,
        &resolved_names,
    );
}

/// Phase 4: check each argument's type against the resolved signature parameter
/// (after type-parameter substitution).
fn check_argument_types(
    analyzer: &mut Analyzer<'_>,
    args: &[HirCallArg],
    call_name: &str,
    signature: &FunctionSig,
    type_param_substitutions: &HashMap<String, String>,
    resolved_names: &[Option<&str>],
) {
    for (arg, resolved) in args.iter().zip(resolved_names) {
        let Some(name) = *resolved else {
            continue;
        };
        let Some(expected_param) = signature.params.iter().find(|param| param.name == name) else {
            continue;
        };
        let Ok(expected_type) = substitute_type_params(
            &analyzer.budget,
            &expected_param.ty.to_string(),
            type_param_substitutions,
        ) else {
            return;
        };
        if check_fn_closure_contract(
            analyzer,
            call_name,
            name,
            &expected_type,
            &signature.type_params,
            &arg.value,
        ) {
            continue;
        }
        if type_contains_unresolved_generic(&expected_type, &signature.type_params) {
            continue;
        }
        if check_variant_payload_type(
            analyzer,
            &PayloadCheckSite::Argument {
                call_name,
                arg_name: name,
            },
            &expected_type,
            &arg.value,
        ) {
            continue;
        }
        let Some(actual_type) = hir_expr_type_name(&arg.value) else {
            continue;
        };
        if unresolved_generic_type(analyzer, actual_type) {
            continue;
        }
        if json_value_accepts_literal(&expected_type, &arg.value) {
            continue;
        }
        if check_map_literal_type(analyzer, &expected_type, &arg.value, "argument") {
            continue;
        }
        if check_list_literal_type(analyzer, &expected_type, &arg.value, "argument") {
            continue;
        }
        if !argument_type_matches(
            &analyzer.expand_type_alias(&expected_type),
            &analyzer.expand_type_alias(actual_type),
        ) {
            analyzer.diagnostics.push(error_cause_manual_fix(
                code::ARGUMENT_TYPE_MISMATCH,
                format!(
                    "argument `{name}` for `{call_name}` has type `{actual_type}`, expected `{}`.",
                    expected_type
                ),
                hir_expr_span(&arg.value).clone(),
                "argument type mismatch",
                "RSScript call argument types must match the resolved callee signature before Rust lowering.",
                "match_argument_type",
                format!(
                    "Pass a value of type `{expected_type}` for `{name}`.",
                ),
            ));
        }
    }
}

/// Phase 5: enforce noescape and local-closure escape rules for each argument,
/// skipping parameters typed as `noescape` function types.
fn check_argument_escaping(
    analyzer: &mut Analyzer<'_>,
    args: &[HirCallArg],
    call_name: &str,
    signature: &FunctionSig,
    noescape_bindings: &HashMap<String, CallbackBinding>,
    local_closure_bindings: &HashMap<String, Span>,
    resolved_names: &[Option<&str>],
) {
    for (arg, resolved) in args.iter().zip(resolved_names) {
        let expected_param =
            resolved.and_then(|name| signature.params.iter().find(|param| param.name == name));
        if expected_param
            .is_some_and(|param| param.ty.qualifiers.noescape && param.ty.is_function())
        {
            continue;
        }
        check_noescape_escape(
            analyzer,
            &arg.value,
            &arg.span,
            noescape_bindings,
            NoescapeEscapeContext::Pass { callee: call_name },
        );
        check_local_closure_escape(
            analyzer,
            &arg.value,
            &arg.span,
            local_closure_bindings,
            LocalClosureEscapeContext::Pass { callee: call_name },
        );
    }
}

/// Resolves the parameter name an argument targets, considering (in order) an
/// explicit `name:` label, constructor field shorthand, and positional binding.
/// Returns `None` when the argument cannot be matched to any parameter name.
fn resolved_arg_param_name<'a>(
    arg: &'a HirCallArg,
    index: usize,
    allow_constructor_field_shorthand: bool,
    allow_positional_args: bool,
    param_names: &'a HashSet<String>,
    signature_params: &'a [ParamSig],
) -> Option<&'a str> {
    arg.name
        .as_deref()
        .or_else(|| {
            constructor_field_shorthand_name(allow_constructor_field_shorthand, arg, param_names)
        })
        .or_else(|| positional_param_name(allow_positional_args, signature_params, index))
}

fn constructor_field_shorthand_name<'a>(
    allow_constructor_field_shorthand: bool,
    arg: &'a HirCallArg,
    param_names: &HashSet<String>,
) -> Option<&'a str> {
    if !allow_constructor_field_shorthand || arg.name.is_some() {
        return None;
    }
    let HirExpr::Ident { name, .. } = &arg.value else {
        return None;
    };
    param_names.contains(name).then_some(name.as_str())
}

fn positional_param_name(
    allow_positional_args: bool,
    signature_params: &[ParamSig],
    index: usize,
) -> Option<&str> {
    if !allow_positional_args {
        return None;
    }
    signature_params.get(index).map(|param| param.name.as_str())
}

/// Enforce the cross-isolate **message** payload contract on `Channel.message<T>`:
/// the element type must be cross-isolate-transferable (a self-contained value
/// with no managed handle), so a message can cross an isolate boundary without
/// sharing mutable state (spec §20.2-3). Skips a still-generic element (an
/// enclosing-function type param), which can't be proven here.
fn check_message_channel_payload(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    call_name: &str,
    signature: &FunctionSig,
    substitutions: &HashMap<String, String>,
    call_span: &Span,
) {
    // `call_name` carries explicit type args (e.g. `Channel.message<List<Int>>`);
    // compare on the root.
    if call_name.split('<').next() != Some("Channel.message") {
        return;
    }
    let Some(param) = signature.type_params.first() else {
        return;
    };
    let Some(element) = substitutions.get(param) else {
        return;
    };
    // A bare enclosing-function type param can't be checked without a `Sendable`
    // bound; leave it (a future bound would enforce it at the caller).
    if function
        .type_params
        .iter()
        .any(|type_param| type_param.name == *element)
    {
        return;
    }
    if crate::checks::local::is_cross_isolate_transferable(element) {
        return;
    }
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::MESSAGE_PAYLOAD_NOT_TRANSFERABLE,
            format!("message channel payload `{element}` is not cross-isolate-transferable."),
            call_span.clone(),
            "non-transferable message payload",
        )
        .with_cause(
            "A message must be self-contained data with no managed handle, so it can cross an isolate boundary without sharing mutable state. v1 allows Copy scalars, `String`, and `Bytes`.",
        )
        .with_fix(
            "use_transferable_message_payload",
            format!(
                "Send a transferable value (a Copy scalar, `String`, or `Bytes`) instead of `{element}`, or use `Channel.bounded` for an in-isolate channel."
            ),
            "manual",
        ),
    );
}
