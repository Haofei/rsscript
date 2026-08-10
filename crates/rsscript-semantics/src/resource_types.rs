//! Platform-neutral resource declaration diagnostics.

use crate::hir::{Hir, HirTypeKind};
use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{Item, Program, TypeKind, TypeRef};

/// Reject raw descriptor handles outside resource-internal declarations.
pub fn fd_surface_diagnostics(program: &Program) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &program.items {
        match item {
            Item::Function(function) => {
                for param in &function.params {
                    if type_ref_contains_name(&param.ty, "Fd") {
                        diagnostics.push(fd_surface_diagnostic(
                            &param.ty.span,
                            "`Fd` parameter outside native boundary",
                            "Use a `resource` type such as `File` instead of exposing raw descriptor handles.",
                        ));
                    }
                }
                if let Some(return_ty) = &function.return_ty
                    && type_ref_contains_name(return_ty, "Fd")
                {
                    diagnostics.push(fd_surface_diagnostic(
                        &return_ty.span,
                        "`Fd` return outside native boundary",
                        "Return a `resource` type such as `File` instead of exposing raw descriptor handles.",
                    ));
                }
            }
            Item::Type(decl) if decl.kind != TypeKind::Resource => {
                for field in &decl.fields {
                    if type_ref_contains_name(&field.ty, "Fd") {
                        diagnostics.push(fd_surface_diagnostic(
                            &field.ty.span,
                            "`Fd` field outside resource internals",
                            "Use a `resource` field wrapper or a non-Fd public value type.",
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    diagnostics
}

/// Reject resource fields stored in non-resource declarations.
pub fn resource_field_diagnostics(hir: &Hir, program: &Program) -> Vec<Diagnostic> {
    program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Type(decl) if hir.type_kind(&decl.name) != Some(HirTypeKind::Resource) => {
                Some((decl.name.as_str(), decl.fields.as_slice()))
            }
            _ => None,
        })
        .flat_map(|(container, fields)| {
            fields
                .iter()
                .filter(move |field| hir.type_kind(&field.ty.name) == Some(HirTypeKind::Resource))
                .map(move |field| {
                    Diagnostic::error(
                        code::RESOURCE_FIELD,
                        format!(
                            "resource `{}` cannot be stored in `{container}`.",
                            field.ty.name
                        ),
                        field.span.clone(),
                        "resource field",
                    )
                    .with_cause("Resources must be used through `with`.")
                    .with_fix(
                        "use_with",
                        "Use the resource through `with` instead.",
                        "manual",
                    )
                })
        })
        .collect()
}

/// Ensure weak fields point at identity-managed classes.
pub fn weak_field_diagnostics(hir: &Hir, program: &Program) -> Vec<Diagnostic> {
    program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Type(decl) => Some(decl.fields.as_slice()),
            _ => None,
        })
        .flat_map(|fields| {
            fields
                .iter()
                .filter(|field| {
                    field.is_weak && hir.type_kind(&field.ty.name) != Some(HirTypeKind::Class)
                })
                .map(|field| {
                    Diagnostic::error(
                        code::INVALID_WEAK_FIELD,
                        format!(
                            "weak field `{}` must point to a class, but `{}` is not a class.",
                            field.name, field.ty.name
                        ),
                        field.span.clone(),
                        "invalid weak field",
                    )
                    .with_cause(
                        "`weak` is only for breaking managed identity-object cycles in the MVP.",
                    )
                    .with_fix(
                        "use_class_or_remove_weak",
                        "Use a class type for the weak field, or remove `weak`.",
                        "manual",
                    )
                })
        })
        .collect()
}

fn type_ref_contains_name(ty: &TypeRef, name: &str) -> bool {
    ty.name == name
        || ty.args.iter().any(|arg| type_ref_contains_name(arg, name))
        || ty
            .fn_params
            .iter()
            .any(|param| type_ref_contains_name(param, name))
        || ty
            .fn_return
            .as_deref()
            .is_some_and(|return_ty| type_ref_contains_name(return_ty, name))
}

fn fd_surface_diagnostic(span: &rsscript_syntax::Span, summary: &str, fix: &str) -> Diagnostic {
    Diagnostic::error(
        code::FD_OUTSIDE_INTERNAL_BOUNDARY,
        summary,
        span.clone(),
        "Fd outside native/resource internals",
    )
    .with_cause(
        "`Fd` is a trusted native/resource-internal descriptor handle, not an ordinary RSScript value type.",
    )
    .with_fix("use_resource_wrapper", fix, "manual")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_resource_boundary_diagnostics_from_source_and_hir() {
        let program = rsscript_syntax::parse_source(
            "resources.rss",
            r#"
resource File { raw: Fd }
struct State {
    file: File
    link: weak State
    raw: Fd
}
fn expose(raw: Fd) -> Fd { raw }
"#,
        );
        let hir = Hir::from_syntax(&program);

        assert_eq!(fd_surface_diagnostics(&program).len(), 3);
        assert_eq!(resource_field_diagnostics(&hir, &program).len(), 1);
        assert_eq!(weak_field_diagnostics(&hir, &program).len(), 1);
    }
}
