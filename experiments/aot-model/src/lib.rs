//! Data contracts owned by the experimental Rust/AOT backend.
//!
//! The Core compiler only reaches this crate behind its explicit `aot-rust`
//! feature. These types are not part of the reviewed SDK surface.

use rsscript_diagnostics::{Diagnostic, Span};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedRustPackage {
    pub package_name: String,
    pub cargo_toml: String,
    pub lib_rs: String,
    pub main_rs: Option<String>,
    pub source_map_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoweredRust {
    pub rust_source: String,
    pub source_map: Vec<RustSourceMapEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RustSourceMapEntry {
    pub kind: String,
    pub source: Span,
    pub generated: Span,
    /// The enclosing RSScript source symbol, stamped per function so a backend
    /// error can name the declaration it maps to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// The Rust symbol produced for the enclosing RSScript function.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lowered_symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemappedRustcDiagnostic {
    pub diagnostic: Diagnostic,
    pub mapped: bool,
}

#[cfg(test)]
mod tests {
    use super::RustSourceMapEntry;
    use rsscript_diagnostics::Span;

    #[test]
    fn source_map_contract_round_trips() {
        let entry = RustSourceMapEntry {
            kind: "call".to_string(),
            source: Span {
                file: "main.rss".to_string(),
                line: 2,
                column: 3,
                length: 4,
            },
            generated: Span {
                file: "src/lib.rs".to_string(),
                line: 8,
                column: 5,
                length: 9,
            },
            symbol: Some("main".to_string()),
            lowered_symbol: Some("rss_main".to_string()),
        };
        let encoded = serde_json::to_string(&entry).expect("model must serialize");
        let decoded: RustSourceMapEntry =
            serde_json::from_str(&encoded).expect("model must deserialize");
        assert_eq!(decoded, entry);
    }
}
