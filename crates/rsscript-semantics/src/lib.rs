//! Platform-neutral semantic model shared by lowering backends.
//!
//! This crate deliberately has no runtime, provider, deployment-policy, or
//! review dependencies.

use std::collections::BTreeSet;

/// Stable identity of a function supplied by an external package provider.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ExternalSymbol(String);

impl ExternalSymbol {
    pub fn new(symbol: impl Into<String>) -> Result<Self, InvalidExternalSymbol> {
        let symbol = symbol.into();
        if symbol.is_empty()
            || symbol.starts_with('.')
            || symbol.ends_with('.')
            || symbol.split('.').any(|part| {
                part.is_empty()
                    || !part
                        .chars()
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            })
        {
            return Err(InvalidExternalSymbol);
        }
        Ok(Self(symbol))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidExternalSymbol;

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
