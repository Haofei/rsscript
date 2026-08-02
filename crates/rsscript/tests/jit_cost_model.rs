//! Step 1 native-tier **profitability** cost-model regression tests.
//!
//! Kept in their own test binary on purpose: the test sets the process-global
//! `RSS_JIT_COST_MODEL=enforce` env var, and isolating it in a separate binary
//! (with a single test function) keeps that from perturbing the native tiering of
//! any other test. Declining for profitability is correctness-safe — the function
//! just stays on the interpreter — so the enforced runs must stay byte-identical to
//! the cost-model-off runs.
#![cfg(feature = "native-jit")]

/// A polymorphic closure dispatch in a hot loop: `dispatch` is called every
/// iteration with one of three closure targets. Its hot native region carries a
/// polymorphic inline-cache (`pic_sites > 0`), the canonical "native ≈ interpreter"
/// case the cost model should decline.
const PIC_SRC: &str = "\
fn dispatch(f: read Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 20000 {
        if i % 6 == 0 {
            let c: Fn(Int) -> Int = |x| { return 0 - x }
            total = total + dispatch(f: read c, x: read i)
        } else if i % 6 < 3 {
            let b: Fn(Int) -> Int = |x| { return x + 7 }
            total = total + dispatch(f: read b, x: read i)
        } else {
            let a: Fn(Int) -> Int = |x| { return x * 2 - 1 }
            total = total + dispatch(f: read a, x: read i)
        }
        i = i + 1
    }
    let _ = String.from_int(value: total)
    return Unit
}
";

/// A pure scalar-arithmetic hot loop: native genuinely beats the interpreter here,
/// so the cost model must KEEP it (never decline). Guards against a blanket gate.
const SCALAR_SRC: &str = "\
fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 20000 {
        total = total + i * 2 - 1
        i = i + 1
    }
    let _ = String.from_int(value: total)
    return Unit
}
";

#[test]
fn cost_model_enforce_declines_polymorphic_pic_keeps_scalar_loop() {
    // Reference output with the cost model OFF (native compiles the PIC region).
    let pic_reference = {
        let exe = rsscript::reg_vm_compile_source("cost-pic.rss", PIC_SRC).expect("compile");
        exe.eval_main_with_args_native_with_stats(std::iter::empty::<String>())
            .expect("off-mode run")
            .0
    };

    // Enforce mode for the remainder of this single-test process.
    // SAFETY: this test binary runs exactly one test, so no concurrent test reads it.
    unsafe {
        std::env::set_var("RSS_JIT_COST_MODEL", "enforce");
    }

    // 1. The polymorphic-PIC region is declined as unprofitable and stays on the
    //    interpreter — byte-identical output, plus the decline is surfaced in
    //    telemetry with a reason that names the PIC signature.
    let exe = rsscript::reg_vm_compile_source("cost-pic.rss", PIC_SRC).expect("compile");
    let (pic_out, pic_stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("enforce-mode PIC run");
    assert_eq!(
        pic_reference.stdout, pic_out.stdout,
        "declining for profitability must be byte-identical to the native/off run"
    );
    assert!(
        pic_stats.unprofitable_declines > 0,
        "the polymorphic PIC region must be declined as unprofitable under enforce: {pic_stats:?}"
    );
    assert!(
        pic_stats
            .unprofitable_decline_reasons
            .keys()
            .any(|r| r.contains("unprofitable") && r.contains("pic_sites=1")),
        "a decline reason should name the PIC signature: {:?}",
        pic_stats.unprofitable_decline_reasons
    );

    // 2. The pure scalar loop is a genuine native win: it must STILL reach the
    //    native tier (here via OSR) and must NOT be declined. Proves the gate is
    //    selective, not a blanket off-switch.
    let exe = rsscript::reg_vm_compile_source("cost-scalar.rss", SCALAR_SRC).expect("compile");
    let (_scalar_out, scalar_stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("enforce-mode scalar run");
    assert!(
        scalar_stats.osr_entries > 0,
        "the scalar loop must still OSR natively under enforce: {scalar_stats:?}"
    );
    assert_eq!(
        scalar_stats.unprofitable_declines, 0,
        "the scalar loop must not be declined as unprofitable: {scalar_stats:?}"
    );

    unsafe {
        std::env::remove_var("RSS_JIT_COST_MODEL");
    }
}
