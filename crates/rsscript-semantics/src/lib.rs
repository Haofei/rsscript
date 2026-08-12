//! Platform-neutral semantic model shared by lowering backends.
//!
//! This crate deliberately has no runtime, provider, deployment-policy, or
//! review dependencies.

use std::collections::BTreeSet;

pub use rsscript_abi_model::{
    DataEffect, ExternalImport, ExternalSymbol, FunctionSignature, InvalidExternalSymbol,
    ParameterSignature, SignatureHash,
};

mod await_placement;
mod call_arguments;
mod call_binding;
mod control_flow;
mod database;
mod declarations;
mod derive_fields;
mod derives;
mod external_types;
mod generic_constraints;
pub mod hir;
mod identities;
mod interface_descriptor;
mod literals;
mod protocol_bounds;
mod resource_types;
mod signatures;
mod source_rules;
mod symbols;
mod try_checks;
mod type_aliases;
mod types;
mod weak_fields;
pub use await_placement::{
    AwaitLiveValueFact, async_call_consumption_diagnostic, await_live_value_diagnostics,
    await_operand_diagnostic, await_placement_diagnostics,
};
pub use call_arguments::{
    CallArgumentFact, CallParameterFact, ReceiverCallEffectFact, call_argument_diagnostics,
    receiver_call_effect_diagnostics,
};
pub use call_binding::{BoundArgument, BoundArgumentSource, CallBinding, CallBindingIssue};
pub use control_flow::{
    bool_condition_diagnostic, for_iterable_diagnostic, function_fallthrough_diagnostics,
    managed_pattern_field_effect_diagnostic, match_expression_arm_type_diagnostics,
    match_guard_mutation_diagnostic, match_literal_type_diagnostic, match_pattern_type_diagnostic,
    match_scrutinee_diagnostic, match_variant_family_diagnostic, missing_return_value_diagnostics,
    structured_match_effect_diagnostic, variant_pattern_arity_diagnostic,
    weakened_pattern_field_effect_diagnostic,
};
pub use database::{
    AnalysisResult, CompilationSession, CompilationSessionStats, FrontendCompletion,
    FrontendStopReason, SemanticDatabase, SessionSourceStore, SourceFileSnapshot, SourceSnapshot,
    SourceStoreError, SourceUpdate, ValidatedProgram,
};
pub use declarations::{
    duplicate_declaration_diagnostics, unknown_binding_diagnostics, unknown_field_diagnostics,
};
pub use derive_fields::derive_field_diagnostics;
pub use derives::derive_syntax_diagnostics;
pub use external_types::{external_binding_type_diagnostics, unknown_type_diagnostics};
pub use generic_constraints::generic_constraint_diagnostics;
pub use literals::{
    char_literal_scalar_diagnostic, integer_literal_range_diagnostic,
    match_char_literal_scalar_diagnostic,
};
pub use try_checks::{try_error_type_diagnostics, try_operand_diagnostic};
pub use weak_fields::{is_weak_upgrade_call, weak_field_upgrade_diagnostic};

/// Builtin source type roots recognized before backend lowering.
pub const BUILTIN_TYPE_NAMES: &[&str] = &[
    "Unit",
    "Bool",
    "Byte",
    "Char",
    "Int",
    "Int8",
    "Int16",
    "Int32",
    "Int64",
    "UInt",
    "UInt8",
    "UInt16",
    "UInt32",
    "UInt64",
    "Float",
    "Float32",
    "Float64",
    "String",
    "StringView",
    "Url",
    "Fd",
    "Bytes",
    "BytesView",
    "Buffer",
    "BufferView",
    "Path",
    "Result",
    "Option",
    "List",
    "Map",
    "Set",
    "Dyn",
    "Fn",
    "Closure",
    "FileError",
    "IOError",
    "HttpError",
    "JsonError",
    "CsvError",
    "NetworkError",
];

pub fn is_builtin_type_name(name: &str) -> bool {
    BUILTIN_TYPE_NAMES.contains(&name)
}

/// Source value identifiers supplied by the language rather than a lexical
/// declaration. Kept beside builtin type identity so frontend clients share
/// the same unresolved-binding boundary.
pub fn is_builtin_value_ident(name: &str) -> bool {
    matches!(name, "true" | "false" | "Unit" | "None" | "null")
}
pub use identities::DefinitionId;
pub use interface_descriptor::{
    INTERFACE_DESCRIPTOR_SCHEMA, InterfaceDescriptorError, InterfaceDescriptorFunctionV1,
    InterfaceDescriptorResourceV1, InterfaceDescriptorV1,
};
pub use protocol_bounds::{protocol_bound_diagnostics, unknown_protocol_diagnostic};
pub use resource_types::{
    fd_surface_diagnostics, resource_field_diagnostics, resource_generic_diagnostics,
    weak_field_diagnostics,
};
pub use rsscript_source_model::{FileId, InterfaceId, ModuleId, SourceRevision};
pub use signatures::signature_diagnostics;
pub use source_rules::forbidden_surface_syntax_diagnostics;
pub use symbols::{
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    document_symbols, symbol_index,
};
pub use type_aliases::cyclic_type_alias_diagnostics;
pub use types::{
    ResolvedParamEffect, ResolvedType, ResolvedTypeKind, SemanticTypeFacts, TypeArena, TypeId,
    TypeQualifiers,
};
pub(crate) use types::{
    builtin_generic_type_params, substitute_type_args, type_arg_names, type_root_name,
};

/// Structured retention facts attached to a callable signature.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetainedParams(BTreeSet<String>);

impl RetainedParams {
    pub fn insert(&mut self, parameter: impl Into<String>) -> bool {
        self.0.insert(parameter.into())
    }

    pub fn contains(&self, parameter: &str) -> bool {
        self.0.contains(parameter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_symbols_are_platform_neutral_names() {
        assert_eq!(
            ExternalSymbol::new("Host.emit").unwrap().as_str(),
            "Host.emit"
        );
        assert!(ExternalSymbol::new("Host..emit").is_err());
    }
}
