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

/// OSR × J3 (Pending #1) correctness: a hot loop that constructs and matches a
/// *non-escaping* scalar `Option<Int>` each iteration, wrapped by non-native I/O
/// (`Log.write` before/after) in the SAME function — so the function is
/// native-INELIGIBLE as a whole and only OSR can run the loop natively. The
/// Option is built and matched strictly inside the loop body and is dead at the
/// loop boundary, so OSR's J3 pre-pass scalar-replaces it (tag + payload scalar
/// registers) making the loop an allocation-free native loop, while the live-in /
/// live-out are the unchanged loop-carried registers (`i`, `total`).
///
/// With OSR forced on, the program's output must be byte-identical to the pure
/// interpreter — which interprets the whole loop, allocating an `Option` per
/// iteration. That byte-identity (and the differential corpus) is the correctness
/// net: a wrong ip-map or live-out restore would diverge here. Under `native-jit`
/// we also assert the loop genuinely OSR'd (`osr_entries > 0`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_option_loop_matches_interpreter() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let mut o: Option<Int> = None
        if i % 3 == 0 {
            o = Some(i * 2)
        }
        match o {
            Some(x) => {
                total = total + x
            }
            None => {
                total = total + 0
            }
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-option.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J3 Option loop must be byte-identical to the interpreter (stdout)"
    );
    // i in 0..60, i%3==0 ⇒ Some(2i): sum of 2i for i in {0,3,...,57} = 2*(0+3+...+57)
    // = 2 * (20 terms, sum = 570) = 1140.
    assert_eq!(osr.stdout.trim_end(), "begin\n1140\n1140");
    assert!(
        stats.osr_entries > 0,
        "the non-escaping Option loop must OSR natively after J3 scalar replacement: {stats:?}",
    );
}

/// OSR × J3 negative test — the safety invariant guard. The core soundness rule of
/// `native_scalar_replace_options_in_region` is that every scalar-replaced `Option`
/// must be DEAD outside `[header, exit)`. Here `o` is declared before the loop AND
/// read in a `match` AFTER it, so it is live across the loop boundary: the region
/// gate MUST refuse to scalar-replace it and therefore MUST NOT OSR (`osr_entries
/// == 0`), or the interpreter would read a stale `o` slot the native loop never
/// wrote back. We assert both that the program is still correct (the interpreter
/// runs the whole loop, no OSR) and that OSR genuinely bailed. This protects against
/// a future edit to `instr_read_regs`/`instr_written_reg` accidentally making a
/// boundary-escaping Option look dead.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_escaping_option_does_not_osr() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut o: Option<Int> = None
    while i < limit {
        o = Some(i)
        match o {
            Some(x) => {
                total = total + x
            }
            None => {
                total = total + 0
            }
        }
        i = i + 1
    }
    match o {
        Some(x) => {
            Log.write(message: read String.from_int(value: x))
        }
        None => {
            Log.write(message: read \"none\")
        }
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-escaping-option.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "an Option read after the loop must still produce interpreter-identical output"
    );
    // i in 0..60: total = 0+1+...+59 = 1770; o = Some(59) after the loop ⇒ "59".
    assert_eq!(osr.stdout.trim_end(), "begin\n59\n1770\n1770");
    assert_eq!(
        stats.osr_entries, 0,
        "a loop whose Option is live after the loop must NOT OSR (the dead-at-boundary \
         gate must bail): {stats:?}",
    );
}

/// OSR × J3 for VARIANTS (Pending #1) correctness: a hot loop that constructs and
/// matches a *non-escaping* single-scalar-payload user `sum`/variant (`Shape`) each
/// iteration, wrapped by non-native I/O (`Log.write` before/after) in the SAME
/// function — so the function is native-INELIGIBLE as a whole and only OSR can run
/// the loop natively. The `Shape` is built and matched strictly inside the loop body
/// and is dead at the loop boundary, so OSR's J3 pre-pass scalar-replaces it (a tag
/// register holding the arm index + one payload register) making the loop an
/// allocation-free native loop, while the live-in/live-out are the unchanged loop-
/// carried registers (`i`, `total`).
///
/// With OSR forced on, the program's output must be byte-identical to the pure
/// interpreter — which interprets the whole loop, allocating a `Shape` per iteration.
/// That byte-identity (and the differential corpus) is the correctness net. Under
/// `native-jit` we also assert the loop genuinely OSR'd (`osr_entries > 0`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_variant_loop_matches_interpreter() {
    let source = "\
sum Shape {
    Circle(radius: Int)
    Square(side: Int)
    Empty
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let mut s: Shape = Empty
        if i % 3 == 0 {
            s = Circle(radius: i)
        } else if i % 3 == 1 {
            s = Square(side: i * 2)
        }
        match s {
            Circle(r) => {
                total = total + read r
            }
            Square(w) => {
                total = total + read w
            }
            Empty => {
                total = total + 0
            }
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-variant.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J3 variant loop must be byte-identical to the interpreter (stdout)"
    );
    // i in 0..60: Circle(i) adds i (i%3==0), Square(2i) adds 2i (i%3==1), Empty
    // adds 0 (i%3==2). Total = 1750.
    assert_eq!(osr.stdout.trim_end(), "begin\n1750\n1750");
    assert!(
        stats.osr_entries > 0,
        "the non-escaping variant loop must OSR natively after J3 scalar replacement: {stats:?}",
    );
}

/// OSR × J3 variant negative test — the dead-at-boundary safety guard. The core
/// soundness rule of `native_scalar_replace_variants_in_region` is that every scalar-
/// replaced variant register must be DEAD outside `[header, exit)`. Here `s` is
/// declared before the loop AND read in a `match` AFTER it, so it is live across the
/// loop boundary: the region gate MUST refuse to scalar-replace it and therefore MUST
/// NOT OSR (`osr_entries == 0`), or the interpreter would read a stale `s` slot the
/// native loop never wrote back. We assert both that the program is still correct (the
/// interpreter runs the whole loop, no OSR) and that OSR genuinely bailed. This
/// protects against a future edit to `instr_read_regs`/`instr_written_reg`
/// accidentally making a boundary-escaping variant look dead.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_escaping_variant_does_not_osr() {
    let source = "\
sum Shape {
    Circle(radius: Int)
    Square(side: Int)
    Empty
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut s: Shape = Empty
    while i < limit {
        s = Circle(radius: i)
        match s {
            Circle(r) => {
                total = total + read r
            }
            Square(w) => {
                total = total + read w
            }
            Empty => {
                total = total + 0
            }
        }
        i = i + 1
    }
    match s {
        Circle(r) => {
            Log.write(message: read String.from_int(value: read r))
        }
        Square(w) => {
            Log.write(message: read String.from_int(value: read w))
        }
        Empty => {
            Log.write(message: read \"empty\")
        }
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-escaping-variant.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a variant read after the loop must still produce interpreter-identical output"
    );
    // i in 0..60: total = 0+1+...+59 = 1770; s = Circle(59) after the loop ⇒ "59".
    assert_eq!(osr.stdout.trim_end(), "begin\n59\n1770\n1770");
    assert_eq!(
        stats.osr_entries, 0,
        "a loop whose variant is live after the loop must NOT OSR (the dead-at-boundary \
         gate must bail): {stats:?}",
    );
}

/// OSR × J3 for MULTI-FIELD VARIANTS (Pending #1 broadening) correctness: a hot loop
/// that constructs and matches a *non-escaping* user `sum Shape` whose arms carry
/// SEVERAL scalar payload fields (`Rect(width, height)`, `Tri(a, b, c)`) each
/// iteration, wrapped by non-native I/O (`Log.write` before/after) in the SAME, once-
/// called function — so the function is native-INELIGIBLE as a whole and only OSR can
/// run the loop natively. The variant is built and field-read strictly inside the loop
/// body and is dead at the loop boundary, so OSR's J3 variant pass scalar-replaces it
/// (a tag register plus one fresh leaf register per `(arm, slot)` payload field, no
/// allocation), making the loop allocation-free native, while the live-in/live-out are
/// the unchanged loop-carried registers (`i`, `total`).
///
/// With OSR forced on, the program's output must be byte-identical to the pure
/// interpreter — which interprets the whole loop, allocating a `Shape` per iteration.
/// That byte-identity (and the differential corpus) is the correctness net. Under
/// `native-jit` we also assert the loop genuinely OSR'd (`osr_entries > 0`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_multifield_variant_loop_matches_interpreter() {
    let source = "\
sum Shape {
    Circle(radius: Int)
    Rect(width: Int, height: Int)
    Tri(a: Int, b: Int, c: Int)
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let mut s: Shape = Circle(radius: i)
        if i % 3 == 1 {
            s = Rect(width: i, height: i * 2)
        } else if i % 3 == 2 {
            s = Tri(a: i, b: i * 2, c: i * 3)
        }
        match read s {
            Circle(r) => {
                total = total + read r
            }
            Rect { width: w, height: h } => {
                total = total + read w + read h
            }
            Tri { a: a, b: b, c: c } => {
                total = total + read a + read b + read c
            }
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-multifield-variant.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J3 multi-field variant loop must be byte-identical to the interpreter (stdout)"
    );
    // i in 0..60: Circle(i) adds i (i%3==0), Rect(i,2i) adds 3i (i%3==1), Tri(i,2i,3i)
    // adds 6i (i%3==2). Total = 570 + 1770 + 3660 = 6000.
    assert_eq!(osr.stdout.trim_end(), "begin\n6000\n6000");
    assert!(
        stats.osr_entries > 0,
        "the non-escaping multi-field variant loop must OSR natively after J3 scalar \
         replacement: {stats:?}",
    );
}

/// OSR × J3 multi-field variant NEGATIVE test — the dead-at-boundary safety guard for
/// multi-field arms. A multi-field variant register that is live ACROSS the loop
/// boundary (read in a `match` AFTER the loop) MUST NOT be scalar-replaced: the region
/// gate must bail and the loop must NOT OSR (`osr_entries == 0`), or the interpreter
/// would read a stale `s` slot the native loop never wrote back. We assert both that
/// the program is still correct (the interpreter runs the whole loop, no OSR) and that
/// OSR genuinely bailed.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_escaping_multifield_variant_does_not_osr() {
    let source = "\
sum Shape {
    Circle(radius: Int)
    Rect(width: Int, height: Int)
    Tri(a: Int, b: Int, c: Int)
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut s: Shape = Circle(radius: 0)
    while i < limit {
        s = Rect(width: i, height: i * 2)
        match read s {
            Circle(r) => {
                total = total + read r
            }
            Rect { width: w, height: h } => {
                total = total + read w + read h
            }
            Tri { a: a, b: b, c: c } => {
                total = total + read a + read b + read c
            }
        }
        i = i + 1
    }
    match read s {
        Circle(r) => {
            Log.write(message: read String.from_int(value: read r))
        }
        Rect { width: w, height: h } => {
            Log.write(message: read String.from_int(value: read w + read h))
        }
        Tri { a: a, b: b, c: c } => {
            Log.write(message: read String.from_int(value: read a + read b + read c))
        }
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-escaping-multifield-variant.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a multi-field variant read after the loop must still produce interpreter-identical output"
    );
    // i in 0..60: each iter adds Rect(i,2i) = 3i, total = 3*(0+1+..+59) = 3*1770 = 5310.
    // s = Rect(59, 118) after the loop ⇒ 59 + 118 = 177.
    assert_eq!(osr.stdout.trim_end(), "begin\n177\n5310\n5310");
    assert_eq!(
        stats.osr_entries, 0,
        "a loop whose multi-field variant is live after the loop must NOT OSR (the \
         dead-at-boundary gate must bail): {stats:?}",
    );
}

/// OSR × J3 for STRUCTS (Pending #1) correctness: a hot loop that constructs and
/// field-reads a *non-escaping* FLAT user `struct Point { x: Int, y: Int }` (scalar
/// fields) each iteration, wrapped by non-native I/O (`Log.write` before/after) in the
/// SAME function — so the function is native-INELIGIBLE as a whole and only OSR can
/// run the loop natively. The `Point` is built and read strictly inside the loop body
/// and is dead at the loop boundary, so OSR's J3 struct pass scalar-replaces it (one
/// register per field slot, no tag, no allocation), making the loop an allocation-free
/// native loop, while the live-in/live-out are the unchanged loop-carried registers
/// (`i`, `total`).
///
/// With OSR forced on, the program's output must be byte-identical to the pure
/// interpreter — which interprets the whole loop, allocating a `Point` per iteration.
/// That byte-identity (and the differential corpus) is the correctness net. Under
/// `native-jit` we also assert the loop genuinely OSR'd (`osr_entries > 0`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_struct_loop_matches_interpreter() {
    let source = "\
struct Point {
    x: Int,
    y: Int
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let p = Point(x: i, y: i * 2)
        total = total + read p.x + read p.y
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-struct.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J3 struct loop must be byte-identical to the interpreter (stdout)"
    );
    // i in 0..60: each iter adds p.x + p.y = i + 2i = 3i. Total = 3 * (0+..+59) =
    // 3 * 1770 = 5310.
    assert_eq!(osr.stdout.trim_end(), "begin\n5310\n5310");
    assert!(
        stats.osr_entries > 0,
        "the non-escaping flat struct loop must OSR natively after J3 scalar replacement: {stats:?}",
    );
}

/// OSR × J3 struct negative test — the dead-at-boundary safety guard. The core
/// soundness rule of `native_scalar_replace_structs_in_region` is that every scalar-
/// replaced struct register must be DEAD outside `[header, exit)`. Here `p` is
/// declared before the loop AND read AFTER it, so it is live across the loop boundary:
/// the region gate MUST refuse to scalar-replace it and therefore MUST NOT OSR
/// (`osr_entries == 0`), or the interpreter would read a stale `p` slot the native loop
/// never wrote back. We assert both that the program is still correct (the interpreter
/// runs the whole loop, no OSR) and that OSR genuinely bailed. This protects against a
/// future edit to `instr_read_regs`/`instr_written_reg` accidentally making a boundary-
/// escaping struct look dead.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_escaping_struct_does_not_osr() {
    let source = "\
struct Point {
    x: Int,
    y: Int
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut p = Point(x: 0, y: 0)
    while i < limit {
        p = Point(x: i, y: i * 2)
        total = total + read p.x + read p.y
        i = i + 1
    }
    Log.write(message: read String.from_int(value: read p.x))
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-escaping-struct.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a struct read after the loop must still produce interpreter-identical output"
    );
    // i in 0..60: total = 3 * 1770 = 5310; p = Point(59, 118) after the loop ⇒ p.x = 59.
    assert_eq!(osr.stdout.trim_end(), "begin\n59\n5310\n5310");
    assert_eq!(
        stats.osr_entries, 0,
        "a loop whose struct is live after the loop must NOT OSR (the dead-at-boundary \
         gate must bail): {stats:?}",
    );
}

/// OSR × J3 NESTED-struct positive test (Pending #1 broadening). A two-level struct
/// `Outer { inner: Inner, tag }` is built and read through a chained field access
/// (`node.inner.value`, `node.inner.weight`) strictly inside the hot loop of an
/// I/O-tangled (native-INELIGIBLE) function. The whole nested struct is dead at the
/// loop boundary, so the recursive J3 struct pass dissolves it innermost-first: each
/// leaf scalar field becomes one register, the struct-typed `inner` slot aliases the
/// inner struct's registers, and the `a.b.c` chain collapses to register moves — the
/// loop runs allocation-free natively via OSR. With OSR forced on, output must be
/// byte-identical to the pure interpreter (which allocates the nested struct each
/// iteration), and the loop must genuinely OSR (`osr_entries > 0`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_nested_struct_loop_matches_interpreter() {
    let source = "\
struct Inner {
    value: Int,
    weight: Int
}

struct Outer {
    inner: Inner,
    tag: Int
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let node = Outer(inner: Inner(value: i, weight: i * 2), tag: i - 1)
        total = total + read node.inner.value + read node.inner.weight + read node.tag
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-nested-struct.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J3 nested struct loop must be byte-identical to the interpreter (stdout)"
    );
    // i in 0..60: each iter adds value + weight + tag = i + 2i + (i-1) = 4i - 1.
    // Total = sum_{i=0}^{59} (4i - 1) = 4*1770 - 60 = 7080 - 60 = 7020.
    assert_eq!(osr.stdout.trim_end(), "begin\n7020\n7020");
    assert!(
        stats.osr_entries > 0,
        "the non-escaping nested struct loop must OSR natively after recursive J3 \
         scalar replacement: {stats:?}",
    );
}

/// OSR × J3 NESTED-struct negative test — the dead-at-boundary guard, recursive case.
/// The outer struct `node` (and thus its inner struct) is declared before the loop and
/// read AFTER it, so it is live across the loop boundary. The recursive struct pass
/// MUST refuse to dissolve it and therefore MUST NOT OSR (`osr_entries == 0`), or the
/// interpreter would read a stale `node`/`node.inner` slot the native loop never wrote
/// back. We assert interpreter-identical output AND that OSR genuinely bailed.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_escaping_nested_struct_does_not_osr() {
    let source = "\
struct Inner {
    value: Int,
    weight: Int
}

struct Outer {
    inner: Inner,
    tag: Int
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut node = Outer(inner: Inner(value: 0, weight: 0), tag: 0)
    while i < limit {
        node = Outer(inner: Inner(value: i, weight: i * 2), tag: i - 1)
        total = total + read node.inner.value + read node.inner.weight + read node.tag
        i = i + 1
    }
    Log.write(message: read String.from_int(value: read node.inner.value))
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-escaping-nested-struct.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a nested struct read after the loop must still produce interpreter-identical output"
    );
    // total = sum_{i=0}^{59}(4i-1) = 7020; node.inner.value = 59 after the loop.
    assert_eq!(osr.stdout.trim_end(), "begin\n59\n7020\n7020");
    assert_eq!(
        stats.osr_entries, 0,
        "a loop whose nested struct is live after the loop must NOT OSR (the dead-at-\
         boundary gate must bail): {stats:?}",
    );
}

/// OSR × inline-leaf-calls (Pending #1) CROSS-FUNCTION positive test. The variant is
/// built in one leaf (`make_shape`) and matched in another (`area`), BOTH called from
/// the hot loop in an I/O-tangled (native-INELIGIBLE) function `f`. The variant thus
/// crosses function boundaries and never dissolves under plain OSR. The fix runs
/// `native_inline_leaf_calls` inside the OSR loop region FIRST, inlining the two
/// `CallKnown` leaves into the loop body so the variant becomes loop-LOCAL; the
/// already-shipped J3 variant scalar-replacement then dissolves it — an allocation-
/// free native loop. Forcing OSR on, the output must be byte-identical to the pure
/// interpreter (which allocates a `Shape` per iteration), and `osr_entries > 0`
/// proves the loop inlined + dissolved + OSR'd across the function boundary.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inline_cross_function_variant_loop() {
    let source = "\
sum Shape {
    Circle(radius: Int)
    Square(side: Int)
    Empty
}

fn make_shape(sel: Int, size: Int) -> Shape {
    if sel == 0 {
        return Circle(radius: size)
    }
    if sel == 1 {
        return Square(side: size)
    }
    return Empty
}

fn area(shape: read Shape) -> Int {
    match shape {
        Circle(radius) => {
            return radius * radius * 3
        }
        Square(side) => {
            return side * side
        }
        Empty => {
            return 0
        }
    }
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let shape = make_shape(sel: i % 3, size: i)
        total = total + area(shape: read shape)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-inline-cross-function-variant.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × inline cross-function variant loop must be byte-identical to the interpreter"
    );
    assert!(
        stats.osr_entries > 0,
        "a cross-function variant (built in make_shape, matched in area) must inline into \
         the loop, dissolve, and OSR natively: {stats:?}",
    );
}

/// OSR × inline-leaf-calls NEGATIVE test — a loop calling a NON-inlinable leaf must
/// NOT OSR. Here `bump` takes a `mut` List parameter and mutates it (`List.append`),
/// so it has `mut_args` and a non-native heap op: `native_callee_inlinable_j3` (and
/// the inline pass) refuses it. It cannot become loop-local, the loop never reaches
/// the native subset, and `osr_entries` MUST stay 0. The output must still be
/// interpreter-identical (the interpreter runs the whole loop). This guards against
/// the inline pass accidentally splicing a side-effecting / mut-arg leaf into the
/// loop body.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_inline_non_inlinable_leaf_does_not_osr() {
    let source = "\
fn bump(xs: mut List<Int>, v: Int) -> Int {
    List.append(list: mut xs, values: read [v])
    return v
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut xs: List<Int> = []
    while i < limit {
        total = total + bump(xs: mut xs, v: i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-inline-non-inlinable-leaf.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a loop calling a non-inlinable (mut-arg) leaf must still be interpreter-identical"
    );
    // i in 0..60: total = 0+1+...+59 = 1770.
    assert_eq!(osr.stdout.trim_end(), "begin\n1770\n1770");
    assert_eq!(
        stats.osr_entries, 0,
        "a loop calling a non-inlinable leaf must NOT OSR: {stats:?}",
    );
}

/// OSR × J2 POSITIVE test — a CAPTURING monomorphic param-handle closure called in a
/// hot loop inside an I/O-tangled (native-INELIGIBLE) once-called function MUST
/// inline (materializing its scalar capture) and OSR to an allocation-free native
/// loop. `apply_loop` takes `g: read Fn(Int) -> Int` and calls it every iteration;
/// `g = |value| value * 2 + base` captures the scalar `base`. The whole function is
/// native-ineligible (the `Log.write`s), so only OSR can run the loop natively — and
/// only by inlining the capturing closure (else the per-iteration `CallClosure` keeps
/// the loop off the native subset). Forcing OSR on, stdout MUST be byte-identical to
/// the pure interpreter AND `osr_entries > 0` proves the loop inlined the capturing
/// closure + materialized the capture + OSR'd.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j2_capturing_closure_loop_matches_interpreter() {
    let source = "\
fn apply_loop(g: read Fn(Int) -> Int, limit: Int, seed: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = seed
    while i < limit {
        total = total + g(i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    let base = 7
    let g = fn(value) captures(read base) effects(pure) {
        return value * 2 + base
    }
    Log.write(message: read String.from_int(value: apply_loop(g: read g, limit: read 4000, seed: read 0)))
    return Unit
}
";
    let file = "jit-osr-j2-capturing-closure.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J2 capturing-closure loop must be byte-identical to the interpreter"
    );
    assert!(
        stats.osr_entries > 0,
        "a capturing monomorphic param-handle closure called in an I/O-tangled hot loop \
         must inline (materializing its scalar capture) and OSR natively: {stats:?}",
    );
}

/// OSR × J2 POSITIVE test (Pending #2 Float-capture broadening) — a CAPTURING
/// monomorphic param-handle closure whose capture is a FLOAT, called in a hot loop
/// inside an I/O-tangled (native-INELIGIBLE) function, MUST inline (bit-reinterpreting
/// the f64 capture from its i64 slot) and OSR to an allocation-free native loop.
/// `apply_loop` takes `g: read Fn(Float) -> Float` and calls it every iteration;
/// `g = |value| value * scale + scale` captures the Float `scale = 1.5`. The capture
/// is materialized via the `closure_capture` helper, which returns `f64::to_bits` as
/// i64; the inlined body's Float-class capture register bit-reinterprets that i64 to
/// f64 (NOT an integer→float conversion). Forcing OSR on, stdout (a Float, formatted
/// via `String.from_float`) MUST be byte-identical to the pure interpreter AND
/// `osr_entries > 0` proves the loop inlined the capturing closure + materialized the
/// FLOAT capture + OSR'd. If the bits were int-converted, the Float would be wrong and
/// stdout would diverge.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j2_float_capture_closure_loop_matches_interpreter() {
    let source = "\
fn apply_loop(g: read Fn(Float) -> Float, limit: Int, seed: Float) -> Float {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = seed
    let mut x = 0.0
    while i < limit {
        total = total + g(read x)
        x = x + 1.0
        i = i + 1
    }
    Log.write(message: read String.from_float(value: total))
    return total
}

fn main() -> Unit {
    let scale = 1.5
    let g = fn(value) captures(read scale) effects(pure) {
        return value * scale + scale
    }
    Log.write(message: read String.from_float(value: apply_loop(g: read g, limit: read 4000, seed: read 0.0)))
    return Unit
}
";
    let file = "jit-osr-j2-float-capture-closure.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J2 Float-capture closure loop must be byte-identical to the interpreter \
         (the f64 capture must be bit-reinterpreted, not int-converted)"
    );
    assert!(
        stats.osr_entries > 0,
        "a capturing monomorphic param-handle closure with a FLOAT capture called in an \
         I/O-tangled hot loop must inline (bit-reinterpreting the f64 capture) and OSR \
         natively: {stats:?}",
    );
}

/// OSR × J2 NEGATIVE test (Pending #2) — a closure capturing a NON-scalar value that
/// HAPPENS TO BE float-typed (`scales: List<Float>`, a heap value) MUST NOT OSR. The
/// Float broadening admits only FLAT scalar captures (Int/Bool/Float); a heap capture
/// — even one whose elements are Floats — cannot be materialized as a scalar via the
/// `closure_capture` helper, so the inline gate's `captures_all_scalar` profile bit
/// goes false on the first observation, the site never inlines, the per-iteration
/// `CallClosure` keeps the loop off the native subset, and `osr_entries` MUST stay 0.
/// Output must still be interpreter-identical. This guards the Float broadening from
/// over-reaching: widening flat-scalar captures to include `Float` must NOT start
/// admitting a heap aggregate just because it carries Floats (which would read garbage
/// bits from a heap handle's slot).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j2_float_heap_capture_closure_does_not_osr() {
    let source = "\
fn apply_loop(g: read Fn(Int) -> Float, limit: Int, seed: Float) -> Float {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = seed
    while i < limit {
        total = total + g(i)
        i = i + 1
    }
    Log.write(message: read String.from_float(value: total))
    return total
}

fn main() -> Unit {
    let scales: List<Float> = [1.5, 2.25, 3.75]
    let g = fn(value) captures(read scales) effects(pure) {
        let n = List.len<Float>(list: read scales)
        return List.get<Float>(list: read scales, index: value - (value / n) * n)
    }
    Log.write(message: read String.from_float(value: apply_loop(g: read g, limit: read 4000, seed: read 0.0)))
    return Unit
}
";
    let file = "jit-osr-j2-float-heap-capture-closure.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a closure capturing a List<Float> (heap) must still be interpreter-identical under OSR"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a closure capturing a heap value (List<Float>) must NOT OSR even though its \
         elements are Floats — only FLAT scalar captures inline: {stats:?}",
    );
}

/// OSR × J2 NEGATIVE test — a closure with a NON-scalar (heap) capture MUST NOT OSR.
/// `g` captures `tag: List<Int>` (a heap value) and reads its length each call, so
/// the capture cannot be materialized as a scalar via the `closure_capture` helper:
/// the inline gate's `captures_all_scalar` profile bit goes false on the first
/// observation, the site never inlines, the per-iteration `CallClosure` keeps the
/// loop off the native subset, and `osr_entries` MUST stay 0. Output must still be
/// interpreter-identical (the interpreter runs the whole loop). This guards against
/// the capturing-closure inline accidentally materializing a heap capture as a
/// scalar (which would read garbage bits).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j2_heap_capture_closure_does_not_osr() {
    let source = "\
fn apply_loop(g: read Fn(Int) -> Int, limit: Int, seed: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = seed
    while i < limit {
        total = total + g(i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    let tag: List<Int> = [1, 2, 3]
    let g = fn(value) captures(read tag) effects(pure) {
        return value + List.len<Int>(list: read tag)
    }
    Log.write(message: read String.from_int(value: apply_loop(g: read g, limit: read 4000, seed: read 0)))
    return Unit
}
";
    let file = "jit-osr-j2-heap-capture-closure.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a closure with a heap capture must still be interpreter-identical under OSR"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a closure with a non-scalar (heap) capture must NOT OSR: {stats:?}",
    );
}

/// OSR × J2 POSITIVE test (Pending #1 stored-closure broadening) — a STORED,
/// POLYMORPHIC, CAPTURING closure called in a hot loop (the `dynamic_closure_call`
/// kernel shape). Each iteration fetches a struct from a `List<Op>` (`op =
/// List.get(ops, sel)`), reads its `apply` closure field (`f = op.apply`), and calls
/// it (`f(index)`). The two stored closures both capture the scalar `base` and
/// differ by `sel` (polymorphic, 2 targets). The whole `main` is native-INELIGIBLE
/// (the `Log.write`s + list construction), so only OSR can run the loop natively —
/// and only by (1) reading the stored closure handle via `ListGetHandle`/
/// `FieldHandle`, (2) dispatching the polymorphic inline cache, and (3) materializing
/// each arm's scalar capture. Forcing OSR on, stdout MUST be byte-identical to the
/// pure interpreter AND `osr_entries > 0` proves the stored/poly/capturing loop OSR'd.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_stored_polymorphic_capturing_closure_loop_matches_interpreter() {
    let source = "\
features: local

struct Op derives(Clone) {
    apply: owned Fn(Int) -> Int
}

fn lim() -> Int {
    return 6000
}

fn main() -> Unit {
    let base = 7
    local ops = List.new<Op>()
    let double_plus = Op(apply: fn(value) captures(read base) effects(pure) {
        return value * 2 + base
    })
    List.push(list: mut ops, value: read double_plus)
    let shift = Op(apply: fn(value) captures(read base) effects(pure) {
        return value + base * 3
    })
    List.push(list: mut ops, value: read shift)

    let limit = lim()
    let mut index = 0
    let mut sel = 0
    let mut total = 0
    while index < limit {
        let op = List.get(list: read ops, index: sel)
        let f = op.apply
        total = total + f(index)
        index = index + 1
        sel = sel + 1
        if sel == 2 {
            sel = 0
        }
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-osr-stored-poly-capturing-closure.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J2 stored/poly/capturing closure loop must be byte-identical to the interpreter"
    );
    assert!(
        stats.osr_entries > 0,
        "a stored, polymorphic, capturing closure called in an I/O-tangled hot loop must \
         read the handle (FieldHandle/ListGetHandle), poly-dispatch, materialize the scalar \
         capture, and OSR natively: {stats:?}",
    );
}

/// OSR × J2 AUTO test (Pending #2 + #1) — the stored/poly/capturing closure loop must
/// auto-OSR by DEFAULT (no `RSS_JIT_OSR`, no override), proving the profile-guided
/// auto-trigger RETRIES past the pending-profile window rather than permanently giving
/// up. The closure-inline gate is profile-guided, so at the first backedge-threshold
/// crossing the site may still be pending; the auto-trigger must reset+re-probe (not
/// `GaveUp`) so OSR fires once the profile freezes. A forced-OSR test cannot catch this
/// (it bypasses the auto `GaveUp` path); this uses `eval_main_with_args_native_with_stats`.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_stored_closure_auto_triggers_without_flag() {
    let source = "\
features: local

struct Op derives(Clone) {
    apply: owned Fn(Int) -> Int
}

fn lim() -> Int {
    return 8000
}

fn main() -> Unit {
    let base = 7
    local ops = List.new<Op>()
    let double_plus = Op(apply: fn(value) captures(read base) effects(pure) {
        return value * 2 + base
    })
    List.push(list: mut ops, value: read double_plus)
    let shift = Op(apply: fn(value) captures(read base) effects(pure) {
        return value + base * 3
    })
    List.push(list: mut ops, value: read shift)

    let limit = lim()
    let mut index = 0
    let mut sel = 0
    let mut total = 0
    while index < limit {
        // The closure call is CONDITIONAL (~1/4 of iterations) so its inline profile
        // warms SLOWER than the backedge counter: at the OSR backedge threshold (1000)
        // the closure has only been called ~250 times (< PROFILE_RECORD_LIMIT 306), so
        // the inline site is still PENDING. The auto-trigger must reset+re-probe rather
        // than GaveUp — exactly the bug this test guards.
        if index % 4 == 0 {
            let op = List.get(list: read ops, index: sel)
            let f = op.apply
            total = total + f(index)
            sel = sel + 1
            if sel == 2 {
                sel = 0
            }
        }
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-osr-stored-closure-auto.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    // NO env flag, NO override: the default native path. Auto-OSR must fire on its own
    // once the closure-inline profile warms — the GaveUp-while-pending bug would
    // permanently disable OSR here (osr_entries would stay 0).
    let (auto, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native auto-OSR run should succeed");
    assert_eq!(
        auto.stdout, interp.stdout,
        "auto-OSR stored/poly/capturing closure loop must be byte-identical to the interpreter",
    );
    assert!(
        stats.osr_entries > 0,
        "the stored/poly/capturing closure loop must AUTO-OSR by default once its profile \
         warms (the auto-trigger must not GaveUp while the inline profile is still pending): \
         {stats:?}",
    );
}

/// OSR × J2 NEGATIVE test (Pending #1) — a stored closure whose callee is NOT
/// native-inlinable MUST NOT OSR, no matter how the site profiles. The stored
/// closure's body calls `Log.write` (an I/O op outside the native subset), so
/// `native_callee_inlinable` (and the capturing variant) rejects it for EVERY
/// profile state — monomorphic, polymorphic, or megamorphic. The per-iteration
/// `CallClosure` therefore keeps the loop off the native subset and `osr_entries`
/// MUST stay 0 deterministically. Output must still be interpreter-identical.
/// (The megamorphic-by-distinct-closures case is exercised structurally by the
/// `polymorphic_closure_inline_targets` 2..=3 cap; this test pins the orthogonal
/// "the speculated callee itself can't be inlined" guard, which never transiently
/// fires the way an incrementally-warming distinct-closure count would.)
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_stored_non_inlinable_closure_does_not_osr() {
    let source = "\
features: local

struct Op derives(Clone) {
    apply: owned Fn(Int) -> Int
}

fn lim() -> Int {
    return 6000
}

fn main() -> Unit {
    let base = 7
    local ops = List.new<Op>()
    List.push(list: mut ops, value: read Op(apply: fn(v) captures(read base) effects(write) {
        Log.write(message: read \"tick\")
        return v * 2 + base
    }))

    let limit = lim()
    let mut index = 0
    let mut total = 0
    while index < limit {
        let op = List.get(list: read ops, index: 0)
        let f = op.apply
        total = total + f(index)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-osr-stored-non-inlinable-closure.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a stored non-inlinable closure loop must still be interpreter-identical under OSR"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a stored closure whose callee is not native-inlinable must NOT OSR: {stats:?}",
    );
}

/// OSR × J2 NEGATIVE test (Pending #1) — a STORED closure with a NON-scalar (heap)
/// capture MUST NOT OSR. The stored closure captures `tag: List<Int>` (a heap value)
/// and reads its length each call, so the capture cannot be materialized as a scalar
/// via the `closure_capture` helper: the inline gate's `captures_all_scalar` profile
/// bit goes false on the first observation, the site never inlines, the per-iteration
/// `CallClosure` keeps the loop off the native subset, and `osr_entries` MUST stay 0.
/// Output must still be interpreter-identical.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_stored_heap_capture_closure_does_not_osr() {
    let source = "\
features: local

struct Op derives(Clone) {
    apply: owned Fn(Int) -> Int
}

fn lim() -> Int {
    return 6000
}

fn main() -> Unit {
    let tag: List<Int> = [1, 2, 3]
    local ops = List.new<Op>()
    List.push(list: mut ops, value: read Op(apply: fn(v) captures(read tag) effects(pure) { return v + List.len<Int>(list: read tag) }))

    let limit = lim()
    let mut index = 0
    let mut total = 0
    while index < limit {
        let op = List.get(list: read ops, index: 0)
        let f = op.apply
        total = total + f(index)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-osr-stored-heap-capture-closure.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a stored closure with a heap capture must still be interpreter-identical under OSR"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a stored closure with a non-scalar (heap) capture must NOT OSR: {stats:?}",
    );
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

/// Pending #2 OSR hot-backedge AUTO-trigger: a hot native-subset scalar loop
/// wrapped by non-native I/O *in the same (once-called) function* — so the
/// function is native-INELIGIBLE as a whole and only OSR can run the loop
/// natively — must auto-fire with NO `RSS_JIT_OSR` env flag and NO test override
/// (the plain `eval_main_with_args_native_with_stats` path). The loop runs far
/// more than the OSR backedge threshold iterations, so the backedge counter
/// crosses the threshold and `try_osr` fires on its own. We assert (a) the output
/// is byte-identical to the pure interpreter and (b) `osr_entries > 0` — i.e. the
/// auto-trigger genuinely handed the loop to native code without any flag.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_auto_triggers_on_hot_loop_without_flag() {
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
    Log.write(message: read String.from_int(value: compute(limit: read 5000)))
    return Unit
}
";
    let file = "jit-osr-auto.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    // NO env flag, NO override: the default native path. Auto-trigger must fire on
    // the hot loop entirely on its own.
    let (auto, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native auto-OSR run should succeed");
    assert_eq!(
        auto.stdout, interp.stdout,
        "auto-OSR loop must be byte-identical to the interpreter (stdout)",
    );
    assert!(
        stats.osr_entries > 0,
        "a hot (> threshold) native-subset loop must AUTO-OSR with no env flag and \
         no override: {stats:?}",
    );
}

/// Pending #2 OSR auto-trigger gating: a loop that runs FEWER than the OSR backedge
/// threshold iterations must NOT auto-fire (the backedge counter never crosses the
/// threshold) — so the OSR compile/setup cost is never paid for a short loop — and
/// the result must still be correct. Run with NO env flag and NO override; assert
/// `osr_entries == 0` and output == interpreter.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_short_loop_does_not_auto_trigger() {
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
    let file = "jit-osr-short.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (auto, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run should succeed");
    assert_eq!(
        auto.stdout, interp.stdout,
        "short-loop result must still be byte-identical to the interpreter",
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a loop shorter than the OSR backedge threshold must NOT auto-fire (no OSR \
         setup paid for tiny loops): {stats:?}",
    );
    // sum_{i=0}^{49} i*i = 40425, matching native_osr_scalar_loop_matches_interpreter.
    assert_eq!(auto.stdout.trim_end(), "begin\n40425\n40425");
}

/// No-amortization profitability gate (native-tier bail-overhead fix). A loop-free
/// body (here a `|x| x * 2 + 1` closure) allocated and dispatched once per loop
/// iteration does O(1) work per native call, so the per-dispatch FFI + marshalling
/// cost can never be amortized — dispatching it natively every iteration is a net
/// loss versus the interpreter. The gate counts native dispatches of back-edge-free
/// bodies and, after `NATIVE_NOAMORTIZE_GIVEUP` (64) of them, demotes the function
/// to NOT_ELIGIBLE so the rest of the loop runs on the interpreter. We assert (a)
/// the body WAS dispatched natively at least once (the gate is on the accept path,
/// not a blanket reject — `native_calls > 0`) and (b) it was demoted long before the
/// loop ended (`native_calls` is bounded far below the 2000 iterations — NOT one
/// dispatch per iteration), and (c) the result is byte-identical to the interpreter.
/// The complementary "loop-bearing bodies are NEVER demoted" half is covered by the
/// whole-loop native wins (e.g. `native_scalar_loop`, every `osr_*` kernel) whose
/// `native_calls == 1`: a loop body compiles as one native call and is exempt.
#[cfg(feature = "native-jit")]
#[test]
fn native_loop_free_per_iteration_dispatch_is_demoted_by_profitability_gate() {
    // `main`'s hot loop calls the loop-free leaf `helper` (`x * 2 + 1`) once per
    // iteration AND has an early `return` inside the body. The early return makes the
    // loop genuinely MULTI-EXIT, so `detect_single_natural_loop` rejects it and the
    // loop can NEVER OSR (no whole-loop native body, no closure-sink) — yet the
    // loop-free leaf is still dispatched natively per iteration, so this is the pure
    // per-iteration-dispatch scenario the no-amortization gate exists to demote.
    //
    // (This shape replaced the previous in-loop `local f = |x| {...}` closure: that
    // captureless non-escaping per-iteration closure is now SUNK + OSR'd by the J3
    // closure-allocation-sinking pass, so it no longer reaches the per-iteration
    // dispatch path. A multi-exit loop calling a non-sinkable leaf keeps a real gate
    // test alive — the leaf can't be sunk because the loop can't OSR at all.)
    //
    // The early return is never taken (`total` never exceeds the guard), so
    // sum_{i=0}^{1999} (2i + 1) = 2000^2 = 4000000.
    let source = "\
features: local

fn helper(x: Int) -> Int {
    return x * 2 + 1
}

fn main() -> Unit {
    let limit = 2000
    let mut i = 0
    let mut total = 0
    while i < limit {
        if total > 1000000000 {
            Log.write(message: read \"overflow\")
            return Unit
        }
        total = total + helper(x: i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-noamortize-gate.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (native, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run should succeed");
    assert_eq!(
        native.stdout, interp.stdout,
        "demoted-leaf result must be byte-identical to the interpreter",
    );
    assert_eq!(native.stdout.trim_end(), "4000000");
    assert_eq!(
        stats.osr_entries, 0,
        "this is a per-iteration native-dispatch scenario, NOT an OSR one (the multi-exit \
         loop — an early in-body return — makes the loop OSR-ineligible): {stats:?}",
    );
    assert!(
        stats.native_calls > 0,
        "the loop-free closure body must be dispatched natively at least once before \
         the gate demotes it (the gate is on the accept path): {stats:?}",
    );
    assert!(
        stats.native_calls < 500,
        "the no-amortization gate must demote the loop-free per-iteration body long \
         before 2000 iterations — native_calls must be bounded, NOT one-per-iteration: \
         {stats:?}",
    );
}

/// OSR × closure-allocation sinking — POSITIVE test. A hot loop allocates a fresh
/// NON-escaping closure (`local f = |x| { x*2+1 }`, a per-iteration `MakeClosure`)
/// and calls it once per iteration, with the loop I/O-tangled (a `Log.write` before
/// and after it in the same once-called `main`, so the whole function is native-
/// INELIGIBLE — only OSR can run the loop). The closure's callee is known STATICALLY
/// from the `MakeClosure`, is non-escaping, and is captureless, so the sinking pass
/// dissolves the alloc and inlines `x*2+1`: the loop becomes pure-scalar and OSRs.
/// Output must be byte-identical to the pure interpreter and `osr_entries > 0`.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_sinks_non_escaping_per_iteration_closure_alloc() {
    let source = "\
features: local

fn main() -> Unit {
    Log.write(message: read \"begin\")
    let limit = 2000
    let mut i = 0
    let mut total = 0
    while i < limit {
        local f = |x| {
            return x * 2 + 1
        }
        total = total + f(i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-osr-closure-sink-positive.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR with a sunk per-iteration closure alloc must be byte-identical to the interpreter",
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n4000000");
    assert!(
        stats.osr_entries > 0,
        "the non-escaping per-iteration closure alloc must be sunk + inlined so the loop \
         OSRs natively: {stats:?}",
    );
}

/// OSR × closure-allocation sinking — NEGATIVE test. A per-iteration closure that is
/// NOT sinkable must NOT OSR. Here the per-iteration `local f = |x| { ... }` CAPTURES
/// a heap value (a `List`, used inside the body via `List.len`), so the callee body
/// is not native-inlinable AND the capture is non-scalar: the sinking analysis
/// rejects it (it never enters `sink_calls`), the `MakeClosure` stays in the loop,
/// and the loop is not native-subset — so it cannot OSR. Output must still be
/// byte-identical to the interpreter (the interpreter runs the whole loop) and
/// `osr_entries == 0`. This is the conservative bail: a per-iteration closure whose
/// value/captures escape the scalar subset is left on its normal heap-allocating
/// path, behavior unchanged.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_does_not_sink_non_inlinable_heap_capturing_per_iteration_closure() {
    let source = "\
features: local

fn main() -> Unit {
    Log.write(message: read \"begin\")
    let limit = 2000
    let mut i = 0
    let mut total = 0
    let data = List.new<Int>()
    while i < limit {
        local f = |x| {
            return x * 2 + 1 + List.len(list: read data)
        }
        total = total + f(i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-osr-closure-sink-negative.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    // Force OSR ON deterministically: even with the OSR hook armed, the non-sinkable
    // closure must keep the loop off the native subset, so no OSR can fire.
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a non-sinkable (heap-capturing) per-iteration closure must run interpreter-identical",
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a per-iteration closure whose body is non-inlinable / capture is non-scalar must \
         NOT be sunk and the loop must NOT OSR: {stats:?}",
    );
}

/// Broadened OSR loop detector — POSITIVE test (the real `variant_match_loop` kernel
/// shape). The hot loop contains an INTERNAL forward `if sel == N { sel = 0 }` reset
/// (a within-body branch + join), and the function calls a NON-inlinable helper
/// (`bench_size`, which does I/O) OUTSIDE the loop before it. The detector now
/// accepts a reducible single-header / single-exit loop with internal forward
/// control flow, and the region-scoped inline pass copies the out-of-region
/// `bench_size` through (its inlinability no longer vetoes OSR for the hot loop).
/// The in-loop variant (`make_shape`/`area`) inlines + dissolves via the shipped J3
/// passes, so the loop OSRs natively. Output must be byte-identical to the pure
/// interpreter and `osr_entries > 0`.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_internal_if_reset_loop_matches_interpreter() {
    let source = "\
sum Shape {
    Circle(radius: Int)
    Square(side: Int)
    Empty
}

fn bench_size(default: Int) -> Int {
    Log.write(message: read \"begin\")
    return default
}

fn make_shape(sel: Int, size: Int) -> Shape {
    if sel == 0 { return Circle(radius: size) }
    if sel == 1 { return Square(side: size) }
    return Empty
}

fn area(shape: read Shape) -> Int {
    match shape {
        Circle(radius) => { return radius * radius * 3 }
        Square(side) => { return side * side }
        Empty => { return 0 }
    }
}

fn main() -> Unit {
    let limit = bench_size(default: read 2000)
    let mut index = 0
    let mut sel = 0
    let mut total = 0
    while index < limit {
        let shape = make_shape(sel: read sel, size: read index)
        total = total + area(shape: read shape)
        index = index + 1
        sel = sel + 1
        if sel == 3 {
            sel = 0
        }
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-osr-internal-if-reset.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR loop with an internal `if sel==N` reset must be byte-identical to the interpreter"
    );
    assert!(
        stats.osr_entries > 0,
        "a reducible single-exit loop with an internal-if reset (the variant_match_loop \
         shape) must OSR after region-scoped inline + variant dissolve: {stats:?}",
    );
}

/// Broadened OSR loop detector — NEGATIVE test. A genuinely MULTI-EXIT loop (an
/// early `return` inside the loop body, in addition to the header's loop-condition
/// exit) is NOT a single-exit natural loop. The detector MUST reject it (`Return`
/// in-body is an extra value-producing exit), so `osr_entries == 0`. Output must
/// still be interpreter-identical (the interpreter runs the whole loop).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_multi_exit_early_return_loop_rejected() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        total = total + i * i
        if total > 1000 {
            return total
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 200)))
    return Unit
}
";
    let file = "jit-osr-multi-exit-return.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a multi-exit (early-return) loop must still run interpreter-identically"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a loop with an in-body early `return` is multi-exit and MUST NOT OSR: {stats:?}",
    );
}

/// Adaptive-tiering knob: `RSS_JIT_OSR_THRESHOLD` overrides the OSR auto-trigger
/// (counting `jit-native`) backedge threshold for sweeps WITHOUT recompiling per
/// value, mirroring `RSS_JIT_OSR`. Default behavior is unchanged (unset ⇒ 1000).
/// This test drives the real `rss bench --mode jit-native --json` binary as a
/// subprocess (so env is set safely via `Command::env`, respecting the crate's
/// `#![forbid(unsafe_code)]`) on a fixed medium-loop kernel whose trip count
/// (600) sits BELOW the default 1000: at the default threshold the counting
/// auto-trigger must NOT fire (`osr_entries == 0`), but with the override lowered
/// to 100 the SAME loop must OSR (`osr_entries > 0`). Deterministic: trip count,
/// kernel, and thresholds are all fixed.
#[cfg(feature = "native-jit")]
#[test]
fn rss_jit_osr_threshold_env_overrides_auto_trigger_fire_point() {
    use std::process::Command;

    let bin = env!("CARGO_BIN_EXE_rss");
    let kernel = std::env::temp_dir().join(format!(
        "rss_osr_threshold_probe_{}_{}.rss",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    ));
    // Native-INELIGIBLE function (Log.write tangled around it) with one hot scalar
    // loop of 600 iterations: only OSR can run the loop natively.
    std::fs::write(
        &kernel,
        "\
fn hot(limit: Int, seed: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = seed
    while i < limit {
        total = total + i * 3 - i / 2 + 7
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: hot(limit: read 600, seed: read 0)))
    return Unit
}
",
    )
    .expect("write kernel");

    let osr_entries = |threshold: Option<&str>| -> u64 {
        let mut cmd = Command::new(bin);
        cmd.args([
            "bench",
            "--json",
            "--mode",
            "jit-native",
            "--iterations",
            "1",
            "--warmup",
            "0",
        ])
        .arg(&kernel);
        cmd.env_remove("RSS_JIT_OSR");
        match threshold {
            Some(t) => {
                cmd.env("RSS_JIT_OSR_THRESHOLD", t);
            }
            None => {
                cmd.env_remove("RSS_JIT_OSR_THRESHOLD");
            }
        }
        let out = cmd.output().expect("run rss bench");
        assert!(
            out.status.success(),
            "rss bench failed: {}",
            String::from_utf8_lossy(&out.stderr),
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        // Pull "osr_entries":<n> out of the JSON line (no serde dep in this test).
        let key = "\"osr_entries\":";
        let pos = stdout
            .find(key)
            .unwrap_or_else(|| panic!("no osr_entries in bench json: {stdout}"));
        let rest = &stdout[pos + key.len()..];
        let end = rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len());
        rest[..end].parse::<u64>().expect("parse osr_entries")
    };

    // Default threshold (1000): trip=600 < 1000 ⇒ counting auto-trigger never fires.
    let default_entries = osr_entries(None);
    // Override lowered to 100: trip=600 > 100 ⇒ the same loop now OSRs.
    let lowered_entries = osr_entries(Some("100"));
    let _ = std::fs::remove_file(&kernel);

    assert_eq!(
        default_entries, 0,
        "trip=600 below default 1000 must NOT OSR at the default threshold",
    );
    assert!(
        lowered_entries > 0,
        "RSS_JIT_OSR_THRESHOLD=100 must make trip=600 OSR via the counting auto-trigger",
    );
}

/// OSR × J3 for RESULTS (deopt-before-heap, Slice 1) — POSITIVE. A leaf `checked`
/// returns `Result<Int, String>`; its COLD `Err` arm builds a heap `String`. Called
/// in an I/O-tangled hot loop where the argument is ALWAYS >= 0 (the Err arm is never
/// built natively), the Ok `Result` is matched/unwrapped in-loop and dead at the
/// boundary. The OSR pre-pass inlines `checked`, splices its Err arm as a native
/// `Bail` (deopt-before-heap), and the Result scalar-replacement pass dissolves the
/// now-always-Ok Result to a scalar — so the loop OSRs. Byte-identical to the
/// interpreter (which builds a real `Ok` per iteration) AND `osr_entries > 0`.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_result_cold_bail_loop_matches_interpreter() {
    let source = "\
fn checked(value: Int) -> Result<Int, String> {
    if value < 0 { return Err(String.copy(value: read \"neg\")) }
    return Ok(value)
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let r: Result<Int, String> = checked(value: read i)
        match r {
            Ok(v) => { total = total + v }
            Err(e) => { total = total + 0 }
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-result-cold-bail.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J3 Result cold-bail loop must be byte-identical to the interpreter (stdout)"
    );
    // i in 0..60, always Ok(i): total = 0+1+...+59 = 1770.
    assert_eq!(osr.stdout.trim_end(), "begin\n1770\n1770");
    assert!(
        stats.osr_entries > 0,
        "the always-Ok Result loop must OSR (Err arm bails + Ok Result dissolves): {stats:?}",
    );
}

/// OSR × J3 for RESULTS — COLD-PATH-DRIVING (the deopt-before-heap soundness net).
/// The SAME shape, but the argument is SOMETIMES negative: on those iterations the
/// `Err(String)` arm IS taken. Natively the inlined Err arm is a `Bail`, so those
/// iterations deopt → the interpreter re-runs the whole loop and builds the real
/// `Err` itself. stdout must stay byte-identical to the pure interpreter — proving
/// the abandon-and-reinterpret-the-loop fallback is sound when the cold arm is taken.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_result_cold_bail_takes_err_arm_matches_interpreter() {
    let source = "\
fn checked(value: Int) -> Result<Int, String> {
    if value < 0 { return Err(String.copy(value: read \"neg\")) }
    return Ok(value)
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let arg = i - 30
        let r: Result<Int, String> = checked(value: read arg)
        match r {
            Ok(v) => { total = total + v }
            Err(e) => { total = total + 1000 }
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-result-cold-bail-err.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, _stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "Result cold-bail loop where the Err arm IS taken must still be interpreter-identical \
         (bail → reinterpret → interpreter builds Err)"
    );
    // arg = i-30 for i in 0..60. arg<0 for i in 0..30 (30 iters ⇒ +1000 each = 30000);
    // arg>=0 for i in 30..60 ⇒ total += arg = 0+1+...+29 = 435. Sum = 30435.
    assert_eq!(osr.stdout.trim_end(), "begin\n30435\n30435");
}

/// OSR × J3 for RESULTS — ESCAPING (the dead-at-boundary safety guard). The `Result`
/// is declared before the loop and matched AFTER it, so it is live across the loop
/// boundary: the Result region gate MUST refuse to scalar-replace it and therefore
/// MUST NOT OSR (`osr_entries == 0`), or the interpreter would read a stale slot the
/// native loop never wrote back. Still interpreter-identical (the whole loop runs on
/// the interpreter).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_escaping_result_does_not_osr() {
    let source = "\
fn checked(value: Int) -> Result<Int, String> {
    if value < 0 { return Err(String.copy(value: read \"neg\")) }
    return Ok(value)
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut r: Result<Int, String> = Ok(0)
    while i < limit {
        r = checked(value: read i)
        match r {
            Ok(v) => { total = total + v }
            Err(e) => { total = total + 0 }
        }
        i = i + 1
    }
    match r {
        Ok(v) => { Log.write(message: read String.from_int(value: v)) }
        Err(e) => { Log.write(message: read \"err\") }
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-escaping-result.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a Result read after the loop must still produce interpreter-identical output"
    );
    // i in 0..60: total = 0+...+59 = 1770; r = Ok(59) after the loop ⇒ "59".
    assert_eq!(osr.stdout.trim_end(), "begin\n59\n1770\n1770");
    assert_eq!(
        stats.osr_entries, 0,
        "a loop whose Result is live after the loop must NOT OSR (dead-at-boundary gate): {stats:?}",
    );
}

/// OSR × J3 combinator expansion (deopt-before-heap, Slice 2) — POSITIVE. The
/// `option_result_chain` shape: a hot loop chains `Option.map`/`and_then`/
/// `unwrap_or` (with inline `|v| {...}` mappers) and `Result.map`/`and_then`/
/// `unwrap_or` over `maybe_even` (Option) and `checked` (Result, heap Err arm),
/// tangled with `Log.write` I/O. The combinator-expansion pass lowers each
/// intrinsic to primitive match/construct form with the mapper sunk+inlined; the
/// Result Err arm bails (Slice 1) and Option/Result scalar-replacement dissolve the
/// per-iteration values, so the loop OSRs. Byte-identical to the interpreter AND
/// `osr_entries > 0`. The argument is ALWAYS even-then-odd alternating and >= 0, so
/// the None arm fires on odd indices but the Err arm never builds heap natively.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_combinator_chain_loop_matches_interpreter() {
    let source = "\
fn maybe_even(value: Int) -> Option<Int> {
    let half = value / 2
    if half * 2 == value { return Some(value) }
    return None
}

fn checked(value: Int) -> Result<Int, String> {
    if value < 0 { return Err(String.copy(value: read \"negative\")) }
    return Ok(value)
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut index = 0
    let mut total = 0
    while index < limit {
        let option_value = Option.and_then<Int, Int>(
            value: read Option.map<Int, Int>(
                value: read maybe_even(value: index),
                mapper: |value| { return value + 1 },
            ),
            mapper: |value| { return Some(value * 2) },
        )
        let option_total = Option.unwrap_or<Int>(value: read option_value, default: read 0)
        let result_value = Result.and_then<Int, String, Int>(
            result: read Result.map<Int, String, Int>(
                result: read checked(value: option_total),
                mapper: |value| { return value + 3 },
            ),
            mapper: |value| { return Ok(value * 2) },
        )
        total = total + Result.unwrap_or<Int, String>(value: read result_value, default: read 0)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 200)))
    return Unit
}
";
    let file = "jit-osr-j3-combinator-chain.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J3 combinator chain must be byte-identical to the interpreter (stdout)"
    );
    assert!(
        stats.osr_entries > 0,
        "the combinator chain loop must OSR (combinators expand + mappers inline + \
         Option/Result dissolve): {stats:?}",
    );
}

/// OSR × J3 combinator expansion — COLD-PATH-DRIVING (the deopt-before-heap net).
/// The combinator chain DYNAMICALLY hits None (odd index ⇒ `maybe_even` is None,
/// the Option combinator None arm) AND Err (negative arg ⇒ `checked` is Err, whose
/// expanded arm is a native `Bail`). On those iterations native bails →
/// the interpreter re-runs the whole loop and builds the real None/Err itself.
/// stdout must stay byte-identical to the pure interpreter — proving the
/// abandon-and-reinterpret-the-loop fallback is sound when the cold arms are taken.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_combinator_chain_cold_path_matches_interpreter() {
    let source = "\
fn maybe_even(value: Int) -> Option<Int> {
    let half = value / 2
    if half * 2 == value { return Some(value) }
    return None
}

fn checked(value: Int) -> Result<Int, String> {
    if value < 0 { return Err(String.copy(value: read \"negative\")) }
    return Ok(value)
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut index = 0
    let mut total = 0
    while index < limit {
        let arg = index - 50
        let option_value = Option.map<Int, Int>(
            value: read maybe_even(value: arg),
            mapper: |value| { return value + 1 },
        )
        let option_total = Option.unwrap_or<Int>(value: read option_value, default: read 7)
        let result_value = Result.map<Int, String, Int>(
            result: read checked(value: arg),
            mapper: |value| { return value + 3 },
        )
        total = total + Result.unwrap_or<Int, String>(value: read result_value, default: read 1000)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 200)))
    return Unit
}
";
    let file = "jit-osr-j3-combinator-cold-path.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, _stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "combinator chain hitting None (odd index) AND Err (negative arg) must still be \
         interpreter-identical (bail → reinterpret → interpreter builds None/Err)"
    );
}

/// OSR × J3 combinator expansion — ESCAPING (the dead-at-boundary safety guard).
/// The Option combinator result is declared before the loop and read AFTER it, so
/// it is live across the loop boundary: the Option scalar-replacement gate MUST
/// refuse to dissolve it ⇒ MUST NOT OSR (`osr_entries == 0`). Still
/// interpreter-identical (the whole loop runs on the interpreter).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_combinator_escaping_does_not_osr() {
    let source = "\
fn maybe_even(value: Int) -> Option<Int> {
    let half = value / 2
    if half * 2 == value { return Some(value) }
    return None
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut index = 0
    let mut total = 0
    let mut last: Option<Int> = None
    while index < limit {
        last = Option.map<Int, Int>(
            value: read maybe_even(value: index),
            mapper: |value| { return value + 1 },
        )
        total = total + Option.unwrap_or<Int>(value: read last, default: read 0)
        index = index + 1
    }
    match last {
        Some(v) => { Log.write(message: read String.from_int(value: v)) }
        None => { Log.write(message: read \"none\") }
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 200)))
    return Unit
}
";
    let file = "jit-osr-j3-combinator-escaping.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "an escaping combinator Option read after the loop must still be interpreter-identical"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a loop whose combinator Option is live after the loop must NOT OSR: {stats:?}",
    );
}
