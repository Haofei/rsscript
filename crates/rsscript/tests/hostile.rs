//! Hostile-input suite: the front end is the first supply-chain boundary for
//! AI-generated and untrusted source, so it must never panic and must fail
//! closed (produce diagnostics) on malformed input rather than silently
//! yielding a "clean" partial result.

use proptest::prelude::*;

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
        assert!(result.is_ok(), "analyzer panicked on malformed input {name}");
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
