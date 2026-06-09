//! Doc-test guard for `AGENT.md`.
//!
//! `AGENT.md` is the in-context language guide a model reads as its *entire*
//! knowledge of RSScript, so a snippet with a syntax error teaches the model
//! wrong with full authority (see blog post 6). This test extracts every
//! ` ```rust ` block and asserts it is syntactically valid RSScript.
//!
//! Scope is deliberately *syntax* validity, not full type checking: many
//! snippets are illustrative fragments that reference types like `Image` or
//! `Request` which don't exist in core. The only diagnostic the syntax pass
//! emits is `RS0015` (unsupported syntax), which is exactly the class of bug
//! this guards against — e.g. teaching `loop <cond> {}` when the real
//! conditional loop is `while <cond> {}`.
//!
//! Authoring rule the guard enforces: a fenced ` ```rust ` block must be either
//! all top-level declarations or all statements (not a mix of both).

use std::path::Path;

fn extract_rust_blocks(src: &str) -> Vec<(usize, String)> {
    let mut blocks = Vec::new();
    let mut in_block = false;
    let mut start_line = 0;
    let mut current = String::new();
    for (index, line) in src.lines().enumerate() {
        if !in_block && line.trim_start().starts_with("```rust") {
            in_block = true;
            start_line = index + 1;
            current.clear();
        } else if in_block && line.trim_start().starts_with("```") {
            in_block = false;
            blocks.push((start_line, current.clone()));
        } else if in_block {
            current.push_str(line);
            current.push('\n');
        }
    }
    blocks
}

/// Replace `{ ... }` documentation elisions with an empty block.
fn normalize_elisions(block: &str) -> String {
    let mut out = block.to_string();
    while let Some(start) = out.find("{ ... }") {
        out.replace_range(start..start + "{ ... }".len(), "{ }");
    }
    out.replace("...", "")
}

/// A snippet is valid if it parses with no `RS0015`, either as top-level items
/// or wrapped as a function body (to accept statement/expression fragments).
fn block_has_syntax_error(block: &str) -> bool {
    let normalized = normalize_elisions(block);
    let unsupported = |source: &str| {
        rsscript::analyze_syntax_source("AGENT.md", source)
            .iter()
            .any(|diagnostic| diagnostic.code == "RS0015")
    };
    if !unsupported(&normalized) {
        return false;
    }
    let wrapped = format!("fn doc_harness() -> Unit {{\n{normalized}\n    return Ok(Unit)\n}}\n");
    unsupported(&wrapped)
}

#[test]
fn agent_md_rust_snippets_are_valid_syntax() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("AGENT.md");
    let source = std::fs::read_to_string(&path).expect("AGENT.md should read");
    let blocks = extract_rust_blocks(&source);
    assert!(!blocks.is_empty(), "no ```rust blocks found in AGENT.md");

    let failures: Vec<String> = blocks
        .iter()
        .filter(|(_, block)| block_has_syntax_error(block))
        .map(|(line, block)| {
            format!(
                "AGENT.md:{line} — {:?}",
                block.lines().next().unwrap_or("").trim()
            )
        })
        .collect();

    assert!(
        failures.is_empty(),
        "{} AGENT.md snippet(s) are not valid RSScript syntax:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn guard_rejects_invalid_syntax() {
    // The bug class this guard exists to catch: a syntax form that doesn't exist.
    assert!(
        block_has_syntax_error("loop some_condition { }\n"),
        "guard must reject `loop <cond>` — the conditional loop keyword is `while`, not `loop`"
    );
    assert!(
        block_has_syntax_error("fn broken( -> Int {\n    return 1\n}\n"),
        "guard must reject a malformed signature"
    );
}
