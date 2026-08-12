//! Generic substitution, protocol bounds, and receiver protocol satisfaction.

use super::*;

pub(super) fn check_generic_call_bounds(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    _callee: &Callee,
    call_name: &str,
    signature: &FunctionSig,
    substitutions: &HashMap<String, String>,
    call_span: &Span,
) {
    let protocol_facts = protocol_satisfaction_facts(analyzer, function);
    for (param, bound) in signature
        .type_params
        .iter()
        .zip(signature.type_param_bounds.iter())
    {
        let Some(GenericBound::Protocol(protocol)) = bound else {
            continue;
        };
        let Some(actual) = substitutions.get(param) else {
            continue;
        };
        // A substitution that still names the callee's own type parameter is
        // unresolved, not evidence that the bound failed. A caller type
        // parameter with the same spelling remains checkable through its own
        // declared bound below.
        if actual == param
            && !function
                .type_params
                .iter()
                .any(|function_param| function_param.name == *actual)
        {
            continue;
        }
        if rsscript_semantics::type_satisfies_protocol_bound(actual, protocol, &protocol_facts) {
            continue;
        }
        let (cause, fix) = rsscript_semantics::protocol_bound_guidance(protocol, actual);
        analyzer
            .diagnostics
            .push(rsscript_semantics::protocol_bound_not_satisfied_diagnostic(
                actual,
                protocol,
                call_name,
                call_span.clone(),
                cause,
                fix,
            ));
    }
}

fn protocol_satisfaction_facts(
    analyzer: &Analyzer<'_>,
    function: &FunctionDecl,
) -> rsscript_semantics::ProtocolSatisfactionFacts {
    rsscript_semantics::protocol_satisfaction_facts(
        &function.type_params,
        analyzer
            .syntax_program
            .protocol_impls
            .iter()
            .map(|protocol_impl| {
                (
                    protocol_impl.protocol.clone(),
                    protocol_impl.type_name.clone(),
                )
            }),
        [&analyzer.syntax_program],
    )
}

pub(super) fn check_dyn_from_call(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    callee: &Callee,
    args: &[HirCallArg],
    call_span: &Span,
) {
    let Some(protocol) = dyn_from_protocol(callee) else {
        return;
    };
    if !analyzer.protocol_name_is_visible(protocol) {
        return;
    }
    let Some(value_arg) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("value"))
        .or_else(|| args.first())
    else {
        return;
    };
    let Some(value_type) = hir_expr_type_name(&value_arg.value) else {
        analyzer
            .diagnostics
            .push(rsscript_semantics::dyn_from_diagnostic(
                protocol,
                "<unknown>",
                call_span.clone(),
                "ExternalBinding construction requires a typed value binding.",
            ));
        return;
    };
    let value_type = strip_fresh_type(value_type);
    if rsscript_semantics::dynamic_protocol_name(value_type).is_some() {
        analyzer.diagnostics.push(rsscript_semantics::dyn_from_diagnostic(
            protocol,
            value_type,
            call_span.clone(),
            "Nested dynamic protocol values are not supported; wrap a concrete implementation value.",
        ));
        return;
    }
    if !rsscript_semantics::type_satisfies_protocol_bound(
        value_type,
        protocol,
        &protocol_satisfaction_facts(analyzer, function),
    ) {
        analyzer.diagnostics.push(rsscript_semantics::dyn_from_diagnostic(
            protocol,
            value_type,
            call_span.clone(),
            "The wrapped value must satisfy the external_binding protocol via an explicit impl.",
        ));
    }
}

pub(super) fn dyn_from_protocol(callee: &Callee) -> Option<&str> {
    let Callee::Qualified { namespace, name } = callee else {
        return None;
    };
    if type_root_name(namespace) != "Dyn" || type_root_name(name) != "from" {
        return None;
    }
    type_arg_names(namespace).and_then(|args| args.first().copied())
}

pub(super) fn check_enum_variant_form(
    analyzer: &mut Analyzer<'_>,
    callee: &Callee,
    args: &[HirCallArg],
    call_span: &Span,
) {
    let variant = callee_name(callee);
    // User sum variants: validate construction against the variant's declared fields. Each field
    // must be supplied exactly once (no unknown name, no duplicate, none missing) and each value is
    // type-checked against its declared field type — so a malformed/ill-typed constructor is a
    // checker error rather than a lowerer panic or a mapped rustc error.
    if let Some(fields) = analyzer.hir.sum_variant_fields(&variant).map(|fields| {
        fields
            .iter()
            .map(|f| (f.name.clone(), f.ty.to_string()))
            .collect::<Vec<(String, String)>>()
    }) {
        // Variants use the same named-field construction form as structs (per the v0.7 spec);
        // positional/unnamed payload args are not allowed.
        if let Some(unnamed) = args.iter().find(|arg| arg.name.is_none()) {
            analyzer
                .diagnostics
                .push(rsscript_semantics::unnamed_variant_field_diagnostic(
                    &variant,
                    unnamed.span.clone(),
                ));
            return;
        }
        let mut seen = vec![false; fields.len()];
        for (index, arg) in args.iter().enumerate() {
            // Resolve which field this arg targets: by name if given, else positionally.
            let field_idx = match &arg.name {
                Some(name) => match fields.iter().position(|(fname, _)| fname == name) {
                    Some(i) => i,
                    None => {
                        analyzer.diagnostics.push(
                            rsscript_semantics::unknown_variant_field_diagnostic(
                                &variant,
                                name,
                                arg.span.clone(),
                            ),
                        );
                        continue;
                    }
                },
                None if index < fields.len() => index,
                None => {
                    analyzer.diagnostics.push(
                        rsscript_semantics::too_many_variant_fields_diagnostic(
                            &variant,
                            fields.len(),
                            args.len(),
                            arg.span.clone(),
                        ),
                    );
                    continue;
                }
            };
            if seen[field_idx] {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::duplicate_variant_field_diagnostic(
                        &variant,
                        &fields[field_idx].0,
                        arg.span.clone(),
                    ));
                continue;
            }
            seen[field_idx] = true;
            // Type-check the value against the declared field type (mirrors binding-payload checks:
            // accept matching JSON/Map/List literals, skip unresolved generics, else require a match).
            let expected = fields[field_idx].1.as_str();
            if json_value_accepts_literal(expected, &arg.value)
                || check_map_literal_type(analyzer, expected, &arg.value, "variant field")
                || check_list_literal_type(analyzer, expected, &arg.value, "variant field")
            {
                continue;
            }
            if let Some(actual) = hir_expr_type_name(&arg.value)
                && !unresolved_generic_type(analyzer, actual)
                && !argument_type_matches(expected, actual)
            {
                analyzer.diagnostics.push(
                    rsscript_semantics::variant_field_type_mismatch_diagnostic(
                        &variant,
                        &fields[field_idx].0,
                        actual,
                        expected,
                        hir_expr_span(&arg.value).clone(),
                    ),
                );
            }
        }
        for (i, provided) in seen.iter().enumerate() {
            if !provided {
                analyzer
                    .diagnostics
                    .push(rsscript_semantics::missing_variant_field_diagnostic(
                        &variant,
                        &fields[i].0,
                        call_span.clone(),
                    ));
            }
        }
        return;
    }
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
    analyzer
        .diagnostics
        .push(rsscript_semantics::conventional_variant_form_diagnostic(
            &variant, form, span,
        ));
}

pub(super) fn check_protocol_receiver_satisfaction(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    callee: &Callee,
    args: &[HirCallArg],
    call_span: &Span,
) {
    let Callee::Qualified { namespace, name } = callee else {
        return;
    };
    if !analyzer.protocol_name_is_visible(namespace) {
        return;
    }
    let Some(self_arg) = args.iter().find(|arg| arg.name.as_deref() == Some("self")) else {
        return;
    };
    let Some(receiver_type) = hir_expr_type_name(&self_arg.value) else {
        return;
    };
    let receiver_type = strip_fresh_type(receiver_type);
    let receiver_root = type_root_name(receiver_type);
    if rsscript_semantics::type_satisfies_protocol_bound(
        receiver_type,
        namespace,
        &protocol_satisfaction_facts(analyzer, function),
    ) {
        return;
    }
    analyzer.diagnostics.push(
        rsscript_semantics::protocol_receiver_not_satisfied_diagnostic(
            receiver_type,
            receiver_root,
            namespace,
            name,
            call_span.clone(),
        ),
    );
}

pub(super) fn call_type_param_substitutions(
    analyzer: &Analyzer<'_>,
    function: Option<&FunctionDecl>,
    callee: &Callee,
    args: &[HirCallArg],
    signature: &FunctionSig,
) -> Option<HashMap<String, String>> {
    let mut substitutions = HashMap::new();
    let generic_params = signature
        .type_params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if generic_params.is_empty() {
        return Some(substitutions);
    }
    if let Some(explicit_args) = explicit_callee_type_args(callee) {
        for (param, actual) in signature.type_params.iter().zip(explicit_args) {
            if generic_params.contains(param.as_str()) {
                substitutions.insert(param.clone(), actual.to_string());
            }
        }
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
    if let Callee::ReceiverCall { receiver, .. } = callee
        && let Some(receiver_param) = signature.params.first()
        && let Some(actual_type) = infer_receiver_expr_type(analyzer, function, receiver)
    {
        let pattern_type = analyzer.expand_type_alias(&receiver_param.ty.to_string());
        let actual_type = analyzer.expand_type_alias(&actual_type);
        if !collect_type_param_substitutions(
            &analyzer.budget,
            &pattern_type,
            &actual_type,
            &generic_params,
            &mut substitutions,
        ) {
            return None;
        }
    }
    if !collect_call_arg_type_param_substitutions(
        analyzer,
        args,
        signature,
        &generic_params,
        &mut substitutions,
    ) {
        return None;
    }
    Some(substitutions)
}

pub(super) fn explicit_callee_type_args(callee: &Callee) -> Option<Vec<&str>> {
    match callee {
        Callee::Name(name) | Callee::Qualified { name, .. } => type_arg_names(name),
        Callee::ReceiverCall { method, .. } => type_arg_names(method),
    }
}

pub(super) fn collect_call_arg_type_param_substitutions(
    analyzer: &Analyzer<'_>,
    args: &[HirCallArg],
    signature: &FunctionSig,
    generic_params: &HashSet<&str>,
    substitutions: &mut HashMap<String, String>,
) -> bool {
    for (index, arg) in args.iter().enumerate() {
        let Some(param) = arg
            .name
            .as_deref()
            .and_then(|name| signature.params.iter().find(|param| param.name == name))
            .or_else(|| {
                constructor_or_named_shorthand_arg_name(arg)
                    .and_then(|name| signature.params.iter().find(|param| param.name == name))
            })
            .or_else(|| signature.params.get(index))
        else {
            continue;
        };
        let Some(actual_type) = hir_expr_type_name(&arg.value) else {
            continue;
        };
        let pattern_type = analyzer.expand_type_alias(&param.ty.to_string());
        let actual_type = analyzer.expand_type_alias(actual_type);
        if !collect_type_param_substitutions(
            &analyzer.budget,
            &pattern_type,
            &actual_type,
            generic_params,
            substitutions,
        ) {
            return false;
        }
    }
    true
}

pub(super) fn constructor_or_named_shorthand_arg_name(arg: &HirCallArg) -> Option<&str> {
    if arg.name.is_some() {
        return None;
    }
    let HirExpr::Ident { name, .. } = &arg.value else {
        return None;
    };
    Some(name.as_str())
}

pub(super) fn infer_receiver_expr_type(
    analyzer: &Analyzer<'_>,
    function: Option<&FunctionDecl>,
    expr: &Expr,
) -> Option<String> {
    match expr {
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => infer_receiver_expr_type(analyzer, function, value),
        Expr::Ident(name, _) => function
            .and_then(|function| analyzer.hir.function_body(&function.name))
            .and_then(|body| {
                body.bindings
                    .iter()
                    .find(|binding| binding.name == *name)
                    .and_then(|binding| binding.ty.as_ref().map(ToString::to_string))
            })
            .or_else(|| builtin_value_type_name(name).map(str::to_string)),
        Expr::Call { .. } => None,
        Expr::Closure { .. }
        | Expr::Match { .. }
        | Expr::ObjectLiteral { .. }
        | Expr::MapLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Field { .. }
        | Expr::Index { .. }
        | Expr::Binary { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => None,
    }
}

pub(super) fn collect_type_param_substitutions(
    budget: &crate::checks::budget::AnalysisBudget,
    pattern: &str,
    actual: &str,
    generic_params: &HashSet<&str>,
    substitutions: &mut HashMap<String, String>,
) -> bool {
    collect_type_param_substitutions_bounded(
        budget,
        pattern,
        actual,
        generic_params,
        substitutions,
        0,
    )
}

pub(super) fn collect_type_param_substitutions_bounded(
    budget: &crate::checks::budget::AnalysisBudget,
    pattern: &str,
    actual: &str,
    generic_params: &HashSet<&str>,
    substitutions: &mut HashMap<String, String>,
    depth: usize,
) -> bool {
    if !budget.check_recursion(depth) || !budget.consume_substitution() {
        return false;
    }
    let pattern = pattern.trim();
    let actual = actual.trim();
    if actual == "?" {
        return true;
    }
    if let Some(pattern) = fresh_type_target(pattern) {
        let actual = fresh_type_target(actual).unwrap_or(actual);
        return collect_type_param_substitutions_bounded(
            budget,
            pattern,
            actual,
            generic_params,
            substitutions,
            depth + 1,
        );
    }
    if generic_params.contains(pattern) {
        substitutions
            .entry(pattern.to_string())
            .or_insert_with(|| actual.to_string());
        return true;
    }
    if is_fn_type(pattern) && is_fn_type(actual) {
        for (pattern_param, actual_param) in fn_param_types(pattern)
            .into_iter()
            .zip(fn_param_types(actual))
        {
            if !collect_type_param_substitutions_bounded(
                budget,
                pattern_param,
                actual_param,
                generic_params,
                substitutions,
                depth + 1,
            ) {
                return false;
            }
        }
        if let (Some(pattern_return), Some(actual_return)) =
            (fn_return_type(pattern), fn_return_type(actual))
            && !collect_type_param_substitutions_bounded(
                budget,
                pattern_return,
                actual_return,
                generic_params,
                substitutions,
                depth + 1,
            )
        {
            return false;
        }
        return true;
    }
    let Some(pattern_args) = type_arg_names(pattern) else {
        return true;
    };
    let Some(actual_args) = type_arg_names(actual) else {
        return true;
    };
    if type_root_name(pattern) != type_root_name(actual) || pattern_args.len() != actual_args.len()
    {
        return true;
    }
    for (pattern_arg, actual_arg) in pattern_args.into_iter().zip(actual_args) {
        if !collect_type_param_substitutions_bounded(
            budget,
            pattern_arg,
            actual_arg,
            generic_params,
            substitutions,
            depth + 1,
        ) {
            return false;
        }
    }
    true
}

struct CompilerSubstitutionBudget<'a>(&'a crate::checks::budget::AnalysisBudget);

impl rsscript_semantics::SubstitutionBudget for CompilerSubstitutionBudget<'_> {
    fn check_recursion(&self, depth: usize) -> bool {
        self.0.check_recursion(depth)
    }

    fn consume_substitution(&self) -> bool {
        self.0.consume_substitution()
    }
}

/// Bridge the compiler's shared frontend budget into the semantics-owned
/// generic substitution rule.
pub(super) fn substitute_type_params(
    budget: &crate::checks::budget::AnalysisBudget,
    type_name: &str,
    substitutions: &HashMap<String, String>,
) -> Result<String, ()> {
    rsscript_semantics::substitute_type_params(
        &CompilerSubstitutionBudget(budget),
        type_name,
        substitutions,
    )
    .map_err(|_| ())
}
