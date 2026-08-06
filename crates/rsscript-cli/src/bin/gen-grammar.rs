//! Regenerates the VS Code TextMate grammar from the lexer keyword tables.
//!
//! Run after changing keywords in `src/lexer.rs`:
//!
//! ```text
//! cargo run --bin gen-grammar
//! ```

use std::path::Path;

fn main() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rsscript::VSCODE_GRAMMAR_PATH);
    let json = rsscript::vscode_tmlanguage_json();
    std::fs::write(&path, &json).unwrap_or_else(|error| {
        panic!("failed to write {}: {error}", path.display());
    });
    println!("wrote {}", path.display());
}
