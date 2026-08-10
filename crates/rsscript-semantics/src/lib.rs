//! Platform-neutral semantic model shared by lowering backends.
//!
//! This crate deliberately has no runtime, provider, deployment-policy, or
//! review dependencies.

use std::collections::BTreeSet;

pub use rsscript_abi_model::{
    DataEffect, ExternalImport, ExternalSymbol, FunctionSignature, InvalidExternalSymbol,
    ParameterSignature, SignatureHash,
};

mod call_binding;
mod database;
mod declarations;
mod derives;
mod external_types;
pub mod hir;
mod identities;
mod interface_descriptor;
mod resource_types;
mod source_rules;
mod symbols;
mod type_aliases;
mod types;
pub use call_binding::{BoundArgument, BoundArgumentSource, CallBinding, CallBindingIssue};
pub use database::{
    AnalysisResult, CompilationSession, CompilationSessionStats, FrontendCompletion,
    FrontendStopReason, SemanticDatabase, SessionSourceStore, SourceFileSnapshot, SourceSnapshot,
    SourceStoreError, SourceUpdate, ValidatedProgram,
};
pub use declarations::{
    duplicate_declaration_diagnostics, unknown_binding_diagnostics, unknown_field_diagnostics,
};
pub use derives::derive_syntax_diagnostics;
pub use external_types::{external_binding_type_diagnostics, unknown_type_diagnostics};

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
pub use resource_types::{
    fd_surface_diagnostics, resource_field_diagnostics, weak_field_diagnostics,
};
pub use rsscript_source_model::{FileId, InterfaceId, ModuleId, SourceRevision};
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
