//! Generative N-way differential — the payoff of the `rss-testgen` framework.
//!
//! `rss_testgen` produces well-typed, terminating RSScript programs; each is run
//! through the backend parity set — VM interpreter, tier-0 JIT, the native tier
//! (+ force-deopt) under the feature, and, when
//! `RSSCRIPT_FULL_BACKEND_PARITY=1`, the compiled-Rust backend. The compiled
//! backend builds a crate per program, so it is opt-in for normal edit/test
//! cycles; raise the counts with `RSS_GENERATIVE_CASES` and
//! `RSS_GENERATIVE_MUTATION_CASES` for a soak. The fast, large-N in-process
//! differential lives in the `rss-testgen` crate's own smoke test; this test
//! folds the generated programs into the executable backend set.

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
        .unwrap_or(4)
}

fn mutation_case_count() -> u64 {
    std::env::var("RSS_GENERATIVE_MUTATION_CASES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(24)
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
        // N-way executable parity; panics (naming the diverging pair and the
        // source) on any disagreement. Full mode includes compiled Rust.
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
    let cases = mutation_case_count();
    let mut checked = 0;
    for n in 0..cases {
        let base = generate(&seed_for(n));
        // Only mutate programs the checker already accepts, so a failure is
        // attributable to the injected defect, not a pre-existing one.
        if !checker_accepts("mutate.rss", &base.source) {
            continue;
        }
        let mutated = rss_testgen::mutate::mutate(&base, &seed_for(n.wrapping_mul(31)));
        rss_testgen::properties::assert_fails_closed("mutate.rss", &mutated);
        checked += 1;
    }
    assert!(
        checked > 0,
        "no generated mutation was checked ({cases} cases)"
    );
}
