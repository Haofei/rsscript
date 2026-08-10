//! Semantic validation for platform-neutral external binding types.

use std::collections::HashSet;

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

fn unknown_type_name(ty: &TypeRef) -> Diagnostic {
    let name = type_ref_name(ty);
    Diagnostic::error(code::UNKNOWN_TYPE, format!("unknown type `{name}`."), ty.span.clone(), "unknown type")
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
}
