use super::*;

impl Analyzer<'_> {
    pub(crate) fn check_unknown_types(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::unknown_type_diagnostics(
                &self.hir,
                &self.syntax_program,
                &self.visible_protocol_names(),
            ));
    }

    pub(crate) fn check_unknown_fields(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::unknown_field_diagnostics(&self.hir));
    }

    pub(crate) fn check_unknown_bindings(&mut self) {
        self.diagnostics
            .extend(rsscript_semantics::unknown_binding_diagnostics(
                &self.hir,
                &self.syntax_program,
            ));
    }

    pub(crate) fn check_fresh_generic_return_bound(
        &mut self,
        function_name: &str,
        return_ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        let target = fresh_return_target_type(return_ty);
        // A protocol method's implicit `Self` parameter is bound `Managed`, which
        // still admits a `fresh Self` return: managed structs/sums are freshly
        // ownable, and the per-instantiation derive (`derives(Clone)`) is checked
        // at the use site. A `fresh Self` from a value scalar is impossible
        // because scalars do not satisfy the protocol's `Managed` `Self` bound.
        let bound = bounds.get(&target.name).and_then(Option::as_ref);
        let fresh_bound_ok = matches!(bound, Some(GenericBound::Struct))
            || (target.name == "Self" && matches!(bound, Some(GenericBound::Managed)));
        if bounds.contains_key(&target.name) && !fresh_bound_ok {
            self.diagnostics.push(
                Diagnostic::error(
                    code::INVALID_FRESH_RETURN_TYPE,
                    format!(
                        "function `{function_name}` returns `fresh {}` but `{}` is not bounded by `Struct`.",
                        target.name, target.name
                    ),
                    target.span.clone(),
                    "invalid fresh generic type",
                )
                .with_cause("A generic `fresh T` return must require `T: Struct` so freshness is valid for every instantiation.")
                .with_fix(
                    "add_struct_bound",
                    format!("Declare `{}` with `{}: Struct`, or remove `fresh`.", target.name, target.name),
                    "manual",
                ),
            );
        }
    }

    pub(crate) fn check_resource_type_param_field(
        &mut self,
        ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        if bounds.get(&ty.name).and_then(Option::as_ref) == Some(&GenericBound::Resource) {
            self.generic_resource_argument_diagnostic(
                &ty.name,
                &ty.name,
                &ty.span,
                "generic resources cannot directly contain `T: Resource`; use an approved resource container.",
            );
        }
        for arg in &ty.args {
            self.check_resource_type_param_field(arg, bounds);
        }
    }
}
