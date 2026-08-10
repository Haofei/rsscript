use super::*;

impl Analyzer<'_> {
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
            .with_cause("Generic containers cannot hold resource values.")
            .with_fix(
                "use_resource_api",
                "Use the resource through `with`, or use a non-resource value type.",
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
                "Do not store `T: Resource` in a generic value.",
                "manual",
            ),
        );
    }
}
