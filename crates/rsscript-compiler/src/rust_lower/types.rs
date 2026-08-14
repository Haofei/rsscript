use crate::diagnostic::{Diagnostic, Span};

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
    /// The enclosing RSScript source symbol (e.g. `helpers.count`), stamped per
    /// function so a remapped backend error can name the declaration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    /// The lowered Rust symbol the enclosing function lowers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lowered_symbol: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemappedRustcDiagnostic {
    pub diagnostic: Diagnostic,
    pub mapped: bool,
}
