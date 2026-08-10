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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_only_unresolved_protocol_bounds() {
        let source =
            rsscript_syntax::parse_source("bounds.rss", "struct Box<T: Missing> { value: T }\n");
        assert_eq!(protocol_bound_diagnostics(&[], &source).len(), 1);
    }
}
