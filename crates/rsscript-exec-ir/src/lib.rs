#![forbid(unsafe_code)]

//! Owned, provider-neutral executable IR shared by the compiler and backends.
//! This crate deliberately has no parser, semantic database, runtime, or
//! Provider dependency.

mod model;
pub use model::*;

use rsscript_abi_model::ExternalSymbol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCall {
    pub symbol: ExternalSymbol,
    pub arguments: Box<[u32]>,
    pub destination: u32,
}

#[derive(Debug, Clone)]
pub struct ExecutableIr {
    program: ExecutableProgram,
    external_imports: Box<[ExecutableExternalImport]>,
}

impl ExecutableIr {
    pub fn new(
        program: ExecutableProgram,
        external_imports: Box<[ExecutableExternalImport]>,
    ) -> Self {
        Self {
            program,
            external_imports,
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
