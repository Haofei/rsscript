//! Provider-independent lowering records.
//!
//! Provider selection happens after compilation, so the lowered instruction
//! contains only a semantic symbol and value locations.

use rsscript_semantics::ExternalSymbol;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalCall {
    pub symbol: ExternalSymbol,
    pub arguments: Box<[u32]>,
    pub destination: u32,
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
