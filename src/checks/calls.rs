use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, Span, code};
use crate::hir::{
    CallResolution, FunctionSig, HirBindingKind, HirBlock, HirCallArg, HirExpr, HirStmt,
};
use crate::syntax::ast::{BinaryOp, Callee, FunctionDecl, Item, TypeRef};

#[derive(Debug, Clone)]
struct CallbackBinding {
    type_name: String,
    span: Span,
}

struct CallCheckContext<'a> {
    noescape_bindings: &'a HashMap<String, CallbackBinding>,
    callback_bindings: &'a HashMap<String, CallbackBinding>,
    local_closure_bindings: &'a HashMap<String, Span>,
}

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
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
                    binding.type_name.as_deref().and_then(|type_name| {
                        is_fn_type(type_name).then_some((
                            binding.name.clone(),
                            CallbackBinding {
                                type_name: type_name.to_string(),
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
                collect_local_closure_bindings(block, &mut local_closure_bindings);
                let context = CallCheckContext {
                    noescape_bindings: &noescape_bindings,
                    callback_bindings: &callback_bindings,
                    local_closure_bindings: &local_closure_bindings,
                };
                check_function_fallthrough(analyzer, function, block);
                check_block(analyzer, function, block, &context);
            }
        }
    }
}

fn check_function_fallthrough(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    block: &HirBlock,
) {
    let Some(return_ty) = &function.return_ty else {
        return;
    };
    if return_ty.name == "Unit" && return_ty.args.is_empty() {
        return;
    }
    let function_type_params = function
        .type_params
        .iter()
        .map(|param| param.name.clone())
        .collect::<Vec<_>>();
    let expected = type_ref_name(return_ty);
    if type_contains_unresolved_generic(&expected, &function_type_params) {
        return;
    }
    if block_may_fall_through(block) {
        return_type_mismatch_diagnostic(
            analyzer,
            &function.name,
            "Unit",
            &expected,
            &function.span,
        );
    }
}

fn block_may_fall_through(block: &HirBlock) -> bool {
    for statement in &block.statements {
        if !statement_may_fall_through(statement) {
            return false;
        }
    }
    true
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
        HirStmt::With { body, .. } => block_may_fall_through(body),
        HirStmt::Let { .. }
        | HirStmt::Expr(_)
        | HirStmt::If {
            else_body: None, ..
        }
        | HirStmt::Loop { .. }
        | HirStmt::For { .. }
        | HirStmt::Match { .. }
        | HirStmt::Unknown(_) => true,
    }
}

fn collect_local_closure_bindings(block: &HirBlock, bindings: &mut HashMap<String, Span>) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                kind: HirBindingKind::LocalLet,
                name,
                value: Some(HirExpr::Closure { body, .. }),
                span,
                ..
            } => {
                bindings.insert(name.clone(), span.clone());
                collect_local_closure_bindings(body, bindings);
            }
            HirStmt::Let {
                value: Some(HirExpr::Closure { body, .. }),
                ..
            } => collect_local_closure_bindings(body, bindings),
            HirStmt::Let { .. } | HirStmt::Return { .. } | HirStmt::Expr(_) => {}
            HirStmt::With { body, .. } => collect_local_closure_bindings(body, bindings),
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_local_closure_bindings(then_body, bindings);
                if let Some(else_body) = else_body {
                    collect_local_closure_bindings(else_body, bindings);
                }
            }
            HirStmt::Loop { body, .. } => collect_local_closure_bindings(body, bindings),
            HirStmt::For { body, .. } => collect_local_closure_bindings(body, bindings),
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_local_closure_bindings(&arm.body, bindings);
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
    let noescape_bindings = context.noescape_bindings;
    let local_closure_bindings = context.local_closure_bindings;
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                value: Some(value),
                type_name,
                value_type_name,
                name,
                span,
                ..
            } => {
                check_binding_type(analyzer, name, type_name, value_type_name, value);
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
                check_expr(analyzer, value, context);
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
                check_expr(analyzer, value, context);
            }
            HirStmt::Return {
                value: None, span, ..
            } => {
                check_return_type(analyzer, function, None, span);
            }
            HirStmt::Expr(value) => {
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
                check_expr(analyzer, value, context);
            }
            HirStmt::With { resource, body, .. } => {
                check_expr(analyzer, resource, context);
                check_block(analyzer, function, body, context);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                check_expr(analyzer, condition, context);
                check_block(analyzer, function, then_body, context);
                if let Some(else_body) = else_body {
                    check_block(analyzer, function, else_body, context);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    check_expr(analyzer, condition, context);
                }
                check_block(analyzer, function, body, context);
            }
            HirStmt::For { iterable, body, .. } => {
                check_expr(analyzer, iterable, context);
                check_block(analyzer, function, body, context);
            }
            HirStmt::Match { value, arms, .. } => {
                check_expr(analyzer, value, context);
                for arm in arms {
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
    if unresolved_generic_type(expected) {
        return;
    }
    if check_binding_variant_payload_type(analyzer, name, expected, value) {
        return;
    }
    let Some(actual) = actual.as_deref() else {
        return;
    };
    if unresolved_generic_type(actual) {
        return;
    }
    if argument_type_matches(expected, actual) {
        return;
    }
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!("binding `{name}` has initializer type `{actual}`, expected `{expected}`."),
            hir_expr_span(value).clone(),
            "binding type mismatch",
        )
        .with_cause("Explicit `let` and `local` type annotations are source-level contracts and must match the initializer before Rust lowering.")
        .with_fix(
            "match_binding_type",
            format!("Initialize `{name}` with a `{expected}` value, or change the binding annotation."),
            "manual",
        ),
    );
}

fn check_binding_variant_payload_type(
    analyzer: &mut Analyzer<'_>,
    name: &str,
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
                check_binding_payload_type(analyzer, name, payload, &expected);
            } else if expected != "Unit" {
                binding_payload_type_mismatch_diagnostic(
                    analyzer,
                    name,
                    "Unit",
                    &expected,
                    hir_expr_span(value),
                );
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
                check_binding_payload_type(analyzer, name, payload, expected);
            } else if expected != "Unit" {
                binding_payload_type_mismatch_diagnostic(
                    analyzer,
                    name,
                    "Unit",
                    expected,
                    hir_expr_span(value),
                );
            }
            true
        }
        _ => false,
    }
}

fn check_binding_payload_type(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    payload: &HirExpr,
    expected: &str,
) {
    if let Some((variant, nested_payload)) = enum_variant_payload(payload)
        && let Some(expected_payload) = expected_variant_payload_type(expected, variant)
    {
        if let Some(nested_payload) = nested_payload {
            check_binding_payload_type(analyzer, name, nested_payload, &expected_payload);
        } else if expected_payload != "Unit" {
            binding_payload_type_mismatch_diagnostic(
                analyzer,
                name,
                "Unit",
                &expected_payload,
                hir_expr_span(payload),
            );
        }
        return;
    }
    let Some(actual) = hir_expr_type_name(payload) else {
        return;
    };
    if unresolved_generic_type(actual) {
        return;
    }
    if !argument_type_matches(expected, actual) {
        binding_payload_type_mismatch_diagnostic(
            analyzer,
            name,
            actual,
            expected,
            hir_expr_span(payload),
        );
    }
}

fn binding_payload_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    name: &str,
    actual: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!(
                "binding `{name}` has initializer payload type `{actual}`, expected `{expected}`."
            ),
            span.clone(),
            "binding type mismatch",
        )
        .with_cause("Result and Option binding initializers are checked against explicit binding payload types before Rust lowering.")
        .with_fix(
            "match_binding_payload_type",
            format!("Initialize `{name}` with a `{expected}` payload, or change the binding annotation."),
            "manual",
        ),
    );
}

fn check_expr(analyzer: &mut Analyzer<'_>, expr: &HirExpr, context: &CallCheckContext<'_>) {
    match expr {
        HirExpr::Call {
            callee,
            args,
            span,
            resolution,
            ..
        } => {
            check_call_args(analyzer, callee, args, span, resolution, context);
            for arg in args {
                check_expr(analyzer, &arg.value, context);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_expr(analyzer, value, context);
        }
        HirExpr::Binary { left, right, .. } => {
            check_expr(analyzer, left, context);
            check_expr(analyzer, right, context);
        }
        HirExpr::Field { base, .. } => check_expr(analyzer, base, context),
        HirExpr::Index { base, index, .. } => {
            check_expr(analyzer, base, context);
            check_expr(analyzer, index, context);
        }
        HirExpr::Closure { body, .. } => {
            // Closure bodies use the enclosing function's return contract only when they
            // are lowered as ordinary statements. noescape callback return contracts are
            // checked at their call/parameter boundary.
            check_expr_block_without_return_contract(analyzer, body, context)
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn check_expr_block_without_return_contract(
    analyzer: &mut Analyzer<'_>,
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
            | HirStmt::Expr(value) => {
                check_expr(analyzer, value, context);
            }
            HirStmt::With { resource, body, .. } => {
                check_expr(analyzer, resource, context);
                check_expr_block_without_return_contract(analyzer, body, context);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                check_expr(analyzer, condition, context);
                check_expr_block_without_return_contract(analyzer, then_body, context);
                if let Some(else_body) = else_body {
                    check_expr_block_without_return_contract(analyzer, else_body, context);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    check_expr(analyzer, condition, context);
                }
                check_expr_block_without_return_contract(analyzer, body, context);
            }
            HirStmt::For { iterable, body, .. } => {
                check_expr(analyzer, iterable, context);
                check_expr_block_without_return_contract(analyzer, body, context);
            }
            HirStmt::Match { value, arms, .. } => {
                check_expr(analyzer, value, context);
                for arm in arms {
                    check_expr_block_without_return_contract(analyzer, &arm.body, context);
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
    if unresolved_generic_type(actual) {
        return;
    }
    if !argument_type_matches(&expected, actual) {
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
            if unresolved_generic_type(actual) {
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
            if unresolved_generic_type(actual) {
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
    let Some(actual) = hir_expr_type_name(payload) else {
        return;
    };
    if unresolved_generic_type(actual) {
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
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RETURN_TYPE_MISMATCH,
            format!(
                "{label} in `{}` has type `{actual}`, expected `{expected}`.",
                function.name
            ),
            span.clone(),
            "return type mismatch",
        )
        .with_cause("Result and Option return constructors are checked against the declared return payload before Rust lowering.")
        .with_fix(
            "match_return_payload_type",
            format!("Return a `{expected}` payload here."),
            "manual",
        ),
    );
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
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RETURN_TYPE_MISMATCH,
            format!("return in `{function_name}` has type `{actual}`, expected `{expected}`."),
            span.clone(),
            "return type mismatch",
        )
        .with_cause("RSScript return types are part of the review contract and must be checked before Rust lowering.")
        .with_fix(
            "match_return_type",
            format!("Return a value of type `{expected}` here."),
            "manual",
        ),
    );
}

fn check_call_args(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    call_span: &Span,
    resolution: &CallResolution,
    context: &CallCheckContext<'_>,
) {
    let noescape_bindings = context.noescape_bindings;
    let callback_bindings = context.callback_bindings;
    let local_closure_bindings = context.local_closure_bindings;
    let call_name = callee_display(callee);
    if is_closure_binding_call(
        callee,
        args,
        resolution,
        callback_bindings,
        local_closure_bindings,
    ) {
        check_callback_call_args(analyzer, callee, args, callback_bindings);
        return;
    }
    if matches!(resolution, CallResolution::EnumVariant) {
        check_enum_variant_form(analyzer, callee, args, call_span);
        return;
    }

    for arg in args {
        if arg.name.is_none() {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::UNNAMED_ARGUMENT,
                    format!("call to `{call_name}` uses an unnamed argument."),
                    arg.span.clone(),
                    "argument must be named",
                )
                .with_cause("RSScript v0.5 requires all non-receiver call arguments to be named.")
                .with_fix(
                    "add_argument_name",
                    "Write the argument as `name: value`.",
                    "manual",
                ),
            );
        }
    }

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
        CallResolution::EnumVariant => return,
    };
    let signature_params = signature.params.clone();
    let param_effects: HashMap<String, &'static str> = signature
        .params
        .iter()
        .filter_map(|param| {
            param
                .effect
                .map(|effect| (param.name.clone(), effect.as_str()))
        })
        .collect();
    let param_names: HashSet<String> = signature_params
        .iter()
        .map(|param| param.name.clone())
        .collect();

    let mut seen_names = HashSet::new();
    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        if !seen_names.insert(name.as_str()) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_ARGUMENT,
                    format!("call to `{call_name}` repeats argument `{name}`."),
                    arg.span.clone(),
                    "duplicate argument",
                )
                .with_cause("Each named parameter can be provided at most once.")
                .with_fix(
                    "remove_duplicate_argument",
                    format!("Remove the extra `{name}: ...` argument."),
                    "manual",
                ),
            );
        }
        if !param_names.contains(name) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_ARGUMENT,
                    format!("call to `{call_name}` has no argument named `{name}`."),
                    arg.span.clone(),
                    "unknown argument",
                )
                .with_cause(format!(
                    "`{call_name}` does not declare a parameter named `{name}`."
                ))
                .with_fix(
                    "rename_argument",
                    format!("Use one of: {}.", join_param_names(&signature_params)),
                    "manual",
                ),
            );
        }
    }

    if args.iter().all(|arg| arg.name.is_some()) {
        let provided_names: HashSet<&str> =
            args.iter().filter_map(|arg| arg.name.as_deref()).collect();
        for param in &signature_params {
            if !provided_names.contains(param.name.as_str()) {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        code::MISSING_ARGUMENT,
                        format!(
                            "call to `{call_name}` is missing required argument `{}`.",
                            param.name
                        ),
                        call_span.clone(),
                        "missing argument",
                    )
                    .with_cause(format!(
                        "`{call_name}` requires a named argument `{}`.",
                        param.name
                    ))
                    .with_fix(
                        "add_argument",
                        format!("Add `{}: ...` to the call.", param.name),
                        "manual",
                    ),
                );
            }
        }
    }

    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        let Some(expected) = param_effects.get(name) else {
            continue;
        };
        if expr_data_effect(&arg.value) != Some(*expected) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::MISSING_DATA_EFFECT,
                    format!("argument `{name}` for `{call_name}` is missing `{expected}`."),
                    hir_expr_span(&arg.value).clone(),
                    "missing data effect",
                )
                .with_cause("Non-Copy parameters require an explicit `read`, `mut`, or `take` call-site effect.")
                .with_fix(
                    "add_data_effect",
                    format!("Write `{name}: {expected} ...` at the call site."),
                    "machine-applicable",
                ),
            );
        }
    }

    let type_param_substitutions = call_type_param_substitutions(analyzer, callee, args, signature);

    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        let Some(expected_param) = signature.params.iter().find(|param| param.name == *name) else {
            continue;
        };
        let expected_type =
            substitute_type_params(&expected_param.type_name, &type_param_substitutions);
        if check_noescape_closure_return_type(
            analyzer,
            &call_name,
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
        if check_argument_variant_payload_type(
            analyzer,
            &call_name,
            name,
            &expected_type,
            &arg.value,
        ) {
            continue;
        }
        let Some(actual_type) = hir_expr_type_name(&arg.value) else {
            continue;
        };
        if unresolved_generic_type(actual_type) {
            continue;
        }
        if !argument_type_matches(&expected_type, actual_type) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::ARGUMENT_TYPE_MISMATCH,
                    format!(
                        "argument `{name}` for `{call_name}` has type `{actual_type}`, expected `{}`.",
                        expected_type
                    ),
                    hir_expr_span(&arg.value).clone(),
                    "argument type mismatch",
                )
                .with_cause("RSScript call argument types must match the resolved callee signature before Rust lowering.")
                .with_fix(
                    "match_argument_type",
                    format!(
                        "Pass a value of type `{expected_type}` for `{name}`.",
                    ),
                    "manual",
                ),
            );
        }
    }

    for arg in args {
        let expected_param = arg
            .name
            .as_ref()
            .and_then(|name| signature.params.iter().find(|param| param.name == *name));
        if expected_param.is_some_and(|param| is_noescape_fn_type(&param.type_name)) {
            continue;
        }
        check_noescape_escape(
            analyzer,
            &arg.value,
            &arg.span,
            noescape_bindings,
            NoescapeEscapeContext::Pass { callee: &call_name },
        );
        check_local_closure_escape(
            analyzer,
            &arg.value,
            &arg.span,
            local_closure_bindings,
            LocalClosureEscapeContext::Pass { callee: &call_name },
        );
    }
}

fn check_enum_variant_form(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    call_span: &Span,
) {
    let variant = callee_name(callee);
    let valid = match variant.as_str() {
        "Ok" | "Err" | "Some" => args.len() == 1 && args[0].name.is_none(),
        "None" | "Result" | "Option" => false,
        _ => true,
    };
    if valid {
        return;
    }

    let form = match variant.as_str() {
        "Ok" => "`Ok(value)`",
        "Err" => "`Err(error)`",
        "Some" => "`Some(value)`",
        "None" => "`None`",
        "Result" => "`Ok(value)` or `Err(error)`",
        "Option" => "`Some(value)` or `None`",
        _ => "the conventional variant form",
    };
    let span = args
        .first()
        .map(|arg| arg.span.clone())
        .unwrap_or_else(|| call_span.clone());
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::UNSUPPORTED_SYNTAX,
            format!("variant `{variant}` must use its conventional RSScript form."),
            span,
            "unsupported variant form",
        )
        .with_cause("Standard Result and Option variants are call-like for checker purposes, but they are not ordinary named-argument calls.")
        .with_fix(
            "use_conventional_variant_form",
            format!("Write this variant as {form}."),
            "manual",
        ),
    );
}

fn call_type_param_substitutions(
    analyzer: &Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    signature: &FunctionSig,
) -> HashMap<String, String> {
    let mut substitutions = HashMap::new();
    let generic_params = signature
        .type_params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if generic_params.is_empty() {
        return substitutions;
    }
    if let Callee::Qualified { namespace, .. } = callee
        && let Some(namespace_args) = type_arg_names(namespace)
    {
        let root = type_root_name(namespace);
        let params = analyzer
            .hir
            .type_info(root)
            .map(|type_info| {
                type_info
                    .type_params
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
            })
            .or_else(|| builtin_generic_type_params(root))
            .unwrap_or_default();
        for (param, actual) in params.into_iter().zip(namespace_args) {
            if generic_params.contains(param) {
                substitutions.insert(param.to_string(), actual.to_string());
            }
        }
    }
    collect_call_arg_type_param_substitutions(args, signature, &generic_params, &mut substitutions);
    substitutions
}

fn collect_call_arg_type_param_substitutions(
    args: &[HirCallArg],
    signature: &FunctionSig,
    generic_params: &HashSet<&str>,
    substitutions: &mut HashMap<String, String>,
) {
    for (index, arg) in args.iter().enumerate() {
        let Some(param) = arg
            .name
            .as_deref()
            .and_then(|name| signature.params.iter().find(|param| param.name == name))
            .or_else(|| signature.params.get(index))
        else {
            continue;
        };
        let Some(actual_type) = hir_expr_type_name(&arg.value) else {
            continue;
        };
        collect_type_param_substitutions(
            &param.type_name,
            actual_type,
            generic_params,
            substitutions,
        );
    }
}

fn collect_type_param_substitutions(
    pattern: &str,
    actual: &str,
    generic_params: &HashSet<&str>,
    substitutions: &mut HashMap<String, String>,
) {
    let pattern = pattern.trim();
    let actual = actual.trim();
    if actual == "?" {
        return;
    }
    if generic_params.contains(pattern) {
        substitutions
            .entry(pattern.to_string())
            .or_insert_with(|| actual.to_string());
        return;
    }
    if is_noescape_fn_type(pattern) && is_noescape_fn_type(actual) {
        for (pattern_param, actual_param) in noescape_param_types(pattern)
            .into_iter()
            .zip(noescape_param_types(actual))
        {
            collect_type_param_substitutions(
                pattern_param,
                actual_param,
                generic_params,
                substitutions,
            );
        }
        if let (Some(pattern_return), Some(actual_return)) =
            (noescape_return_type(pattern), noescape_return_type(actual))
        {
            collect_type_param_substitutions(
                pattern_return,
                actual_return,
                generic_params,
                substitutions,
            );
        }
        return;
    }
    let Some(pattern_args) = type_arg_names(pattern) else {
        return;
    };
    let Some(actual_args) = type_arg_names(actual) else {
        return;
    };
    if type_root_name(pattern) != type_root_name(actual) || pattern_args.len() != actual_args.len()
    {
        return;
    }
    for (pattern_arg, actual_arg) in pattern_args.into_iter().zip(actual_args) {
        collect_type_param_substitutions(pattern_arg, actual_arg, generic_params, substitutions);
    }
}

fn substitute_type_params(type_name: &str, substitutions: &HashMap<String, String>) -> String {
    if let Some(replacement) = substitutions.get(type_name) {
        return replacement.clone();
    }
    if let Some(return_ty) = noescape_return_type(type_name) {
        let params = noescape_param_types(type_name)
            .into_iter()
            .map(|param| substitute_type_params(param, substitutions))
            .collect::<Vec<_>>()
            .join(", ");
        return format!(
            "noescape Fn({params}) -> {}",
            substitute_type_params(return_ty, substitutions)
        );
    }
    let Some(args) = type_arg_names(type_name) else {
        return type_name.to_string();
    };
    let root = type_root_name(type_name);
    let args = args
        .into_iter()
        .map(|arg| substitute_type_params(arg, substitutions))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{root}<{args}>")
}

fn check_noescape_closure_return_type(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    expected_type: &str,
    generic_params: &[String],
    value: &HirExpr,
) -> bool {
    if !is_noescape_fn_type(expected_type) {
        return false;
    }
    let HirExpr::Closure { params, body, .. } = value else {
        return false;
    };
    let expected_params = noescape_param_types(expected_type);
    if params.len() != expected_params.len() {
        callback_arity_mismatch_diagnostic(
            analyzer,
            call_name,
            arg_name,
            params.len(),
            expected_params.len(),
            hir_expr_span(value),
        );
        return true;
    }
    let expected_return = noescape_return_type(expected_type).unwrap_or("Unit");
    let (local_bindings, managed_bindings) = closure_binding_sets(body);
    let contract = CallbackContract {
        call_name,
        arg_name,
        expected_return,
        generic_params,
        params,
        param_types: &expected_params,
        local_bindings,
        managed_bindings,
    };
    check_callback_body_call_argument_types(analyzer, body, &contract);
    let returns = closure_return_sites(body);
    if returns.is_empty() {
        if !type_pattern_matches(expected_return, "Unit", generic_params) {
            callback_return_type_mismatch_diagnostic(
                analyzer,
                call_name,
                arg_name,
                "Unit",
                expected_return,
                &body.span,
            );
        }
        return true;
    }
    for return_site in returns {
        match return_site {
            ClosureReturn::Expr(expr) => {
                check_callback_return_expr_type(analyzer, expr, &contract);
            }
            ClosureReturn::Unit(span) => {
                if !type_pattern_matches(expected_return, "Unit", generic_params) {
                    callback_return_type_mismatch_diagnostic(
                        analyzer,
                        call_name,
                        arg_name,
                        "Unit",
                        expected_return,
                        span,
                    );
                }
            }
        }
    }
    true
}

fn check_callback_call_args(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    callback_bindings: &HashMap<String, CallbackBinding>,
) {
    let Callee::Name(name) = callee else {
        return;
    };
    let Some(binding) = callback_bindings.get(name) else {
        return;
    };
    let expected_params = fn_param_types(&binding.type_name);
    if args.len() != expected_params.len() {
        callback_call_arity_mismatch_diagnostic(
            analyzer,
            name,
            args.len(),
            expected_params.len(),
            args.first()
                .map_or_else(|| binding.span.clone(), |arg| arg.span.clone()),
        );
        return;
    }
    for (index, (arg, expected)) in args.iter().zip(expected_params).enumerate() {
        let Some(actual) = hir_expr_type_name(&arg.value) else {
            continue;
        };
        if unresolved_generic_type(actual) {
            continue;
        }
        if !argument_type_matches(expected, actual) {
            callback_call_argument_type_mismatch_diagnostic(
                analyzer,
                name,
                index,
                actual,
                expected,
                hir_expr_span(&arg.value),
            );
        }
    }
}

enum ClosureReturn<'a> {
    Expr(&'a HirExpr),
    Unit(&'a Span),
}

struct CallbackContract<'a> {
    call_name: &'a str,
    arg_name: &'a str,
    expected_return: &'a str,
    generic_params: &'a [String],
    params: &'a [String],
    param_types: &'a [&'a str],
    local_bindings: HashSet<String>,
    managed_bindings: HashSet<String>,
}

fn closure_binding_sets(body: &HirBlock) -> (HashSet<String>, HashSet<String>) {
    let mut local_bindings = HashSet::new();
    let mut managed_bindings = HashSet::new();
    collect_closure_binding_sets(body, &mut local_bindings, &mut managed_bindings);
    (local_bindings, managed_bindings)
}

fn collect_closure_binding_sets(
    block: &HirBlock,
    local_bindings: &mut HashSet<String>,
    managed_bindings: &mut HashSet<String>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let { kind, name, .. } => match kind {
                HirBindingKind::LocalLet => {
                    local_bindings.insert(name.clone());
                }
                HirBindingKind::ManagedLet | HirBindingKind::Param => {
                    managed_bindings.insert(name.clone());
                }
            },
            HirStmt::With { body, .. } | HirStmt::Loop { body, .. } => {
                collect_closure_binding_sets(body, local_bindings, managed_bindings);
            }
            HirStmt::For { binding, body, .. } => {
                managed_bindings.insert(binding.clone());
                collect_closure_binding_sets(body, local_bindings, managed_bindings);
            }
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_closure_binding_sets(then_body, local_bindings, managed_bindings);
                if let Some(else_body) = else_body {
                    collect_closure_binding_sets(else_body, local_bindings, managed_bindings);
                }
            }
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_closure_binding_sets(&arm.body, local_bindings, managed_bindings);
                }
            }
            HirStmt::Return { .. }
            | HirStmt::Expr(_)
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn closure_return_sites(body: &HirBlock) -> Vec<ClosureReturn<'_>> {
    let mut returns = Vec::new();
    for statement in &body.statements {
        collect_explicit_return_sites(statement, &mut returns);
    }
    if !body
        .statements
        .iter()
        .any(|statement| matches!(statement, HirStmt::Return { .. }))
        && let Some(statement) = body.statements.iter().next_back()
    {
        collect_implicit_closure_return_sites(statement, &mut returns);
    }
    returns
}

fn collect_explicit_return_sites<'a>(statement: &'a HirStmt, returns: &mut Vec<ClosureReturn<'a>>) {
    match statement {
        HirStmt::Return {
            value: Some(value), ..
        } => returns.push(ClosureReturn::Expr(value)),
        HirStmt::Return {
            value: None, span, ..
        } => returns.push(ClosureReturn::Unit(span)),
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => {
            collect_explicit_return_sites_from_block(then_body, returns);
            if let Some(else_body) = else_body {
                collect_explicit_return_sites_from_block(else_body, returns);
            }
        }
        HirStmt::Loop { body, .. } | HirStmt::With { body, .. } => {
            collect_explicit_return_sites_from_block(body, returns);
        }
        HirStmt::For { body, .. } => collect_explicit_return_sites_from_block(body, returns),
        HirStmt::Match { arms, .. } => {
            for arm in arms {
                collect_explicit_return_sites_from_block(&arm.body, returns);
            }
        }
        HirStmt::Let { .. }
        | HirStmt::Expr(_)
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_explicit_return_sites_from_block<'a>(
    block: &'a HirBlock,
    returns: &mut Vec<ClosureReturn<'a>>,
) {
    for statement in &block.statements {
        collect_explicit_return_sites(statement, returns);
    }
}

fn collect_implicit_closure_return_sites<'a>(
    statement: &'a HirStmt,
    returns: &mut Vec<ClosureReturn<'a>>,
) {
    match statement {
        HirStmt::Expr(value) => returns.push(ClosureReturn::Expr(value)),
        HirStmt::Let { span, .. } => returns.push(ClosureReturn::Unit(span)),
        HirStmt::If {
            then_body,
            else_body: Some(else_body),
            ..
        } => {
            if let Some(statement) = then_body.statements.last() {
                collect_implicit_closure_return_sites(statement, returns);
            }
            if let Some(statement) = else_body.statements.last() {
                collect_implicit_closure_return_sites(statement, returns);
            }
        }
        HirStmt::Return { .. }
        | HirStmt::With { .. }
        | HirStmt::If { .. }
        | HirStmt::Loop { .. }
        | HirStmt::For { .. }
        | HirStmt::Match { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn check_callback_body_call_argument_types(
    analyzer: &mut Analyzer<'_>,
    body: &HirBlock,
    contract: &CallbackContract<'_>,
) {
    for statement in &body.statements {
        match statement {
            HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value) => check_callback_call_argument_types(analyzer, value, contract),
            HirStmt::With { resource, body, .. } => {
                check_callback_call_argument_types(analyzer, resource, contract);
                check_callback_body_call_argument_types(analyzer, body, contract);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                check_callback_call_argument_types(analyzer, condition, contract);
                check_callback_body_call_argument_types(analyzer, then_body, contract);
                if let Some(else_body) = else_body {
                    check_callback_body_call_argument_types(analyzer, else_body, contract);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    check_callback_call_argument_types(analyzer, condition, contract);
                }
                check_callback_body_call_argument_types(analyzer, body, contract);
            }
            HirStmt::For { iterable, body, .. } => {
                check_callback_call_argument_types(analyzer, iterable, contract);
                check_callback_body_call_argument_types(analyzer, body, contract);
            }
            HirStmt::Match { value, arms, .. } => {
                check_callback_call_argument_types(analyzer, value, contract);
                for arm in arms {
                    check_callback_body_call_argument_types(analyzer, &arm.body, contract);
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

fn check_callback_return_expr_type(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    contract: &CallbackContract<'_>,
) {
    check_callback_operator_operand_types(analyzer, expr, contract);
    check_callback_call_argument_types(analyzer, expr, contract);
    if check_callback_variant_return_type(analyzer, expr, contract) {
        return;
    }
    if check_callback_fresh_return_type(analyzer, expr, contract.expected_return, contract) {
        return;
    }
    let Some(actual) = callback_expr_type_name(expr, contract.params, contract.param_types) else {
        return;
    };
    if !type_pattern_matches(contract.expected_return, &actual, contract.generic_params) {
        callback_return_type_mismatch_diagnostic(
            analyzer,
            contract.call_name,
            contract.arg_name,
            &actual,
            contract.expected_return,
            hir_expr_span(expr),
        );
    }
}

fn check_callback_variant_return_type(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    contract: &CallbackContract<'_>,
) -> bool {
    let Some((variant, payload)) = enum_variant_payload(expr) else {
        return false;
    };
    match (type_root_name(contract.expected_return), variant) {
        ("Option", "None") => true,
        ("Option", "Some") => {
            let Some(expected_payload) = type_arg_names(contract.expected_return)
                .and_then(|args| args.first().map(|arg| arg.trim().to_string()))
            else {
                return false;
            };
            check_callback_payload_return_type(
                analyzer,
                payload,
                &expected_payload,
                expr,
                contract,
            );
            true
        }
        ("Result", "Ok" | "Err") => {
            let Some(args) = type_arg_names(contract.expected_return) else {
                return false;
            };
            let Some(expected_payload) = (match variant {
                "Ok" => args.first(),
                "Err" => args.get(1),
                _ => None,
            }) else {
                return false;
            };
            check_callback_payload_return_type(
                analyzer,
                payload,
                expected_payload.trim(),
                expr,
                contract,
            );
            true
        }
        _ => false,
    }
}

fn check_callback_payload_return_type(
    analyzer: &mut Analyzer<'_>,
    payload: Option<&HirExpr>,
    expected: &str,
    fallback: &HirExpr,
    contract: &CallbackContract<'_>,
) {
    let Some(payload) = payload else {
        if !type_pattern_matches(expected, "Unit", contract.generic_params) {
            callback_return_type_mismatch_diagnostic(
                analyzer,
                contract.call_name,
                contract.arg_name,
                "Unit",
                expected,
                hir_expr_span(fallback),
            );
        }
        return;
    };
    check_callback_call_argument_types(analyzer, payload, contract);
    check_callback_operator_operand_types(analyzer, payload, contract);
    if let Some((variant, nested_payload)) = enum_variant_payload(payload)
        && let Some(expected_payload) = expected_variant_payload_type(expected, variant)
    {
        check_callback_payload_return_type(
            analyzer,
            nested_payload,
            &expected_payload,
            payload,
            contract,
        );
        return;
    }
    if check_callback_fresh_return_type(analyzer, payload, expected, contract) {
        return;
    }
    let Some(actual) = callback_expr_type_name(payload, contract.params, contract.param_types)
    else {
        return;
    };
    if !type_pattern_matches(expected, &actual, contract.generic_params) {
        callback_return_type_mismatch_diagnostic(
            analyzer,
            contract.call_name,
            contract.arg_name,
            &actual,
            expected,
            hir_expr_span(payload),
        );
    }
}

fn check_callback_fresh_return_type(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    expected: &str,
    contract: &CallbackContract<'_>,
) -> bool {
    let Some(target) = fresh_type_target(expected) else {
        return false;
    };
    let Some(actual) = callback_expr_type_name(expr, contract.params, contract.param_types) else {
        return false;
    };
    if !type_pattern_matches(target, &actual, contract.generic_params) {
        callback_return_type_mismatch_diagnostic(
            analyzer,
            contract.call_name,
            contract.arg_name,
            &actual,
            expected,
            hir_expr_span(expr),
        );
        return true;
    }
    if callback_expr_is_fresh_value(expr, target, contract) {
        return true;
    }
    if let Some(name) = fresh_return_ident(expr) {
        callback_fresh_return_not_clean_diagnostic(
            analyzer,
            contract.call_name,
            contract.arg_name,
            name,
            expected,
            hir_expr_span(expr),
        );
    } else {
        callback_fresh_return_unknown_diagnostic(
            analyzer,
            contract.call_name,
            contract.arg_name,
            expected,
            hir_expr_span(expr),
        );
    }
    true
}

fn callback_expr_is_fresh_value(
    expr: &HirExpr,
    target: &str,
    contract: &CallbackContract<'_>,
) -> bool {
    match expr {
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            callback_expr_is_fresh_value(value, target, contract)
        }
        HirExpr::Ident { name, .. } => {
            contract.local_bindings.contains(name) && !contract.managed_bindings.contains(name)
        }
        HirExpr::Field { base, access, .. } if !access.is_handle && !access.is_weak => {
            fresh_return_ident(base).is_some_and(|name| {
                contract.local_bindings.contains(name) && !contract.managed_bindings.contains(name)
            })
        }
        HirExpr::Call {
            callee,
            resolution,
            type_name,
            ..
        } => {
            type_name.as_deref().is_some_and(|actual| {
                type_pattern_matches(target, actual, contract.generic_params)
                    && (callee_name(callee) == target
                        || matches!(resolution, CallResolution::Resolved { signature, .. } if signature.returns_fresh))
            })
        }
        HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => false,
    }
}

fn fresh_return_ident(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => fresh_return_ident(value),
        HirExpr::Field { base, access, .. } if !access.is_handle && !access.is_weak => {
            fresh_return_ident(base)
        }
        HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn callback_expr_type_name(
    expr: &HirExpr,
    callback_params: &[String],
    callback_param_types: &[&str],
) -> Option<String> {
    if let HirExpr::Ident { name, .. } = expr
        && let Some(index) = callback_params.iter().position(|param| param == name)
    {
        return callback_param_types
            .get(index)
            .map(|type_name| type_name.to_string());
    }
    if let HirExpr::Effect { value, .. }
    | HirExpr::Manage { value, .. }
    | HirExpr::Spawn { value, .. }
    | HirExpr::Await { value, .. }
    | HirExpr::Try { value, .. } = expr
    {
        return callback_expr_type_name(value, callback_params, callback_param_types);
    }
    if let HirExpr::Binary { op, .. } = expr {
        return match op {
            BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::Less
            | BinaryOp::LessEqual
            | BinaryOp::Greater
            | BinaryOp::GreaterEqual
            | BinaryOp::LogicalAnd
            | BinaryOp::LogicalOr => Some("Bool".to_string()),
            BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide => None,
        };
    }
    hir_expr_type_name(expr).map(str::to_string)
}

fn check_callback_call_argument_types(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    contract: &CallbackContract<'_>,
) {
    match expr {
        HirExpr::Call {
            callee,
            args,
            resolution,
            ..
        } => {
            if let CallResolution::Resolved { signature, .. } = resolution {
                check_callback_resolved_call_argument_types(
                    analyzer, callee, args, signature, contract,
                );
            }
            for arg in args {
                check_callback_call_argument_types(analyzer, &arg.value, contract);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            check_callback_call_argument_types(analyzer, left, contract);
            check_callback_call_argument_types(analyzer, right, contract);
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_callback_call_argument_types(analyzer, value, contract);
        }
        HirExpr::Field { base, .. } => check_callback_call_argument_types(analyzer, base, contract),
        HirExpr::Index { base, index, .. } => {
            check_callback_call_argument_types(analyzer, base, contract);
            check_callback_call_argument_types(analyzer, index, contract);
        }
        HirExpr::Closure { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn check_callback_resolved_call_argument_types(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    signature: &FunctionSig,
    contract: &CallbackContract<'_>,
) {
    let call_name = callee_display(callee);
    let type_param_substitutions = call_type_param_substitutions(analyzer, callee, args, signature);
    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        let Some(expected_param) = signature.params.iter().find(|param| param.name == *name) else {
            continue;
        };
        let expected_type =
            substitute_type_params(&expected_param.type_name, &type_param_substitutions);
        if type_contains_unresolved_generic(&expected_type, &signature.type_params) {
            continue;
        }
        if signature.retained_params.contains(name)
            && let Some((local_name, local_span)) =
                callback_retained_local_use(&arg.value, contract)
        {
            callback_retained_local_diagnostic(analyzer, &call_name, name, &local_name, local_span);
        }
        let Some(actual_type) =
            callback_expr_type_name(&arg.value, contract.params, contract.param_types)
        else {
            continue;
        };
        if unresolved_generic_type(&actual_type) {
            continue;
        }
        if !argument_type_matches(&expected_type, &actual_type) {
            callback_call_site_argument_type_mismatch_diagnostic(
                analyzer,
                &call_name,
                name,
                &actual_type,
                &expected_type,
                hir_expr_span(&arg.value),
            );
        }
    }
}

fn callback_retained_local_use(
    expr: &HirExpr,
    contract: &CallbackContract<'_>,
) -> Option<(String, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } if contract.local_bindings.contains(name) => {
            Some((name.clone(), span.clone()))
        }
        HirExpr::Field {
            base, access, span, ..
        } if !access.is_handle && !access.is_weak => fresh_return_ident(base)
            .filter(|name| contract.local_bindings.contains(*name))
            .map(|name| (name.to_string(), span.clone()))
            .or_else(|| callback_retained_local_use(base, contract)),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            callback_retained_local_use(value, contract)
        }
        HirExpr::Call { args, .. } => args
            .iter()
            .find_map(|arg| callback_retained_local_use(&arg.value, contract)),
        HirExpr::Binary { left, right, .. } => callback_retained_local_use(left, contract)
            .or_else(|| callback_retained_local_use(right, contract)),
        HirExpr::Index { base, index, .. } => callback_retained_local_use(base, contract)
            .or_else(|| callback_retained_local_use(index, contract)),
        HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Field { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_)
        | HirExpr::Ident { .. } => None,
    }
}

fn check_callback_operator_operand_types(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    contract: &CallbackContract<'_>,
) {
    match expr {
        HirExpr::Binary {
            op,
            left,
            right,
            span,
        } => {
            check_callback_operator_operand_types(analyzer, left, contract);
            check_callback_operator_operand_types(analyzer, right, contract);
            let (Some(left_type), Some(right_type)) = (
                callback_expr_type_name(left, contract.params, contract.param_types),
                callback_expr_type_name(right, contract.params, contract.param_types),
            ) else {
                return;
            };
            match op {
                BinaryOp::Equal | BinaryOp::NotEqual => {
                    if type_root_name(&left_type) != type_root_name(&right_type) {
                        callback_operator_type_mismatch_diagnostic(
                            analyzer,
                            span,
                            callback_operator_label(*op),
                            &left_type,
                            &right_type,
                            "matching operand types",
                        );
                    }
                }
                BinaryOp::Less
                | BinaryOp::LessEqual
                | BinaryOp::Greater
                | BinaryOp::GreaterEqual => {
                    if !is_numeric_type_name(&left_type) || !is_numeric_type_name(&right_type) {
                        callback_operator_type_mismatch_diagnostic(
                            analyzer,
                            span,
                            callback_operator_label(*op),
                            &left_type,
                            &right_type,
                            "numeric operands",
                        );
                    }
                }
                BinaryOp::LogicalAnd | BinaryOp::LogicalOr => {
                    if type_root_name(&left_type) != "Bool" || type_root_name(&right_type) != "Bool"
                    {
                        callback_operator_type_mismatch_diagnostic(
                            analyzer,
                            span,
                            callback_operator_label(*op),
                            &left_type,
                            &right_type,
                            "Bool operands",
                        );
                    }
                }
                BinaryOp::Add | BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide => {}
            }
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                check_callback_operator_operand_types(analyzer, &arg.value, contract);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            check_callback_operator_operand_types(analyzer, value, contract);
        }
        HirExpr::Field { base, .. } => {
            check_callback_operator_operand_types(analyzer, base, contract)
        }
        HirExpr::Index { base, index, .. } => {
            check_callback_operator_operand_types(analyzer, base, contract);
            check_callback_operator_operand_types(analyzer, index, contract);
        }
        HirExpr::Closure { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn callback_operator_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: &Span,
    operator: &str,
    left_type: &str,
    right_type: &str,
    expected: &str,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::OPERATOR_TYPE_MISMATCH,
            format!(
                "operator `{operator}` has operands `{left_type}` and `{right_type}`, expected {expected}."
            ),
            span.clone(),
            "operator type mismatch",
        )
        .with_cause(
            "`noescape Fn(...)` callback parameter types apply inside callback expressions before Rust lowering.",
        )
        .with_fix(
            "use_typed_operator_operands",
            "Use operands with matching RSScript types.",
            "manual",
        ),
    );
}

fn callback_operator_label(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Equal => "==",
        BinaryOp::NotEqual => "!=",
        BinaryOp::Less => "<",
        BinaryOp::LessEqual => "<=",
        BinaryOp::Greater => ">",
        BinaryOp::GreaterEqual => ">=",
        BinaryOp::LogicalAnd => "&&",
        BinaryOp::LogicalOr => "||",
    }
}

fn is_numeric_type_name(type_name: &str) -> bool {
    matches!(type_root_name(type_name), "Int" | "Float")
}

fn callback_return_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!(
                "callback argument `{arg_name}` for `{call_name}` returns `{actual}`, expected `{expected}`."
            ),
            span.clone(),
            "argument type mismatch",
        )
        .with_cause(
            "`noescape Fn() -> T` callback return types are part of the call signature and must be checked before Rust lowering.",
        )
        .with_fix(
            "match_callback_return_type",
            format!("Return a `{expected}` value from this callback."),
            "manual",
        ),
    );
}

fn callback_fresh_return_not_clean_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    name: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!(
                "callback argument `{arg_name}` for `{call_name}` returns non-fresh value `{name}`, expected `{expected}`."
            ),
            span.clone(),
            "argument type mismatch",
        )
        .with_cause("`noescape Fn() -> fresh T` callback returns are fresh contracts and cannot return captured or managed values.")
        .with_fix(
            "return_fresh_callback_value",
            "Return a struct constructor, fresh call, or local value created inside the callback.",
            "manual",
        ),
    );
}

fn callback_fresh_return_unknown_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!(
                "callback argument `{arg_name}` for `{call_name}` returns value whose freshness cannot be proven, expected `{expected}`."
            ),
            span.clone(),
            "argument type mismatch",
        )
        .with_cause("`noescape Fn() -> fresh T` callback returns must be proven fresh before Rust lowering.")
        .with_fix(
            "return_fresh_callback_value",
            "Return a struct constructor, fresh call, or local value created inside the callback.",
            "manual",
        ),
    );
}

fn callback_retained_local_diagnostic(
    analyzer: &mut Analyzer<'_>,
    callee: &str,
    param: &str,
    local_name: &str,
    span: Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::LOCAL_VALUE_RETAINED,
            format!("retaining API `{callee}` cannot retain local value `{local_name}`."),
            span,
            "local value retained",
        )
        .with_cause(format!("`{callee}` declares `effects(retains({param}))`."))
        .with_fix(
            "manage_local",
            format!("Pass `{param}` through `manage {local_name}` before retaining it."),
            "manual",
        ),
    );
}

fn callback_arity_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    actual: usize,
    expected: usize,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!(
                "callback argument `{arg_name}` for `{call_name}` has {actual} parameter(s), expected {expected}."
            ),
            span.clone(),
            "argument type mismatch",
        )
        .with_cause(
            "`noescape Fn(...) -> T` callback parameter counts are part of the call signature and must be checked before Rust lowering.",
        )
        .with_fix(
            "match_callback_parameter_count",
            format!("Use a callback with {expected} parameter(s)."),
            "manual",
        ),
    );
}

fn callback_call_arity_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    callback_name: &str,
    actual: usize,
    expected: usize,
    span: Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!(
                "callback `{callback_name}` called with {actual} argument(s), expected {expected}."
            ),
            span,
            "argument type mismatch",
        )
        .with_cause(
            "`noescape Fn(...)` callback calls must match the callback parameter contract before Rust lowering.",
        )
        .with_fix(
            "match_callback_call_arity",
            format!("Call `{callback_name}` with {expected} argument(s)."),
            "manual",
        ),
    );
}

fn callback_call_argument_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    callback_name: &str,
    index: usize,
    actual: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!(
                "argument {} for callback `{callback_name}` has type `{actual}`, expected `{expected}`.",
                index + 1
            ),
            span.clone(),
            "argument type mismatch",
        )
        .with_cause(
            "`noescape Fn(...)` callback argument types are part of the callback contract and must be checked before Rust lowering.",
        )
        .with_fix(
            "match_callback_call_argument_type",
            format!("Pass a `{expected}` value for argument {}.", index + 1),
            "manual",
        ),
    );
}

fn callback_call_site_argument_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!(
                "argument `{arg_name}` for `{call_name}` has type `{actual}`, expected `{expected}`."
            ),
            span.clone(),
            "argument type mismatch",
        )
        .with_cause(
            "`noescape Fn(...)` callback parameter types apply to ordinary calls inside callback expressions before Rust lowering.",
        )
        .with_fix(
            "match_callback_body_call_argument_type",
            format!("Pass a `{expected}` value for `{arg_name}`."),
            "manual",
        ),
    );
}

fn type_pattern_matches(expected: &str, actual: &str, generic_params: &[String]) -> bool {
    if argument_type_matches(expected, actual) {
        return true;
    }
    if generic_params.iter().any(|param| param == expected) {
        return true;
    }
    let Some(expected_args) = type_arg_names(expected) else {
        return false;
    };
    let Some(actual_args) = type_arg_names(actual) else {
        return false;
    };
    type_root_name(expected) == type_root_name(actual)
        && expected_args.len() == actual_args.len()
        && expected_args
            .into_iter()
            .zip(actual_args)
            .all(|(expected, actual)| {
                type_pattern_matches(expected.trim(), actual.trim(), generic_params)
            })
}

fn check_argument_variant_payload_type(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
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
                check_argument_payload_type(analyzer, call_name, arg_name, payload, &expected);
            } else if expected != "Unit" {
                argument_payload_type_mismatch_diagnostic(
                    analyzer,
                    call_name,
                    arg_name,
                    "Unit",
                    &expected,
                    hir_expr_span(value),
                );
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
                check_argument_payload_type(analyzer, call_name, arg_name, payload, expected);
            } else if expected != "Unit" {
                argument_payload_type_mismatch_diagnostic(
                    analyzer,
                    call_name,
                    arg_name,
                    "Unit",
                    expected,
                    hir_expr_span(value),
                );
            }
            true
        }
        _ => false,
    }
}

fn check_argument_payload_type(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    payload: &HirExpr,
    expected: &str,
) {
    if let Some((variant, nested_payload)) = enum_variant_payload(payload)
        && let Some(expected_payload) = expected_variant_payload_type(expected, variant)
    {
        if let Some(nested_payload) = nested_payload {
            check_argument_payload_type(
                analyzer,
                call_name,
                arg_name,
                nested_payload,
                &expected_payload,
            );
        } else if expected_payload != "Unit" {
            argument_payload_type_mismatch_diagnostic(
                analyzer,
                call_name,
                arg_name,
                "Unit",
                &expected_payload,
                hir_expr_span(payload),
            );
        }
        return;
    }
    let Some(actual) = hir_expr_type_name(payload) else {
        return;
    };
    if unresolved_generic_type(actual) {
        return;
    }
    if !argument_type_matches(expected, actual) {
        argument_payload_type_mismatch_diagnostic(
            analyzer,
            call_name,
            arg_name,
            actual,
            expected,
            hir_expr_span(payload),
        );
    }
}

fn argument_payload_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::ARGUMENT_TYPE_MISMATCH,
            format!(
                "argument `{arg_name}` for `{call_name}` has payload type `{actual}`, expected `{expected}`."
            ),
            span.clone(),
            "argument type mismatch",
        )
        .with_cause("Result and Option argument constructors are checked against the resolved parameter payload before Rust lowering.")
        .with_fix(
            "match_argument_payload_type",
            format!("Pass a `{expected}` payload for `{arg_name}`."),
            "manual",
        ),
    );
}

fn is_closure_binding_call(
    callee: &Callee,
    _args: &[HirCallArg],
    resolution: &CallResolution,
    callback_bindings: &HashMap<String, CallbackBinding>,
    local_closure_bindings: &HashMap<String, Span>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && matches!(callee, Callee::Name(name) if callback_bindings.contains_key(name) || local_closure_bindings.contains_key(name))
}

fn is_noescape_callback_call(
    callee: &Callee,
    _args: &[HirCallArg],
    resolution: &CallResolution,
    noescape_bindings: &HashMap<String, CallbackBinding>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && matches!(callee, Callee::Name(name) if noescape_bindings.contains_key(name))
}

#[derive(Debug, Clone, Copy)]
enum NoescapeEscapeContext<'a> {
    Store,
    Return,
    UseAsValue,
    Pass { callee: &'a str },
}

#[derive(Debug, Clone, Copy)]
enum LocalClosureEscapeContext<'a> {
    Store,
    Return,
    UseAsValue,
    Pass { callee: &'a str },
}

fn check_local_closure_escape(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    context_span: &Span,
    local_closure_bindings: &HashMap<String, Span>,
    context: LocalClosureEscapeContext<'_>,
) {
    let Some((name, use_span)) = local_closure_escape_use(expr, local_closure_bindings) else {
        return;
    };
    analyzer.diagnostics.push(local_closure_escape_diagnostic(
        name,
        use_span,
        context_span.clone(),
        context,
    ));
}

fn local_closure_escape_use<'a>(
    expr: &'a HirExpr,
    local_closure_bindings: &'a HashMap<String, Span>,
) -> Option<(&'a str, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } if local_closure_bindings.contains_key(name) => {
            Some((name.as_str(), span.clone()))
        }
        HirExpr::Call {
            callee,
            args,
            resolution,
            ..
        } if is_local_closure_call(callee, args, resolution, local_closure_bindings) => None,
        HirExpr::Call {
            args, resolution, ..
        } => {
            for arg in args {
                if call_arg_targets_noescape_param(arg, resolution) {
                    continue;
                }
                if let Some(found) = local_closure_escape_use(&arg.value, local_closure_bindings) {
                    return Some(found);
                }
            }
            None
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => local_closure_escape_use(value, local_closure_bindings),
        HirExpr::Binary { left, right, .. } => {
            local_closure_escape_use(left, local_closure_bindings)
                .or_else(|| local_closure_escape_use(right, local_closure_bindings))
        }
        HirExpr::Field { base, .. } => local_closure_escape_use(base, local_closure_bindings),
        HirExpr::Index { base, index, .. } => {
            local_closure_escape_use(base, local_closure_bindings)
                .or_else(|| local_closure_escape_use(index, local_closure_bindings))
        }
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                if let Some(found) =
                    local_closure_any_use_in_stmt(statement, local_closure_bindings)
                {
                    return Some(found);
                }
            }
            None
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn is_local_closure_call(
    callee: &Callee,
    args: &[HirCallArg],
    resolution: &CallResolution,
    local_closure_bindings: &HashMap<String, Span>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && args.is_empty()
        && matches!(callee, Callee::Name(name) if local_closure_bindings.contains_key(name))
}

fn check_noescape_escape(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    context_span: &Span,
    noescape_bindings: &HashMap<String, CallbackBinding>,
    context: NoescapeEscapeContext<'_>,
) {
    let Some((name, use_span)) = noescape_escape_use(expr, noescape_bindings) else {
        return;
    };
    analyzer.diagnostics.push(noescape_escape_diagnostic(
        name,
        use_span,
        context_span.clone(),
        context,
    ));
}

fn noescape_escape_use<'a>(
    expr: &'a HirExpr,
    noescape_bindings: &'a HashMap<String, CallbackBinding>,
) -> Option<(&'a str, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } if noescape_bindings.contains_key(name) => {
            Some((name.as_str(), span.clone()))
        }
        HirExpr::Call {
            callee,
            args,
            resolution,
            ..
        } if is_noescape_callback_call(callee, args, resolution, noescape_bindings) => None,
        HirExpr::Call {
            args, resolution, ..
        } => {
            for arg in args {
                if call_arg_targets_noescape_param(arg, resolution) {
                    continue;
                }
                if let Some(found) = noescape_escape_use(&arg.value, noescape_bindings) {
                    return Some(found);
                }
            }
            None
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => noescape_escape_use(value, noescape_bindings),
        HirExpr::Binary { left, right, .. } => noescape_escape_use(left, noescape_bindings)
            .or_else(|| noescape_escape_use(right, noescape_bindings)),
        HirExpr::Field { base, .. } => noescape_escape_use(base, noescape_bindings),
        HirExpr::Index { base, index, .. } => noescape_escape_use(base, noescape_bindings)
            .or_else(|| noescape_escape_use(index, noescape_bindings)),
        HirExpr::Closure { body, .. } => {
            for statement in &body.statements {
                if let Some(found) = noescape_any_use_in_stmt(statement, noescape_bindings) {
                    return Some(found);
                }
            }
            None
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn noescape_any_use_in_stmt<'a>(
    statement: &'a HirStmt,
    noescape_bindings: &'a HashMap<String, CallbackBinding>,
) -> Option<(&'a str, Span)> {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => noescape_any_use(value, noescape_bindings),
        HirStmt::With { resource, body, .. } => noescape_any_use(resource, noescape_bindings)
            .or_else(|| {
                body.statements
                    .iter()
                    .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings))
            }),
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => noescape_any_use(condition, noescape_bindings)
            .or_else(|| {
                then_body
                    .statements
                    .iter()
                    .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings))
            })
            .or_else(|| {
                else_body.as_ref().and_then(|body| {
                    body.statements.iter().find_map(|statement| {
                        noescape_any_use_in_stmt(statement, noescape_bindings)
                    })
                })
            }),
        HirStmt::Loop {
            condition, body, ..
        } => condition
            .as_ref()
            .and_then(|condition| noescape_any_use(condition, noescape_bindings))
            .or_else(|| {
                body.statements
                    .iter()
                    .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings))
            }),
        HirStmt::For { iterable, body, .. } => noescape_any_use(iterable, noescape_bindings)
            .or_else(|| {
                body.statements
                    .iter()
                    .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings))
            }),
        HirStmt::Match { value, arms, .. } => {
            noescape_any_use(value, noescape_bindings).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body.statements.iter().find_map(|statement| {
                        noescape_any_use_in_stmt(statement, noescape_bindings)
                    })
                })
            })
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => None,
    }
}

fn noescape_any_use<'a>(
    expr: &'a HirExpr,
    noescape_bindings: &'a HashMap<String, CallbackBinding>,
) -> Option<(&'a str, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } if noescape_bindings.contains_key(name) => {
            Some((name.as_str(), span.clone()))
        }
        HirExpr::Call {
            callee, args, span, ..
        } => {
            if let Callee::Name(name) = callee
                && noescape_bindings.contains_key(name)
            {
                return Some((name.as_str(), span.clone()));
            }
            args.iter()
                .find_map(|arg| noescape_any_use(&arg.value, noescape_bindings))
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => noescape_any_use(value, noescape_bindings),
        HirExpr::Binary { left, right, .. } => noescape_any_use(left, noescape_bindings)
            .or_else(|| noescape_any_use(right, noescape_bindings)),
        HirExpr::Field { base, .. } => noescape_any_use(base, noescape_bindings),
        HirExpr::Index { base, index, .. } => noescape_any_use(base, noescape_bindings)
            .or_else(|| noescape_any_use(index, noescape_bindings)),
        HirExpr::Closure { body, .. } => body
            .statements
            .iter()
            .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings)),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn local_closure_any_use_in_stmt<'a>(
    statement: &'a HirStmt,
    local_closure_bindings: &'a HashMap<String, Span>,
) -> Option<(&'a str, Span)> {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => local_closure_any_use(value, local_closure_bindings),
        HirStmt::With { resource, body, .. } => {
            local_closure_any_use(resource, local_closure_bindings).or_else(|| {
                body.statements.iter().find_map(|statement| {
                    local_closure_any_use_in_stmt(statement, local_closure_bindings)
                })
            })
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => local_closure_any_use(condition, local_closure_bindings)
            .or_else(|| {
                then_body.statements.iter().find_map(|statement| {
                    local_closure_any_use_in_stmt(statement, local_closure_bindings)
                })
            })
            .or_else(|| {
                else_body.as_ref().and_then(|body| {
                    body.statements.iter().find_map(|statement| {
                        local_closure_any_use_in_stmt(statement, local_closure_bindings)
                    })
                })
            }),
        HirStmt::Loop {
            condition, body, ..
        } => condition
            .as_ref()
            .and_then(|condition| local_closure_any_use(condition, local_closure_bindings))
            .or_else(|| {
                body.statements.iter().find_map(|statement| {
                    local_closure_any_use_in_stmt(statement, local_closure_bindings)
                })
            }),
        HirStmt::For { iterable, body, .. } => {
            local_closure_any_use(iterable, local_closure_bindings).or_else(|| {
                body.statements.iter().find_map(|statement| {
                    local_closure_any_use_in_stmt(statement, local_closure_bindings)
                })
            })
        }
        HirStmt::Match { value, arms, .. } => local_closure_any_use(value, local_closure_bindings)
            .or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body.statements.iter().find_map(|statement| {
                        local_closure_any_use_in_stmt(statement, local_closure_bindings)
                    })
                })
            }),
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => None,
    }
}

fn local_closure_any_use<'a>(
    expr: &'a HirExpr,
    local_closure_bindings: &'a HashMap<String, Span>,
) -> Option<(&'a str, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } if local_closure_bindings.contains_key(name) => {
            Some((name.as_str(), span.clone()))
        }
        HirExpr::Call {
            callee, args, span, ..
        } => {
            if let Callee::Name(name) = callee
                && local_closure_bindings.contains_key(name)
            {
                return Some((name.as_str(), span.clone()));
            }
            args.iter()
                .find_map(|arg| local_closure_any_use(&arg.value, local_closure_bindings))
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => local_closure_any_use(value, local_closure_bindings),
        HirExpr::Binary { left, right, .. } => local_closure_any_use(left, local_closure_bindings)
            .or_else(|| local_closure_any_use(right, local_closure_bindings)),
        HirExpr::Field { base, .. } => local_closure_any_use(base, local_closure_bindings),
        HirExpr::Index { base, index, .. } => local_closure_any_use(base, local_closure_bindings)
            .or_else(|| local_closure_any_use(index, local_closure_bindings)),
        HirExpr::Closure { body, .. } => body
            .statements
            .iter()
            .find_map(|statement| local_closure_any_use_in_stmt(statement, local_closure_bindings)),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn call_arg_targets_noescape_param(arg: &HirCallArg, resolution: &CallResolution) -> bool {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return false;
    };
    arg.name
        .as_ref()
        .and_then(|name| signature.params.iter().find(|param| param.name == *name))
        .is_some_and(|param| is_noescape_fn_type(&param.type_name))
}

fn is_noescape_fn_type(type_name: &str) -> bool {
    type_name.trim().starts_with("noescape ") && is_fn_type(type_name)
}

fn is_fn_type(type_name: &str) -> bool {
    let type_name = type_name.trim();
    type_name
        .strip_prefix("noescape ")
        .unwrap_or(type_name)
        .strip_prefix("Fn(")
        .and_then(|rest| rest.split_once(')'))
        .is_some()
}

fn noescape_return_type(type_name: &str) -> Option<&str> {
    type_name
        .trim()
        .strip_prefix("noescape ")
        .and_then(fn_return_type)
}

fn fn_return_type(type_name: &str) -> Option<&str> {
    let type_name = type_name.trim();
    type_name
        .strip_prefix("noescape ")
        .unwrap_or(type_name)
        .strip_prefix("Fn(")
        .and_then(|rest| rest.split_once(')'))
        .and_then(|(_, rest)| rest.trim_start().strip_prefix("->"))
        .map(str::trim)
}

fn noescape_param_types(type_name: &str) -> Vec<&str> {
    type_name
        .trim()
        .strip_prefix("noescape ")
        .map(fn_param_types)
        .unwrap_or_default()
}

fn fn_param_types(type_name: &str) -> Vec<&str> {
    let type_name = type_name.trim();
    let Some(params) = type_name
        .strip_prefix("noescape ")
        .unwrap_or(type_name)
        .strip_prefix("Fn(")
        .and_then(|rest| rest.split_once(')').map(|(params, _)| params.trim()))
    else {
        return Vec::new();
    };
    if params.is_empty() {
        Vec::new()
    } else {
        split_top_level_type_args(params)
    }
}

fn noescape_escape_diagnostic(
    name: &str,
    use_span: Span,
    context_span: Span,
    context: NoescapeEscapeContext<'_>,
) -> Diagnostic {
    let (summary, cause) = match context {
        NoescapeEscapeContext::Store => (
            format!("noescape callback `{name}` cannot be stored."),
            "`noescape Fn()` parameters are temporary callback capabilities and cannot be bound into stored values.".to_string(),
        ),
        NoescapeEscapeContext::Return => (
            format!("noescape callback `{name}` cannot be returned."),
            "`noescape Fn()` parameters cannot escape the current function through a return value.".to_string(),
        ),
        NoescapeEscapeContext::UseAsValue => (
            format!("noescape callback `{name}` cannot be used as an ordinary value."),
            "Call the noescape callback directly, or pass it to another resolved `noescape Fn()` parameter.".to_string(),
        ),
        NoescapeEscapeContext::Pass { callee } => (
            format!("noescape callback `{name}` cannot be passed to `{callee}` as an ordinary value."),
            "Forwarding a noescape callback is only allowed when the target parameter is also `noescape Fn()`.".to_string(),
        ),
    };
    Diagnostic::error(
        code::NOESCAPE_CALLBACK_ESCAPE,
        summary,
        use_span,
        "noescape callback escapes",
    )
    .with_cause(cause)
    .with_cause(format!(
        "The escaping context starts at {}:{}.",
        context_span.line, context_span.column
    ))
    .with_fix(
        "keep_noescape_local",
        "Call the callback directly, or change the API to an ordinary managed callback type.",
        "manual",
    )
}

fn local_closure_escape_diagnostic(
    name: &str,
    use_span: Span,
    context_span: Span,
    context: LocalClosureEscapeContext<'_>,
) -> Diagnostic {
    let (summary, cause) = match context {
        LocalClosureEscapeContext::Store => (
            format!("local closure `{name}` cannot be stored in a managed binding."),
            "A closure bound with `local` is an exclusive local capability and cannot become managed data.".to_string(),
        ),
        LocalClosureEscapeContext::Return => (
            format!("local closure `{name}` cannot be returned."),
            "A local closure cannot escape the function where its local captures are valid.".to_string(),
        ),
        LocalClosureEscapeContext::UseAsValue => (
            format!("local closure `{name}` cannot be used as an ordinary value."),
            "Call the local closure directly, or pass it to a resolved `noescape Fn()` parameter.".to_string(),
        ),
        LocalClosureEscapeContext::Pass { callee } => (
            format!("local closure `{name}` cannot be passed to `{callee}` as an ordinary value."),
            "Forwarding a local closure is only allowed when the target parameter is `noescape Fn()`.".to_string(),
        ),
    };
    Diagnostic::error(
        code::LOCAL_CLOSURE_ESCAPE,
        summary,
        use_span,
        "local closure escapes",
    )
    .with_cause(cause)
    .with_cause(format!(
        "The escaping context starts at {}:{}.",
        context_span.line, context_span.column
    ))
    .with_fix(
        "keep_local_closure_noescape",
        "Call the closure locally, or pass it to a noescape callback parameter.",
        "manual",
    )
}

fn join_param_names(params: &[crate::hir::ParamSig]) -> String {
    params
        .iter()
        .map(|param| param.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn callee_name(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { name, .. } => name.clone(),
    }
}

fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
    }
}

fn expr_data_effect(expr: &HirExpr) -> Option<&'static str> {
    match expr {
        HirExpr::Effect { effect, .. } => Some(effect.as_str()),
        _ => None,
    }
}

fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident {
            name, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| builtin_value_type_name(name)),
        HirExpr::Number { .. } => Some("Int"),
        HirExpr::String { .. } => Some("String"),
        HirExpr::Call {
            callee, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| enum_variant_type_name(callee)),
        HirExpr::Effect {
            value, type_name, ..
        }
        | HirExpr::Manage {
            value, type_name, ..
        }
        | HirExpr::Spawn {
            value, type_name, ..
        }
        | HirExpr::Await {
            value, type_name, ..
        }
        | HirExpr::Try {
            value, type_name, ..
        } => type_name.as_deref().or_else(|| hir_expr_type_name(value)),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Binary { .. }
        | HirExpr::Index { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn builtin_value_type_name(name: &str) -> Option<&'static str> {
    match name {
        "true" | "false" => Some("Bool"),
        "Unit" => Some("Unit"),
        "None" => Some("Option<?>"),
        _ => None,
    }
}

fn enum_variant_type_name(callee: &Callee) -> Option<&'static str> {
    match callee_name(callee).as_str() {
        "Some" | "None" => Some("Option<?>"),
        "Ok" | "Err" => Some("Result<?>"),
        _ => None,
    }
}

fn argument_type_matches(expected: &str, actual: &str) -> bool {
    if expected == actual {
        return true;
    }
    if expected == "Self" {
        return true;
    }
    if strip_fresh_type(expected) == strip_fresh_type(actual) {
        return true;
    }
    if actual == "Option<?>" {
        return type_root_name(expected) == "Option";
    }
    if actual == "Result<?>" {
        return type_root_name(expected) == "Result";
    }
    false
}

fn strip_fresh_type(type_name: &str) -> &str {
    type_name
        .trim()
        .strip_prefix("fresh ")
        .unwrap_or(type_name.trim())
}

fn is_result_type_name(type_name: &str) -> bool {
    type_root_name(type_name) == "Result"
}

fn is_option_type_name(type_name: &str) -> bool {
    type_root_name(type_name) == "Option"
}

fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        format!(
            "{}<{}>",
            ty.name,
            ty.args
                .iter()
                .map(type_ref_name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

fn type_contains_unresolved_generic(type_name: &str, generics: &[String]) -> bool {
    generics.iter().any(|generic| {
        type_name == generic
            || noescape_return_type(type_name)
                .is_some_and(|return_type| type_contains_unresolved_generic(return_type, generics))
            || noescape_param_types(type_name)
                .iter()
                .any(|param_type| type_contains_unresolved_generic(param_type, generics))
            || type_arg_names(type_name).is_some_and(|args| {
                args.iter()
                    .any(|arg| type_contains_unresolved_generic(arg, generics))
            })
    })
}

fn unresolved_generic_type(type_name: &str) -> bool {
    let root = type_root_name(type_name);
    (root.len() == 1 && root.chars().all(|ch| ch.is_ascii_uppercase()))
        || type_arg_names(type_name)
            .is_some_and(|args| args.iter().any(|arg| unresolved_generic_type(arg)))
        || noescape_return_type(type_name).is_some_and(unresolved_generic_type)
        || noescape_param_types(type_name)
            .iter()
            .any(|param_type| unresolved_generic_type(param_type))
}

fn type_arg_names(type_name: &str) -> Option<Vec<&str>> {
    let inner = type_name
        .split_once('<')
        .and_then(|(_, rest)| rest.strip_suffix('>'))?;
    Some(split_top_level_type_args(inner))
}

fn builtin_generic_type_params(root: &str) -> Option<Vec<&'static str>> {
    match root {
        "List" | "Set" | "Option" | "ResourcePool" => Some(vec!["T"]),
        "Map" | "Result" => Some(vec!["K", "V"]),
        _ => None,
    }
}

fn split_top_level_type_args(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(args[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    if start < args.len() {
        parts.push(args[start..].trim());
    }
    parts
}

fn type_root_name(type_name: &str) -> &str {
    let type_name = type_name
        .trim()
        .strip_prefix("fresh ")
        .unwrap_or(type_name.trim());
    type_name
        .split_once('<')
        .map_or(type_name, |(root, _)| root)
}

fn hir_expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
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
        | HirExpr::Unknown(span) => span,
    }
}
