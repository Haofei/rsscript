//! Differential execution of the example corpus.
//!
//! `checker_frontend::misc::examples_have_no_diagnostics_and_lower_to_runnable_packages`
//! already proves every `examples/scripts/**/*.rss` type-checks and lowers to a
//! `todo!`-free Rust package. That is the *static* half. This is the *dynamic*
//! half: the synchronous, side-effect-free example scripts are run through every
//! executable backend (VM interpreter, JIT, native tier when built, and the
//! compiled-Rust path) and required to agree. It catches the "valid example,
//! divergent backend" / doc-drift class without per-example expected-output
//! files.
//!
//! Scripts that need real runtime resources (files, network, env, the clock) or
//! that exercise the async scheduler are skipped here — they are non-trivial to
//! run deterministically in a unit test and are covered by the dedicated
//! `vm_eval_parity` fixtures (with temp dirs and local servers) and the static
//! sweep above. The skip is decided from the source itself so a newly added pure
//! example is picked up automatically.

mod common;

use common::differential::backends_agreed_stdout;

/// Namespaces / constructs whose behavior depends on real runtime resources or
/// on scheduler timing, which this in-process differential cannot reproduce
/// deterministically. A script touching any of these is left to the static
/// check+lower sweep and the resource-backed `vm_eval_parity` tests.
const NON_DETERMINISTIC_MARKERS: &[&str] = &[
    // Real I/O and host boundaries.
    "File.",
    "Directory.",
    "Http.",
    "WebSocket.",
    "Socket.",
    "Tcp",
    "Net.",
    "Process.",
    "Env.",
    "System.",
    // Wall-clock / timing.
    "Clock.",
    "Deadline.",
    "Instant.",
    "Timer.",
    "sleep",
    // Non-determinism.
    "Random.",
    "Rng.",
    // Async scheduler (ordering is covered by the async parity fixtures).
    "spawn ",
    "await ",
    "async ",
    "Channel",
    "TaskGroup",
    "Cancellation",
    "Select",
];

fn needs_runtime_resources(source: &str) -> bool {
    NON_DETERMINISTIC_MARKERS
        .iter()
        .any(|marker| source.contains(marker))
}

#[test]
fn example_scripts_agree_across_backends() {
    let mut executed = Vec::new();
    let mut skipped = Vec::new();

    for path in common::recursive_paths_with_extension("examples/scripts", "rss") {
        let source = common::read_fixture(&path);
        let name = path.to_str().expect("utf-8 path");

        if needs_runtime_resources(&source) {
            skipped.push(name.to_string());
            continue;
        }

        // Panics (with the diverging backend pair and the source) on any
        // failure or mismatch, so a divergence names the exact example.
        let _ = backends_agreed_stdout(name, &source, &[]);
        executed.push(name.to_string());
    }

    // Guard against the predicate silently excluding everything (which would
    // turn this into a vacuous test): the pure `basic/` scripts alone exceed
    // this floor, so dropping below it signals the corpus or predicate drifted.
    assert!(
        executed.len() >= 6,
        "expected the differential sweep to execute several example scripts, \
         ran {} (skipped {}): executed={executed:?}",
        executed.len(),
        skipped.len()
    );
}
