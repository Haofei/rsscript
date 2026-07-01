//! Fast JIT acceptance matrix for local VM/JIT work.
//!
//! These tests intentionally stay in the `runtime` target and use only the
//! in-process backend set: interpreter, tier-0 JIT, and, with `native-jit`,
//! native plus deopt/OSR stress backends. The slower generated-Rust backend
//! remains covered by the `differential` target.

mod common;

use common::differential::{assert_backends_agree_on, fast_backends};

fn assert_fast_jit_backends_agree(file: &str, source: &str) {
    assert_backends_agree_on(file, source, &[], &fast_backends());
}

#[cfg(feature = "native-jit")]
#[test]
fn native_jit_fast_backends_include_deopt_every_safepoint() {
    let names: Vec<&'static str> = fast_backends()
        .into_iter()
        .map(|backend| backend.name())
        .collect();
    assert!(
        names.contains(&"vm-jit-native-force-all-safepoints"),
        "native differential backend set must include deopt-every-safepoint stress; got {names:?}",
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_jit_precompiles_cold_scalar_call_chains() {
    let source = "\
fn inc(n: Int) -> Int {
    return n + 1
}

fn twice(n: Int) -> Int {
    return inc(n: read n) + inc(n: read n)
}

fn caller(n: Int) -> Int {
    return twice(n: read n) + twice(n: read n)
}

fn main() -> Unit {
    let value = caller(n: read 10)
    Log.write(message: read String.from_int(value: read value))
    return Unit
}
";
    let executable =
        rsscript::reg_vm_compile_source("native-call-edge.rss", source).expect("source compiles");
    let (output, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
        .expect("native eval succeeds");

    assert_eq!(output.stdout, "44\n");
    assert!(
        stats.native_call_edges >= 2,
        "expected caller chain to compile with native call edges, stats={stats:?}"
    );
    assert!(
        stats.native_call_depth_max >= 2,
        "expected caller chain to report nested native call depth, stats={stats:?}"
    );

    let (_reported_output, reported_stats, lines) =
        rsscript::reg_vm_eval_source_main_native_osr_report(
            "native-call-edge-report.rss",
            source,
            std::iter::empty::<&str>(),
        )
        .expect("reported native eval succeeds");
    assert!(
        reported_stats.native_call_edges >= 2 && reported_stats.native_call_depth_max >= 2,
        "reported run should compile the native call chain, stats={reported_stats:?}"
    );
    let report = lines.join("\n");
    assert!(
        report.contains("jit-report: summary")
            && report.contains(&format!(
                "native_call_edges={}",
                reported_stats.native_call_edges
            ))
            && report.contains(&format!(
                "native_call_depth_max={}",
                reported_stats.native_call_depth_max
            )),
        "RSS_JIT_REPORT should expose native call ABI telemetry, got:\n{report}"
    );
}

#[cfg(feature = "native-jit")]
#[test]
fn native_jit_precompiles_handle_return_call_chains() {
    let source = "\
fn make_text(value: Int) -> String {
    return String.from_int(value: value + 100)
}

fn text_len(value: Int) -> Int {
    let text = make_text(value: read value)
    return String.len(value: read text)
}

fn relay_text(value: Int) -> String {
    return make_text(value: read value)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: text_len(value: read 20)))
    Log.write(message: read relay_text(value: read 20))
    Log.write(message: read String.from_int(value: text_len(value: read 23)))
    Log.write(message: read relay_text(value: read 23))
    return Unit
}
";
    let executable = rsscript::reg_vm_compile_source("native-call-handle-edge.rss", source)
        .expect("source compiles");
    let (output, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<&str>())
        .expect("native eval succeeds");

    assert_eq!(output.stdout, "3\n120\n3\n123\n");
    assert!(
        stats.native_calls > 0,
        "expected native tier to execute at least one function, stats={stats:?}"
    );
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

/// Phase 3 (Float write-side): a struct `Float` field mutated through a `mut`
/// parameter (which escapes, so it is NOT scalar-replaced) lowers to the new
/// `FieldSetFloat` host helper — the write-side counterpart of the existing
/// `FieldFloat` read. All fast JIT backends (incl. native + deopt-stress) must
/// agree with the interpreter, and the loop must actually run natively.
#[cfg(feature = "native-jit")]
#[test]
fn jit_acceptance_runs_float_field_write_helper() {
    let source = "\
struct Acc {
    total: Float
}

fn bump(a: mut Acc, n: Int) -> Float {
    let mut i = 0
    while i < n {
        a.total = a.total + 1.5
        i = i + 1
    }
    return a.total
}

fn main() -> Unit {
    let mut a = Acc(total: 0.0)
    let r = bump(a: mut a, n: read 100)
    Log.write(message: read String.from_float(value: read r))
    return Unit
}
";
    let file = "jit-accept-float-field-write.rss";
    assert_fast_jit_backends_agree(file, source);
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (_out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert!(
        stats.native_calls > 0,
        "the Float field-write loop should run natively via FieldSetFloat: {stats:?}",
    );
}

/// Phase 4 (collection element-type breadth): pushing a `Float` onto a
/// `List<Float>` through a `mut` parameter lowers to the new `ListPushFloat` host
/// helper (write-side counterpart of the existing `ListGetFloat`), enabled by the
/// generalized float-arg helper ABI. All fast JIT backends must agree with the
/// interpreter and the loop must run natively.
#[cfg(feature = "native-jit")]
#[test]
fn jit_acceptance_runs_float_list_push_helper() {
    let source = "\
fn fill(xs: mut List<Float>, n: Int) -> Int {
    let mut i = 0
    while i < n {
        List.push<Float>(list: mut xs, value: read 1.5)
        i = i + 1
    }
    return 0
}

fn main() -> Unit {
    let mut xs = List<Float>.new()
    let r = fill(xs: mut xs, n: read 10)
    let mut total = 0.0
    let mut j = 0
    while j < List.len<Float>(list: read xs) {
        total = total + List.get<Float>(list: read xs, index: j)
        j = j + 1
    }
    Log.write(message: read String.from_float(value: read total))
    return Unit
}
";
    let file = "jit-accept-float-list-push.rss";
    assert_fast_jit_backends_agree(file, source);
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (_out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert!(
        stats.native_calls > 0,
        "the Float list-push loop should run natively via ListPushFloat: {stats:?}",
    );
}

/// TV2 mutable-flat-float: an in-place `List.set<Float>` loop over a `mut
/// List<Float>` param classifies the param as `FlatFloatMut` and lowers the write
/// to `ListSetFloatDirect` (direct f64 buffer store, bounds-checked) — the write
/// side of the existing `ListGetFloatDirect`. Backends must agree with the
/// interpreter and the loop must run natively.
#[cfg(feature = "native-jit")]
#[test]
fn jit_acceptance_runs_flat_float_in_place_set() {
    let source = "\
fn scale(xs: mut List<Float>, n: Int) -> Int {
    let mut i = 0
    while i < n {
        let v = List.get<Float>(list: read xs, index: i)
        let doubled = v * 2.0
        List.set<Float>(list: mut xs, index: i, value: read doubled)
        i = i + 1
    }
    return 0
}

fn main() -> Unit {
    Log.write(message: read \"begin\")
    let mut xs = List<Float>.new()
    let mut k = 0
    while k < 200 {
        List.push<Float>(list: mut xs, value: read 1.5)
        k = k + 1
    }
    let r = scale(xs: mut xs, n: read 200)
    let mut total = 0.0
    let mut j = 0
    while j < List.len<Float>(list: read xs) {
        total = total + List.get<Float>(list: read xs, index: j)
        j = j + 1
    }
    Log.write(message: read Float.to_string(value: read total))
    return Unit
}
";
    let file = "jit-accept-flat-float-set.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "in-place Float list-set must be byte-identical to the interpreter"
    );
    assert!(
        stats.osr_entries > 0,
        "the hot flat Float list-set loop should OSR natively via ListSetFloatDirect: {stats:?}",
    );
}

/// `List.is_empty<Int>` on a `read List<Int>` param directizes to `ListIsEmptyDirect`
/// (a `lens`-slot length read compared to zero — no per-iteration host call). The
/// typed call lets flat classification pin the param even though it is used ONLY via
/// is_empty, so the function reaches the native tier instead of declining. Must be
/// byte-identical across every backend.
#[cfg(feature = "native-jit")]
#[test]
fn native_jit_directizes_flat_list_is_empty() {
    let source = "\
fn count_nonempty(xs: read List<Int>, reps: Int) -> Int {
    let mut i = 0
    let mut total = 0
    while i < reps {
        if List.is_empty<Int>(list: read xs) {
            total = total + 1
        } else {
            total = total + 2
        }
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    let mut xs = List<Int>.new()
    List.push<Int>(list: mut xs, value: read 1)
    List.push<Int>(list: mut xs, value: read 2)
    Log.write(message: read String.from_int(value: count_nonempty(xs: read xs, reps: read 5)))
    return Unit
}
";
    let file = "jit-accept-flat-is-empty.rss";
    // Correctness across interp / tier-0 / native / deopt-every-safepoint backends.
    assert_fast_jit_backends_agree(file, source);
    // Directization: the is_empty-only flat param reaches the native tier (a host
    // call or a decline would still agree, so also assert it compiled).
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (output, stats) = executable
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native eval succeeds");
    assert_eq!(output.stdout, "10\n", "5 reps × 2 (xs non-empty) = 10");
    assert!(
        stats.compiled > 0,
        "the flat List.is_empty function should compile native, not decline: {stats:?}"
    );
}

/// Float-value map read: `Map.get` on a `Map<Int, Float>` fused with its Option
/// match lowers to the new `MatchMapGetFloat` JitInstr (f64 value channel + the
/// shared found-flag), the read-side counterpart of `MatchMapGetInt`. The lookup
/// itself is the interpreter's `map.get`; native only marshals the f64 payload.
/// Backends must agree with the interpreter and the loop must run natively.
#[cfg(feature = "native-jit")]
#[test]
fn jit_acceptance_runs_float_map_get_match() {
    let source = "\
fn sum_lookups(m: read Map<Int, Float>, n: Int) -> Float {
    let mut total = 0.0
    let mut i = 0
    while i < n {
        match Map.get<Int, Float>(map: read m, key: read i) {
            Some(v) => { total = total + v }
            None => { total = total }
        }
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    let mut m = Map<Int, Float>.new()
    Map.insert<Int, Float>(map: mut m, key: read 0, value: read 1.5)
    Map.insert<Int, Float>(map: mut m, key: read 1, value: read 2.5)
    Map.insert<Int, Float>(map: mut m, key: read 2, value: read 3.5)
    Log.write(message: read Float.to_string(value: read sum_lookups(m: read m, n: read 3)))
    return Unit
}
";
    let file = "jit-accept-float-map-get.rss";
    assert_fast_jit_backends_agree(file, source);
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (_out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert!(
        stats.native_calls > 0,
        "the Float map-get-match loop should run natively via MatchMapGetFloat: {stats:?}",
    );
}

/// Phase 4 (collection element-type breadth): inserting a `Float` value into a
/// `Map<Int, Float>` and pushing a `Float` onto a `Deque<Float>`, both through `mut`
/// parameters, lower to the new `MapInsertFloat` / `DequePushBackFloat` helpers
/// (value-side counterparts enabled by the float-arg helper ABI). Backends must
/// agree with the interpreter and the loop must run natively.
#[cfg(feature = "native-jit")]
#[test]
fn jit_acceptance_runs_float_map_and_deque_helpers() {
    let source = "\
fn fill(m: mut Map<Int, Float>, d: mut Deque<Float>, n: Int) -> Int {
    let mut i = 0
    while i < n {
        Map.insert<Int, Float>(map: mut m, key: read i, value: read 1.5)
        Deque.push_back<Float>(deque: mut d, value: read 2.5)
        i = i + 1
    }
    return 0
}

fn main() -> Unit {
    let mut m = Map<Int, Float>.new()
    let mut d = Deque<Float>.new()
    let r = fill(m: mut m, d: mut d, n: read 4)
    let mut total = 0.0
    match Map.get<Int, Float>(map: read m, key: read 2) {
        Some(v) => { total = total + v }
        None => { total = total }
    }
    match Deque.pop_front<Float>(deque: mut d) {
        Some(v) => { total = total + v }
        None => { total = total }
    }
    Log.write(message: read Float.to_string(value: read total))
    return Unit
}
";
    let file = "jit-accept-float-map-deque.rss";
    assert_fast_jit_backends_agree(file, source);
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (_out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert!(
        stats.native_calls > 0,
        "the Float map-insert / deque-push loop should run natively: {stats:?}",
    );
}

/// Float-value sorted-map read: `SortedMap.get` on a `SortedMap<Int, Float>` fused
/// with its Option match lowers to `MatchSortedMapGetFloat` (f64 value channel +
/// shared sorted-map found flag; binary search is the interpreter's). Backends must
/// agree with the interpreter and the loop must run natively.
#[cfg(feature = "native-jit")]
#[test]
fn jit_acceptance_runs_float_sorted_map_get_match() {
    let source = "\
fn sum_lookups(m: read SortedMap<Int, Float>, n: Int) -> Float {
    let mut total = 0.0
    let mut i = 0
    while i < n {
        match SortedMap.get<Int, Float>(map: read m, key: read i) {
            Some(v) => { total = total + v }
            None => { total = total }
        }
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    let mut m = SortedMap<Int, Float>.new()
    SortedMap.insert<Int, Float>(map: mut m, key: read 0, value: read 1.5)
    SortedMap.insert<Int, Float>(map: mut m, key: read 1, value: read 2.5)
    SortedMap.insert<Int, Float>(map: mut m, key: read 2, value: read 3.5)
    Log.write(message: read Float.to_string(value: read sum_lookups(m: read m, n: read 3)))
    return Unit
}
";
    let file = "jit-accept-float-sorted-map-get.rss";
    assert_fast_jit_backends_agree(file, source);
    let exe = rsscript::reg_vm_compile_source(file, source).expect("compile");
    let (_out, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert!(
        stats.native_calls > 0,
        "the Float sorted-map-get loop should run natively via MatchSortedMapGetFloat: {stats:?}",
    );
}

/// Float deque pop: `Deque.pop_front<Float>` lowers to the `DequePopFrontFloat`
/// helper (f64 value channel; empty deque bails to the interpreter's None path) — the
/// value-side mirror of `DequePopFrontInt`. This is a PARITY test: native
/// reachability of `match Deque.pop` is currently blocked for BOTH Int and Float by
/// a pre-existing, type-agnostic limitation — J3 Option scalar-replacement seeds only
/// from `MakeSome`/`LoadNone`, not from the deque pop's directly-produced `Option`.
/// The helper is verified correct here against the interpreter and is ready the
/// moment that shared fusion lands. (See also the Int form, equally not-yet-native.)
#[cfg(feature = "native-jit")]
#[test]
fn jit_acceptance_runs_float_deque_pop() {
    let source = "\
fn drain(d: mut Deque<Float>, n: Int) -> Float {
    let mut total = 0.0
    let mut i = 0
    while i < n {
        match Deque.pop_front<Float>(deque: mut d) {
            Some(v) => { total = total + v }
            None => { total = total }
        }
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    let mut d = Deque<Float>.new()
    Deque.push_back<Float>(deque: mut d, value: read 1.5)
    Deque.push_back<Float>(deque: mut d, value: read 2.5)
    Deque.push_back<Float>(deque: mut d, value: read 3.5)
    Log.write(message: read Float.to_string(value: read drain(d: mut d, n: read 3)))
    return Unit
}
";
    let file = "jit-accept-float-deque-pop.rss";
    // Parity across all fast JIT backends (incl. native + deopt-stress). The
    // DequePopFrontFloat helper is exercised wherever the deque pop reaches native;
    // correctness (incl. the empty→None bail) is the guarantee asserted here.
    assert_fast_jit_backends_agree(file, source);
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
    let precise = rsscript::reg_vm_eval_source_main_native_precise(
        file,
        source,
        std::iter::empty::<String>(),
    )
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
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
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
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR read-heap loop must be byte-identical to the interpreter (stdout)"
    );
    // 3+1+4+1+5+9+2+6 = 31.
    assert_eq!(osr.stdout.trim_end(), "begin\n31\n31");
}

/// Nested-loop OSR (roadmap #4), all-native variant: an OUTER accumulator loop with
/// an INNER counting loop, wrapped by non-native I/O so the function is whole-native
/// ineligible and only OSR can run a loop natively. The OSR pipeline is multi-loop-
/// aware (`detect_natural_loops` → `select_osr_candidate_loop`): with both loops in
/// the native subset the OUTER region out-scores the inner on length, so the whole
/// nested `[outer-header, outer-exit)` region compiles as ONE native loop with the
/// inner backedge as an in-region jump. Byte parity with the interpreter is the
/// correctness net (a wrong ip-map over the nested CFG would diverge here).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_nested_inner_loop_matches_interpreter() {
    let source = "\
fn nested_sum(rows: Int, cols: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut total = 0
    let mut i = 0
    while i < rows {
        let mut j = 0
        while j < cols {
            let idx = i * cols + j
            total = total + idx
            j = j + 1
        }
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: nested_sum(rows: read 40, cols: read 40)))
    return Unit
}
";
    let file = "jit-osr-nested.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "nested-loop OSR must be byte-identical to the interpreter (stdout)"
    );
    // i*cols+j over i in 0..40, j in 0..40 is a bijection onto 0..1599, so the total
    // is sum(0..=1599) = 1599*1600/2 = 1279200.
    assert_eq!(osr.stdout.trim_end(), "begin\n1279200\n1279200");

    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (_osr2, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run (stats)");
    assert!(
        stats.osr_entries > 0,
        "the nested hot loop must OSR natively: {stats:?}",
    );
}

/// Nested-loop OSR (roadmap #4), DIRTY-outer variant: the outer loop body itself
/// does non-native I/O (`Log.write`), so the outer region fails the transform-
/// candidate filter and `select_osr_candidate_loop` picks the INNER loop. This
/// exercises the re-entrant case — the inner loop is OSR-entered, exits back to the
/// interpreted outer body each outer iteration, and re-enters native on the next
/// outer trip. Parity proves the repeated OSR entry/exit boundary stays correct.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_nested_inner_loop_with_dirty_outer_matches_interpreter() {
    let source = "\
fn nested_sum(rows: Int, cols: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut total = 0
    let mut i = 0
    while i < rows {
        let mut j = 0
        while j < cols {
            let idx = i * cols + j
            total = total + idx
            j = j + 1
        }
        Log.write(message: read String.from_int(value: i))
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: nested_sum(rows: read 3, cols: read 200)))
    return Unit
}
";
    let file = "jit-osr-nested-dirty.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>())
            .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "dirty-outer nested-loop OSR must be byte-identical to the interpreter (stdout)"
    );
    // Per outer i: sum_{j<200}(i*200+j) = 200*200*i + sum(0..=199) = 40000*i + 19900.
    // i=0,1,2 ⇒ 19900 + 59900 + 99900 = 179700. The outer prints i each trip.
    assert_eq!(osr.stdout.trim_end(), "begin\n0\n1\n2\n179700\n179700");

    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (_osr2, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run (stats)");
    assert!(
        stats.osr_entries > 0,
        "the inner hot loop must OSR natively even with a dirty (I/O) outer body: {stats:?}",
    );
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
/// `native_scalar_replace_options_in_region` is that a scalar-replaced `Option` that
/// is live after the loop may only OSR when it can be RECONSTRUCTED at OSR-exit
/// (J0.1(b): always-`Some` with an unconditional scalar def — see
/// `native_osr_j3_live_after_always_some_option_reconstructs`). Here `o` is
/// CONDITIONALLY `None` (a `None` outcome inside the loop), so it is NOT always-`Some`
/// and has no scalar payload to reconstruct on the `None` path: the pass MUST refuse
/// to scalar-replace it and therefore MUST NOT OSR (`osr_entries == 0`), or the
/// interpreter would read a stale `o` slot. We assert both that the program is still
/// correct (the interpreter runs the whole loop, no OSR) and that OSR genuinely
/// bailed. This protects the dead/None-arm-not-reconstructible boundary case against a
/// future edit that accidentally makes a non-reconstructible escaping Option OSR.
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
        if i < 5 {
            o = None
        } else {
            o = Some(i)
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
    // i<5 ⇒ o=None (total += 0); i in 5..60 ⇒ total += i = sum(5..=59) = 1760.
    // o after the loop = Some(59) (last iteration, i=59 >= 5) ⇒ "59".
    assert_eq!(osr.stdout.trim_end(), "begin\n59\n1760\n1760");
    assert_eq!(
        stats.osr_entries, 0,
        "a loop whose Option is conditionally None (not reconstructible) and live after \
         the loop must NOT OSR: {stats:?}",
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

/// OSR × J3 LOOP-CARRIED struct positive test. A non-escaping struct `p` is created
/// BEFORE the hot loop, MUTATED IN PLACE across iterations (`p.x = p.x + p.vx`, …, a
/// per-iteration `SetFieldSlot` heap write the native tier cannot perform), and DEAD
/// after the loop. The shipped loop-LOCAL struct pass cannot help (the `MakeStruct`
/// is in the pre-header, and the in-loop `SetFieldSlot` is neither native-subset nor
/// `MakeStruct`). `native_loop_carried_struct_in_region` dissolves `p` into one
/// loop-carried scalar register per field (the init sources become the live-in
/// leaves; each in-loop field read/write becomes a register Move), so the loop OSRs to
/// a pure-scalar native loop. The body is I/O-tangled (`Log.write` makes `f`
/// native-INELIGIBLE) so only OSR can run it natively. Output must be byte-identical
/// to the pure interpreter (which mutates the heap struct each iteration) AND the loop
/// must genuinely OSR (`osr_entries > 0`).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_loop_carried_struct_loop_matches_interpreter() {
    let source = "\
struct Particle {
    x: Int,
    y: Int,
    vx: Int,
    vy: Int
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut p = Particle(x: 0, y: 0, vx: 1, vy: 2)
    let mut i = 0
    let mut total = 0
    while i < limit {
        p.x = p.x + p.vx
        p.y = p.y + p.vy
        p.vx = p.vx + 1
        p.vy = p.vy - 1
        total = total + p.x + p.y
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
    let file = "jit-osr-j3-loop-carried-struct.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "OSR × J3 loop-carried struct mutation must be byte-identical to the interpreter"
    );
    assert!(
        stats.osr_entries > 0,
        "the non-escaping loop-carried struct (dead after loop) must OSR natively after \
         loop-carried scalar replacement: {stats:?}",
    );
}

/// OSR × native heap writes for a loop-carried struct that is LIVE after the loop.
/// The scalar loop-carried-struct pass must not dissolve this shape into registers
/// because `p.x` is read after the loop and would require heap reconstruction. The
/// native heap-write layer can still compile the original `SetFieldSlot` loop by
/// journaling the heap writes directly, so the program should remain interpreter-
/// identical AND OSR natively.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_loop_carried_struct_live_after_loop_uses_heap_writes() {
    let source = "\
struct Particle {
    x: Int,
    y: Int,
    vx: Int,
    vy: Int
}

fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut p = Particle(x: 0, y: 0, vx: 1, vy: 2)
    let mut i = 0
    let mut total = 0
    while i < limit {
        p.x = p.x + p.vx
        p.y = p.y + p.vy
        p.vx = p.vx + 1
        p.vy = p.vy - 1
        total = total + p.x + p.y
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
    let file = "jit-osr-j3-loop-carried-struct-live.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "a loop-carried struct read after the loop must still be interpreter-identical"
    );
    assert!(
        stats.osr_entries > 0,
        "a loop-carried struct LIVE after the loop should OSR through native heap writes: {stats:?}",
    );
}

/// OSR × native heap writes for a loop-carried struct that ESCAPES. The scalar
/// loop-carried-struct pass must not dissolve the returned heap object into dead
/// registers, but the native heap-write path can still update the original heap
/// struct directly. The program should remain interpreter-identical AND OSR natively.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_loop_carried_struct_escaping_uses_heap_writes() {
    let source = "\
struct Particle {
    x: Int,
    y: Int,
    vx: Int,
    vy: Int
}

fn f(limit: Int) -> Particle {
    Log.write(message: read \"begin\")
    let mut p = Particle(x: 0, y: 0, vx: 1, vy: 2)
    let mut i = 0
    while i < limit {
        p.x = p.x + p.vx
        p.y = p.y + p.vy
        p.vx = p.vx + 1
        p.vy = p.vy - 1
        i = i + 1
    }
    return p
}

fn main() -> Unit {
    let q = f(limit: read 60)
    Log.write(message: read String.from_int(value: read q.x))
    return Unit
}
";
    let file = "jit-osr-j3-loop-carried-struct-escaping.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "an escaping (returned) loop-carried struct must still be interpreter-identical"
    );
    assert!(
        stats.osr_entries > 0,
        "a loop-carried struct that ESCAPES should OSR through native heap writes: {stats:?}",
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
    while i < 400 {
        let a: Fn(Int) -> Int = |x| { return x * 2 - 1 }
        total = total + dispatch(f: read a, x: read i)
        i = i + 1
    }
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    assert_fast_jit_backends_agree("jit-accept-mono-closure-inline.rss", source);
    #[cfg(feature = "native-jit")]
    {
        let executable =
            rsscript::reg_vm_compile_source("jit-accept-mono-closure-inline.rss", source)
                .expect("source compiles");
        let (_native, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
            .expect("native JIT run should succeed");
        assert!(
            stats.profile_closure_guard_sites > 0,
            "monomorphic closure inline must emit a profile-guided guard: {stats:?}",
        );
    }
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
    // Warm phase: 400 calls, all closure A (x * 2). Past PROFILE_RECORD_LIMIT, so
    // the dispatch site freezes monomorphic on A and is inlined+compiled to native.
    while i < 400 {
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
        let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    while i < 400 {
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
    while i < 400 {
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

    // The cost model (default `enforce`) declines this loop-free polymorphic PIC as
    // native ≈ interpreter; disable it here so the PIC mechanism compiles and its
    // telemetry can be verified. (The differential above already passed under enforce
    // because declining is correctness-safe.)
    #[cfg(feature = "native-jit")]
    rsscript::with_native_cost_model_disabled(|| {
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
        assert!(
            stats.profile_closure_id_reads > 0,
            "polymorphic closure inline must emit a profile-guided closure-id dispatch: {stats:?}",
        );
        assert!(
            stats.profile_closure_pic_sites > 0,
            "polymorphic closure inline must report PIC sites: {stats:?}",
        );
        assert!(
            stats.profile_closure_pic_arms >= 3,
            "three-way polymorphic closure inline must report at least three PIC arms: {stats:?}",
        );
        assert_eq!(
            stats.compile_failed, 0,
            "native compilation must not fail: {stats:?}",
        );
    });
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
    while i < 400 {
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

    // Disable the cost model (default `enforce` declines this loop-free PIC) so the
    // inline-cache + bail mechanism compiles and its telemetry is verifiable.
    #[cfg(feature = "native-jit")]
    rsscript::with_native_cost_model_disabled(|| {
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
        assert!(
            stats.profile_closure_id_reads > 0,
            "polymorphic closure inline must emit a profile-guided closure-id dispatch: {stats:?}",
        );
        assert_eq!(
            stats.compile_failed, 0,
            "native compilation must not fail: {stats:?}",
        );
    });
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
/// Fixed medium-loop kernel whose trip count (600) sits BELOW the default 1000:
/// at the default threshold the counting auto-trigger must NOT fire
/// (`osr_entries == 0`), but with the override lowered to 100 the SAME loop must
/// OSR (`osr_entries > 0`). Deterministic: trip count, kernel, and thresholds
/// are all fixed.
#[cfg(feature = "native-jit")]
#[test]
fn rss_jit_osr_threshold_env_overrides_auto_trigger_fire_point() {
    // Native-INELIGIBLE function (Log.write tangled around it) with one hot scalar
    // loop of 600 iterations: only OSR can run the loop natively.
    let source = "\
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
";

    let osr_entries = |threshold: Option<&str>| -> u64 {
        let _osr_guard = EnvGuard::remove("RSS_JIT_OSR");
        let _threshold_guard = EnvGuard::set_or_remove("RSS_JIT_OSR_THRESHOLD", threshold);
        let executable = rsscript::reg_vm_compile_source("rss-osr-threshold-probe.rss", source)
            .expect("threshold probe compiles");
        let (_output, stats) = executable
            .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
            .expect("native run with stats");
        stats.osr_entries
    };

    struct EnvGuard {
        key: &'static str,
        old: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn remove(key: &'static str) -> Self {
            let old = std::env::var_os(key);
            unsafe {
                std::env::remove_var(key);
            }
            Self { key, old }
        }

        fn set_or_remove(key: &'static str, value: Option<&str>) -> Self {
            let old = std::env::var_os(key);
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
            Self { key, old }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match self.old.take() {
                    Some(value) => std::env::set_var(self.key, value),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    // Default threshold (1000): trip=600 < 1000 ⇒ counting auto-trigger never fires.
    let default_entries = osr_entries(None);
    // Override lowered to 100: trip=600 > 100 ⇒ the same loop now OSRs.
    let lowered_entries = osr_entries(Some("100"));

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

/// OSR × J3 — J0.1(b) live-after always-`Ok` Result RECONSTRUCTION (POSITIVE). Unlike
/// `native_osr_j3_escaping_result_does_not_osr` (whose `r` is built via a branchy
/// inlined `checked`, so the `Ok` def is NOT unconditionally reached and the pass
/// still declines), here `r = Ok(i)` is a single, unconditionally-reached in-region
/// definition with a SCALAR (`Int`) payload, and `r` is read AFTER the loop. The
/// RESULT-SR pass dissolves `r` to a scalar payload register and records a
/// reconstruction recipe; at OSR-exit the loop rebuilds `Ok(last i)` into `r`'s slot
/// so the post-loop `match r` is interpreter-identical. This is the J0.1(b) slice:
/// a previously-declining live-after Result now OSRs. Because `r`'s value is observed
/// after the loop, a wrong reconstruction diverges here (and on the differential).
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_live_after_always_ok_result_reconstructs() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut r: Result<Int, String> = Ok(0)
    while i < limit {
        r = Ok(i)
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
    let file = "jit-osr-j3-live-after-ok-result.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "live-after Result reconstruction must be byte-identical to the interpreter"
    );
    // i in 0..60: total = 0+...+59 = 1770; r = Ok(59) after the loop ⇒ "59".
    assert_eq!(osr.stdout.trim_end(), "begin\n59\n1770\n1770");
    assert!(
        stats.osr_entries > 0,
        "a live-after always-Ok Result with an unconditional scalar def must OSR via \
         reconstruction (J0.1(b)): {stats:?}",
    );
}

/// OSR × J3 — J0.1(b) live-after always-`Some` Option RECONSTRUCTION (POSITIVE), the
/// Option analog of the Result reconstruction above. `o = Some(i)` is a single,
/// unconditionally-reached in-region `MakeSome` (always-`Some`, scalar payload) and
/// `o` is read AFTER the loop. The OPTION-SR pass records a recipe; at OSR-exit the
/// loop rebuilds `Some(last i)` into `o`'s slot so the post-loop `match o` is
/// interpreter-identical. Contrast `native_osr_j3_escaping_option_does_not_osr`,
/// whose `o` is conditionally `Some`/`None` (it has a `None` outcome) and so still
/// declines.
#[cfg(feature = "native-jit")]
#[test]
fn native_osr_j3_live_after_always_some_option_reconstructs() {
    let source = "\
fn f(limit: Int) -> Int {
    Log.write(message: read \"begin\")
    let mut i = 0
    let mut total = 0
    let mut o: Option<Int> = Some(0)
    while i < limit {
        o = Some(i)
        match o {
            Some(v) => { total = total + v }
            None => { total = total + 0 }
        }
        i = i + 1
    }
    match o {
        Some(v) => { Log.write(message: read String.from_int(value: v)) }
        None => { Log.write(message: read \"none\") }
    }
    Log.write(message: read String.from_int(value: total))
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: f(limit: read 60)))
    return Unit
}
";
    let file = "jit-osr-j3-live-after-some-option.rss";
    let interp = common::run_vm_source(file, source, &[]).expect("interp run");
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let (osr, stats) = executable
        .eval_main_with_args_native_osr_with_stats(std::iter::empty::<String>())
        .expect("osr native run");
    assert_eq!(
        interp.stdout, osr.stdout,
        "live-after Option reconstruction must be byte-identical to the interpreter"
    );
    // i in 0..60: total = 1770; o = Some(59) after the loop ⇒ "59".
    assert_eq!(osr.stdout.trim_end(), "begin\n59\n1770\n1770");
    assert!(
        stats.osr_entries > 0,
        "a live-after always-Some Option with an unconditional scalar def must OSR via \
         reconstruction (J0.1(b)): {stats:?}",
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let err = rsscript::reg_vm_eval_source_main_with_args(
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
        let (out, stats, lines) = rsscript::reg_vm_eval_source_main_native_osr_report(
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
    let (out, stats, lines) = rsscript::reg_vm_eval_source_main_native_osr_report(
        file,
        source,
        std::iter::empty::<String>(),
    )
    .expect("report run");
    assert_eq!(interp.stdout, out.stdout, "decline must stay byte-identical");
    assert!(
        stats.unprofitable_declines > 0,
        "the PIC must be declined under the default cost model: {stats:?}",
    );
    let report = lines.join("\n");
    // The cost-model decline summary (built from this run's telemetry) shows the PIC
    // decline with its score breakdown...
    assert!(
        report.contains("jit-report: cost-model decline summary")
            && report.contains("pic_sites=1"),
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
    let (out, stats, lines) = rsscript::reg_vm_eval_source_main_native_osr_report(
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
    let (out, stats, lines) = rsscript::reg_vm_eval_source_main_native_osr_report(
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
    let (out, stats, lines) = rsscript::reg_vm_eval_source_main_native_osr_report(
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
    let (out, stats, lines) = rsscript::reg_vm_eval_source_main_native_osr_report(
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
    let (out, _stats, lines) = rsscript::reg_vm_eval_source_main_native_osr_report(
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
    let (out, stats, lines) = rsscript::reg_vm_eval_source_main_native_osr_report(
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
    let (out, stats, lines) = rsscript::reg_vm_eval_source_main_native_osr_report(
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
features: local

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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
features: local

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
    let executable = rsscript::reg_vm_compile_source(file, source).expect("source compiles");
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
features: local

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
    rsscript::reg_vm_compile_source(file, source).expect("source compiles");
    let interp = common::run_vm_source(file, source, &[]);
    let osr =
        rsscript::reg_vm_eval_source_main_native_osr(file, source, std::iter::empty::<String>());
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
    let exe = rsscript::reg_vm_compile_source("native-fib.rss", source).expect("compile");
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
    let exe = rsscript::reg_vm_compile_source("native-deep-sum.rss", source).expect("compile");
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
    let exe = rsscript::reg_vm_compile_source("perf-fib.rss", source).expect("compile");
    let (_o, stats) = exe
        .eval_main_with_args_native_with_stats(std::iter::empty::<String>())
        .expect("native run");
    assert!(stats.native_calls > 0, "fib must run natively: {stats:?}");

    let best = |native: bool| -> std::time::Duration {
        let mut best = std::time::Duration::MAX;
        for _ in 0..3 {
            let exe = rsscript::reg_vm_compile_source("perf-fib.rss", source).expect("compile");
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
    let exe = rsscript::reg_vm_compile_source("native-mutual-float.rss", source).expect("compile");
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
    let exe = rsscript::reg_vm_compile_source("native-mutual.rss", source).expect("compile");
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
    let exe = rsscript::reg_vm_compile_source("native-mutual-bool.rss", source).expect("compile");
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
    let exe = rsscript::reg_vm_compile_source("native-self-bool.rss", source).expect("compile");
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
    let exe = rsscript::reg_vm_compile_source("native-self-float.rss", source).expect("compile");
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

// ---------------------------------------------------------------------------
// Recursive native fast paths must honor the same limit gate as `try_native`:
// Cranelift code polls neither `step_budget` nor `cancel` and allocates off the
// `mem_budget` meter, so with any of them armed the recursive paths must NOT
// dispatch natively (they run on the interpreter / tier-0, which `tick()`s).
// ---------------------------------------------------------------------------

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

/// J0.5 (in-generated-code `VmLimits`), POSITIVE: with `step_budget` armed but
/// generous, the hot loop OSRs into the armed native variant (`step_budget` ticked per
/// instruction, tested at the loop header) and still completes byte-identically to the
/// interpreter — proving the native step accounting does not falsely trip.
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
    assert!(
        stats.osr_entries > 0,
        "the hot loop must OSR natively under an armed step budget: {stats:?}",
    );
}

/// J0.5, ENFORCEMENT: the SAME OSR loop under a tight `step_budget` must trip cleanly —
/// the armed native variant accumulates the per-instruction tick count, the loop-header
/// test crosses the budget, native bails, and the interpreter (the sole limit
/// authority) raises the step-budget error. A native loop must not bypass the budget.
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

/// J0.5, CANCEL: with an ambient `cancel` flag set and OSR forced, the armed native
/// variant polls the flag at the loop header and bails to the interpreter, which raises
/// the cancellation error — a native loop must remain interruptible.
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

/// J0.5 mem (POSITIVE): a non-allocating hot loop (pure scalar arithmetic) now OSRs
/// even with `mem_budget` armed — it charges `mem_budget` exactly zero, identical to the
/// interpreter (which only accounts at allocation/growth sites), so running it natively
/// without in-code byte accounting is sound. Must match the interpreter byte-for-byte.
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
    assert!(
        stats.osr_entries > 0,
        "a non-allocating loop must OSR under an armed mem_budget: {stats:?}",
    );
}

/// J0.5 mem (PARITY): a `Map<String,Int>`-insert hot loop now RUNS natively under an
/// armed `mem_budget` — the interpreter charges map inserts ZERO bytes (its only
/// per-iteration `account_bytes` sites are `List.push`/`List.append` growth and
/// list/map LITERAL construction), so a native map-insert loop charges exactly the same
/// (zero): exact parity, no in-code accounting required. Must OSR and match the
/// interpreter byte-for-byte. (A `ListPush`-charging loop, by contrast, still declines —
/// but that case is already vetoed by OSR growth-admissibility for live-in lists.)
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
        .expect("map-insert loop must run under mem_budget (map inserts charge zero)");
    assert_eq!(
        interp.stdout, output.stdout,
        "the map-insert loop must stay interpreter-identical under mem_budget"
    );
    assert_eq!(output.stdout.trim_end(), "begin\n1");
    assert!(
        stats.osr_entries > 0,
        "a map-insert loop must OSR under mem_budget (zero-charge parity): {stats:?}",
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
    assert!(
        stats.osr_entries > 0,
        "list-push loop must OSR under mem_budget: {stats:?}"
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
