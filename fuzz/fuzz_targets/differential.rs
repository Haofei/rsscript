//! Coverage-guided generative differential.
//!
//! libFuzzer's bytes are decoded by `rss-testgen` into a well-typed, terminating
//! RSScript program, which is then run through every in-process backend (VM
//! interpreter, tier-0 JIT, and — under the `native-jit` feature — the native
//! tier plus deopt/OSR stress twins). Any disagreement panics, which libFuzzer
//! records as a crash with the offending seed; replay it with
//! `rss-testgen <hex-seed>`.
//!
//! The compiled-Rust backend is deliberately excluded here (a `rustc` invocation
//! per case is far too slow for coverage-guided fuzzing); it is folded in by the
//! bounded in-suite test `crates/rsscript-compiler/tests/generative.rs`.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rss_testgen::generate;
use rss_testgen::properties::{assert_inprocess_agree, checker_accepts};

fuzz_target!(|data: &[u8]| {
    let program = generate(data);
    // Only programs the checker accepts can be run; a rejected program is the
    // generator's concern (its accept rate is policed by the rss-testgen smoke
    // test), not a backend bug.
    if checker_accepts("fuzz.rss", &program.source) {
        assert_inprocess_agree("fuzz.rss", &program.source, &[]);
    }
});
