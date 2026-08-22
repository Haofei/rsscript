//! Never-panic contract for the RSScript front end.
//!
//! `input bytes -> lexer -> parser -> checker -> lowerer` must never panic,
//! hang, or stack-overflow on *any* input — valid or hostile. The checker owns
//! semantics, so lowering is only attempted when the checker reports no
//! error-severity diagnostics; that path must not panic either (a clean RSScript
//! diagnostic is fine, a Rust panic is a bug). libFuzzer's coverage feedback
//! drives the lexer/parser recovery paths far deeper than the random-string
//! proptest in `tests/hostile.rs`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rsscript_sdk::compile::{Compiler, Severity};

fuzz_target!(|data: &[u8]| {
    // RSScript source is text; non-UTF-8 input is the lexer's concern but the
    // public entry points take `&str`, so decode first and let invalid UTF-8 be
    // a no-op for this target (a dedicated bytes target could exercise the raw
    // lexer if it gains a byte-level entry point).
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };

    let diagnostics = Compiler.check("fuzz.rss", source);

    // Only feed the lowerer programs the checker accepts: invalid programs must
    // never reach code generation (the "RSScript owns semantics" contract).
    let has_errors = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);
    if !has_errors {
        let _ = Compiler.compile("fuzz.rss", source);
    }
});
