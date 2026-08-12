//! Protocol-bound visibility diagnostics.

use std::collections::{BTreeMap, HashSet};

use crate::{
    ResolvedType,
    hir::{FunctionSig, ParamSig},
};
use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{FunctionDecl, GenericBound, Item, Program};

/// Derive source-level protocol declaration diagnostics.
///
/// Protocol methods are explicit, bodyless contracts. The parser records them
/// as qualified functions (`Protocol.method`), so this rule stays independent
/// of HIR construction and is shared by every compiler front end.
pub fn protocol_declaration_diagnostics(
    items: &[Item],
    visible_protocol_names: &HashSet<String>,
) -> Vec<Diagnostic> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .flat_map(|function| {
            let belongs_to_protocol = function_belongs_to_protocol(function, visible_protocol_names);
            let mut diagnostics = Vec::new();
            if !function.body.statements.is_empty() && belongs_to_protocol {
                diagnostics.push(crate::unsupported_syntax_diagnostic(
                    function.span.clone(),
                    "unsupported protocol method body",
                    "Protocols are effect-carrying external_binding contracts in v0.7. Protocol methods are bodyless signatures; default method bodies are not part of the RSScript protocol model.",
                ));
            }
            if function.default_impl_marker && !belongs_to_protocol {
                diagnostics.push(crate::unsupported_syntax_diagnostic(
                    function.span.clone(),
                    "unsupported default implementation marker",
                    "`= _` is reserved for protocol method contracts so defaulted protocol behavior is review-visible.",
                ));
            }
            diagnostics
        })
        .collect()
}

/// Return the method names declared by `protocol` in a source/interface item
/// stream. The parser canonicalizes protocol methods as qualified functions.
pub fn protocol_method_names(items: &[Item], protocol: &str) -> HashSet<String> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(function),
            _ => None,
        })
        .filter_map(|function| {
            let (namespace, method) = split_qualified_name(&function.name);
            (namespace == Some(protocol)).then(|| method.to_owned())
        })
        .collect()
}

fn function_belongs_to_protocol(
    function: &FunctionDecl,
    visible_protocol_names: &HashSet<String>,
) -> bool {
    split_qualified_name(&function.name)
        .0
        .is_some_and(|namespace| visible_protocol_names.contains(namespace))
}

fn split_qualified_name(name: &str) -> (Option<&str>, &str) {
    name.rsplit_once('.')
        .map_or((None, name), |(namespace, method)| {
            (Some(namespace), method)
        })
}

/// Derive unknown protocol-bound diagnostics from the same interface/source
/// snapshot used to build HIR.
pub fn protocol_bound_diagnostics(
    interface_programs: &[Program],
    source_program: &Program,
) -> Vec<Diagnostic> {
    let names = interface_programs
        .iter()
        .flat_map(|program| program.protocols.iter())
        .chain(source_program.protocols.iter())
        .map(|protocol| protocol.name.as_str())
        .collect::<HashSet<_>>();
    interface_programs
        .iter()
        .chain(std::iter::once(source_program))
        .flat_map(|program| program.items.iter())
        .flat_map(|item| match item {
            Item::Type(decl) => decl.type_params.iter().collect::<Vec<_>>(),
            Item::Function(function) => function.type_params.iter().collect(),
            _ => Vec::new(),
        })
        .filter_map(|param| match &param.bound {
            Some(GenericBound::Protocol(protocol)) if !names.contains(protocol.as_str()) => {
                Some(unknown_protocol_diagnostic(protocol, &param.span))
            }
            _ => None,
        })
        .collect()
}

/// Construct the canonical diagnostic for an unresolved protocol reference.
/// Protocol implementation mapping checks use this same semantic fact.
pub fn unknown_protocol_diagnostic(name: &str, span: &rsscript_syntax::Span) -> Diagnostic {
    Diagnostic::error(
        code::UNKNOWN_PROTOCOL,
        format!("unknown protocol `{name}`."),
        span.clone(),
        "unknown protocol",
    )
    .with_cause("Protocol bounds and implementations must name an explicit `protocol` declaration.")
    .with_fix(
        "declare_protocol",
        format!("Declare `protocol {name} {{ ... }}` or use a declared protocol name."),
        "manual",
    )
}

/// Construct the canonical diagnostic for a protocol implementation mapping
/// that cannot satisfy its declared protocol contract.
pub fn protocol_impl_mismatch_diagnostic(
    protocol: &str,
    type_name: &str,
    method: &str,
    span: &rsscript_syntax::Span,
    label: impl Into<String>,
    cause: impl Into<String>,
) -> Diagnostic {
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
    )
}

/// Compare a resolved protocol method signature with its mapped concrete
/// implementation after substituting `Self` with the implementation type.
/// Returns the stable human-readable mismatch reason used by the protocol
/// implementation diagnostic.
pub fn protocol_signature_mismatch(
    protocol: &FunctionSig,
    target: &FunctionSig,
    concrete_type: &str,
) -> Option<String> {
    if protocol.is_async != target.is_async {
        return Some("async/sync kind must match the protocol method exactly.".to_owned());
    }
    if protocol.params.len() != target.params.len() {
        return Some(format!(
            "parameter count mismatch: protocol has {}, implementation has {}.",
            protocol.params.len(),
            target.params.len()
        ));
    }
    for (protocol_param, target_param) in protocol.params.iter().zip(&target.params) {
        if let Some(reason) = protocol_param_mismatch(protocol_param, target_param, concrete_type) {
            return Some(reason);
        }
    }
    let substitutions =
        BTreeMap::from([("Self".to_owned(), ResolvedType::from_display(concrete_type))]);
    let protocol_return = protocol
        .return_ty
        .as_ref()
        .map(|return_type| return_type.substitute(&substitutions));
    if protocol_return != target.return_ty {
        return Some(format!(
            "return type mismatch: protocol expects `{}`, implementation returns `{}`.",
            protocol_return
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "Unit".to_owned()),
            target
                .return_ty
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "Unit".to_owned())
        ));
    }
    if protocol.returns_fresh != target.returns_fresh {
        return Some("fresh return mode must match the protocol method exactly.".to_owned());
    }
    if protocol.retained_params != target.retained_params {
        return Some("retains(...) effects must match the protocol method exactly.".to_owned());
    }
    None
}

fn protocol_param_mismatch(
    protocol: &ParamSig,
    target: &ParamSig,
    concrete_type: &str,
) -> Option<String> {
    if protocol.name != target.name {
        return Some(format!(
            "parameter name mismatch: protocol expects `{}`, implementation has `{}`.",
            protocol.name, target.name
        ));
    }
    if protocol.effect != target.effect {
        return Some(format!(
            "parameter effect mismatch for `{}`: protocol expects `{}`, implementation has `{}`.",
            protocol.name,
            protocol
                .effect
                .map(|effect| effect.as_str())
                .unwrap_or("none"),
            target
                .effect
                .map(|effect| effect.as_str())
                .unwrap_or("none")
        ));
    }
    let expected = protocol.ty.substitute(&BTreeMap::from([(
        "Self".to_owned(),
        ResolvedType::from_display(concrete_type),
    )]));
    if expected != target.ty {
        return Some(format!(
            "parameter type mismatch for `{}`: protocol expects `{expected}`, implementation has `{}`.",
            protocol.name, target.ty
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_only_unresolved_protocol_bounds() {
        let source =
            rsscript_syntax::parse_source("bounds.rss", "struct Box<T: Missing> { value: T }\n");
        assert_eq!(protocol_bound_diagnostics(&[], &source).len(), 1);
    }

    #[test]
    fn protocol_implementation_mismatch_contract_is_canonical() {
        let program = rsscript_syntax::parse_source("impl.rss", "impl Missing for Item {}");
        let span = &program.protocol_impls[0].span;
        let diagnostic = protocol_impl_mismatch_diagnostic(
            "Missing",
            "Item",
            "run",
            span,
            "missing protocol method mapping",
            "Item must map protocol method run to a concrete function.",
        );
        assert_eq!(diagnostic.code, code::PACKAGE_INTERFACE_MISMATCH);
        assert_eq!(diagnostic.label, "missing protocol method mapping");
        assert_eq!(diagnostic.fixes[0].kind, "fix_protocol_impl_mapping");
    }

    #[test]
    fn protocol_declaration_rules_reject_bodies_and_free_default_markers() {
        let source = rsscript_syntax::parse_source(
            "protocol.rss",
            r#"
protocol Writer {
    fn write(self: read Self) -> Unit { return }
}
fn helper() -> Unit = _
"#,
        );
        let names = source
            .protocols
            .iter()
            .map(|protocol| protocol.name.clone())
            .collect();
        let diagnostics = protocol_declaration_diagnostics(&source.items, &names);
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].label, "unsupported protocol method body");
        assert_eq!(
            diagnostics[1].label,
            "unsupported default implementation marker"
        );
    }

    #[test]
    fn protocol_method_names_only_include_the_requested_protocol() {
        let source = rsscript_syntax::parse_source(
            "protocols.rss",
            r#"
protocol Writer { fn write(self: read Self) -> Unit }
protocol Reader { fn read(self: read Self) -> Unit }
"#,
        );
        assert_eq!(
            protocol_method_names(&source.items, "Writer"),
            HashSet::from(["write".to_owned()])
        );
    }
}
