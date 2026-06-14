//! Generator-quality + in-process differential smoke test.
//!
//! Drives the generator over many deterministic seeds and asserts (a) the
//! checker accepts the overwhelming majority of generated programs — a low
//! accept rate means the generator drifted from the language — and (b) every
//! accepted program runs identically on each in-process backend (interpreter vs
//! tier-0 JIT, plus the native tier under `--features native-jit`).
//!
//! The full N-way differential including the compiled-Rust backend lives in the
//! `rsscript` test crate (`tests/generative.rs`); this keeps the lib crate's own
//! checks fast and dependency-light.

use rss_testgen::generate;
use rss_testgen::properties::{assert_inprocess_agree, checker_accepts};

/// A spread of seeds: a counter expanded to bytes gives wide, deterministic
/// coverage of the generator's decision space.
fn seeds(count: u64) -> impl Iterator<Item = Vec<u8>> {
    (0..count).map(|n| {
        let mut bytes = n.to_le_bytes().to_vec();
        // Mix in a second word so longer decision sequences vary too.
        bytes.extend_from_slice(&n.wrapping_mul(2_654_435_761).to_le_bytes());
        bytes
    })
}

#[test]
fn generated_programs_are_accepted_and_backends_agree() {
    let total = 600;
    let mut accepted = 0;
    let mut rejections: Vec<(u64, String)> = Vec::new();

    for (index, seed) in seeds(total).enumerate() {
        let program = generate(&seed);
        if checker_accepts("gen.rss", &program.source) {
            accepted += 1;
            // Must agree across every in-process backend (panics on divergence).
            assert_inprocess_agree("gen.rss", &program.source, &[]);
        } else if rejections.len() < 5 {
            rejections.push((index as u64, program.source));
        }
    }

    let rate = accepted as f64 / total as f64;
    assert!(
        rate >= 0.95,
        "generator accept rate {rate:.3} below 0.95 ({accepted}/{total}); \
         first rejections:\n{}",
        rejections
            .iter()
            .map(|(i, src)| format!("--- seed #{i} ---\n{src}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}
