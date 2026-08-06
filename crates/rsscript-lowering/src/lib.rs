//! Provider-independent executable IR.
//!
//! The executable IR is the single checked input consumed by VM and optional
//! backends. Provider selection happens after compilation, so imports contain
//! only semantic symbols and signatures.

use std::collections::BTreeSet;

use rsscript_semantics::hir::{CallResolution, Hir, HirFunctionBody};
use rsscript_semantics::{ExternalSymbol, SemanticTypeFacts};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCall {
    pub symbol: ExternalSymbol,
    pub arguments: Box<[u32]>,
    pub destination: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableFunction {
    pub name: String,
    pub is_async: bool,
}

/// An owned, validated, provider-neutral executable input.
///
/// The current instruction lowering still consumes the checked HIR projection,
/// but that projection is owned by this phase value. No backend borrows the
/// compiler database or depends on its lifetime. Subsequent instruction-model
/// extraction can therefore happen without changing the phase API.
#[derive(Debug, Clone)]
pub struct ExecutableIr {
    typed_hir: Hir,
    functions: Box<[ExecutableFunction]>,
    external_imports: Box<[ExternalSymbol]>,
}

impl ExecutableIr {
    pub fn from_validated_hir(typed_hir: &Hir) -> Self {
        let functions = typed_hir
            .function_bodies()
            .filter_map(|(name, body)| {
                body.block.as_ref().map(|_| ExecutableFunction {
                    name: name.to_string(),
                    is_async: typed_hir
                        .resolve_function(None, name)
                        .is_some_and(|signature| signature.is_async),
                })
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();

        let external_imports = typed_hir
            .call_sites()
            .iter()
            .filter_map(|call| match &call.resolution {
                CallResolution::Resolved { signature, .. } if signature.is_external => {
                    let symbol = signature.namespace.as_ref().map_or_else(
                        || signature.name.clone(),
                        |namespace| format!("{namespace}.{}", signature.name),
                    );
                    ExternalSymbol::new(symbol).ok()
                }
                _ => None,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .into_boxed_slice();

        Self {
            typed_hir: typed_hir.clone(),
            functions,
            external_imports,
        }
    }

    pub fn typed_hir(&self) -> &Hir {
        &self.typed_hir
    }

    pub fn semantic_types(&self) -> &SemanticTypeFacts {
        self.typed_hir.semantic_types()
    }

    pub fn functions(&self) -> &[ExecutableFunction] {
        &self.functions
    }

    pub fn function_body(&self, name: &str) -> Option<&HirFunctionBody> {
        self.typed_hir.function_body(name)
    }

    pub fn external_imports(&self) -> &[ExternalSymbol] {
        &self.external_imports
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
