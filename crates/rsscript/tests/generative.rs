//! Generative N-way differential — the payoff of the `rss-testgen` framework.
//!
//! `rss_testgen` produces well-typed, terminating RSScript programs; each is run
//! through the **full** backend set — VM interpreter, tier-0 JIT, the native tier
//! (+ force-deopt) under the feature, *and the compiled-Rust backend* — and they
//! must agree (or all fail). The compiled backend builds a crate per program, so
//! this is intentionally bounded; raise the count with `RSS_GENERATIVE_CASES` for
//! a soak. The fast, large-N in-process differential lives in the `rss-testgen`
//! crate's own smoke test; this test exists to fold in the compiled backend that
//! only the test crate can run.

mod common;

use rss_testgen::generate;
use rss_testgen::properties::checker_accepts;

/// Deterministic seed spread (a counter expanded to bytes).
fn seed_for(n: u64) -> Vec<u8> {
    let mut bytes = n.to_le_bytes().to_vec();
    bytes.extend_from_slice(&n.wrapping_mul(2_654_435_761).to_le_bytes());
    bytes
}

fn case_count() -> u64 {
    std::env::var("RSS_GENERATIVE_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(16)
}

#[test]
fn generated_programs_agree_across_all_backends() {
    let cases = case_count();
    let mut checked = 0;

    for n in 0..cases {
        let program = generate(&seed_for(n));
        // The generator's accept rate is asserted in the rss-testgen smoke test;
        // here we simply skip the rare program the checker rejects rather than
        // feed an invalid program to the compiled backend.
        if !checker_accepts("generative.rss", &program.source) {
            continue;
        }
        // Full N-way incl. the compiled-Rust backend; panics (naming the diverging
        // pair and the source) on any disagreement.
        common::differential::assert_backends_agree("generative.rss", &program.source, &[]);
        checked += 1;
    }

    assert!(
        checked > 0,
        "no generated program was checked across backends ({cases} cases)"
    );
}

#[test]
fn generated_programs_fail_closed_when_mutated() {
    // Negative generation: inject one targeted defect into each generated program
    // and require the checker to reject it (with the expected diagnostic) AND
    // produce no Rust — the "RSScript owns semantics" contract. In-process only
    // (no compiled backend), so this can sweep many cases cheaply.
    for n in 0..200u64 {
        let base = generate(&seed_for(n));
        // Only mutate programs the checker already accepts, so a failure is
        // attributable to the injected defect, not a pre-existing one.
        if !checker_accepts("mutate.rss", &base.source) {
            continue;
        }
        let mutated = rss_testgen::mutate::mutate(&base, &seed_for(n.wrapping_mul(31)));
        rss_testgen::properties::assert_fails_closed("mutate.rss", &mutated);
    }
}
