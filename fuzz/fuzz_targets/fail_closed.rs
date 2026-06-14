//! Coverage-guided negative generation (fail-closed contract).
//!
//! The seed is decoded into a well-typed program; if the checker accepts it, one
//! targeted defect is injected (rss-testgen's `mutate`) and the result must be
//! rejected by the checker with the expected diagnostic **and** must not lower to
//! Rust. A crash here means a defect slipped past the front end to the backend —
//! a "RSScript owns semantics" violation. Replay with `rss-testgen <hex-seed>`
//! (then inspect; the mutation is deterministic from the same seed).

#![no_main]

use libfuzzer_sys::fuzz_target;
use rss_testgen::generate;
use rss_testgen::mutate::mutate;
use rss_testgen::properties::{assert_fails_closed, checker_accepts};

fuzz_target!(|data: &[u8]| {
    let base = generate(data);
    if checker_accepts("fuzz.rss", &base.source) {
        let mutated = mutate(&base, data);
        assert_fails_closed("fuzz.rss", &mutated);
    }
});
