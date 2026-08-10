//! Type-alias graph diagnostics owned by the semantic layer.
//!
//! Alias cycles are source-level semantic facts. They are derived from an
//! immutable program snapshot and do not require compiler orchestration,
//! lowering, or a runtime backend.

use std::collections::{BTreeMap, BTreeSet};

use rsscript_diagnostics::{Diagnostic, code};
use rsscript_syntax::ast::{Item, Program, TypeRef};

struct AliasCycleDefinition {
    parameters: BTreeSet<String>,
    target: TypeRef,
}

/// Derive diagnostics for cyclic type aliases across interfaces and source.
///
/// Interface programs are evaluated before the source program so the result is
/// independent of compiler pass orchestration. A generic parameter shadows an
/// alias of the same spelling inside its declaration.
pub fn cyclic_type_alias_diagnostics(
    interface_programs: &[Program],
    source_program: &Program,
) -> Vec<Diagnostic> {
    let aliases = interface_programs
        .iter()
        .flat_map(|program| program.items.iter())
        .chain(source_program.items.iter())
        .filter_map(|item| match item {
            Item::TypeAlias(alias) => Some((
                alias.name.clone(),
                AliasCycleDefinition {
                    parameters: alias
                        .type_params
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect(),
                    target: alias.target.clone(),
                },
            )),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();

    interface_programs
        .iter()
        .flat_map(|program| program.items.iter())
        .chain(source_program.items.iter())
        .filter_map(|item| match item {
            Item::TypeAlias(alias) => Some((
                alias.name.clone(),
                alias.span.clone(),
                alias_cycle(&alias.name, &aliases, &mut Vec::new()),
            )),
            _ => None,
        })
        .filter_map(|(name, span, cycle)| {
            cycle.map(|cycle| {
                Diagnostic::error(
                    code::CYCLIC_TYPE_ALIAS,
                    format!("Type alias `{name}` is cyclic."),
                    span,
                    "cyclic type alias",
                )
                .with_cause(format!("Alias expansion repeats: {}.", cycle.join(" -> ")))
                .with_fix(
                    "break_type_alias_cycle",
                    "Change at least one alias target to a non-cyclic concrete type.",
                    "manual",
                )
            })
        })
        .collect()
}

fn alias_cycle(
    name: &str,
    aliases: &BTreeMap<String, AliasCycleDefinition>,
    stack: &mut Vec<String>,
) -> Option<Vec<String>> {
    if let Some(index) = stack.iter().position(|candidate| candidate == name) {
        let mut cycle = stack[index..].to_vec();
        cycle.push(name.to_string());
        return Some(cycle);
    }
    let definition = aliases.get(name)?;
    stack.push(name.to_string());
    for dependency in alias_dependencies(&definition.target, aliases, &definition.parameters) {
        if let Some(cycle) = alias_cycle(&dependency, aliases, stack) {
            return Some(cycle);
        }
    }
    stack.pop();
    None
}

fn alias_dependencies(
    ty: &TypeRef,
    aliases: &BTreeMap<String, AliasCycleDefinition>,
    bound_parameters: &BTreeSet<String>,
) -> Vec<String> {
    let mut dependencies = Vec::new();
    if !bound_parameters.contains(&ty.name) && aliases.contains_key(&ty.name) {
        dependencies.push(ty.name.clone());
    }
    for argument in &ty.args {
        dependencies.extend(alias_dependencies(argument, aliases, bound_parameters));
    }
    for parameter in &ty.fn_params {
        dependencies.extend(alias_dependencies(parameter, aliases, bound_parameters));
    }
    if let Some(return_type) = &ty.fn_return {
        dependencies.extend(alias_dependencies(return_type, aliases, bound_parameters));
    }
    dependencies
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_a_cycle_with_the_declared_alias_span() {
        let source = rsscript_syntax::parse_source(
            "aliases.rss",
            "type First = Second\ntype Second = First\n",
        );
        let diagnostics = cyclic_type_alias_diagnostics(&[], &source);

        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].code, code::CYCLIC_TYPE_ALIAS);
        assert_eq!(diagnostics[0].span.file, "aliases.rss");
        assert_eq!(diagnostics[0].span.line, 1);
        assert!(
            diagnostics[0]
                .causes
                .iter()
                .any(|cause| cause.contains("First -> Second -> First"))
        );
    }

    #[test]
    fn generic_parameter_does_not_create_an_alias_edge() {
        let source = rsscript_syntax::parse_source("generic.rss", "type Loop<Loop> = Loop\n");
        assert!(cyclic_type_alias_diagnostics(&[], &source).is_empty());
    }
}
