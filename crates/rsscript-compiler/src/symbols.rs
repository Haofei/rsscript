//! Execution-only source-symbol inventory adapter.
//!
//! Editor navigation symbols are owned by `rsscript-semantics`. The compiler
//! retains only the Rust-lowering-specific inventory needed by execution tools.

pub use rsscript_semantics::{
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    document_symbols, symbol_index,
};
