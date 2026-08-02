use std::collections::BTreeMap;

use crate::syntax::ast::{DataEffect, Item, Program, TypeRef};

use super::external_boundary_function_key;

pub(in crate::rust_lower) fn collect_function_type_params(
    program: &Program,
    interface_programs: &[Program],
) -> BTreeMap<String, Vec<String>> {
    let mut type_params = BTreeMap::new();
    for interface_program in interface_programs {
        collect_program_function_type_params(interface_program, &mut type_params);
    }
    collect_program_function_type_params(program, &mut type_params);
    type_params
}

pub(in crate::rust_lower) fn collect_function_param_types(
    program: &Program,
    interface_programs: &[Program],
) -> BTreeMap<String, Vec<(String, TypeRef)>> {
    let mut param_types = BTreeMap::new();
    for interface_program in interface_programs {
        collect_program_function_param_types(interface_program, &mut param_types);
    }
    collect_program_function_param_types(program, &mut param_types);
    param_types
}

pub(in crate::rust_lower) fn collect_function_param_effects(
    program: &Program,
    interface_programs: &[Program],
) -> BTreeMap<String, Vec<(String, Option<DataEffect>)>> {
    let mut param_effects = BTreeMap::new();
    for interface_program in interface_programs {
        collect_program_function_param_effects(interface_program, &mut param_effects);
    }
    collect_program_function_param_effects(program, &mut param_effects);
    param_effects
}

fn collect_program_function_param_effects(
    program: &Program,
    param_effects: &mut BTreeMap<String, Vec<(String, Option<DataEffect>)>>,
) {
    for item in &program.items {
        if let Item::Function(function) = item {
            param_effects.insert(
                function.name.clone(),
                function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.effective_effect()))
                    .collect(),
            );
        }
    }
}

fn collect_program_function_param_types(
    program: &Program,
    param_types: &mut BTreeMap<String, Vec<(String, TypeRef)>>,
) {
    for item in &program.items {
        if let Item::Function(function) = item {
            param_types.insert(
                function.name.clone(),
                function
                    .params
                    .iter()
                    .map(|param| (param.name.clone(), param.ty.clone()))
                    .collect(),
            );
        }
    }
}

/// Per-function ordered list of parameter default-value expressions (parallel to
/// the param-type list). Used to fill omitted trailing arguments at call sites,
/// since Rust has no default parameters.
pub(in crate::rust_lower) fn collect_function_param_defaults(
    program: &Program,
    interface_programs: &[Program],
) -> BTreeMap<String, Vec<Option<crate::syntax::ast::Expr>>> {
    let mut defaults = BTreeMap::new();
    for interface_program in interface_programs {
        collect_program_function_param_defaults(interface_program, &mut defaults);
    }
    collect_program_function_param_defaults(program, &mut defaults);
    defaults
}

fn collect_program_function_param_defaults(
    program: &Program,
    defaults: &mut BTreeMap<String, Vec<Option<crate::syntax::ast::Expr>>>,
) {
    for item in &program.items {
        if let Item::Function(function) = item {
            defaults.insert(
                function.name.clone(),
                function
                    .params
                    .iter()
                    .map(|param| param.default.clone())
                    .collect(),
            );
        }
    }
}

fn collect_program_function_type_params(
    program: &Program,
    type_params: &mut BTreeMap<String, Vec<String>>,
) {
    for item in &program.items {
        if let Item::Function(function) = item {
            type_params.insert(
                external_boundary_function_key(&function.name),
                function
                    .type_params
                    .iter()
                    .map(|param| param.name.clone())
                    .collect(),
            );
        }
    }
}
