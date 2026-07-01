use super::*;

/// Infer the type of a built-in `Option`/`Result` variant constructor call so an
/// untyped local (`let o = Some(5)`) carries a type and downstream argument checks
/// are not silently skipped. The variant's known payload position is filled from the
/// argument; the other generic position (e.g. the error type of `Ok`) is left as a
/// single-uppercase placeholder so `unresolved_generic_type` skips it rather than
/// reporting a spurious mismatch.
fn infer_enum_variant_type(
    hir: &Hir,
    variant: &str,
    args: &[crate::syntax::ast::CallArg],
    value_types: &HashMap<String, String>,
) -> Option<String> {
    let payload_type = |args: &[crate::syntax::ast::CallArg]| {
        args.first()
            .and_then(|arg| infer_hir_expr_type(hir, &arg.value, value_types))
    };
    match variant {
        "Some" => Some(format!("Option<{}>", payload_type(args)?)),
        "Ok" => Some(format!("Result<{}, E>", payload_type(args)?)),
        "Err" => Some(format!("Result<T, {}>", payload_type(args)?)),
        // A user-declared sum variant constructs a value of its sum type, so a `Number(value: 5)`
        // call has type `Token` — letting the normal arg/binding type checks catch misuse.
        _ => hir.sum_type_for_variant(variant).map(str::to_string),
    }
}

pub(crate) fn infer_hir_expr_type(
    hir: &Hir,
    expr: &Expr,
    value_types: &HashMap<String, String>,
) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => value_types
            .get(name)
            .cloned()
            .or_else(|| hir.sum_type_for_variant(name).map(str::to_string)),
        Expr::Binary { .. } => None,
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            infer_hir_expr_type(hir, value, value_types)
        }
        Expr::Spawn { value, .. } => {
            infer_hir_expr_type(hir, value, value_types).map(|ty| format!("Task<{ty}>"))
        }
        Expr::Await { value, .. } => infer_hir_expr_type(hir, value, value_types)
            .and_then(|ty| task_inner_type(&ty))
            .or_else(|| infer_hir_expr_type(hir, value, value_types)),
        Expr::Try { value, .. } => {
            infer_hir_expr_type(hir, value, value_types).and_then(|ty| result_ok_type(&ty))
        }
        Expr::Match { arms, .. } => arms
            .first()
            .and_then(|arm| infer_closure_return_type(hir, &arm.body, value_types)),
        Expr::Call { callee, args, .. } => {
            let resolution = match callee {
                Callee::ReceiverCall {
                    receiver, method, ..
                } => {
                    if let Some(receiver_type) = infer_hir_expr_type(hir, receiver, value_types) {
                        hir.resolve_receiver_call(&receiver_type, method, value_types)
                            .0
                    } else {
                        CallResolution::Unknown
                    }
                }
                _ => hir.resolve_call(callee),
            };
            match resolution {
                CallResolution::Resolved { signature, .. } => {
                    infer_signature_return_type(hir, &signature, callee, args, value_types)
                        .or(signature.return_type)
                }
                CallResolution::Ambiguous { .. } | CallResolution::Unknown => match callee {
                    Callee::Name(name) => value_types
                        .get(name)
                        .and_then(|type_name| fn_return_type(type_name))
                        .map(str::to_string),
                    Callee::Qualified { .. } | Callee::ReceiverCall { .. } => None,
                },
                CallResolution::EnumVariant => {
                    infer_enum_variant_type(hir, callee_name(callee), args, value_types)
                }
            }
        }
        Expr::Field { base, name, .. } => {
            let base_type = infer_hir_expr_type(hir, base, value_types)?;
            let type_info = hir.type_info(&base_type)?;
            let field = type_info.fields.get(name)?;
            Some(substituted_field_type(type_info, &base_type, field))
        }
        Expr::Index { .. } => None,
        Expr::Number(value, _) => Some(number_literal_type_name(value).to_string()),
        Expr::String(_, _) | Expr::MultilineString(_, _) => Some("String".to_string()),
        Expr::CharLiteral(_, _) => Some("Char".to_string()),
        Expr::ObjectLiteral { .. } => Some("JsonLiteral".to_string()),
        Expr::MapLiteral { .. } => Some("MapLiteral".to_string()),
        Expr::ArrayLiteral { items, .. } => {
            let item_type = items
                .first()
                .and_then(|item| infer_hir_expr_type(hir, item, value_types))
                .unwrap_or_else(|| "?".to_string());
            Some(format!("List<{item_type}>"))
        }
        Expr::Closure { .. } | Expr::Unknown(_) => None,
    }
}

fn infer_signature_return_type(
    hir: &Hir,
    signature: &FunctionSig,
    callee: &Callee,
    args: &[CallArg],
    value_types: &HashMap<String, String>,
) -> Option<String> {
    let return_type = signature.return_type.as_ref()?;
    if signature.type_params.is_empty() {
        return None;
    }

    let generic_params = signature
        .type_params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut substitutions = HashMap::new();
    collect_callee_type_substitutions(signature, callee, &generic_params, &mut substitutions);
    collect_namespace_type_substitutions(hir, callee, &generic_params, &mut substitutions);
    collect_receiver_type_substitutions(
        hir,
        signature,
        callee,
        value_types,
        &generic_params,
        &mut substitutions,
    );
    collect_arg_type_substitutions(
        hir,
        signature,
        args,
        value_types,
        &generic_params,
        &mut substitutions,
    );

    if substitutions.is_empty() {
        None
    } else {
        Some(substitute_type_params(return_type, &substitutions))
    }
}

fn collect_callee_type_substitutions(
    signature: &FunctionSig,
    callee: &Callee,
    generic_params: &HashSet<&str>,
    substitutions: &mut HashMap<String, String>,
) {
    let type_args = match callee {
        Callee::Name(name) | Callee::Qualified { name, .. } => type_arg_names(name),
        Callee::ReceiverCall { method, .. } => type_arg_names(method),
    };
    let Some(type_args) = type_args else {
        return;
    };
    for (param, actual) in signature.type_params.iter().zip(type_args) {
        if generic_params.contains(param.as_str()) {
            substitutions
                .entry(param.to_string())
                .or_insert_with(|| actual.to_string());
        }
    }
}

fn collect_namespace_type_substitutions(
    hir: &Hir,
    callee: &Callee,
    generic_params: &HashSet<&str>,
    substitutions: &mut HashMap<String, String>,
) {
    let Callee::Qualified { namespace, .. } = callee else {
        return;
    };
    let root = type_root_name(namespace);
    let Some(namespace_args) = type_arg_names(namespace) else {
        return;
    };
    let params = hir
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
            substitutions
                .entry(param.to_string())
                .or_insert_with(|| actual.to_string());
        }
    }
}

fn collect_receiver_type_substitutions(
    hir: &Hir,
    signature: &FunctionSig,
    callee: &Callee,
    value_types: &HashMap<String, String>,
    generic_params: &HashSet<&str>,
    substitutions: &mut HashMap<String, String>,
) {
    let Callee::ReceiverCall { receiver, .. } = callee else {
        return;
    };
    let Some(receiver_param) = signature.params.first() else {
        return;
    };
    let Some(actual_type) = infer_hir_expr_type(hir, receiver, value_types) else {
        return;
    };
    collect_type_substitutions(
        &receiver_param.type_name,
        &actual_type,
        generic_params,
        substitutions,
    );
}

fn collect_arg_type_substitutions(
    hir: &Hir,
    signature: &FunctionSig,
    args: &[CallArg],
    value_types: &HashMap<String, String>,
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
        let (pattern_type, actual_type) = if let Some(expected_return_type) =
            noescape_return_type(&param.type_name)
            && let Expr::Closure { body, .. } = &arg.value
            && let Some(actual_return_type) = infer_closure_return_type(hir, body, value_types)
        {
            (expected_return_type.to_string(), actual_return_type)
        } else {
            let Some(actual_type) = infer_arg_expr_type(hir, &arg.value, value_types) else {
                continue;
            };
            (param.type_name.clone(), actual_type)
        };
        collect_type_substitutions(&pattern_type, &actual_type, generic_params, substitutions);
    }
}

pub(super) fn infer_closure_return_type(
    hir: &Hir,
    body: &Block,
    value_types: &HashMap<String, String>,
) -> Option<String> {
    if let Some(statement) = body.statements.iter().next_back() {
        match statement {
            Stmt::Return(stmt) => {
                return stmt
                    .value
                    .as_ref()
                    .and_then(|value| infer_hir_expr_type(hir, value, value_types))
                    .or_else(|| Some("Unit".to_string()));
            }
            Stmt::Expr(value) => return infer_hir_expr_type(hir, value, value_types),
            Stmt::Let(_) | Stmt::LetElse(_) | Stmt::Assign(_) => {
                return Some("Unit".to_string());
            }
            Stmt::With { .. }
            | Stmt::If { .. }
            | Stmt::Loop { .. }
            | Stmt::For(_)
            | Stmt::TaskGroup(_)
            | Stmt::Select(_)
            | Stmt::Match { .. }
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => return None,
        }
    }
    Some("Unit".to_string())
}

fn infer_arg_expr_type(
    hir: &Hir,
    expr: &Expr,
    value_types: &HashMap<String, String>,
) -> Option<String> {
    match expr {
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => infer_arg_expr_type(hir, value, value_types),
        Expr::Ident(name, _) => value_types.get(name).cloned(),
        Expr::Call { .. } => infer_hir_expr_type(hir, expr, value_types),
        Expr::Closure { params, body, .. } => infer_closure_return_type(hir, body, value_types)
            .map(|return_type| {
                let params = (0..params.len())
                    .map(|_| "?")
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("noescape Fn({params}) -> {return_type}")
            }),
        Expr::Match { .. } => infer_hir_expr_type(hir, expr, value_types),
        Expr::ObjectLiteral { .. } | Expr::MapLiteral { .. } | Expr::ArrayLiteral { .. } => {
            infer_hir_expr_type(hir, expr, value_types)
        }
        Expr::Field { .. } => infer_hir_expr_type(hir, expr, value_types),
        // Scalar literals carry a type so generic construction can unify it with a
        // type parameter (`Pair(item0: 1)` -> `A = Int`).
        Expr::Number(value, _) => Some(number_literal_type_name(value).to_string()),
        Expr::String(_, _) | Expr::MultilineString(_, _) => Some("String".to_string()),
        Expr::CharLiteral(_, _) => Some("Char".to_string()),
        Expr::Index { .. } | Expr::Binary { .. } | Expr::Unknown(_) => None,
    }
}

fn collect_type_substitutions(
    pattern: &str,
    actual: &str,
    generic_params: &HashSet<&str>,
    substitutions: &mut HashMap<String, String>,
) {
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
            collect_type_substitutions(pattern_param, actual_param, generic_params, substitutions);
        }
        if let (Some(pattern_return), Some(actual_return)) =
            (noescape_return_type(pattern), noescape_return_type(actual))
        {
            collect_type_substitutions(
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
        collect_type_substitutions(pattern_arg, actual_arg, generic_params, substitutions);
    }
}

/// The type of `field` accessed on a value of type `base_type`, with the type's
/// generic parameters replaced by `base_type`'s concrete arguments — so `item0`
/// on `__Tuple2<Int, String>` resolves to `Int`, not the declared parameter `A`.
pub(super) fn substituted_field_type(
    type_info: &TypeInfo,
    base_type: &str,
    field: &FieldInfo,
) -> String {
    let args = type_arg_names(base_type).unwrap_or_default();
    if args.is_empty() || type_info.type_params.is_empty() {
        return field.type_name.clone();
    }
    let substitutions: HashMap<String, String> = type_info
        .type_params
        .iter()
        .cloned()
        .zip(args.into_iter().map(str::to_string))
        .collect();
    substitute_type_params(&field.type_name, &substitutions)
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

use crate::text_util::builtin_generic_type_params;

pub(super) fn capability_protocol(type_name: &str) -> Option<&str> {
    let root = type_root_name(type_name);
    if root != "Capability" {
        return None;
    }
    type_arg_names(type_name).and_then(|args| args.first().copied())
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

fn noescape_return_type(type_name: &str) -> Option<&str> {
    type_name
        .trim()
        .strip_prefix("noescape ")
        .and_then(fn_return_type)
}

fn is_noescape_fn_type(type_name: &str) -> bool {
    type_name
        .strip_prefix("noescape Fn(")
        .and_then(|rest| rest.split_once(')'))
        .is_some()
}

fn noescape_param_types(type_name: &str) -> Vec<&str> {
    let Some(params) = type_name
        .strip_prefix("noescape Fn(")
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

fn result_ok_type(type_name: &str) -> Option<String> {
    let inner = type_name
        .strip_prefix("Result<")
        .and_then(|rest| rest.strip_suffix('>'))?;
    split_top_level_type_args(inner)
        .into_iter()
        .next()
        .map(strip_fresh_type)
        .map(str::to_string)
}

pub(super) fn list_element_type(type_name: &str) -> Option<String> {
    let inner = strip_fresh_type(type_name)
        .strip_prefix("List<")
        .and_then(|rest| rest.strip_suffix('>'))?;
    split_top_level_type_args(inner)
        .into_iter()
        .next()
        .map(str::to_string)
}

pub(super) fn stream_item_type(type_name: &str) -> Option<String> {
    let inner = strip_fresh_type(type_name)
        .strip_prefix("Stream<")
        .and_then(|rest| rest.strip_suffix('>'))?;
    split_top_level_type_args(inner)
        .into_iter()
        .next()
        .map(str::to_string)
}

fn task_inner_type(type_name: &str) -> Option<String> {
    type_name
        .strip_prefix("Task<")
        .and_then(|rest| rest.strip_suffix('>'))
        .map(str::to_string)
}

pub(super) fn match_pattern_binding_type(
    pattern: &MatchPattern,
    value_type: Option<&str>,
) -> Option<(String, String)> {
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type.map(|ty| (name.clone(), ty.to_string()));
    }
    let MatchPattern::Variant {
        name,
        binding: Some(binding),
        ..
    } = pattern
    else {
        return None;
    };
    let value_type = value_type?;
    let inner = value_type
        .strip_prefix("Option<")
        .and_then(|rest| rest.strip_suffix('>'));
    if name == "Some" {
        return inner.and_then(|ty| match_pattern_binding_type(binding, Some(ty.trim())));
    }
    let inner = value_type
        .strip_prefix("Result<")
        .and_then(|rest| rest.strip_suffix('>'));
    let args = inner.map(split_top_level_type_args)?;
    match name.as_str() {
        "Ok" => args
            .first()
            .and_then(|ty| match_pattern_binding_type(binding, Some(ty.trim()))),
        "Err" => args
            .get(1)
            .and_then(|ty| match_pattern_binding_type(binding, Some(ty.trim()))),
        _ => None,
    }
}

pub(super) fn match_pattern_binding_types(
    hir: &Hir,
    pattern: &MatchPattern,
    value_type: Option<&str>,
) -> Vec<(String, String)> {
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type
            .map(|ty| vec![(name.clone(), ty.to_string())])
            .unwrap_or_default();
    }
    if let Some(binding) = match_pattern_binding_type(pattern, value_type) {
        return vec![binding];
    }

    if let MatchPattern::Variant {
        name,
        binding: Some(binding),
        ..
    } = pattern
    {
        let Some(value_type) = value_type else {
            return Vec::new();
        };
        let root = type_root_name(value_type);
        if hir
            .sum_type_for_variant(name)
            .is_some_and(|sum| sum == root)
            && let Some(field_types) = hir.sum_variant_fields.get(name)
            && let Some(field_type) = field_types.first()
        {
            let substitutions = binding_substitutions(hir, value_type);
            let field_type_name = substitute_type_params(&field_type.type_name, &substitutions);
            return match_pattern_binding_types(hir, binding, Some(&field_type_name));
        }
    }

    if let MatchPattern::List {
        prefix,
        rest,
        suffix,
        ..
    } = pattern
    {
        let Some(value_type) = value_type else {
            return Vec::new();
        };
        // Element patterns bind at the list's element type `T` (`List<T>`); a
        // bound rest segment is itself a `List<T>`.
        let element_type = value_type
            .strip_prefix("List<")
            .and_then(|rest| rest.strip_suffix('>'))
            .map(str::trim);
        let mut bindings = Vec::new();
        for element in prefix.iter().chain(suffix) {
            bindings.extend(match_pattern_binding_types(hir, element, element_type));
        }
        if let Some(Some(rest_name)) = rest {
            bindings.push((rest_name.clone(), value_type.to_string()));
        }
        return bindings;
    }

    let MatchPattern::Struct { name, fields, .. } = pattern else {
        return Vec::new();
    };
    let Some(value_type) = value_type else {
        return Vec::new();
    };

    let root = type_root_name(value_type);
    let field_types = if hir
        .sum_type_for_variant(name)
        .is_some_and(|sum| sum == root)
    {
        hir.sum_variant_fields.get(name)
    } else {
        None
    };

    let substitutions = binding_substitutions(hir, value_type);
    if let Some(field_types) = field_types {
        return collect_struct_pattern_binding_types(hir, fields, field_types, &substitutions);
    }

    if name == root
        && let Some(type_info) = hir.type_info(root)
    {
        let field_types = type_info.fields.values().cloned().collect::<Vec<_>>();
        return collect_struct_pattern_binding_types(hir, fields, &field_types, &substitutions);
    }

    Vec::new()
}

/// Build a substitution from a generic type's declared parameters to the
/// concrete arguments in `value_type` (`Pair<Int, Int>` -> `{A: Int, B: Int}`),
/// so match-bound fields carry their resolved element types.
fn binding_substitutions(hir: &Hir, value_type: &str) -> HashMap<String, String> {
    let args = type_arg_names(value_type).unwrap_or_default();
    if args.is_empty() {
        return HashMap::new();
    }
    let root = type_root_name(value_type);
    let params = hir
        .type_info(root)
        .map(|type_info| type_info.type_params.to_vec())
        .or_else(|| {
            builtin_generic_type_params(root)
                .map(|params| params.into_iter().map(String::from).collect())
        })
        .unwrap_or_default();
    params
        .into_iter()
        .zip(args.into_iter().map(String::from))
        .collect()
}

fn collect_struct_pattern_binding_types(
    hir: &Hir,
    fields: &[crate::syntax::ast::MatchFieldPattern],
    field_types: &[FieldInfo],
    substitutions: &HashMap<String, String>,
) -> Vec<(String, String)> {
    let mut bindings = Vec::new();
    for field in fields.iter().filter(|field| !field.ignored) {
        let Some(field_type) = field_types
            .iter()
            .find(|candidate| candidate.name == field.name)
        else {
            continue;
        };
        let field_type_name = substitute_type_params(&field_type.type_name, substitutions);
        if let Some(pattern) = &field.pattern {
            bindings.extend(match_pattern_binding_types(
                hir,
                pattern,
                Some(&field_type_name),
            ));
        } else if let Some(binding) = &field.binding {
            bindings.push((binding.clone(), field_type_name));
        }
    }
    bindings
}

fn classify_block_return_expr(
    hir: &Hir,
    block: &Block,
    value_types: &HashMap<String, String>,
) -> HirReturnProof {
    let Some(statement) = block.statements.iter().next_back() else {
        return HirReturnProof::NoValue;
    };
    match statement {
        Stmt::Return(stmt) => stmt
            .value
            .as_ref()
            .map_or(HirReturnProof::NoValue, |value| {
                classify_return_expr(hir, value, value_types)
            }),
        Stmt::Expr(value) => classify_return_expr(hir, value, value_types),
        Stmt::Let(_) | Stmt::LetElse(_) | Stmt::Assign(_) => HirReturnProof::NoValue,
        Stmt::With { .. }
        | Stmt::If { .. }
        | Stmt::Loop { .. }
        | Stmt::For(_)
        | Stmt::TaskGroup(_)
        | Stmt::Select(_)
        | Stmt::Match { .. }
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => HirReturnProof::Unknown,
    }
}

pub(super) fn classify_return_expr(
    hir: &Hir,
    expr: &Expr,
    value_types: &HashMap<String, String>,
) -> HirReturnProof {
    match expr {
        // `true` / `false` are boolean literals (lexed as identifiers).
        Expr::Ident(name, _) if name == "true" || name == "false" => HirReturnProof::Literal,
        // A bare payload-free sum variant (`return MUL`) names a freshly-valued
        // variant constant; it owns nothing borrowed, so it is fresh.
        Expr::Ident(name, _) if hir.sum_type_for_variant(name).is_some() => HirReturnProof::Literal,
        Expr::Ident(name, _) => HirReturnProof::Ident { name: name.clone() },
        Expr::Call { callee, args, .. } => {
            if matches!(callee_name(callee), "Err" | "None") {
                return HirReturnProof::NoValue;
            }
            if matches!(callee_name(callee), "Ok" | "Some")
                && let Some(arg) = args.first()
            {
                return classify_return_expr(hir, &arg.value, value_types);
            }
            let resolution = match callee {
                Callee::ReceiverCall {
                    receiver, method, ..
                } => infer_hir_expr_type(hir, receiver, value_types).map_or(
                    CallResolution::Unknown,
                    |receiver_type| {
                        hir.resolve_receiver_call(&receiver_type, method, value_types)
                            .0
                    },
                ),
                _ => hir.resolve_call(callee),
            };
            match resolution {
                CallResolution::Resolved {
                    signature,
                    kind:
                        ResolvedCalleeKind::Constructor {
                            type_kind: HirTypeKind::Struct,
                        },
                } if signature.returns_fresh => HirReturnProof::StructConstructor,
                CallResolution::Resolved { signature, .. } if signature.returns_fresh => {
                    HirReturnProof::FreshCall
                }
                CallResolution::Resolved {
                    kind:
                        ResolvedCalleeKind::Constructor {
                            type_kind: HirTypeKind::Struct,
                        },
                    ..
                } => HirReturnProof::StructConstructor,
                // A sum/enum variant constructor (`Pair(a: 1, b: 2)`,
                // `ArgInts(values: take vals)`, `Some(x)`/`Ok(x)` wrappers) builds
                // a brand-new value, so the result is fresh; a moved-in (`take`)
                // payload transfers ownership into the fresh shell.
                CallResolution::EnumVariant => HirReturnProof::Literal,
                CallResolution::Resolved { .. }
                | CallResolution::Ambiguous { .. }
                | CallResolution::Unknown => HirReturnProof::Unknown,
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => classify_return_expr(hir, value, value_types),
        Expr::Match { arms, .. } => arms.first().map_or(HirReturnProof::Unknown, |arm| {
            classify_block_return_expr(hir, &arm.body, value_types)
        }),
        Expr::ObjectLiteral { .. } | Expr::MapLiteral { .. } | Expr::ArrayLiteral { .. } => {
            HirReturnProof::FreshCall
        }
        // String / numeric literals own nothing borrowed; returning one is fresh.
        Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::CharLiteral(_, _)
        | Expr::MultilineString(_, _) => HirReturnProof::Literal,
        Expr::Field { .. }
        | Expr::Index { .. }
        | Expr::Binary { .. }
        | Expr::Closure { .. }
        | Expr::Unknown(_) => HirReturnProof::Unknown,
    }
}
