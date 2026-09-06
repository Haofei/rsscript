#![forbid(unsafe_code)]

/// Build fingerprint used by external composition roots to invalidate compiler
/// output caches whenever the implementation inputs change.
pub const COMPILED_CACHE_FINGERPRINT: &str = env!("RSSCRIPT_COMPILED_CACHE_FINGERPRINT");

#[cfg(feature = "lowering")]
mod compiler_output;
mod core_index;
mod diagnostic {
    pub use rsscript_diagnostics::*;
}
mod editor_grammar;
mod generate;
mod interfaces;
mod lexer {
    pub(crate) use rsscript_syntax::lexer::*;
}
mod symbols;
pub mod syntax;

#[cfg(feature = "bytecode")]
pub use compiler_output::{
    BytecodeCompileError, compile_ir_to_bytecode, compile_validated_to_bytecode,
};
#[cfg(feature = "lowering")]
pub use compiler_output::{
    CompiledIr, compile_frontend_input_to_ir, compile_source_to_ir, compile_validated_to_ir,
};
pub use core_index::core_package_index_json;
pub use diagnostic::{
    Diagnostic, DiagnosticExplanation, Fix, FixEdit, Severity, Span, explain_diagnostic_code,
    format_diagnostic_explanation, format_diagnostics_human, format_diagnostics_json,
    format_diagnostics_json_with_source,
};
pub use editor_grammar::{VSCODE_GRAMMAR_PATH, vscode_tmlanguage_json};
pub use generate::{
    Completeness, Completion, CompletionKind, ContinuationOptions, Continuations, Effect,
    ExpectedType, GenerateContext, GenerationCheckpoint, GenerationCoreInterfacePolicy,
    GenerationInterfaceSetSnapshot, GenerationInterfaceSnapshot, GenerationQueryIdentity,
    GenerationQuerySnapshot, GenerationRestoreError, GenerationSession, GenerationSessionStats,
    IdentifierRoleName, LiteralKindName, ParserTerminal, PrefixStatus, SemanticValidity, TextRange,
    TypeRef, valid_continuations,
};
pub use rsscript_semantics::{
    analyze_frontend_input_snapshot_with_operation, analyze_source, analyze_source_result,
    analyze_source_result_with_operation, analyze_source_with_core, analyze_source_with_interfaces,
    analyze_source_with_interfaces_result, analyze_source_with_interfaces_result_with_operation,
    analyze_source_with_interfaces_without_core,
    analyze_source_with_interfaces_without_core_result, analyze_source_without_core,
    analyze_sources_with_interfaces, analyze_sources_with_interfaces_result,
    analyze_sources_with_interfaces_result_with_operation,
    analyze_sources_with_interfaces_without_core,
    analyze_sources_with_interfaces_without_core_result, analyze_syntax_source, core_interfaces,
    standard_package_interfaces, validate_source, validate_source_with_operation,
    validate_sources_with_interfaces, validate_sources_with_interfaces_with_operation,
    validate_sources_with_interfaces_without_core,
};
pub use rsscript_syntax::{format_program, format_source, lint_source};

pub use rsscript_semantics::{
    AnalysisResult, FrontendCompletion, FrontendInputSnapshot, FrontendStopReason,
    SemanticDatabase, SessionInterfacePolicy, SourceFileSnapshot, SourceSnapshot, ValidatedProgram,
};
pub use symbols::{
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    document_symbols, symbol_index,
};
