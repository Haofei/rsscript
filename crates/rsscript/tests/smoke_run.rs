//! "Does it actually run" smoke gate.
//!
//! Backlog #7: `rss check` / the review oracle were green while the package had
//! never actually *executed*. Type-checking and lowering passing is not proof a
//! program runs. These tests take small-but-non-trivial representative programs
//! (arithmetic + a function call + a `match` + `Log.write`) and confirm they
//! genuinely EXECUTE — on BOTH tiers:
//!
//!   1. the register VM (`reg_vm_eval_source_main`), with an explicit expected
//!      stdout/value assertion, and
//!   2. the compiled (AOT) backend, lowered to Rust + built + run,
//!
//! asserting the two tiers produce identical output. This catches "checks pass
//! but it doesn't run" regressions that pure check/type tests would miss.
//!
//! Kept deliberately small and deterministic so the gate stays fast.
#![allow(unused_imports)]

mod common;

// `reg_vm_eval_source_main` is re-exported from the crate root as `eval_source_main`.
use rsscript::eval_source_main as reg_vm_eval_source_main;

/// A representative program: arithmetic, a user function call, a `match` over the
/// result, and `Log.write` for stdout. Exercises the value path AND the I/O path.
const SMOKE_PROGRAM: &str = "\
fn classify(n: Int) -> String {
    match n {
        0 => { return read \"zero\" }
        _ => {
            if n < 0 {
                return read \"negative\"
            }
            return read \"positive\"
        }
    }
}

fn main() -> Unit {
    let a = 2
    let b = 3
    let total = a + b * 4
    Log.write(message: read classify(n: read total))
    Log.write(message: read String.from_int(value: read total))
    return Unit
}
";

/// Tier 1: the program must EVALUATE on the register VM and produce the exact
/// expected stdout and return value — not merely type-check.
#[test]
fn smoke_program_evaluates_on_reg_vm() {
    let output = reg_vm_eval_source_main("smoke_run.rss", SMOKE_PROGRAM)
        .expect("smoke program should evaluate on the register VM");

    assert_eq!(output.stdout, "positive\n14\n", "reg-VM stdout");
    assert_eq!(output.value, "Unit", "reg-VM return value");
}

/// Tier 1 + 2: the SAME program must run identically on the register VM AND the
/// compiled (AOT) backend. `assert_vm_eval_matches_backend` evals (interpreter +
/// JIT) and compiles+runs the program, asserting all tiers agree — proving it
/// genuinely executes end-to-end, not just type-checks.
#[test]
fn smoke_program_runs_on_vm_and_aot() {
    common::assert_vm_eval_matches_backend(
        "smoke_run.rss",
        "rsscript_smoke_run",
        SMOKE_PROGRAM,
    );
}
