use std::collections::HashSet;

use crate::analyzer::{
    Analyzer, function_belongs_to_protocol, function_body_belongs_to_protocol,
    protocol_method_names, protocol_signature_mismatch, split_qualified_name,
};
use crate::syntax::ast::Item;
use rsscript_semantics::is_builtin_type_name;

impl Analyzer<'_> {
    pub(crate) fn check_protocol_contracts(&mut self) {
        let protocol_names = self.visible_protocol_names();
        let items = self.visible_protocol_items();
        self.diagnostics
            .extend(rsscript_semantics::protocol_bound_diagnostics(
                &self.interface_programs,
                &self.syntax_program,
            ));
        for item in &items {
            match item {
                Item::Type(_) => {}
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

    pub(crate) fn visible_protocol_names(&self) -> HashSet<String> {
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
}
