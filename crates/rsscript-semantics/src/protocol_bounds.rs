//! Protocol-bound visibility diagnostics.

use std::collections::HashSet;

use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{GenericBound, Item, Program};

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
}
