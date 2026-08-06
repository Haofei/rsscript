//! Hostile-input robustness suite. Parsing and checking externally supplied
//! source must never panic and must fail closed on malformed input. Passing
//! these tests does not make the in-process VM an isolation boundary.

use proptest::prelude::*;
use rsscript::{CancellationToken, EvalError, VmLimits};

mod common;
use common::reg_vm_eval_source_main_with_limits;

/// Invariant 1 (runtime resilience): configured VM limits turn selected
/// resource-exhaustion cases into recoverable `EvalError::Runtime` values. They
/// do not replace process isolation for untrusted execution.
///
/// Helper: eval `source`'s `main` under `limits`, returning the result.
fn eval_limited(source: &str, limits: VmLimits) -> Result<rsscript::EvalOutput, EvalError> {
    reg_vm_eval_source_main_with_limits("hostile.rss", source, std::iter::empty::<String>(), limits)
}

/// A1: unbounded self-recursion hits the default-on depth cap and returns a
/// clean "recursion depth" error rather than overflowing the native stack.
#[test]
fn deep_recursion_returns_clean_error_not_crash() {
    // Bounded defaults catch self-recursion before the native stack overflows.
    let source = r#"
fn f(n: Int) -> Int {
    return f(n: n + 1)
}

fn main() -> Int {
    return f(n: 0)
}
"#;
    let err = eval_limited(source, VmLimits::default()).expect_err("must error, not crash");
    match err {
        EvalError::Runtime(msg) => assert!(
            msg.contains("recursion depth"),
            "expected recursion-depth error, got: {msg}"
        ),
        other => panic!("expected EvalError::Runtime, got {other:?}"),
    }
}

/// B3: an infinite loop with a step budget configured returns a clean "step
/// budget" error rather than hanging forever.
#[test]
fn infinite_loop_with_step_budget_returns_clean_error_not_hang() {
    let source = r#"
fn main() -> Int {
    let mut x = 0
    while true {
        x = x + 1
    }
    return x
}
"#;
    let limits = VmLimits {
        step_budget: Some(100_000),
        ..VmLimits::default()
    };
    let err = eval_limited(source, limits).expect_err("must error, not hang");
    match err {
        EvalError::Runtime(msg) => assert!(
            msg.contains("step budget"),
            "expected step-budget error, got: {msg}"
        ),
        other => panic!("expected EvalError::Runtime, got {other:?}"),
    }
}

/// B4: an unbounded allocation loop with a memory ceiling configured returns a
/// clean "memory limit" error rather than getting OOM-killed.
#[test]
fn runaway_allocation_with_memory_ceiling_returns_clean_error() {
    let source = r#"fn main() -> Int {
    let mut index = 0
    local values = List<Int>.new()
    while index < 100000000 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }
    return 0
}
"#;
    // 1 MiB ceiling: far below what the loop would allocate, so the push handler
    // trips long before the host runs out of memory. A generous step budget is
    // also set so the test fails loudly if memory accounting ever regresses
    // (otherwise an unbounded loop would just hang).
    let limits = VmLimits {
        mem_budget: Some(1 << 20),
        step_budget: Some(50_000_000),
        ..VmLimits::default()
    };
    let err = eval_limited(source, limits).expect_err("must error, not OOM");
    match err {
        EvalError::Runtime(msg) => assert!(
            msg.contains("memory limit"),
            "expected memory-limit error, got: {msg}"
        ),
        other => panic!("expected EvalError::Runtime, got {other:?}"),
    }
}

#[test]
fn map_capacity_growth_is_charged_to_memory_budget() {
    let source = r#"
fn main() -> Int {
    let map = Map.new<Int, Int>()
    let mut index = 0
    while index < 1000000 {
        Map.insert<Int, Int>(map: mut map, key: index, value: index)
        index = index + 1
    }
    return Map.len<Int, Int>(map)
}
"#;
    let error = eval_limited(
        source,
        VmLimits {
            mem_budget: Some(16 * 1024),
            step_budget: Some(10_000_000),
            ..VmLimits::default()
        },
    )
    .expect_err("map growth must hit memory budget");
    assert!(matches!(error, EvalError::Runtime(message) if message.contains("memory limit")));
}

#[test]
fn fresh_collection_intrinsics_are_charged_to_memory_budget() {
    let cases = [
        (
            "list-reverse",
            r#"fn main() -> Int {
    local values = List<Int>.new()
    let mut i = 0
    while i < 64 {
        List.push<Int>(list: mut values, value: i)
        i = i + 1
    }
    return List.len(list: List.reverse<Int>(list: values))
}"#,
            700,
        ),
        (
            "map-keys",
            r#"fn main() -> Int {
    let map = Map.new<Int, Int>()
    let mut i = 0
    while i < 32 {
        Map.insert<Int, Int>(map: mut map, key: i, value: i)
        i = i + 1
    }
    return List.len(list: Map.keys<Int, Int>(map: map))
}"#,
            1900,
        ),
        (
            "string-split",
            r#"fn main() -> Int {
    return List.len(list: String.split(value: "alpha,beta,gamma", delimiter: ","))
}"#,
            16,
        ),
        (
            "bytes-concat",
            r#"fn main() -> Int {
    local left = Bytes.from_string(value: "abcd")
    local right = Bytes.from_string(value: "efgh")
    local joined = Bytes.concat(left: left, right: right)
    return Bytes.len(value: joined)
}"#,
            12,
        ),
    ];

    for (name, source, mem_budget) in cases {
        let error = eval_limited(
            source,
            VmLimits {
                mem_budget: Some(mem_budget),
                step_budget: Some(1_000_000),
                ..VmLimits::default()
            },
        )
        .unwrap_err();
        assert!(
            matches!(error, EvalError::Runtime(ref message) if message.contains("memory limit")),
            "{name}: expected memory-limit error, got {error:?}"
        );
    }
}

#[test]
fn intrinsic_and_constructor_results_are_charged_before_publication() {
    let cases = [
        (
            "buffer-new",
            r#"fn main() -> Int {
    local buffer = Buffer.new(size: 1048576)
    return Buffer.len(buffer: buffer)
}"#
            .to_string(),
            1024,
        ),
        (
            "base64-encode",
            format!(
                "fn main() -> Int {{\n    let encoded = Base64.encode(value: \"{}\")\n    return String.len(value: encoded)\n}}",
                "x".repeat(8192)
            ),
            4096,
        ),
        (
            "json-parse",
            format!(
                "fn main() -> Int {{\n    let parsed = Json.parse(text: \"[{}]\")\n    return 0\n}}",
                std::iter::repeat_n("0", 2048).collect::<Vec<_>>().join(",")
            ),
            8192,
        ),
    ];

    for (name, source, mem_budget) in cases {
        let error = eval_limited(
            &source,
            VmLimits {
                mem_budget: Some(mem_budget),
                step_budget: Some(1_000_000),
                ..VmLimits::default()
            },
        )
        .expect_err("fresh intrinsic result must exceed the tight memory budget");
        assert!(
            matches!(error, EvalError::Runtime(ref message) if message.contains("memory limit")),
            "{name}: expected memory-limit error, got {error:?}"
        );
    }
}

#[test]
fn ordinary_vm_growth_paths_respect_memory_budget() {
    let cases = [
        (
            "string-concat",
            r#"fn main() -> Int {
    let value = String.concat(left: "abcdefghijklmnopqrstuvwxyz", right: "ABCDEFGHIJKLMNOPQRSTUVWXYZ")
    return String.len(value: value)
}"#,
            32,
        ),
        (
            "string-builder",
            r#"fn main() -> Int {
    local builder = StringBuilder.new()
    StringBuilder.push(builder: mut builder, value: "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ")
    return String.len(value: StringBuilder.finish(builder: take builder))
}"#,
            32,
        ),
        (
            "deque-growth",
            r#"fn main() -> Int {
    local values = Deque<Int>.new()
    let mut i = 0
    while i < 64 {
        Deque.push_back<Int>(deque: mut values, value: i)
        i = i + 1
    }
    return Deque.len<Int>(deque: values)
}"#,
            256,
        ),
        (
            "sorted-set-growth",
            r#"fn main() -> Int {
    local values = SortedSet<Int>.new()
    let mut i = 0
    while i < 32 {
        SortedSet.insert<Int>(set: mut values, value: i)
        i = i + 1
    }
    return SortedSet.len<Int>(set: values)
}"#,
            256,
        ),
        (
            "sorted-map-growth",
            r#"fn main() -> Int {
    local values = SortedMap<Int, Int>.new()
    let mut i = 0
    while i < 24 {
        SortedMap.insert<Int, Int>(map: mut values, key: i, value: i)
        i = i + 1
    }
    return SortedMap.len<Int, Int>(map: values)
}"#,
            512,
        ),
        (
            "list-sort-scratch",
            r#"fn main() -> Int {
    let mut values = [16, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1]
    List.sort_with<Int>(list: mut values, compare: |left, right| {
        return left - right
    })
    return values[0]
}"#,
            200,
        ),
    ];

    for (name, source, mem_budget) in cases {
        let error = eval_limited(
            source,
            VmLimits {
                mem_budget: Some(mem_budget),
                step_budget: Some(1_000_000),
                ..VmLimits::default()
            },
        )
        .expect_err("allocation path must exceed its tight memory budget");
        assert!(
            matches!(error, EvalError::Runtime(ref message) if message.contains("memory limit")),
            "{name}: expected memory-limit error, got {error:?}"
        );
    }
}

#[test]
fn shake_output_respects_memory_budget_and_hard_cap() {
    let source = r#"
fn main() -> Int {
    let bytes = Bytes.from_string(value: "input")
    let output = Hash.shake128_bytes(value: bytes, out_len: 1048576)
    return Bytes.len(value: output)
}
"#;
    let error = eval_limited(
        source,
        VmLimits {
            mem_budget: Some(64 * 1024),
            ..VmLimits::default()
        },
    )
    .expect_err("SHAKE output must hit memory budget");
    assert!(matches!(error, EvalError::Runtime(message) if message.contains("memory limit")));

    let too_large = source.replace("1048576", "67108865");
    let error = eval_limited(&too_large, VmLimits::default())
        .expect_err("SHAKE output above hard cap must fail");
    assert!(matches!(error, EvalError::Runtime(message) if message.contains("output exceeds")));
}

#[test]
fn integer_math_invalid_domains_return_language_errors() {
    let cases = [
        "Math.abs(value: -9223372036854775807 - 1)",
        "Math.clamp(value: 1, min: 2, max: 1)",
        "Math.pow(base: 2, exponent: -1)",
        "Math.pow(base: 2, exponent: 4294967296)",
        "Math.pow(base: 9223372036854775807, exponent: 2)",
    ];
    for expression in cases {
        let source = format!("fn main() -> Int {{ return {expression} }}");
        let error = eval_limited(&source, VmLimits::default())
            .expect_err("invalid math domain must return a language error");
        assert!(
            matches!(error, EvalError::Runtime(_)),
            "{expression}: {error:?}"
        );
    }
}

/// Default limits must NOT trip on ordinary code: a trusted run leaves the
/// step/memory budgets off and the depth cap generous, so a normal program
/// (modest recursion + a real loop + a list build) completes cleanly.
#[test]
fn default_limits_do_not_trip_on_normal_code() {
    let source = r#"fn fib(n: Int) -> Int {
    if n < 2 {
        return n
    }
    return fib(n: n - 1) + fib(n: n - 2)
}

fn main() -> Int {
    let mut index = 0
    local values = List<Int>.new()
    while index < 1000 {
        List.push<Int>(list: mut values, value: read index)
        index = index + 1
    }
    return fib(n: 20)
}
"#;
    let output = eval_limited(source, VmLimits::default()).expect("normal code must succeed");
    assert_eq!(output.value, "6765");
}

/// B3 follow-up: the ambient `cancel` flag is the host-level preemption hook for
/// a tight compute loop that never awaits or checks the cooperative RSS
/// `CancellationToken`. Set BEFORE eval and with NO step budget, the running
/// `while true {}` is preempted at the first throttled `tick()` poll and returns
/// a clean "evaluation cancelled" error — deterministic, no threads/timing.
#[test]
fn ambient_cancel_flag_preempts_infinite_loop() {
    let source = r#"
fn main() -> Int {
    let mut x = 0
    while true {
        x = x + 1
    }
    return x
}
"#;
    let flag = CancellationToken::new();
    flag.cancel();
    let limits = VmLimits {
        cancel: Some(flag.clone()),
        ..VmLimits::default()
    };
    let err = eval_limited(source, limits).expect_err("must error, not hang");
    match err {
        EvalError::Runtime(msg) => assert!(
            msg.contains("cancelled"),
            "expected cancellation error, got: {msg}"
        ),
        other => panic!("expected EvalError::Runtime, got {other:?}"),
    }
}

/// B3 follow-up (negative): a cancel flag that is present but `false` must not
/// trip — a short normal program still completes with `Ok`. Proves the hook is
/// opt-in per-fire, not merely per-presence.
#[test]
fn ambient_cancel_flag_unset_does_not_trip() {
    let source = r#"
fn main() -> Int {
    let mut x = 0
    while x < 10 {
        x = x + 1
    }
    return x
}
"#;
    let flag = CancellationToken::new();
    let limits = VmLimits {
        cancel: Some(flag.clone()),
        ..VmLimits::default()
    };
    let output = eval_limited(source, limits).expect("flag false => normal completion");
    assert_eq!(output.value, "10");
    // Sanity: the host still holds the flag and can set it for a future run.
    assert!(!flag.is_cancelled());
}

/// C6 (leak policy, positive): transient scalar work in a loop does not leak.
/// A long arithmetic loop reuses a fixed set of registers each iteration, so the
/// VM's best-effort byte accounting stays bounded and the program completes under
/// a tight `mem_budget` rather than tripping a false OOM. (No threads/timing.)
#[test]
fn transient_scalar_work_completes_under_memory_ceiling() {
    let source = r#"
fn main() -> Int {
    let mut index = 0
    let mut acc = 0
    while index < 200000 {
        acc = (acc + index) % 1000000
        index = index + 1
    }
    return acc
}
"#;
    // A tight 1 MiB ceiling: register-stack growth is bounded per frame and does
    // not grow per iteration, so a non-allocating loop never approaches it.
    let limits = VmLimits {
        mem_budget: Some(1 << 20),
        step_budget: Some(50_000_000),
        ..VmLimits::default()
    };
    let output = eval_limited(source, limits).expect("non-allocating loop must not leak/OOM");
    // 200000 -> documents the loop ran to completion (exact value unimportant).
    assert!(!output.value.is_empty());
}

/// C6 (leak policy, bounded): the value model uses `Rc`, so unbounded retained
/// growth — including the only way to form a cycle, a self-referential mutable
/// container — is the leak class of concern. rsscript does NOT run a cycle
/// collector; instead the B4 `mem_budget` backstops it: the run trips a clean
/// "memory limit" error (bounded), never an unbounded grow-until-host-OOM crash.
///
/// Note (a soundness bonus surfaced writing this test): the `local`/effect
/// checker actively *rejects* pushing a `local` value into another container
/// (RS0501 "retaining API cannot retain local value"), so a true `Rc` cycle
/// cannot even be expressed from safe source — cycles are rarer than the policy
/// assumes. This test therefore exercises the general retained-growth leak (a
/// list accumulating fresh values without bound), which is what `mem_budget`
/// must bound regardless of whether the retained graph is acyclic or cyclic.
#[test]
fn self_referential_container_is_bounded_by_memory_ceiling() {
    let source = r#"fn main() -> Int {
    local values = List<String>.new()
    let mut index = 0
    while index < 100000000 {
        List.push<String>(list: mut values, value: read "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx")
        index = index + 1
    }
    return 0
}
"#;
    let limits = VmLimits {
        mem_budget: Some(1 << 20),
        step_budget: Some(50_000_000),
        ..VmLimits::default()
    };
    let err = eval_limited(source, limits).expect_err("must trip a bounded error, not leak/crash");
    match err {
        EvalError::Runtime(msg) => assert!(
            msg.contains("memory limit"),
            "expected memory-limit (bounded) error, got: {msg}"
        ),
        other => panic!("expected EvalError::Runtime, got {other:?}"),
    }
}

/// B5 (output flood): a loop that writes to stdout every iteration is bounded by
/// `stdout_budget` — the run trips a clean "stdout budget" error instead of
/// growing the captured buffer until the host runs out of memory. A generous step
/// budget is set so a regression in stdout accounting fails loudly rather than
/// hanging.
#[test]
fn stdout_flood_with_output_budget_returns_clean_error() {
    let source = r#"
fn main() -> Int {
    let mut index = 0
    while index < 100000000 {
        Log.write(message: read "spam spam spam spam spam")
        index = index + 1
    }
    return 0
}
"#;
    let limits = VmLimits {
        stdout_budget: Some(64 * 1024),
        step_budget: Some(50_000_000),
        ..VmLimits::default()
    };
    let err = eval_limited(source, limits).expect_err("must trip on output flood, not OOM");
    match err {
        EvalError::Runtime(msg) => assert!(
            msg.contains("stdout budget"),
            "expected stdout-budget error, got: {msg}"
        ),
        other => panic!("expected EvalError::Runtime, got {other:?}"),
    }
}

/// B6 (intrinsic-call flood): a loop that performs a stdlib call every iteration
/// is bounded by `intrinsic_call_budget`. The run trips a clean error once it
/// exceeds the configured number of runtime-library calls.
#[test]
fn intrinsic_call_flood_with_call_budget_returns_clean_error() {
    let source = r#"
fn main() -> Int {
    let mut index = 0
    let mut total = 0
    while index < 100000000 {
        total = total + String.len(value: read "abc")
        index = index + 1
    }
    return total
}
"#;
    let limits = VmLimits {
        intrinsic_call_budget: Some(1_000),
        step_budget: Some(50_000_000),
        ..VmLimits::default()
    };
    let err = eval_limited(source, limits).expect_err("must trip on intrinsic-call flood");
    match err {
        EvalError::Runtime(msg) => assert!(
            msg.contains("intrinsic call budget"),
            "expected intrinsic-call-budget error, got: {msg}"
        ),
        other => panic!("expected EvalError::Runtime, got {other:?}"),
    }
}

/// A normal program that writes a little output and makes a few stdlib calls
/// completes within the bounded defaults.
#[test]
fn default_limits_allow_normal_output_and_host_calls() {
    let source = r#"
fn main() -> Int {
    let mut index = 0
    while index < 5 {
        Log.write(message: read "hello")
        index = index + 1
    }
    return index
}
"#;
    let output = eval_limited(source, VmLimits::default()).expect("normal code must succeed");
    assert_eq!(output.value, "5");
    assert_eq!(output.stdout, "hello\nhello\nhello\nhello\nhello\n");
}

/// Analyze every file under tests/corpus/malformed/. None may panic. Files not
/// prefixed `gap-` must also fail closed (report a diagnostic); `gap-` files are
/// known fail-open gaps the fuzzer surfaced (still must not panic).
#[test]
fn malformed_corpus_never_panics_and_fails_closed() {
    let dir = std::path::Path::new("tests/hostile-malformed");
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("malformed corpus dir exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "rss").unwrap_or(false))
        .collect();
    entries.sort();
    assert!(!entries.is_empty(), "malformed corpus is empty");

    for path in entries {
        let source = std::fs::read_to_string(&path).unwrap_or_default();
        let name = path.display().to_string();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let result = std::panic::catch_unwind(|| rsscript::analyze_source(&name, &source));
        assert!(
            result.is_ok(),
            "analyzer panicked on malformed input {name}"
        );
        let diagnostics = result.unwrap();
        if !file_name.starts_with("gap-") {
            assert!(
                !diagnostics.is_empty(),
                "malformed input {name} produced no diagnostics (should fail closed)"
            );
        }
    }
}

/// A few explicit adversarial strings (kept inline so the intent is visible).
#[test]
fn adversarial_strings_do_not_panic() {
    let inputs = [
        "",
        "\"",
        "\u{202e}\u{202d}",
        "let x = \"\\(",
        "fn f() { match x {",
        "\u{0}\u{0}\u{0}",
        "fn f() -> Int { return 0x }",
        &"(".repeat(5000),
        &"fn f(){}\n".repeat(2000),
    ];
    for (index, source) in inputs.iter().enumerate() {
        let result =
            std::panic::catch_unwind(|| rsscript::analyze_source("adversarial.rss", source));
        assert!(
            result.is_ok(),
            "analyzer panicked on adversarial input #{index}: {source:?}"
        );
    }
}

proptest! {
    // proptest catches panics and shrinks to a minimal reproducer, so a bare
    // call is the fuzz target: any string the generator produces must not crash
    // the front end.
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn arbitrary_text_never_panics_the_front_end(source in ".{0,400}") {
        let _ = rsscript::analyze_source("fuzz.rss", &source);
    }

    #[test]
    fn arbitrary_utf8_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        if let Ok(source) = String::from_utf8(bytes) {
            let _ = rsscript::analyze_source("fuzz.rss", &source);
        }
    }

    // Source biased toward RSScript tokens is more likely to reach deep parser
    // and checker paths.
    #[test]
    fn token_soup_never_panics(
        tokens in proptest::collection::vec(
            prop::sample::select(vec![
                "fn", "let", "return", "struct", "native", "effects", "read", "mut",
                "take", "fresh", "match", "(", ")", "{", "}", "<", ">", ":", ",",
                "->", "Int", "String", "x", "\"", "|", "=", "0", "99999999999",
            ]),
            0..80,
        ),
    ) {
        let source = tokens.join(" ");
        let _ = rsscript::analyze_source("fuzz.rss", &source);
    }
}
