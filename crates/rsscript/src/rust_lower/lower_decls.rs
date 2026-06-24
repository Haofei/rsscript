
use crate::syntax::ast::{
    ConstDecl, DataEffect, EffectDecl, FieldDecl,
    FunctionDecl, GenericBound, Param, SumTypeDecl, TypeAliasDecl, TypeDecl, TypeKind, TypeRef,
};

use super::helpers::*;
use super::source_map::generated_span_at_end;
use super::types::RustSourceMapEntry;

use super::lowerer::*;

impl RustLowerer<'_> {
    pub(super) fn lower_protocol_traits(&mut self, out: &mut String) {
        for protocol in &self.program.protocols {
            self.record_source_marker(out, 0, "protocol", &protocol.span);
            out.push_str(&format!("pub trait {} {{\n", rust_ident(&protocol.name)));
            for method in protocol_methods(self.program, &protocol.name) {
                out.push_str("    fn ");
                out.push_str(&rust_ident(protocol_method_name(&method.name)));
                out.push('(');
                out.push_str(&self.lower_protocol_trait_params(method));
                out.push(')');
                if let Some(return_ty) = &method.return_ty {
                    out.push_str(" -> ");
                    out.push_str(&self.lower_return_type(return_ty, method.returns_fresh));
                }
                out.push_str(";\n");
            }
            out.push_str("}\n\n");
        }
    }


    pub(super) fn lower_protocol_trait_params(&self, method: &FunctionDecl) -> String {
        method
            .params
            .iter()
            .map(|param| {
                if param.name == "self" {
                    return match param.effect {
                        Some(DataEffect::Read) => "&self".to_string(),
                        Some(DataEffect::Mut) => "&mut self".to_string(),
                        Some(DataEffect::Take) | None => "self".to_string(),
                    };
                }
                self.lower_param(param)
            })
            .collect::<Vec<_>>()
            .join(", ")
    }


    pub(super) fn lower_protocol_impls(&mut self, out: &mut String) {
        for protocol_impl in &self.program.protocol_impls {
            out.push_str(&format!(
                "impl {} for {} {{\n",
                rust_ident(&protocol_impl.protocol),
                rust_ident(&protocol_impl.type_name)
            ));
            for mapping in &protocol_impl.mappings {
                let Some(method) =
                    protocol_method(self.program, &protocol_impl.protocol, &mapping.method)
                else {
                    continue;
                };
                out.push_str("    fn ");
                out.push_str(&rust_ident(&mapping.method));
                out.push('(');
                out.push_str(&self.lower_protocol_trait_params(method));
                out.push(')');
                if let Some(return_ty) = &method.return_ty {
                    out.push_str(" -> ");
                    out.push_str(&self.lower_return_type(return_ty, method.returns_fresh));
                }
                out.push_str(" {\n        ");
                if method
                    .return_ty
                    .as_ref()
                    .is_none_or(|return_ty| return_ty.name != "Unit")
                {
                    out.push_str("return ");
                }
                out.push_str(&rust_function_ident(&mapping.target));
                out.push('(');
                out.push_str(
                    &method
                        .params
                        .iter()
                        .map(protocol_impl_forward_arg)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                out.push_str(");\n");
                out.push_str("    }\n");
            }
            out.push_str("}\n\n");
        }
    }


    pub(super) fn lower_capability_enums(&mut self, out: &mut String) {
        for protocol in &self.program.protocols {
            let impls = self.protocol_capability_impls(&protocol.name);
            if impls.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "pub enum {} {{\n",
                capability_enum_name(&protocol.name)
            ));
            for protocol_impl in impls {
                out.push_str(&format!(
                    "    {}({}),\n",
                    rust_ident(&protocol_impl.type_name),
                    rust_ident(&protocol_impl.type_name)
                ));
            }
            out.push_str("}\n\n");
        }
    }


    pub(super) fn lower_capability_impls(&mut self, out: &mut String) {
        for protocol in &self.program.protocols {
            let impls = self.protocol_capability_impls(&protocol.name);
            if impls.is_empty() {
                continue;
            }
            out.push_str(&format!(
                "impl {} for {} {{\n",
                rust_ident(&protocol.name),
                capability_enum_name(&protocol.name)
            ));
            for method in protocol_methods(self.program, &protocol.name) {
                out.push_str("    fn ");
                out.push_str(&rust_ident(protocol_method_name(&method.name)));
                out.push('(');
                out.push_str(&self.lower_protocol_trait_params(method));
                out.push(')');
                let returns_unit = method
                    .return_ty
                    .as_ref()
                    .is_none_or(|return_ty| return_ty.name == "Unit");
                if let Some(return_ty) = &method.return_ty {
                    out.push_str(" -> ");
                    out.push_str(&self.lower_return_type(return_ty, method.returns_fresh));
                }
                out.push_str(" {\n        ");
                if !returns_unit {
                    out.push_str("return ");
                }
                out.push_str("match self {\n");
                for protocol_impl in &impls {
                    let Some(mapping) = protocol_impl
                        .mappings
                        .iter()
                        .find(|mapping| mapping.method == protocol_method_name(&method.name))
                    else {
                        continue;
                    };
                    out.push_str(&format!(
                        "            {}::{}(inner) => {}({}),\n",
                        capability_enum_name(&protocol.name),
                        rust_ident(&protocol_impl.type_name),
                        rust_function_ident(&mapping.target),
                        method
                            .params
                            .iter()
                            .map(capability_impl_forward_arg)
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                out.push_str("        }");
                if returns_unit {
                    out.push(';');
                }
                out.push_str("\n    }\n");
            }
            out.push_str("}\n\n");
        }
    }


    pub(super) fn protocol_capability_impls(&self, protocol: &str) -> Vec<&crate::syntax::ast::ProtocolImpl> {
        self.program
            .protocol_impls
            .iter()
            .filter(|protocol_impl| protocol_impl.protocol == protocol)
            .collect()
    }


    pub(super) fn protocol_impl_namespace(&self, receiver_type: &str, method: &str) -> Option<String> {
        let mut matches = self
            .program
            .protocol_impls
            .iter()
            .filter(|protocol_impl| protocol_impl.type_name == receiver_type)
            .filter(|protocol_impl| {
                protocol_impl
                    .mappings
                    .iter()
                    .any(|mapping| mapping.method == method)
            })
            .map(|protocol_impl| protocol_impl.protocol.clone());
        let first = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(first)
    }


    pub(super) fn lower_type_decl(&mut self, ty: &TypeDecl, out: &mut String) {
        let generated_start = out.len();
        let marker = self.record_source_marker(out, 0, "type", &ty.span);
        if ty.kind == TypeKind::Resource {
            out.push_str("#[must_use]\n");
        }
        let has_closure_field = ty
            .fields
            .iter()
            .any(|field| type_ref_holds_closure(&field.ty));
        let derive_str = self.compute_derive_attr(
            &ty.derives,
            ty.kind == TypeKind::Resource,
            has_closure_field,
        );
        out.push_str(&derive_str);
        out.push_str(&format!(
            "{}struct {}{} {{\n",
            visibility(true),
            rust_ident(&ty.name),
            lower_generic_params(&ty.type_params)
        ));
        for field in &ty.fields {
            self.lower_field_decl(field, out);
        }
        out.push_str("}\n");
        if has_closure_field {
            // A stored `owned Fn` (`Rc<dyn Fn..>`) is not `Debug`, so the type
            // cannot derive `Debug`; emit a manual placeholder impl so generic
            // `{:?}`-using code (and the auto-`Debug` contract) still holds.
            out.push_str(&manual_struct_debug_impl(
                &rust_ident(&ty.name),
                &ty.type_params,
                &ty.fields,
            ));
        }

        if ty.kind == TypeKind::Resource {
            out.push_str(&format!(
                "\nimpl{} rsscript_runtime::Resource for {}{} {{}}\n",
                lower_impl_generics(&ty.type_params),
                rust_ident(&ty.name),
                lower_generic_args(&ty.type_params)
            ));
            out.push_str(&format!(
                "\nimpl{} Drop for {}{} {{\n",
                lower_impl_generics(&ty.type_params),
                rust_ident(&ty.name),
                lower_generic_args(&ty.type_params)
            ));
            out.push_str("    fn drop(&mut self) {\n");
            if let Some(drop_body) = &ty.drop_body {
                let previous_drop_field_names = std::mem::take(&mut self.drop_field_names);
                self.drop_field_names = ty.fields.iter().map(|field| field.name.clone()).collect();
                self.lower_block(drop_body, out, 2);
                self.drop_field_names = previous_drop_field_names;
            } else {
                out.push_str("        // RSScript resource has no explicit drop body.\n");
            }
            out.push_str("    }\n");
            out.push_str("}\n");
        }
        self.widen_generated_span(
            &marker.generated,
            generated_line_count(&out[generated_start..]),
        );
    }


    pub(super) fn lower_field_decl(&mut self, field: &FieldDecl, out: &mut String) {
        let rust_ty = self.lower_type_ref(&field.ty, ManagedPosition::Bare);
        self.source_map.push(RustSourceMapEntry {
            kind: "field".to_string(),
            source: field.span.clone(),
            generated: generated_span_at_end(out, "src/lib.rs", "field"),
            ..Default::default()
        });
        if field.is_weak {
            out.push_str(&format!(
                "    pub {}: rsscript_runtime::WeakManaged<{}>,\n",
                rust_ident(&field.name),
                rust_ty
            ));
        } else if field.is_handle
            || matches!(self.type_kinds.get(&field.ty.name), Some(TypeKind::Class))
        {
            out.push_str(&format!(
                "    pub {}: rsscript_runtime::Managed<{}>,\n",
                rust_ident(&field.name),
                rust_ty
            ));
        } else {
            out.push_str(&format!(
                "    pub {}: {},\n",
                rust_ident(&field.name),
                rust_ty
            ));
        }
    }


    pub(super) fn lower_sum_type(&mut self, sum: &SumTypeDecl, out: &mut String) {
        let has_closure_field = sum
            .variants
            .iter()
            .any(|variant| variant.fields.iter().any(|f| type_ref_holds_closure(&f.ty)));
        let derive_str = self.compute_derive_attr(&sum.derives, false, has_closure_field);
        out.push_str(&derive_str);
        out.push_str(&format!(
            "{}enum {} {{\n",
            visibility(sum.is_public),
            rust_ident(&sum.name)
        ));
        for variant in &sum.variants {
            if variant.fields.is_empty() {
                out.push_str(&format!("    {},\n", rust_ident(&variant.name)));
            } else {
                out.push_str(&format!("    {} {{\n", rust_ident(&variant.name)));
                for field in &variant.fields {
                    let rust_ty = self.lower_type_ref(&field.ty, ManagedPosition::Bare);
                    out.push_str(&format!(
                        "        {}: {},\n",
                        rust_ident(&field.name),
                        rust_ty
                    ));
                }
                out.push_str("    },\n");
            }
        }
        out.push_str("}\n");
    }


    /// Compute the `#[derive(...)]` attribute string.
    /// If user specified `derives(...)`, use those (always including Debug).
    /// Otherwise use defaults: Debug + Clone for non-resource, Debug only for resource.
    pub(super) fn compute_derive_attr(
        &self,
        user_derives: &[String],
        is_resource: bool,
        has_closure_field: bool,
    ) -> String {
        if user_derives.is_empty() {
            if is_resource {
                return "#[derive(Debug)]\n".to_string();
            } else if has_closure_field {
                // A stored `owned Fn` (`Rc<dyn Fn>`) is `Clone` but not `Debug`;
                // a manual `Debug` impl is emitted alongside the type.
                return "#[derive(Clone)]\n".to_string();
            } else {
                return "#[derive(Debug, Clone)]\n".to_string();
            }
        }
        let mut rust_derives = Vec::new();
        // `Rc<dyn Fn>` is `Clone` but not `Debug`/`PartialEq`/`Eq`/`Hash`/`Ord`.
        // For a closure-holding type, emit only the traits the closure can
        // satisfy (`Clone`, serde shims) and provide `Debug` via a manual impl;
        // the others are genuinely unavailable on a function value.
        if !has_closure_field {
            push_unique_derive(&mut rust_derives, "Debug");
        }
        for d in user_derives {
            match d.as_str() {
                "Debug" => {}
                "Clone" => push_unique_derive(&mut rust_derives, "Clone"),
                "Eq" if !has_closure_field => {
                    push_unique_derive(&mut rust_derives, "PartialEq");
                    push_unique_derive(&mut rust_derives, "Eq");
                }
                "Ord" if !has_closure_field => {
                    push_unique_derive(&mut rust_derives, "PartialEq");
                    push_unique_derive(&mut rust_derives, "Eq");
                    push_unique_derive(&mut rust_derives, "PartialOrd");
                    push_unique_derive(&mut rust_derives, "Ord");
                }
                "Hash" if !has_closure_field => push_unique_derive(&mut rust_derives, "Hash"),
                "Eq" | "Ord" | "Hash" => {}
                "JsonEncode" => push_unique_derive(&mut rust_derives, "serde::Serialize"),
                "JsonDecode" => push_unique_derive(&mut rust_derives, "serde::Deserialize"),
                "Schema" | "ReviewSchema" => {}
                _ => {}
            }
        }
        format!("#[derive({})]\n", rust_derives.join(", "))
    }


    pub(super) fn lower_type_alias(&mut self, alias: &TypeAliasDecl, out: &mut String) {
        let vis = visibility(alias.is_public);
        let generics = lower_generic_params(&alias.type_params);
        let target = self.lower_type_ref(&alias.target, ManagedPosition::Bare);
        out.push_str(&format!(
            "{}type {}{} = {};\n",
            vis,
            rust_ident(&alias.name),
            generics,
            target
        ));
    }


    pub(super) fn lower_const_decl(&mut self, decl: &ConstDecl, out: &mut String) {
        let vis = visibility(decl.is_public);
        let ty_str = if let Some(ty) = &decl.type_annotation {
            let lowered = self.lower_type_ref(ty, ManagedPosition::Bare);
            // Rust `const` cannot hold heap-allocated String; use &'static str
            if lowered == "String" {
                "&'static str".to_string()
            } else {
                lowered
            }
        } else {
            infer_const_type(&decl.value)
        };
        let value_str = lower_const_value(&decl.value);
        out.push_str(&format!(
            "{}const {}: {} = {};\n",
            vis,
            rust_ident(&decl.name).to_uppercase(),
            ty_str,
            value_str
        ));
    }


    pub(super) fn lower_function(&mut self, function: &FunctionDecl, out: &mut String) {
        let previous_param_effects = std::mem::take(&mut self.param_effects);
        let previous_value_types = std::mem::take(&mut self.value_types);
        let previous_managed_bindings = std::mem::take(&mut self.managed_bindings);
        let previous_read_view_bindings = std::mem::take(&mut self.read_view_bindings);
        let previous_retained_params = std::mem::take(&mut self.current_retained_params);
        let previous_mutated_bindings = std::mem::take(&mut self.mutated_bindings);
        let previous_return_type = self.current_return_type.take();
        let previous_async_executor = self.current_async_executor.take();
        self.param_effects = function
            .params
            .iter()
            .filter_map(|param| param.effect.map(|effect| (param.name.clone(), effect)))
            .collect();
        self.value_types = function
            .params
            .iter()
            .map(|param| (param.name.clone(), param.ty.clone()))
            .collect();
        self.generic_protocol_bounds = function
            .type_params
            .iter()
            .filter_map(|tp| match &tp.bound {
                Some(GenericBound::Protocol(protocol)) => Some((tp.name.clone(), protocol.clone())),
                _ => None,
            })
            .collect();
        self.managed_bindings = function
            .params
            .iter()
            .filter(|param| self.is_class_type(&param.ty))
            .map(|param| param.name.clone())
            .collect();
        self.current_retained_params = function
            .effects
            .iter()
            .filter_map(|effect| match effect {
                EffectDecl::Retains(param) => Some(param.clone()),
                EffectDecl::Name(_) => None,
            })
            .collect();
        self.mutated_bindings = collect_mutated_bindings(&function.body);
        self.current_return_type = function.return_ty.clone();
        // A user `async fn` lowers to a pending chain. `task_group` statements
        // are represented as explicit pending boundaries inside that chain; the
        // runnable entry point keeps a normal `fn main` that drives the chain
        // with `run_pending`.
        let is_runnable = is_runnable_main(function);
        let lower_as_pending_chain = function.is_async && !is_runnable;
        let lower_as_async_main = function.is_async && is_runnable;
        self.current_async_executor = (!function.is_async
            && block_needs_async_executor(&function.body))
        .then(|| "__rsscript_async_executor".to_string());
        let generated_start = out.len();
        let source_map_start = self.source_map.len();
        let marker = self.record_source_marker(out, 0, "function", &function.span);
        let is_public = function.is_public || is_runnable_main(function);
        out.push_str(&format!(
            "{}fn {}{}(",
            visibility(is_public),
            rust_function_ident(&function.name),
            lower_generic_params(&function.type_params)
        ));
        out.push_str(
            &function
                .params
                .iter()
                .map(|param| self.lower_param(param))
                .collect::<Vec<_>>()
                .join(", "),
        );
        out.push(')');
        if let Some(return_ty) = &function.return_ty {
            out.push_str(" -> ");
            let lowered = self.lower_return_type(return_ty, function.returns_fresh);
            if lower_as_pending_chain {
                // A leaf async fn returns a pending of its declared return type.
                out.push_str(&format!("impl rsscript_runtime::Pending<{lowered}>"));
            } else {
                out.push_str(&lowered);
            }
        }
        out.push_str(" {\n");
        if function.effects.iter().any(is_native_boundary) {
            out.push_str(
                "    // RSScript native/unsafe boundary: review before binding implementation.\n",
            );
        }
        if lower_as_pending_chain {
            let chain = self.lower_async_chain(&function.body.statements);
            out.push_str(&format!("    {chain}\n"));
        } else if lower_as_async_main {
            // The entrypoint is the only place that drives a pending to a value.
            let chain = self.lower_async_chain(&function.body.statements);
            out.push_str(&format!("    rsscript_runtime::run_pending({chain})\n"));
        } else {
            if let Some(executor) = &self.current_async_executor {
                out.push_str(&format!(
                    "    let mut {executor} = rsscript_runtime::Executor::new();\n"
                ));
            }
            self.lower_block(&function.body, out, 1);
        }
        out.push_str("}\n");
        self.widen_generated_span(
            &marker.generated,
            generated_line_count(&out[generated_start..]),
        );
        // Stamp every source-map entry produced while lowering this function with
        // its source symbol and lowered Rust symbol, so a remapped backend error
        // can name the declaration (not just the line).
        let lowered_symbol = rust_function_ident(&function.name);
        for entry in &mut self.source_map[source_map_start..] {
            entry.symbol = Some(function.name.clone());
            entry.lowered_symbol = Some(lowered_symbol.clone());
        }
        self.param_effects = previous_param_effects;
        self.value_types = previous_value_types;
        self.managed_bindings = previous_managed_bindings;
        self.read_view_bindings = previous_read_view_bindings;
        self.current_retained_params = previous_retained_params;
        self.mutated_bindings = previous_mutated_bindings;
        self.current_return_type = previous_return_type;
        self.current_async_executor = previous_async_executor;
    }


    pub(super) fn lower_param(&self, param: &Param) -> String {
        let ty = if param.effect == Some(DataEffect::Read)
            && self.current_retained_params.contains(&param.name)
            && !self.is_class_type(&param.ty)
            && self.type_kinds.contains_key(&param.ty.name)
        {
            format!(
                "rsscript_runtime::Managed<{}>",
                self.lower_type_ref(&param.ty, ManagedPosition::Bare)
            )
        } else {
            self.lower_type_ref(&param.ty, ManagedPosition::Param)
        };
        let rust_ty = match param.effect {
            // Copy primitives are passed `read` by value (call sites already lower
            // `read <int>` by value via `read_effect_lowers_by_value`); the param
            // type must match so the value is owned inside the body.
            Some(DataEffect::Read) if Self::read_effect_lowers_by_value(&param.ty) => ty,
            Some(DataEffect::Read) => format!("&{ty}"),
            Some(DataEffect::Mut) if self.is_class_type(&param.ty) => format!("&{ty}"),
            Some(DataEffect::Mut) => format!("&mut {ty}"),
            Some(DataEffect::Take) | None => ty,
        };
        let name = rust_value_ident(&param.name);
        if (param.ty.is_noescape || param.ty.is_owned) && param.ty.name == "Fn" {
            format!("mut {name}: {rust_ty}")
        } else {
            format!("{name}: {rust_ty}")
        }
    }


    pub(super) fn lower_return_type(&self, ty: &TypeRef, returns_fresh: bool) -> String {
        let position = if returns_fresh {
            ManagedPosition::FreshReturn
        } else {
            ManagedPosition::Return
        };
        self.lower_type_ref(ty, position)
    }

}
