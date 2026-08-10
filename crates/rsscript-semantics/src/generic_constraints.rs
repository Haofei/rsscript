//! Semantic generic-bound diagnostics independent of compiler orchestration.

use std::collections::HashMap;

use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{GenericBound, GenericParam, Item, Program, TypeKind, TypeRef};

/// Validate resource generic fields and fresh generic return requirements.
pub fn generic_constraint_diagnostics(program: &Program) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &program.items {
        match item {
            Item::Type(decl) if decl.kind == TypeKind::Resource => {
                let bounds = generic_bounds(&decl.type_params);
                for param in &decl.type_params {
                    if param.bound.is_none() {
                        diagnostics.push(generic_resource_argument_diagnostic(
                            &param.name,
                            &param.name,
                            &param.span,
                            "resource type parameters must declare an explicit bound.",
                        ));
                    }
                }
                for field in &decl.fields {
                    collect_resource_type_param_field(&field.ty, &bounds, &mut diagnostics);
                }
            }
            Item::Function(function) if function.returns_fresh => {
                let Some(return_ty) = &function.return_ty else {
                    continue;
                };
                let bounds = generic_bounds(&function.type_params);
                let target = fresh_return_target_type(return_ty);
                let bound = bounds.get(&target.name).and_then(Option::as_ref);
                let fresh_bound_ok = matches!(bound, Some(GenericBound::Struct))
                    || (target.name == "Self" && matches!(bound, Some(GenericBound::Managed)));
                if bounds.contains_key(&target.name) && !fresh_bound_ok {
                    diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_FRESH_RETURN_TYPE,
                            format!(
                                "function `{}` returns `fresh {}` but `{}` is not bounded by `Struct`.",
                                function.name, target.name, target.name
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
            _ => {}
        }
    }
    diagnostics
}

fn generic_bounds(params: &[GenericParam]) -> HashMap<String, Option<GenericBound>> {
    params
        .iter()
        .map(|param| (param.name.clone(), param.bound.clone()))
        .collect()
}

fn collect_resource_type_param_field(
    ty: &TypeRef,
    bounds: &HashMap<String, Option<GenericBound>>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if bounds.get(&ty.name).and_then(Option::as_ref) == Some(&GenericBound::Resource) {
        diagnostics.push(generic_resource_argument_diagnostic(
            &ty.name,
            &ty.name,
            &ty.span,
            "generic resources cannot directly contain `T: Resource`; use an approved resource container.",
        ));
    }
    for arg in &ty.args {
        collect_resource_type_param_field(arg, bounds, diagnostics);
    }
}

fn fresh_return_target_type(return_ty: &TypeRef) -> &TypeRef {
    if matches!(return_ty.name.as_str(), "Result" | "Option")
        && let Some(first_arg) = return_ty.args.first()
    {
        return first_arg;
    }
    return_ty
}

fn generic_resource_argument_diagnostic(
    generic_name: &str,
    resource_name: &str,
    span: &rsscript_syntax::Span,
    cause: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::RESOURCE_GENERIC_ARGUMENT,
        format!("generic type `{generic_name}` cannot be used with resource `{resource_name}`."),
        span.clone(),
        "resource generic misuse",
    )
    .with_cause(cause)
    .with_fix(
        "add_or_change_resource_bound",
        "Do not store `T: Resource` in a generic value.",
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_resource_fields_and_fresh_generic_returns() {
        let program = rsscript_syntax::parse_source(
            "constraints.rss",
            r#"
resource Bag<T: Resource> { value: T }
fn make<T: Managed>() -> fresh T
"#,
        );
        let diagnostics = generic_constraint_diagnostics(&program);

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].code, code::RESOURCE_GENERIC_ARGUMENT);
        assert_eq!(diagnostics[1].code, code::INVALID_FRESH_RETURN_TYPE);
    }
}
