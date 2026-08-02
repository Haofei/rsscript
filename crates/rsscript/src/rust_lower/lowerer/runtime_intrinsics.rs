//! Runtime intrinsic, constructor, receiver, and native-boundary call emission.

use super::*;

impl<'a> RustLowerer<'a> {
    /// Lowers `json_decode` and `json_encode` codec calls.
    pub(in crate::rust_lower) fn lower_call_json_codec(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> Option<String> {
        if is_json_decode_callee(callee) {
            return Some(self.lower_json_decode_call(callee, args, span));
        }
        if is_json_encode_callee(callee)
            && let Some(value_arg) = args
                .iter()
                .find(|arg| arg.name.as_deref() == Some("value") || arg.name.is_none())
        {
            return Some(self.lower_json_value(&value_arg.value));
        }
        None
    }

    /// `Task.cancellation_token()` resolves to the enclosing task_group's token,
    /// or a never-cancelled token outside one.
    pub(in crate::rust_lower) fn lower_call_task_cancellation_token(
        &mut self,
        callee: &Callee,
    ) -> Option<String> {
        if let Callee::Qualified { namespace, name } = callee
            && type_root_name(namespace) == "Task"
            && type_root_name(name) == "cancellation_token"
        {
            return Some(match &self.current_task_group_token {
                Some(guard) => format!("{guard}.token()"),
                None => "rsscript_runtime::cancellation_never()".to_string(),
            });
        }
        None
    }

    /// `Callee::Name` constructor forms: user struct/class constructors, user
    /// sum-type payload-variant construction, Rust enum constructors, and
    /// runtime struct constructors. Returns `None` if the callee is not a name
    /// or names no known constructor (so the call falls through to dispatch).
    pub(in crate::rust_lower) fn lower_call_named_constructor(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> Option<String> {
        let Callee::Name(name) = callee else {
            return None;
        };
        // A turbofish constructor callee carries its type arguments in the
        // raw name (e.g. `Pair<Int>`), but `type_kinds` and the per-type
        // field/handle lookups are keyed by the bare root (`Pair`). Root the
        // name so the named-field constructor path is taken (and the struct
        // literal is emitted as `Pair { .. }`, letting Rust infer the args)
        // instead of falling through to a positional tuple-struct call.
        let ctor_name = type_root_name(name);
        if let Some(type_kind) = self.type_kinds.get(ctor_name).copied() {
            let declared_fields = self.type_fields.get(ctor_name).cloned().unwrap_or_default();
            let field_names = declared_fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>();
            let field_has_default = declared_fields
                .iter()
                .map(|field| field.default.is_some())
                .collect::<Vec<_>>();
            let field_allows_shorthand = vec![true; declared_fields.len()];
            let argument_names = args
                .iter()
                .map(|argument| argument.name.as_deref())
                .collect::<Vec<_>>();
            let argument_shorthand_names = args
                .iter()
                .map(|argument| match &argument.value {
                    Expr::Ident(name, _) => Some(name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let binding = crate::call_binding::CallBinding::bind(
                &field_names,
                &field_has_default,
                &field_allows_shorthand,
                &argument_names,
                &argument_shorthand_names,
                0,
            );
            if !binding.is_complete() {
                unreachable_lowering("constructor call binding", span);
            }

            let mut fields = Vec::new();
            for bound in binding.evaluation_order() {
                let field_decl = &declared_fields[bound.parameter_index];
                let (value, value_span, is_default) = match bound.source {
                    crate::call_binding::BoundArgumentSource::Receiver => unreachable!(),
                    crate::call_binding::BoundArgumentSource::Explicit(source_index) => {
                        let argument = &args[source_index];
                        (&argument.value, &argument.span, false)
                    }
                    crate::call_binding::BoundArgumentSource::Default => (
                        field_decl
                            .default
                            .as_ref()
                            .expect("bound constructor default should exist"),
                        span,
                        true,
                    ),
                };
                let field_name = field_decl.name.as_str();
                let field = rust_ident(field_name);
                let is_weak_field = self.is_weak_field(ctor_name, field_name);
                let previous_lowering_default = self.lowering_default;
                self.lowering_default |= is_default;
                if is_weak_field {
                    fields.push(format!(
                        "{field}: {}",
                        self.lower_explicit_weak_field_value(value)
                    ));
                } else if self.is_runtime_handle_field(ctor_name, field_name) {
                    fields.push(format!(
                        "{field}: {}",
                        self.lower_runtime_handle_field_value(name, field_name, value, value_span)
                    ));
                } else {
                    let value = self
                        .field_type(name, field_name)
                        .map(|expected| self.lower_expr_for_expected_type(value, &expected))
                        .unwrap_or_else(|| self.lower_owned_expr(value));
                    fields.push(format!("{field}: {value}"));
                }
                self.lowering_default = previous_lowering_default;
            }
            let fields = fields.join(", ");
            let constructed = format!("{} {{ {fields} }}", rust_ident(ctor_name));
            if type_kind == TypeKind::Class {
                return Some(format!(
                    "rsscript_runtime::manage_at({constructed}, {})",
                    lower_source_span(span)
                ));
            }
            return Some(constructed);
        }

        // User sum-type payload-variant construction: emit the qualified, struct-style
        // form `Enum::Variant { field: value, ... }` to match the lowered enum (whose
        // payload variants use named fields). Nullary variants emit `Enum::Variant`.
        if let Some(sum_name) = self.find_sum_type_for_variant(ctor_name) {
            let declared_fields = self
                .program
                .items
                .iter()
                .find_map(|item| {
                    if let Item::SumType(sum) = item {
                        sum.variants
                            .iter()
                            .find(|variant| variant.name == ctor_name)
                            .map(|variant| variant.fields.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            if declared_fields.is_empty() {
                return Some(format!(
                    "{}::{}",
                    rust_ident(&sum_name),
                    rust_ident(ctor_name)
                ));
            }

            let field_names = declared_fields
                .iter()
                .map(|field| field.name.as_str())
                .collect::<Vec<_>>();
            let field_has_default = declared_fields
                .iter()
                .map(|field| field.default.is_some())
                .collect::<Vec<_>>();
            let field_allows_shorthand = vec![true; declared_fields.len()];
            let argument_names = args
                .iter()
                .map(|argument| argument.name.as_deref())
                .collect::<Vec<_>>();
            let argument_shorthand_names = args
                .iter()
                .map(|argument| match &argument.value {
                    Expr::Ident(name, _) => Some(name.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>();
            let binding = crate::call_binding::CallBinding::bind(
                &field_names,
                &field_has_default,
                &field_allows_shorthand,
                &argument_names,
                &argument_shorthand_names,
                0,
            );
            if !binding.is_complete() {
                unreachable_lowering("sum variant call binding", span);
            }

            let mut fields = Vec::new();
            for bound in binding.evaluation_order() {
                let field = &declared_fields[bound.parameter_index];
                let (value, is_default) = match bound.source {
                    crate::call_binding::BoundArgumentSource::Receiver => unreachable!(),
                    crate::call_binding::BoundArgumentSource::Explicit(source_index) => {
                        (&args[source_index].value, false)
                    }
                    crate::call_binding::BoundArgumentSource::Default => (
                        field
                            .default
                            .as_ref()
                            .expect("bound variant default should exist"),
                        true,
                    ),
                };
                let previous_lowering_default = self.lowering_default;
                self.lowering_default |= is_default;
                let value = self.lower_expr_for_expected_type(value, &field.ty);
                self.lowering_default = previous_lowering_default;
                fields.push(format!("{}: {value}", rust_ident(&field.name)));
            }
            return Some(format!(
                "{}::{} {{ {} }}",
                rust_ident(&sum_name),
                rust_ident(ctor_name),
                fields.join(", ")
            ));
        }

        if is_rust_enum_constructor(name) {
            let args = args
                .iter()
                .map(|arg| self.lower_expr(&arg.value))
                .collect::<Vec<_>>()
                .join(", ");
            return Some(format!("{}({args})", rust_ident(name)));
        }

        if let Some((target, fields)) = runtime_struct_constructor(name) {
            let mut lowered_fields = Vec::new();
            for (index, arg) in args.iter().enumerate() {
                let Some(field) = arg.name.as_deref().or_else(|| fields.get(index).copied()) else {
                    continue;
                };
                lowered_fields.push(format!(
                    "{}: {}",
                    rust_ident(field),
                    self.lower_owned_expr(&arg.value)
                ));
            }
            return Some(format!("{target} {{ {} }}", lowered_fields.join(", ")));
        }

        None
    }

    pub(in crate::rust_lower) fn lower_call_after_binding(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> String {
        if let Some(lowered) = self.lower_call_receiver(callee, args) {
            lowered
        } else {
            self.lower_call_dispatch(callee, args, span)
        }
    }

    /// Canonicalize named arguments without changing source evaluation order.
    /// The generated temporaries contain the exact Rust ABI values (`T`, `&T`, or
    /// `&mut T`), while the final call is laid out by declared parameter slot.
    pub(in crate::rust_lower) fn lower_bound_call(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> Option<String> {
        let (key, receiver_offset, stage_callee) = match callee {
            Callee::ReceiverCall {
                receiver, method, ..
            } => {
                let receiver_type = self.infer_expr_type(receiver)?;
                let namespace = self.receiver_call_namespace(&receiver_type, method);
                let key = external_boundary_function_key(&format!("{namespace}.{method}"));
                (
                    key,
                    1,
                    Callee::Qualified {
                        namespace,
                        name: method.clone(),
                    },
                )
            }
            _ => (external_boundary_callee_key(callee), 0, callee.clone()),
        };
        let params = self.function_param_types.get(&key)?.clone();
        let effects = self
            .function_param_effects
            .get(&key)
            .cloned()
            .unwrap_or_else(|| {
                params
                    .iter()
                    .map(|(name, _)| (name.clone(), None))
                    .collect()
            });
        let defaults = self
            .function_param_defaults
            .get(&key)
            .cloned()
            .unwrap_or_else(|| vec![None; params.len()]);
        let helpers = self
            .function_param_default_helpers
            .get(&key)
            .cloned()
            .unwrap_or_else(|| vec![None; params.len()]);

        let parameter_names = params
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        let parameter_has_default = defaults.iter().map(Option::is_some).collect::<Vec<_>>();
        let parameter_allows_shorthand = effects
            .iter()
            .map(|(_, effect)| *effect == Some(DataEffect::Read))
            .collect::<Vec<_>>();
        let argument_names = args
            .iter()
            .map(|argument| argument.name.as_deref())
            .collect::<Vec<_>>();
        let argument_shorthand_names = args
            .iter()
            .map(|argument| match &argument.value {
                Expr::Ident(name, _) => Some(name.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let binding = crate::call_binding::CallBinding::bind(
            &parameter_names,
            &parameter_has_default,
            &parameter_allows_shorthand,
            &argument_names,
            &argument_shorthand_names,
            receiver_offset,
        );
        if !binding.is_complete() {
            return None;
        }

        let explicit_slots = (0..args.len())
            .map(|source_index| {
                binding
                    .explicit(source_index)
                    .map(|arg| arg.parameter_index)
            })
            .collect::<Option<Vec<_>>>()?;
        let inserted_default = binding.defaults().next().is_some();

        let canonical_explicit = explicit_slots
            .iter()
            .copied()
            .eq(receiver_offset..receiver_offset + explicit_slots.len());
        if canonical_explicit && !inserted_default {
            return None;
        }

        let mut slots = vec![None::<CallArg>; params.len()];
        let mut evaluations = Vec::with_capacity(params.len());
        for bound in binding.evaluation_order() {
            let argument = match bound.source {
                crate::call_binding::BoundArgumentSource::Receiver => continue,
                crate::call_binding::BoundArgumentSource::Explicit(source_index) => {
                    let mut argument = args[source_index].clone();
                    if argument.name.is_none()
                        && matches!(
                            &argument.value,
                            Expr::Ident(name, _) if *name == params[bound.parameter_index].0
                        )
                    {
                        argument.name = Some(params[bound.parameter_index].0.clone());
                    }
                    argument
                }
                crate::call_binding::BoundArgumentSource::Default => {
                    let helper = helpers
                        .get(bound.parameter_index)
                        .and_then(Option::as_ref)?;
                    let default_value = Expr::Call {
                        callee: Callee::Name(helper.clone()),
                        args: Vec::new(),
                        span: span.clone(),
                    };
                    let effect = effects
                        .get(bound.parameter_index)
                        .and_then(|(_, effect)| *effect);
                    let value = effect.map_or(default_value.clone(), |effect| Expr::Effect {
                        effect,
                        value: Box::new(default_value),
                        span: span.clone(),
                    });
                    CallArg {
                        name: Some(params[bound.parameter_index].0.clone()),
                        value,
                        malformed: false,
                        span: span.clone(),
                    }
                }
            };
            slots[bound.parameter_index] = Some(argument.clone());
            evaluations.push((bound.parameter_index, argument));
        }

        // Defaults alone are already in the required evaluation and ABI order.
        // Avoid temporary blocks for the common trailing-default case.
        if canonical_explicit {
            let canonical = slots
                .into_iter()
                .skip(receiver_offset)
                .map(|slot| slot.expect("bound call slot should be complete"))
                .collect::<Vec<_>>();
            return Some(self.lower_call_after_binding(callee, &canonical, span));
        }

        let call_id = self.call_temp_counter;
        self.call_temp_counter += 1;
        let mut prelude = String::new();
        let mut call_callee = callee.clone();
        let mut canonical = vec![None::<CallArg>; params.len()];
        let mut temporary_names = Vec::new();

        // CallBinding places the receiver first in source evaluation order.
        // Stage its final Rust ABI value before reordered explicit arguments;
        // the synthetic receiver identifier is marked with the same effect so
        // `lower_call_receiver` consumes it without adding another borrow.
        if let Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } = callee
        {
            let receiver_type = self.infer_expr_type(receiver)?;
            let temp = format!("__rss_call_{call_id}_receiver");
            let lowered = match effect.unwrap_or(DataEffect::Read) {
                DataEffect::Mut
                    if let Expr::Ident(receiver_name, _) = receiver.as_ref()
                        && self.param_effects.get(receiver_name) == Some(&DataEffect::Mut) =>
                {
                    rust_ident(receiver_name)
                }
                DataEffect::Mut
                    if matches!(
                        receiver.as_ref(),
                        Expr::Ident(..) | Expr::Field { .. } | Expr::Index { .. }
                    ) =>
                {
                    format!("&mut {}", self.lower_assignment_target(receiver))
                }
                DataEffect::Mut => format!("&mut {}", self.lower_expr(receiver)),
                DataEffect::Read if is_copy_type_ref(&receiver_type) => self.lower_expr(receiver),
                DataEffect::Read
                    if (receiver_type.name == "List"
                        && matches!(receiver.as_ref(), Expr::ArrayLiteral { .. }))
                        || (receiver_type.name == "Map"
                            && matches!(receiver.as_ref(), Expr::MapLiteral { .. })) =>
                {
                    format!(
                        "&{}",
                        self.lower_expr_for_expected_type(receiver, &receiver_type)
                    )
                }
                DataEffect::Read => format!("&{}", self.lower_expr(receiver)),
                DataEffect::Take => self.lower_expr(receiver),
            };
            prelude.push_str(&format!("let {temp} = {lowered}; "));
            self.value_types.insert(temp.clone(), receiver_type);
            self.param_effects
                .insert(temp.clone(), effect.unwrap_or(DataEffect::Read));
            call_callee = Callee::ReceiverCall {
                receiver: Box::new(Expr::Ident(temp.clone(), span.clone())),
                method: method.clone(),
                effect: *effect,
            };
            temporary_names.push(temp);
        }

        for (parameter_index, arg) in evaluations {
            let temp = format!("__rss_call_{call_id}_arg_{parameter_index}");
            let lowered = self.lower_call_arg_for_callee(&stage_callee, &arg, parameter_index);
            prelude.push_str(&format!("let {temp} = {lowered}; "));

            let (_, ty) = &params[parameter_index];
            self.value_types.insert(temp.clone(), ty.clone());
            let effect = effects.get(parameter_index).and_then(|(_, effect)| *effect);
            if let Some(effect) = effect {
                self.param_effects.insert(temp.clone(), effect);
            }
            let ident = Expr::Ident(temp.clone(), arg.span.clone());
            let value = effect.map_or(ident.clone(), |effect| Expr::Effect {
                effect,
                value: Box::new(ident),
                span: arg.span.clone(),
            });
            canonical[parameter_index] = Some(CallArg {
                name: Some(params[parameter_index].0.clone()),
                value,
                malformed: false,
                span: arg.span,
            });
            temporary_names.push(temp);
        }

        let canonical = canonical
            .into_iter()
            .skip(receiver_offset)
            .map(|arg| arg.expect("bound call slot should be complete"))
            .collect::<Vec<_>>();
        let call = self.lower_call_after_binding(&call_callee, &canonical, span);
        for name in temporary_names {
            self.value_types.remove(&name);
            self.param_effects.remove(&name);
        }
        Some(format!("{{ {prelude}{call} }}"))
    }

    /// Receiver-call shorthand (`receiver.method(..)`): resolve the receiver's
    /// type, pick the protocol/facade/native namespace, apply the receiver's
    /// borrow/effect, and emit the qualified call with the receiver as the first
    /// argument. Returns `None` if the callee is not a receiver call.
    pub(in crate::rust_lower) fn lower_call_receiver(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
    ) -> Option<String> {
        if let Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } = callee
        {
            let receiver_type = self.infer_expr_type(receiver);
            let receiver_type_name = receiver_type
                .as_ref()
                .map(type_ref_display_name)
                .unwrap_or_else(|| "Unknown".to_string());
            let receiver_type_root = type_root_name(&receiver_type_name).to_string();
            if method == "clone"
                && (self.type_derives_clone(&receiver_type_root)
                    || Self::is_builtin_clone_value(&receiver_type_root))
            {
                // Explicit `.clone()` -> Rust's `Clone`: a user struct/sum that derives
                // `Clone`, or a builtin value type that lowers to a `Clone` Rust type but has
                // no dedicated clone intrinsic (`List`/`Map`/`Set`/…). Without the builtin
                // case these fell through to a dangling `List_clone`-style call (the checker
                // accepts `.clone()` via the `Clone` protocol, so it never errored at the
                // front end — E0425 at the Rust backend). `.clone()` borrows its receiver, so
                // a place behind a `&` (read param / field of one) works without moving. Types
                // that don't derive Clone are rejected earlier (RS0206). `String`/`Json` keep
                // their own clone intrinsics and are handled below.
                return Some(format!("{}.clone()", self.lower_expr(receiver)));
            }
            let receiver_rust_type = receiver_type
                .as_ref()
                .map(|ty| self.lower_type_ref(ty, ManagedPosition::Bare))
                .unwrap_or_else(|| rust_ident(&receiver_type_name));
            // Resolve namespace: check generic protocol bounds first,
            // then concrete protocol impls, then fall back to the type
            // name itself. This mirrors HIR receiver-call resolution for
            // the non-ambiguous cases that are allowed to reach lowering.
            let namespace = self
                .generic_protocol_bounds
                .get(&receiver_type_name)
                .cloned()
                .or_else(|| dyn_protocol_name(&receiver_type_name).map(str::to_string))
                .or_else(|| self.protocol_impl_namespace(&receiver_type_root, method))
                .or_else(|| {
                    receiver_facade_namespace(&receiver_type_root, method).map(str::to_string)
                })
                .unwrap_or(receiver_type_root);
            let is_protocol = self.protocol_names.contains(&namespace);
            let lowered_receiver = match (*effect).unwrap_or(DataEffect::Read) {
                DataEffect::Mut
                    if let Expr::Ident(receiver_name, _) = receiver.as_ref()
                        && self.param_effects.get(receiver_name) == Some(&DataEffect::Mut) =>
                {
                    rust_ident(receiver_name)
                }
                DataEffect::Mut
                    if matches!(
                        receiver.as_ref(),
                        Expr::Ident(..) | Expr::Field { .. } | Expr::Index { .. }
                    ) =>
                {
                    format!("&mut {}", self.lower_assignment_target(receiver))
                }
                DataEffect::Mut => format!("&mut {}", self.lower_expr(receiver)),
                DataEffect::Read
                    if let Expr::Ident(receiver_name, _) = receiver.as_ref()
                        && self.param_effects.get(receiver_name) == Some(&DataEffect::Read) =>
                {
                    rust_ident(receiver_name)
                }
                DataEffect::Read if receiver_type.as_ref().is_some_and(is_copy_type_ref) => {
                    self.lower_expr(receiver)
                }
                DataEffect::Read
                    if receiver_type.as_ref().is_some_and(|ty| {
                        (ty.name == "List"
                            && matches!(receiver.as_ref(), Expr::ArrayLiteral { .. }))
                            || (ty.name == "Map"
                                && matches!(receiver.as_ref(), Expr::MapLiteral { .. }))
                    }) =>
                {
                    let receiver_type = receiver_type.as_ref().expect("checked above");
                    format!(
                        "&{}",
                        self.lower_expr_for_expected_type(receiver, receiver_type)
                    )
                }
                DataEffect::Read => format!("&{}", self.lower_expr(receiver)),
                DataEffect::Take => self.lower_expr(receiver),
            };
            let lowered_args = args
                .iter()
                .enumerate()
                .map(|(index, arg)| {
                    if runtime_collection_intrinsic_borrows_arg(
                        type_root_name(&namespace),
                        type_root_name(method),
                        arg.name.as_deref(),
                        index + 1,
                    ) {
                        let value = match &arg.value {
                            Expr::Effect { value, .. } => value.as_ref(),
                            value => value,
                        };
                        if let Expr::Ident(name, _) = value
                            && self.read_view_bindings.contains(name)
                            && self
                                .value_types
                                .get(name)
                                .is_some_and(|ty| !is_copy_type_ref(ty))
                        {
                            return rust_value_ident(name);
                        }
                        return format!("&({})", self.lower_expr(value));
                    }
                    if let Some(expected) = self.receiver_call_expected_arg_type(
                        &namespace,
                        method,
                        receiver_type.as_ref(),
                        arg.name.as_deref(),
                        index,
                    ) {
                        let expected = self.canonical_type_ref(&expected);
                        if let Some(effect) =
                            self.receiver_call_expected_arg_effect(&namespace, method, index)
                        {
                            if effect == DataEffect::Read {
                                let value = match &arg.value {
                                    Expr::Effect {
                                        effect: DataEffect::Read,
                                        value,
                                        ..
                                    } => value.as_ref(),
                                    value => value,
                                };
                                let lowered = self.lower_receiver_positional_arg(value, &expected);
                                let abi_type = self
                                    .receiver_call_declared_arg_type(
                                        &namespace,
                                        method,
                                        arg.name.as_deref(),
                                        index,
                                    )
                                    .unwrap_or_else(|| expected.clone());
                                if Self::read_effect_lowers_by_value(&abi_type) {
                                    return lowered;
                                }
                                if let Expr::Ident(name, _) = value
                                    && self.param_effects.get(name) == Some(&DataEffect::Read)
                                    && !Self::read_effect_lowers_by_value(&expected)
                                {
                                    return lowered;
                                }
                                return format!("&({lowered})");
                            }
                            let effective_value = match &arg.value {
                                Expr::Effect { .. } => arg.value.clone(),
                                value => Expr::Effect {
                                    effect,
                                    value: Box::new(value.clone()),
                                    span: arg.span.clone(),
                                },
                            };
                            return self
                                .lower_call_arg_for_expected_type(&effective_value, &expected);
                        }
                        if arg.name.is_none() {
                            if expected.name == "Fn" {
                                return self.lower_expr_for_expected_type(&arg.value, &expected);
                            }
                            let lowered = self.lower_receiver_positional_arg(&arg.value, &expected);
                            if is_copy_type_ref(&expected) {
                                return lowered;
                            }
                            return format!("&{lowered}");
                        }
                        return self.lower_call_arg_for_expected_type(&arg.value, &expected);
                    }
                    self.lower_expr(&arg.value)
                })
                .collect::<Vec<_>>()
                .join(", ");
            let all_args = if lowered_args.is_empty() {
                lowered_receiver
            } else {
                format!("{lowered_receiver}, {lowered_args}")
            };
            let qualified_key = external_boundary_function_key(&format!("{namespace}.{method}"));
            if let Some(native_target) = self.external_bindings.get(&qualified_key).cloned() {
                return Some(format!("{native_target}({all_args})"));
            }
            let callee_str = if is_protocol {
                format!(
                    "<{} as {}>::{}",
                    receiver_rust_type,
                    rust_ident(&namespace),
                    rust_ident(method)
                )
            } else {
                lower_callee(&Callee::Qualified {
                    namespace: namespace.clone(),
                    name: method.clone(),
                })
            };
            return Some(format!("{callee_str}({all_args})"));
        }

        None
    }

    /// Generic / fallthrough call dispatch: string-concat and weak intrinsics,
    /// native-bound free functions, external_binding-from-protocol, protocol callees, and the default
    /// `callee(args...)` form (including trailing defaulted-parameter fill-in).
    pub(in crate::rust_lower) fn lower_call_dispatch(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        _span: &Span,
    ) -> String {
        if is_string_concat_callee(callee) {
            return lower_string_concat_call(self, args);
        }
        if is_weak_from_callee(callee) {
            return lower_weak_from_call(self, args);
        }
        if is_weak_upgrade_callee(callee) {
            return lower_weak_upgrade_call(self, args);
        }
        if let Some(native_target) = self
            .external_bindings
            .get(&external_boundary_callee_key(callee))
            .cloned()
        {
            let args = args
                .iter()
                .map(|arg| self.lower_expr(&arg.value))
                .collect::<Vec<_>>()
                .join(", ");
            return format!("{native_target}({args})");
        }
        if let Some(protocol) = dyn_from_protocol(callee) {
            return self.lower_dyn_from_call(protocol, args);
        }
        if self.is_protocol_callee(callee) {
            let lowered_callee = lower_protocol_callee(callee);
            let args = args
                .iter()
                .enumerate()
                .map(|(index, arg)| self.lower_call_arg_for_callee(callee, arg, index))
                .collect::<Vec<_>>()
                .join(", ");
            return format!("{lowered_callee}({args})");
        }
        let lowered_callee = lower_callee(callee);
        let args = args
            .iter()
            .enumerate()
            .map(|(index, arg)| self.lower_call_arg_for_callee(callee, arg, index))
            .collect::<Vec<_>>();
        let args = args.join(", ");
        format!("{lowered_callee}({args})")
    }
}
