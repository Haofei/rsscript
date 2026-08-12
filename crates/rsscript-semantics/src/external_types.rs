//! Semantic validation for platform-neutral external binding types.

use std::collections::HashSet;

use crate::{hir::Hir, is_builtin_type_name};
use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::TypeRef;

/// Validate the `Dyn<Protocol>` spelling without compiler or runtime state.
pub fn external_binding_type_diagnostics(
    ty: &TypeRef,
    generic_params: &HashSet<&str>,
    visible_protocols: &HashSet<String>,
) -> Vec<Diagnostic> {
    if ty.args.len() != 1 {
        return vec![unknown_type_name(ty)];
    }
    let protocol = &ty.args[0];
    if !protocol.args.is_empty()
        || !protocol.fn_params.is_empty()
        || protocol.fn_return.is_some()
        || protocol.is_fresh
        || protocol.is_noescape
        || protocol.is_owned
    {
        return vec![unknown_type_name(ty)];
    }
    if generic_params.contains(protocol.name.as_str()) || visible_protocols.contains(&protocol.name)
    {
        return Vec::new();
    }
    vec![
        Diagnostic::error(
            code::UNKNOWN_PROTOCOL,
            format!("unknown protocol `{}`.", protocol.name),
            protocol.span.clone(),
            "unknown protocol",
        )
        .with_cause(
            "Protocol bounds and implementations must name an explicit `protocol` declaration.",
        )
        .with_fix(
            "declare_protocol",
            format!(
                "Declare `protocol {} {{ ... }}` or use a declared protocol name.",
                protocol.name
            ),
            "manual",
        ),
    ]
}

/// Validate all source type references in a program against the resolved HIR.
pub fn unknown_type_diagnostics(
    hir: &Hir,
    program: &rsscript_syntax::ast::Program,
    visible_protocols: &HashSet<String>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for item in &program.items {
        let (params, types): (HashSet<&str>, Vec<&TypeRef>) = match item {
            rsscript_syntax::ast::Item::Type(decl) => (
                decl.type_params
                    .iter()
                    .map(|param| param.name.as_str())
                    .collect(),
                decl.fields.iter().map(|field| &field.ty).collect(),
            ),
            rsscript_syntax::ast::Item::Function(function) => {
                let mut types = function
                    .params
                    .iter()
                    .map(|param| &param.ty)
                    .collect::<Vec<_>>();
                if let Some(return_ty) = &function.return_ty {
                    types.push(return_ty);
                }
                (
                    function
                        .type_params
                        .iter()
                        .map(|param| param.name.as_str())
                        .collect(),
                    types,
                )
            }
            _ => continue,
        };
        for ty in types {
            check_type_ref(hir, ty, &params, visible_protocols, &mut diagnostics);
        }
    }
    diagnostics
}

fn check_type_ref(
    hir: &Hir,
    ty: &TypeRef,
    generic_params: &HashSet<&str>,
    visible_protocols: &HashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if ty.name == "Dyn" {
        diagnostics.extend(external_binding_type_diagnostics(
            ty,
            generic_params,
            visible_protocols,
        ));
    } else if !hir.has_type_alias(&ty.name) && !known_type_ref(hir, ty, generic_params) {
        diagnostics.push(unknown_type_name(ty));
    }
    for arg in &ty.args {
        check_type_ref(hir, arg, generic_params, visible_protocols, diagnostics);
    }
    for param in &ty.fn_params {
        check_type_ref(hir, param, generic_params, visible_protocols, diagnostics);
    }
    if let Some(return_ty) = &ty.fn_return {
        check_type_ref(
            hir,
            return_ty,
            generic_params,
            visible_protocols,
            diagnostics,
        );
    }
}

fn known_type_ref(hir: &Hir, ty: &TypeRef, generic_params: &HashSet<&str>) -> bool {
    ty.name.is_empty()
        || ((ty.is_noescape || ty.is_owned) && ty.name == "Fn")
        || generic_params.contains(ty.name.as_str())
        || is_builtin_type_name(&ty.name)
        || hir.type_info(&ty.name).is_some()
}

fn unknown_type_name(ty: &TypeRef) -> Diagnostic {
    unknown_type_name_diagnostic(&type_ref_name(ty), &ty.span)
}

/// Construct the canonical diagnostic for an unresolved source-level type.
///
/// This accepts a rendered type name so declaration and protocol-implementation
/// checks can share the same language contract without rebuilding a syntax
/// `TypeRef` solely to emit a diagnostic.
pub fn unknown_type_name_diagnostic(name: &str, span: &rsscript_syntax::Span) -> Diagnostic {
    Diagnostic::error(code::UNKNOWN_TYPE, format!("unknown type `{name}`."), span.clone(), "unknown type")
        .with_cause("RSScript type checking must resolve source-level types before Rust lowering.")
        .with_fix("declare_or_import_type", format!("Declare `{name}`, import an `.rssi` contract that declares it, or use a known core/runtime type."), "manual")
}

fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let prefix = ty
                    .effective_fn_param_effect(index)
                    .map(|effect| format!("{} ", effect.as_str()))
                    .unwrap_or_default();
                format!("{prefix}{}", type_ref_name(param))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        format!(
            "{}<{}>",
            ty.name,
            ty.args
                .iter()
                .map(type_ref_name)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_dyn_protocol_visibility() {
        let program = rsscript_syntax::parse_source(
            "dyn.rss",
            "fn f(x: Dyn<Missing>) -> Unit { return Unit }",
        );
        let ty = match &program.items[0] {
            rsscript_syntax::ast::Item::Function(function) => &function.params[0].ty,
            _ => unreachable!(),
        };
        assert_eq!(
            external_binding_type_diagnostics(ty, &HashSet::new(), &HashSet::new())[0].code,
            code::UNKNOWN_PROTOCOL
        );
        let mut visible = HashSet::new();
        visible.insert("Missing".to_string());
        assert!(external_binding_type_diagnostics(ty, &HashSet::new(), &visible).is_empty());
    }

    #[test]
    fn unresolved_type_contract_is_shared_by_declaration_checks() {
        let program = rsscript_syntax::parse_source("types.rss", "struct Item { value: Missing }");
        let span = match &program.items[0] {
            rsscript_syntax::ast::Item::Type(decl) => &decl.fields[0].ty.span,
            _ => unreachable!(),
        };
        let diagnostic = unknown_type_name_diagnostic("Missing", span);
        assert_eq!(diagnostic.code, code::UNKNOWN_TYPE);
        assert_eq!(diagnostic.label, "unknown type");
        assert_eq!(diagnostic.fixes[0].kind, "declare_or_import_type");
    }
}
