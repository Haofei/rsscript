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
        TokenKind::Unknown(_) => "Unknown",
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
        TokenKind::Unknown(c) => escape(&c.to_string()),
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
    // Fail on any malformed non-empty dump line rather than silently dropping it
    // (a stray debug line or a garbled token would otherwise vanish and let a
    // broken lexer pass parity by emitting fewer/no tokens).
    output
        .stdout
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            parse_line(l).ok_or_else(|| format!("rss lexer emitted a malformed dump line: {l:?}"))
        })
        .collect()
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

/// Phase-2 NEGATIVE smoke (non-ignored): the rss parser must REJECT malformed
/// source, matching the Rust oracle. The accept-only tiny sample above would
/// still pass if the rss parser degenerated to always printing `OK`; this closes
/// that gap without needing the (ignored) full-corpus gate.
#[test]
fn parser_rejects_malformed_source_smoke() {
    let source = "fn main() -> Unit {\n    return Unit\n}\n\nfn\n";
    let oracle = parse_oracle_error("parser-negative.rss", source);
    assert!(
        oracle.is_some(),
        "oracle Rust parser must reject the malformed sample (else the smoke test proves nothing)"
    );
    let exe = compile_parser().expect("rss parser should compile");
    let actual = run_parser(&exe, source).expect("rss parser should run");
    // Recognition tier: both must reject (accept-vs-reject only).
    compare_parse(oracle, actual, false).unwrap_or_else(|msg| panic!("{msg}"));
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

/// Phase-3 POSITIVE smoke (non-ignored): the rss checker must REPORT RS0005 for a
/// duplicate declaration, matching the analyzer. The no-duplicate tiny sample
/// above would still pass if the rss checker degenerated to always printing
/// `CLEAN`; this closes that gap without the (ignored) full-corpus gate.
#[test]
fn checker_reports_rs0005_for_duplicate_declaration_smoke() {
    let source =
        "fn dup() -> Unit {\n    return Unit\n}\nfn dup() -> Unit {\n    return Unit\n}\n";
    let oracle = checker_oracle_codes("checker-duplicate.rss", source);
    assert!(
        oracle.contains(&"RS0005".to_string()),
        "oracle analyzer must report RS0005 for the duplicate declaration; got {oracle:?}"
    );
    let exe = compile_checker().expect("rss checker should compile");
    let actual = run_checker(&exe, source).expect("rss checker should run");
    assert_eq!(
        oracle, actual,
        "checker parity diverged on the duplicate-declaration smoke test"
    );
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

// ---------------------------------------------------------------------------
// AST-dump parity — format contract + Rust oracle (step 1 of frontend object
// parity). The rss parser will one day emit the canonical AST dump defined in
// `selfhost/AST_FORMAT.md`; this oracle emits it from the surface-preserving
// tree (`crate::syntax::parse_source_raw`, NOT the desugared `parse_source`).
// Byte-identical dumps = AST parity. This ships BEFORE `parser.rss` builds an
// AST, exactly as the token FORMAT.md + oracle preceded the rss lexer.
//
// The serializer is TOTAL over the AST (every Item/Stmt/Expr/Pattern variant is
// rendered) so a future producer cannot pass by silently dropping a node. Tier 0:
// structure + payload, spans omitted (span parity is the final phase).
// ---------------------------------------------------------------------------

use crate::syntax::ast;

fn push_line(out: &mut String, depth: usize, content: &str) {
    for _ in 0..depth {
        out.push_str("  ");
    }
    out.push_str(content);
    out.push('\n');
}

fn feature_name(f: ast::FileFeature) -> &'static str {
    match f {
        ast::FileFeature::Local => "local",
        ast::FileFeature::Native => "native",
        ast::FileFeature::Unsafe => "unsafe",
        ast::FileFeature::Async => "async",
        ast::FileFeature::Device => "device",
        ast::FileFeature::Ffi => "ffi",
        ast::FileFeature::Reflection => "reflection",
    }
}

fn type_kind_str(k: ast::TypeKind) -> &'static str {
    match k {
        ast::TypeKind::Class => "class",
        ast::TypeKind::Struct => "struct",
        ast::TypeKind::Resource => "resource",
    }
}

fn let_kind_str(k: ast::LetKind) -> &'static str {
    match k {
        ast::LetKind::Managed => "managed",
        ast::LetKind::Local => "local",
    }
}

fn binop_name(op: ast::BinaryOp) -> &'static str {
    match op {
        ast::BinaryOp::Add => "add",
        ast::BinaryOp::Subtract => "subtract",
        ast::BinaryOp::Multiply => "multiply",
        ast::BinaryOp::Divide => "divide",
        ast::BinaryOp::Modulo => "modulo",
        ast::BinaryOp::BitAnd => "bit-and",
        ast::BinaryOp::BitOr => "bit-or",
        ast::BinaryOp::BitXor => "bit-xor",
        ast::BinaryOp::ShiftLeft => "shift-left",
        ast::BinaryOp::ShiftRight => "shift-right",
        ast::BinaryOp::Equal => "equal",
        ast::BinaryOp::NotEqual => "not-equal",
        ast::BinaryOp::Less => "less",
        ast::BinaryOp::LessEqual => "less-equal",
        ast::BinaryOp::Greater => "greater",
        ast::BinaryOp::GreaterEqual => "greater-equal",
        ast::BinaryOp::LogicalAnd => "logical-and",
        ast::BinaryOp::LogicalOr => "logical-or",
    }
}

/// Truth: the canonical AST dump of the surface-preserving parse tree.
fn ast_oracle_dump(file: &str, source: &str) -> String {
    let program = crate::syntax::parse_source_raw(file, source);
    let mut out = String::new();
    push_line(&mut out, 0, "program");
    for f in &program.features {
        push_line(&mut out, 1, &format!("feature {}", feature_name(*f)));
    }
    for item in &program.items {
        dump_item(&mut out, 1, item);
    }
    for p in &program.protocols {
        push_line(&mut out, 1, &format!("protocol {}", p.name));
    }
    for pi in &program.protocol_impls {
        push_line(
            &mut out,
            1,
            &format!("protocol-impl protocol={} type={}", pi.protocol, pi.type_name),
        );
        for m in &pi.mappings {
            push_line(
                &mut out,
                2,
                &format!("mapping method={} target={}", m.method, m.target),
            );
        }
    }
    for f in &program.unknown_features {
        push_line(&mut out, 1, &format!("unknown-feature {}", escape(&f.name)));
    }
    for f in &program.duplicate_features {
        push_line(&mut out, 1, &format!("duplicate-feature {}", escape(&f.name)));
    }
    for _ in &program.unknown_top_level_spans {
        push_line(&mut out, 1, "unknown-top-level");
    }
    for _ in &program.malformed_declaration_spans {
        push_line(&mut out, 1, "malformed-declaration");
    }
    out
}

fn dump_item(out: &mut String, depth: usize, item: &ast::Item) {
    match item {
        ast::Item::Module(m) => {
            push_line(out, depth, &format!("module path={}", m.path.join(".")));
        }
        ast::Item::Use(u) => {
            let mut line = format!("use path={} glob={}", u.path.join("."), u.glob);
            if let Some(a) = &u.alias {
                line.push_str(&format!(" alias={a}"));
            }
            push_line(out, depth, &line);
        }
        ast::Item::Type(t) => {
            push_line(
                out,
                depth,
                &format!(
                    "type kind={} name={} public={} opaque={}",
                    type_kind_str(t.kind),
                    t.name,
                    t.is_public,
                    t.is_opaque
                ),
            );
            for g in &t.type_params {
                dump_generic(out, depth + 1, g);
            }
            for d in &t.derives {
                push_line(out, depth + 1, &format!("derive {d}"));
            }
            for f in &t.fields {
                dump_field(out, depth + 1, f);
            }
            for _ in &t.malformed_generic_param_spans {
                push_line(out, depth + 1, "malformed-generic");
            }
            for _ in &t.malformed_field_spans {
                push_line(out, depth + 1, "malformed-field");
            }
            if let Some(b) = &t.drop_body {
                push_line(out, depth + 1, "drop");
                dump_block(out, depth + 2, b);
            }
        }
        ast::Item::SumType(s) => {
            push_line(
                out,
                depth,
                &format!("sum name={} public={}", s.name, s.is_public),
            );
            for g in &s.type_params {
                dump_generic(out, depth + 1, g);
            }
            for d in &s.derives {
                push_line(out, depth + 1, &format!("derive {d}"));
            }
            for v in &s.variants {
                push_line(out, depth + 1, &format!("variant name={}", v.name));
                for f in &v.fields {
                    dump_field(out, depth + 2, f);
                }
            }
        }
        ast::Item::TypeAlias(a) => {
            push_line(
                out,
                depth,
                &format!("type-alias name={} public={}", a.name, a.is_public),
            );
            for g in &a.type_params {
                dump_generic(out, depth + 1, g);
            }
            dump_type_ref(out, depth + 1, &a.target, "target", None);
        }
        ast::Item::Const(c) => {
            push_line(
                out,
                depth,
                &format!("const name={} public={}", c.name, c.is_public),
            );
            if let Some(t) = &c.type_annotation {
                dump_type_ref(out, depth + 1, t, "type", None);
            }
            push_line(out, depth + 1, "value");
            dump_expr(out, depth + 2, &c.value);
        }
        ast::Item::Function(f) => dump_function(out, depth, f),
    }
}

fn dump_function(out: &mut String, depth: usize, f: &ast::FunctionDecl) {
    let mut line = format!(
        "fn name={} public={} async={} native={} has-body={}",
        f.name, f.is_public, f.is_async, f.is_native, f.has_body
    );
    if f.default_impl_marker {
        line.push_str(" default-impl=true");
    }
    if f.returns_fresh {
        line.push_str(" returns-fresh=true");
    }
    push_line(out, depth, &line);
    if let Some(r) = &f.deprecated_reason {
        push_line(out, depth + 1, &format!("deprecated {}", escape(r)));
    }
    if let Some(l) = &f.lower_name {
        push_line(out, depth + 1, &format!("lower-name {l}"));
    }
    for g in &f.type_params {
        dump_generic(out, depth + 1, g);
    }
    for p in &f.params {
        dump_param(out, depth + 1, p);
    }
    if let Some(r) = &f.return_ty {
        dump_type_ref(out, depth + 1, r, "return-type", None);
    }
    for e in &f.effects {
        match e {
            ast::EffectDecl::Name(n) => push_line(out, depth + 1, &format!("effect-name {n}")),
            ast::EffectDecl::Retains(n) => {
                push_line(out, depth + 1, &format!("effect-retains {n}"))
            }
        }
    }
    for _ in &f.malformed_generic_param_spans {
        push_line(out, depth + 1, "malformed-generic");
    }
    for _ in &f.malformed_param_spans {
        push_line(out, depth + 1, "malformed-param");
    }
    for _ in &f.malformed_effect_spans {
        push_line(out, depth + 1, "malformed-effect");
    }
    push_line(out, depth + 1, "body");
    dump_block(out, depth + 2, &f.body);
}

fn dump_generic(out: &mut String, depth: usize, g: &ast::GenericParam) {
    push_line(out, depth, &format!("generic name={}", g.name));
    if let Some(b) = &g.bound {
        let s = match b {
            ast::GenericBound::Managed => "bound managed".to_string(),
            ast::GenericBound::Struct => "bound struct".to_string(),
            ast::GenericBound::Resource => "bound resource".to_string(),
            ast::GenericBound::Protocol(p) => format!("bound protocol={p}"),
        };
        push_line(out, depth + 1, &s);
    }
}

fn dump_field(out: &mut String, depth: usize, f: &ast::FieldDecl) {
    push_line(
        out,
        depth,
        &format!(
            "field name={} handle={} weak={}",
            f.name, f.is_handle, f.is_weak
        ),
    );
    dump_type_ref(out, depth + 1, &f.ty, "type", None);
    if let Some(d) = &f.default {
        push_line(out, depth + 1, "default");
        dump_expr(out, depth + 2, d);
    }
}

fn dump_param(out: &mut String, depth: usize, p: &ast::Param) {
    let mut line = format!("param name={}", p.name);
    if let Some(e) = p.effect {
        line.push_str(&format!(" effect={}", e.as_str()));
    }
    push_line(out, depth, &line);
    dump_type_ref(out, depth + 1, &p.ty, "type", None);
    if let Some(d) = &p.default {
        push_line(out, depth + 1, "default");
        dump_expr(out, depth + 2, d);
    }
}

fn dump_type_ref(
    out: &mut String,
    depth: usize,
    tr: &ast::TypeRef,
    tag: &str,
    eff: Option<ast::DataEffect>,
) {
    let mut line = String::from(tag);
    if let Some(e) = eff {
        line.push_str(&format!(" effect={}", e.as_str()));
    }
    line.push_str(&format!(
        " name={} fresh={} noescape={} owned={}",
        tr.name, tr.is_fresh, tr.is_noescape, tr.is_owned
    ));
    push_line(out, depth, &line);
    for a in &tr.args {
        dump_type_ref(out, depth + 1, a, "arg", None);
    }
    for (i, p) in tr.fn_params.iter().enumerate() {
        let e = tr.fn_param_effects.get(i).copied().flatten();
        dump_type_ref(out, depth + 1, p, "fn-param", e);
    }
    if let Some(r) = &tr.fn_return {
        dump_type_ref(out, depth + 1, r, "fn-return", None);
    }
}

fn dump_block(out: &mut String, depth: usize, b: &ast::Block) {
    push_line(out, depth, "block");
    for s in &b.statements {
        dump_stmt(out, depth + 1, s);
    }
}

fn dump_match(
    out: &mut String,
    depth: usize,
    value: &ast::Expr,
    eff: Option<ast::DataEffect>,
    arms: &[ast::MatchArm],
    malformed_arms: usize,
    tag: &str,
) {
    let mut line = String::from(tag);
    if let Some(e) = eff {
        line.push_str(&format!(" effect={}", e.as_str()));
    }
    push_line(out, depth, &line);
    push_line(out, depth + 1, "value");
    dump_expr(out, depth + 2, value);
    for arm in arms {
        push_line(out, depth + 1, "arm");
        push_line(out, depth + 2, "pattern");
        dump_pattern(out, depth + 3, &arm.pattern);
        if let Some(g) = &arm.guard {
            push_line(out, depth + 2, "guard");
            dump_expr(out, depth + 3, g);
        }
        dump_block(out, depth + 2, &arm.body);
    }
    for _ in 0..malformed_arms {
        push_line(out, depth + 1, "malformed-arm");
    }
}

fn dump_stmt(out: &mut String, depth: usize, s: &ast::Stmt) {
    match s {
        ast::Stmt::Let(l) => {
            let mut line = format!(
                "let kind={} name={} mut={} async={}",
                let_kind_str(l.kind),
                l.name,
                l.is_mut,
                l.is_async
            );
            if l.malformed {
                line.push_str(" malformed=true");
            }
            if let Some(names) = &l.destructure {
                line.push_str(&format!(" destructure={}", names.join(",")));
            }
            push_line(out, depth, &line);
            if let Some(t) = &l.type_annotation {
                dump_type_ref(out, depth + 1, t, "type", None);
            }
            if let Some(v) = &l.value {
                push_line(out, depth + 1, "value");
                dump_expr(out, depth + 2, v);
            }
        }
        ast::Stmt::Return(r) => {
            push_line(out, depth, "return");
            if let Some(v) = &r.value {
                push_line(out, depth + 1, "value");
                dump_expr(out, depth + 2, v);
            }
        }
        ast::Stmt::With(w) => {
            push_line(out, depth, &format!("with binding={}", w.binding));
            push_line(out, depth + 1, "resource");
            dump_expr(out, depth + 2, &w.resource);
            dump_block(out, depth + 1, &w.body);
        }
        ast::Stmt::MalformedWith(_) => push_line(out, depth, "malformed-with"),
        ast::Stmt::If(i) => {
            push_line(out, depth, "if");
            push_line(out, depth + 1, "cond");
            dump_expr(out, depth + 2, &i.condition);
            push_line(out, depth + 1, "then");
            dump_block(out, depth + 2, &i.then_body);
            if let Some(e) = &i.else_body {
                push_line(out, depth + 1, "else");
                dump_block(out, depth + 2, e);
            }
        }
        ast::Stmt::MalformedIf(_) => push_line(out, depth, "malformed-if"),
        ast::Stmt::Loop(l) => {
            push_line(out, depth, "loop");
            if let Some(c) = &l.condition {
                push_line(out, depth + 1, "cond");
                dump_expr(out, depth + 2, c);
            }
            dump_block(out, depth + 1, &l.body);
        }
        ast::Stmt::MalformedLoop(_) => push_line(out, depth, "malformed-loop"),
        ast::Stmt::For(f) => {
            push_line(
                out,
                depth,
                &format!("for binding={} async={}", f.binding, f.is_async),
            );
            push_line(out, depth + 1, "iter");
            dump_expr(out, depth + 2, &f.iterable);
            dump_block(out, depth + 1, &f.body);
        }
        ast::Stmt::MalformedFor(_) => push_line(out, depth, "malformed-for"),
        ast::Stmt::Match(m) => dump_match(
            out,
            depth,
            &m.value,
            m.scrutinee_effect,
            &m.arms,
            m.malformed_arm_spans.len(),
            "match",
        ),
        ast::Stmt::MalformedMatch(_) => push_line(out, depth, "malformed-match"),
        ast::Stmt::TaskGroup(t) => {
            push_line(out, depth, "task-group");
            dump_block(out, depth + 1, &t.body);
        }
        ast::Stmt::Select(s) => {
            push_line(out, depth, "select");
            for arm in &s.arms {
                push_line(out, depth + 1, &format!("select-arm binding={}", arm.binding));
                push_line(out, depth + 2, "operation");
                dump_expr(out, depth + 3, &arm.operation);
                dump_block(out, depth + 2, &arm.body);
            }
        }
        ast::Stmt::Break(_) => push_line(out, depth, "break"),
        ast::Stmt::Continue(_) => push_line(out, depth, "continue"),
        ast::Stmt::LetElse(l) => {
            push_line(out, depth, &format!("let-else binding={}", l.binding_name));
            push_line(out, depth + 1, "pattern");
            dump_pattern(out, depth + 2, &l.pattern);
            push_line(out, depth + 1, "value");
            dump_expr(out, depth + 2, &l.value);
            push_line(out, depth + 1, "else");
            dump_block(out, depth + 2, &l.else_body);
        }
        ast::Stmt::Assign(a) => {
            push_line(out, depth, "assign");
            push_line(out, depth + 1, "target");
            dump_expr(out, depth + 2, &a.target);
            push_line(out, depth + 1, "value");
            dump_expr(out, depth + 2, &a.value);
        }
        ast::Stmt::Expr(e) => {
            push_line(out, depth, "expr-stmt");
            dump_expr(out, depth + 1, e);
        }
        ast::Stmt::Unknown(_) => push_line(out, depth, "unknown-stmt"),
    }
}

fn dump_pattern(out: &mut String, depth: usize, p: &ast::MatchPattern) {
    match p {
        ast::MatchPattern::Binding { name, .. } => {
            push_line(out, depth, &format!("pat-binding name={name}"));
        }
        ast::MatchPattern::Variant { name, bindings, .. } => {
            push_line(out, depth, &format!("pat-variant name={name}"));
            for b in bindings {
                dump_pattern(out, depth + 1, b);
            }
        }
        ast::MatchPattern::Struct {
            name,
            fields,
            has_rest,
            ..
        } => {
            push_line(
                out,
                depth,
                &format!("pat-struct name={name} rest={has_rest}"),
            );
            for f in fields {
                let mut line = format!("pat-field name={} ignored={}", f.name, f.ignored);
                if let Some(b) = &f.binding {
                    line.push_str(&format!(" binding={b}"));
                }
                if let Some(e) = f.effect {
                    line.push_str(&format!(" effect={}", e.as_str()));
                }
                push_line(out, depth + 1, &line);
                if let Some(sub) = &f.pattern {
                    dump_pattern(out, depth + 2, sub);
                }
            }
        }
        ast::MatchPattern::Literal { value, .. } => {
            let (kind, payload) = match value {
                ast::MatchLiteral::Int(s) => ("int", escape(s)),
                ast::MatchLiteral::String(s) => ("string", escape(s)),
                ast::MatchLiteral::Char(s) => ("char", escape(s)),
                ast::MatchLiteral::Bool(b) => ("bool", b.to_string()),
            };
            push_line(out, depth, &format!("pat-literal kind={kind} {payload}"));
        }
        ast::MatchPattern::List {
            prefix,
            rest,
            suffix,
            ..
        } => {
            let rest_s = match rest {
                None => "none".to_string(),
                Some(None) => "ignore".to_string(),
                Some(Some(n)) => n.clone(),
            };
            push_line(out, depth, &format!("pat-list rest={rest_s}"));
            if !prefix.is_empty() {
                push_line(out, depth + 1, "list-prefix");
                for pp in prefix {
                    dump_pattern(out, depth + 2, pp);
                }
            }
            if !suffix.is_empty() {
                push_line(out, depth + 1, "list-suffix");
                for pp in suffix {
                    dump_pattern(out, depth + 2, pp);
                }
            }
        }
        ast::MatchPattern::Wildcard(_) => push_line(out, depth, "pat-wildcard"),
    }
}

fn dump_callee(out: &mut String, depth: usize, c: &ast::Callee) {
    match c {
        ast::Callee::Name(n) => push_line(out, depth, &format!("callee-name name={n}")),
        ast::Callee::Qualified { namespace, name } => push_line(
            out,
            depth,
            &format!("callee-qualified namespace={namespace} name={name}"),
        ),
        ast::Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => {
            let mut line = format!("callee-receiver method={method}");
            if let Some(e) = effect {
                line.push_str(&format!(" effect={}", e.as_str()));
            }
            push_line(out, depth, &line);
            dump_expr(out, depth + 1, receiver);
        }
    }
}

fn dump_expr(out: &mut String, depth: usize, e: &ast::Expr) {
    match e {
        ast::Expr::Ident(s, _) => push_line(out, depth, &format!("ident {}", escape(s))),
        ast::Expr::Number(s, _) => push_line(out, depth, &format!("number {}", escape(s))),
        ast::Expr::String(s, _) => push_line(out, depth, &format!("string {}", escape(s))),
        ast::Expr::CharLiteral(s, _) => push_line(out, depth, &format!("char {}", escape(s))),
        ast::Expr::MultilineString(s, _) => {
            push_line(out, depth, &format!("multiline {}", escape(s)))
        }
        ast::Expr::ObjectLiteral { fields, .. } => {
            push_line(out, depth, "object");
            for f in fields {
                push_line(out, depth + 1, &format!("object-field name={}", f.name));
                dump_expr(out, depth + 2, &f.value);
            }
        }
        ast::Expr::MapLiteral { entries, .. } => {
            push_line(out, depth, "map");
            for en in entries {
                push_line(out, depth + 1, "map-entry");
                push_line(out, depth + 2, "key");
                dump_expr(out, depth + 3, &en.key);
                push_line(out, depth + 2, "value");
                dump_expr(out, depth + 3, &en.value);
            }
        }
        ast::Expr::ArrayLiteral { items, .. } => {
            push_line(out, depth, "array");
            for it in items {
                dump_expr(out, depth + 1, it);
            }
        }
        ast::Expr::Binary {
            op, left, right, ..
        } => {
            push_line(out, depth, &format!("binary op={}", binop_name(*op)));
            dump_expr(out, depth + 1, left);
            dump_expr(out, depth + 1, right);
        }
        ast::Expr::Field { base, name, .. } => {
            push_line(out, depth, &format!("field-access name={name}"));
            dump_expr(out, depth + 1, base);
        }
        ast::Expr::Index { base, index, .. } => {
            push_line(out, depth, "index");
            dump_expr(out, depth + 1, base);
            dump_expr(out, depth + 1, index);
        }
        ast::Expr::Call { callee, args, .. } => {
            push_line(out, depth, "call");
            dump_callee(out, depth + 1, callee);
            for a in args {
                let mut line = String::from("arg");
                if let Some(n) = &a.name {
                    line.push_str(&format!(" name={n}"));
                }
                if a.malformed {
                    line.push_str(" malformed=true");
                }
                push_line(out, depth + 1, &line);
                dump_expr(out, depth + 2, &a.value);
            }
        }
        ast::Expr::Effect { effect, value, .. } => {
            push_line(out, depth, &format!("effect kind={}", effect.as_str()));
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Manage { value, .. } => {
            push_line(out, depth, "manage");
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Spawn { value, .. } => {
            push_line(out, depth, "spawn");
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Await { value, .. } => {
            push_line(out, depth, "await");
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Try { value, .. } => {
            push_line(out, depth, "try");
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Closure {
            params,
            captures,
            declared_effects,
            explicit,
            body,
            ..
        } => {
            push_line(out, depth, &format!("closure explicit={explicit}"));
            for p in params {
                push_line(out, depth + 1, &format!("closure-param {p}"));
            }
            for c in captures {
                push_line(
                    out,
                    depth + 1,
                    &format!("capture effect={} name={}", c.effect.as_str(), c.name),
                );
            }
            for d in declared_effects {
                push_line(out, depth + 1, &format!("declared-effect {d}"));
            }
            push_line(out, depth + 1, "body");
            dump_block(out, depth + 2, body);
        }
        ast::Expr::Match {
            value,
            scrutinee_effect,
            arms,
            malformed_arm_spans,
            ..
        } => dump_match(
            out,
            depth,
            value,
            *scrutinee_effect,
            arms,
            malformed_arm_spans.len(),
            "match-expr",
        ),
        ast::Expr::Unknown(_) => push_line(out, depth, "unknown-expr"),
    }
}

/// Phase-5 proof (non-ignored): the AST oracle is deterministic and total —
/// dumping the tiny sample twice is identical, non-empty, and the serializer
/// panics on no node (totality is exercised more broadly by the corpus test).
#[test]
fn ast_oracle_dump_is_deterministic_smoke() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let a = ast_oracle_dump("samples/tiny.rss", &source);
    let b = ast_oracle_dump("samples/tiny.rss", &source);
    assert_eq!(a, b, "AST oracle dump must be deterministic");
    assert!(!a.is_empty(), "AST oracle dump must be non-empty");
    assert!(a.starts_with("program\n"), "dump must start with `program`");
}

/// Phase-5 golden (non-ignored): pins the exact AST dump of the tiny sample so
/// the format contract in `selfhost/AST_FORMAT.md` is locked BEFORE `parser.rss`
/// targets it. When the rss parser is built, its dump must equal this byte for
/// byte at tier 0.
#[test]
fn ast_oracle_dump_tiny_sample_golden() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let dump = ast_oracle_dump("samples/tiny.rss", &source);
    let expected = "\
program
  fn name=add public=false async=false native=false has-body=true
    param name=x
      type name=Int fresh=false noescape=false owned=false
    return-type name=Int fresh=false noescape=false owned=false
    body
      block
        return
          value
            binary op=add
              ident x
              number 1
";
    assert_eq!(dump, expected, "AST dump golden mismatch\n--- actual ---\n{dump}");
}

/// Phase-5 totality gate (ignored by default): the AST oracle renders every file
/// in the corpus without panicking and deterministically. This proves the
/// serializer is total over the real grammar — no unhandled node — which is the
/// precondition for it being a trustworthy parity oracle once `parser.rss` emits
/// the same dump.
#[test]
#[ignore]
fn ast_oracle_total_over_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root);
    let mut ok = 0usize;
    let mut empty: Vec<String> = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(&root).unwrap_or(file).display().to_string();
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        let a = ast_oracle_dump(&rel, &source);
        let b = ast_oracle_dump(&rel, &source);
        assert_eq!(a, b, "{rel}: AST oracle dump is non-deterministic");
        if a.trim().is_empty() || !a.starts_with("program\n") {
            empty.push(rel);
        } else {
            ok += 1;
        }
    }
    eprintln!(
        "\n=== ast_oracle_total_over_corpus ===\n  files: {}\n  ok: {ok}\n  degenerate: {}\n",
        files.len(),
        empty.len()
    );
    for line in empty.iter().take(20) {
        eprintln!("[degenerate] {line}");
    }
    assert!(empty.is_empty(), "{} files produced a degenerate dump", empty.len());
}

// ---------------------------------------------------------------------------
// AST-dump PARITY — the rss producer (`selfhost/astdump.rss`) vs the oracle.
//
// Step 2: the rss recursive-descent parser streams the canonical AST dump; the
// harness compares it byte-for-byte against `ast_oracle_dump`. Coverage is a
// growing core (see astdump.rss): the curated `samples/ast/*.rss` set is a
// non-ignored fast gate (also the risk mitigation for corpus-gate runtime — AST
// dumps are much larger than token dumps), while `ast_parity_corpus` measures
// unaided reach over all 556 files and ratchets a floor. Residual divergences
// are tracked as SH-025.
// ---------------------------------------------------------------------------

fn compile_astdump() -> Result<RegVmExecutable, String> {
    let combined = combined_tool_source("astdump.rss")?;
    reg_vm_compile_source("selfhost/astdump.rss", &combined)
        .map_err(|e| format!("rss astdump failed to compile: {e:?}"))
}

/// Run the precompiled rss AST-dump producer; its stdout IS the dump.
fn run_astdump(exe: &RegVmExecutable, source: &str) -> Result<String, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss astdump failed to run: {e:?}"))?;
    Ok(output.stdout)
}

/// Curated AST-parity samples under `selfhost/samples/ast/`, sorted for stable order.
fn ast_sample_files() -> Vec<PathBuf> {
    let dir = selfhost_dir().join("samples/ast");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "rss"))
        .collect();
    files.sort();
    files
}

/// Step-2 gate (non-ignored): the rss producer matches the oracle byte-for-byte
/// on the tiny sample — the end-to-end proof that the streaming producer, the
/// dump format, and the oracle all agree.
#[test]
fn ast_parity_tiny_sample() {
    let sample_path = selfhost_dir().join("samples/tiny.rss");
    let source = std::fs::read_to_string(&sample_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", sample_path.display()));
    let oracle = ast_oracle_dump("samples/tiny.rss", &source);
    let exe = compile_astdump().expect("rss astdump should compile");
    let actual = run_astdump(&exe, &source).expect("rss astdump should run");
    assert_eq!(
        actual, oracle,
        "AST parity mismatch on tiny.rss\n--- oracle ---\n{oracle}\n--- rss ---\n{actual}"
    );
}

/// Step-2 gate (non-ignored): the rss producer matches the oracle byte-for-byte
/// on every curated sample. This is the fast inner-loop gate; keep the samples
/// within the producer's supported core so it stays green as coverage grows.
#[test]
fn ast_parity_samples() {
    let exe = compile_astdump().expect("rss astdump should compile");
    let mut mismatches: Vec<String> = Vec::new();
    let files = ast_sample_files();
    // Guard against a vacuous pass: a missing/unreadable samples dir would make
    // `files` empty and this test green while covering nothing.
    assert!(
        files.len() >= AST_SAMPLE_MIN,
        "expected >= {AST_SAMPLE_MIN} curated AST samples under selfhost/samples/ast/, found {} \
         (missing or unreadable dir makes this test pass vacuously)",
        files.len()
    );
    for file in &files {
        let rel = file
            .strip_prefix(selfhost_dir())
            .unwrap_or(file)
            .display()
            .to_string();
        let source = std::fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
        let oracle = ast_oracle_dump(&rel, &source);
        match run_astdump(&exe, &source) {
            Err(e) => mismatches.push(format!("{rel}: run error: {e}")),
            Ok(actual) => {
                if actual != oracle {
                    mismatches.push(format!(
                        "{rel}: mismatch\n--- oracle ---\n{oracle}\n--- rss ---\n{actual}"
                    ));
                }
            }
        }
    }
    assert!(
        mismatches.is_empty(),
        "AST parity failed on {} of {} samples:\n{}",
        mismatches.len(),
        files.len(),
        mismatches.join("\n\n")
    );
}

/// Minimum number of curated AST samples that must exist (guards `ast_parity_samples`
/// against a vacuous pass if the samples dir goes missing).
const AST_SAMPLE_MIN: usize = 6;

/// Floor for `ast_parity_corpus` — the number of corpus files whose rss AST dump
/// already matches the oracle byte-for-byte. Ratchets up as the producer's
/// coverage grows; a drop signals a regression. (Full parity = files.len().)
const AST_CORPUS_PARITY_FLOOR: usize = 248;

/// Step-2 measurement gate (ignored by default): how many corpus files the rss
/// producer reproduces byte-for-byte. Not full parity yet — this ratchets a floor
/// so coverage can only grow, asserts the producer never crashes (0 run-failures),
/// and prints the current count so the residual (SH-025) is visible.
///
/// RUNTIME: this compiles the rss producer once and runs it over all ~560 corpus
/// files on the reg-VM; in a debug build that is slow (minutes). Run it in release
/// for a quick measurement:
/// `cargo test -p rsscript --release --lib selfhost_parity::ast_parity_corpus -- --ignored --nocapture`.
/// The fast inner-loop gate is `ast_parity_samples` (non-ignored, curated subset).
#[test]
#[ignore]
fn ast_parity_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root);
    let exe = compile_astdump().expect("rss astdump should compile");
    let mut ok = 0usize;
    let mut run_failures = 0usize;
    let mut sample_mismatches: Vec<String> = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(&root).unwrap_or(file).display().to_string();
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        let oracle = ast_oracle_dump(&rel, &source);
        match run_astdump(&exe, &source) {
            Err(_) => run_failures += 1,
            Ok(actual) => {
                if actual == oracle {
                    ok += 1;
                } else if sample_mismatches.len() < 10 {
                    sample_mismatches.push(rel);
                }
            }
        }
    }
    let total = files.len();
    eprintln!(
        "\n=== ast_parity_corpus ===\n  files: {total}\n  byte-exact: {ok}\n  \
         run-failures: {run_failures}\n  floor: {AST_CORPUS_PARITY_FLOOR}\n"
    );
    for rel in &sample_mismatches {
        eprintln!("[mismatch] {rel}");
    }
    // The producer must never crash on corpus input — unsupported constructs are
    // expected to mismatch (partial/`unknown-*` output), not error. A run-failure
    // is a real regression even if `ok` still clears the floor.
    assert_eq!(
        run_failures, 0,
        "rss AST producer had {run_failures} run-failures over {total} corpus files \
         (it must degrade to a mismatch, never crash)"
    );
    assert!(
        ok >= AST_CORPUS_PARITY_FLOOR,
        "AST corpus parity regressed: {ok} byte-exact < floor {AST_CORPUS_PARITY_FLOOR}"
    );
}
