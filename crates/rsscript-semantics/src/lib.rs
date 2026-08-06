//! Platform-neutral semantic model shared by lowering backends.
//!
//! This crate deliberately has no runtime, provider, deployment-policy, or
//! review dependencies.

use std::collections::BTreeSet;

pub use rsscript_abi_model::{
    DataEffect, ExternalImport, ExternalSymbol, FunctionSignature, InvalidExternalSymbol,
    ParameterSignature, SignatureHash,
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
