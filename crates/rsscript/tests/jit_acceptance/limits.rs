// ---------------------------------------------------------------------------
// Recursive native fast paths must honor the same limit gate as `try_native`:
// Cranelift code polls neither `step_budget` nor `cancel` and allocates off the
// `mem_budget` meter, so with any of them armed the recursive paths must NOT
// dispatch natively (they run on the interpreter / tier-0, which `tick()`s).
// ---------------------------------------------------------------------------

#[cfg(feature = "native-jit")]
const FIB_SELF_SRC: &str = "\
fn fib(n: Int) -> Int {
    if n < 2 { return n }
    return fib(n: read n - 1) + fib(n: read n - 2)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: fib(n: read 28)))
    return Unit
}
";

#[cfg(feature = "native-jit")]
const IS_EVEN_MUTUAL_SRC: &str = "\
fn is_even(n: Int) -> Int {
    if n < 1 { return 1 }
    return is_odd(n: read n - 1)
}
fn is_odd(n: Int) -> Int {
    if n < 1 { return 0 }
    return is_even(n: read n - 1)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: is_even(n: read 20)))
    return Unit
}
";

/// Self-recursive native candidate (`fib`) must NOT dispatch natively while a
/// `step_budget` is armed (native never `tick()`s, so it would bypass the budget).
/// The budget here is generous enough to complete on the interpreter; the proof is
/// `native_calls == 0` (native refused) with the correct result.
#[cfg(feature = "native-jit")]
#[test]
fn native_self_recursion_refused_when_step_budget_armed() {
    let exe = rsscript::reg_vm_compile_source("limit-self.rss", FIB_SELF_SRC).expect("compile");
    let limits = rsscript::VmLimits {
        step_budget: Some(50_000_000),
        ..rsscript::VmLimits::default()
    };
    let (out, stats) = exe
        .eval_main_with_args_native_with_limits(std::iter::empty::<String>(), limits)
        .expect("completes within budget");
    assert_eq!(out.stdout.trim_end(), "317811");
    assert_eq!(
        stats.native_calls, 0,
        "armed step_budget must refuse native self-recursion: {stats:?}"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_recursion_obeys_custom_max_depth_via_interpreter() {
    let exe =
        rsscript::reg_vm_compile_source("limit-custom-depth.rss", FIB_SELF_SRC).expect("compile");
    let (out, stats) = exe
        .eval_main_with_args_native_with_limits(
            std::iter::empty::<String>(),
            rsscript::VmLimits {
                max_depth: 64,
                ..rsscript::VmLimits::default()
            },
        )
        .expect("custom limit above fib depth");
    assert_eq!(out.stdout.trim_end(), "317811");
    assert_eq!(
        stats.native_calls, 0,
        "custom language depth must use the exact interpreter frame accounting"
    );

    let err = exe
        .eval_main_with_args_native_with_limits(
            std::iter::empty::<String>(),
            rsscript::VmLimits {
                max_depth: 8,
                ..rsscript::VmLimits::default()
            },
        )
        .expect_err("small custom depth must stop recursion");
    assert!(
        matches!(err, rsscript::EvalError::Runtime(ref message) if message.contains("recursion depth limit")),
        "expected recursion-depth error, got {err:?}"
    );
}

/// Mutual-recursion native candidate (`is_even`/`is_odd`) must NOT dispatch natively
/// while a `step_budget` is armed.
#[cfg(feature = "native-jit")]
#[test]
fn native_mutual_recursion_refused_when_step_budget_armed() {
    let exe =
        rsscript::reg_vm_compile_source("limit-mutual.rss", IS_EVEN_MUTUAL_SRC).expect("compile");
    let limits = rsscript::VmLimits {
        step_budget: Some(50_000_000),
        ..rsscript::VmLimits::default()
    };
    let (out, stats) = exe
        .eval_main_with_args_native_with_limits(std::iter::empty::<String>(), limits)
        .expect("completes within budget");
    assert_eq!(out.stdout.trim_end(), "1");
    assert_eq!(
        stats.native_calls, 0,
        "armed step_budget must refuse native mutual recursion: {stats:?}"
    );
}

/// A present `cancel` flag (even un-triggered) must refuse recursive native dispatch,
/// matching `try_native` — native code can never observe the flag, so the cooperative
/// interpreter path (which polls it in `tick()`) must run instead.
#[cfg(feature = "native-jit")]
#[test]
fn native_recursion_refused_when_cancel_armed() {
    let exe = rsscript::reg_vm_compile_source("limit-cancel.rss", FIB_SELF_SRC).expect("compile");
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let limits = rsscript::VmLimits {
        cancel: Some(cancel),
        ..rsscript::VmLimits::default()
    };
    let (out, stats) = exe
        .eval_main_with_args_native_with_limits(std::iter::empty::<String>(), limits)
        .expect("completes (flag never set)");
    assert_eq!(out.stdout.trim_end(), "317811");
    assert_eq!(
        stats.native_calls, 0,
        "an armed cancel flag must refuse native recursion: {stats:?}"
    );
}

/// Enforcement, not just refusal: a small `step_budget` must actually PREEMPT a
/// recursive native candidate (with the bug, native ran `fib(28)` to completion and
/// silently bypassed the budget). Now it runs on the interpreter and trips the budget.
#[cfg(feature = "native-jit")]
#[test]
fn native_self_recursion_step_budget_preempts() {
    let exe = rsscript::reg_vm_compile_source("limit-preempt.rss", FIB_SELF_SRC).expect("compile");
    let limits = rsscript::VmLimits {
        step_budget: Some(1_000),
        ..rsscript::VmLimits::default()
    };
    let err = exe
        .eval_main_with_args_native_with_limits(std::iter::empty::<String>(), limits)
        .expect_err("small step budget must preempt, not be bypassed by native");
    match err {
        rsscript::EvalError::Runtime(msg) => assert!(
            msg.contains("step budget"),
            "expected step-budget error, got: {msg}"
        ),
        other => panic!("expected Runtime(step budget), got {other:?}"),
    }
}

/// Shared OSR-shaped kernel for the J0.5 limit tests: the loop is wrapped by
/// non-native `Log.write` I/O, so the function is native-INELIGIBLE as a whole and the
/// hot loop is taken via OSR (not whole-function native) — which is exactly the tier
/// J0.5 must enforce limits in. `read N` blocks const-folding.
#[cfg(feature = "native-jit")]
const J05_OSR_KERNEL: &str = "\
fn loopsum(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut total = 0
    let mut i = 0
    while i < n {
        total = total + i
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: loopsum(n: read 5000)))
    return Unit
}
";

/// A generous armed `step_budget` keeps OSR interpreted until transformed regions
/// carry exact source-step costs, while preserving the successful result.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_completes_under_generous_step_budget() {
    let exe = rsscript::reg_vm_compile_source("j05-step-ok.rss", J05_OSR_KERNEL).expect("compile");
    let limits = rsscript::VmLimits {
        step_budget: Some(10_000_000),
        ..rsscript::VmLimits::default()
    };
    let (output, stats) = exe
        .eval_main_with_args_native_osr_with_limits(std::iter::empty::<String>(), limits)
        .expect("generous step budget must not trip");
    // sum(0..5000) = 5000 * 4999 / 2 = 12497500.
    assert_eq!(output.stdout.trim_end(), "begin\n12497500");
    assert_eq!(
        stats.osr_entries, 0,
        "armed step budgets must stay interpreted"
    );
}

/// The same loop under a tight `step_budget` must trip cleanly on the interpreter,
/// which remains the sole resource-limit authority while OSR is declined.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_trips_tight_step_budget() {
    let exe =
        rsscript::reg_vm_compile_source("j05-step-trip.rss", J05_OSR_KERNEL).expect("compile");
    let limits = rsscript::VmLimits {
        step_budget: Some(5_000),
        ..rsscript::VmLimits::default()
    };
    let err = exe
        .eval_main_with_args_native_osr_with_limits(std::iter::empty::<String>(), limits)
        .expect_err("a tight step budget must preempt the native loop, not be bypassed");
    match err {
        rsscript::EvalError::Runtime(msg) => assert!(
            msg.contains("step budget"),
            "expected step-budget error, got: {msg}"
        ),
        other => panic!("expected Runtime(step budget), got {other:?}"),
    }
}

#[cfg(feature = "native-jit")]
#[test]
fn native_osr_host_call_budget_stays_interpreted_and_enforced() {
    let source = "\
fn measure(s: String, n: Int) -> Int {
    Log.write(message: \"begin\")
    let mut total = 0
    let mut i = 0
    while i < n {
        total = total + String.len(value: s)
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: String.from_int(value: measure(s: \"abc\", n: 200)))
    return Unit
}
";
    let exe = rsscript::reg_vm_compile_source("osr-host-budget.rss", source).expect("compile");
    let (out, stats) = exe
        .eval_main_with_args_native_osr_with_limits(
            std::iter::empty::<String>(),
            rsscript::VmLimits {
                host_call_budget: Some(10_000),
                ..rsscript::VmLimits::default()
            },
        )
        .expect("generous host-call budget");
    assert_eq!(out.stdout.trim_end(), "begin\n600");
    assert_eq!(
        stats.osr_entries, 0,
        "armed host-call budget must decline OSR"
    );

    let err = exe
        .eval_main_with_args_native_osr_with_limits(
            std::iter::empty::<String>(),
            rsscript::VmLimits {
                host_call_budget: Some(10),
                ..rsscript::VmLimits::default()
            },
        )
        .expect_err("tight host-call budget must be enforced by the interpreter");
    assert!(
        matches!(err, rsscript::EvalError::Runtime(ref message) if message.contains("host call budget")),
        "expected host-call-budget error, got {err:?}"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_osr_materializes_written_handle_live_out() {
    let source = "\
fn choose(left: String, right: String, n: Int) -> String {
    Log.write(message: \"begin\")
    let mut selected = left
    let mut i = 0
    while i < n {
        selected = right
        i = i + 1
    }
    return selected
}

fn main() -> Unit {
    Log.write(message: choose(left: \"wrong\", right: \"right\", n: 200))
    return Unit
}
";
    let file = "osr-handle-liveout.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interpreter");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (native, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("native OSR");
    assert_eq!(native.stdout, interp.stdout);
    assert_eq!(native.stdout.trim_end(), "begin\nright");
    assert!(
        stats.osr_entries > 0,
        "handle live-out loop must exercise OSR: {stats:?}"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_osr_restores_bool_live_out() {
    let source = "\
fn final_flag(seed: Bool, n: Int) -> Bool {
    Log.write(message: \"begin\")
    let mut flag = false
    let mut i = 0
    while i < n {
        if seed {
            flag = i % 2 == 0
        } else {
            flag = i % 2 != 0
        }
        i = i + 1
    }
    return flag
}

fn main() -> Unit {
    Log.write(message: String.from_bool(value: final_flag(seed: true, n: 201)))
    return Unit
}
";
    let file = "osr-bool-liveout.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interpreter");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (native, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("native OSR");
    assert_eq!(native.stdout, interp.stdout);
    assert_eq!(native.stdout.trim_end(), "begin\ntrue");
    assert!(
        stats.osr_entries > 0,
        "Bool live-out loop must exercise OSR: {stats:?}"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_osr_flat_and_nested_handle_alias_falls_back() {
    let source = "\
features: local

struct Holder {
    items: handle List<Int>
}

fn update(xs: mut List<Int>, holder: mut Holder, n: Int) -> Int {
    Log.write(message: \"begin\")
    let mut total = 0
    let mut i = 0
    while i < n {
        List.set(list: mut xs, index: 0, value: i)
        total = total + List.get(list: holder.items, index: 0)
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    local xs = List<Int>.new()
    List.push(list: mut xs, value: 0)
    local holder = Holder(items: read xs)
    Log.write(message: String.from_int(value: update(xs: mut xs, holder: mut holder, n: 200)))
    return Unit
}
";
    let file = "osr-flat-nested-alias.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interpreter");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (native, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("native fallback");
    assert_eq!(native.stdout, interp.stdout);
    assert_eq!(native.stdout.trim_end(), "begin\n19900");
    assert_eq!(
        stats.osr_entries, 0,
        "nested Handle alias must be rejected before flat buffers are pinned"
    );
}

/// With cancellation armed, OSR stays interpreted and the interpreter raises the
/// cancellation error. A hot loop must never bypass cooperative cancellation.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_cancel_flag_preempts() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    // A long loop so the interpreter, after native bails on the set flag, still reaches
    // its own (every-1024-step) cancel poll before finishing.
    let source = "\
fn loopsum(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut total = 0
    let mut i = 0
    while i < n {
        total = total + i
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: loopsum(n: read 1000000)))
    return Unit
}
";
    let exe = rsscript::reg_vm_compile_source("j05-cancel.rss", source).expect("compile");
    let flag = Arc::new(AtomicBool::new(true));
    let limits = rsscript::VmLimits {
        cancel: Some(Arc::clone(&flag)),
        ..rsscript::VmLimits::default()
    };
    let err = exe
        .eval_main_with_args_native_osr_with_limits(std::iter::empty::<String>(), limits)
        .expect_err("a set cancel flag must preempt the native loop");
    match err {
        rsscript::EvalError::Runtime(msg) => assert!(
            msg.contains("cancelled"),
            "expected cancellation error, got: {msg}"
        ),
        other => panic!("expected Runtime(cancelled), got {other:?}"),
    }
}

/// J0.4 #1 (heap-key collection write): a hot loop inserting into a `Map<String, Int>`
/// with String keys lowers to the native `MapInsertHandleKeyInt` helper — the key is
/// resolved and hashed by the host's own `VmMapKey` (never re-hashed in native) and the
/// write is journaled (§7.2). Must OSR and match the interpreter byte-for-byte.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_map_insert_string_key_matches_interpreter() {
    // The key is a live-in `String` param (an input handle), not freshly allocated in
    // the loop, so the loop is native-subset (no escaping allocation) and OSRs; each
    // iteration inserts `k -> i` (overwriting), exercising `MapInsertHandleKeyInt`.
    let source = "\
fn build_map(k: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut m = Map<String, Int>.new()
    let mut i = 0
    while i < n {
        Map.insert<String, Int>(map: mut m, key: read k, value: read i)
        i = i + 1
    }
    return Map.len<String, Int>(map: read m)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: build_map(k: read \"hello\", n: read 200)))
    return Unit
}
";
    let file = "j1-map-strkey.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "string-key map insert OSR must match the interpreter"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n1");

    let executable = rsscript::reg_vm_compile_source(file, source).expect("compiles");
    let (_osr2, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run (stats)");
    assert!(
        stats.osr_entries > 0,
        "the hot string-key insert loop must OSR: {stats:?}",
    );
}

/// Even a non-allocating hot loop stays interpreted while `mem_budget` is armed. This
/// conservative gate avoids depending on incomplete per-transform allocation effects.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_nonallocating_loop_runs_under_mem_budget() {
    let exe = rsscript::reg_vm_compile_source("j05-mem-ok.rss", J05_OSR_KERNEL).expect("compile");
    let limits = rsscript::VmLimits {
        mem_budget: Some(1 << 20),
        ..rsscript::VmLimits::default()
    };
    let (output, stats) = exe
        .eval_main_with_args_native_osr_with_limits(std::iter::empty::<String>(), limits)
        .expect("a non-allocating loop must run under mem_budget");
    // sum(0..5000) = 12497500 (same kernel as the step-budget tests).
    assert_eq!(output.stdout.trim_end(), "begin\n12497500");
    assert_eq!(
        stats.osr_entries, 0,
        "armed memory budgets must stay interpreted"
    );
}

/// A map-insert loop also stays interpreted under `mem_budget`; helper-specific zero
/// charges are not enough to prove the surrounding transformed region preserves all
/// allocation accounting.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_map_insert_loop_runs_under_mem_budget() {
    let source = "\
fn build_map(k: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut m = Map<String, Int>.new()
    let mut i = 0
    while i < n {
        Map.insert<String, Int>(map: mut m, key: read k, value: read i)
        i = i + 1
    }
    return Map.len<String, Int>(map: read m)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: build_map(k: read \"hello\", n: read 200)))
    return Unit
}
";
    let file = "j05-mem-mapinsert.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let limits = rsscript::VmLimits {
        mem_budget: Some(1 << 20),
        ..rsscript::VmLimits::default()
    };
    let (output, stats) = exe
        .eval_main_with_args_native_osr_with_limits(std::iter::empty::<String>(), limits)
        .expect("map-insert loop must run under mem_budget without transformed OSR");
    assert_eq!(
        interp.stdout, output.stdout,
        "the map-insert loop must stay interpreter-identical under mem_budget"
    );
    assert_eq!(output.stdout.trim_end(), "begin\n1");
    assert_eq!(
        stats.osr_entries, 0,
        "armed memory budgets must stay interpreted"
    );
}

/// J0.4 #1 correctness guard (heap-VALUE list growth): a hot loop pushing a heap value
/// (a `String`) onto a `List<String>` accumulator. A `List<String>` is a BOXED list
/// (16-byte `VmValue` element slots), whose capacity growth the native tier does not yet
/// account for, so once the pushed value is correctly classified `Handle` the loop
/// CORRECTLY DECLINES OSR and runs on the interpreter — producing byte-for-byte output.
/// (It previously appeared to OSR, but only because the heap value was mis-typed `Int`,
/// which mis-pushed each element's heap-table INDEX as a flat-int — a silent corruption a
/// single-`len` check could not see. The fix that types heap params `Handle` exposed and
/// corrected this.) Must match the interpreter and must NOT mis-OSR a Boxed-list growth.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_list_push_handle_matches_interpreter() {
    let source = "\
fn build_list(s: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut xs = List<String>.new()
    let mut i = 0
    while i < n {
        List.push<String>(list: mut xs, value: read s)
        i = i + 1
    }
    // Discriminating: read an element back as a String. A mis-typed (flat-int) push would
    // store handle indices, so this read would diverge from the interpreter.
    return String.len(value: read List.get<String>(list: read xs, index: read 0))
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: build_list(s: read \"hi\", n: read 200)))
    return Unit
}
";
    let file = "j1-list-push-handle.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "heap-value list push must match the interpreter (Boxed-list growth runs on the interpreter)"
    );
    // Element 0 is "hi" (len 2).
    assert_eq!(osr.stdout.trim_end(), "begin\n2");
}

/// J0.1 #7 (two-armed scalar Result): a Result built as EITHER `Ok(scalar)` or
/// `Err(scalar)` in-loop (both arms genuinely taken) and matched in-loop, dead at the
/// boundary, now OSRs — the new two-armed Result scalar-replacement dissolves it to a
/// boolean tag + shared scalar payload (`MatchResult` routes on the tag). The two-armed
/// case is observable (the match feeds the running total), so byte-identity to the
/// interpreter proves the tag routing + payload selection are correct.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_two_armed_scalar_result_matches_interpreter() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let mut r: Result<Int, Int> = Ok(0)
        if i < 50 {
            r = Ok(i)
        } else {
            r = Err(i)
        }
        match r {
            Ok(v) => { total = total + v }
            Err(e) => { total = total + e + 1000 }
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 100)))
    return Unit
}
";
    let file = "jit-osr-j3-two-armed-result.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "two-armed scalar Result loop must be byte-identical to the interpreter (stdout)"
    );
    // i in 0..100: i<50 ⇒ Ok(i) ⇒ +i (sum 0..49 = 1225); i>=50 ⇒ Err(i) ⇒ +(i+1000)
    // (sum 50..99 = 3725, plus 1000*50 = 50000 ⇒ 53725). total = 1225 + 53725 = 54950.
    assert_eq!(osr.stdout.trim_end(), "begin\n54950\n54950");
    assert!(
        stats.osr_entries > 0,
        "the two-armed scalar Result loop must OSR (tag + payload dissolution): {stats:?}",
    );
}

/// J0.1 #7 (two-armed scalar Result, LIVE-AFTER): a Result assigned `Ok(i)`/`Err(i)`
/// in-loop and matched AFTER the loop (live-after) now OSRs — the OSR-exit
/// reconstruction rebuilds `Ok(payload)` or `Err(payload)` from the live-out tag. The
/// post-loop match observes the reconstructed value (Err arm adds +1000), so byte
/// identity to the interpreter proves the tag-driven Ok/Err reconstruction is correct.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_two_armed_result_live_after_reconstructs() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut last: Result<Int, Int> = Ok(0)
    while i < limit {
        if i < 50 {
            last = Ok(i)
        } else {
            last = Err(i)
        }
        total = total + 1
        i = i + 1
    }
    match last {
        Ok(v) => { total = total + v }
        Err(e) => { total = total + e + 1000 }
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 100)))
    return Unit
}
";
    let file = "jit-osr-j3-two-armed-result-live-after.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "two-armed live-after Result loop must be byte-identical to the interpreter"
    );
    // Loop: total += 1 each of 100 iters ⇒ 100. last = (i=99 ⇒ Err(99)). Post-loop match:
    // Err arm ⇒ +99+1000 = 1099. total = 100 + 1099 = 1199.
    assert_eq!(osr.stdout.trim_end(), "begin\n1199\n1199");
    assert!(
        stats.osr_entries > 0,
        "the two-armed live-after Result loop must OSR (tag-driven Ok/Err reconstruction): {stats:?}",
    );
}

/// J0.4 #1 (heap-value collection write, Set): a hot loop inserting a heap value (a
/// `String`) into a `Set<String>` lowers to the native `SetInsertHandle` helper — the
/// value is resolved and hashed by the host's own `VmMapKey`, the write is journaled.
/// Must OSR and match the interpreter byte-for-byte.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_set_insert_string_matches_interpreter() {
    let source = "\
fn build_set(s: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut seen = Set<String>.new()
    let mut i = 0
    while i < n {
        Set.insert(set: mut seen, value: read s)
        i = i + 1
    }
    return Set.len(set: read seen)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: build_set(s: read \"x\", n: read 200)))
    return Unit
}
";
    let file = "j1-set-insert-string.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "string set insert OSR must match the interpreter"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n1");

    let executable = rsscript::reg_vm_compile_source(file, source).expect("compiles");
    let (_osr2, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run (stats)");
    assert!(
        stats.osr_entries > 0,
        "the hot string-set insert loop must OSR: {stats:?}",
    );
}

/// J0.4 #1: native `SortedSet<String>.insert` (heap value) + `SortedMap<String,Int>.insert`
/// (heap key). Both lower to their `*Handle*` helpers, resolving + ordering via the host's
/// own logic; must OSR and match the interpreter.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_sorted_set_and_map_string_insert_matches_interpreter() {
    let set_src = "\
fn build(s: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut seen = SortedSet<String>.new()
    let mut i = 0
    while i < n {
        SortedSet.insert(set: mut seen, value: read s)
        i = i + 1
    }
    return SortedSet.len(set: read seen)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(s: read \"x\", n: read 200)))
    return Unit
}
";
    let interp = common::run_vm_source("j1-sortedset.rss", set_src, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source("j1-sortedset.rss", set_src).expect("compile");
    let (osr, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(
        interp.stdout, osr.stdout,
        "sorted-set string insert must match interpreter"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n1");
    assert!(
        stats.osr_entries > 0,
        "sorted-set string insert loop must OSR: {stats:?}"
    );

    let map_src = "\
fn build(k: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut m = SortedMap<String, Int>.new()
    let mut i = 0
    while i < n {
        SortedMap.insert<String, Int>(map: mut m, key: read k, value: read i)
        i = i + 1
    }
    return SortedMap.len<String, Int>(map: read m)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(k: read \"k\", n: read 200)))
    return Unit
}
";
    let interp2 = common::run_vm_source("j1-sortedmap.rss", map_src, &[]).expect("interp");
    let exe2 = rsscript::reg_vm_compile_source("j1-sortedmap.rss", map_src).expect("compile");
    let (osr2, stats2) = exe2
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(
        interp2.stdout, osr2.stdout,
        "sorted-map string-key insert must match interpreter"
    );
    assert_eq!(osr2.stdout.trim_end(), "begin\n1");
    assert!(
        stats2.osr_entries > 0,
        "sorted-map string-key insert loop must OSR: {stats2:?}"
    );
}

/// J0.4 #1 (heap-value struct field write): a hot loop setting a struct's `String` field
/// to a heap value lowers to the native `FieldSetHandle` helper (COW struct rebuild +
/// writeback). Must OSR and match the interpreter byte-for-byte.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_field_set_handle_matches_interpreter() {
    let source = "\
struct Holder {
    name: String
}

fn build(s: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut h = Holder(name: read \"init\")
    let mut i = 0
    while i < n {
        h.name = s
        i = i + 1
    }
    return String.len(value: read h.name)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(s: read \"hello\", n: read 200)))
    return Unit
}
";
    let file = "j1-field-set-handle.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "heap-value field set OSR must match the interpreter"
    );
    // After the loop h.name = "hello" (len 5).
    assert_eq!(osr.stdout.trim_end(), "begin\n5");

    let executable = rsscript::reg_vm_compile_source(file, source).expect("compiles");
    let (_osr2, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run (stats)");
    assert!(
        stats.osr_entries > 0,
        "the hot heap-value field-set loop must OSR: {stats:?}",
    );
}

/// J0.4 #1 (heap-value collection write, Deque): a hot loop pushing a heap value (a
/// `String`) onto a `Deque<String>` (front + back) lowers to the native
/// `DequePushBackHandle`/`DequePushFrontHandle` helpers. Must OSR and match interpreter.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_deque_push_handle_matches_interpreter() {
    let source = "\
fn build(s: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut d = Deque<String>.new()
    let mut i = 0
    while i < n {
        Deque.push_back<String>(deque: mut d, value: read s)
        Deque.push_front<String>(deque: mut d, value: read s)
        i = i + 1
    }
    return Deque.len(deque: read d)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(s: read \"z\", n: read 100)))
    return Unit
}
";
    let file = "j1-deque-push-handle.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "deque heap push OSR must match the interpreter"
    );
    // 100 iters * 2 pushes = 200.
    assert_eq!(osr.stdout.trim_end(), "begin\n200");

    let executable = rsscript::reg_vm_compile_source(file, source).expect("compiles");
    let (_osr2, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run (stats)");
    assert!(
        stats.osr_entries > 0,
        "the hot deque heap-push loop must OSR: {stats:?}"
    );
}

#[cfg(feature = "native-jit")]
/// J0.1 #7 (heap-payload, same-typed dead-at-boundary): a hot loop with a two-armed
/// `Result<String, String>` whose BOTH arms call `String.len` on a live heap payload
/// must OSR and match the interpreter byte-for-byte. This exercises the expanded-path
/// compile chain for a heap-payload Result — which requires (a) the string-length-fold
/// pass to LEAVE a non-foldable `String.len` in place (native `StringLen` helper) rather
/// than bail the whole OSR, and (b) the translator's Handle-`Move` alias-class
/// computations to use union-find (the dissolution's cyclic Move graph would otherwise
/// oscillate and hang the translator). Regression guard for both fixes.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_two_armed_heap_string_result_matches_interpreter() {
    let source = "\
fn f(s: read String, limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let mut r: Result<String, String> = Ok(read \"x\")
        if i < 50 { r = Ok(s) } else { r = Err(s) }
        match r {
            Ok(a) => { total = total + String.len(value: read a) }
            Err(b) => { total = total + String.len(value: read b) }
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(s: read \"hello\", limit: read 100)))
    return Unit
}
";
    let file = "j7-two-armed-heap-string.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "two-armed heap-payload Result OSR must match the interpreter"
    );
    // 100 iters * 5 ("hello") = 500.
    assert_eq!(osr.stdout.trim_end(), "begin\n500\n500");

    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (_osr2, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run (stats)");
    assert!(
        stats.osr_entries > 0,
        "the hot two-armed heap-payload Result loop must OSR: {stats:?}",
    );
}

/// J0.1 #7 (heap-payload, DIFFERENT-typed dead-at-boundary): a two-armed
/// `Result<Int, String>` (Int `Ok`, heap `Err`) consumed per-arm (`Ok` adds the Int,
/// `Err` adds `String.len` of the heap payload), dead at the loop boundary. PER-ARM
/// payload registers give each arm its own typed payload, so the mixed Int/Handle Result
/// dissolves and OSRs (it previously declined — a shared payload couldn't be typed).
/// Must OSR and match the interpreter byte-for-byte.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_two_armed_mixed_result_matches_interpreter() {
    let source = "\
fn f(s: read String, limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let mut r: Result<Int, String> = Ok(read 0)
        if i < 50 { r = Ok(read i) } else { r = Err(s) }
        match r {
            Ok(v) => { total = total + v }
            Err(b) => { total = total + String.len(value: read b) }
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(s: read \"hello\", limit: read 100)))
    return Unit
}
";
    let file = "j7-two-armed-mixed.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    // Sum of 0..50 (1225) + 50*5 (250) = 1475.
    assert_eq!(
        interp.stdout, osr.stdout,
        "mixed-typed two-armed Result must match interpreter"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n1475\n1475");

    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (_osr2, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run (stats)");
    assert!(
        stats.osr_entries > 0,
        "the hot mixed-typed two-armed Result loop must OSR with per-arm payloads: {stats:?}",
    );
}

/// J0.1 #7 correctness guard: a LIVE-AFTER two-armed `Result<String, String>` whose
/// match arms `return` directly (no in-region loop work besides the construction). The
/// heap-payload live-after RECONSTRUCTION itself now works (see
/// `native_osr_two_armed_heap_result_live_after_reconstructs`); this construction-only
/// shape may still decline OSR on the profitability/candidacy gate, so it asserts only
/// byte-for-byte interpreter parity (no hang, no miscompile) regardless of whether it
/// OSRs — guarding the boundary either way.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_two_armed_heap_result_live_after_declines_safely() {
    let source = "\
fn f(s: read String, limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut r: Result<String, String> = Ok(read \"init\")
    let mut i = 0
    while i < limit {
        if i < 50 { r = Ok(s) } else { r = Err(s) }
        i = i + 1
    }
    match r {
        Ok(a) => { return String.len(value: read a) }
        Err(b) => { return String.len(value: read b) + 1000 }
    }
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(s: read \"hello\", limit: read 100)))
    return Unit
}
";
    let file = "j7-two-armed-heap-live-after.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    // Last iter (i=99) takes Err ⇒ String.len("hello") + 1000 = 1005.
    assert_eq!(
        interp.stdout, osr.stdout,
        "live-after heap Result must match interpreter"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n1005");
}

/// J0.1 #7 (heap-payload LIVE-AFTER reconstruction): a two-armed `Result<String,String>`
/// built in the loop and read AFTER it (live-after) with a heap payload. At OSR-exit the
/// per-arm payload register holds a heap-table index; the deopt record carries it
/// (`DeoptValue::Handle`) and the reconstruction resolves it against the still-live JIT
/// heap to rebuild `Ok`/`Err`. Requires: runtime param-type seeding (so the payload
/// `Move` from the live-in `String` lowers and classifies Handle) + the Handle-carrying
/// deopt ABI. Must OSR and match the interpreter byte-for-byte.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_two_armed_heap_result_live_after_reconstructs() {
    let source = "\
fn f(s: read String, limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut last: Result<String, String> = Ok(read \"init\")
    while i < limit {
        if i < 50 { last = Ok(s) } else { last = Err(s) }
        total = total + 1
        i = i + 1
    }
    match last {
        Ok(a) => { total = total + String.len(value: read a) }
        Err(b) => { total = total + String.len(value: read b) + 1000 }
    }
    Log.write(message: read String.from_int(value: total))
    return total
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(s: read \"hello\", limit: read 100)))
    return Unit
}
";
    let file = "j7-two-armed-heap-live-after.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    // Loop: total += 1 each of 100 iters ⇒ 100. last = (i=99 ⇒ Err("hello")). Post-loop
    // Err arm ⇒ +5+1000 = 1005. total = 100 + 1005 = 1105.
    assert_eq!(
        interp.stdout, osr.stdout,
        "live-after heap-payload Result OSR must match the interpreter"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n1105\n1105");

    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (_osr2, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run (stats)");
    assert!(
        stats.osr_entries > 0,
        "the hot live-after heap-payload Result loop must OSR: {stats:?}",
    );
}

/// J0.4 #1 correctness (heap-key map insert + lookup): DISCRIMINATING regression guard.
/// Inserts `k -> i` for a `String` key param in a hot loop (OSR via `MapInsertHandleKeyInt`),
/// then looks the key back up. A heap key mis-typed as the Int handle-index (the bug the
/// runtime param-type seeding fixes) would store the wrong key and make this lookup miss,
/// diverging from the interpreter — which a `Map.len`-only check could not detect.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_map_insert_string_key_lookup_matches_interpreter() {
    let source = "\
fn build(k: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut m = Map<String, Int>.new()
    let mut i = 0
    while i < n {
        Map.insert<String, Int>(map: mut m, key: read k, value: read i)
        i = i + 1
    }
    match Map.get<String, Int>(map: read m, key: read k) {
        Some(found) => { return found }
        None => { return 0 - 1 }
    }
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(k: read \"key\", n: read 200)))
    return Unit
}
";
    let file = "j1-map-strkey-lookup.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    // Last inserted value for "key" is 199.
    assert_eq!(
        interp.stdout, osr.stdout,
        "heap-key map insert+lookup must match interpreter"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n199");

    let exe = rsscript::reg_vm_compile_source(file, source).expect("compiles");
    let (_o, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert!(
        stats.osr_entries > 0,
        "hot heap-key insert loop must OSR: {stats:?}"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_whole_function_heap_key_get_matches_interpreter() {
    // No internal hot loop in `lookup` → if it goes native it's the WHOLE-FUNCTION tier.
    let source = "\
fn lookup(m: read Map<String, Int>, k: read String) -> Int {
    match Map.get<String, Int>(map: read m, key: read k) {
        Some(v) => { return v }
        None => { return 0 - 1 }
    }
}
fn main() -> Unit {
    let mut m = Map<String, Int>.new()
    Map.insert<String, Int>(map: mut m, key: read \"alpha\", value: read 11)
    Map.insert<String, Int>(map: mut m, key: read \"beta\", value: read 22)
    let mut i = 0
    let mut acc = 0
    while i < 300 {
        acc = acc + lookup(m: read m, k: read \"beta\")
        i = i + 1
    }
    Log.write(message: read String.from_int(value: acc))
    return Unit
}
";
    let file = "wf.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let nat = rsscript::reg_vm_eval_source_main_native(file, source, std::iter::empty::<String>())
        .expect("native");
    eprintln!(
        "WF interp={:?} native={:?}",
        interp.stdout.trim_end(),
        nat.stdout.trim_end()
    );
    assert_eq!(
        interp.stdout, nat.stdout,
        "whole-function heap-key get correctness"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_whole_function_heap_key_insert_matches_interpreter() {
    // `add` has NO internal loop → native compile would be the whole-function tier.
    let source = "\
fn add(m: mut Map<String, Int>, k: read String, v: Int) -> Int {
    Map.insert<String, Int>(map: mut m, key: read k, value: read v)
    match Map.get<String, Int>(map: read m, key: read k) {
        Some(x) => { return x }
        None => { return 0 - 1 }
    }
}
fn main() -> Unit {
    let mut m = Map<String, Int>.new()
    let mut i = 0
    let mut acc = 0
    while i < 400 {
        acc = add(m: mut m, k: read \"key\", v: read i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: acc))
    return Unit
}
";
    let file = "wf2.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native");
    eprintln!(
        "WF2 interp={:?} native={:?} considered={} compiled={} native_calls={}",
        interp.stdout.trim_end(),
        nat.stdout.trim_end(),
        stats.considered,
        stats.compiled,
        stats.native_calls
    );
    assert_eq!(
        interp.stdout, nat.stdout,
        "whole-function heap-key insert correctness"
    );
}

/// J0.4 #1 correctness (heap Set/SortedMap): DISCRIMINATING guards. Insert distinct heap
/// keys/values in a hot loop, then probe membership / look a value up. A mis-typed
/// (handle-index Int) operand would store wrong keys and make the probe diverge.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_heap_set_and_sorted_map_discriminating() {
    // Set<String>: insert two DISTINCT keys repeatedly, then test membership of each.
    let set_src = "\
fn build(a: read String, b: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut s = Set<String>.new()
    let mut i = 0
    while i < n {
        Set.insert<String>(set: mut s, value: read a)
        Set.insert<String>(set: mut s, value: read b)
        i = i + 1
    }
    let mut r = 0
    if Set.contains<String>(set: read s, value: read a) { r = r + 1 }
    if Set.contains<String>(set: read s, value: read b) { r = r + 10 }
    if Set.contains<String>(set: read s, value: read \"absent\") { r = r + 100 }
    return r
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(a: read \"x\", b: read \"y\", n: read 200)))
    return Unit
}
";
    let interp = common::run_vm_source("j1-set-disc.rss", set_src, &[]).expect("interp");
    let osr = rsscript::reg_vm_eval_source_main_native_osr(
        "j1-set-disc.rss",
        set_src,
        std::iter::empty::<String>(),
    )
    .expect("osr");
    assert_eq!(
        interp.stdout, osr.stdout,
        "Set<String> insert+contains must match interpreter"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n11");

    // SortedMap<String,Int>: insert k -> i, then look k back up.
    let sm_src = "\
fn build(k: read String, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut m = SortedMap<String, Int>.new()
    let mut i = 0
    while i < n {
        SortedMap.insert<String, Int>(map: mut m, key: read k, value: read i)
        i = i + 1
    }
    match SortedMap.get<String, Int>(map: read m, key: read k) {
        Some(v) => { return v }
        None => { return 0 - 1 }
    }
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(k: read \"key\", n: read 200)))
    return Unit
}
";
    let interp2 = common::run_vm_source("j1-sortedmap-disc.rss", sm_src, &[]).expect("interp");
    let osr2 = rsscript::reg_vm_eval_source_main_native_osr(
        "j1-sortedmap-disc.rss",
        sm_src,
        std::iter::empty::<String>(),
    )
    .expect("osr");
    assert_eq!(
        interp2.stdout, osr2.stdout,
        "SortedMap<String,Int> insert+lookup must match interpreter"
    );
    assert_eq!(osr2.stdout.trim_end(), "begin\n199");
}

/// J0.5 mem (PARITY, #6): a flat `List<Int>` build loop now RUNS natively under an armed
/// `mem_budget` — `ListPush*` charges the flat-capacity growth in its host helper,
/// mirroring the interpreter's `account_bytes`. DISCRIMINATING: under a generous budget it
/// OSRs and completes; under a tight budget it must ERROR exactly like the interpreter (if
/// native failed to charge the pushes, it would complete and DIVERGE — returning a value
/// where the interpreter errors). A mem-over-budget bail rolls back the loop's list writes
/// and reruns on the interpreter, which recharges and errors at the precise push.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_list_push_int_charges_mem_budget() {
    let source = "\
fn build(n: Int) -> Int {
    let mut xs = List<Int>.new()
    let mut i = 0
    while i < n {
        List.push<Int>(list: mut xs, value: read i)
        i = i + 1
    }
    return List.len(list: read xs)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(n: read 20000)))
    return Unit
}
";
    let file = "j05-list-push-mem.rss";
    // (A) Generous budget: the loop OSRs, charges per push, stays within budget, completes.
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let ok = rsscript::VmLimits {
        mem_budget: Some(1 << 24),
        ..rsscript::VmLimits::default()
    };
    let (out, stats) = exe
        .eval_main_with_args_native_osr_with_limits(std::iter::empty::<String>(), ok)
        .expect("flat list-push loop must run under a generous mem_budget");
    assert_eq!(out.stdout.trim_end(), "20000");
    assert_eq!(
        stats.osr_entries, 0,
        "armed memory budgets must stay interpreted"
    );

    // (B) Tight budget the build exceeds: native must ERROR identically to the interpreter.
    let interp_err = rsscript::reg_vm_eval_source_main_with_limits(
        file,
        source,
        std::iter::empty::<String>(),
        rsscript::VmLimits {
            mem_budget: Some(16384),
            ..rsscript::VmLimits::default()
        },
    )
    .expect_err("interpreter must exceed the tight mem_budget");
    let exe2 = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let nat_err = exe2
        .eval_main_with_args_native_osr_with_limits(
            std::iter::empty::<String>(),
            rsscript::VmLimits {
                mem_budget: Some(16384),
                ..rsscript::VmLimits::default()
            },
        )
        .expect_err("native must ALSO exceed (it charges ListPush growth)");
    assert_eq!(
        format!("{interp_err:?}"),
        format!("{nat_err:?}"),
        "mem-over-budget error must match the interpreter exactly",
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_osr_aliased_struct_field_write_matches_interpreter() {
    // J0.4 #8 (aliased heap in-place write): a caller-aliased `mut Acc` struct whose
    // scalar field is read-modify-written in a hot loop. The function is I/O-wrapped
    // (`Log.write`) so it is whole-tier-INELIGIBLE and the loop reaches the OSR path
    // (a body with no I/O is tier-0-dispatched at the call site and never OSRs). The
    // OSR scalar-field replacement dissolves the field RMW to a loop-carried scalar,
    // writes it back to the struct on OSR exit, and the `mut`-param propagation carries
    // the result to the caller — so `main`'s `a.total` (read AFTER the call) must match
    // the interpreter. DISCRIMINATING: reads the field back through the caller, not via
    // the callee return alone, so a writeback that failed to propagate would diverge.
    let source = "\
struct Acc { total: Int }
fn bump(a: mut Acc, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    while i < n {
        a.total = a.total + 2
        i = i + 1
    }
    return a.total
}
fn main() -> Unit {
    let mut a = Acc(total: 0)
    let r = bump(a: mut a, n: read 3000)
    Log.write(message: read String.from_int(value: r))
    Log.write(message: read String.from_int(value: a.total))
    return Unit
}
";
    let file = "s4.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(
        interp.stdout, nat.stdout,
        "aliased struct field write must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "aliased struct field RMW must OSR (entries={})",
        stats.osr_entries
    );
}

/// J0.4 #8 (aliased heap in-place write — Map): a caller-aliased `mut Map<Int,Int>`
/// inserted into in a hot loop, then read back THROUGH the caller after the call. The
/// function is I/O-wrapped so the loop reaches the OSR path. A Map is `Rc<RefCell<..>>`,
/// so the in-place insert helper mutates the shared map and the caller observes it.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_aliased_map_insert_matches_interpreter() {
    let source = "\
fn fill(m: mut Map<Int, Int>, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    while i < n {
        Map.insert<Int, Int>(map: mut m, key: read i, value: read i)
        i = i + 1
    }
    return n
}
fn main() -> Unit {
    let mut m = Map<Int, Int>.new()
    let r = fill(m: mut m, n: read 2000)
    match Map.get<Int, Int>(map: read m, key: read 1999) {
        Some(v) => { Log.write(message: read String.from_int(value: v)) }
        None => { Log.write(message: read \"missing\") }
    }
    Log.write(message: read String.from_int(value: r))
    return Unit
}
";
    let file = "s4_map.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(
        interp.stdout, nat.stdout,
        "aliased map insert must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "aliased map insert must OSR (entries={})",
        stats.osr_entries
    );
}

/// J0.4 #8 (aliased heap in-place write — Deque): a caller-aliased `mut Deque<Int>`
/// pushed to in a hot loop, then read back THROUGH the caller. Deque is `Rc<RefCell<..>>`
/// so the in-place push propagates to the caller.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_aliased_deque_push_matches_interpreter() {
    let source = "\
fn fill(d: mut Deque<Int>, n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    while i < n {
        Deque.push_back<Int>(deque: mut d, value: read i)
        i = i + 1
    }
    return n
}
fn main() -> Unit {
    let mut d = Deque<Int>.new()
    let r = fill(d: mut d, n: read 2000)
    Log.write(message: read String.from_int(value: Deque.len<Int>(deque: read d)))
    Log.write(message: read String.from_int(value: r))
    return Unit
}
";
    let file = "s4_deque.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(
        interp.stdout, nat.stdout,
        "aliased deque push must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "aliased deque push must OSR (entries={})",
        stats.osr_entries
    );
}

/// J0.1 #7 probe: an inlined leaf call inside a hot OSR loop, where the callee has a
/// COLD arm that builds a heap value (lowers to a native `Bail`). The cold arm IS taken
/// partway through (at i==1500), forcing a mid-inlined-call deopt. The existing
/// rollback+rerun must reproduce the interpreter's output exactly (the "inline
/// frame-chain" would make resume PRECISE, but correctness holds via rerun either way).
/// I/O-wrapped so the function is whole-tier-ineligible and the loop reaches OSR.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_cold_bail_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 {
        let s = String.from_int(value: read x)
        return String.len(value: read s)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    // #7 foldable cold-arm sub-case (now SUPPORTED): `classify`'s cold arm is
    // `String.len(String.from_int(x))`, which the whole-body string-fold dissolves to
    // digit-count arithmetic BEFORE the inlinability check — so the leaf becomes
    // pure-scalar native, inlines, and the loop OSRs (no heap arm left to bail on). The
    // fold is semantics-preserving, so output still matches the interpreter exactly.
    assert_eq!(
        interp.stdout, nat.stdout,
        "inlined leaf call with foldable cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "foldable cold-arm inlined leaf call must OSR after callee-fold (entries={})",
        stats.osr_entries
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_scalar_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 { return 7 }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7b.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(interp.stdout, nat.stdout);
    assert!(
        stats.osr_entries >= 1,
        "pure-scalar inlined leaf call must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 foldable cold-arm sub-case (Bytes sibling): an inlined leaf whose cold arm builds a
/// measured-throwaway Bytes value (`Bytes.len(Bytes.from_string(..))`). The chained
/// callee-fold (string then Bytes length-law fold) dissolves it to byte-length arithmetic
/// before the inlinability check, so the leaf becomes pure-scalar native, inlines, and the
/// loop OSRs — with byte-exact interpreter parity (the fold is semantics-preserving).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_bytes_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 {
        let b = Bytes.from_string(value: read \"hello\")
        return Bytes.len(value: read b)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7c.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(
        interp.stdout, nat.stdout,
        "bytes cold-arm inlined leaf must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "foldable bytes cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, pure scalar READER): a cold arm that builds a
/// heap String (`String.from_int`) and returns an Int via a NON-length, NON-foldable
/// intrinsic (`String.count`). The heap source `s` is DEAD at the arm boundary (consumed
/// by `count` inside the arm); the live-out value is the SCALAR `count` result — so this
/// is a deopt-replaceable cold arm, NOT a frame-chain case. It previously declined OSR
/// only because `String.count` was not classified as a `cold_arm_pure_value_op` (the arm
/// was never detected). Now `String.count`/`contains`/`index_of`/`starts_with` are
/// `cold_arm_pure_reader`s (pure, first-order, side-effect-free, scalar-returning,
/// re-runnable after a `Bail`), so the arm is detected, spliced to a native `Bail`, the
/// leaf inlines, and the loop OSRs. The rare cold path (i==1500) rolls back and re-runs
/// on the interpreter, which recomputes the count faithfully. Must match the interpreter
/// AND OSR.
///
/// The TRUE remaining frame-chain domain is a cold arm containing a WRITE/SUSPEND op or a
/// higher-order closure combinator (not side-effect-free, so not re-runnable by a plain
/// Bail) — those correctly decline today. Per the 2026-06-29 analysis that is a PERF-only
/// gap (§7.2 makes abandon-and-reinterpret always correct), not a correctness gap.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_pure_reader_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 {
        let s = String.from_int(value: read x)
        return String.count(value: read s, needle: read \"5\")
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7d.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(
        interp.stdout, nat.stdout,
        "pure-reader cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "pure-reader (String.count) cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29): a leaf whose COLD arm RETURNS a heap value
/// built by `String.slice` — a pure, read-only Allocate producer now on the
/// `cold_arm_pure_builder` whitelist. The payload is the live `Err` value (NOT measured
/// by `String.len`, so no length-fold applies); before the whitelist expansion the arm
/// was undetectable and the leaf declined inlining (loop did not OSR). Now the arm is a
/// deopt-replaceable cold arm spliced to a native `Bail`: the loop OSRs, and the rare
/// cold path (i==1500) rolls back and re-runs on the interpreter, which rebuilds the
/// sliced `Err` faithfully. Must match the interpreter byte-for-byte AND OSR.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_string_slice_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Result<Int, String> {
    if x == 1500 {
        return Err(String.slice(value: read \"boundary value reached here\", start: read 0, len: read 8))
    }
    return Ok(read x)
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        match classify(x: read i) {
            Ok(v) => { acc = acc + v }
            Err(e) => { acc = acc + String.len(value: read e) }
        }
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7e.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(
        interp.stdout, nat.stdout,
        "String.slice cold-arm inlined leaf must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "String.slice cold-arm inlined leaf must OSR after whitelist expansion (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, second builder): a leaf whose COLD arm RETURNS
/// a heap value built by `String.pad_left` — another pure, read-only Allocate producer
/// now on the `cold_arm_pure_builder` whitelist. Same shape as the `String.slice` case:
/// the heap `Err` payload is live (consumed via `String.len`), not folded; the arm
/// becomes a deopt-replaceable cold arm and the loop OSRs, with the rare cold path
/// re-running on the interpreter. Must match the interpreter byte-for-byte AND OSR.
///
/// (The `Bytes` builders `BytesFromString`/`BytesSlice` are equally whitelisted and
/// unit-covered by `cold_arm_pure_builders_whitelist`, but a cold arm returning a `Bytes`
/// `Err` payload does not OSR here — `Result<_, Bytes>` payload dissolution is a separate,
/// not-yet-implemented native path, orthogonal to this whitelist. Demonstrated with the
/// String-payload builders, which the two-armed heap-Result path already supports.)
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_pad_left_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Result<Int, String> {
    if x == 1500 {
        return Err(String.pad_left(value: read \"x\", width: 8, fill: read \"*\"))
    }
    return Ok(read x)
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        match classify(x: read i) {
            Ok(v) => { acc = acc + v }
            Err(e) => { acc = acc + String.len(value: read e) }
        }
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7f.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr");
    assert_eq!(
        interp.stdout, nat.stdout,
        "String.pad_left cold-arm inlined leaf must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "String.pad_left cold-arm inlined leaf must OSR after whitelist expansion (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, arm-LOCAL WRITE): a cold arm that builds a
/// fresh `List`, PUSHES to it, and returns a scalar query of it
/// (`let t = []; t.push(x); return List.len(t)`). `ListPush` is a heap WRITE, but it is
/// safe in a deopt-replaceable cold arm: native bails at the arm start and NEVER executes
/// the arm, so the interpreter re-runs the whole arm (push included) on
/// abandon-and-reinterpret. The classifier additionally requires the mutated collection
/// to be DEFINED INSIDE the arm (non-escaping / not caller-aliased — guarded by the
/// arm-local mutation check), so the fallback has no aliased-heap interaction to reason
/// about. This is the safe, OUTPUT-TESTABLE half of the former frame-chain "boundary":
/// the arm is spliced to a native `Bail`, the leaf inlines, and the loop OSRs, with the
/// rare cold path re-running on the interpreter. Must match the interpreter AND OSR.
///
/// The remaining declined cases are now narrower still: cold arms mutating a CALLER-ALIASED
/// collection, or containing a Suspend / higher-order closure combinator. Those are
/// perf-only (§7.2 keeps correctness) and still decline pending a directed per-case repro.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_arm_local_write_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 {
        let mut tmp: List<Int> = []
        List.push(list: mut tmp, value: read x)
        return List.len(list: read tmp)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7g.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    assert_eq!(
        interp.stdout, nat.stdout,
        "arm-local write cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "arm-local write (List.push) cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, Map write): the same cold-arm-write pattern
/// generalized to `Map` — `let m = Map.new(); m.insert(k, v); return Map.len(m)`.
/// `MapInsert` is a heap WRITE; native bails at the arm start and never executes it, so the
/// interpreter re-runs the whole arm on replay. Exercises `MakeMap`/`MapInsert` in
/// `cold_arm_pure_value_op` plus `MapNew`/`MapLen` as cold-arm builder/reader. Must OSR+match.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_arm_local_map_write_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 {
        let mut m = Map<Int, Int>.new()
        Map.insert(map: mut m, key: read x, value: read x)
        return Map.len(map: read m)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7h.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    assert_eq!(
        interp.stdout, nat.stdout,
        "arm-local map write cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "arm-local write (Map.insert) cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, arm-local Set write): arm-local-write pattern
/// generalized to `Set` — `let s = Set.new(); s.insert(x); return Set.len(s)`.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_arm_local_set_write_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 {
        let mut s = Set<Int>.new()
        Set.insert(set: mut s, value: read x)
        return Set.len(set: read s)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7i.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    assert_eq!(
        interp.stdout, nat.stdout,
        "arm-local set write cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "arm-local write (Set.insert) cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, arm-local Deque write): arm-local-write pattern
/// generalized to `Deque` — `let d = Deque.new(); d.push_back(x); return Deque.len(d)`.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_arm_local_deque_write_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 {
        let mut d = Deque<Int>.new()
        Deque.push_back(deque: mut d, value: read x)
        return Deque.len(deque: read d)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7j.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    assert_eq!(
        interp.stdout, nat.stdout,
        "arm-local deque write cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "arm-local write (Deque.push_back) cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, caller-ALIASED write): `classify` takes a
/// `mut List` param and pushes to it ONLY in the cold arm (i==1500) — the caller (`run`)
/// also holds `acc` and reads it back after the loop. This is the case the arm-local guard
/// previously rejected; it is now admitted because a cold-arm `Bail` is provably handled by
/// abort+replay (cold arms are inline-only → non-identity ip_map → precise resume is
/// structurally unreachable; the OSR handler fallbacks any mid-loop bail). The loop OSRs and
/// the rare cold path rolls back all journaled native writes and re-runs on the interpreter,
/// which performs the aliased push itself. Must match the interpreter byte-for-byte AND OSR.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_aliased_write_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int, acc: mut List<Int>) -> Int {
    if x == 1500 {
        List.push(list: mut acc, value: read x)
        return 0
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc: List<Int> = []
    let mut total = 0
    let mut i = 0
    while i < n {
        total = total + classify(x: read i, acc: mut acc)
        i = i + 1
    }
    return total + List.len(list: read acc)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7k.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    assert_eq!(
        interp.stdout, nat.stdout,
        "aliased cold-arm write must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "caller-aliased write cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, nested CALL): a cold arm that CALLS another
/// function (`helper`, deliberately non-inlinable — it does I/O — so it stays a `CallKnown`
/// rather than being inlined into a pure-value arm). The call is admitted in the bailable
/// cold arm: native bails at the arm start and never executes it, and the cold-arm `Bail`
/// always takes abort+replay, so the interpreter runs `helper` ONCE on replay — its I/O and
/// return happen exactly as without the JIT. The loop OSRs; the rare cold path (i==1500)
/// re-runs on the interpreter. Must match the interpreter byte-for-byte AND OSR.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_nested_call_cold_arm_matches_interpreter() {
    let source = "\
fn helper(x: Int) -> Int {
    Log.write(message: read \"cold path hit\")
    return x * 7
}
fn classify(x: Int) -> Int {
    if x == 1500 {
        return helper(x: read x)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7l.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    assert_eq!(
        interp.stdout, nat.stdout,
        "nested-call cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "nested-call cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, mut-arg nested CALL): a cold arm calls a
/// non-inlinable function (`appendErr`, does I/O) passing a `mut` collection argument. The
/// mut-arg writeback into the caller's register only happens on the cold/bail path
/// (interpreter replay), never in native — the same situation as a caller-aliased heap
/// write, sound under abort+replay. (`classify` itself takes no `mut` param — a mut-param
/// *leaf* would not inline — so the mutated `tmp` is arm-local; what is exercised here is
/// the mut-ARG `CallKnown` in the cold arm.) Must match the interpreter AND OSR.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_mut_arg_nested_call_cold_arm_matches_interpreter() {
    let source = "\
fn appendErr(acc: mut List<Int>, x: Int) -> Int {
    Log.write(message: read \"appending\")
    List.push(list: mut acc, value: read x)
    return List.len(list: read acc)
}
fn classify(x: Int) -> Int {
    if x == 1500 {
        let mut tmp: List<Int> = []
        return appendErr(acc: mut tmp, x: read x)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7m.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    assert_eq!(
        interp.stdout, nat.stdout,
        "mut-arg nested-call cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "mut-arg nested-call cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage slice (2026-06-29, higher-order COMBINATOR): a cold arm that builds
/// a closure and runs an `Option.map` / `Option.unwrap_or` combinator chain over it. The
/// combinators are higher-order (they invoke the closure), but that is safe in a bailable
/// cold arm — native bails at the arm start and never executes the arm, so the combinator
/// and its closure run ONLY on the interpreter replay (their effects happen exactly as
/// without the JIT). The loop OSRs; the rare cold path re-runs on the interpreter. Must
/// match the interpreter byte-for-byte AND OSR.
/// REVIEW REPRO (2026-06-30): non-`mut` heap param mutated in native must NOT leak to the
/// caller. The interpreter deep-copies a non-`mut` param at entry (pass-by-value), so the
/// caller's list is unchanged; native drops `DeepCopy`, so if it writes the param's buffer
/// in place it would leak. `xs` is non-`mut`; `mutate` sets index 0 in a hot loop.
#[cfg(feature = "native-jit")]
#[test]
fn native_non_mut_heap_param_mutation_does_not_leak_to_caller() {
    let source = "\
features: local

fn mutate(xs: read List<Int>, limit: Int) -> Int {
    let mut ys = xs
    let mut i = 0
    while i < limit {
        List.set(list: mut ys, index: 0, value: read i)
        i = i + 1
    }
    return List.get(list: read ys, index: 0)
}
fn main() -> Unit {
    local xs = List<Int>.new()
    List.push(list: mut xs, value: read 7)
    let r = mutate(xs: read xs, limit: read 200000)
    Log.write(message: read String.from_int(value: List.get(list: read xs, index: 0)))
    Log.write(message: read String.from_int(value: r))
    return Unit
}
";
    let file = "deepcopy_leak.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, _stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    // Caller's xs[0] must stay 7 (deep-copied); the local copy returns 199999. The native
    // path must DECLINE rather than mutate the caller's `Rc` in place (the soundness guard).
    assert_eq!(interp.stdout.trim_end(), "7\n199999");
    assert_eq!(
        interp.stdout, nat.stdout,
        "non-mut heap param mutation must not leak to caller"
    );
}

/// REVIEW REPRO (2026-07-02): the `scalar_regs` register-reuse elision leak.
/// `local(name)` reuses one register per variable name, so a loop var bound to a
/// scalar in one loop and a heap value in another shared a register; the (default-on)
/// DeepCopy-elision analysis then wrongly marked the heap binding scalar and elided
/// its copy, so the returned inner list aliased the caller's `grid` and a later push
/// mutated it. With the poison fix the copy is kept: `grid[0]` stays length 1.
#[test]
fn elision_reused_loop_var_scalar_then_heap_does_not_leak() {
    let source = "\
features: local

fn pick(nums: read List<Int>, grid: read List<List<Int>>) -> List<Int> {
    for item in nums {
        let bump = item + 1
        Log.write(message: read String.from_int(value: bump))
    }
    for item in grid {
        return item
    }
    return List<Int>.new()
}
fn main() -> Unit {
    let grid = [ [1] ]
    let nums = [ 7 ]
    local got = pick(nums: read nums, grid: read grid)
    List.push<Int>(list: mut got, value: read 99)
    let first = List.get<List<Int>>(list: read grid, index: 0)
    Log.write(message: read String.from_int(value: List.len(list: read first)))
    return Unit
}
";
    let interp = common::run_vm_source("elision_reuse_leak.rss", source, &[]).expect("interp run");
    // `8` from the scalar loop, then `1` — grid[0] must NOT have grown to 2.
    assert_eq!(interp.stdout.trim_end(), "8\n1");
}

/// REVIEW REPRO (2026-07-02): returning a `mut` scalar parameter used to panic in
/// the reg-VM ("read uninitialized register") because `Return` moved the value out
/// of the register the subsequent `mut`-writeback still needed. It must return the
/// value cleanly.
#[test]
fn mut_scalar_param_returned_does_not_crash() {
    let source = "\
features: local

fn ident(i: mut Int) -> Int {
    return i
}
fn main() -> Unit {
    local x = 5
    let y = ident(i: mut x)
    Log.write(message: read String.from_int(value: y))
    return Unit
}
";
    let interp = common::run_vm_source("mut_return.rss", source, &[]).expect("interp run");
    assert_eq!(interp.stdout.trim_end(), "5");
}

/// REVIEW REPRO (2026-07-02): a `mut` scalar CLASS-FIELD argument used to be a
/// silent no-op on the reg-VM — the field was lowered to a temp for the call, the
/// callee's mutation was written back into the temp, but never stored back into
/// the field. The VM now restores `mut`-place args after the call (matching AOT's
/// `&mut` place semantics), so `w.count` reflects the callee's increment.
#[test]
fn mut_scalar_class_field_arg_is_written_back() {
    let source = "\
class Widget {
    count: Int
}
fn bump(n: mut Int) -> Unit {
    n = n + 1
    return Unit
}
fn main() -> Unit {
    let w = Widget(count: 5)
    bump(n: mut w.count)
    Log.write(message: read String.from_int(value: w.count))
    return Unit
}
";
    let interp = common::run_vm_source("mut_field_writeback.rss", source, &[]).expect("interp run");
    assert_eq!(interp.stdout.trim_end(), "6");
}

/// REVIEW REPRO (2026-06-30): the store-and-reload leak. A non-`mut` `read List<Int>` param
/// is stored into a struct field, read back, and mutated. Storing launders direct-alias
/// taint, so the guard must also flag STORING a tainted (mutable) value into caller-visible
/// heap. Interpreter mutates the deep copy (caller's `xs[0]` stays 7); native must DECLINE
/// rather than store/mutate the caller's original handle. Companion to the direct-alias test.
#[cfg(feature = "native-jit")]
#[test]
fn native_store_reload_mutate_non_mut_heap_param_does_not_leak() {
    let source = "\
features: local

struct Box {
    items: List<Int>
}
fn leak(xs: read List<Int>, n: Int) -> Int {
    let b = Box(items: read xs)
    let mut i = 0
    while i < n {
        let mut inner = b.items
        List.set(list: mut inner, index: 0, value: read i)
        i = i + 1
    }
    return List.get(list: read xs, index: 0)
}
fn main() -> Unit {
    local xs = List<Int>.new()
    List.push(list: mut xs, value: read 7)
    let r = leak(xs: read xs, n: read 200000)
    Log.write(message: read String.from_int(value: List.get(list: read xs, index: 0)))
    Log.write(message: read String.from_int(value: r))
    return Unit
}
";
    let file = "store_reload_leak.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, _stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    // Caller's xs[0] stays 7 (interpreter mutated only the deep copy); native must match.
    assert_eq!(interp.stdout.trim_end(), "7\n199999");
    assert_eq!(
        interp.stdout, nat.stdout,
        "store-and-reload mutation of a non-mut heap param must not leak to caller"
    );
}

/// REVIEW REPRO (2026-06-30): the INLINED-callee store leak. A hot loop calls `stash`, a
/// native-inlinable leaf taking `xs: read List<Int>` and storing it into a `mut` struct
/// field; after the loop the field is reloaded and mutated. The callee param's `DeepCopy` is
/// spliced into the loop with an offset register NOT covered by the outer signature — the
/// previous two-set guard left it un-store-tainted, so the leak slipped through. The
/// proven-immutable analysis now classifies the inlined param via its arg-marshalling `Move`
/// (it resolves to a non-immutable local list ⇒ unproven ⇒ tainted), so the store is flagged
/// and native declines. Interpreter stores a deep copy each call (caller's `xs[0]` stays 7);
/// native must match.
#[cfg(feature = "native-jit")]
#[test]
fn native_inlined_leaf_store_of_non_mut_heap_param_does_not_leak() {
    let source = "\
features: local

struct Holder {
    items: List<Int>
}
fn stash(xs: read List<Int>, h: mut Holder) -> Int {
    h.items = read xs
    return List.len(list: read xs)
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    local holder = Holder(items: List<Int>.new())
    local xs = List<Int>.new()
    List.push(list: mut xs, value: read 7)
    let mut i = 0
    let mut acc = 0
    while i < n {
        acc = acc + stash(xs: read xs, h: mut holder)
        i = i + 1
    }
    let mut inner = holder.items
    List.set(list: mut inner, index: 0, value: read 999)
    return List.get(list: read xs, index: 0)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "inlined_store_leak.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, _stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    // Caller's xs[0] must stay 7 (interpreter stored a deep copy); native must not leak the
    // original handle into `holder` and then mutate it.
    assert_eq!(interp.stdout.trim_end(), "begin\n7");
    assert_eq!(
        interp.stdout, nat.stdout,
        "inlined-leaf store of a non-mut heap param must not leak to caller"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_combinator_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 {
        let opt: Option<Int> = Some(read x)
        let mapped = Option.map<Int, Int>(value: read opt, mapper: |v| { return v * 2 })
        return Option.unwrap_or<Int>(value: read mapped, default: read 0)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7n.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    assert_eq!(
        interp.stdout, nat.stdout,
        "combinator cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "higher-order combinator cold-arm inlined leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// #7 cold-arm coverage (2026-06-29, CAPTURING-closure combinator): like the combinator
/// test but the cold-arm closure CAPTURES a loop value (`|v| v + x`), so `MakeClosure` has
/// a non-empty capture list — a distinct path from the captureless case. On the cold-arm
/// bail the interpreter rebuilds the closure (capturing the replayed value) and runs it.
/// Must match the interpreter AND OSR.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_call_capturing_combinator_cold_arm_matches_interpreter() {
    let source = "\
fn classify(x: Int) -> Int {
    if x == 1500 {
        let opt: Option<Int> = Some(read x)
        let mapped = Option.map<Int, Int>(value: read opt, mapper: |v| { return v + x })
        return Option.unwrap_or<Int>(value: read mapped, default: read 0)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < n {
        acc = acc + classify(x: read i)
        i = i + 1
    }
    return acc
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7p.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    assert_eq!(
        interp.stdout, nat.stdout,
        "capturing-closure combinator cold arm must match interpreter"
    );
    assert!(
        stats.osr_entries >= 1,
        "capturing-closure combinator cold-arm leaf must OSR (entries={})",
        stats.osr_entries
    );
}

/// REVIEW PROBE (2026-06-29): the dangerous hot+cold aliased double-write. The HOT path
/// does a native aliased `List.push` EVERY iteration (#8 in-place write to the caller's
/// `acc`), and the cold arm (i==1500) forces a deopt-replaceable bail (heap build + pure
/// reader). On the cold-arm bail, `heap_tx.abort()` MUST roll back the native hot-path
/// pushes accumulated since OSR entry before the interpreter replays — otherwise `acc`
/// double-counts. `run` reads `List.len(acc)` back, so a rollback gap shows as a wrong
/// length. This is the exact case the aliased-write admission's soundness depends on and
/// that the shipped aliased test did NOT exercise (its hot path never touched `acc`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_hot_and_cold_aliased_write_rollback_matches_interpreter() {
    let source = "\
fn classify(x: Int, acc: mut List<Int>) -> Int {
    List.push(list: mut acc, value: read x)
    if x == 1500 {
        let s = String.from_int(value: read x)
        return String.count(value: read s, needle: read \"5\")
    }
    return 0
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc: List<Int> = []
    let mut total = 0
    let mut i = 0
    while i < n {
        total = total + classify(x: read i, acc: mut acc)
        i = i + 1
    }
    return total + List.len(list: read acc)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7probe.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    // `acc` is pushed every iteration (0..2999) = 3000 elems; classify returns 1 only at
    // i==1500 (`String.count("1500","5")`), else 0 -> 1 + 3000 = 3001. A rollback gap on
    // the cold-arm bail would double-count `acc`.
    assert_eq!(nat.stdout.trim_end(), "begin\n3001");
    assert_eq!(
        interp.stdout, nat.stdout,
        "hot+cold aliased write must match interpreter (rollback of native hot pushes)"
    );
    assert!(
        stats.osr_entries >= 1,
        "hot+cold aliased write loop must OSR (entries={})",
        stats.osr_entries
    );
}

/// REVIEW (2026-06-29) CHARACTERIZATION: the boundary of the mut-arg cold-arm support. A
/// leaf that takes a `mut` collection PARAM and passes it as a `mut` ARG to a non-inlinable
/// callee in a cold arm (run -> classify(mut acc) -> appendErr(mut acc)) currently DECLINES
/// native OSR (osr_entries=0) and runs on the interpreter — correct, just not optimized.
/// This pins the real boundary (correcting an earlier imprecise note): a mut-param leaf
/// with a DIRECT heap write in its cold arm DOES inline+OSR (see
/// `native_osr_inlined_leaf_call_aliased_write_cold_arm_matches_interpreter`); it is the
/// two-level mut-arg writeback through the inline boundary to a caller-aliased target that
/// declines. The arm-local mut-arg call (`..._mut_arg_nested_call_...`) OSRs. Output parity
/// holds regardless; only OSR-ness differs.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inlined_leaf_caller_aliased_mut_arg_call_cold_arm_matches_interpreter() {
    let source = "\
fn appendErr(acc: mut List<Int>, x: Int) -> Int {
    Log.write(message: read \"appending\")
    List.push(list: mut acc, value: read x)
    return List.len(list: read acc)
}
fn classify(x: Int, acc: mut List<Int>) -> Int {
    if x == 1500 {
        return appendErr(acc: mut acc, x: read x)
    }
    return x + 1
}
fn run(n: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc: List<Int> = []
    let mut total = 0
    let mut i = 0
    while i < n {
        total = total + classify(x: read i, acc: mut acc)
        i = i + 1
    }
    return total + List.len(list: read acc)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: run(n: read 3000)))
    return Unit
}
";
    let file = "p7o.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp");
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (nat, _stats) = exe
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("run");
    // Correctness is the contract; this shape declines OSR today (characterization).
    assert_eq!(
        interp.stdout, nat.stdout,
        "caller-aliased mut-arg call cold arm must match interpreter even when it declines"
    );
}

/// Item #2 prerequisite (transform-driver / deopt-mapping safety): a DEEP
/// multi-transform OSR region — an inlined leaf call, a non-escaping `Option<Int>`,
/// and a non-escaping `Result<Int,Int>` (user variant) all dissolved by scalar
/// replacement inside one hot loop, so several region transforms and their ip-maps
/// compose. Run through every fast backend INCLUDING deopt-every-safepoint, which
/// forces a bail at each native safepoint and resumes via the composed ip-map — so a
/// wrong ip-map composition (the exact failure a shared transform driver could
/// introduce) would diverge here. Byte-identical parity is the guard.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_deep_multi_transform_survives_deopt_every_safepoint() {
    let source = "\
fn leaf(x: Int) -> Int {
    return x * 3 - 1
}

fn hot(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut acc = 0
    let mut i = 0
    while i < limit {
        let mut o: Option<Int> = None
        if i % 2 == 0 {
            o = Some(leaf(x: read i))
        } else {
            o = None
        }
        let mut r: Result<Int, Int> = Ok(read 0)
        if i % 3 == 0 {
            r = Ok(read i)
        } else {
            let neg = 0 - i
            r = Err(read neg)
        }
        match o {
            Some(v) => {
                acc = acc + v
            }
            None => {
                acc = acc + 1
            }
        }
        match r {
            Ok(v) => {
                acc = acc + v
            }
            Err(e) => {
                acc = acc - e
            }
        }
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: hot(limit: read 200)))
    return Unit
}
";
    let file = "jit-osr-deep-multi-transform.rss";
    // Deopt-every parity across the whole composed transform pipeline.
    assert_fast_jit_backends_agree(file, source);
    // Confirm it actually exercises the OSR pipeline (not trivially declined).
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (_out, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert!(
        stats.osr_entries > 0,
        "the deep multi-transform loop should OSR natively: {stats:?}",
    );
}
