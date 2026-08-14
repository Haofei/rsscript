//! Execution-only source-symbol inventory adapter.
//!
//! Editor navigation symbols are owned by `rsscript-semantics`. The compiler
//! retains only the Rust-lowering-specific inventory needed by execution tools.

pub use rsscript_semantics::{
    Definition, Reference, RssDocumentSymbol, SymbolIndex, SymbolInfo, SymbolKind, SymbolLookup,
    document_symbols, symbol_index,
};

/// Source-qualified declaration identity paired with the backend lowering name.
#[derive(Debug, Clone)]
#[cfg(feature = "execution")]
pub struct SymbolInventoryEntry {
    pub module: String,
    pub qualname: String,
    pub kind: SymbolKind,
    pub span: rsscript_diagnostics::Span,
    pub lowered_name: String,
}

/// Build the execution-only source inventory. Editor-facing symbol indexing is
/// deliberately delegated to `rsscript-semantics` above.
#[cfg(feature = "execution")]
pub fn symbol_inventory(file: &str, source: &str) -> Vec<SymbolInventoryEntry> {
    let module = std::path::Path::new(file)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file)
        .to_string();
    let overrides = crate::lower_names::collect_lower_name_overrides(
        &rsscript_syntax::parse_source_raw(file, source),
    );
    let previous = crate::lower_names::set_lower_name_overrides(overrides);
    let index = symbol_index(file, source);
    let entries = index
        .definitions()
        .iter()
        .filter(|definition| {
            matches!(
                definition.kind,
                SymbolKind::Function | SymbolKind::Type | SymbolKind::Const | SymbolKind::Variant
            )
        })
        .map(|definition| SymbolInventoryEntry {
            module: module.clone(),
            qualname: definition.name.clone(),
            kind: definition.kind,
            span: definition.span.clone(),
            lowered_name: crate::lower_names::lowered_symbol_name(&definition.name),
        })
        .collect();
    crate::lower_names::set_lower_name_overrides(previous);
    entries
}

#[cfg(all(test, feature = "execution"))]
mod tests {
    use super::*;

    #[test]
    fn inventory_preserves_module_identity_and_lowered_names() {
        let source = concat!(
            "const MAX_RETRIES: Int = 3\n",
            "struct Device { id: Int }\n",
            "fn Device.open(id: Int) -> fresh Device {\n",
            "    return Device(id: id)\n",
            "}\n",
        );
        let inventory = symbol_inventory("helpers.rss", source);
        assert!(inventory.iter().all(|entry| entry.module == "helpers"));
        let open = inventory
            .iter()
            .find(|entry| entry.qualname == "Device.open")
            .expect("member function inventory entry");
        assert_eq!(open.kind, SymbolKind::Function);
        assert_eq!(open.lowered_name, "Device_open");
    }

    #[test]
    fn inventory_applies_lower_name_pins() {
        let source = concat!(
            "#lower_name(\"helpers__count\")\n",
            "fn count(value: read Int) -> Int { return value }\n",
        );
        let inventory = symbol_inventory("helpers.rss", source);
        assert_eq!(
            inventory
                .iter()
                .find(|entry| entry.qualname == "count")
                .expect("count inventory entry")
                .lowered_name,
            "helpers__count"
        );
    }
}
