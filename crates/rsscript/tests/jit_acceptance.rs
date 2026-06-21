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
