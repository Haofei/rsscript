//! Owned, borrowed, retained, and closure argument lowering.

use super::*;

impl<'a> RustLowerer<'a> {
    pub(in crate::rust_lower) fn lower_binary_operand(
        &mut self,
        expr: &Expr,
        parent: BinaryOp,
        is_right: bool,
    ) -> String {
        let lowered = self.lower_expr(expr);
        let Expr::Binary { op: child, .. } = expr else {
            return lowered;
        };
        let parent_precedence = rust_binary_precedence(parent);
        let child_precedence = rust_binary_precedence(*child);
        let chained_comparison =
            rust_binary_is_comparison(parent) && rust_binary_is_comparison(*child);
        if child_precedence < parent_precedence
            || (is_right && child_precedence == parent_precedence)
            || chained_comparison
        {
            format!("({lowered})")
        } else {
            lowered
        }
    }

    pub(in crate::rust_lower) fn lower_owned_expr(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Effect {
                effect: DataEffect::Read,
                value,
                span,
            }
            | Expr::Effect {
                effect: DataEffect::Mut,
                value,
                span,
            } => {
                if self.expr_lowers_to_managed_non_class_handle(value) {
                    format!(
                        "rsscript_runtime::unwrap_runtime({}.try_read_at({})).clone()",
                        self.lower_expr(value),
                        lower_source_span(span)
                    )
                } else {
                    format!("{}.clone()", self.lower_expr(value))
                }
            }
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                ..
            } => self.lower_expr(value),
            _ => self.lower_expr(expr),
        }
    }

    pub(in crate::rust_lower) fn lower_expr_for_expected_type(
        &mut self,
        expr: &Expr,
        expected: &TypeRef,
    ) -> String {
        let expected = self.canonical_type_ref(expected);
        let expected = &expected;
        if expected.name == "Fn"
            && let Expr::Closure { params, body, .. } = expr
        {
            // Every caller of this entry point places the closure into a
            // STORABLE slot (struct/sum field init, return value, collection or
            // Option/Result payload). A storable `owned Fn` slot lowers to
            // `Rc<dyn Fn>`, so the closure literal is wrapped in `Rc::new`. The
            // sole non-storable (direct parameter) case is handled separately in
            // `lower_call_arg_for_expected_type`.
            return self
                .lower_closure_for_expected_fn(params, body, expected, /* storable */ true);
        }
        if let Expr::Call {
            callee: Callee::Name(name),
            args,
            ..
        } = expr
        {
            if expected.name == "Result" && expected.args.len() == 2 {
                match (name.as_str(), args.as_slice()) {
                    ("Ok", [arg]) => {
                        let payload =
                            self.lower_owned_expr_for_expected_type(&arg.value, &expected.args[0]);
                        return format!("Ok({payload})");
                    }
                    ("Err", [arg]) => {
                        let payload =
                            self.lower_owned_expr_for_expected_type(&arg.value, &expected.args[1]);
                        return format!("Err({payload})");
                    }
                    ("Ok", []) if expected.args[0].name == "Unit" => return "Ok(())".to_string(),
                    ("Err", []) if expected.args[1].name == "Unit" => return "Err(())".to_string(),
                    _ => {}
                }
            }
            if expected.name == "Option" && expected.args.len() == 1 {
                match (name.as_str(), args.as_slice()) {
                    ("Some", [arg]) => {
                        let payload =
                            self.lower_owned_expr_for_expected_type(&arg.value, &expected.args[0]);
                        return format!("Some({payload})");
                    }
                    ("Some", []) if expected.args[0].name == "Unit" => {
                        return "Some(())".to_string();
                    }
                    ("None", []) => return "None".to_string(),
                    _ => {}
                }
            }
        }
        if expected.name == "JsonValue" && expr_is_json_literal(expr) {
            let lowered = self.lower_json_value(expr);
            return format!("rsscript_runtime::json_value(&{lowered})");
        }
        if expected.name == "Map" && matches!(expr, Expr::MapLiteral { .. }) {
            return self.lower_map_literal(expr, expected);
        }
        if expected.name == "List" && matches!(expr, Expr::ArrayLiteral { .. }) {
            return self.lower_list_literal(expr, expected);
        }
        if expected.name == "String"
            && expected.args.is_empty()
            && let Expr::Ident(name, _) = expr
            && self.param_effects.get(name) == Some(&DataEffect::Read)
        {
            return format!("{}.clone()", rust_value_ident(name));
        }
        if let Expr::Effect {
            effect: DataEffect::Read,
            value,
            ..
        } = expr
        {
            let lowered = self.lower_expr(value);
            if is_copy_type_ref(expected) {
                return lowered;
            }
            return format!("{lowered}.clone()");
        }
        // Integer literal in a typed slot (e.g. a sized `Int32` param/field):
        // emit the suffix matching the expected type so it lowers as the right
        // Rust integer rather than `lower_expr`'s default `i64`.
        if let Expr::Number(value, _) = expr
            && !value.contains('.')
            && let Some(suffix) = rust_int_literal_suffix(&expected.name)
        {
            return format!("{value}{suffix}");
        }
        self.lower_expr(expr)
    }

    pub(in crate::rust_lower) fn lower_closure_for_expected_fn(
        &mut self,
        params: &[String],
        body: &Block,
        expected: &TypeRef,
        storable: bool,
    ) -> String {
        let expected = self.canonical_type_ref(expected);
        let expected = &expected;
        // A storable `owned Fn` slot is `Rc<dyn Fn(..)>`; wrap the closure value
        // in `Rc::new(..)` so it matches that type and stays `Clone`/shareable.
        // The closure itself always `move`s its captures (owned/Copy only, per
        // the checker's capture-soundness rule), so it can outlive the builder.
        let wrap_rc = storable && expected.is_owned;
        let inner = self.lower_closure_for_expected_fn_inner(params, body, expected);
        if wrap_rc {
            format!("std::rc::Rc::new({inner})")
        } else {
            inner
        }
    }

    pub(in crate::rust_lower) fn lower_closure_for_expected_fn_inner(
        &mut self,
        params: &[String],
        body: &Block,
        expected: &TypeRef,
    ) -> String {
        let previous_value_types = self.value_types.clone();
        let previous_param_effects = self.param_effects.clone();
        let previous_read_view_bindings = self.read_view_bindings.clone();
        for (param, ty) in params.iter().zip(expected.fn_params.iter()) {
            self.value_types.insert(param.clone(), ty.clone());
        }
        // Register each closure parameter's data effect so the body lowers
        // exactly like a regular function with the same parameter effects: a
        // `read T` parameter is a borrowed `&T` (reads `.clone()` out of it where
        // needed), a `mut T` parameter is an exclusive `&mut T` whose field
        // assignments propagate to the caller, and a `take`/defaulted parameter is
        // owned by value. This is what lets a stored `mut Ctx` rule mutate `ctx`.
        for (index, param) in params.iter().enumerate() {
            if let Some(effect) = expected.effective_fn_param_effect(index) {
                self.param_effects.insert(param.clone(), effect);
                if effect == DataEffect::Read {
                    self.read_view_bindings.insert(param.clone());
                }
            }
        }
        // The closure's parameter list must match the stored
        // `Rc<dyn Fn(&UOp, &mut Ctx) -> ..>` signature: annotate each parameter
        // with the same ref it would carry as a regular function parameter
        // (`read T` -> `&T`, `mut T` -> `&mut T`, owned otherwise), mirroring
        // `lower_param`'s effect-to-ref mapping.
        let lowered_params = params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let name = rust_ident(param);
                let Some(ty) = expected
                    .fn_params
                    .get(index)
                    .filter(|ty| self.type_ref_is_concrete_for_annotation(ty))
                else {
                    return name;
                };
                let bare = self.lower_type_ref(ty, ManagedPosition::Param);
                let effect = expected.effective_fn_param_effect(index);
                // Match the stored `Rc<dyn Fn(..)>` parameter ABI exactly (see
                // `lower_type_ref` for `Fn`): `read T` -> `&T`, `mut T` -> `&mut T`,
                // owned otherwise. Kept uniform (no by-value-`Copy` shortcut) so
                // the closure literal, its stored type, and every call site agree.
                let rust_ty = match effect {
                    Some(DataEffect::Read) => format!("&{bare}"),
                    Some(DataEffect::Mut) => format!("&mut {bare}"),
                    Some(DataEffect::Take) | None => bare,
                };
                format!("{name}: {rust_ty}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let previous_return_type = self.current_return_type.take();
        let closure_prefix = if expected.is_owned { "move " } else { "" };
        if let [Stmt::Expr(value)] = body.statements.as_slice() {
            let lowered = format!(
                "{closure_prefix}|{lowered_params}| {}",
                self.lower_expr(value)
            );
            self.current_return_type = previous_return_type;
            self.value_types = previous_value_types;
            self.param_effects = previous_param_effects;
            self.read_view_bindings = previous_read_view_bindings;
            return lowered;
        }
        let mut out = String::new();
        out.push_str(&format!("{closure_prefix}|{lowered_params}| {{\n"));
        self.lower_block(body, &mut out, 1);
        out.push('}');
        self.current_return_type = previous_return_type;
        self.value_types = previous_value_types;
        self.param_effects = previous_param_effects;
        self.read_view_bindings = previous_read_view_bindings;
        out
    }

    pub(in crate::rust_lower) fn lower_json_decode_call(
        &mut self,
        callee: &Callee,
        args: &[CallArg],
        span: &Span,
    ) -> String {
        let Some(arg) = args
            .iter()
            .find(|arg| {
                arg.name
                    .as_deref()
                    .is_some_and(|name| name == "value" || name == "text")
                    || arg.name.is_none()
            })
            .or_else(|| args.first())
        else {
            unreachable_lowering("Json.decode call argument", span)
        };
        let type_suffix = json_decode_type_arg(callee)
            .map(|name| {
                let ty = type_ref_from_display(name, span);
                format!("::<{}>", self.lower_type_ref(&ty, ManagedPosition::Nested))
            })
            .unwrap_or_default();
        if is_json_decode_text_callee(callee) {
            let text = self.lower_decode_read_arg(&arg.value);
            format!("rsscript_runtime::json_decode_text{type_suffix}(&{text})")
        } else {
            let json_ty = simple_type_ref("JsonValue", span);
            let value = match &arg.value {
                Expr::Effect {
                    effect: DataEffect::Read,
                    value,
                    ..
                } => self.lower_expr_for_expected_type(value, &json_ty),
                _ => self.lower_expr_for_expected_type(&arg.value, &json_ty),
            };
            format!("rsscript_runtime::json_decode_value{type_suffix}(&{value})")
        }
    }

    pub(in crate::rust_lower) fn lower_decode_read_arg(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Effect {
                effect: DataEffect::Read,
                value,
                ..
            } => self.lower_expr(value),
            _ => self.lower_expr(expr),
        }
    }

    pub(in crate::rust_lower) fn lower_assignment_target(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Field { base, name, span } => {
                let base_is_managed_class = self
                    .infer_expr_type(base)
                    .is_some_and(|ty| self.is_class_type(&ty));
                if base_is_managed_class || self.expr_lowers_to_managed_non_class_handle(base) {
                    format!(
                        "rsscript_runtime::unwrap_runtime({}.try_write_at({})).{}",
                        self.lower_expr(base),
                        lower_source_span(span),
                        rust_ident(name)
                    )
                } else {
                    format!(
                        "{}.{}",
                        self.lower_assignment_target(base),
                        rust_ident(name)
                    )
                }
            }
            Expr::Index { base, index, .. } => {
                format!(
                    "{}[rsscript_runtime::checked_list_index({})]",
                    self.lower_assignment_target(base),
                    self.lower_expr(index)
                )
            }
            Expr::Ident(..) => self.lower_expr(expr),
            _ => self.lower_expr(expr),
        }
    }

    pub(in crate::rust_lower) fn lower_call_arg_for_callee(
        &mut self,
        callee: &Callee,
        arg: &CallArg,
        index: usize,
    ) -> String {
        // Calling a first-class closure value (`let f = r.fxn; f(read u, mut ctx)`):
        // the callee is `Rc<dyn Fn(P0, P1, ..) -> R>` whose parameters carry the
        // stored `Fn` type's per-parameter data effects. Pass each argument to
        // match that effect's Rust ABI: `read T` -> `&T` (by value for `Copy`),
        // `mut T` -> `&mut T` (the closure may mutate it; the borrow is exclusive
        // for the call), and a `take`/defaulted parameter by value (a `read` of a
        // non-`Copy` value becomes an owned `.clone()`). This mirrors `lower_param`
        // and the stored `Rc<dyn Fn(&UOp, &mut Ctx)>` signature.
        if self.callee_is_closure_value(callee) {
            let effect = self.closure_value_param_effect(callee, index);
            let inner = match &arg.value {
                Expr::Effect { value, .. } => value.as_ref(),
                other => other,
            };
            return match effect {
                Some(DataEffect::Read) => format!("&{}", self.lower_expr(inner)),
                Some(DataEffect::Mut) => {
                    // When the argument is itself a `mut` parameter it is already
                    // a `&mut T`; reborrow it as the closure's `&mut T` argument by
                    // passing the binding directly (`f(read u, mut ctx)` where
                    // `ctx: &mut Ctx`), exactly like a `mut`-arg to a regular
                    // function. Otherwise take `&mut` of the lowered place.
                    if let Expr::Ident(name, _) = inner
                        && self.param_effects.get(name) == Some(&DataEffect::Mut)
                    {
                        rust_value_ident(name)
                    } else if matches!(inner, Expr::Field { .. } | Expr::Index { .. }) {
                        // A `mut` place (`mut cache.items`) must borrow the live
                        // storage via the write path, not a read-view clone (which
                        // would silently drop the closure's mutation).
                        format!("&mut {}", self.lower_assignment_target(inner))
                    } else {
                        format!("&mut {}", self.lower_expr(inner))
                    }
                }
                // A defaulted/`take` parameter is passed by value; a `read`
                // call-site marker without a declared effect (older value-model
                // `Fn` types) still lowers by value via `lower_owned_expr`.
                _ => self.lower_owned_expr(&arg.value),
            };
        }
        if runtime_intrinsic_wants_managed_handle_arg(callee, arg.name.as_deref())
            && let Expr::Effect { effect, value, .. } = &arg.value
            && self.expr_lowers_to_managed_handle(value)
        {
            return self.lower_managed_handle_effect_arg(*effect, value);
        }
        if self.call_arg_is_retained(callee, arg, index)
            && runtime_intrinsic_target(callee).is_none()
            && let Expr::Effect { effect, value, .. } = &arg.value
            && self.expr_lowers_to_managed_handle(value)
        {
            return self.lower_managed_handle_effect_arg(*effect, value);
        }
        if runtime_intrinsic_borrows_arg(callee, arg.name.as_deref(), index) {
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
        // Closure `read Int`/`Bool` parameters are represented as references in
        // Rust. When static signature lookup is unavailable (for example after
        // package-level function merging), still pass the scalar value rather
        // than leaking `&Int` into a by-value helper call.
        if let Expr::Ident(name, _) = &arg.value
            && self.read_view_bindings.contains(name)
            && self.value_types.get(name).is_some_and(is_copy_type_ref)
        {
            return format!("*{}", rust_value_ident(name));
        }
        // A `read`-effect argument passed to a user function's *by-value* `Copy`
        // parameter (declared with no `read`/`mut` effect, so it lowers to e.g.
        // `f64`, not `&f64`) is passed by value, not borrowed. Without this a
        // `read`-float argument was borrowed against a by-value `f64` parameter —
        // a VM↔compiler build gap. Receiver/intrinsic calls aren't `Callee::Name`,
        // so their `&T` ABI (`char_is_alpha(&char)`, …) is unaffected.
        if let Callee::Name(_) = callee
            && let Expr::Effect {
                effect: DataEffect::Read,
                value,
                ..
            } = &arg.value
            && let Some(expected) = self.expected_call_arg_type(callee, arg, index)
            && is_copy_type_ref(&expected)
            && !matches!(
                self.expected_call_arg_effect(callee, arg, index),
                Some(DataEffect::Read | DataEffect::Mut)
            )
        {
            return self.lower_expr_for_expected_type(value, &expected);
        }
        if let Some(effect) = self.expected_call_arg_effect(callee, arg, index)
            && let Some(expected) = self.expected_call_arg_type(callee, arg, index)
        {
            let effective_value = match &arg.value {
                Expr::Effect { .. } => arg.value.clone(),
                value => Expr::Effect {
                    effect,
                    value: Box::new(value.clone()),
                    span: arg.span.clone(),
                },
            };
            return self.lower_call_arg_for_expected_type(&effective_value, &expected);
        }
        if let Some(expected) = self.expected_call_arg_type(callee, arg, index) {
            return self.lower_call_arg_for_expected_type(&arg.value, &expected);
        }
        self.lower_expr(&arg.value)
    }

    pub(in crate::rust_lower) fn lower_call_arg_for_expected_type(
        &mut self,
        value: &Expr,
        expected: &TypeRef,
    ) -> String {
        let expected = self.canonical_type_ref(expected);
        let expected = &expected;
        if is_copy_type_ref(expected)
            && let Expr::Ident(name, _) = value
            && self.read_view_bindings.contains(name)
        {
            return format!("*{}", rust_value_ident(name));
        }
        if expected.name == "Fn" {
            if let Expr::Closure { params, body, .. } = value {
                // A closure passed to a function PARAMETER typed `owned Fn` (or
                // `noescape Fn`) is consumed in-place: the parameter lowers to
                // `impl FnMut`, NOT a stored `Rc<dyn Fn>`. So this path lowers
                // the closure WITHOUT the `Rc::new` storable wrapper.
                return self.lower_closure_for_expected_fn(
                    params, body, expected, /* storable */ false,
                );
            }
            return self.lower_expr_for_expected_type(value, expected);
        }
        match value {
            Expr::Call {
                callee:
                    Callee::ReceiverCall {
                        effect: Some(DataEffect::Read) | None,
                        method,
                        ..
                    },
                ..
            } if method != "clone" && !is_copy_type_ref(expected) => {
                // `.clone()` yields an owned value (it must not be re-borrowed when passed to a
                // by-value param); every other `read` receiver-call returns a borrow-friendly result.
                format!("&{}", self.lower_expr_for_expected_type(value, expected))
            }
            Expr::Effect {
                effect: DataEffect::Read,
                value,
                span,
                ..
            } => {
                if Self::read_effect_lowers_by_value(expected) {
                    self.lower_expr_for_expected_type(value, expected)
                } else if self.expr_lowers_to_managed_non_class_handle(value) {
                    format!(
                        "&*rsscript_runtime::unwrap_runtime({}.try_read_at({}))",
                        self.lower_expr(value),
                        lower_source_span(span)
                    )
                } else if expr_is_json_literal(value)
                    || (expected.name == "Map" && matches!(value.as_ref(), Expr::MapLiteral { .. }))
                    || (expected.name == "List"
                        && matches!(value.as_ref(), Expr::ArrayLiteral { .. }))
                {
                    format!("&{}", self.lower_expr_for_expected_type(value, expected))
                } else if let Expr::Ident(name, _) = value.as_ref()
                    && self.param_effects.get(name) == Some(&DataEffect::Read)
                {
                    // A `read`-PARAM already lowers to `&T`; passing it on as a `read`
                    // argument must NOT add another `&` (that produced `&&T` and an
                    // ill-typed call, e.g. `list_push(&mut v, &&node)`). Mirrors the
                    // `mut`-param case just below.
                    rust_value_ident(name)
                } else {
                    format!("&({})", self.lower_expr(value))
                }
            }
            Expr::Effect {
                effect: DataEffect::Mut,
                value,
                span,
                ..
            } => {
                if let Expr::Ident(name, _) = value.as_ref()
                    && self.param_effects.get(name) == Some(&DataEffect::Mut)
                {
                    rust_value_ident(name)
                } else if self.is_class_type(expected) {
                    format!("&{}", self.lower_expr_for_expected_type(value, expected))
                } else if self.expr_lowers_to_managed_non_class_handle(value) {
                    format!(
                        "&mut *rsscript_runtime::unwrap_runtime({}.try_write_at({}))",
                        self.lower_expr(value),
                        lower_source_span(span)
                    )
                } else if matches!(value.as_ref(), Expr::Field { .. } | Expr::Index { .. }) {
                    // A `mut` place argument (`mut cache.items`, `mut xs[i]`) must
                    // borrow the LIVE storage, not a read-view clone: the default
                    // `lower_expr` path lowers a managed-class field through
                    // `try_read_at(..).field.clone()`, so the callee mutates a
                    // throwaway copy and the write is silently lost (VM mutates,
                    // AOT does not). `lower_assignment_target` routes the place
                    // through `try_write_at` (no clone).
                    format!("&mut {}", self.lower_assignment_target(value))
                } else {
                    format!(
                        "&mut {}",
                        self.lower_expr_for_expected_type(value, expected)
                    )
                }
            }
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                ..
            } => self.lower_expr_for_expected_type(value, expected),
            _ => self.lower_expr_for_expected_type(value, expected),
        }
    }

    pub(in crate::rust_lower) fn expected_call_arg_type(
        &self,
        callee: &Callee,
        arg: &CallArg,
        index: usize,
    ) -> Option<TypeRef> {
        let key = native_boundary_callee_key(callee);
        let params = self.function_param_types.get(&key)?;
        if let Some(name) = arg.name.as_deref() {
            params
                .iter()
                .find(|(param_name, _)| param_name == name)
                .map(|(_, ty)| ty.clone())
        } else {
            params.get(index).map(|(_, ty)| ty.clone())
        }
    }

    pub(in crate::rust_lower) fn expected_call_arg_effect(
        &self,
        callee: &Callee,
        arg: &CallArg,
        index: usize,
    ) -> Option<DataEffect> {
        let key = native_boundary_callee_key(callee);
        let params = self.function_param_effects.get(&key)?;
        if let Some(name) = arg.name.as_deref() {
            params
                .iter()
                .find(|(param_name, _)| param_name == name)
                .and_then(|(_, effect)| *effect)
        } else {
            params.get(index).and_then(|(_, effect)| *effect)
        }
    }
}
