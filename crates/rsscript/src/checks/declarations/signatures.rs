use std::collections::HashSet;

use crate::analyzer::{
    Analyzer, function_belongs_to_protocol, function_body_belongs_to_protocol, generic_bounds,
    is_builtin_type_name, protocol_method_names, protocol_signature_mismatch, split_qualified_name,
    type_ref_is_copy, type_ref_is_noescape,
};
use crate::diagnostic::{Diagnostic, code};
use crate::syntax::ast::{FunctionDecl, GenericBound, GenericParam, Item, Param, TypeKind};

impl Analyzer<'_> {
    pub(crate) fn check_signature_explicitness(&mut self) {
        let items = self.syntax_program.items.clone();
        let protocol_names = self
            .syntax_program
            .protocols
            .iter()
            .map(|protocol| protocol.name.clone())
            .collect::<HashSet<_>>();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            let is_qualified_method = function.name.contains('.');
            let is_protocol_method = function
                .name
                .split_once('.')
                .is_some_and(|(namespace, _)| protocol_names.contains(namespace));

            self.check_return_type_explicit(function);
            self.check_params(function, is_qualified_method);
            self.check_protocol_self_parameter(function, is_protocol_method);
            self.check_retains_parameters(function);
        }
    }

    /// Functions must declare an explicit return type (no inference at API
    /// boundaries).
    pub(super) fn check_return_type_explicit(&mut self, function: &FunctionDecl) {
        if function.return_ty.is_none() {
            self.diagnostics.push(
                Diagnostic::error(
                    code::MISSING_RETURN_TYPE,
                    format!("function `{}` must declare an explicit return type.", function.name),
                    function.span.clone(),
                    "missing return type",
                )
                .with_cause("Public APIs must not rely on inference; this checker applies the canonical rule to all functions.")
                .with_fix("add_return_type", "Add `-> Unit` or another explicit return type.", "manual"),
            );
        }
    }

    /// Per-parameter checks: misplaced `self` and explicit parameter types.
    pub(super) fn check_params(&mut self, function: &FunctionDecl, is_qualified_method: bool) {
        for (index, param) in function.params.iter().enumerate() {
            if param.name == "self" && (!is_qualified_method || index != 0) {
                self.invalid_self_parameter_diagnostic(
                    function,
                    param,
                    "`self` may only be the first parameter of a qualified method signature.",
                );
            }
            if param.ty.name.is_empty() {
                self.diagnostics.push(
                    Diagnostic::error(
                        code::MISSING_PARAMETER_TYPE,
                        format!(
                            "parameter `{}` in `{}` must declare an explicit type.",
                            param.name, function.name
                        ),
                        param.span.clone(),
                        "missing parameter type",
                    )
                    .with_fix(
                        "add_parameter_type",
                        "Add an explicit parameter type.",
                        "manual",
                    ),
                );
            }
        }
    }

    /// Protocol methods must declare `self: read|mut|take Self` first.
    pub(super) fn check_protocol_self_parameter(
        &mut self,
        function: &FunctionDecl,
        is_protocol_method: bool,
    ) {
        if !is_protocol_method {
            return;
        }
        match function.params.first() {
            Some(param)
                if param.name == "self"
                    && param.ty.name == "Self"
                    && param.effective_effect().is_some() => {}
            Some(param) => self.invalid_self_parameter_diagnostic(
                function,
                param,
                "Protocol methods must declare `self: read|mut|take Self` as their first parameter.",
            ),
            None => self.diagnostics.push(
                Diagnostic::error(
                    code::INVALID_SELF_PARAMETER,
                    format!(
                        "protocol method `{}` must declare an explicit `self` parameter.",
                        function.name
                    ),
                    function.span.clone(),
                    "missing protocol self parameter",
                )
                .with_cause("Protocol calls are explicit external_binding calls, so the receiver must be review-visible as `self: read|mut|take Self`.")
                .with_fix(
                    "add_protocol_self",
                    "Add `self: read Self`, `self: mut Self`, or `self: take Self` as the first parameter.",
                    "manual",
                ),
            ),
        }
    }

    /// `retains(p)` items: `p` must be a parameter, non-Copy, and not a
    /// noescape callback.
    pub(super) fn check_retains_parameters(&mut self, function: &FunctionDecl) {
        let param_names: HashSet<&str> = function
            .params
            .iter()
            .map(|param| param.name.as_str())
            .collect();
        for param in &function.retained_params {
            if !param_names.contains(param.as_str()) {
                self.diagnostics.push(
                    Diagnostic::error(
                        code::UNKNOWN_RETAINED_PARAMETER,
                        format!(
                            "`{}` declares `retains({param})`, but `{param}` is not a parameter.",
                            function.name
                        ),
                        function.span.clone(),
                        "unknown retained parameter",
                    )
                    .with_cause(
                        "Retention effects must name a parameter from the same function signature.",
                    )
                    .with_fix(
                        "fix_retains_parameter",
                        "Rename the retained parameter or remove this retention effect.",
                        "manual",
                    ),
                );
                continue;
            }
            if let Some(function_param) = function
                .params
                .iter()
                .find(|function_param| function_param.name == *param)
                && type_ref_is_copy(&function_param.ty)
            {
                self.diagnostics.push(
                    Diagnostic::error(
                        code::UNKNOWN_RETAINED_PARAMETER,
                        format!(
                            "`{}` declares `retains({param})`, but `{param}` is Copy.",
                            function.name
                        ),
                        function_param.span.clone(),
                        "Copy parameter cannot be retained",
                    )
                    .with_cause(
                        "`retains(x)` marks a managed retention boundary. Copy values have no managed handle to retain.",
                    )
                    .with_fix(
                        "remove_copy_retains",
                        format!("Remove `retains({param})`."),
                        "manual",
                    ),
                );
                continue;
            }
            if function.params.iter().any(|function_param| {
                function_param.name == *param && type_ref_is_noescape(&function_param.ty)
            }) {
                self.diagnostics.push(
                    Diagnostic::error(
                        code::NOESCAPE_CALLBACK_ESCAPE,
                        format!(
                            "`{}` cannot retain noescape callback parameter `{param}`.",
                            function.name
                        ),
                        function.span.clone(),
                        "noescape callback escapes",
                    )
                    .with_cause(
                        "`noescape Fn()` parameters may be called or forwarded to another noescape parameter, but they cannot be retained after return.",
                    )
                    .with_fix(
                        "remove_noescape_retention",
                        format!("Remove `retains({param})`, or use an ordinary managed callback type."),
                        "manual",
                    ),
                );
            }
        }
    }

    pub(super) fn invalid_self_parameter_diagnostic(
        &mut self,
        function: &FunctionDecl,
        param: &Param,
        cause: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_SELF_PARAMETER,
                format!("invalid `self` parameter in `{}`.", function.name),
                param.span.clone(),
                "invalid self parameter",
            )
            .with_cause(cause)
            .with_fix(
                "fix_self_parameter",
                "Use a different parameter name, or make this the first parameter of an explicit method/protocol signature.",
                "manual",
            ),
        );
    }

    pub(crate) fn check_generic_constraints(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    let bounds = generic_bounds(&decl.type_params);
                    if decl.kind == TypeKind::Resource {
                        for param in &decl.type_params {
                            if param.bound.is_none() {
                                self.generic_resource_argument_diagnostic(
                                    &param.name,
                                    &param.name,
                                    &param.span,
                                    "resource type parameters must declare an explicit bound.",
                                );
                            }
                        }
                        for field in &decl.fields {
                            self.check_resource_type_param_field(&field.ty, &bounds);
                        }
                    }
                }
                Item::Function(function) => {
                    let bounds = generic_bounds(&function.type_params);
                    if let Some(return_ty) = &function.return_ty
                        && function.returns_fresh
                    {
                        self.check_fresh_generic_return_bound(&function.name, return_ty, &bounds);
                    }
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }
    }

    pub(crate) fn check_protocol_contracts(&mut self) {
        let protocol_names = self.visible_protocol_names();
        let items = self.visible_protocol_items();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    for param in &decl.type_params {
                        self.check_protocol_bound(param, &protocol_names);
                    }
                }
                Item::Function(function) => {
                    if function_body_belongs_to_protocol(function, &protocol_names) {
                        self.unsupported_syntax(
                            function.span.clone(),
                            "unsupported protocol method body",
                            "Protocols are effect-carrying external_binding contracts in v0.7. Protocol methods are bodyless signatures; default method bodies are not part of the RSScript protocol model.",
                        );
                    }
                    if function.default_impl_marker
                        && !function_belongs_to_protocol(function, &protocol_names)
                    {
                        self.unsupported_syntax(
                            function.span.clone(),
                            "unsupported default implementation marker",
                            "`= _` is reserved for protocol method contracts so defaulted protocol behavior is review-visible.",
                        );
                    }
                    for param in &function.type_params {
                        self.check_protocol_bound(param, &protocol_names);
                    }
                }
                Item::Module(_)
                | Item::Use(_)
                | Item::SumType(_)
                | Item::TypeAlias(_)
                | Item::Const(_) => {}
            }
        }

        let protocol_impls = self.syntax_program.protocol_impls.clone();
        for protocol_impl in &protocol_impls {
            if !protocol_names.contains(&protocol_impl.protocol) {
                self.unknown_protocol_diagnostic(&protocol_impl.protocol, &protocol_impl.span);
                continue;
            }
            if self.hir.type_info(&protocol_impl.type_name).is_none()
                && !is_builtin_type_name(&protocol_impl.type_name)
            {
                self.unknown_type_name_diagnostic(&protocol_impl.type_name, &protocol_impl.span);
            }
            let protocol_methods = protocol_method_names(&items, &protocol_impl.protocol);
            let mapped_methods = protocol_impl
                .mappings
                .iter()
                .map(|mapping| mapping.method.clone())
                .collect::<HashSet<_>>();
            for method in &protocol_methods {
                if !mapped_methods.contains(method) {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        method,
                        &protocol_impl.span,
                        "missing protocol method mapping",
                        format!(
                            "`{}` must map protocol method `{method}` to a concrete function.",
                            protocol_impl.type_name
                        ),
                    );
                }
            }

            for mapping in &protocol_impl.mappings {
                let Some(protocol_signature) = self
                    .hir
                    .resolve_function(Some(&protocol_impl.protocol), &mapping.method)
                else {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        &mapping.method,
                        &mapping.span,
                        "unknown protocol method",
                        format!(
                            "`{}` does not declare method `{}`.",
                            protocol_impl.protocol, mapping.method
                        ),
                    );
                    continue;
                };
                let (target_namespace, target_name) = split_qualified_name(&mapping.target);
                let Some(target_signature) = self
                    .hir
                    .resolve_function(target_namespace.as_deref(), target_name)
                else {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        &mapping.method,
                        &mapping.span,
                        "unknown protocol implementation target",
                        format!(
                            "Mapped target `{}` must resolve to a concrete function.",
                            mapping.target
                        ),
                    );
                    continue;
                };
                if let Some(reason) = protocol_signature_mismatch(
                    protocol_signature,
                    target_signature,
                    &protocol_impl.type_name,
                ) {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        &mapping.method,
                        &mapping.span,
                        "protocol implementation signature mismatch",
                        reason,
                    );
                }
            }
        }
    }

    pub(super) fn visible_protocol_names(&self) -> HashSet<String> {
        self.interface_programs
            .iter()
            .flat_map(|program| program.protocols.iter())
            .chain(self.syntax_program.protocols.iter())
            .map(|protocol| protocol.name.clone())
            .collect()
    }

    pub(crate) fn protocol_name_is_visible(&self, name: &str) -> bool {
        self.interface_programs
            .iter()
            .flat_map(|program| program.protocols.iter())
            .chain(self.syntax_program.protocols.iter())
            .any(|protocol| protocol.name == name)
    }

    pub(super) fn visible_protocol_items(&self) -> Vec<Item> {
        self.interface_programs
            .iter()
            .flat_map(|program| program.items.iter().cloned())
            .chain(self.syntax_program.items.iter().cloned())
            .collect()
    }

    pub(super) fn check_protocol_bound(
        &mut self,
        param: &GenericParam,
        protocol_names: &HashSet<String>,
    ) {
        let Some(GenericBound::Protocol(protocol)) = &param.bound else {
            return;
        };
        if !protocol_names.contains(protocol) {
            self.unknown_protocol_diagnostic(protocol, &param.span);
        }
    }
}
