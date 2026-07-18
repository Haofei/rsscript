use super::*;

impl Analyzer<'_> {
    pub(super) fn invalid_resource_pool_type_diagnostic(
        &mut self,
        summary: impl Into<String>,
        span: crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_RESOURCE_POOL_TYPE,
                summary,
                span,
                "invalid ResourcePool type",
            )
            .with_cause("`ResourcePool<T>` is the privileged container for long-lived resource values, so `T` must be a resource.")
            .with_fix(
                "use_resource_type",
                "Use a resource type argument or a non-resource container for ordinary values.",
                "manual",
            ),
        );
    }

    pub(super) fn fd_surface_diagnostic(
        &mut self,
        span: crate::diagnostic::Span,
        summary: impl Into<String>,
        fix: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::FD_OUTSIDE_INTERNAL_BOUNDARY,
                summary,
                span,
                "Fd outside native/resource internals",
            )
            .with_cause(
                "`Fd` is a trusted native/resource-internal descriptor handle, not an ordinary RSScript value type.",
            )
            .with_fix("use_resource_wrapper", fix, "manual"),
        );
    }

    pub(super) fn unknown_type_diagnostic(&mut self, ty: &TypeRef) {
        self.unknown_type_name_diagnostic(&type_ref_name(ty), &ty.span);
    }

    pub(crate) fn unknown_type_name_diagnostic(
        &mut self,
        name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_TYPE,
                format!("unknown type `{name}`."),
                span.clone(),
                "unknown type",
            )
            .with_cause("RSScript type checking must resolve source-level types before Rust lowering.")
            .with_fix(
                "declare_or_import_type",
                format!(
                    "Declare `{}`, import an `.rssi` contract that declares it, or use a known core/runtime type.",
                    name
                ),
                "manual",
            ),
        );
    }

    pub(crate) fn unknown_protocol_diagnostic(
        &mut self,
        name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_PROTOCOL,
                format!("unknown protocol `{name}`."),
                span.clone(),
                "unknown protocol",
            )
            .with_cause(
                "Protocol bounds and implementations must name an explicit `protocol` declaration.",
            )
            .with_fix(
                "declare_protocol",
                format!("Declare `protocol {name} {{ ... }}` or use a declared protocol name."),
                "manual",
            ),
        );
    }

    pub(crate) fn protocol_impl_mismatch_diagnostic(
        &mut self,
        protocol: &str,
        type_name: &str,
        method: &str,
        span: &crate::diagnostic::Span,
        label: impl Into<String>,
        cause: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::PACKAGE_INTERFACE_MISMATCH,
                format!("`{type_name}` does not satisfy protocol `{protocol}` method `{method}`."),
                span.clone(),
                label,
            )
            .with_cause(cause)
            .with_fix(
                "fix_protocol_impl_mapping",
                "Update the protocol impl mapping or concrete function signature to match the protocol contract exactly.",
                "manual",
            ),
        );
    }

    pub(super) fn unknown_field_diagnostic(
        &mut self,
        field_name: &str,
        base_type: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_FIELD,
                format!("unknown field `{field_name}` on type `{base_type}`."),
                span.clone(),
                "unknown field",
            )
            .with_cause("RSScript field accesses must resolve before Rust lowering.")
            .with_fix(
                "use_declared_field",
                format!("Use a field declared on `{base_type}` or update the type declaration."),
                "manual",
            ),
        );
    }

    pub(super) fn unknown_binding_diagnostic(
        &mut self,
        name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_BINDING,
                format!("unknown value binding `{name}`."),
                span.clone(),
                "unknown binding",
            )
            .with_cause("RSScript values must resolve before Rust lowering.")
            .with_fix(
                "declare_binding",
                format!("Declare `{name}` before using it or pass it as a parameter."),
                "manual",
            ),
        );
    }

    pub(super) fn resource_generic_argument_diagnostic(
        &mut self,
        generic_name: &str,
        resource_name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::RESOURCE_GENERIC_ARGUMENT,
                format!(
                    "generic type `{generic_name}` cannot be instantiated with resource `{resource_name}`."
                ),
                span.clone(),
                "resource generic argument",
            )
            .with_cause("Only explicit resource APIs such as `ResourcePool<T: Resource>` may hold resources.")
            .with_fix(
                "use_resource_api",
                "Use `with`, `ResourcePool<T: Resource>`, or a non-resource value type.",
                "manual",
            ),
        );
    }

    pub(crate) fn generic_resource_argument_diagnostic(
        &mut self,
        generic_name: &str,
        resource_name: &str,
        span: &crate::diagnostic::Span,
        cause: &str,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::RESOURCE_GENERIC_ARGUMENT,
                format!(
                    "generic type `{generic_name}` cannot be used with resource `{resource_name}`."
                ),
                span.clone(),
                "resource generic misuse",
            )
            .with_cause(cause)
            .with_fix(
                "add_or_change_resource_bound",
                "Use explicit `T: Resource` only with approved resource APIs such as `ResourcePool<T>`.",
                "manual",
            ),
        );
    }

    pub(super) fn noalloc_allocation_diagnostic(
        &mut self,
        function_name: &str,
        span: &crate::diagnostic::Span,
        cause: String,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NOALLOC_ALLOCATION,
                format!("`{function_name}` is declared noalloc but contains an allocation site."),
                span.clone(),
                "allocation in noalloc function",
            )
            .with_cause(cause)
            .with_fix(
                "remove_allocation_or_noalloc",
                "Remove the allocation site, or remove `noalloc` from the function effects.",
                "manual",
            ),
        );
    }

    pub(super) fn allocating_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NOALLOC_CALL,
                format!(
                    "`{function_name}` is declared noalloc but calls possibly allocating function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "possibly allocating call in noalloc function",
            )
            .with_cause(
                "A `noalloc` function may only call enum variants or functions also declared `effects(noalloc)`.",
            )
            .with_fix(
                "remove_noalloc_or_call_noalloc",
                "Remove `noalloc`, or call only APIs whose signatures are declared `effects(noalloc)`.",
                "manual",
            ),
        );
    }

    pub(super) fn runtime_guarantee_call_diagnostic(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        match guarantee {
            RuntimeGuarantee::Noalloc => {
                self.allocating_call_diagnostic(function_name, callee, span)
            }
            RuntimeGuarantee::Pure => self.non_pure_call_diagnostic(function_name, callee, span),
            RuntimeGuarantee::NoBlock => self.blocking_call_diagnostic(function_name, callee, span),
            RuntimeGuarantee::NoPanic => self.panic_call_diagnostic(function_name, callee, span),
        }
    }

    pub(super) fn non_pure_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(checks::diagnostic_helpers::error_cause_manual_fix(
            code::INVALID_PURE_EFFECT,
            format!(
                "`{function_name}` is declared pure but calls non-pure function `{}`.",
                callee_display(callee)
            ),
            span.clone(),
            "non-pure call in pure function",
            "A `pure` function may only call constructors, enum variants, or functions also declared `effects(pure)`.",
            "remove_pure_or_call_pure",
            "Remove `pure`, or call only APIs whose signatures are declared `effects(pure)`.",
        ));
    }

    pub(super) fn pure_manage_diagnostic(
        &mut self,
        function_name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(checks::diagnostic_helpers::error_cause_manual_fix(
            code::INVALID_PURE_EFFECT,
            format!("`{function_name}` is declared pure but uses `manage`."),
            span.clone(),
            "manage in pure function",
            "`manage` consumes a local value and changes its ownership boundary; `pure` functions may observe inputs but must not consume local values.",
            "remove_manage_or_pure",
            "Move the `manage` operation outside the pure function, or remove `pure`.",
        ));
    }

    pub(super) fn pure_with_resource_diagnostic(
        &mut self,
        function_name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(checks::diagnostic_helpers::error_cause_manual_fix(
            code::INVALID_PURE_EFFECT,
            format!("`{function_name}` is declared pure but opens a resource scope."),
            span.clone(),
            "resource scope in pure function",
            "`with` introduces deterministic resource lifetime behavior; `pure` functions may observe inputs but must not open resource scopes.",
            "remove_with_or_pure",
            "Move resource handling outside the pure function, or remove `pure`.",
        ));
    }

    pub(crate) fn pure_resource_return_diagnostic(
        &mut self,
        function_name: &str,
        span: crate::diagnostic::Span,
        resource_name: &str,
    ) {
        self.diagnostics.push(checks::diagnostic_helpers::error_cause_manual_fix(
            code::INVALID_PURE_EFFECT,
            format!(
                "`{function_name}` is declared pure but returns resource `{resource_name}`."
            ),
            span,
            "resource return in pure function",
            "Returning a resource creates a lifetime boundary; `pure` functions must not open or return resources.",
            "remove_resource_return_or_pure",
            "Return an ordinary value, or remove `pure` from the resource-producing function.",
        ));
    }

    pub(crate) fn resource_return_type_name<'a>(&self, ty: &'a TypeRef) -> Option<&'a str> {
        let target = if matches!(ty.name.as_str(), "Result" | "Option") {
            ty.args.first().unwrap_or(ty)
        } else {
            ty
        };
        if self.hir.type_kind(&target.name) == Some(HirTypeKind::Resource) {
            Some(target.name.as_str())
        } else {
            None
        }
    }

    pub(super) fn blocking_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NO_BLOCK_CALL,
                format!(
                    "`{function_name}` is declared no_block but calls possibly blocking function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "possibly blocking call in no_block function",
            )
            .with_cause(
                "A `no_block` function may only call constructors, enum variants, or functions also declared `effects(no_block)`.",
            )
            .with_fix(
                "remove_no_block_or_call_no_block",
                "Remove `no_block`, or call only APIs whose signatures are declared `effects(no_block)`.",
                "manual",
            ),
        );
    }

    pub(super) fn panic_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NO_PANIC_CALL,
                format!(
                    "`{function_name}` is declared no_panic but calls possibly panicking function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "possibly panicking call in no_panic function",
            )
            .with_cause(
                "A `no_panic` function may only call constructors, enum variants, or functions also declared `effects(no_panic)`.",
            )
            .with_fix(
                "remove_no_panic_or_call_no_panic",
                "Remove `no_panic`, or call only APIs whose signatures are declared `effects(no_panic)`.",
                "manual",
            ),
        );
    }
}
