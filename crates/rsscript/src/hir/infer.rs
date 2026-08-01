use std::collections::BTreeMap;

use crate::semantic::ResolvedType;

use super::*;

/// Converts type arguments carried by the syntax callee spelling into the
/// structural representation used by HIR inference. Callee type arguments are
/// the sole remaining textual input here; inferred local types never round-trip
/// through a display string.
pub(crate) fn resolved_type_from_source(source: &str) -> ResolvedType {
    let mut rest = source.trim();
    let mut qualifiers = crate::semantic::TypeQualifiers::default();
    loop {
        if let Some(value) = rest.strip_prefix("fresh ").map(str::trim) {
            qualifiers.fresh = true;
            rest = value;
        } else if let Some(value) = rest.strip_prefix("noescape ").map(str::trim) {
            qualifiers.noescape = true;
            rest = value;
        } else if let Some(value) = rest.strip_prefix("owned ").map(str::trim) {
            qualifiers.owned = true;
            rest = value;
        } else {
            break;
        }
    }
    if let Some(parameters) = rest.strip_prefix("Fn(")
        && let Some(close) = matching_function_close(parameters)
    {
        let parameter_text = &parameters[..close];
        let parameters = crate::text_util::split_top_level_type_args(parameter_text);
        let (parameters, effects): (Vec<_>, Vec<_>) = parameters
            .into_iter()
            .filter(|parameter| !parameter.is_empty())
            .map(|parameter| {
                let (effect, parameter) = if let Some(parameter) = parameter.strip_prefix("read ") {
                    (Some(crate::semantic::ResolvedParamEffect::Read), parameter)
                } else if let Some(parameter) = parameter.strip_prefix("mut ") {
                    (Some(crate::semantic::ResolvedParamEffect::Mut), parameter)
                } else if let Some(parameter) = parameter.strip_prefix("take ") {
                    (Some(crate::semantic::ResolvedParamEffect::Take), parameter)
                } else {
                    (None, parameter)
                };
                (resolved_type_from_source(parameter), effect)
            })
            .unzip();
        let return_type = rest
            .get("Fn(".len() + close + 1..)
            .map(str::trim)
            .and_then(|suffix| suffix.strip_prefix("->"))
            .map(str::trim)
            .filter(|suffix| !suffix.is_empty())
            .map(resolved_type_from_source);
        return ResolvedType::function(parameters, effects, return_type, qualifiers);
    }
    let mut resolved = ResolvedType::named(
        type_root_name(rest),
        type_arg_names(rest)
            .unwrap_or_default()
            .into_iter()
            .map(resolved_type_from_source),
    );
    resolved.qualifiers = qualifiers;
    resolved
}

fn matching_function_close(parameters: &str) -> Option<usize> {
    let mut nested = 0usize;
    for (index, character) in parameters.char_indices() {
        match character {
            '<' | '(' => nested = nested.saturating_add(1),
            '>' => nested = nested.saturating_sub(1),
            ')' if nested == 0 => return Some(index),
            ')' => nested = nested.saturating_sub(1),
            _ => {}
        }
    }
    None
}

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
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    let payload_type = |args: &[crate::syntax::ast::CallArg]| {
        args.first()
            .and_then(|arg| infer_hir_expr_type(hir, &arg.value, value_types))
    };
    match variant {
        "Some" => Some(ResolvedType::named("Option", [payload_type(args)?])),
        "Ok" => Some(ResolvedType::named(
            "Result",
            [payload_type(args)?, ResolvedType::named("E", [])],
        )),
        "Err" => Some(ResolvedType::named(
            "Result",
            [ResolvedType::named("T", []), payload_type(args)?],
        )),
        // A user-declared sum variant constructs a value of its sum type, so a `Number(value: 5)`
        // call has type `Token` — letting the normal arg/binding type checks catch misuse.
        _ => hir
            .sum_type_for_variant(variant)
            .map(|name| ResolvedType::named(name, [])),
    }
}

pub(crate) fn infer_hir_expr_type(
    hir: &Hir,
    expr: &Expr,
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    match expr {
        Expr::Ident(name, _) => value_types.get(name).cloned().or_else(|| {
            hir.sum_type_for_variant(name)
                .map(|name| ResolvedType::named(name, []))
        }),
        Expr::Binary { .. } => None,
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            infer_hir_expr_type(hir, value, value_types)
        }
        Expr::Spawn { value, .. } => {
            infer_hir_expr_type(hir, value, value_types).map(|ty| ResolvedType::named("Task", [ty]))
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
                        hir.resolve_receiver_call_structured(&receiver_type, method, value_types)
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
                        .or_else(|| signature.return_ty.clone())
                }
                CallResolution::Ambiguous { .. } | CallResolution::Unknown => match callee {
                    Callee::Name(name) => value_types.get(name).and_then(fn_return_type),
                    Callee::Qualified { .. } | Callee::ReceiverCall { .. } => None,
                },
                CallResolution::EnumVariant => {
                    infer_enum_variant_type(hir, callee_name(callee), args, value_types)
                }
            }
        }
        Expr::Field { base, name, .. } => {
            let base_type = infer_hir_expr_type(hir, base, value_types)?;
            let canonical_base_type = hir.canonical_type_name(&base_type.to_string());
            let type_info = hir.type_info(&canonical_base_type)?;
            let field = type_info.fields.get(name)?;
            Some(substituted_field_type(hir, type_info, &base_type, field))
        }
        Expr::Index { .. } => None,
        Expr::Number(value, _) => Some(ResolvedType::named(number_literal_type_name(value), [])),
        Expr::String(_, _) | Expr::MultilineString(_, _) => Some(ResolvedType::named("String", [])),
        Expr::CharLiteral(_, _) => Some(ResolvedType::named("Char", [])),
        Expr::ObjectLiteral { .. } => Some(ResolvedType::named("JsonLiteral", [])),
        Expr::MapLiteral { .. } => Some(ResolvedType::named("MapLiteral", [])),
        Expr::ArrayLiteral { items, .. } => {
            let item_type = items
                .first()
                .and_then(|item| infer_hir_expr_type(hir, item, value_types))
                .unwrap_or_else(|| ResolvedType::named("?", []));
            Some(ResolvedType::named("List", [item_type]))
        }
        Expr::Closure { .. } | Expr::Unknown(_) => None,
    }
}

fn infer_signature_return_type(
    hir: &Hir,
    signature: &FunctionSig,
    callee: &Callee,
    args: &[CallArg],
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    let return_type = signature.return_ty.clone()?;
    if signature.type_params.is_empty() {
        return None;
    }

    let generic_params = signature
        .type_params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut substitutions = BTreeMap::new();
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
        Some(return_type.substitute(&substitutions))
    }
}

fn collect_callee_type_substitutions(
    signature: &FunctionSig,
    callee: &Callee,
    generic_params: &HashSet<&str>,
    substitutions: &mut BTreeMap<String, ResolvedType>,
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
                .or_insert_with(|| resolved_type_from_source(actual));
        }
    }
}

fn collect_namespace_type_substitutions(
    hir: &Hir,
    callee: &Callee,
    generic_params: &HashSet<&str>,
    substitutions: &mut BTreeMap<String, ResolvedType>,
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
                .or_insert_with(|| resolved_type_from_source(actual));
        }
    }
}

fn collect_receiver_type_substitutions(
    hir: &Hir,
    signature: &FunctionSig,
    callee: &Callee,
    value_types: &HirValueTypes,
    generic_params: &HashSet<&str>,
    substitutions: &mut BTreeMap<String, ResolvedType>,
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
    receiver_param
        .ty
        .clone()
        .collect_substitutions(&actual_type, generic_params, substitutions);
}

fn collect_arg_type_substitutions(
    hir: &Hir,
    signature: &FunctionSig,
    args: &[CallArg],
    value_types: &HirValueTypes,
    generic_params: &HashSet<&str>,
    substitutions: &mut BTreeMap<String, ResolvedType>,
) {
    for (index, arg) in args.iter().enumerate() {
        let Some((_parameter_index, param)) = arg
            .name
            .as_deref()
            .and_then(|name| {
                signature
                    .params
                    .iter()
                    .enumerate()
                    .find(|(_, param)| param.name == name)
            })
            .or_else(|| signature.params.get(index).map(|param| (index, param)))
        else {
            continue;
        };
        let (actual_type, structural_pattern) = if param.ty.qualifiers.noescape
            && let Some(expected_return_type) = param.ty.function_return()
            && let Expr::Closure { body, .. } = &arg.value
            && let Some(actual_return_type) = infer_closure_return_type(hir, body, value_types)
        {
            (actual_return_type, expected_return_type.clone())
        } else {
            let Some(actual_type) = infer_arg_expr_type(hir, &arg.value, value_types) else {
                continue;
            };
            (actual_type, param.ty.clone())
        };
        structural_pattern.collect_substitutions(&actual_type, generic_params, substitutions);
    }
}

pub(super) fn infer_closure_return_type(
    hir: &Hir,
    body: &Block,
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
    if let Some(statement) = body.statements.iter().next_back() {
        match statement {
            Stmt::Return(stmt) => {
                return stmt
                    .value
                    .as_ref()
                    .and_then(|value| infer_hir_expr_type(hir, value, value_types))
                    .or_else(|| Some(ResolvedType::named("Unit", [])));
            }
            Stmt::Expr(value) => return infer_hir_expr_type(hir, value, value_types),
            Stmt::Let(_) | Stmt::LetElse(_) | Stmt::Assign(_) => {
                return Some(ResolvedType::named("Unit", []));
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
    Some(ResolvedType::named("Unit", []))
}

fn infer_arg_expr_type(
    hir: &Hir,
    expr: &Expr,
    value_types: &HirValueTypes,
) -> Option<ResolvedType> {
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
                ResolvedType::function(
                    (0..params.len()).map(|_| ResolvedType::named("?", [])),
                    (0..params.len()).map(|_| None),
                    Some(return_type),
                    crate::semantic::TypeQualifiers {
                        noescape: true,
                        ..crate::semantic::TypeQualifiers::default()
                    },
                )
            }),
        Expr::Match { .. } => infer_hir_expr_type(hir, expr, value_types),
        Expr::ObjectLiteral { .. } | Expr::MapLiteral { .. } | Expr::ArrayLiteral { .. } => {
            infer_hir_expr_type(hir, expr, value_types)
        }
        Expr::Field { .. } => infer_hir_expr_type(hir, expr, value_types),
        // Scalar literals carry a type so generic construction can unify it with a
        // type parameter (`Pair(item0: 1)` -> `A = Int`).
        Expr::Number(value, _) => Some(ResolvedType::named(number_literal_type_name(value), [])),
        Expr::String(_, _) | Expr::MultilineString(_, _) => Some(ResolvedType::named("String", [])),
        Expr::CharLiteral(_, _) => Some(ResolvedType::named("Char", [])),
        Expr::Index { .. } | Expr::Binary { .. } | Expr::Unknown(_) => None,
    }
}

/// The type of `field` accessed on a value of type `base_type`, with the type's
/// generic parameters replaced by `base_type`'s concrete arguments — so `item0`
/// on `__Tuple2<Int, String>` resolves to `Int`, not the declared parameter `A`.
pub(super) fn substituted_field_type(
    _hir: &Hir,
    type_info: &TypeInfo,
    base_type: &ResolvedType,
    field: &FieldInfo,
) -> ResolvedType {
    let args = base_type.arguments();
    if args.is_empty() || type_info.type_params.is_empty() {
        return field.ty.clone();
    }
    let substitutions: BTreeMap<String, ResolvedType> = type_info
        .type_params
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect();
    field.ty.substitute(&substitutions)
}

use crate::text_util::builtin_generic_type_params;

pub(super) fn capability_protocol(type_name: &ResolvedType) -> Option<String> {
    type_name
        .named_argument("Capability", 0)
        .map(ToString::to_string)
}

fn fn_return_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name.function_return().cloned()
}

fn result_ok_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name
        .named_argument("Result", 0)
        .cloned()
        .map(ResolvedType::without_fresh)
}

pub(super) fn list_element_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name.named_argument("List", 0).cloned()
}

pub(super) fn stream_item_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name.named_argument("Stream", 0).cloned()
}

fn task_inner_type(type_name: &ResolvedType) -> Option<ResolvedType> {
    type_name.named_argument("Task", 0).cloned()
}

pub(super) fn match_pattern_binding_type(
    pattern: &MatchPattern,
    value_type: Option<&ResolvedType>,
) -> Option<(String, ResolvedType)> {
    match_pattern_binding_resolved_type(pattern, value_type)
}

fn match_pattern_binding_resolved_type(
    pattern: &MatchPattern,
    value_type: Option<&ResolvedType>,
) -> Option<(String, ResolvedType)> {
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type.map(|ty| (name.clone(), ty.clone()));
    }
    let MatchPattern::Variant { name, bindings, .. } = pattern else {
        return None;
    };
    // Option/Result carry a single positional payload.
    let Some(binding) = bindings.first() else {
        return None;
    };
    let value_type = value_type?;
    if name == "Some" {
        return value_type
            .named_argument("Option", 0)
            .and_then(|ty| match_pattern_binding_resolved_type(binding, Some(ty)));
    }
    match name.as_str() {
        "Ok" => value_type
            .named_argument("Result", 0)
            .and_then(|ty| match_pattern_binding_resolved_type(binding, Some(ty))),
        "Err" => value_type
            .named_argument("Result", 1)
            .and_then(|ty| match_pattern_binding_resolved_type(binding, Some(ty))),
        _ => None,
    }
}

pub(super) fn match_pattern_binding_types(
    hir: &Hir,
    pattern: &MatchPattern,
    value_type: Option<&ResolvedType>,
) -> Vec<(String, ResolvedType)> {
    let canonical_value_type = value_type.map(|ty| {
        let canonical = hir.canonical_type_name(&ty.to_string());
        resolved_type_from_source(&canonical)
    });
    let value_type = canonical_value_type.as_ref();
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type
            .map(|ty| vec![(name.clone(), ty.clone())])
            .unwrap_or_default();
    }
    if let Some(binding) = match_pattern_binding_type(pattern, value_type) {
        return vec![binding];
    }

    if let MatchPattern::Variant { name, bindings, .. } = pattern
        && !bindings.is_empty()
    {
        let Some(value_type) = value_type else {
            return Vec::new();
        };
        let root = value_type.root_name().unwrap_or_default();
        if hir
            .sum_type_for_variant(name)
            .is_some_and(|sum| sum == root)
            && let Some(field_types) = hir.sum_variant_fields.get(name)
        {
            let substitutions = binding_substitutions(hir, value_type);
            // Zip each positional sub-pattern with the variant's declared fields
            // by index.
            let mut result = Vec::new();
            for (binding, field_type) in bindings.iter().zip(field_types.iter()) {
                let field_type = field_type.ty.substitute(&substitutions);
                result.extend(match_pattern_binding_types(hir, binding, Some(&field_type)));
            }
            return result;
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
        let element_type = value_type.named_argument("List", 0).cloned();
        let mut bindings = Vec::new();
        for element in prefix.iter().chain(suffix) {
            bindings.extend(match_pattern_binding_types(
                hir,
                element,
                element_type.as_ref(),
            ));
        }
        if let Some(Some(rest_name)) = rest {
            bindings.push((rest_name.clone(), value_type.clone()));
        }
        return bindings;
    }

    let MatchPattern::Struct { name, fields, .. } = pattern else {
        return Vec::new();
    };
    let Some(value_type) = value_type else {
        return Vec::new();
    };

    let root = value_type.root_name().unwrap_or_default();
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
fn binding_substitutions(hir: &Hir, value_type: &ResolvedType) -> BTreeMap<String, ResolvedType> {
    let args = value_type.arguments();
    if args.is_empty() {
        return BTreeMap::new();
    }
    let root = value_type.root_name().unwrap_or_default();
    let params = hir
        .type_info(root)
        .map(|type_info| type_info.type_params.to_vec())
        .or_else(|| {
            builtin_generic_type_params(root)
                .map(|params| params.into_iter().map(String::from).collect())
        })
        .unwrap_or_default();
    params.into_iter().zip(args.iter().cloned()).collect()
}

fn collect_struct_pattern_binding_types(
    hir: &Hir,
    fields: &[crate::syntax::ast::MatchFieldPattern],
    field_types: &[FieldInfo],
    substitutions: &BTreeMap<String, ResolvedType>,
) -> Vec<(String, ResolvedType)> {
    let mut bindings = Vec::new();
    for field in fields.iter().filter(|field| !field.ignored) {
        let Some(field_type) = field_types
            .iter()
            .find(|candidate| candidate.name == field.name)
        else {
            continue;
        };
        let field_type = field_type.ty.substitute(substitutions);
        if let Some(pattern) = &field.pattern {
            bindings.extend(match_pattern_binding_types(hir, pattern, Some(&field_type)));
        } else if let Some(binding) = &field.binding {
            bindings.push((binding.clone(), field_type));
        }
    }
    bindings
}

fn classify_block_return_expr(
    hir: &Hir,
    block: &Block,
    value_types: &HirValueTypes,
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
    value_types: &HirValueTypes,
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
                        hir.resolve_receiver_call_structured(&receiver_type, method, value_types)
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
