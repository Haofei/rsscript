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
        if type_satisfies_protocol_bound(analyzer, function, actual, protocol) {
            continue;
        }
        let (cause, fix) = protocol_bound_guidance(protocol, actual);
        analyzer.diagnostics.push(
            Diagnostic::error(
                code::PROTOCOL_NOT_SATISFIED,
                format!(
                    "type `{actual}` does not satisfy protocol `{protocol}` required by `{call_name}`."
                ),
                call_span.clone(),
                "protocol not satisfied",
            )
            .with_cause(cause)
            .with_fix("satisfy_protocol_bound", fix, "manual"),
        );
    }
}

pub(super) fn type_satisfies_protocol_bound(
    analyzer: &Analyzer<'_>,
    function: &FunctionDecl,
    actual: &str,
    protocol: &str,
) -> bool {
    let actual_root = type_root_name(strip_fresh_type(actual));
    if dyn_protocol(actual).is_some_and(|dyn_protocol| dyn_protocol == protocol) {
        return true;
    }
    if protocol == "Ord" && builtin_type_is_ord(actual_root) {
        return true;
    }
    if (protocol == "Hashable" || protocol == "Eq") && builtin_type_is_hashable(actual_root) {
        return true;
    }
    if protocol == "Clone" && builtin_type_is_clone(actual_root) {
        return true;
    }
    // `List<T>`/`Option<T>`/`Result<A, B>` are `Hashable`/`Eq` exactly when their
    // element types are, so a key like `List<Coord>` is satisfiable structurally.
    if (protocol == "Hashable" || protocol == "Eq")
        && matches!(actual_root, "List" | "Option" | "Result")
        && let Some(args) = type_arg_names(strip_fresh_type(actual))
    {
        return args
            .iter()
            .all(|arg| type_satisfies_protocol_bound(analyzer, function, arg, protocol));
    }
    // `List<T>`/`Option<T>`/`Result<A, B>` are `Clone` exactly when their element
    // types are, mirroring the structural derive support for value containers.
    if protocol == "Clone"
        && matches!(actual_root, "List" | "Option" | "Result")
        && let Some(args) = type_arg_names(strip_fresh_type(actual))
    {
        return args
            .iter()
            .all(|arg| type_satisfies_protocol_bound(analyzer, function, arg, protocol));
    }
    if function.type_params.iter().any(|param| {
        param.name == actual_root
            && matches!(
                param.bound.as_ref(),
                Some(GenericBound::Protocol(bound)) if bound == protocol
            )
    }) {
        return true;
    }
    if analyzer
        .syntax_program
        .protocol_impls
        .iter()
        .any(|protocol_impl| {
            protocol_impl.protocol == protocol && protocol_impl.type_name == actual_root
        })
    {
        return true;
    }
    type_derives_protocol(&analyzer.syntax_program.items, actual_root, protocol)
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
        analyzer.diagnostics.push(dyn_from_diagnostic(
            protocol,
            "<unknown>",
            call_span.clone(),
            "ExternalBinding construction requires a typed value binding.",
        ));
        return;
    };
    let value_type = strip_fresh_type(value_type);
    if dyn_protocol(value_type).is_some() {
        analyzer.diagnostics.push(dyn_from_diagnostic(
            protocol,
            value_type,
            call_span.clone(),
            "Nested dynamic protocol values are not supported; wrap a concrete implementation value.",
        ));
        return;
    }
    if !type_satisfies_protocol_bound(analyzer, function, value_type, protocol) {
        analyzer.diagnostics.push(dyn_from_diagnostic(
            protocol,
            value_type,
            call_span.clone(),
            "The wrapped value must satisfy the external_binding protocol via an explicit impl.",
        ));
    }
}

/// Protocol-specific cause/fix text for an unsatisfied generic protocol bound.
/// `Hashable`/`Eq` are compiler-derived structural contracts (used by
/// `Map`/`Set` keys), so the suggestion points at the concrete `derives(...)`
/// list rather than the comparator wording used for `Ord`.
pub(super) fn protocol_bound_guidance(protocol: &str, actual: &str) -> (&'static str, String) {
    match protocol {
        "Hashable" => (
            "A `Map` key / `Set` element must be `Hashable` (and therefore `Eq`). Hashability is a compiler-derived structural contract: a builtin scalar key, or a managed struct/sum that derives `Eq` and `Hash`.",
            format!(
                "Add `derives(Eq, Hash)` to `{actual}` so the compiler derives a structural hash and equality, or use a hashable key type."
            ),
        ),
        "Eq" => (
            "Equality is a compiler-derived structural contract: a builtin scalar, or a managed struct/sum that derives `Eq` (or `Ord`, which implies `Eq`).",
            format!("Add `derives(Eq)` to `{actual}`, or use an equatable type."),
        ),
        _ => (
            "Generic protocol bounds are nominal. Use a type with a matching derive, add a compatible generic bound, or pass an explicit comparator API.",
            format!(
                "Add `derives({protocol})` to `{actual}` if the compiler-owned ordering is intended, or call an API that accepts an explicit comparator."
            ),
        ),
    }
}

pub(super) fn builtin_type_is_ord(type_name: &str) -> bool {
    matches!(type_name, "Int" | "String" | "Bool")
}

/// Builtin scalar types that are `Hashable`/`Eq` directly (no derive needed).
/// Mirrors the structural derive support in the analyzer (`Float` is excluded
/// because it is neither `Eq` nor `Hash`).
pub(super) fn builtin_type_is_hashable(type_name: &str) -> bool {
    matches!(
        type_name,
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Bool"
            | "Byte"
            | "Char"
            | "Unit"
            | "String"
    )
}

/// Builtin scalar types that are `Clone` directly (no derive needed). Every
/// value scalar is copyable, including `Float` (which is not `Eq`/`Hash`).
pub(super) fn builtin_type_is_clone(type_name: &str) -> bool {
    matches!(
        type_name,
        "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Bool"
            | "Byte"
            | "Char"
            | "Unit"
            | "Float"
            | "Float32"
            | "Float64"
            | "String"
    )
}

/// Whether a user-declared `type_name` satisfies a compiler-derived `protocol`
/// bound. `Ord` requires `derives(Ord)`; `Hashable` requires `derives(Hash)`;
/// `Eq` requires `derives(Eq)` or `derives(Ord)` (which implies `Eq`).
pub(super) fn type_derives_protocol(items: &[Item], type_name: &str, protocol: &str) -> bool {
    let derive_satisfies = |derives: &[String]| -> bool {
        let has = |name: &str| derives.iter().any(|derive| derive == name);
        match protocol {
            "Ord" => has("Ord"),
            "Hashable" => has("Hash"),
            "Eq" => has("Eq") || has("Ord"),
            "Clone" => has("Clone"),
            _ => false,
        }
    };
    if !matches!(protocol, "Ord" | "Hashable" | "Eq" | "Clone") {
        return false;
    }
    items.iter().any(|item| match item {
        Item::Type(decl) => decl.name == type_name && derive_satisfies(&decl.derives),
        Item::SumType(sum) => sum.name == type_name && derive_satisfies(&sum.derives),
        _ => false,
    })
}

pub(super) fn dyn_protocol(type_name: &str) -> Option<&str> {
    let root = type_root_name(strip_fresh_type(type_name));
    if root != "Dyn" {
        return None;
    }
    type_arg_names(type_name).and_then(|args| args.first().copied())
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

pub(super) fn dyn_from_diagnostic(
    protocol: &str,
    value_type: &str,
    span: Span,
    cause: &'static str,
) -> Diagnostic {
    Diagnostic::error(
        code::PROTOCOL_NOT_SATISFIED,
        format!("cannot construct `Dyn<{protocol}>` from `{value_type}`."),
        span,
        "external_binding protocol not satisfied",
    )
    .with_cause(cause)
    .with_cause(
        "Dyn values are explicit dynamic protocol boundaries; construction requires a concrete value with a visible protocol implementation.",
    )
    .with_fix(
        "add_protocol_impl",
        format!("Declare `impl {protocol} for {value_type} {{ ... }}` or wrap a value that already satisfies `{protocol}`."),
        "manual",
    )
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
            analyzer.diagnostics.push(Diagnostic::error(
                code::UNNAMED_ARGUMENT,
                format!(
                    "variant `{variant}` must be constructed with named fields, e.g. `{variant}(field: value)`."
                ),
                unnamed.span.clone(),
                "variant field must be named",
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
                        analyzer.diagnostics.push(Diagnostic::error(
                            code::UNKNOWN_ARGUMENT,
                            format!("variant `{variant}` has no field `{name}`."),
                            arg.span.clone(),
                            "unknown variant field",
                        ));
                        continue;
                    }
                },
                None if index < fields.len() => index,
                None => {
                    analyzer.diagnostics.push(Diagnostic::error(
                        code::UNKNOWN_ARGUMENT,
                        format!(
                            "variant `{variant}` has {} field(s) but {} were given.",
                            fields.len(),
                            args.len()
                        ),
                        arg.span.clone(),
                        "too many variant fields",
                    ));
                    continue;
                }
            };
            if seen[field_idx] {
                analyzer.diagnostics.push(Diagnostic::error(
                    code::DUPLICATE_ARGUMENT,
                    format!(
                        "variant `{variant}` field `{}` is provided more than once.",
                        fields[field_idx].0
                    ),
                    arg.span.clone(),
                    "duplicate variant field",
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
                analyzer.diagnostics.push(Diagnostic::error(
                    code::ARGUMENT_TYPE_MISMATCH,
                    format!(
                        "variant `{variant}` field `{}` has type `{actual}`, expected `{expected}`.",
                        fields[field_idx].0
                    ),
                    hir_expr_span(&arg.value).clone(),
                    "variant field type mismatch",
                ));
            }
        }
        for (i, provided) in seen.iter().enumerate() {
            if !provided {
                analyzer.diagnostics.push(Diagnostic::error(
                    code::MISSING_ARGUMENT,
                    format!("variant `{variant}` is missing field `{}`.", fields[i].0),
                    call_span.clone(),
                    "missing variant field",
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
    if type_satisfies_protocol_bound(analyzer, function, receiver_type, namespace) {
        return;
    }
    if function.type_params.iter().any(|param| {
        param.name == receiver_root
            && matches!(
                param.bound.as_ref(),
                Some(GenericBound::Protocol(protocol)) if protocol == namespace
            )
    }) {
        return;
    }
    if analyzer
        .syntax_program
        .protocol_impls
        .iter()
        .any(|protocol_impl| {
            protocol_impl.protocol == *namespace && protocol_impl.type_name == receiver_root
        })
    {
        return;
    }
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::PROTOCOL_NOT_SATISFIED,
            format!(
                "receiver type `{receiver_type}` does not satisfy protocol `{namespace}` for `{namespace}.{name}`."
            ),
            call_span.clone(),
            "protocol not satisfied",
        )
        .with_cause("Protocols are nominal external_binding contracts. A protocol call must be backed by an explicit generic bound or an explicit protocol implementation.")
        .with_fix(
            "add_protocol_bound_or_impl",
            format!("Add a `{receiver_root}: {namespace}` generic bound or declare `impl {namespace} for {receiver_root} {{ ... }}`."),
            "manual",
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

pub(super) fn substitute_type_params(
    budget: &crate::checks::budget::AnalysisBudget,
    type_name: &str,
    substitutions: &HashMap<String, String>,
) -> Result<String, ()> {
    substitute_type_params_bounded(budget, type_name, substitutions, 0)
}

pub(super) fn substitute_type_params_bounded(
    budget: &crate::checks::budget::AnalysisBudget,
    type_name: &str,
    substitutions: &HashMap<String, String>,
    depth: usize,
) -> Result<String, ()> {
    if !budget.check_recursion(depth) || !budget.consume_substitution() {
        return Err(());
    }
    if let Some(replacement) = substitutions.get(type_name) {
        return Ok(replacement.clone());
    }
    if let Some(target) = fresh_type_target(type_name) {
        return Ok(format!(
            "fresh {}",
            substitute_type_params_bounded(budget, target, substitutions, depth + 1)?
        ));
    }
    if let Some(return_ty) = fn_return_type(type_name) {
        let prefix = fn_type_prefix(type_name);
        let params = fn_param_types(type_name)
            .into_iter()
            .map(|param| substitute_type_params_bounded(budget, param, substitutions, depth + 1))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        return Ok(format!(
            "{prefix}Fn({params}) -> {}",
            substitute_type_params_bounded(budget, return_ty, substitutions, depth + 1)?
        ));
    }
    let Some(args) = type_arg_names(type_name) else {
        return Ok(type_name.to_string());
    };
    let root = type_root_name(type_name);
    let args = args
        .into_iter()
        .map(|arg| substitute_type_params_bounded(budget, arg, substitutions, depth + 1))
        .collect::<Result<Vec<_>, _>>()?
        .join(", ");
    Ok(format!("{root}<{args}>"))
}
