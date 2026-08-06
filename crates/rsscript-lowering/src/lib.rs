//! Provider-independent executable IR.
//!
//! The executable IR is the single checked input consumed by VM and optional
//! backends. Provider selection happens after compilation, so imports contain
//! only semantic symbols and signatures.

use rsscript_semantics::hir::Hir;
use rsscript_semantics::{ExternalSymbol, SemanticTypeFacts};

mod model;
pub use model::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCall {
    pub symbol: ExternalSymbol,
    pub arguments: Box<[u32]>,
    pub destination: u32,
}

/// An owned, validated, provider-neutral executable input.
#[derive(Debug, Clone)]
pub struct ExecutableIr {
    program: ExecutableProgram,
    external_imports: Box<[ExecutableExternalImport]>,
    semantic_types: std::sync::Arc<SemanticTypeFacts>,
}

impl ExecutableIr {
    pub fn from_validated_hir(typed_hir: &Hir) -> Self {
        let (program, external_imports) = model::project_hir(typed_hir);
        Self {
            program,
            external_imports,
            semantic_types: std::sync::Arc::new(typed_hir.semantic_types().clone()),
        }
    }

    pub fn program(&self) -> &ExecutableProgram {
        &self.program
    }

    pub fn functions(&self) -> impl Iterator<Item = &ExecutableFunction> {
        self.program.functions()
    }

    pub fn external_imports(&self) -> &[ExecutableExternalImport] {
        &self.external_imports
    }

    /// Structural semantic facts retained for the experimental source backend.
    /// The VM consumes only [`Self::program`].
    pub fn semantic_types(&self) -> &SemanticTypeFacts {
        &self.semantic_types
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_call_has_no_provider_identity() {
        let call = ExternalCall {
            symbol: ExternalSymbol::new("Host.emit").unwrap(),
            arguments: Box::new([1]),
            destination: 2,
        };
        assert_eq!(call.symbol.as_str(), "Host.emit");
    }
}
