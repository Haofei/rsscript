//! Hostile-input suite: the front end is the first supply-chain boundary for
//! AI-generated and untrusted source, so it must never panic and must fail
//! closed (produce diagnostics) on malformed input rather than silently
//! yielding a "clean" partial result.

use proptest::prelude::*;
use rsscript::{EvalError, VmLimits, reg_vm_eval_source_main_with_limits};

/// Invariant 1 (runtime hardening): no agent program may crash or hang the host.
/// These cases prove that the reg-VM's sandbox limits turn the three classic
/// resource-exhaustion attacks — unbounded recursion (native stack overflow =
/// SIGSEGV), an infinite loop (hang), and runaway allocation (OOM-kill) — into
/// clean, recoverable `EvalError::Runtime` values instead. They run in the dev
/// profile via the public limit-aware eval entry point.
///
/// Helper: eval `source`'s `main` under `limits`, returning the result.
fn eval_limited(source: &str, limits: VmLimits) -> Result<rsscript::EvalOutput, EvalError> {
    reg_vm_eval_source_main_with_limits(
        "hostile.rss",
        source,
        std::iter::empty::<String>(),
        limits,
    )
}

/// A1: unbounded self-recursion hits the default-on depth cap and returns a
/// clean "recursion depth" error rather than overflowing the native stack.
#[test]
fn deep_recursion_returns_clean_error_not_crash() {
    // Default limits: depth cap on, step/memory budgets off — this is exactly
    // what a trusted run would use, and it still catches `f(){f()}`.
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
    let source = r#"features: local

fn main() -> Int {
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

/// Default limits must NOT trip on ordinary code: a trusted run leaves the
/// step/memory budgets off and the depth cap generous, so a normal program
/// (modest recursion + a real loop + a list build) completes cleanly.
#[test]
fn default_limits_do_not_trip_on_normal_code() {
    let source = r#"features: local

fn fib(n: Int) -> Int {
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
