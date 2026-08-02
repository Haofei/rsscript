// ---------------------------------------------------------------------------
// Self-tail-call optimization (TCO): a self-tail-call is rewritten to an
// arg-rebind + backward jump (recursion -> loop), so the function loses its
// call-graph self-edge and becomes native-eligible. These lock the three
// behaviours: a self-tail-recursive accumulator now runs *native*; non-tail and
// mutual recursion are left *untouched* (still interpreted); and an unbounded
// self-tail-recursion with no base case still trips the recursion-depth cap
// (the limit-observability soundness gate) rather than looping forever.
// ---------------------------------------------------------------------------

/// Positive: `sum_to(n, acc)` is self-tail-recursive with an accumulator, the
/// canonical TCO shape (`return sum_to(n: n - 1, acc: acc + n)`). After TCO it is
/// a loop with no self-edge, so it compiles + runs on the native tier
/// (`native_calls > 0`) — proof TCO made a previously recursion-only function
/// native-eligible. The driver tangles it with I/O (`Log.write`) to exercise the
/// real entry path, and the result must stay byte-identical to the interpreter.
#[cfg(feature = "native-jit")]
#[test]
fn tco_self_tail_accumulator_runs_native_and_matches_interpreter() {
    let source = "\
fn sum_to(n: Int, acc: Int) -> Int {
    if n == 0 {
        return acc
    }
    return sum_to(n: n - 1, acc: acc + n)
}

fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 100 {
        total = total + sum_to(n: 200, acc: 0)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "tco-self-tail-accumulator.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (native, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run should succeed");
    assert_eq!(
        interp.stdout, native.stdout,
        "TCO must not change the observable result"
    );
    assert_eq!(native.stdout.trim(), "2010000");
    assert!(
        stats.native_calls > 0 && stats.compiled > 0,
        "the self-tail-recursive accumulator must now run native after TCO: {stats:?}"
    );
    assert_eq!(stats.native_bails, 0, "no native bail expected: {stats:?}");
}

/// Negative (non-tail recursion): `fib(n) = fib(n-1) + fib(n-2)` is tree
/// recursion — the self-call result is consumed by `+`, so it is NOT in tail
/// position. TCO must NOT fire (the result is observed by an `AddInt`), so `fib`
/// stays genuinely recursive. It now runs NATIVELY via the native-call ABI's
/// self-recursion (`CallSelf`), not via TCO. Output must match the interpreter.
#[cfg(feature = "native-jit")]
#[test]
fn tco_leaves_non_tail_tree_recursion_untouched() {
    let source = "\
fn fib(n: Int) -> Int {
    if n < 2 {
        return n
    }
    return fib(n: n - 1) + fib(n: n - 2)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: fib(n: 20)))
    return Unit
}
";
    let file = "tco-non-tail-fib.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (native, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run should succeed");
    assert_eq!(
        interp.stdout, native.stdout,
        "result must match interpreter"
    );
    assert_eq!(native.stdout.trim(), "6765");
    // TCO does not fire (the self-call is not in tail position), so `fib` stays
    // genuinely recursive — but it now runs NATIVELY via the native-call ABI's
    // self-recursion (`CallSelf`), not via TCO and not on the interpreter.
    assert!(
        stats.native_calls > 0,
        "non-tail tree recursion should run via native self-recursion: {stats:?}"
    );
}

/// Mutual recursion (deep): `is_even`/`is_odd` (Bool) call *each other*. The Bool
/// group is native-eligible (slice 4 + Bool support). `is_even(1000)` recurses past
/// the native depth cap, so the deep entry bails cleanly at the cap and finishes on
/// the interpreter — but the interpreter's *shallow* sub-calls (depth < cap) still
/// complete natively. The result must match the interpreter regardless of where the
/// cap falls. The pure-native shallow case is `native_mutual_recursion_bool_runs_native`.
#[cfg(feature = "native-jit")]
#[test]
fn tco_leaves_mutual_recursion_untouched() {
    let source = "\
fn is_even(n: Int) -> Bool {
    if n == 0 {
        return true
    }
    return is_odd(n: n - 1)
}

fn is_odd(n: Int) -> Bool {
    if n == 0 {
        return false
    }
    return is_even(n: n - 1)
}

fn main() -> Unit {
    Log.write(message: read String.from_bool(value: is_even(n: 1000)))
    return Unit
}
";
    let file = "tco-mutual-recursion.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (native, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run should succeed");
    assert_eq!(
        interp.stdout, native.stdout,
        "result must match interpreter"
    );
    assert_eq!(native.stdout.trim(), "true");
    // The deep entry bails at the cap; shallow sub-calls (depth < cap) still complete
    // natively, so the native path participates without changing the result.
    assert!(
        stats.native_calls > 0,
        "deep mutual recursion should still native-accelerate its shallow sub-calls: {stats:?}"
    );
}

/// Soundness (recursion-depth-limit observability): `spin(n) = return spin(n+1)`
/// is a self-tail-call, but it has NO base case — every reachable exit is the
/// self-call. TCO MUST refuse it (converting it to a loop would replace the clean
/// `"recursion depth limit exceeded"` error with an infinite hang). We verify the
/// function still errors cleanly with the depth message under default limits,
/// proving the limit stays observable exactly as before.
#[test]
fn tco_preserves_depth_limit_for_baseless_self_tail_recursion() {
    let source = "\
fn spin(n: Int) -> Int {
    return spin(n: n + 1)
}

fn main() -> Int {
    return spin(n: 0)
}
";
    let err = common::reg_vm_eval_source_main_with_args(
        "tco-baseless-spin.rss",
        source,
        std::iter::empty::<String>(),
    )
    .expect_err("a baseless self-tail-recursion must error, not loop forever");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("recursion depth"),
        "expected a clean recursion-depth error (TCO must not convert a baseless \
         self-tail-recursion to an infinite loop), got: {msg}"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn tco_unreachable_base_case_preserves_depth_limit() {
    let source = "\
fn spin(n: Int) -> Int {
    if n == 0 {
        return 0
    }
    return spin(n: n)
}

fn main() -> Int {
    return spin(n: 1)
}
";
    let executable =
        common::compile_vm_source("tco-unreachable-base.rss", source).expect("compile");
    let err = executable
        .eval_main_with_args_native_with_limits(
            std::iter::empty::<String>(),
            rsscript::VmLimits {
                max_depth: 32,
                ..rsscript::VmLimits::default()
            },
        )
        .expect_err("an unreachable base case must not turn recursion into an unbounded loop");
    assert!(
        matches!(err, rsscript::EvalError::Runtime(ref message) if message.contains("recursion depth limit")),
        "expected recursion-depth error, got {err:?}"
    );
}

/// IntToFloat OSR positive test: a hot loop that converts the loop counter with
/// `Int.to_float` and then does pure FLOAT arithmetic (`+ f * 0.5 - 1.0`), wrapped
/// by non-native I/O (`Log.write` before/after) in the SAME function — so the
/// function is whole-function native-INELIGIBLE and only OSR can run the loop
/// natively. The in-loop `Int.to_float` lowers to a `CallIntrinsic { IntToFloat }`;
/// before native IntToFloat lowering existed this bailed OSR (a non-native
/// `CallIntrinsic` in-region), so the float loop never won. With the native
/// signed-int→f64 conversion (`fcvt_from_sint`) the loop is now OSR-eligible.
///
/// With OSR forced on the program's stdout (including the post-loop
/// `Float.to_string`) must be byte-identical to the pure interpreter — the float
/// formatting must match EXACTLY, which is the bit-parity net for the conversion.
/// Under `native-jit` we also assert the loop genuinely OSR'd (`osr_entries > 0`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_int_to_float_loop_matches_interpreter() {
    let source = "\
fn compute(limit: Int) -> Float {
    Log.write(message: read \"begin\")
    let mut index = 0
    let mut acc = 0.0
    while index < limit {
        let f = Int.to_float(value: read index)
        acc = acc + f * 0.5 - 1.0
        index = index + 1
    }
    Log.write(message: read Float.to_string(value: read acc))
    return acc
}

fn main() -> Unit {
    Log.write(message: read Float.to_string(value: read compute(limit: read 1000)))
    return Unit
}
";
    let file = "jit-osr-int-to-float.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR Int.to_float + float-arith loop must be byte-identical to the \
         interpreter (stdout, including the Float.to_string formatting)"
    );
    assert!(
        stats.osr_entries > 0,
        "an Int.to_float + float-arith loop must OSR natively now that IntToFloat \
         has a native fcvt_from_sint lowering: {stats:?}",
    );
}

/// `Math.floor`/`Math.ceil` OSR positive test (Float→Int native lowering): a hot
/// loop converts the counter to a float, biases it to a fractional value, then sums
/// `Math.floor` + `Math.ceil` of it — both Float→Int rounding casts. The loop is
/// wrapped by non-native I/O so only OSR can run it natively. Each `Math.floor`/
/// `Math.ceil` lowers to a `CallIntrinsic` that now becomes a native `FloatToInt`
/// (round, then saturating f64→i64). Output (the Int sum, formatted) must be
/// byte-identical to the interpreter — the bit-parity net for the rounding+cast —
/// and under `native-jit` the loop must genuinely OSR.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_float_to_int_loop_matches_interpreter() {
    let source = "\
fn compute(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut index = 0
    let mut total = 0
    while index < limit {
        let f = Int.to_float(value: read index) * 0.5 - 3.0
        total = total + Math.floor(value: read f) + Math.ceil(value: read f)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: compute(limit: read 1000)))
    return Unit
}
";
    let file = "jit-osr-float-to-int.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR Math.floor/Math.ceil (Float→Int) loop must be byte-identical to the \
         interpreter (stdout, including the rounding+saturating cast)"
    );
    assert!(
        stats.osr_entries > 0,
        "a Math.floor/Math.ceil loop must OSR natively now that Float→Int has a \
         native FloatToInt lowering: {stats:?}",
    );
}

/// IntToFloat OSR negative test — proving we admit only the *inline-convert*
/// intrinsics (`IntToFloat`/`Math.floor`/`Math.ceil`), not `CallIntrinsic` broadly.
/// This loop calls a DIFFERENT, still-unsupported intrinsic each iteration —
/// `String.to_uppercase`, a heap-String producer that is NOT in the native subset
/// AND is NOT a length-foldable producer (so the string length-fold pass cannot
/// dissolve it either, leaving a real non-subset `StringLen`). Because that
/// `CallIntrinsic` is in-region and unsupported, the loop MUST NOT OSR
/// (`osr_entries == 0`), or the interpreter would diverge. The output must still be
/// interpreter-identical. This guards against a future edit accidentally broadening
/// the `CallIntrinsic` admission beyond the inline-convert shapes.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_other_intrinsic_in_loop_does_not_osr() {
    let source = "\
fn compute(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut index = 0
    let mut total = 0
    while index < limit {
        let s = String.to_uppercase(value: read \"abc\")
        total = total + String.len(value: read s)
        index = index + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: compute(limit: read 1000)))
    return Unit
}
";
    let file = "jit-osr-other-intrinsic.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a loop using a still-unsupported intrinsic must stay interpreter-identical"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a loop with a non-IntToFloat CallIntrinsic in-region must NOT OSR — only the \
         single IntToFloat shape is admitted, not CallIntrinsic broadly: {stats:?}",
    );
}

/// OSR × J3 string length-law folding — POSITIVE test. A hot loop builds a
/// non-escaping string (`from_int` → `concat` → `slice`) used ONLY by
/// `String.len`, inside an I/O-tangled (native-INELIGIBLE) function. The length
/// fold dissolves every allocation to arithmetic on operand byte lengths, leaving a
/// pure-scalar loop that OSRs. We assert byte-identical stdout to the interpreter
/// AND that the loop genuinely OSR'd (`osr_entries > 0`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_string_length_fold_loop_matches_interpreter() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let s = String.from_int(value: i)
        let k = String.concat(left: read \"v=\", right: read s)
        let h = String.slice(value: read k, start: 0, len: 3)
        total = total + String.len(value: read k) + String.len(value: read h)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 50)))
    return Unit
}
";
    let file = "jit-osr-j3-string-length-fold.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J3 string length-fold loop must be byte-identical to the interpreter (stdout)"
    );
    // i in 0..50: len(k)=2+digits(i) summed = 100 + (10*1 + 40*2) = 190; len(h)=
    // min(3, 2+digits(i)) = 3 always ⇒ 150. total = 340.
    assert_eq!(osr.stdout.trim_end(), "begin\n340\n340");
    assert!(
        stats.osr_entries > 0,
        "a non-escaping length-only string loop must OSR after length-law folding: {stats:?}",
    );
}

/// OSR × J3 string length-fold — NEGATIVE test (escape). The constructed string is
/// ALSO logged (escapes the loop region), so its allocation cannot be deleted: the
/// pass must refuse to fold it and the loop must NOT OSR (`osr_entries == 0`). The
/// program stays interpreter-identical (the interpreter runs the whole loop).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_string_length_fold_escaping_does_not_osr() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let s = String.from_int(value: i)
        let k = String.concat(left: read \"v=\", right: read s)
        total = total + String.len(value: read k)
        if i == 0 {
            Log.write(message: read k)
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 50)))
    return Unit
}
";
    let file = "jit-osr-j3-string-length-fold-escaping.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "an escaping (logged) constructed string must stay interpreter-identical"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a constructed string that ESCAPES (is logged) must NOT fold/OSR: {stats:?}",
    );
}

/// OSR × J3 string length-fold — NEGATIVE test (unprovable law / non-ASCII slice).
/// The string is built from a NON-ASCII literal, so `String.slice`'s char-boundary
/// clamp depends on the actual bytes and the slice length law is NOT provable: the
/// pass must bail that producer and the loop must NOT OSR (`osr_entries == 0`).
/// `String.len` is byte length, so the byte-vs-char distinction is exercised here.
/// Stays interpreter-identical.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_string_length_fold_non_ascii_slice_does_not_osr() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    while i < limit {
        let s = String.from_int(value: i)
        let k = String.concat(left: read \"café-\", right: read s)
        let h = String.slice(value: read k, start: 0, len: 3)
        total = total + String.len(value: read h)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 50)))
    return Unit
}
";
    let file = "jit-osr-j3-string-length-fold-non-ascii.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a non-ASCII slice (unprovable length law) must stay interpreter-identical"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a String.slice of an unprovably-ASCII string must NOT fold/OSR: {stats:?}",
    );
}

// --- Lever 2: RSS_JIT_REPORT missed-optimization report correctness ----------
//
// These tests pin that the observational report emits the RIGHT reason for the
// canonical cases AND that the reason is accurate — a region the report says went
// "native: ok" / "osr: entered" really did (stats back it), and one it says "not
// native/osr: <reason>" really did not. The report is observational, so the run's
// output is byte-identical to the plain OSR path (verified throughout).

/// Find the report block for function `name` (blocks are `\n`-joined, first line
/// is `jit-report: fn \`<name>\``).
#[cfg(feature = "native-jit")]
fn report_block<'a>(lines: &'a [String], name: &str) -> &'a str {
    let needle = format!("fn `{name}`");
    lines
        .iter()
        .find(|b| b.lines().next().is_some_and(|h| h.contains(&needle)))
        .map(|s| s.as_str())
        .unwrap_or_else(|| panic!("no report block for fn `{name}` in {lines:#?}"))
}

#[cfg(feature = "native-jit")]
#[test]
fn report_profile_guided_pic_shows_hottest_first_order() {
    let source = "\
fn dispatch(f: read Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 400 {
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
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-report-profile-pic.rss";
    // Default `enforce` declines this loop-free PIC; disable the cost model so the
    // PIC compiles and the profile-guided report details are present to assert.
    rsscript::with_native_cost_model_disabled(|| {
        let interp = common::run_vm_source(file, source, &[]).expect("interp run");
        let (out, stats, lines) = common::reg_vm_eval_source_main_native_osr_report(
            file,
            source,
            std::iter::empty::<String>(),
        )
        .expect("osr+report run");
        assert_eq!(
            interp.stdout, out.stdout,
            "report run must be byte-identical"
        );
        let block = report_block(&lines, "dispatch");
        assert!(
            block.contains("profile: closure@")
                && block.contains("polymorphic")
                && block.contains("observed=[")
                && block.contains("pic=hottest-first[")
                && block.contains("pic_arms=3"),
            "weighted closure dispatcher should report profile-guided PIC details, got:\n{block}",
        );
        assert!(
            stats.profile_closure_id_reads > 0,
            "report fixture should compile a polymorphic closure PIC: {stats:?}",
        );
    });
}

/// Telemetry (reviewer #5): the report must explain "why no JIT" for a cost-model
/// decline. The SAME polymorphic-PIC program, run under the DEFAULT (enforce) cost
/// model (no override), is declined as unprofitable — the report surfaces both a
/// `cost-model decline summary` block (with the score breakdown) and the per-function
/// `declined by cost model` verdict, so a developer sees why it stayed interpreted.
#[cfg(feature = "native-jit")]
#[test]
fn report_explains_cost_model_decline_for_polymorphic_pic() {
    let source = "\
fn dispatch(f: read Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 400 {
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
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-report-cost-model-decline.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    // No cost-model override: the default (enforce) declines the loop-free PIC.
    let (out, stats, lines) = common::reg_vm_eval_source_main_native_osr_report(
        file,
        source,
        std::iter::empty::<String>(),
    )
    .expect("report run");
    assert_eq!(
        interp.stdout, out.stdout,
        "decline must stay byte-identical"
    );
    assert!(
        stats.unprofitable_declines > 0,
        "the PIC must be declined under the default cost model: {stats:?}",
    );
    let report = lines.join("\n");
    // The cost-model decline summary (built from this run's telemetry) shows the PIC
    // decline with its score breakdown...
    assert!(
        report.contains("jit-report: cost-model decline summary") && report.contains("pic_sites=1"),
        "report must summarize the cost-model decline with its score breakdown, got:\n{report}",
    );
    // ...and the per-function verdict now attributes it to the actual declined
    // function via runtime attribution (item #2) — reliable even for a profile-guided
    // PIC, which a re-derivation would miss.
    assert!(
        report.contains("not native: declined by cost model"),
        "a function block must be attributed to the cost-model decline, got:\n{report}",
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn report_shows_profile_guided_branch_feedback() {
    let source = "\
fn branchy(i: Int) -> Int {
    let marker = Path.from_string(value: read \"branch-profile\")
    if i % 4 == 0 {
        return 10
    }
    return 1
}

fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 400 {
        total = total + branchy(i: read i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-report-branch-feedback.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let (out, stats, lines) = common::reg_vm_eval_source_main_native_osr_report(
        file,
        source,
        std::iter::empty::<String>(),
    )
    .expect("osr+report run");
    assert_eq!(
        interp.stdout, out.stdout,
        "branch-profile report run must be byte-identical"
    );
    let block = report_block(&lines, "branchy");
    assert!(
        block.contains("profile: branch@")
            && block.contains("taken=")
            && block.contains("fallthrough=")
            && block.contains("taken_pct=")
            && block.contains("bias="),
        "branch-heavy loop should report branch feedback, got:\n{block}",
    );
    assert!(
        stats.profile_branch_sites > 0 && stats.profile_branch_samples > 0,
        "branch feedback should also be exposed through NativeStats: {stats:?}",
    );
}

/// A winning pure-Int scalar loop: the whole function is in the native subset, so it
/// compiles+runs as one native body (`native: ok`) and OSR never has to fire. The
/// report must say `native: ok` and `osr: eligible` (the loop body IS in the subset),
/// and the stats must confirm the function actually ran natively (accuracy).
#[cfg(feature = "native-jit")]
#[test]
fn report_native_scalar_loop_says_native_ok() {
    let source = "\
fn hot(limit: Int, seed: Int) -> Int {
    let mut i = 0
    let mut total = seed
    while i < limit {
        total = total + i * 3 - i / 2 + 7
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: hot(limit: read 50, seed: read 0)))
    return Unit
}
";
    let file = "jit-report-native-scalar.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let (out, stats, lines) = common::reg_vm_eval_source_main_native_osr_report(
        file,
        source,
        std::iter::empty::<String>(),
    )
    .expect("osr+report run");
    assert_eq!(
        interp.stdout, out.stdout,
        "report run must be byte-identical"
    );
    let block = report_block(&lines, "hot");
    assert!(
        block.contains("native: ok") && block.contains("osr: n/a"),
        "scalar loop should report native: ok + osr: n/a, got:\n{block}"
    );
    assert!(
        stats.native_calls >= 1,
        "accuracy: function really ran native: {stats:?}"
    );
}

/// Native Bytes-slice test: `Bytes.slice` + `Bytes.len` now lower through the
/// helper-backed native path, so this whole function can run native. The report
/// must say `native: ok` and `osr: n/a`, and stats must confirm native execution.
#[cfg(feature = "native-jit")]
#[test]
fn report_bytes_scan_says_native_ok() {
    let source = "\
fn scan(data: read Bytes, limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        let head = Bytes.slice(value: read data, start: 0, len: 5)
        total = total + Bytes.len(value: read head)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    let data = Bytes.from_string(value: read \"the quick brown fox\")
    Log.write(message: read String.from_int(value: scan(data: read data, limit: read 50)))
    return Unit
}
";
    let file = "jit-report-bytes-scan.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let (out, stats, lines) = common::reg_vm_eval_source_main_native_osr_report(
        file,
        source,
        std::iter::empty::<String>(),
    )
    .expect("osr+report run");
    assert_eq!(
        interp.stdout, out.stdout,
        "report run must be byte-identical"
    );
    let block = report_block(&lines, "scan");
    assert!(
        block.contains("native: ok") && block.contains("osr: n/a"),
        "bytes helper loop should report native: ok + osr: n/a, got:\n{block}"
    );
    assert!(
        stats.native_calls >= 1 && stats.osr_entries == 0,
        "accuracy: bytes loop should run whole-function native, not OSR: {stats:?}"
    );
}

/// POSITIVE Bytes-fold test: a non-escaping Bytes value built ONLY to be measured
/// (`Bytes.len` of `Bytes.slice`) in an I/O-tangled function, with the source `data`
/// a loop-invariant `Bytes.from_string(<literal>)` defined before the loop (so its byte
/// length is a compile-time constant). The Bytes length-fold dissolves the per-iteration
/// `Bytes.slice` allocation into byte-length arithmetic and the loop OSRs to native.
/// Read-only: the result MUST be byte-identical to the interpreter AND `osr_entries > 0`.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_bytes_length_fold_constant_source_osrs_and_matches_interpreter() {
    // `data = "the quick brown fox"` is 19 bytes. Per iteration:
    //   len(slice(data,0,5)) + len(data) = 5 + 19 = 24.
    // limit = 50 ⇒ total = 24 * 50 = 1200. The leading `Log.write("begin")` makes the
    // function native-INELIGIBLE as a whole, so only the hot loop can OSR.
    let source = "\
fn scan(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let data = Bytes.from_string(value: read \"the quick brown fox\")
    let mut index = 0
    let mut total = 0
    while index < limit {
        let head = Bytes.slice(value: read data, start: 0, len: 5)
        total = total + Bytes.len(value: read head) + Bytes.len(value: read data)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: scan(limit: read 50)))
    return Unit
}
";
    let file = "jit-osr-bytes-fold-positive.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "Bytes length-fold loop must be byte-identical to the interpreter (stdout)"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n1200");
    assert!(
        stats.osr_entries > 0,
        "the non-escaping constant-source Bytes length loop must OSR after the fold: {stats:?}",
    );
}

/// BUG-1 REGRESSION (Bytes.len in the loop CONDITION): a folded `Bytes.len` read in the
/// header itself (`while index < Bytes.len(data)`) over a non-escaping constant-source
/// Bytes, in an I/O-tangled fn. The fold materializes the constant length register at the
/// header so it is definitely-assigned on entry to the native OSR header block; without
/// that, OSR entry (which lands AT the header) reads the length register BEFORE it is
/// initialized ⇒ uninitialized/garbage loop bound ⇒ wrong total. The loop MUST OSR
/// (`osr_entries > 0`) AND be byte-identical to the interpreter.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_bytes_length_fold_in_condition_osrs_and_matches_interpreter() {
    // `data = "the quick brown fox"` is 19 bytes; `Bytes.len(data)` in the condition
    // folds to the constant 19. The loop runs 19 times, adding 7 each iteration ⇒
    // total = 19 * 7 = 133. The leading `Log.write("begin")` makes the function
    // native-INELIGIBLE as a whole, so only the hot loop can OSR.
    let source = "\
fn scan() -> Int {
    Log.write(message: read \"begin\")
    let data = Bytes.from_string(value: read \"the quick brown fox\")
    let mut index = 0
    let mut total = 0
    while index < Bytes.len(value: read data) {
        total = total + 7
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: scan()))
    return Unit
}
";
    let file = "jit-osr-bytes-fold-condition.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "Bytes.len-in-condition loop must be byte-identical to the interpreter (stdout)"
    );
    assert_eq!(osr.stdout.trim_end(), "begin\n133");
    assert!(
        stats.osr_entries > 0,
        "the constant-source Bytes.len-in-condition loop must OSR after the fold: {stats:?}",
    );
}

/// NEGATIVE Bytes-fold test (escape): the same constant-source Bytes value, but the
/// `head` slice ESCAPES the measurement — it is also passed to `Log.write` (an opaque
/// non-fold consumer). The escape analysis must refuse to dissolve `head` (its
/// allocation is observed), so the allocating `Bytes.slice` survives and the loop does
/// NOT OSR (`osr_entries == 0`), while the result stays interpreter-identical.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_bytes_length_fold_escaping_slice_does_not_osr() {
    let source = "\
fn scan(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let data = Bytes.from_string(value: read \"the quick brown fox\")
    let mut index = 0
    let mut total = 0
    while index < limit {
        let head = Bytes.slice(value: read data, start: 0, len: 5)
        total = total + Bytes.len(value: read head)
        Log.write(message: read Bytes.to_string(value: read head))
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: scan(limit: read 3)))
    return Unit
}
";
    let file = "jit-osr-bytes-fold-escaping.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "escaping-Bytes loop must be byte-identical to the interpreter (stdout)"
    );
    assert_eq!(
        stats.osr_entries, 0,
        "a Bytes slice that escapes to Log.write must NOT be dissolved / must NOT OSR: {stats:?}",
    );
}

/// `String.slice` with a runtime source parameter now lowers through the
/// helper-backed native path, so this whole function can run native. The report
/// must say `native: ok` and `osr: n/a`; stats confirm native execution.
#[cfg(feature = "native-jit")]
#[test]
fn report_string_slice_runtime_source_says_native_ok() {
    let source = "\
fn build(text: read String, limit: Int) -> Int {
    let mut index = 0
    let mut total = 0
    while index < limit {
        let head = String.slice(value: read text, start: 0, len: 3)
        total = total + String.len(value: read head)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: build(text: read \"hello world\", limit: read 50)))
    return Unit
}
";
    let file = "jit-report-string-slice.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let (out, stats, lines) = common::reg_vm_eval_source_main_native_osr_report(
        file,
        source,
        std::iter::empty::<String>(),
    )
    .expect("osr+report run");
    assert_eq!(
        interp.stdout, out.stdout,
        "report run must be byte-identical"
    );
    let block = report_block(&lines, "build");
    assert!(
        block.contains("native: ok") && block.contains("osr: n/a"),
        "string helper loop should report native: ok + osr: n/a, got:\n{block}"
    );
    assert!(
        stats.native_calls >= 1 && stats.osr_entries == 0,
        "accuracy: string helper loop should run whole-function native, not OSR: {stats:?}"
    );
}

/// A non-loop function (no back-edge): the report must say `not osr: no loop`, and
/// it must never claim an OSR entry. Pins the negative-shape reason.
#[cfg(feature = "native-jit")]
#[test]
fn report_loopless_function_says_no_loop() {
    let source = "\
fn add3(a: Int, b: Int, c: Int) -> Int {
    let s = a + b
    let t = s + c
    return t + 1
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: add3(a: read 1, b: read 2, c: read 3)))
    return Unit
}
";
    let file = "jit-report-loopless.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let (out, _stats, lines) = common::reg_vm_eval_source_main_native_osr_report(
        file,
        source,
        std::iter::empty::<String>(),
    )
    .expect("osr+report run");
    assert_eq!(
        interp.stdout, out.stdout,
        "report run must be byte-identical"
    );
    let block = report_block(&lines, "add3");
    assert!(
        block.contains("not osr: no loop"),
        "loopless fn should report 'not osr: no loop', got:\n{block}"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn report_groups_native_decline_reasons_by_count() {
    let source = "\
fn blocked_a() -> Int {
    let path = Path.from_string(value: read \"alpha\")
    return String.len(value: read Path.to_string(path: read path))
}

fn blocked_b() -> Int {
    let path = Path.from_string(value: read \"beta\")
    return String.len(value: read Path.to_string(path: read path))
}

fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 8 {
        total = total + blocked_a() + blocked_b()
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-report-decline-summary.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let (out, stats, lines) = common::reg_vm_eval_source_main_native_osr_report(
        file,
        source,
        std::iter::empty::<String>(),
    )
    .expect("osr+report run");
    assert_eq!(
        interp.stdout, out.stdout,
        "report run must be byte-identical"
    );
    let block = report_block(&lines, "blocked_a");
    let reason = block
        .lines()
        .find_map(|line| line.strip_prefix("  not native: "))
        .expect("blocked_a should have a native decline reason");
    let summary = lines
        .iter()
        .find(|line| line.starts_with("jit-report: native decline summary"))
        .unwrap_or_else(|| panic!("missing native decline summary in {lines:#?}"));
    assert!(
        summary.contains(&format!("  2x {reason}")),
        "decline summary should group equivalent blocked functions; reason={reason:?}; summary:\n{summary}",
    );
    let stats_json = stats.to_json();
    assert_eq!(
        stats_json["native_decline_reasons"][reason].as_u64(),
        Some(2),
        "NativeStats JSON should expose grouped decline reasons for perf tooling; stats={stats_json}",
    );
}

/// `option_result_chain` report: the combinator mappers and the scalar+`Option`
/// leaf `maybe_even` inline into the OSR region and the `Option`s dissolve, so the
/// loop OSRs even though the enclosing function is NOT whole-function native. The
/// report accurately notes BOTH the whole-function decline (a non-inlinable call)
/// and the OSR entry; stats confirm the OSR.
#[cfg(feature = "native-jit")]
#[test]
fn report_option_result_chain_osrs_via_combinator_expansion() {
    let source = "\
fn maybe_even(value: Int) -> Option<Int> {
    let half = value / 2
    if half * 2 == value { return Some(value) }
    return None
}

fn f(limit: Int) -> Int {
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
        total = total + Option.unwrap_or<Int>(value: read option_value, default: read 0)
        index = index + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 200)))
    return Unit
}
";
    let file = "jit-report-option-result-chain.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let (out, stats, lines) = common::reg_vm_eval_source_main_native_osr_report(
        file,
        source,
        std::iter::empty::<String>(),
    )
    .expect("osr+report run");
    assert_eq!(
        interp.stdout, out.stdout,
        "report run must be byte-identical"
    );
    let block = report_block(&lines, "f");
    assert!(
        block.contains("osr: entered"),
        "combinator chain loop should OSR after combinator expansion + inline, got:\n{block}"
    );
    assert!(
        block.contains("non-inlinable call"),
        "report should accurately note the whole-function native decline, got:\n{block}"
    );
    assert!(
        stats.osr_entries > 0,
        "accuracy: combinator chain OSRs in the current pipeline: {stats:?}"
    );
}

/// Direct typed-list reads on the OSR path (perf win): a hot loop indexing a
/// loop-invariant **non-param** `List<Int>` built before the loop, read in-loop only
/// via `List.get`/`List.len`, inside an I/O-tangled (native-INELIGIBLE) function. The
/// list is marshalled into the OSR live-in window as a borrow-pinned flat buffer and
/// its `List.get`/`List.len` lower to bounds-checked direct loads (no per-iteration
/// host helper). Output must be byte-identical to the interpreter AND the loop must
/// genuinely OSR (`osr_entries > 0`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_direct_invariant_list_read_matches_interpreter() {
    let source = "\
fn main() -> Unit {
    Log.write(message: read \"begin\")
    let mut index = 0
    let mut step_index = 0
    let mut total = 0
    local steps = List<Int>.new()
    List.push<Int>(list: mut steps, value: read 2)
    List.push<Int>(list: mut steps, value: read 3)
    List.push<Int>(list: mut steps, value: read 5)
    List.push<Int>(list: mut steps, value: read 7)
    while index < 8000 {
        let step = List.get<Int>(list: read steps, index: step_index)
        total = total + index * step
        index = index + 1
        step_index = step_index + 1
        if step_index == List.len<Int>(list: read steps) {
            step_index = 0
        }
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-osr-direct-invariant-list.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR direct invariant-list loop must be byte-identical to the interpreter"
    );
    // total = sum over index 0..7999 of index * steps[index % 4] (steps = [2,3,5,7]).
    // The byte-identity assertion above is the correctness net; we additionally pin the
    // shape (a "begin" line then the numeric total) to catch a silently-empty run.
    assert!(
        osr.stdout.starts_with("begin\n") && osr.stdout.trim_end().lines().count() == 2,
        "expected a begin line then the numeric total, got: {:?}",
        osr.stdout
    );
    // The loop must genuinely OSR: under the direct typed-list path the in-loop
    // `List.get`/`List.len` are bounds-checked direct loads (no per-iteration host
    // helper) and `List.len` is hoisted to the marshalled length. (The helper-call
    // elimination itself is exercised by the vm-jit `ListGetIntDirect`/`ListLenDirect`
    // OSR-window unit test and confirmed by the int-arith benchmark.)
    assert!(
        stats.osr_entries > 0,
        "the loop-invariant typed-list loop must OSR natively: {stats:?}",
    );
}

/// A list MUTATED inside the loop (`List.push` in-loop). The list `xs` is a function
/// LOCAL (non-parameter), so it is handle-accessed — NOT a pinned flat buffer (flat
/// pinning is params-only) — which makes in-loop growth safe. So the loop now OSRs (the
/// growth-admissibility veto allows non-parameter list growth, J0.4 #1) and the
/// `List.get`/`List.push` go through the journaled heap helpers. The result must stay
/// byte-identical to the interpreter — this guards that a mutated list is NOT wrongly
/// treated as a fixed flat buffer (which would corrupt reads after a realloc).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_mutated_list_in_loop_stays_correct() {
    let source = "\
fn main() -> Unit {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    local xs = List<Int>.new()
    List.push<Int>(list: mut xs, value: read 1)
    while i < 3000 {
        total = total + List.get<Int>(list: read xs, index: 0)
        List.push<Int>(list: mut xs, value: read i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    Log.write(message: read String.from_int(value: List.len<Int>(list: read xs)))
    return Unit
}
";
    let file = "jit-osr-mutated-list.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = common::compile_vm_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a list mutated in-loop must stay interpreter-identical"
    );
    // xs[0] is always 1 ⇒ total = 3000; final len = 1 + 3000 = 3001. Byte-identity (and
    // the cross-backend differential) is the correctness net that proves the mutated
    // local list is handle-accessed correctly across the in-loop realloc, not pinned.
    assert_eq!(osr.stdout.trim_end(), "begin\n3000\n3001");
    // The local (non-parameter) list is handle-accessed, so its in-loop growth is safe
    // and the loop OSRs (J0.4 #1: non-parameter list growth is admissible).
    assert!(
        stats.osr_entries > 0,
        "a mutated non-parameter (handle-accessed) list loop should OSR safely: {stats:?}",
    );
}

/// Out-of-bounds direct read: a loop-invariant typed list indexed past its length
/// must deopt to interpreter-identical behavior (a real out-of-bounds error), NOT UB.
/// The bounds-checked direct load bails at the OOB index and the interpreter re-runs
/// the loop and raises the out-of-bounds itself, so the program's observable result
/// matches the pure interpreter exactly.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_direct_list_oob_deopts_like_interpreter() {
    let source = "\
fn main() -> Unit {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    local xs = List<Int>.new()
    List.push<Int>(list: mut xs, value: read 10)
    List.push<Int>(list: mut xs, value: read 20)
    while i < 100 {
        total = total + List.get<Int>(list: read xs, index: i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    let file = "jit-osr-direct-list-oob.rss";
    // Sanity: the program compiles (a malformed source, not an OOB, would fail here).
    common::compile_vm_source(file, source).expect("source compiles");
    let interp = common::run_vm_source(file, source, &[]);
    let osr =
        common::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>());
    // Both must agree: an out-of-bounds index at i==2 raises the SAME error (or same
    // partial output) under OSR as under the interpreter. The direct read bounds-check
    // deopts at the OOB index; the interpreter then raises the real OOB.
    match (interp, osr) {
        (Ok(interp), Ok(osr)) => assert_eq!(
            interp.stdout, osr.stdout,
            "OOB direct read must be interpreter-identical (stdout)"
        ),
        (Err(_), Err(_)) => { /* both error the same way: OOB raised */ }
        (i, o) => panic!("interpreter/OSR disagree on OOB outcome: interp={i:?} osr={o:?}"),
    }
}

/// Store-to-load forwarding removes the second direct-list bounds check, but the
/// preceding store remains guarded. An out-of-bounds store must therefore still
/// deopt and reproduce the interpreter error rather than reaching the forwarded
/// value.
#[cfg(feature = "native-jit")]
#[test]
fn native_direct_list_forwarded_load_keeps_store_oob_guard() {
    let source = "\
fn replace(xs: mut List<Int>, index: Int) -> Int {
    List.set<Int>(list: mut xs, index: index, value: 9)
    return List.get<Int>(list: xs, index: index)
}

fn main() -> Unit {
    local xs = List<Int>.new()
    List.push<Int>(list: mut xs, value: 1)
    Log.write(message: \"begin\")
    let value = replace(xs: mut xs, index: 1)
    Log.write(message: String.from_int(value: value))
    return Unit
}
";
    let file = "jit-direct-list-forwarded-load-oob.rss";
    common::compile_vm_source(file, source).expect("source compiles");
    let interp = common::run_vm_source(file, source, &[]);
    let native =
        common::reg_vm_eval_source_main_native(file, source, std::iter::empty::<String>());

    assert!(
        matches!((&interp, &native), (Err(_), Err(_))),
        "interpreter and native execution must both reject the out-of-bounds store: \
         interp={interp:?} native={native:?}",
    );
}

/// Native-call ABI (slice 3): a non-tail self-recursive `fib` runs NATIVELY (via
/// `CallSelf`) and is byte-identical to the interpreter. `native_calls > 0` proves
/// the native self-recursive path actually executed (not the tier-0 scalar executor).
#[cfg(feature = "native-jit")]
#[test]
fn native_self_recursive_fib_runs_native_and_matches_interpreter() {
    let source = "\
fn fib(n: Int) -> Int {
    if n < 2 { return n }
    return fib(n: read n - 1) + fib(n: read n - 2)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: fib(n: read 28)))
    return Unit
}
";
    let interp = common::run_vm_source("native-fib.rss", source, &[]).expect("interp");
    let exe = common::compile_vm_source("native-fib.rss", source).expect("compile");
    let (out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert_eq!(out.stdout.trim_end(), "317811");
    assert_eq!(
        interp.stdout, out.stdout,
        "fib native must match the interpreter"
    );
    assert!(
        stats.native_calls > 0,
        "fib must run via the native self-recursive path: {stats:?}",
    );
}

/// Native-call ABI (slice 3): self-recursion DEEPER than the native depth cap must
/// stay correct — the entry depth guard bails to the interpreter/tier-0 (no host
/// stack overflow / crash) and the result still matches the interpreter.
#[cfg(feature = "native-jit")]
#[test]
fn native_self_recursive_deep_recursion_stays_correct_past_cap() {
    // Linear sum to depth 1000 (well past the native cap): n + sum(n-1), sum(0)=0.
    let source = "\
fn sum(n: Int) -> Int {
    if n <= 0 { return 0 }
    return n + sum(n: read n - 1)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: sum(n: read 1000)))
    return Unit
}
";
    let interp = common::run_vm_source("native-deep-sum.rss", source, &[]).expect("interp");
    let exe = common::compile_vm_source("native-deep-sum.rss", source).expect("compile");
    let (out, _stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run (deep recursion bails to fallback, no crash)");
    assert_eq!(out.stdout.trim_end(), "500500");
    assert_eq!(
        interp.stdout, out.stdout,
        "deep self-recursion past the native cap must stay interpreter-identical"
    );
}

/// Native-call ABI (slice 3) — PERFORMANCE PROOF: native self-recursive `fib`
/// (Cranelift, via `CallSelf`) is materially faster than the non-native baseline.
/// Times the best of several runs of `fib(32)` on each path and asserts the native
/// path wins (and actually ran natively). Prints the measured speedup.
#[cfg(feature = "native-jit")]
#[test]
#[ignore = "release-only performance gate"]
fn native_self_recursion_perf_beats_baseline() {
    let source = "\
fn fib(n: Int) -> Int {
    if n < 2 { return n }
    return fib(n: read n - 1) + fib(n: read n - 2)
}
fn main() -> Unit {
    Log.write(message: read String.from_int(value: fib(n: read 32)))
    return Unit
}
";
    // Confirm it genuinely runs natively (not the tier-0 scalar executor).
    let exe = common::compile_vm_source("perf-fib.rss", source).expect("compile");
    let (_o, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert!(stats.native_calls > 0, "fib must run natively: {stats:?}");

    let best = |native: bool| -> std::time::Duration {
        let mut best = std::time::Duration::MAX;
        for _ in 0..3 {
            let exe = common::compile_vm_source("perf-fib.rss", source).expect("compile");
            let t = std::time::Instant::now();
            let out = if native {
                exe.eval_main_with_args_native(std::iter::empty::<String>())
            } else {
                exe.eval_main_with_args(std::iter::empty::<String>())
            }
            .expect("run");
            let dt = t.elapsed();
            assert_eq!(out.stdout.trim_end(), "2178309");
            best = best.min(dt);
        }
        best
    };
    let baseline = best(false);
    let native = best(true);
    let speedup = baseline.as_secs_f64() / native.as_secs_f64();
    eprintln!("fib(32): baseline={baseline:?} native={native:?} speedup={speedup:.2}x");
    assert!(
        native < baseline,
        "native self-recursion ({native:?}) must beat the baseline ({baseline:?})"
    );
}

/// Mutual recursion generalized to scalar Float (the Phase-2 treatment applied to
/// the group path): a FLOAT-returning mutually-recursive cycle now runs natively via
/// the co-compiled group, with Float params/return marshalled via to_bits/from_bits.
/// Before, the group analysis admitted only Int/Bool members. Byte-identical to the
/// interpreter (incl. float formatting) and `native_calls > 0`.
#[cfg(feature = "native-jit")]
#[test]
fn native_mutual_recursion_float_runs_native() {
    let source = "\
fn fa(n: Int) -> Float {
    if n <= 0 { return 1.0 }
    return 1.5 + fb(n: n - 1)
}
fn fb(n: Int) -> Float {
    if n <= 0 { return 2.0 }
    return 0.5 + fa(n: n - 1)
}
fn main() -> Unit {
    Log.write(message: read Float.to_string(value: read fa(n: 11)))
    Log.write(message: read Float.to_string(value: read fb(n: 10)))
    return Unit
}
";
    let interp = common::run_vm_source("native-mutual-float.rss", source, &[]).expect("interp");
    let exe = common::compile_vm_source("native-mutual-float.rss", source).expect("compile");
    let (out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert_eq!(
        interp.stdout, out.stdout,
        "Float mutual recursion native must be byte-identical to the interpreter"
    );
    assert!(
        stats.native_calls > 0,
        "Float mutual recursion should run via the native group path: {stats:?}",
    );
}

/// Native-call ABI (slice 4): MUTUAL recursion `is_even`/`is_odd` runs NATIVELY via
/// the co-compiled group (CallGroup), byte-identical to the interpreter.
/// `native_calls > 0` proves the native group path executed.
#[cfg(feature = "native-jit")]
#[test]
fn native_mutual_recursion_runs_native_and_matches_interpreter() {
    let source = "\
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
    Log.write(message: read String.from_int(value: is_odd(n: read 21)))
    return Unit
}
";
    let interp = common::run_vm_source("native-mutual.rss", source, &[]).expect("interp");
    let exe = common::compile_vm_source("native-mutual.rss", source).expect("compile");
    let (out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert_eq!(out.stdout.trim_end(), "1\n1");
    assert_eq!(
        interp.stdout, out.stdout,
        "mutual recursion native must match interpreter"
    );
    assert!(
        stats.native_calls > 0,
        "is_even/is_odd must run via the native mutual-recursion group: {stats:?}",
    );
}

/// Native-call ABI (slice 4 + Bool support): a Bool-returning mutual-recursion
/// group (`is_even`/`is_odd` -> Bool) runs NATIVELY when the depth stays under the
/// cap, byte-identical to the interpreter, and the i64 result wraps back to `Bool`.
#[cfg(feature = "native-jit")]
#[test]
fn native_mutual_recursion_bool_runs_native() {
    let source = "\
fn is_even(n: Int) -> Bool {
    if n == 0 { return true }
    return is_odd(n: n - 1)
}
fn is_odd(n: Int) -> Bool {
    if n == 0 { return false }
    return is_even(n: n - 1)
}
fn main() -> Unit {
    Log.write(message: read String.from_bool(value: is_even(n: 20)))
    Log.write(message: read String.from_bool(value: is_odd(n: 20)))
    return Unit
}
";
    let interp = common::run_vm_source("native-mutual-bool.rss", source, &[]).expect("interp");
    let exe = common::compile_vm_source("native-mutual-bool.rss", source).expect("compile");
    let (out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert_eq!(out.stdout.trim_end(), "true\nfalse");
    assert_eq!(
        interp.stdout, out.stdout,
        "Bool mutual recursion native must match interpreter"
    );
    assert!(
        stats.native_calls > 0,
        "shallow Bool mutual recursion should run natively: {stats:?}",
    );
}

/// Phase 1 (self-recursion widened to i64 scalar kinds): a Bool-returning *self*-
/// recursive function now runs on the native `CallSelf` fast path (and the tier-0
/// i64 executor on a depth-cap bail), with the i64 result wrapped back to `Bool` —
/// where before only Int self-recursion was admitted (Bool declined to the
/// interpreter). Byte-identical to the interpreter, and `native_calls > 0`.
#[cfg(feature = "native-jit")]
#[test]
fn native_self_recursion_bool_runs_native() {
    let source = "\
fn even_down(n: Int) -> Bool {
    if n == 0 { return true }
    if n == 1 { return false }
    return even_down(n: n - 2)
}
fn main() -> Unit {
    Log.write(message: read String.from_bool(value: even_down(n: 20)))
    Log.write(message: read String.from_bool(value: even_down(n: 7)))
    return Unit
}
";
    let interp = common::run_vm_source("native-self-bool.rss", source, &[]).expect("interp");
    let exe = common::compile_vm_source("native-self-bool.rss", source).expect("compile");
    let (out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert_eq!(out.stdout.trim_end(), "true\nfalse");
    assert_eq!(
        interp.stdout, out.stdout,
        "Bool self-recursion native must match interpreter"
    );
    assert!(
        stats.native_calls > 0,
        "Bool self-recursion should run via the native CallSelf fast path: {stats:?}",
    );
}

/// Phase 2 (recursion eligibility unified with the general native subset): a
/// FLOAT-returning self-recursive function now runs on the native `CallSelf` fast
/// path — its Float params/return marshal via `to_bits`/`from_bits` and its body
/// uses native float arithmetic. Before Phase 2 the bespoke Int-arith-only
/// recursion analysis rejected any Float body, so this ran on the interpreter.
/// Output (float-formatted) must be byte-identical to the interpreter, and
/// `native_calls > 0`.
#[cfg(feature = "native-jit")]
#[test]
fn native_self_recursion_float_runs_native() {
    let source = "\
fn fpow(base: Float, n: Int) -> Float {
    if n <= 0 { return 1.0 }
    return base * fpow(base: base, n: n - 1)
}
fn main() -> Unit {
    Log.write(message: read Float.to_string(value: read fpow(base: 2.0, n: 10)))
    Log.write(message: read Float.to_string(value: read fpow(base: 1.5, n: 4)))
    return Unit
}
";
    let interp = common::run_vm_source("native-self-float.rss", source, &[]).expect("interp");
    let exe = common::compile_vm_source("native-self-float.rss", source).expect("compile");
    let (out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert_eq!(
        interp.stdout, out.stdout,
        "Float self-recursion native must be byte-identical to the interpreter \
         (including float formatting)"
    );
    assert!(
        stats.native_calls > 0,
        "Float self-recursion should run via the native CallSelf fast path now that \
         recursion uses the general native subset: {stats:?}",
    );
}

