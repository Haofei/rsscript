//! Guards the committed VS Code grammar against drift from the lexer keyword
//! tables. If this fails, run `cargo run --bin gen-grammar`.

use std::path::Path;

#[test]
fn vscode_grammar_is_up_to_date() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rsscript_sdk::VSCODE_GRAMMAR_PATH);
    let committed = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let generated = rsscript_sdk::vscode_tmlanguage_json();

    assert_eq!(
        committed,
        generated,
        "\n{} is stale relative to the lexer keyword tables.\n\
         Regenerate it with: cargo run --bin gen-grammar\n",
        rsscript_sdk::VSCODE_GRAMMAR_PATH
    );
}
