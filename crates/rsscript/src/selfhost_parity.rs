//! Self-hosting stress-test harness (test-only).
//!
//! Runs the rss-written lexer (`selfhost/lexer.rss`) on rss source and compares
//! its canonical token dump against the real Rust lexer (`crate::lexer::lex`),
//! which defines truth. In-process: the corpus file's *content* is passed as
//! argv[0] to the rss program, whose stdout is the dump. See `selfhost/FORMAT.md`.
//!
//! No new public API and no new CLI: this module is `#[cfg(test)]` and reaches
//! the private `crate::lexer` and the VM entry point directly. Divergences are
//! recorded as `SH-NNN` entries in `docs/ledgers/rss-selfhost-ledger.md`.

use std::path::PathBuf;

use crate::lexer::{TokenKind, lex};
use crate::{RegVmExecutable, Severity, analyze_source, reg_vm_compile_source};

/// One token in the canonical dump. Positions are `None` when the producer
/// emitted placeholders (the rss lexer does so until spans are implemented).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CanonTok {
    line: usize,
    col: usize,
    len: usize,
    kind: String,
    payload: String,
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn selfhost_dir() -> PathBuf {
    workspace_root().join("selfhost")
}

/// Comparison tier from `RSS_SELFHOST_TIER` (default 0). 0 = kind+payload,
/// 1 = +position, 2 = +byte length.
fn tier() -> u8 {
    match std::env::var("RSS_SELFHOST_TIER").ok().as_deref() {
        Some("1") => 1,
        Some("2") => 2,
        _ => 0,
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out
}

fn kind_name(kind: &TokenKind) -> &'static str {
    match kind {
        TokenKind::Ident(_) => "Ident",
        TokenKind::Number(_) => "Number",
        TokenKind::String(_) => "String",
        TokenKind::Char(_) => "Char",
        TokenKind::InterpolatedString(_) => "InterpolatedString",
        TokenKind::MultilineString(_) => "MultilineString",
        TokenKind::Keyword(_) => "Keyword",
        TokenKind::Symbol(_) => "Symbol",
        TokenKind::Eof => "Eof",
    }
}

fn payload(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Ident(s)
        | TokenKind::Number(s)
        | TokenKind::String(s)
        | TokenKind::Char(s)
        | TokenKind::InterpolatedString(s)
        | TokenKind::MultilineString(s) => escape(s),
        TokenKind::Keyword(k) | TokenKind::Symbol(k) => escape(k),
        TokenKind::Eof => String::new(),
    }
}

/// Truth: the real Rust lexer's canonical token stream.
fn oracle_dump(file: &str, source: &str) -> Vec<CanonTok> {
    lex(file, source)
        .iter()
        .map(|t| CanonTok {
            line: t.span.line,
            col: t.span.column,
            len: t.span.length,
            kind: kind_name(&t.kind).to_string(),
            payload: payload(&t.kind),
        })
        .collect()
}

/// Parse a `L:C:N\tKIND\tPAYLOAD` line into a token (permissive on positions).
fn parse_line(line: &str) -> Option<CanonTok> {
    let mut parts = line.splitn(3, '\t');
    let pos = parts.next()?;
    let kind = parts.next()?.to_string();
    let payload = parts.next().unwrap_or("").to_string();
    let mut nums = pos.split(':');
    let line_no = nums.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let col = nums.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let len = nums.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    Some(CanonTok {
        line: line_no,
        col,
        len,
        kind,
        payload,
    })
}

/// Read a self-hosted tool source, prepended with the shared `scan.rss` prelude.
/// The single-file VM model has no cross-file import, so the shared scanner is
/// concatenated (scan.rss first, then a newline, then the tool file) and the
/// combined program is compiled as one unit. `features: local` therefore appears
/// exactly once (only in scan.rss).
fn combined_tool_source(tool: &str) -> Result<String, String> {
    let dir = selfhost_dir();
    let scan_path = dir.join("scan.rss");
    let scan_src = std::fs::read_to_string(&scan_path)
        .map_err(|e| format!("cannot read {}: {e}", scan_path.display()))?;
    let tool_path = dir.join(tool);
    let tool_src = std::fs::read_to_string(&tool_path)
        .map_err(|e| format!("cannot read {}: {e}", tool_path.display()))?;
    Ok(format!("{scan_src}\n{tool_src}"))
}

/// Compile `selfhost/lexer.rss` (with the shared prelude) once for reuse.
fn compile_lexer() -> Result<RegVmExecutable, String> {
    let combined = combined_tool_source("lexer.rss")?;
    reg_vm_compile_source("selfhost/lexer.rss", &combined)
        .map_err(|e| format!("rss lexer failed to compile: {e:?}"))
}

/// Run a precompiled rss lexer over `source` and parse its dump.
fn rss_dump_with(exe: &RegVmExecutable, source: &str) -> Result<Vec<CanonTok>, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss lexer failed to run: {e:?}"))?;
    Ok(output
        .stdout
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(parse_line)
        .collect())
}

/// Convenience: compile + run once (used by the single-file smoke test).
fn rss_dump(source: &str) -> Result<Vec<CanonTok>, String> {
    rss_dump_with(&compile_lexer()?, source)
}

/// Compare two token streams at the active tier; `Ok(())` or a diff message.
fn compare(oracle: &[CanonTok], actual: &[CanonTok], tier: u8) -> Result<(), String> {
    let field = |t: &CanonTok| match tier {
        0 => format!("{}\t{}", t.kind, t.payload),
        1 => format!("{}:{}\t{}\t{}", t.line, t.col, t.kind, t.payload),
        _ => format!("{}:{}:{}\t{}\t{}", t.line, t.col, t.len, t.kind, t.payload),
    };
    let n = oracle.len().max(actual.len());
    for i in 0..n {
        let o = oracle.get(i).map(field);
        let a = actual.get(i).map(field);
        if o != a {
            return Err(format!(
                "token #{i} diverges (tier {tier}):\n  oracle: {o:?}\n  rss:    {a:?}\n  \
                 (oracle {} tokens, rss {} tokens)",
                oracle.len(),
                actual.len()
            ));
        }
    }
    Ok(())
}

/// Recursively collect `*.rss` files under `root`, skipping build output.
fn collect_rss_files(root: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == "target" || name == ".git" {
                    continue;
                }
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rss") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Phase-0 proof: the rss lexer matches the Rust lexer on a tiny sample.
#[test]
fn lexer_parity_tiny_sample() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let oracle = oracle_dump("samples/tiny.rss", &source);
    let actual = rss_dump(&source).expect("rss lexer should run");
    compare(&oracle, &actual, tier()).unwrap_or_else(|msg| panic!("{msg}"));
}

/// Phase-1 gate (ignored by default; run with `-- --ignored`): the rss lexer
/// matches the Rust lexer over the whole `.rss` corpus. Prints a summary of
/// divergences (run-failures vs token mismatches) before asserting all pass.
#[test]
#[ignore]
fn lexer_parity_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root);
    let tier = tier();
    let exe = compile_lexer().expect("rss lexer should compile");
    let mut run_failures: Vec<String> = Vec::new();
    let mut mismatches: Vec<String> = Vec::new();
    let mut ok = 0usize;
    for file in &files {
        let rel = file.strip_prefix(&root).unwrap_or(file).display().to_string();
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                run_failures.push(format!("{rel}: unreadable: {e}"));
                continue;
            }
        };
        let oracle = oracle_dump(&rel, &source);
        match rss_dump_with(&exe, &source) {
            Err(e) => run_failures.push(format!("{rel}: {e}")),
            Ok(actual) => match compare(&oracle, &actual, tier) {
                Ok(()) => ok += 1,
                Err(msg) => mismatches.push(format!("{rel}: {msg}")),
            },
        }
    }
    let total = files.len();
    eprintln!(
        "\n=== lexer_parity_corpus (tier {tier}) ===\n  files: {total}\n  ok: {ok}\n  \
         run-failures: {}\n  token-mismatches: {}\n",
        run_failures.len(),
        mismatches.len()
    );
    for line in run_failures.iter().take(15) {
        eprintln!("[run-fail] {line}");
    }
    for line in mismatches.iter().take(15) {
        eprintln!("[mismatch] {line}");
    }
    assert!(
        run_failures.is_empty() && mismatches.is_empty(),
        "lexer parity failed: {} run-failures, {} mismatches (of {total})",
        run_failures.len(),
        mismatches.len()
    );
}

/// Phase-4 perf probe (ignored; run with `--release -- --ignored --nocapture`):
/// how much slower is the self-hosted rss lexer (on the reg-VM) than the native
/// Rust `lex()` over the whole corpus? This is the "is the self-hosted tool
/// slow?" macro-benchmark — a real workload, not a microkernel. Feeds the parked
/// VM value-representation / intrinsic-dispatch perf work.
#[test]
#[ignore]
fn lexer_perf_corpus() {
    use std::time::Instant;
    let root = workspace_root();
    let files = collect_rss_files(&root);
    let exe = compile_lexer().expect("rss lexer should compile");
    let mut rust_ns: u128 = 0;
    let mut rss_ns: u128 = 0;
    let mut bytes: usize = 0;
    let mut n_ok = 0usize;
    for file in &files {
        let rel = file.strip_prefix(&root).unwrap_or(file).display().to_string();
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        bytes += source.len();
        let t0 = Instant::now();
        let _ = lex(&rel, &source);
        rust_ns += t0.elapsed().as_nanos();
        let t1 = Instant::now();
        if exe.eval_main_with_args([source.clone()]).is_ok() {
            n_ok += 1;
        }
        rss_ns += t1.elapsed().as_nanos();
    }
    let rust_ms = rust_ns as f64 / 1e6;
    let rss_ms = rss_ns as f64 / 1e6;
    eprintln!(
        "\n=== lexer_perf_corpus ===\n  files: {} (ran {n_ok})\n  bytes: {bytes}\n  \
         Rust lex():   {rust_ms:.1} ms  ({:.1} MB/s)\n  rss lexer/VM: {rss_ms:.1} ms  \
         ({:.1} MB/s)\n  slowdown (rss/Rust): {:.1}x\n",
        files.len(),
        bytes as f64 / 1e6 / (rust_ms / 1e3),
        bytes as f64 / 1e6 / (rss_ms / 1e3),
        rss_ms / rust_ms,
    );
}

// ---------------------------------------------------------------------------
// Phase 2 — parser recognition parity.
//
// The rss parser (`selfhost/parser.rss`) recognizes rss source and prints a
// verdict: `OK` if it accepts, or `ERR <line> <col>` at the first syntax error.
// Oracle: the real Rust parser `crate::syntax::parse_source_raw`, which never
// panics and collects parse errors as span vectors on the returned `Program`.
// Recognition tier (default): compare accept-vs-reject only. Position tier
// (`RSS_SELFHOST_PARSE_TIER=1`): also compare the first-error line:col.
// ---------------------------------------------------------------------------

/// Oracle verdict: `None` if the Rust parser accepts, else the first parse
/// error's (line, column).
fn parse_oracle_error(file: &str, source: &str) -> Option<(usize, usize)> {
    let program = crate::syntax::parse_source_raw(file, source);
    let mut spans: Vec<(usize, usize)> = Vec::new();
    for s in &program.unknown_top_level_spans {
        spans.push((s.line, s.column));
    }
    for s in &program.malformed_declaration_spans {
        spans.push((s.line, s.column));
    }
    for f in &program.unknown_features {
        spans.push((f.span.line, f.span.column));
    }
    for f in &program.duplicate_features {
        spans.push((f.span.line, f.span.column));
    }
    spans.sort_unstable();
    spans.into_iter().next()
}

fn parse_position_tier() -> bool {
    std::env::var("RSS_SELFHOST_PARSE_TIER").ok().as_deref() == Some("1")
}

fn compile_parser() -> Result<RegVmExecutable, String> {
    let combined = combined_tool_source("parser.rss")?;
    reg_vm_compile_source("selfhost/parser.rss", &combined)
        .map_err(|e| format!("rss parser failed to compile: {e:?}"))
}

/// Run the precompiled rss parser; parse its verdict line.
fn run_parser(exe: &RegVmExecutable, source: &str) -> Result<Option<(usize, usize)>, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss parser failed to run: {e:?}"))?;
    let verdict = output
        .stdout
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    if verdict == "OK" {
        Ok(None)
    } else if let Some(rest) = verdict.strip_prefix("ERR") {
        let mut nums = rest.split_whitespace();
        let line = nums.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let col = nums.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        Ok(Some((line, col)))
    } else {
        Err(format!("unrecognized parser verdict: {verdict:?}"))
    }
}

/// Compare parser verdicts. Recognition tier: accept-vs-reject. Position tier:
/// also the first-error coordinates.
fn compare_parse(
    oracle: Option<(usize, usize)>,
    actual: Option<(usize, usize)>,
    position: bool,
) -> Result<(), String> {
    if oracle.is_some() != actual.is_some() {
        return Err(format!(
            "accept/reject diverges: oracle={:?} rss={:?}",
            oracle, actual
        ));
    }
    if position && oracle != actual {
        return Err(format!(
            "first-error position diverges: oracle={:?} rss={:?}",
            oracle, actual
        ));
    }
    Ok(())
}

/// Phase-2 proof: the rss parser agrees with the Rust parser on a tiny sample.
#[test]
fn parser_parity_tiny_sample() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let oracle = parse_oracle_error("samples/tiny.rss", &source);
    let exe = compile_parser().expect("rss parser should compile");
    let actual = run_parser(&exe, &source).expect("rss parser should run");
    compare_parse(oracle, actual, parse_position_tier()).unwrap_or_else(|msg| panic!("{msg}"));
}

/// Phase-2 gate (ignored by default): the rss parser's accept/reject matches the
/// Rust parser over the whole `.rss` corpus.
#[test]
#[ignore]
fn parser_parity_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root);
    let position = parse_position_tier();
    let exe = compile_parser().expect("rss parser should compile");
    let mut run_failures: Vec<String> = Vec::new();
    let mut mismatches: Vec<String> = Vec::new();
    let mut ok = 0usize;
    for file in &files {
        let rel = file.strip_prefix(&root).unwrap_or(file).display().to_string();
        let source = match std::fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                run_failures.push(format!("{rel}: unreadable: {e}"));
                continue;
            }
        };
        let oracle = parse_oracle_error(&rel, &source);
        match run_parser(&exe, &source) {
            Err(e) => run_failures.push(format!("{rel}: {e}")),
            Ok(actual) => match compare_parse(oracle, actual, position) {
                Ok(()) => ok += 1,
                Err(msg) => mismatches.push(format!("{rel}: {msg}")),
            },
        }
    }
    let total = files.len();
    eprintln!(
        "\n=== parser_parity_corpus (position={position}) ===\n  files: {total}\n  ok: {ok}\n  \
         run-failures: {}\n  verdict-mismatches: {}\n",
        run_failures.len(),
        mismatches.len()
    );
    for line in run_failures.iter().take(20) {
        eprintln!("[run-fail] {line}");
    }
    for line in mismatches.iter().take(20) {
        eprintln!("[mismatch] {line}");
    }
    assert!(
        run_failures.is_empty() && mismatches.is_empty(),
        "parser parity failed: {} run-failures, {} mismatches (of {total})",
        run_failures.len(),
        mismatches.len()
    );
}

// ---------------------------------------------------------------------------
// Phase 3 — checker parity (semantic diagnostics).
//
// The rss checker (`selfhost/check.rss`) reproduces a chosen subset of analyzer
// diagnostics and prints the codes it finds (one per line, or `CLEAN`). Oracle:
// the real analyzer `crate::analyze_source`, filtered to the same target codes.
// We start with RS0005 (DUPLICATE_DECLARATION — duplicate top-level item names
// and duplicate struct/sum fields), decidable from declaration structure alone
// (no expression/statement parsing needed; see SH-021).
// ---------------------------------------------------------------------------

/// Diagnostic codes the rss checker is expected to reproduce.
const CHECKER_TARGET_CODES: &[&str] = &["RS0005"];

fn is_target_code(code: &str) -> bool {
    CHECKER_TARGET_CODES.contains(&code)
}

/// Oracle: the set of target diagnostic codes the real analyzer reports.
fn checker_oracle_codes(file: &str, source: &str) -> Vec<String> {
    let mut codes: Vec<String> = analyze_source(file, source)
        .into_iter()
        .filter(|d| d.severity == Severity::Error && is_target_code(&d.code))
        .map(|d| d.code)
        .collect();
    codes.sort();
    codes.dedup();
    codes
}

fn compile_checker() -> Result<RegVmExecutable, String> {
    let combined = combined_tool_source("check.rss")?;
    reg_vm_compile_source("selfhost/check.rss", &combined)
        .map_err(|e| format!("rss checker failed to compile: {e:?}"))
}

/// Run the rss checker; parse the target codes it reports (`CLEAN` => none).
fn run_checker(exe: &RegVmExecutable, source: &str) -> Result<Vec<String>, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss checker failed to run: {e:?}"))?;
    let mut codes: Vec<String> = output
        .stdout
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && *l != "CLEAN" && is_target_code(l))
        .map(|l| l.to_string())
        .collect();
    codes.sort();
    codes.dedup();
    Ok(codes)
}

/// Phase-3 proof: the rss checker agrees with the analyzer on a tiny sample
/// (no duplicates → both report no target codes).
#[test]
fn checker_parity_tiny_sample() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let oracle = checker_oracle_codes("samples/tiny.rss", &source);
    let exe = compile_checker().expect("rss checker should compile");
    let actual = run_checker(&exe, &source).expect("rss checker should run");
    assert_eq!(oracle, actual, "checker parity diverged on tiny sample");
}

/// Phase-3 gate (ignored by default): the rss checker's target-code diagnostics
/// match the analyzer over the whole `.rss` corpus.
#[test]
#[ignore]
fn checker_parity_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root);
    let exe = compile_checker().expect("rss checker should compile");
    let mut run_failures: Vec<String> = Vec::new();
    let mut mismatches: Vec<String> = Vec::new();
    let mut ok = 0usize;
    for file in &files {
        let rel = file.strip_prefix(&root).unwrap_or(file).display().to_string();
        let Ok(source) = std::fs::read_to_string(file) else {
            run_failures.push(format!("{rel}: unreadable"));
            continue;
        };
        let oracle = checker_oracle_codes(&rel, &source);
        match run_checker(&exe, &source) {
            Err(e) => run_failures.push(format!("{rel}: {e}")),
            Ok(actual) => {
                if actual == oracle {
                    ok += 1;
                } else {
                    mismatches.push(format!("{rel}: oracle={oracle:?} rss={actual:?}"));
                }
            }
        }
    }
    let total = files.len();
    eprintln!(
        "\n=== checker_parity_corpus (codes {CHECKER_TARGET_CODES:?}) ===\n  files: {total}\n  \
         ok: {ok}\n  run-failures: {}\n  code-mismatches: {}\n",
        run_failures.len(),
        mismatches.len()
    );
    for line in run_failures.iter().take(20) {
        eprintln!("[run-fail] {line}");
    }
    for line in mismatches.iter().take(20) {
        eprintln!("[mismatch] {line}");
    }
    assert!(
        run_failures.is_empty() && mismatches.is_empty(),
        "checker parity failed: {} run-failures, {} mismatches (of {total})",
        run_failures.len(),
        mismatches.len()
    );
}
