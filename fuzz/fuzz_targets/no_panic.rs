//! Coverage-guided proof of **Invariant 1**: no agent program can cause a Rust
//! panic / native crash — every program fault is a clean `EvalError`.
//!
//! libFuzzer's bytes are decoded by `rss-testgen` into a well-typed program. If
//! the checker accepts it, the program is evaluated on the reg-VM through the
//! limits-aware entry point under GENEROUS but finite `VmLimits`, so the three
//! classic resource-exhaustion attacks the generator might stumble into —
//! unbounded recursion (native stack overflow = SIGSEGV), an infinite loop
//! (hang), and runaway allocation (OOM) — all terminate as a recoverable
//! `EvalError::Runtime(_)` instead of crashing or hanging the fuzzer.
//!
//! The result must be `Ok(_)` or `Err(EvalError::_)` and NEVER a panic. We do
//! NOT use `catch_unwind`: a panic/abort inside eval is exactly the failure we
//! are hunting, and libFuzzer already records any panic/abort as a crash with
//! the offending seed (replay with `rss-testgen <hex-seed>`). The `match` below
//! is therefore the *assertion* — it documents the only two acceptable shapes;
//! anything that escapes it as a panic is the bug Invariant 1 forbids.

#![no_main]

use libfuzzer_sys::fuzz_target;
use rsscript::{EvalError, VmLimits, reg_vm_eval_source_main_with_limits};
use rss_testgen::generate;
use rss_testgen::properties::checker_accepts;

fuzz_target!(|data: &[u8]| {
    let program = generate(data);
    // Only checker-accepted programs are runnable; a rejected program is the
    // generator's concern (its accept rate is policed by rss-testgen's smoke
    // test), not a runtime invariant.
    if !checker_accepts("fuzz.rss", &program.source) {
        return;
    }

    // Generous-but-finite budgets: real generated programs run well under these,
    // but any accidental non-termination / deep recursion / runaway growth trips
    // a clean `EvalError::Runtime` rather than hanging or crashing the fuzzer.
    let limits = VmLimits {
        max_depth: 16_384,
        step_budget: Some(50_000_000),
        mem_budget: Some(512 * 1024 * 1024),
        // No ambient cancel flag: the fuzzer relies on the step/mem budgets to
        // bound each run; the host-level preemption hook is not exercised here.
        cancel: None,
        stdout_budget: Some(16 * 1024 * 1024),
        host_call_budget: Some(1_000_000),
        ..VmLimits::default()
    };

    let result = reg_vm_eval_source_main_with_limits(
        "fuzz.rss",
        &program.source,
        std::iter::empty::<String>(),
        limits,
    );

    // Invariant 1: the only acceptable outcomes are a clean value or a clean
    // error value. Reaching this point at all means eval did not panic; the
    // match makes the contract explicit (and the binding is consumed so the
    // optimizer can't elide the call).
    match result {
        Ok(_) | Err(EvalError::Diagnostics(_)) | Err(EvalError::Runtime(_)) => {}
    }
});
