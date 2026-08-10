//! Declaration-identity diagnostics owned by the semantic layer.
//!
//! These checks consume the resolved HIR inventory rather than compiler
//! orchestration state, so every frontend client can obtain the same duplicate
//! declaration facts and source spans.

use crate::hir::{DuplicateSymbolKind, Hir};
use rsscript_diagnostics::{Diagnostic, code};

/// Derive stable duplicate-declaration diagnostics from a resolved HIR symbol
/// inventory. The HIR lowerer records the first and duplicate spans while it
/// constructs callable, type, constructor, and field namespaces.
pub fn duplicate_declaration_diagnostics(hir: &Hir) -> Vec<Diagnostic> {
    hir.duplicate_symbols()
        .iter()
        .map(|duplicate| {
            Diagnostic::error(
                code::DUPLICATE_DECLARATION,
                format!(
                    "{} `{}` is declared more than once.",
                    duplicate_symbol_label(duplicate.kind),
                    duplicate.name
                ),
                duplicate.duplicate_span.clone(),
                "duplicate declaration",
            )
            .with_cause(format!(
                "The first declaration is at {}:{}.",
                duplicate.first_span.line, duplicate.first_span.column
            ))
            .with_fix(
                "rename_declaration",
                "Rename or remove one declaration so the symbol table is unambiguous.",
                "manual",
            )
        })
        .collect()
}

fn duplicate_symbol_label(kind: DuplicateSymbolKind) -> &'static str {
    match kind {
        DuplicateSymbolKind::Function => "function",
        DuplicateSymbolKind::Type => "type",
        DuplicateSymbolKind::Constructor => "callable",
        DuplicateSymbolKind::Field => "field",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_keep_resolved_hir_identity_and_source_spans() {
        let program = rsscript_syntax::parse_source(
            "duplicate.rss",
            "fn same() -> Unit { return Unit }\nfn same() -> Unit { return Unit }\n",
        );
        let hir = Hir::from_syntax(&program);
        let diagnostics = duplicate_declaration_diagnostics(&hir);

        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, code::DUPLICATE_DECLARATION);
        assert_eq!(diagnostics[0].span.file, "duplicate.rss");
        assert_eq!(diagnostics[0].span.line, 2);
    }
}
