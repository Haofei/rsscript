use std::collections::BTreeMap;

use crate::semantic::ResolvedType;

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
                        .or_else(|| signature.return_ty.map(|ty| ty.to_string()))
                }
                CallResolution::Ambiguous { .. } | CallResolution::Unknown => match callee {
                    Callee::Name(name) => value_types
                        .get(name)
                        .and_then(|type_name| fn_return_type(type_name))
                        .map(|return_type| return_type.to_string()),
                    Callee::Qualified { .. } | Callee::ReceiverCall { .. } => None,
                },
                CallResolution::EnumVariant => {
                    infer_enum_variant_type(hir, callee_name(callee), args, value_types)
                }
            }
        }
        Expr::Field { base, name, .. } => {
            let base_type = hir.canonical_type_name(&infer_hir_expr_type(hir, base, value_types)?);
            let type_info = hir.type_info(&base_type)?;
            let field = type_info.fields.get(name)?;
            Some(substituted_field_type(hir, type_info, &base_type, field))
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
        Some(return_type.substitute(&substitutions).to_string())
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
                .or_insert_with(|| ResolvedType::from_display(actual));
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
                .or_insert_with(|| ResolvedType::from_display(actual));
        }
    }
}

fn collect_receiver_type_substitutions(
    hir: &Hir,
    signature: &FunctionSig,
    callee: &Callee,
    value_types: &HashMap<String, String>,
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
    receiver_param.ty.clone().collect_substitutions(
        &ResolvedType::from_display(&actual_type),
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
        structural_pattern.collect_substitutions(
            &ResolvedType::from_display(&actual_type),
            generic_params,
            substitutions,
        );
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

/// The type of `field` accessed on a value of type `base_type`, with the type's
/// generic parameters replaced by `base_type`'s concrete arguments — so `item0`
/// on `__Tuple2<Int, String>` resolves to `Int`, not the declared parameter `A`.
pub(super) fn substituted_field_type(
    _hir: &Hir,
    type_info: &TypeInfo,
    base_type: &str,
    field: &FieldInfo,
) -> String {
    let args = type_arg_names(base_type).unwrap_or_default();
    if args.is_empty() || type_info.type_params.is_empty() {
        return field.ty.to_string();
    }
    let substitutions: BTreeMap<String, ResolvedType> = type_info
        .type_params
        .iter()
        .cloned()
        .zip(args.into_iter().map(ResolvedType::from_display))
        .collect();
    field.ty.substitute(&substitutions).to_string()
}

use crate::text_util::builtin_generic_type_params;

pub(super) fn capability_protocol(type_name: &str) -> Option<String> {
    ResolvedType::from_display(type_name)
        .named_argument("Capability", 0)
        .map(ToString::to_string)
}

fn fn_return_type(type_name: &str) -> Option<ResolvedType> {
    ResolvedType::from_display(type_name)
        .function_return()
        .cloned()
}

fn result_ok_type(type_name: &str) -> Option<String> {
    ResolvedType::from_display(type_name)
        .named_argument("Result", 0)
        .cloned()
        .map(ResolvedType::without_fresh)
        .map(|ty| ty.to_string())
}

pub(super) fn list_element_type(type_name: &str) -> Option<String> {
    ResolvedType::from_display(type_name)
        .named_argument("List", 0)
        .map(ToString::to_string)
}

pub(super) fn stream_item_type(type_name: &str) -> Option<String> {
    ResolvedType::from_display(type_name)
        .named_argument("Stream", 0)
        .map(ToString::to_string)
}

fn task_inner_type(type_name: &str) -> Option<String> {
    ResolvedType::from_display(type_name)
        .named_argument("Task", 0)
        .map(ToString::to_string)
}

pub(super) fn match_pattern_binding_type(
    pattern: &MatchPattern,
    value_type: Option<&str>,
) -> Option<(String, String)> {
    let value_type = value_type.map(ResolvedType::from_display);
    match_pattern_binding_resolved_type(pattern, value_type.as_ref())
        .map(|(name, ty)| (name, ty.to_string()))
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
    value_type: Option<&str>,
) -> Vec<(String, String)> {
    let canonical_value_type = value_type.map(|ty| hir.canonical_type_name(ty));
    let value_type = canonical_value_type.as_deref();
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type
            .map(|ty| vec![(name.clone(), ty.to_string())])
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
        let root = type_root_name(value_type);
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
                let field_type_name = field_type.ty.substitute(&substitutions).to_string();
                result.extend(match_pattern_binding_types(
                    hir,
                    binding,
                    Some(&field_type_name),
                ));
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
        let element_type = ResolvedType::from_display(value_type)
            .named_argument("List", 0)
            .map(ToString::to_string);
        let mut bindings = Vec::new();
        for element in prefix.iter().chain(suffix) {
            bindings.extend(match_pattern_binding_types(
                hir,
                element,
                element_type.as_deref(),
            ));
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
fn binding_substitutions(hir: &Hir, value_type: &str) -> BTreeMap<String, ResolvedType> {
    let args = type_arg_names(value_type).unwrap_or_default();
    if args.is_empty() {
        return BTreeMap::new();
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
        .zip(args.into_iter().map(ResolvedType::from_display))
        .collect()
}

fn collect_struct_pattern_binding_types(
    hir: &Hir,
    fields: &[crate::syntax::ast::MatchFieldPattern],
    field_types: &[FieldInfo],
    substitutions: &BTreeMap<String, ResolvedType>,
) -> Vec<(String, String)> {
    let mut bindings = Vec::new();
    for field in fields.iter().filter(|field| !field.ignored) {
        let Some(field_type) = field_types
            .iter()
            .find(|candidate| candidate.name == field.name)
        else {
            continue;
        };
        let field_type_name = field_type.ty.substitute(substitutions).to_string();
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
