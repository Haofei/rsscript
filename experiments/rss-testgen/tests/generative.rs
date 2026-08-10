//! Experimental generated-program differential and fail-closed maintenance.
//!
//! This belongs to the test-generation workspace, not the reviewed SDK Core
//! test closure. It intentionally exercises the VM/JIT experiment surface.

use rss_testgen::{generate, mutate, properties};

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
fn generated_programs_agree_across_inprocess_experimental_backends() {
    let mut checked = 0;
    for index in 0..case_count() {
        let program = generate(&seed_for(index));
        if !properties::checker_accepts("generative.rss", &program.source) {
            continue;
        }
        properties::assert_inprocess_agree("generative.rss", &program.source, &[]);
        checked += 1;
    }
    assert!(checked > 0, "no generated program entered differential testing");
}

#[test]
fn generated_program_mutations_fail_closed() {
    let mut checked = 0;
    for index in 0..mutation_case_count() {
        let base = generate(&seed_for(index));
        if !properties::checker_accepts("mutate.rss", &base.source) {
            continue;
        }
        let mutated = mutate::mutate(&base, &seed_for(index.wrapping_mul(31)));
        properties::assert_fails_closed("mutate.rss", &mutated);
        checked += 1;
    }
    assert!(checked > 0, "no generated mutation entered fail-closed testing");
}
