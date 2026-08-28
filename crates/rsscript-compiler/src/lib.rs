#![forbid(unsafe_code)]

/// Build fingerprint used by external composition roots to invalidate compiler
/// output caches whenever the implementation inputs change.
pub const COMPILED_CACHE_FINGERPRINT: &str = env!("RSSCRIPT_COMPILED_CACHE_FINGERPRINT");

mod analyzer {
    //! Transitional compiler-local path for callers that have not yet moved to
    //! the semantic-owned frontend query API.
    pub(crate) use rsscript_semantics::*;
}
#[cfg(feature = "lowering")]
mod compiler_output;
mod core_index;
mod diagnostic {
    pub use rsscript_diagnostics::*;
}
mod editor_grammar;
mod generate;
#[cfg(all(test, feature = "selfhost-parity"))]
mod interface_metadata;
mod interfaces;
mod lexer {
    pub(crate) use rsscript_syntax::lexer::*;
}
#[cfg(all(test, feature = "selfhost-parity"))]
mod selfhost_parity;
mod symbols;
pub mod syntax;
#[cfg(all(test, feature = "selfhost-parity"))]
mod test_interfaces;
#[allow(dead_code)]
mod text_util {
    #[allow(unused_imports)]
    pub(crate) use rsscript_text::*;
}
#[cfg(all(test, feature = "selfhost-parity"))]
mod vm_adapter {
    use rsscript_vm::{EvalError, RegVmExecutable};

    pub(crate) fn reg_vm_compile_sources(
        sources: &[(&str, &str)],
    ) -> Result<RegVmExecutable, EvalError> {
        let interfaces = crate::interfaces::standard_package_interfaces().collect::<Vec<_>>();
        let validated = crate::analyzer::validate_sources_with_interfaces(sources, &interfaces)
            .map_err(EvalError::Diagnostics)?;
        let snapshot_digest = format!("sha256:{}", "0".repeat(64));
        let artifact =
            crate::compiler_output::compile_validated_to_bytecode(&validated, &snapshot_digest)
                .map_err(|error| EvalError::Runtime(error.to_string()))?;
        let bytes = artifact
            .to_bytes()
            .map_err(|error| EvalError::Runtime(error.to_string()))?;
        let verified = rsscript_bytecode::BytecodeVerifier::default()
            .verify(&bytes)
            .map_err(|error| EvalError::Runtime(error.to_string()))?;
        RegVmExecutable::from_verified_bytecode(verified)
    }
}

#[cfg(all(test, feature = "selfhost-parity"))]
use rsscript_vm::RegVmExecutable;

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
    CommitBehavior, Completion, CompletionKind, ContinuationOptions, Continuations, Effect,
    ExpectedType, GenerateContext, LiteralClass, PrefixStatus, SymbolCompleteness, TextRange,
    TypeRef, prefix_status, valid_continuations,
};
pub use rsscript_semantics::{
    analyze_frontend_input_snapshot_with_operation, analyze_source, analyze_source_result,
    analyze_source_result_with_operation, analyze_source_with_core, analyze_source_with_interfaces,
    analyze_source_with_interfaces_result, analyze_source_with_interfaces_result_with_operation,
    analyze_source_with_interfaces_without_core, analyze_source_without_core,
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
    SemanticDatabase, SourceFileSnapshot, SourceSnapshot, ValidatedProgram,
};
pub use symbols::{
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    document_symbols, symbol_index,
};
