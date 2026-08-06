//! Structured signature, field, parameter, and constructor facts.

//! Structural HIR construction helpers.

use super::*;

pub(super) fn hir_binding_kind(kind: LetKind) -> HirBindingKind {
    match kind {
        LetKind::Managed => HirBindingKind::ManagedLet,
        LetKind::Local => HirBindingKind::LocalLet,
    }
}

pub(super) fn function_kind(signature: &FunctionSig) -> ResolvedCalleeKind {
    if signature.is_builtin {
        ResolvedCalleeKind::BuiltinFunction
    } else {
        ResolvedCalleeKind::UserFunction
    }
}

pub(super) fn is_enum_variant_call(name: &str) -> bool {
    matches!(name, "Ok" | "Err" | "Some" | "None" | "Result" | "Option")
}

pub(in crate::hir) fn callee_name(callee: &Callee) -> &str {
    match callee {
        Callee::Name(name) | Callee::Qualified { name, .. } => type_root_name(name),
        Callee::ReceiverCall { method, .. } => method.as_str(),
    }
}

pub(super) fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => format!(
            "{} {}.{method}",
            (*effect).map(|e| e.as_str()).unwrap_or("read"),
            receiver_call_label(receiver)
        ),
    }
}

pub(super) fn receiver_call_label(receiver: &Expr) -> String {
    match receiver {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::MultilineString(value, _) => format!("{value:?}"),
        Expr::Field { base, name, .. } => format!("{}.{}", receiver_call_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", receiver_call_label(base)),
        Expr::Call { callee, .. } => format!("{}()", callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            receiver_call_label(value)
        }
        _ => "<expr>".to_string(),
    }
}

pub(super) fn receiver_facade_namespace(receiver_root: &str, method: &str) -> Option<&'static str> {
    match receiver_root {
        "JsonValue" | "JsonLiteral" => Some("Json"),
        "String" if method.starts_with("json_") => Some("Json"),
        _ => None,
    }
}

pub(super) fn function_sig_from_decl(
    function: &FunctionDecl,
    is_builtin: bool,
    is_external: bool,
) -> FunctionSig {
    let (namespace, name) = split_function_name(&function.name);
    FunctionSig {
        namespace,
        name,
        is_public: function.is_public,
        is_async: function.is_async,
        type_params: function
            .type_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        type_param_bounds: function
            .type_params
            .iter()
            .map(|param| param.bound.clone())
            .collect(),
        params: function.params.iter().map(param_sig_from_decl).collect(),
        return_ty: function.return_ty.as_ref().map(ResolvedType::from_type_ref),
        returns_fresh: function.returns_fresh,
        retained_params: function.retained_params.iter().cloned().collect(),
        is_builtin,
        is_external,
    }
}

pub(super) fn split_function_name(name: &str) -> (Option<String>, String) {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        (Some(namespace.to_string()), name.to_string())
    } else {
        (None, name.to_string())
    }
}

pub(super) fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let prefix = match ty.effective_fn_param_effect(index) {
                    Some(effect) => format!("{} ", effect.as_str()),
                    None => String::new(),
                };
                format!("{prefix}{}", type_ref_name(param))
            })
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
        let args = ty
            .args
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

pub(super) fn record_duplicate_symbol(
    duplicates: &mut Vec<DuplicateSymbol>,
    symbols: &mut HashMap<String, (DuplicateSymbolKind, Span)>,
    kind: DuplicateSymbolKind,
    name: &str,
    span: &Span,
) {
    if let Some((first_kind, first_span)) = symbols.get(name) {
        duplicates.push(DuplicateSymbol {
            kind: duplicate_symbol_kind(*first_kind, kind),
            name: name.to_string(),
            first_span: first_span.clone(),
            duplicate_span: span.clone(),
        });
        return;
    }

    symbols.insert(name.to_string(), (kind, span.clone()));
}

pub(super) fn record_duplicate_fields(duplicates: &mut Vec<DuplicateSymbol>, type_decl: &TypeDecl) {
    let mut fields = HashMap::new();
    for field in &type_decl.fields {
        record_duplicate_symbol(
            duplicates,
            &mut fields,
            DuplicateSymbolKind::Field,
            &format!("{}.{}", type_decl.name, field.name),
            &field.span,
        );
    }
}

pub(super) fn duplicate_symbol_kind(
    first: DuplicateSymbolKind,
    duplicate: DuplicateSymbolKind,
) -> DuplicateSymbolKind {
    match (first, duplicate) {
        (DuplicateSymbolKind::Function, DuplicateSymbolKind::Function) => {
            DuplicateSymbolKind::Function
        }
        (DuplicateSymbolKind::Type, DuplicateSymbolKind::Type) => DuplicateSymbolKind::Type,
        (DuplicateSymbolKind::Field, DuplicateSymbolKind::Field) => DuplicateSymbolKind::Field,
        _ => DuplicateSymbolKind::Constructor,
    }
}

pub(super) fn param_sig_from_decl(param: &Param) -> ParamSig {
    ParamSig {
        name: param.name.clone(),
        effect: effective_param_effect(param),
        ty: ResolvedType::from_type_ref(&param.ty),
        default: param.default.clone(),
    }
}

/// Syntax records whether the author wrote an effect. Semantics add the silent
/// `read` default for ordinary data values; the established closure/Fd
/// exceptions continue to pass without a data-effect external_binding.
pub(super) fn effective_param_effect(param: &Param) -> Option<ParamEffect> {
    param.effective_effect().map(param_effect_from_data_effect)
}

pub(super) fn hir_expr_already_read(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Effect { .. } => true,
        HirExpr::Call {
            callee: Callee::ReceiverCall { effect, .. },
            ..
        } => matches!(effect, Some(DataEffect::Read) | None),
        _ => false,
    }
}

pub(super) fn param_effect_from_data_effect(effect: DataEffect) -> ParamEffect {
    match effect {
        DataEffect::Read => ParamEffect::Read,
        DataEffect::Mut => ParamEffect::Mut,
        DataEffect::Take => ParamEffect::Take,
    }
}

pub(super) fn type_info_from_decl(type_decl: &TypeDecl) -> TypeInfo {
    let fields_ordered = type_decl
        .fields
        .iter()
        .map(field_info_from_decl)
        .collect::<Vec<_>>();
    TypeInfo {
        name: type_decl.name.clone(),
        kind: type_kind_from_decl(type_decl.kind),
        type_params: type_decl
            .type_params
            .iter()
            .map(|param| param.name.clone())
            .collect::<Vec<_>>()
            .into_boxed_slice(),
        fields: fields_ordered
            .iter()
            .map(|field| (field.name.clone(), field.clone()))
            .collect(),
        fields_ordered,
    }
}

pub(super) fn type_kind_from_decl(kind: TypeKind) -> HirTypeKind {
    match kind {
        TypeKind::Class => HirTypeKind::Class,
        TypeKind::Struct => HirTypeKind::Struct,
        TypeKind::Resource => HirTypeKind::Resource,
    }
}

pub(super) fn field_info_from_decl(field: &FieldDecl) -> FieldInfo {
    FieldInfo {
        name: field.name.clone(),
        ty: ResolvedType::from_type_ref(&field.ty),
        is_handle: field.is_handle,
        is_weak: field.is_weak,
        default: field.default.clone(),
    }
}

pub(super) fn constructor_sig_from_type(type_info: &TypeInfo, is_builtin: bool) -> FunctionSig {
    FunctionSig {
        namespace: None,
        name: type_info.name.clone(),
        is_public: is_builtin,
        is_async: false,
        type_params: type_info.type_params.clone(),
        type_param_bounds: vec![None; type_info.type_params.len()],
        params: type_info
            .fields_ordered
            .iter()
            .map(|field| ParamSig {
                name: field.name.clone(),
                effect: None,
                ty: field.ty.clone(),
                default: field.default.clone(),
            })
            .collect(),
        // A generic struct's constructor returns the type *applied to its params*
        // (`Wrap<T>`), not the bare name (`Wrap`). Carrying the params lets
        // `infer_signature_return_type` substitute them from the arguments
        // (`Wrap(item: 7)` -> `Wrap<Int>`); a bare name leaves nothing to
        // substitute and spuriously rejects `let w: Wrap<Int> = Wrap(item: 7)`.
        return_ty: Some(ResolvedType::named(
            type_info.name.clone(),
            type_info
                .type_params
                .iter()
                .map(|parameter| ResolvedType::named(parameter.clone(), [])),
        )),
        returns_fresh: type_info.kind == HirTypeKind::Struct,
        retained_params: HashSet::new(),
        is_builtin,
        is_external: false,
    }
}

pub(super) fn qualified_key(namespace: &str, name: &str) -> String {
    format!("{namespace}.{name}")
}

/// The evaluated sub-expressions of an assignment *target* (the place on the left
/// of `=`), so a checker pass can analyze them like any other expression. The
/// write root itself is excluded (assigning to `x` *defines* `x`, it doesn't read
/// it), but a field/index base *is* read to reach the place, and an index
/// expression is arbitrary evaluated code. So:
///   `x = v`        -> [] (pure write)
///   `x.field = v`  -> [base]                 (base is read)
///   `xs[i] = v`    -> [base, index]          (base read, index evaluated)
/// Nested places recurse naturally because the base is itself a `Field`/`Index`.
/// Used by passes that previously only inspected the assigned `value`, missing
/// awaits, `?`, moves, etc. inside the target (e.g. `xs[await f()] = v`).
pub(super) fn assign_target_reads_impl(target: &HirExpr) -> Vec<&HirExpr> {
    match target {
        HirExpr::Ident { .. } => Vec::new(),
        HirExpr::Field { base, .. } => vec![base.as_ref()],
        HirExpr::Index { base, index, .. } => vec![base.as_ref(), index.as_ref()],
        // Defensive: any other target shape is checked as a whole expression.
        other => vec![other],
    }
}
