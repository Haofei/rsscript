//! Closure call contracts, callback return checking, and escape restrictions.

use super::*;

pub(super) fn check_fn_closure_contract(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    expected_type: &str,
    generic_params: &[String],
    value: &HirExpr,
) -> bool {
    if !is_fn_type(expected_type) {
        return false;
    }
    let HirExpr::Closure { params, body, .. } = value else {
        return false;
    };
    let expected_params = fn_param_types(expected_type);
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
    let expected_return = fn_return_type(expected_type).unwrap_or("Unit");
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

pub(super) fn check_callback_call_args(
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
        if has_unresolved_generic_fact(analyzer, actual) {
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

pub(super) enum ClosureReturn<'a> {
    Expr(&'a HirExpr),
    Unit(&'a Span),
}

pub(super) struct CallbackContract<'a> {
    call_name: &'a str,
    arg_name: &'a str,
    expected_return: &'a str,
    generic_params: &'a [String],
    params: &'a [String],
    param_types: &'a [&'a str],
    local_bindings: HashSet<String>,
    managed_bindings: HashSet<String>,
}

pub(super) fn closure_binding_sets(body: &HirBlock) -> (HashSet<String>, HashSet<String>) {
    let mut local_bindings = HashSet::new();
    let mut managed_bindings = HashSet::new();
    collect_closure_binding_sets(body, &mut local_bindings, &mut managed_bindings);
    (local_bindings, managed_bindings)
}

pub(super) fn collect_closure_binding_sets(
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
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_closure_binding_sets(&arm.body, local_bindings, managed_bindings);
                }
            }
            HirStmt::Return { .. }
            | HirStmt::Expr(_)
            | HirStmt::Assign { .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

pub(super) fn closure_return_sites(body: &HirBlock) -> Vec<ClosureReturn<'_>> {
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

pub(super) fn collect_explicit_return_sites<'a>(
    statement: &'a HirStmt,
    returns: &mut Vec<ClosureReturn<'a>>,
) {
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
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_explicit_return_sites_from_block(&arm.body, returns);
            }
        }
        HirStmt::Let { .. }
        | HirStmt::Expr(_)
        | HirStmt::Assign { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_explicit_return_sites_from_block<'a>(
    block: &'a HirBlock,
    returns: &mut Vec<ClosureReturn<'a>>,
) {
    for statement in &block.statements {
        collect_explicit_return_sites(statement, returns);
    }
}

pub(super) fn collect_implicit_closure_return_sites<'a>(
    statement: &'a HirStmt,
    returns: &mut Vec<ClosureReturn<'a>>,
) {
    match statement {
        HirStmt::Expr(value) | HirStmt::Assign { value, .. } => {
            returns.push(ClosureReturn::Expr(value))
        }
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
        | HirStmt::Select { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn check_callback_body_call_argument_types(
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
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                check_callback_call_argument_types(analyzer, value, contract)
            }
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
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    check_callback_call_argument_types(analyzer, &arm.operation, contract);
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

pub(super) fn check_callback_return_expr_type(
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

pub(super) fn check_callback_variant_return_type(
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

pub(super) fn check_callback_payload_return_type(
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

pub(super) fn check_callback_fresh_return_type(
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

pub(super) fn callback_expr_is_fresh_value(
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
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Match { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => false,
    }
}

pub(super) fn fresh_return_ident(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => fresh_return_ident(value),
        HirExpr::Field { base, access, .. } if !access.is_handle && !access.is_weak => {
            fresh_return_ident(base)
        }
        HirExpr::Manage { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Match { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn callback_expr_type_name(
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
            BinaryOp::Add
            | BinaryOp::Subtract
            | BinaryOp::Multiply
            | BinaryOp::Divide
            | BinaryOp::Modulo => None,
            BinaryOp::BitAnd
            | BinaryOp::BitOr
            | BinaryOp::BitXor
            | BinaryOp::ShiftLeft
            | BinaryOp::ShiftRight => Some("Int".to_string()),
        };
    }
    hir_expr_type_name(expr).map(str::to_string)
}

pub(super) fn check_callback_call_argument_types(
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
        HirExpr::Match { value, arms, .. } => {
            check_callback_call_argument_types(analyzer, value, contract);
            for arm in arms {
                check_callback_body_call_argument_types(analyzer, &arm.body, contract);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_callback_call_argument_types(analyzer, &entry.key, contract);
                check_callback_call_argument_types(analyzer, &entry.value, contract);
            }
        }
        HirExpr::Closure { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn check_callback_resolved_call_argument_types(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    signature: &FunctionSig,
    contract: &CallbackContract<'_>,
) {
    let call_name = callee_display(callee);
    let Some(type_param_substitutions) =
        call_type_param_substitutions(analyzer, None, callee, args, signature)
    else {
        return;
    };
    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        let Some(expected_param) = signature.params.iter().find(|param| param.name == *name) else {
            continue;
        };
        let Ok(expected_type) = substitute_type_params(
            &analyzer.budget,
            &expected_param.ty.to_string(),
            &type_param_substitutions,
        ) else {
            return;
        };
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
        if has_unresolved_generic_fact(analyzer, &actual_type) {
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

pub(super) fn callback_retained_local_use(
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
        HirExpr::ObjectLiteral { fields, .. } => fields
            .iter()
            .find_map(|field| callback_retained_local_use(&field.value, contract)),
        HirExpr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| callback_retained_local_use(&entry.key, contract))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| callback_retained_local_use(&entry.value, contract))
            }),
        HirExpr::ArrayLiteral { items, .. } => items
            .iter()
            .find_map(|item| callback_retained_local_use(item, contract)),
        HirExpr::Binary { left, right, .. } => callback_retained_local_use(left, contract)
            .or_else(|| callback_retained_local_use(right, contract)),
        HirExpr::Index { base, index, .. } => callback_retained_local_use(base, contract)
            .or_else(|| callback_retained_local_use(index, contract)),
        HirExpr::Match { value, arms, .. } => {
            callback_retained_local_use(value, contract).or_else(|| {
                arms.iter()
                    .find_map(|arm| callback_retained_local_use_in_block(&arm.body, contract))
            })
        }
        HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Field { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_)
        | HirExpr::Ident { .. } => None,
    }
}

pub(super) fn callback_retained_local_use_in_block(
    body: &HirBlock,
    contract: &CallbackContract<'_>,
) -> Option<(String, Span)> {
    body.statements
        .iter()
        .find_map(|statement| match statement {
            HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => callback_retained_local_use(value, contract),
            HirStmt::With { resource, body, .. } => callback_retained_local_use(resource, contract)
                .or_else(|| callback_retained_local_use_in_block(body, contract)),
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => callback_retained_local_use(condition, contract)
                .or_else(|| callback_retained_local_use_in_block(then_body, contract))
                .or_else(|| {
                    else_body
                        .as_ref()
                        .and_then(|body| callback_retained_local_use_in_block(body, contract))
                }),
            HirStmt::Loop {
                condition, body, ..
            } => condition
                .as_ref()
                .and_then(|condition| callback_retained_local_use(condition, contract))
                .or_else(|| callback_retained_local_use_in_block(body, contract)),
            HirStmt::For { iterable, body, .. } => callback_retained_local_use(iterable, contract)
                .or_else(|| callback_retained_local_use_in_block(body, contract)),
            HirStmt::Match { value, arms, .. } => callback_retained_local_use(value, contract)
                .or_else(|| {
                    arms.iter()
                        .find_map(|arm| callback_retained_local_use_in_block(&arm.body, contract))
                }),
            HirStmt::Select { arms, .. } => arms.iter().find_map(|arm| {
                callback_retained_local_use(&arm.operation, contract)
                    .or_else(|| callback_retained_local_use_in_block(&arm.body, contract))
            }),
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => None,
        })
}

pub(super) fn check_callback_operator_operand_types(
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
                BinaryOp::Add
                | BinaryOp::Subtract
                | BinaryOp::Multiply
                | BinaryOp::Divide
                | BinaryOp::Modulo => {}
                BinaryOp::BitAnd
                | BinaryOp::BitOr
                | BinaryOp::BitXor
                | BinaryOp::ShiftLeft
                | BinaryOp::ShiftRight => {
                    if type_root_name(&left_type) != "Int" || type_root_name(&right_type) != "Int" {
                        callback_operator_type_mismatch_diagnostic(
                            analyzer,
                            span,
                            callback_operator_label(*op),
                            &left_type,
                            &right_type,
                            "Int operands",
                        );
                    }
                }
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
        HirExpr::Match { value, arms, .. } => {
            check_callback_operator_operand_types(analyzer, value, contract);
            for arm in arms {
                check_callback_body_operator_operand_types(analyzer, &arm.body, contract);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                check_callback_operator_operand_types(analyzer, &entry.key, contract);
                check_callback_operator_operand_types(analyzer, &entry.value, contract);
            }
        }
        HirExpr::Closure { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn check_callback_body_operator_operand_types(
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
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                check_callback_operator_operand_types(analyzer, value, contract)
            }
            HirStmt::With { resource, body, .. } => {
                check_callback_operator_operand_types(analyzer, resource, contract);
                check_callback_body_operator_operand_types(analyzer, body, contract);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                check_callback_operator_operand_types(analyzer, condition, contract);
                check_callback_body_operator_operand_types(analyzer, then_body, contract);
                if let Some(else_body) = else_body {
                    check_callback_body_operator_operand_types(analyzer, else_body, contract);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    check_callback_operator_operand_types(analyzer, condition, contract);
                }
                check_callback_body_operator_operand_types(analyzer, body, contract);
            }
            HirStmt::For { iterable, body, .. } => {
                check_callback_operator_operand_types(analyzer, iterable, contract);
                check_callback_body_operator_operand_types(analyzer, body, contract);
            }
            HirStmt::Match { value, arms, .. } => {
                check_callback_operator_operand_types(analyzer, value, contract);
                for arm in arms {
                    check_callback_body_operator_operand_types(analyzer, &arm.body, contract);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    check_callback_operator_operand_types(analyzer, &arm.operation, contract);
                    check_callback_body_operator_operand_types(analyzer, &arm.body, contract);
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

pub(super) fn callback_operator_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    span: &Span,
    operator: &str,
    left_type: &str,
    right_type: &str,
    expected: &str,
) {
    analyzer.diagnostics.push(
        rsscript_semantics::callback_operator_type_mismatch_diagnostic(
            operator,
            left_type,
            right_type,
            expected,
            span.clone(),
        ),
    );
}

pub(super) fn callback_operator_label(op: BinaryOp) -> &'static str {
    match op {
        BinaryOp::Add => "+",
        BinaryOp::Subtract => "-",
        BinaryOp::Multiply => "*",
        BinaryOp::Divide => "/",
        BinaryOp::Modulo => "%",
        BinaryOp::BitAnd => "&",
        BinaryOp::BitOr => "|",
        BinaryOp::BitXor => "^",
        BinaryOp::ShiftLeft => "<<",
        BinaryOp::ShiftRight => ">>",
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

pub(super) fn is_numeric_type_name(type_name: &str) -> bool {
    matches!(type_root_name(type_name), "Int" | "Float")
}

pub(super) fn callback_return_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        rsscript_semantics::callback_return_type_mismatch_diagnostic(
            call_name,
            arg_name,
            actual,
            expected,
            span.clone(),
        ),
    );
}

pub(super) fn callback_fresh_return_not_clean_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    name: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        rsscript_semantics::callback_fresh_return_not_clean_diagnostic(
            call_name,
            arg_name,
            name,
            expected,
            span.clone(),
        ),
    );
}

pub(super) fn callback_fresh_return_unknown_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        rsscript_semantics::callback_fresh_return_unknown_diagnostic(
            call_name,
            arg_name,
            expected,
            span.clone(),
        ),
    );
}

pub(super) fn callback_retained_local_diagnostic(
    analyzer: &mut Analyzer<'_>,
    callee: &str,
    param: &str,
    local_name: &str,
    span: Span,
) {
    analyzer
        .diagnostics
        .push(rsscript_semantics::retained_local_diagnostic(
            local_name, callee, param, span,
        ));
}

pub(super) fn callback_arity_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    actual: usize,
    expected: usize,
    span: &Span,
) {
    analyzer
        .diagnostics
        .push(rsscript_semantics::callback_arity_mismatch_diagnostic(
            call_name,
            arg_name,
            actual,
            expected,
            span.clone(),
        ));
}

pub(super) fn callback_call_arity_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    callback_name: &str,
    actual: usize,
    expected: usize,
    span: Span,
) {
    analyzer
        .diagnostics
        .push(rsscript_semantics::callback_call_arity_mismatch_diagnostic(
            callback_name,
            actual,
            expected,
            span,
        ));
}

pub(super) fn callback_call_argument_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    callback_name: &str,
    index: usize,
    actual: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        rsscript_semantics::callback_call_argument_type_mismatch_diagnostic(
            callback_name,
            index,
            actual,
            expected,
            span.clone(),
        ),
    );
}

pub(super) fn callback_call_site_argument_type_mismatch_diagnostic(
    analyzer: &mut Analyzer<'_>,
    call_name: &str,
    arg_name: &str,
    actual: &str,
    expected: &str,
    span: &Span,
) {
    analyzer.diagnostics.push(
        rsscript_semantics::callback_call_site_argument_type_mismatch_diagnostic(
            call_name,
            arg_name,
            actual,
            expected,
            span.clone(),
        ),
    );
}

pub(super) fn type_pattern_matches(
    expected: &str,
    actual: &str,
    generic_params: &[String],
) -> bool {
    if argument_type_matches(expected, actual) {
        return true;
    }
    if let Some(expected) = fresh_type_target(expected) {
        let actual = fresh_type_target(actual).unwrap_or(actual);
        return type_pattern_matches(expected, actual, generic_params);
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

pub(super) fn is_closure_binding_call(
    callee: &Callee,
    _args: &[HirCallArg],
    resolution: &CallResolution,
    callback_bindings: &HashMap<String, CallbackBinding>,
    local_closure_bindings: &HashMap<String, Span>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && matches!(callee, Callee::Name(name) if callback_bindings.contains_key(name) || local_closure_bindings.contains_key(name))
}

pub(super) fn is_noescape_callback_call(
    callee: &Callee,
    _args: &[HirCallArg],
    resolution: &CallResolution,
    noescape_bindings: &HashMap<String, CallbackBinding>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && matches!(callee, Callee::Name(name) if noescape_bindings.contains_key(name))
}

pub(super) use rsscript_semantics::ClosureEscapeContext as LocalClosureEscapeContext;
pub(super) use rsscript_semantics::ClosureEscapeContext as NoescapeEscapeContext;

pub(super) fn check_local_closure_escape(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    context_span: &Span,
    local_closure_bindings: &HashMap<String, Span>,
    context: LocalClosureEscapeContext<'_>,
) {
    let Some((name, use_span)) = local_closure_escape_use(expr, local_closure_bindings) else {
        return;
    };
    analyzer
        .diagnostics
        .push(rsscript_semantics::local_closure_escape_diagnostic(
            name,
            use_span,
            context_span.clone(),
            context,
        ));
}

pub(super) fn local_closure_escape_use<'a>(
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
        HirExpr::Match { value, arms, .. } => {
            local_closure_escape_use(value, local_closure_bindings).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body.statements.iter().find_map(|statement| {
                        local_closure_any_use_in_stmt(statement, local_closure_bindings)
                    })
                })
            })
        }
        HirExpr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| local_closure_escape_use(&entry.key, local_closure_bindings))
            .or_else(|| {
                entries.iter().find_map(|entry| {
                    local_closure_escape_use(&entry.value, local_closure_bindings)
                })
            }),
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn is_local_closure_call(
    callee: &Callee,
    args: &[HirCallArg],
    resolution: &CallResolution,
    local_closure_bindings: &HashMap<String, Span>,
) -> bool {
    matches!(resolution, CallResolution::Unknown)
        && args.is_empty()
        && matches!(callee, Callee::Name(name) if local_closure_bindings.contains_key(name))
}

pub(super) fn check_noescape_escape(
    analyzer: &mut Analyzer<'_>,
    expr: &HirExpr,
    context_span: &Span,
    noescape_bindings: &HashMap<String, CallbackBinding>,
    context: NoescapeEscapeContext<'_>,
) {
    let Some((name, use_span)) = noescape_escape_use(expr, noescape_bindings) else {
        return;
    };
    analyzer
        .diagnostics
        .push(rsscript_semantics::noescape_escape_diagnostic(
            name,
            use_span,
            context_span.clone(),
            context,
        ));
}

pub(super) fn noescape_escape_use<'a>(
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
        HirExpr::Match { value, arms, .. } => noescape_escape_use(value, noescape_bindings)
            .or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body.statements.iter().find_map(|statement| {
                        noescape_any_use_in_stmt(statement, noescape_bindings)
                    })
                })
            }),
        HirExpr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| noescape_escape_use(&entry.key, noescape_bindings))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| noescape_escape_use(&entry.value, noescape_bindings))
            }),
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn noescape_any_use_in_stmt<'a>(
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
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => noescape_any_use(value, noescape_bindings),
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
        HirStmt::Select { arms, .. } => arms.iter().find_map(|arm| {
            noescape_any_use(&arm.operation, noescape_bindings).or_else(|| {
                arm.body
                    .statements
                    .iter()
                    .find_map(|statement| noescape_any_use_in_stmt(statement, noescape_bindings))
            })
        }),
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => None,
    }
}

pub(super) fn noescape_any_use<'a>(
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
        HirExpr::Match { value, arms, .. } => {
            noescape_any_use(value, noescape_bindings).or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body.statements.iter().find_map(|statement| {
                        noescape_any_use_in_stmt(statement, noescape_bindings)
                    })
                })
            })
        }
        HirExpr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| noescape_any_use(&entry.key, noescape_bindings))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| noescape_any_use(&entry.value, noescape_bindings))
            }),
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn local_closure_any_use_in_stmt<'a>(
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
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => local_closure_any_use(value, local_closure_bindings),
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
        HirStmt::Select { arms, .. } => arms.iter().find_map(|arm| {
            local_closure_any_use(&arm.operation, local_closure_bindings).or_else(|| {
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

pub(super) fn local_closure_any_use<'a>(
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
        HirExpr::Match { value, arms, .. } => local_closure_any_use(value, local_closure_bindings)
            .or_else(|| {
                arms.iter().find_map(|arm| {
                    arm.body.statements.iter().find_map(|statement| {
                        local_closure_any_use_in_stmt(statement, local_closure_bindings)
                    })
                })
            }),
        HirExpr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| local_closure_any_use(&entry.key, local_closure_bindings))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| local_closure_any_use(&entry.value, local_closure_bindings))
            }),
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn call_arg_targets_noescape_param(
    arg: &HirCallArg,
    resolution: &CallResolution,
) -> bool {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return false;
    };
    arg.name
        .as_ref()
        .and_then(|name| signature.params.iter().find(|param| param.name == *name))
        .is_some_and(|param| param.ty.qualifiers.noescape && param.ty.is_function())
}

pub(super) fn is_noescape_fn_type(type_name: &str) -> bool {
    type_name.trim().starts_with("noescape ") && is_fn_type(type_name)
}

pub(super) fn is_fn_type(type_name: &str) -> bool {
    let type_name = type_name.trim();
    type_name
        .strip_prefix("noescape ")
        .or_else(|| type_name.strip_prefix("owned "))
        .unwrap_or(type_name)
        .strip_prefix("Fn(")
        .and_then(|rest| rest.split_once(')'))
        .is_some()
}

pub(super) fn fn_return_type(type_name: &str) -> Option<&str> {
    let type_name = type_name.trim();
    type_name
        .strip_prefix("noescape ")
        .or_else(|| type_name.strip_prefix("owned "))
        .unwrap_or(type_name)
        .strip_prefix("Fn(")
        .and_then(|rest| rest.split_once(')'))
        .and_then(|(_, rest)| rest.trim_start().strip_prefix("->"))
        .map(str::trim)
}

pub(super) fn fn_param_types(type_name: &str) -> Vec<&str> {
    let type_name = type_name.trim();
    let Some(params) = type_name
        .strip_prefix("noescape ")
        .or_else(|| type_name.strip_prefix("owned "))
        .unwrap_or(type_name)
        .strip_prefix("Fn(")
        .and_then(|rest| rest.split_once(')').map(|(params, _)| params.trim()))
    else {
        return Vec::new();
    };
    if params.is_empty() {
        Vec::new()
    } else {
        // A `Fn(...)` parameter may carry a leading data effect (`read`/`mut`/
        // `take`); every caller here compares the parameter's TYPE, not its
        // effect (the effect is enforced separately by the analyzer's closure
        // mutability/borrow machinery), so strip the keyword to the bare type.
        split_top_level_type_args(params)
            .into_iter()
            .map(fn_param_bare_type)
            .collect()
    }
}

/// Strip a leading `read`/`mut`/`take` effect keyword from a `Fn` parameter
/// type string, leaving the bare type (`"mut Ctx"` -> `"Ctx"`).
pub(super) fn fn_param_bare_type(param: &str) -> &str {
    let param = param.trim();
    for keyword in ["read ", "mut ", "take "] {
        if let Some(rest) = param.strip_prefix(keyword) {
            return rest.trim();
        }
    }
    param
}
