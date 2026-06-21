//! Fast JIT acceptance matrix for local VM/JIT work.
//!
//! These tests intentionally stay in the `runtime` target and use only the
//! in-process backend set: interpreter, tier-0 JIT, and, with `native-jit`,
//! native plus force-deopt. The slower generated-Rust backend remains covered by
//! the `differential` target.

mod common;

use common::differential::{assert_backends_agree_on, fast_backends};

fn assert_fast_jit_backends_agree(file: &str, source: &str) {
    assert_backends_agree_on(file, source, &[], &fast_backends());
}

#[test]
fn jit_acceptance_runs_cross_function_loop_calls() {
    let source = "\
fn square(n: Int) -> Int {
    return n * n
}

fn weight(n: Int) -> Int {
    if n < 0 {
        return 0 - n
    }
    return n
}

fn accumulate(limit: Int) -> Int {
    let mut total = 0
    let mut i = 0
    while i < limit {
        total = total + square(n: read i) - weight(n: read i)
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: accumulate(limit: read 8)))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-cross-call.rss", source);
}

#[test]
fn jit_acceptance_runs_branchy_inlined_callees() {
    let source = "\
fn clampish(x: Int, lo: Int, hi: Int) -> Int {
    if x < lo {
        return lo
    }
    if x > hi {
        return hi
    }
    return x
}

fn digits(value: Int) -> Int {
    let mut n = value
    if n < 0 {
        n = 0 - n
    }
    let mut count = 0
    while n > 0 {
        count = count + 1
        n = n / 10
    }
    return count
}

fn driver(limit: Int) -> Int {
    let mut total = 0
    let mut i = 0
    while i < limit {
        total = total + clampish(x: read i, lo: read 3, hi: read 17) + digits(value: read i)
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: driver(limit: read 250)))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-branchy-inline.rss", source);
}

#[test]
fn jit_acceptance_runs_collection_index_mutation_ops() {
    let source = "\
fn compute(xs: mut List<Int>, ys: read List<Int>, table: mut Map<Int, Int>) -> Int {
    List.set<Int>(list: mut xs, index: 2, value: read 100)
    List.append<Int>(list: mut xs, values: read ys)
    let mut total = 0
    let mut j = 0
    while j < List.len<Int>(list: read xs) {
        total = total + List.get<Int>(list: read xs, index: j)
        j = j + 1
    }
    match List.pop<Int>(list: mut xs) {
        Some(v) => {
            total = total + v
        }
        None => {
            total = total
        }
    }
    Map.insert<Int, Int>(map: mut table, key: read 9, value: read total)
    match Map.remove<Int, Int>(map: mut table, key: read 1) {
        Some(v) => {
            total = total + v
        }
        None => {
            total = total
        }
    }
    match Map.get<Int, Int>(map: read table, key: read 2) {
        Some(v) => {
            total = total + v
        }
        None => {
            total = total
        }
    }
    List.clear<Int>(list: mut xs)
    total = total + List.len<Int>(list: read xs)
    return total
}

fn main() -> Unit {
    let mut xs = List<Int>.new()
    let mut i = 0
    while i < 6 {
        let sq = i * i
        List.push<Int>(list: mut xs, value: read sq)
        i = i + 1
    }
    let mut ys = List<Int>.new()
    List.push<Int>(list: mut ys, value: read 7)
    let mut table = Map<Int, Int>.new()
    Map.insert<Int, Int>(map: mut table, key: read 1, value: read 50)
    Map.insert<Int, Int>(map: mut table, key: read 2, value: read 25)
    Log.write(message: read String.from_int(value: compute(xs: mut xs, ys: read ys, table: mut table)))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-collections.rss", source);
}

#[test]
fn jit_acceptance_runs_heap_read_helpers() {
    let source = "\
struct Vec2 {
    x: Int,
    y: Int
}

fn blend(p: read Vec2, xs: read List<Int>, n: Int) -> Int {
    let mut acc = 0
    let mut i = 0
    let len = List.len<Int>(list: read xs)
    while i < n {
        acc = acc + p.x * 2 + p.y
        if i < len {
            acc = acc + List.get<Int>(list: read xs, index: i)
        }
        acc = acc - i
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    let xs = [10, 20, 30, 40]
    let total = blend(p: read Vec2(x: 5, y: 9), xs: read xs, n: read 64)
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-heap-reads.rss", source);
}

#[test]
fn jit_acceptance_runs_float_heap_read_helpers() {
    let source = "\
struct Vec2f {
    x: Float,
    y: Float
}

fn blend(p: read Vec2f, xs: read List<Float>, n: Int) -> Float {
    let mut acc = 0.0
    let mut i = 0
    let len = List.len<Float>(list: read xs)
    while i < n {
        acc = acc + p.x * 2.0 + p.y
        if i < len {
            acc = acc + List.get<Float>(list: read xs, index: i)
        }
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    let xs = [1.5, 2.25, 3.75, 4.0]
    let total = blend(p: read Vec2f(x: 0.5, y: 9.0), xs: read xs, n: read 64)
    Log.write(message: read String.from_float(value: total))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-float-heap-reads.rss", source);
}

#[test]
fn jit_acceptance_runs_float_parameter_loop() {
    let source = "\
fn blend(x: Float, k: Float, n: Int) -> Float {
    let bias = 0.5
    let mut acc = x
    let mut i = 0
    while i < n {
        acc = acc * k + bias - x
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    Log.write(message: read String.from_float(value: blend(x: read 1.25, k: read 0.5, n: read 16)))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-float-params.rss", source);
}

#[test]
fn jit_acceptance_falls_back_for_recursive_calls() {
    let source = "\
fn fact(n: Int) -> Int {
    if n <= 1 {
        return 1
    }
    return n * fact(n: read n - 1)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: fact(n: read 8)))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-recursive-fallback.rss", source);
}

#[cfg(feature = "native-jit")]
#[test]
fn native_jit_acceptance_reports_real_native_execution() {
    let source = "\
fn hot(limit: Int) -> Int {
    let mut total = 0
    let mut i = 0
    while i < limit {
        total = total + i * 3 - 1
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: hot(limit: read 256)))
    return Unit
}
";
    let executable =
        rsscript::reg_vm_compile_source("jit-native-stats.rss", source).expect("source compiles");
    let (output, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native JIT run should succeed");

    assert_eq!(output.stdout.trim(), "97664");
    assert!(
        stats.translated > 0,
        "native tier should translate code: {stats:?}"
    );
    assert!(
        stats.compiled > 0,
        "native tier should compile code: {stats:?}"
    );
    assert!(
        stats.native_calls > 0,
        "native tier should execute compiled code: {stats:?}"
    );
    assert_eq!(
        stats.compile_failed, 0,
        "native tier should not fail compilation: {stats:?}"
    );
}

/// Regression: a native-eligible function with an *unused* (hence under-typed)
/// parameter must still reach the native tier. Before the `DeepCopy` lowering fix
/// the unused param pinned no native type, so the lowerer's `ty[reg]?` bailed and
/// the whole function silently fell back to the interpreter (`translated == 0`) — a
/// ~128x slowdown with no error. This locks that it translates and runs natively.
#[cfg(feature = "native-jit")]
#[test]
fn native_tier_accepts_unused_under_typed_parameter() {
    // `unused` is never read in the body, so it acquires no native type. It must be
    // passed *by value* (not `read`) so the lowerer emits a `DeepCopy` of it — that
    // `DeepCopy` of an untyped register is the exact shape the fix unblocks (a `read`
    // / borrowed param gets no copy and would not reproduce the bug).
    let source = "\
fn hot(limit: Int, unused: Int) -> Int {
    let mut total = 0
    let mut i = 0
    while i < limit {
        total = total + i
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: hot(limit: read 256, unused: 999)))
    return Unit
}
";
    let executable = rsscript::reg_vm_compile_source("jit-native-unused-param.rss", source)
        .expect("source compiles");
    let (output, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native JIT run should succeed");

    // sum of 0..255 == 32640
    assert_eq!(output.stdout.trim(), "32640");
    assert!(
        stats.translated > 0 && stats.native_calls > 0,
        "function with an unused/under-typed param must reach native, not fall back: {stats:?}"
    );
    assert_eq!(
        stats.compile_failed, 0,
        "native tier should not fail compilation: {stats:?}"
    );
}

/// J0.2 precise resume-at-safepoint deopt: when a native-eligible function bails
/// at a REAL guard mid-function, the precise path reconstructs the interpreter
/// register window from the J0.1b captured live values, sets the frame `ip` to
/// the safepoint's `resume_ip`, and resumes interpretation there — instead of
/// re-running the function from the top. The observable result must be identical
/// to the pure interpreter.
///
/// `accumulate` runs several arithmetic statements (`a`, `b`, `c` become live
/// registers), then its final `a * c` overflows i64 on the chosen inputs. Native
/// executes the prefix, captures `{a, b, c}`, and bails inside the `MulInt`
/// guard at a non-first safepoint. Precise resume restores those registers and
/// re-enters the interpreter AT the multiply, which re-traps exactly as the pure
/// interpreter does — so both backends surface the identical `main`-returned
/// error (overflow). This proves reconstruction + resume-ip placement are sound
/// (a wrong `resume_ip` or a missing/garbled register would diverge here).
#[cfg(feature = "native-jit")]
#[test]
fn native_precise_resume_at_real_guard_matches_interpreter() {
    // `a * c` overflows i64; the earlier statements compute live registers the
    // deopt must capture and restore.
    let source = "\
fn accumulate(seed: Int) -> Int {
    let a = seed + 7
    let b = a - 4
    let c = b + 1
    return a * c
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: accumulate(seed: read 3037000503)))
    return Unit
}
";
    let file = "jit-precise-resume-overflow.rss";
    let interp = common::run_vm_source(file, source, &[]);
    let precise = rsscript::reg_vm_eval_source_main_native_precise(
        file,
        source,
        std::iter::empty::<String>(),
    );

    // This program must actually exercise a bail: the multiply overflows, so the
    // interpreter run does not produce the normal product (otherwise the test
    // would pass vacuously). Overflow surfaces either as a hard `Err` or as a
    // `main`-returned `Err` variant depending on the program shape; either way it
    // is NOT a clean success printing the wrapped value.
    let interp_is_trap = match &interp {
        Err(_) => true,
        Ok(out) => matches!(
            &out.native_value,
            Some(rsscript::NativeValue::Variant { name, .. }) if name == "Err"
        ),
    };
    assert!(
        interp_is_trap,
        "precondition: the overflow program must trap on the interpreter; got {interp:?}",
    );
    // Precise resume must reproduce the identical outcome (same error). Compare
    // the normalized Debug form so any divergence (success-vs-error, or a
    // different error) fails loudly.
    assert_eq!(
        format!("{interp:?}"),
        format!("{precise:?}"),
        "precise resume-at-safepoint must match the pure interpreter on a real \
         mid-function guard bail",
    );
}

/// Companion success-path check: a native-eligible function whose guards never
/// fire runs natively to completion (no reconstruction), and a precise-mode run
/// must still produce the identical value as the interpreter. Locks that turning
/// the flag on never perturbs the clean-completion path.
#[cfg(feature = "native-jit")]
#[test]
fn native_precise_clean_completion_matches_interpreter() {
    let source = "\
fn hot(limit: Int) -> Int {
    let mut total = 0
    let mut i = 0
    while i < limit {
        total = total + i * 3 - 1
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: hot(limit: read 256)))
    return Unit
}
";
    let file = "jit-precise-clean.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let precise =
        rsscript::reg_vm_eval_source_main_native_precise(file, source, std::iter::empty::<String>())
            .expect("precise native run");
    assert_eq!(interp.stdout, precise.stdout);
    assert_eq!(precise.stdout.trim(), "97664");
}

/// J5.2 OSR (on-stack replacement) correctness: a native-subset hot scalar loop
/// wrapped by non-native I/O *in the same function* (a `Log.write` before and
/// after the loop), so the function as a whole is native-INELIGIBLE and only OSR
/// can run the loop natively. With OSR forced on, the program's output (the
/// pre-loop log line, the loop's computed total, the post-loop log line, and the
/// returned value) must be byte-identical to the pure interpreter. This proves the
/// OSR-entry loaded the live-in window correctly, the loop ran natively, and the
/// OSR-exit resumed the interpreter at the post-loop ip with the live-out window.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_scalar_loop_matches_interpreter() {
    let source = "\
fn compute(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        total = total + i * i
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: compute(limit: read 50)))
    return Unit
}
";
    let file = "jit-osr-scalar.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr = rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR loop must be byte-identical to the interpreter (stdout)"
    );
    // sum_{i=0}^{49} i*i = 40425; the wrapping I/O lines must also match exactly.
    assert_eq!(osr.stdout.trim_end(), "begin\n40425\n40425");
}

/// J5.2 OSR over a **read-heap** loop: the hot loop reads list elements
/// (`List.len`/`List.get`) — the read-only heap-helper subset — inside an
/// I/O-tangled, native-ineligible function. With OSR forced on, the result must be
/// byte-identical to the interpreter (exercises the host-helper read path under
/// OSR-entry/exit, including the handle-param window marshalling).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_read_heap_loop_matches_interpreter() {
    let source = "\
fn sum_list(values: read List<Int>) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let n = List.len(list: read values)
    while i < n {
        total = total + List.get(list: read values, index: read i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    let xs = [3, 1, 4, 1, 5, 9, 2, 6]
    Log.write(message: read String.from_int(value: sum_list(values: read xs)))
    return Unit
}
";
    let file = "jit-osr-readheap.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr = rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR read-heap loop must be byte-identical to the interpreter (stdout)"
    );
    // 3+1+4+1+5+9+2+6 = 31.
    assert_eq!(osr.stdout.trim_end(), "begin\n31\n31");
}

/// J2.1 profile-guided monomorphic closure inlining — the program shape the
/// optimization targets: a higher-order `dispatch(f, x)` whose closure parameter is
/// the same callee on every warm call, so J1 profiles the `CallClosure` site as
/// monomorphic and J2 inlines that callee behind a closure-identity guard. All
/// backends (interpreter / tier-0 / native / force-deopt) must agree — the
/// differential proves the inlined native path is byte-identical to the
/// interpreter.
#[test]
fn jit_acceptance_runs_monomorphic_closure_inline() {
    let source = "\
fn dispatch(f: read Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut total = 0
    let mut i = 0
    while i < 200 {
        let a: Fn(Int) -> Int = |x| { return x * 2 - 1 }
        total = total + dispatch(f: read a, x: read i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-mono-closure-inline.rss", source);
}

/// J2.1 GUARD-BAIL (forced polymorphic): warm `dispatch` to a monomorphic profile
/// on closure A (so its `CallClosure` site is inlined behind an identity guard and
/// the function tiers up to native), then drive the SAME site with a DIFFERENT
/// closure B. The identity guard detects `B != A`, BAILS to the interpreter (the
/// existing re-run-from-top fallback — sound because the inlined subset is
/// side-effect-free), and the interpreter handles B correctly. All backends must
/// agree, so the result equals the pure interpreter on both A and B.
#[test]
fn jit_acceptance_monomorphic_inline_guard_bails_on_different_closure() {
    let source = "\
fn dispatch(f: read Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut total = 0
    let mut i = 0
    // Warm phase: 150 calls, all closure A (x * 2). Far past PROFILE_WARMUP, so the
    // dispatch site profiles monomorphic on A and is inlined+compiled to native.
    while i < 150 {
        let a: Fn(Int) -> Int = |x| { return x * 2 }
        total = total + dispatch(f: read a, x: read i)
        i = i + 1
    }
    // Bail phase: a DIFFERENT closure B (x + 1000) through the now-native, A-inlined
    // site. The identity guard sees B != A and bails to the interpreter, which runs
    // B. If the guard were unsound, B would compute A's result (x * 2) and diverge.
    let mut j = 0
    while j < 40 {
        let b: Fn(Int) -> Int = |x| { return x + 1000 }
        total = total + dispatch(f: read b, x: read j)
        j = j + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-accept-mono-inline-guard-bail.rss";
    // Differential: native (with the A-inlined guard) must equal every other backend.
    assert_fast_jit_backends_agree(file, source);

    // And specifically confirm the native tier really executed inlined code on this
    // program (so the guard-bail path was genuinely exercised, not skipped), while
    // still matching the pure interpreter byte-for-byte.
    #[cfg(feature = "native-jit")]
    {
        let interp = common::run_vm_source(file, source, &[]).expect("interp run");
        let executable =
            rsscript::reg_vm_compile_source(file, source).expect("source compiles");
        let (native, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
            .expect("native JIT run should succeed");
        assert_eq!(
            native.stdout, interp.stdout,
            "native (A-inlined, guard-bailing on B) must equal the pure interpreter",
        );
        assert!(
            stats.translated > 0 && stats.native_calls > 0,
            "the monomorphic dispatch site must reach native (inline + guard): {stats:?}",
        );
        assert_eq!(
            stats.compile_failed, 0,
            "native compilation must not fail: {stats:?}",
        );
    }
}

/// J2.2 POLYMORPHIC inline cache (2 callees). A higher-order `dispatch(f, x)` site
/// warmed by alternating between TWO distinct non-capturing closures A/B, so J1
/// profiles the `CallClosure` site as Polymorphic with two keys and J2 emits a
/// 2-arm inline cache (read the closure id once, dispatch to the matching inlined
/// body, bail on no match). All backends must agree byte-for-byte.
#[test]
fn jit_acceptance_runs_polymorphic_closure_inline_two() {
    let source = "\
fn dispatch(f: read Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut total = 0
    let mut i = 0
    while i < 200 {
        if i % 2 == 0 {
            let a: Fn(Int) -> Int = |x| { return x * 2 - 1 }
            total = total + dispatch(f: read a, x: read i)
        } else {
            let b: Fn(Int) -> Int = |x| { return x + 7 }
            total = total + dispatch(f: read b, x: read i)
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-poly-closure-inline-2.rss", source);
}

/// J2.2 POLYMORPHIC inline cache (3 callees) + native-execution proof. A higher-
/// order `dispatch(f, x)` site warmed by ROTATING among THREE distinct
/// non-capturing closures A/B/C (`i % 3`), so J1 profiles the `CallClosure` site as
/// Polymorphic with three keys and J2 emits a 3-arm inline cache. The differential
/// (`assert_fast_jit_backends_agree`) proves EVERY inlined dispatch arm is correct:
/// the native run (which routes A/B/C through their three inlined arms) equals the
/// pure interpreter byte-for-byte, so each of the three arms computes the right
/// closure's result. Under `native-jit` we additionally assert the native tier
/// really ran (`translated > 0 && native_calls > 0 && compile_failed == 0`).
#[test]
fn jit_acceptance_runs_polymorphic_closure_inline_three() {
    let source = "\
fn dispatch(f: read Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut total = 0
    let mut i = 0
    while i < 300 {
        if i % 3 == 0 {
            let a: Fn(Int) -> Int = |x| { return x * 2 - 1 }
            total = total + dispatch(f: read a, x: read i)
        } else if i % 3 == 1 {
            let b: Fn(Int) -> Int = |x| { return x + 7 }
            total = total + dispatch(f: read b, x: read i)
        } else {
            let c: Fn(Int) -> Int = |x| { return 0 - x }
            total = total + dispatch(f: read c, x: read i)
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-accept-poly-closure-inline-3.rss";
    // Differential: native (3-arm inline cache) must equal every other backend, so
    // each of the three arms (A/B/C) is exercised and computes the correct result.
    assert_fast_jit_backends_agree(file, source);

    #[cfg(feature = "native-jit")]
    {
        let interp = common::run_vm_source(file, source, &[]).expect("interp run");
        let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
        let (native, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
            .expect("native JIT run should succeed");
        assert_eq!(
            native.stdout, interp.stdout,
            "native (3-arm poly inline cache) must equal the pure interpreter",
        );
        assert!(
            stats.translated > 0 && stats.native_calls > 0,
            "the polymorphic dispatch site must reach native (inline cache): {stats:?}",
        );
        assert_eq!(
            stats.compile_failed, 0,
            "native compilation must not fail: {stats:?}",
        );
    }
}

/// J3 SCALAR REPLACEMENT — the optimization's payoff shape: a hot loop that
/// constructs and matches a *non-escaping* `Option` with a scalar (`Int`) payload.
/// Escape analysis proves the Option never leaves the function, the rewrite pre-pass
/// dissolves `MakeSome`/`LoadNone`/`MatchOption`/`UnwrapSome` into tag + payload
/// scalar registers, and the function then compiles through the existing native
/// subset with NO heap allocation. The differential proves the scalar-replaced
/// native run is byte-identical to the interpreter, and under `native-jit` we assert
/// the loop genuinely reached native (`translated > 0 && native_calls > 0 &&
/// compile_failed == 0`).
#[test]
fn jit_acceptance_runs_scalar_replaced_option_loop() {
    // `hot` constructs a *non-escaping* `Option<Int>` each iteration (only ever
    // matched/unwrapped, never stored or returned). J3 escape analysis proves it
    // never escapes and the rewrite pre-pass dissolves the `MakeSome`/`LoadNone`/
    // `MatchOption`/`UnwrapSome` into tag + payload scalar registers, so `hot`
    // compiles through the native subset with no allocation.
    let source = "\
fn hot(limit: Int) -> Int {
    let mut acc = 0
    let mut i = 0
    while i < limit {
        let mut o: Option<Int> = None
        if i % 2 == 0 {
            o = Some(i * 2)
        }
        match o {
            Some(x) => {
                acc = acc + x
            }
            None => {
                acc = acc + 0
            }
        }
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: hot(limit: read 256)))
    return Unit
}
";
    let file = "jit-accept-scalar-replace-option.rss";
    // Differential: scalar-replaced native must equal every other backend byte-for-byte.
    assert_fast_jit_backends_agree(file, source);

    #[cfg(feature = "native-jit")]
    {
        let interp = common::run_vm_source(file, source, &[]).expect("interp run");
        let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
        let (native, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
            .expect("native JIT run should succeed");
        assert_eq!(
            native.stdout, interp.stdout,
            "scalar-replaced Option loop (native) must equal the pure interpreter",
        );
        assert!(
            stats.translated > 0 && stats.native_calls > 0,
            "the non-escaping Option loop must reach native after scalar replacement: {stats:?}",
        );
        assert_eq!(
            stats.compile_failed, 0,
            "native compilation must not fail: {stats:?}",
        );
    }
}

/// J3 NEGATIVE — an ESCAPING `Option` must NOT be scalar-replaced. Here the
/// constructed `Option` is pushed into a `List`, so it escapes the function; escape
/// analysis sees the non-recognized use and leaves the whole function on its
/// interpreter path (no scalar replacement, no native eligibility via this route).
/// The result must still be correct. The differential proves correctness across all
/// backends; this is purely a conservatism guard (an unsound transform of an
/// escaping Option would diverge here).
#[test]
fn jit_acceptance_does_not_scalar_replace_escaping_option() {
    let source = "\
fn pick(i: Int) -> Option<Int> {
    if i % 3 == 0 {
        return Some(i)
    }
    return None
}

fn build(limit: Int) -> Int {
    let mut xs = List<Option<Int>>.new()
    let mut i = 0
    while i < limit {
        let o = pick(i: read i)
        List.push<Option<Int>>(list: mut xs, value: read o)
        i = i + 1
    }
    let mut acc = 0
    let mut j = 0
    while j < List.len<Option<Int>>(list: read xs) {
        match List.get<Option<Int>>(list: read xs, index: j) {
            Some(x) => {
                acc = acc + x
            }
            None => {
                acc = acc + 0
            }
        }
        j = j + 1
    }
    return acc
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(limit: read 60)))
    return Unit
}
";
    // All backends must agree: the escaping Option is handled correctly on every
    // path (it is never scalar-replaced; `build` falls back to the interpreter for
    // the heap ops). If escape analysis wrongly scalar-replaced it, this diverges.
    assert_fast_jit_backends_agree("jit-accept-escaping-option.rss", source);
}

/// J2.2 POLYMORPHIC inline-cache MISS-BAIL. Warm `dispatch` to a Polymorphic
/// profile over THREE closures A/B/C (so its `CallClosure` site gets a 3-arm inline
/// cache and tiers up to native), then drive the SAME site with a FOURTH, never-
/// before-seen closure D. No arm matches D's id, so the cache bails via the
/// existing re-run-from-top fallback (sound: every inlined arm is side-effect-free)
/// and the interpreter handles D. The result still equals the pure interpreter, so
/// the miss-bail is correct. Under `native-jit` we also confirm the native tier ran
/// (proving the bail path was genuinely exercised, not skipped).
#[test]
fn jit_acceptance_polymorphic_inline_bails_on_fourth_closure() {
    let source = "\
fn dispatch(f: read Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut total = 0
    let mut i = 0
    // Warm phase: rotate A/B/C so the dispatch site profiles Polymorphic (3 keys),
    // gets a 3-arm inline cache, and tiers up to native.
    while i < 300 {
        if i % 3 == 0 {
            let a: Fn(Int) -> Int = |x| { return x * 2 - 1 }
            total = total + dispatch(f: read a, x: read i)
        } else if i % 3 == 1 {
            let b: Fn(Int) -> Int = |x| { return x + 7 }
            total = total + dispatch(f: read b, x: read i)
        } else {
            let c: Fn(Int) -> Int = |x| { return 0 - x }
            total = total + dispatch(f: read c, x: read i)
        }
        i = i + 1
    }
    // Miss phase: a FOURTH closure D (x * 100), never among A/B/C, through the now-
    // native, 3-arm-inlined site. No arm matches D's id; the cache bails to the
    // interpreter, which runs D. If the dispatch were unsound, D would take one of
    // A/B/C's arms and diverge.
    let mut j = 0
    while j < 40 {
        let d: Fn(Int) -> Int = |x| { return x * 100 }
        total = total + dispatch(f: read d, x: read j)
        j = j + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-accept-poly-inline-miss-bail.rss";
    assert_fast_jit_backends_agree(file, source);

    #[cfg(feature = "native-jit")]
    {
        let interp = common::run_vm_source(file, source, &[]).expect("interp run");
        let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
        let (native, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
            .expect("native JIT run should succeed");
        assert_eq!(
            native.stdout, interp.stdout,
            "native (3-arm poly cache, bailing on D) must equal the pure interpreter",
        );
        assert!(
            stats.translated > 0 && stats.native_calls > 0,
            "the polymorphic dispatch site must reach native (inline cache + bail): {stats:?}",
        );
        assert_eq!(
            stats.compile_failed, 0,
            "native compilation must not fail: {stats:?}",
        );
    }
}
