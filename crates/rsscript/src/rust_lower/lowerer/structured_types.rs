//! Semantic-fact-backed call substitution, receiver typing, and field typing.

use super::*;
use crate::text_util::builtin_generic_type_params;

impl<'a> RustLowerer<'a> {
    pub(in crate::rust_lower) fn lower_capability_from_call(
        &mut self,
        protocol: &str,
        args: &[CallArg],
    ) -> String {
        let Some(value_arg) = args
            .iter()
            .find(|arg| arg.name.as_deref() == Some("value"))
            .or_else(|| args.first())
        else {
            return format!("{}::/* missing value */", capability_enum_name(protocol));
        };
        let value_type = self.infer_expr_type(&value_arg.value);
        let value_type = value_type.map(|ty| self.canonical_type_ref(&ty));
        let value_type_name = value_type
            .as_ref()
            .map(type_ref_display_name)
            .unwrap_or_else(|| "Unknown".to_string());
        let value_type_root = type_root_name(&value_type_name);
        format!(
            "{}::{}({})",
            capability_enum_name(protocol),
            rust_ident(value_type_root),
            self.lower_expr(&value_arg.value)
        )
    }

    pub(in crate::rust_lower) fn infer_call_return_type(
        &self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> Option<TypeRef> {
        let key = native_boundary_callee_key(callee);
        let return_ty = self.function_return_types.get(&key)?.clone();
        let Some(type_params) = self.function_type_params.get(&key) else {
            return Some(return_ty);
        };
        if type_params.is_empty() {
            return Some(return_ty);
        }

        let mut substitutions = BTreeMap::new();
        self.collect_explicit_call_type_substitutions(
            callee,
            type_params,
            span,
            &mut substitutions,
        );
        self.collect_arg_type_substitutions(callee, args, &mut substitutions);

        if substitutions.is_empty() {
            Some(return_ty)
        } else {
            Some(substitute_type_ref(&return_ty, &substitutions))
        }
    }

    pub(in crate::rust_lower) fn collect_explicit_call_type_substitutions(
        &self,
        callee: &Callee,
        type_params: &[String],
        span: &Span,
        substitutions: &mut BTreeMap<String, TypeRef>,
    ) {
        let explicit = match callee {
            Callee::Name(name) | Callee::Qualified { name, .. } => type_arg_names(name),
            Callee::ReceiverCall { method, .. } => type_arg_names(method),
        };
        if let Some(explicit) = explicit {
            for (param, actual) in type_params.iter().zip(explicit) {
                substitutions
                    .entry(param.clone())
                    .or_insert_with(|| type_ref_from_display(actual, span));
            }
        }

        let Callee::Qualified { namespace, .. } = callee else {
            return;
        };
        let Some(namespace_args) = type_arg_names(namespace) else {
            return;
        };
        let Some(namespace_params) = builtin_generic_type_params(type_root_name(namespace)) else {
            return;
        };
        for (param, actual) in namespace_params.into_iter().zip(namespace_args) {
            if type_params.iter().any(|type_param| type_param == param) {
                substitutions
                    .entry(param.to_string())
                    .or_insert_with(|| type_ref_from_display(actual, span));
            }
        }
    }

    pub(in crate::rust_lower) fn collect_arg_type_substitutions(
        &self,
        callee: &Callee,
        args: &[CallArg],
        substitutions: &mut BTreeMap<String, TypeRef>,
    ) {
        let key = native_boundary_callee_key(callee);
        let Some(params) = self.function_param_types.get(&key) else {
            return;
        };
        let Some(type_params) = self.function_type_params.get(&key) else {
            return;
        };
        for (index, arg) in args.iter().enumerate() {
            let Some((_, expected)) = arg
                .name
                .as_deref()
                .and_then(|name| params.iter().find(|(param_name, _)| param_name == name))
                .or_else(|| params.get(index))
            else {
                continue;
            };
            let Some(actual) = self.infer_call_arg_type(&arg.value) else {
                continue;
            };
            collect_type_ref_substitutions(expected, &actual, type_params, substitutions);
        }
    }

    pub(in crate::rust_lower) fn call_arg_is_retained(
        &self,
        callee: &Callee,
        arg: &CallArg,
        _index: usize,
    ) -> bool {
        let Some(name) = arg.name.as_deref() else {
            return false;
        };
        self.retained_params_by_callee
            .get(&native_boundary_callee_key(callee))
            .is_some_and(|retained| retained.contains(name))
    }

    pub(in crate::rust_lower) fn is_protocol_callee(&self, callee: &Callee) -> bool {
        matches!(callee, Callee::Qualified { namespace, .. } if self.protocol_names.contains(namespace))
    }

    pub(in crate::rust_lower) fn lower_return_expr(&mut self, expr: &Expr) -> String {
        let lowered = if let Some(expected) = self.current_return_type.clone() {
            self.lower_expr_for_expected_type(expr, &expected)
        } else {
            self.lower_expr(expr)
        };
        if self
            .current_return_type
            .as_ref()
            .is_some_and(is_result_type)
            && !is_result_constructor_expr(expr)
            && !self.expr_returns_result(expr)
        {
            format!("Ok({lowered})")
        } else {
            lowered
        }
    }

    pub(in crate::rust_lower) fn expr_returns_result(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Call { callee, .. } => {
                if let Callee::ReceiverCall {
                    receiver, method, ..
                } = callee
                {
                    return self
                        .infer_expr_type(receiver)
                        .map(|receiver_type| {
                            let namespace = self.receiver_call_namespace(&receiver_type, method);
                            let qualified = Callee::Qualified {
                                namespace,
                                name: method.clone(),
                            };
                            self.function_return_types
                                .get(&native_boundary_callee_key(&qualified))
                                .is_some_and(is_result_type)
                        })
                        .unwrap_or(false);
                }
                self.function_return_types
                    .get(&native_boundary_callee_key(callee))
                    .is_some_and(is_result_type)
                    || matches!(
                        callee,
                        Callee::Name(name)
                            if self
                                .value_types
                                .get(name)
                                .and_then(fn_type_return)
                                .is_some_and(is_result_type)
                    )
            }
            Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
                self.expr_returns_result(value)
            }
            Expr::Match { arms, .. } => {
                arms.first().is_some_and(|arm| {
                    arm.body.statements.iter().next_back().is_some_and(
                        |statement| match statement {
                            Stmt::Return(stmt) => stmt
                                .value
                                .as_ref()
                                .is_some_and(|value| self.expr_returns_result(value)),
                            Stmt::Expr(value) => self.expr_returns_result(value),
                            _ => false,
                        },
                    )
                })
            }
            _ => false,
        }
    }

    pub(in crate::rust_lower) fn receiver_call_expected_arg_type(
        &self,
        namespace: &str,
        method: &str,
        receiver_type: Option<&TypeRef>,
        arg_name: Option<&str>,
        arg_index: usize,
    ) -> Option<TypeRef> {
        let positional_name = || {
            let qualified = Callee::Qualified {
                namespace: namespace.to_string(),
                name: method.to_string(),
            };
            let key = native_boundary_callee_key(&qualified);
            self.function_param_types
                .get(&key)?
                .get(arg_index + 1)
                .map(|(name, _)| name.as_str())
        };
        let resolved_arg_name = arg_name.or_else(positional_name);
        if type_root_name(namespace) == "List" {
            let receiver_type = receiver_type?;
            let item_type = receiver_type.args.first()?.clone();
            return match type_root_name(method) {
                "push" | "set" if resolved_arg_name == Some("value") => Some(item_type),
                "append" if resolved_arg_name == Some("values") => Some(TypeRef {
                    name: "List".to_string(),
                    args: vec![item_type],
                    malformed_arg_spans: Vec::new(),
                    is_fresh: false,
                    is_noescape: false,
                    is_owned: false,
                    fn_params: Vec::new(),
                    fn_param_effects: Vec::new(),
                    fn_return: None,
                    span: receiver_type.span.clone(),
                }),
                "join" if resolved_arg_name == Some("separator") => {
                    Some(simple_type_ref("String", &receiver_type.span))
                }
                "count_where" | "any" | "all" | "find" | "filter"
                    if resolved_arg_name == Some("predicate") =>
                {
                    Some(fn_type_ref(
                        vec![item_type],
                        Some(simple_type_ref("Bool", &receiver_type.span)),
                        &receiver_type.span,
                    ))
                }
                "map" if resolved_arg_name == Some("mapper") => {
                    Some(fn_type_ref(vec![item_type], None, &receiver_type.span))
                }
                "flat_map" if resolved_arg_name == Some("mapper") => {
                    Some(fn_type_ref(vec![item_type], None, &receiver_type.span))
                }
                "sort_with" if resolved_arg_name == Some("compare") => Some(fn_type_ref(
                    vec![item_type.clone(), item_type],
                    Some(simple_type_ref("Int", &receiver_type.span)),
                    &receiver_type.span,
                )),
                _ => None,
            };
        }
        if type_root_name(namespace) == "Result" {
            let receiver_type = receiver_type?;
            let ok_type = receiver_type.args.first()?.clone();
            let err_type = receiver_type.args.get(1)?.clone();
            return match type_root_name(method) {
                "unwrap_or" if resolved_arg_name == Some("default") => Some(ok_type),
                "map" if resolved_arg_name == Some("mapper") => {
                    Some(fn_type_ref(vec![ok_type], None, &receiver_type.span))
                }
                "and_then" if resolved_arg_name == Some("mapper") => Some(fn_type_ref(
                    vec![ok_type],
                    Some(TypeRef {
                        name: "Result".to_string(),
                        args: vec![simple_type_ref("Unit", &receiver_type.span), err_type],
                        malformed_arg_spans: Vec::new(),
                        is_fresh: false,
                        is_noescape: false,
                        is_owned: false,
                        fn_params: Vec::new(),
                        fn_param_effects: Vec::new(),
                        fn_return: None,
                        span: receiver_type.span.clone(),
                    }),
                    &receiver_type.span,
                )),
                _ => None,
            };
        }
        if type_root_name(namespace) == "Option" {
            let receiver_type = receiver_type?;
            let item_type = receiver_type.args.first()?.clone();
            return match type_root_name(method) {
                "unwrap_or" if resolved_arg_name == Some("default") => Some(item_type),
                "map" | "and_then" if resolved_arg_name == Some("mapper") => {
                    Some(fn_type_ref(vec![item_type], None, &receiver_type.span))
                }
                "filter" if resolved_arg_name == Some("predicate") => Some(fn_type_ref(
                    vec![item_type],
                    Some(simple_type_ref("Bool", &receiver_type.span)),
                    &receiver_type.span,
                )),
                _ => None,
            };
        }
        if type_root_name(namespace) == "Map" {
            let receiver_type = receiver_type?;
            let key_type = receiver_type.args.first()?.clone();
            let value_type = receiver_type.args.get(1)?.clone();
            return match type_root_name(method) {
                "contains_key" | "get" | "get_or_default" | "insert" | "insert_old" | "remove"
                    if resolved_arg_name == Some("key") =>
                {
                    Some(key_type)
                }
                "get_or_default" | "insert" | "insert_old"
                    if resolved_arg_name == Some("value") =>
                {
                    Some(value_type)
                }
                "get_or_default" if resolved_arg_name == Some("default") => Some(value_type),
                _ => None,
            };
        }
        let qualified = Callee::Qualified {
            namespace: namespace.to_string(),
            name: method.to_string(),
        };
        let key = native_boundary_callee_key(&qualified);
        if let Some(params) = self.function_param_types.get(&key) {
            if let Some(arg_name) = arg_name
                && let Some((_, ty)) = params.iter().find(|(param_name, _)| param_name == arg_name)
            {
                return Some(ty.clone());
            }
            if arg_name.is_none()
                && let Some((_, ty)) = params.get(arg_index + 1)
            {
                return Some(ty.clone());
            }
        }
        None
    }

    pub(in crate::rust_lower) fn lower_receiver_positional_arg(
        &mut self,
        value: &Expr,
        expected: &TypeRef,
    ) -> String {
        let expected = self.canonical_type_ref(expected);
        let expected = &expected;
        if expected.name == "String"
            && expected.args.is_empty()
            && let Expr::Ident(name, _) = value
            && self.param_effects.get(name) == Some(&DataEffect::Read)
        {
            return rust_value_ident(name);
        }
        self.lower_expr_for_expected_type(value, expected)
    }

    pub(in crate::rust_lower) fn receiver_call_namespace(
        &self,
        receiver_type: &TypeRef,
        method: &str,
    ) -> String {
        let receiver_type = self.canonical_type_ref(receiver_type);
        let receiver_type_name = type_ref_display_name(&receiver_type);
        let receiver_type_root = type_root_name(&receiver_type_name).to_string();
        self.generic_protocol_bounds
            .get(&receiver_type_name)
            .cloned()
            .or_else(|| capability_protocol_name(&receiver_type_name).map(str::to_string))
            .or_else(|| self.protocol_impl_namespace(&receiver_type_root, method))
            .or_else(|| receiver_facade_namespace(&receiver_type_root, method).map(str::to_string))
            .unwrap_or(receiver_type_root)
    }

    pub(in crate::rust_lower) fn receiver_call_expected_arg_effect(
        &self,
        namespace: &str,
        method: &str,
        arg_index: usize,
    ) -> Option<DataEffect> {
        let qualified = Callee::Qualified {
            namespace: namespace.to_string(),
            name: method.to_string(),
        };
        let key = native_boundary_callee_key(&qualified);
        self.function_param_effects.get(&key)?.get(arg_index + 1)?.1
    }

    pub(in crate::rust_lower) fn receiver_call_declared_arg_type(
        &self,
        namespace: &str,
        method: &str,
        arg_name: Option<&str>,
        arg_index: usize,
    ) -> Option<TypeRef> {
        let qualified = Callee::Qualified {
            namespace: namespace.to_string(),
            name: method.to_string(),
        };
        let params = self
            .function_param_types
            .get(&native_boundary_callee_key(&qualified))?;
        if let Some(arg_name) = arg_name {
            params
                .iter()
                .find(|(param_name, _)| param_name == arg_name)
                .map(|(_, ty)| ty.clone())
        } else {
            params.get(arg_index + 1).map(|(_, ty)| ty.clone())
        }
    }

    /// Builtin value types whose `.clone()` the checker resolves (via the `Clone`
    /// protocol) and which lower to a `Clone` Rust type but have no dedicated clone
    /// runtime intrinsic — so `.clone()` must lower to Rust's `.clone()` directly.
    /// Without this they fell through to a dangling `List_clone`-style call (E0425).
    /// `String`/`Json` are excluded (they have their own clone intrinsics);
    /// `Deque`/`Set`/`Map`/resources are rejected at check time, so they never reach
    /// here.
    pub(in crate::rust_lower) fn is_builtin_clone_value(name: &str) -> bool {
        matches!(name, "List" | "Bytes" | "Buffer")
    }

    pub(in crate::rust_lower) fn is_string_comparison_operand(&self, expr: &Expr) -> bool {
        matches!(expr, Expr::String(_, _) | Expr::MultilineString(_, _))
            || self
                .infer_expr_type(expr)
                .is_some_and(|ty| ty.name == "String" && ty.args.is_empty())
    }

    pub(in crate::rust_lower) fn lower_string_comparison_operand(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::String(value, _) => format!("{:?}", decode_string_token(value)),
            Expr::MultilineString(value, _) => format!("{value:?}"),
            _ if self.is_string_comparison_operand(expr) => {
                format!("{}.as_str()", self.lower_expr(expr))
            }
            _ => self.lower_expr(expr),
        }
    }

    pub(in crate::rust_lower) fn lower_retained_expr_for_expected_type(
        &mut self,
        expr: &Expr,
        expected: &TypeRef,
    ) -> String {
        let expected = self.canonical_type_ref(expected);
        let expected = &expected;
        match expr {
            Expr::Ident(name, _) if !is_copy_type_ref(expected) => {
                format!("{}.clone()", rust_value_ident(name))
            }
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                ..
            } => self.lower_owned_expr_for_expected_type(value, expected),
            _ => self.lower_owned_expr_for_expected_type(expr, expected),
        }
    }

    pub(in crate::rust_lower) fn lower_owned_expr_for_expected_type(
        &mut self,
        expr: &Expr,
        expected: &TypeRef,
    ) -> String {
        let expected = self.canonical_type_ref(expected);
        let expected = &expected;
        match expr {
            Expr::Ident(name, _)
                if !is_copy_type_ref(expected)
                    && matches!(
                        self.param_effects.get(name),
                        Some(DataEffect::Read | DataEffect::Mut)
                    ) =>
            {
                format!("{}.clone()", rust_value_ident(name))
            }
            Expr::Effect {
                effect: DataEffect::Read | DataEffect::Mut,
                value,
                ..
            } => {
                let lowered = self.lower_expr_for_expected_type(value, expected);
                if is_copy_type_ref(expected) {
                    lowered
                } else {
                    format!("{lowered}.clone()")
                }
            }
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                ..
            } => self.lower_expr_for_expected_type(value, expected),
            _ => self.lower_expr_for_expected_type(expr, expected),
        }
    }

    pub(in crate::rust_lower) fn field_type(
        &self,
        type_name: &str,
        field_name: &str,
    ) -> Option<TypeRef> {
        let span = Span {
            file: "<inferred>".to_string(),
            line: 1,
            column: 1,
            length: 1,
        };
        let ty = self.canonical_type_ref(&ResolvedType::from_display(type_name).to_type_ref(&span));
        let root = type_root_name(&ty.name);
        if let Some(semantic_types) = self.semantic_types
            && let Some(type_facts) = semantic_types.named_type(root)
        {
            let field = type_facts
                .fields
                .iter()
                .find(|(name, _)| name == field_name)
                .map(|(_, ty)| semantic_types.arena().get(*ty).to_type_ref(&span))?;
            return Some(substitute_generic_type(
                &field,
                &type_facts.type_parameters,
                &ty.args,
            ));
        }
        let field = self
            .type_fields
            .get(root)?
            .iter()
            .find(|field| field.name == field_name)
            .map(|field| field.ty.clone())?;
        let params = self.type_params.get(root).map(Vec::as_slice).unwrap_or(&[]);
        Some(substitute_generic_type(&field, params, &ty.args))
    }

    pub(in crate::rust_lower) fn is_resource_type(&self, ty: &TypeRef) -> bool {
        let ty = self.canonical_type_ref(ty);
        matches!(self.type_kinds.get(&ty.name), Some(TypeKind::Resource))
    }
}
