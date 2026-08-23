//! Formatter-stability metamorphic property under coverage-guided fuzzing.
//!
//! Reformatting must be idempotent: `format(format(x)) == format(x)`. A
//! formatter that keeps changing a fixed point is reordering or dropping tokens,
//! which is exactly the class of bug the metamorphic test
//! (`tests/metamorphic.rs`, `reformat` transform) checks on a fixed corpus —
//! here libFuzzer searches for a counterexample across arbitrary inputs.
//!
//! Only applied to inputs the checker accepts, so the formatter is exercised on
//! well-formed programs (formatting malformed input has no defined fixed point).

#![no_main]

use libfuzzer_sys::fuzz_target;
use rsscript_sdk::{compile::Compiler, compile::Severity, language::format_source};

fuzz_target!(|data: &[u8]| {
    let Ok(source) = std::str::from_utf8(data) else {
        return;
    };

    let diagnostics = Compiler.check("fuzz.rss", source);
    let has_errors = diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity == Severity::Error);
    if has_errors {
        return;
    }

    let once = format_source("fuzz.rss", source);
    let twice = format_source("fuzz.rss", &once);
    assert_eq!(
        once, twice,
        "format_source is not idempotent\n--- once ---\n{once}\n--- twice ---\n{twice}"
    );
});
