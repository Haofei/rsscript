//! Type-name, field-shape, and resource-type validation.
//!
//! Kept as a named phase so the self-hosted checker can mirror this boundary
//! without coupling its partial AST to Rust implementation details.

use std::collections::{BTreeMap, BTreeSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::syntax::ast::{Item, TypeRef};

struct AliasCycleDefinition {
    parameters: BTreeSet<String>,
    target: TypeRef,
}

pub(crate) fn check_names(analyzer: &mut Analyzer<'_>) {
    check_alias_cycles(analyzer);
    analyzer.check_unknown_types();
    analyzer.check_unknown_fields();
    analyzer.check_unknown_bindings();
    analyzer.check_fd_surface();
}

fn check_alias_cycles(analyzer: &mut Analyzer<'_>) {
    let aliases = analyzer
        .interface_programs
        .iter()
        .flat_map(|program| program.items.iter())
        .chain(analyzer.syntax_program.items.iter())
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
    let declared = analyzer
        .interface_programs
        .iter()
        .flat_map(|program| program.items.iter())
        .chain(analyzer.syntax_program.items.iter())
        .filter_map(|item| match item {
            Item::TypeAlias(alias) => Some((
                alias.name.clone(),
                alias.span.clone(),
                alias_cycle(&alias.name, &aliases, &mut Vec::new()),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();

    for (name, span, cycle) in declared {
        let Some(cycle) = cycle else {
            continue;
        };
        analyzer.diagnostics.push(
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
            ),
        );
    }
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

pub(crate) fn check_resource_shapes(analyzer: &mut Analyzer<'_>) {
    analyzer.check_resource_fields();
    analyzer.check_weak_fields();
    analyzer.check_resource_pool_type_arguments();
    analyzer.check_resource_generic_arguments();
}
