use std::collections::{BTreeMap, BTreeSet};

use crate::diagnostic::Span;
use crate::semantic::{ResolvedType, SemanticTypeFacts};
use crate::syntax::ast::{
    BinaryOp, Block, CallArg, Callee, DataEffect, Expr, FieldDecl, ForStmt, GenericParam, Item,
    LetStmt, MatchPattern, MatchStmt, Param, Program, Stmt, TypeKind, TypeRef,
};

use super::helpers::*;
use super::intrinsics::*;
use super::types::{LoweredRust, RustSourceMapEntry};

mod expressions;
mod ownership;
mod program;
mod runtime_intrinsics;
mod structured_types;
mod support;

pub(in crate::rust_lower) use support::*;

pub(super) struct RustLowerer<'a> {
    pub(super) program: &'a Program,
    pub(super) semantic_types: Option<&'a SemanticTypeFacts>,
    pub(super) type_kinds: BTreeMap<String, TypeKind>,
    pub(super) type_fields: BTreeMap<String, Vec<FieldDecl>>,
    pub(super) type_params: BTreeMap<String, Vec<String>>,
    pub(super) type_aliases: BTreeMap<String, (Vec<String>, TypeRef)>,
    pub(super) protocol_names: BTreeSet<String>,
    pub(super) external_boundary_callees: BTreeSet<String>,
    pub(super) async_external_boundary_callees: BTreeSet<String>,
    pub(super) external_bindings: BTreeMap<String, String>,
    pub(super) function_return_types: BTreeMap<String, TypeRef>,
    pub(super) function_type_params: BTreeMap<String, Vec<String>>,
    pub(super) function_param_types: BTreeMap<String, Vec<(String, TypeRef)>>,
    pub(super) function_param_defaults: BTreeMap<String, Vec<Option<Expr>>>,
    pub(super) function_param_default_helpers: BTreeMap<String, Vec<Option<String>>>,
    pub(super) const_names: BTreeSet<String>,
    pub(super) function_param_effects: BTreeMap<String, Vec<(String, Option<DataEffect>)>>,
    pub(super) retained_params_by_callee: BTreeMap<String, BTreeSet<String>>,
    pub(super) param_effects: BTreeMap<String, DataEffect>,
    pub(super) value_types: BTreeMap<String, TypeRef>,
    pub(super) managed_bindings: BTreeSet<String>,
    pub(super) read_view_bindings: BTreeSet<String>,
    pub(super) current_retained_params: BTreeSet<String>,
    pub(super) mutated_bindings: BTreeSet<String>,
    pub(super) drop_field_names: BTreeSet<String>,
    pub(super) current_return_type: Option<TypeRef>,
    pub(super) current_async_executor: Option<String>,
    /// Name of the enclosing `task_group`'s cancellation guard local, if any, so
    /// `Task.cancellation_token()` resolves to that scope's token.
    pub(super) current_task_group_token: Option<String>,
    pub(super) source_map: Vec<RustSourceMapEntry>,
    /// Maps generic type param name -> protocol bound name for receiver-call resolution
    pub(super) generic_protocol_bounds: BTreeMap<String, String>,
    pub(super) call_temp_counter: usize,
    pub(super) lowering_default: bool,
}

pub(super) struct AsyncTaskGroupBoundary {
    pub(super) pending: String,
    pub(super) returns_result: bool,
}

impl<'a> RustLowerer<'a> {
    pub(super) fn new(
        program: &'a Program,
        external_bindings: BTreeMap<String, String>,
        interface_programs: &[Program],
    ) -> Self {
        // Type kinds (struct/class/resource) for `is_class_type`/`is_resource_type`,
        // constructor lowering, etc. Built from the current program *and* dependency
        // interfaces, so a class/resource/struct defined in another package and
        // constructed/held in this source is classified correctly (otherwise it
        // falls through to the unknown-type path and mis-lowers, e.g. a named-field
        // class constructed as a positional `Widget(1)` call — review #8).
        //
        // Bundled standard-library interface types are EXCLUDED because their
        // implementations are supplied by the AOT runtime rather than emitted as
        // local Rust structs. Only dependency interface types are ingested; the
        // current program wins on any conflict.
        let builtin_type_names = builtin_interface_type_names();
        let type_declarations = interface_programs
            .iter()
            .flat_map(|interface| interface.items.iter())
            .filter(|item| match item {
                Item::Type(ty) => !builtin_type_names.contains(ty.name.as_str()),
                _ => false,
            })
            .chain(program.items.iter())
            .filter_map(|item| match item {
                Item::Type(ty) => Some((
                    ty.name.clone(),
                    ty.kind,
                    ty.type_params
                        .iter()
                        .map(|parameter| parameter.name.clone())
                        .collect::<Vec<_>>(),
                    ty.fields.clone(),
                )),
                Item::SumType(_) => None,
                Item::Function(_)
                | Item::TypeAlias(_)
                | Item::Const(_)
                | Item::Module(_)
                | Item::Use(_) => None,
            })
            .collect::<Vec<_>>();
        let type_kinds = type_declarations
            .iter()
            .map(|(name, kind, _, _)| (name.clone(), *kind))
            .collect();
        let type_params = type_declarations
            .iter()
            .map(|(name, _, params, _)| (name.clone(), params.clone()))
            .collect();
        let type_fields = type_declarations
            .into_iter()
            .map(|(name, _, _, fields)| (name, fields))
            .collect();
        let type_aliases = interface_programs
            .iter()
            .flat_map(|interface| interface.items.iter())
            .chain(program.items.iter())
            .filter_map(|item| match item {
                Item::TypeAlias(alias) => Some((
                    alias.name.clone(),
                    (
                        alias
                            .type_params
                            .iter()
                            .map(|parameter| parameter.name.clone())
                            .collect(),
                        alias.target.clone(),
                    ),
                )),
                _ => None,
            })
            .collect();
        let external_boundary_callees =
            collect_external_boundary_callees(program, interface_programs);
        let async_external_boundary_callees =
            collect_async_external_boundary_callees(program, interface_programs);
        let protocol_names = program
            .protocols
            .iter()
            .map(|protocol| protocol.name.clone())
            .collect();
        let function_return_types = collect_function_return_types(program, interface_programs);
        let function_type_params = collect_function_type_params(program, interface_programs);
        let function_param_types = collect_function_param_types(program, interface_programs);
        let function_param_defaults = collect_function_param_defaults(program, interface_programs);
        let function_param_default_helpers = function_param_defaults
            .iter()
            .map(|(function, defaults)| {
                let base = rust_function_ident(function);
                (
                    function.clone(),
                    defaults
                        .iter()
                        .enumerate()
                        .map(|(index, default)| {
                            default
                                .as_ref()
                                .map(|_| format!("__rss_default_{base}_{index}"))
                        })
                        .collect(),
                )
            })
            .collect();
        let const_names = program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Const(decl) => Some(decl.name.clone()),
                _ => None,
            })
            .collect();
        let function_param_effects = collect_function_param_effects(program, interface_programs);
        let retained_params_by_callee =
            collect_function_retained_params(program, interface_programs);

        Self {
            program,
            semantic_types: None,
            type_kinds,
            type_fields,
            type_params,
            type_aliases,
            protocol_names,
            external_boundary_callees,
            async_external_boundary_callees,
            external_bindings,
            function_return_types,
            function_type_params,
            function_param_types,
            function_param_defaults,
            function_param_default_helpers,
            const_names,
            function_param_effects,
            retained_params_by_callee,
            param_effects: BTreeMap::new(),
            value_types: BTreeMap::new(),
            managed_bindings: BTreeSet::new(),
            read_view_bindings: BTreeSet::new(),
            current_retained_params: BTreeSet::new(),
            mutated_bindings: BTreeSet::new(),
            drop_field_names: BTreeSet::new(),
            current_return_type: None,
            current_async_executor: None,
            current_task_group_token: None,
            source_map: Vec::new(),
            generic_protocol_bounds: BTreeMap::new(),
            call_temp_counter: 0,
            lowering_default: false,
        }
    }

    pub(super) fn new_validated(
        program: &'a Program,
        executable: &'a rsscript_lowering::ExecutableIr,
        external_bindings: BTreeMap<String, String>,
        interface_programs: &[Program],
    ) -> Self {
        let semantic_types = executable.semantic_types();
        let mut lowerer = Self::new(program, external_bindings, interface_programs);
        let span = Span {
            file: "<semantic-type>".to_string(),
            line: 1,
            column: 1,
            length: 1,
        };
        lowerer
            .function_return_types
            .extend(semantic_types.functions().filter_map(|(name, facts)| {
                facts.return_type.map(|return_type| {
                    (
                        name.to_string(),
                        semantic_types.arena().get(return_type).to_type_ref(&span),
                    )
                })
            }));
        lowerer.function_type_params.extend(
            semantic_types
                .functions()
                .map(|(name, facts)| (name.to_string(), facts.type_parameters.to_vec())),
        );
        lowerer
            .function_param_types
            .extend(semantic_types.functions().map(|(name, facts)| {
                (
                    name.to_string(),
                    facts
                        .parameters
                        .iter()
                        .map(|(parameter, ty)| {
                            (
                                parameter.clone(),
                                semantic_types.arena().get(*ty).to_type_ref(&span),
                            )
                        })
                        .collect(),
                )
            }));
        lowerer.semantic_types = Some(semantic_types);
        lowerer
    }
}
