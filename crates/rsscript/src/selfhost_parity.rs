//! Self-hosting stress-test harness (test-only).
//!
//! Runs the rss-written lexer (`selfhost/lexer.rss`) on rss source and compares
//! its canonical token dump against the real Rust lexer (`crate::lexer::lex`),
//! which defines truth. In-process: the corpus file's *content* is passed as
//! argv[0] to the rss program, whose stdout is the dump. See `docs/self-hosting.md`.
//!
//! No new public API and no new CLI: this module is `#[cfg(test)]` and reaches
//! the private `crate::lexer` and the VM entry point directly. Divergences are
//! recorded as `SH-NNN` entries in `docs/self-hosting.md`.

use std::collections::{BTreeSet, VecDeque};
use std::path::PathBuf;

use crate::diagnostic::SELFHOST_CHECKER_TARGET_CODES;
use crate::interface_metadata::{
    collect_interface_metadata, format_selfhost_interface_metadata_rss,
};
use crate::interfaces::default_interfaces;
use crate::lexer::{TokenKind, lex};
use crate::reg_vm::reg_vm_compile_sources;
use crate::syntax::ast::Item;
use crate::syntax::parse_source_raw;
use crate::{RegVmExecutable, Severity, analyze_source};

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

fn selfhost_import_to_tool(path: &[String], glob: bool) -> Option<String> {
    if path.len() >= 2 && path.first().is_some_and(|segment| segment == "selfhost") {
        let tool_path = if glob || path.len() == 2 {
            &path[1..]
        } else {
            &path[1..path.len() - 1]
        };
        if tool_path.len() == 1 && tool_path[0] == "interfaces" {
            return Some("generated/interface_metadata.rss".to_string());
        }
        Some(format!("{}.rss", tool_path.join("/")))
    } else {
        None
    }
}

fn generated_selfhost_source(tool: &str) -> Option<String> {
    if tool == "generated/interface_metadata.rss" {
        let interfaces = default_interfaces().collect::<Vec<_>>();
        let metadata = collect_interface_metadata(&interfaces);
        Some(format_selfhost_interface_metadata_rss(&metadata))
    } else {
        None
    }
}

fn selfhost_imports(file: &str, source: &str) -> Vec<String> {
    parse_source_raw(file, source)
        .items
        .into_iter()
        .filter_map(|item| match item {
            Item::Use(decl) => selfhost_import_to_tool(&decl.path, decl.glob),
            _ => None,
        })
        .collect()
}

#[test]
fn selfhost_import_resolution_maps_symbols_to_owning_tool() {
    assert_eq!(
        selfhost_import_to_tool(&["selfhost".into(), "scan".into()], false).as_deref(),
        Some("scan.rss")
    );
    assert_eq!(
        selfhost_import_to_tool(&["selfhost".into(), "scan".into(), "Tok".into()], false)
            .as_deref(),
        Some("scan.rss")
    );
    assert_eq!(
        selfhost_import_to_tool(&["selfhost".into(), "scan".into()], true).as_deref(),
        Some("scan.rss")
    );
    assert_eq!(
        selfhost_import_to_tool(
            &["selfhost".into(), "interfaces".into(), "Lookup".into()],
            false
        )
        .as_deref(),
        Some("generated/interface_metadata.rss")
    );
}

/// Read a self-hosted tool and its declared `use selfhost.*` dependencies as
/// separate VM sources. `use` is not a filesystem loader in RSS itself; the
/// test-only harness resolves local selfhost modules before calling the normal
/// multi-source VM compiler.
fn tool_sources(tool: &str) -> Result<Vec<(String, String)>, String> {
    let dir = selfhost_dir();
    let mut seen = BTreeSet::new();
    let mut queue = VecDeque::from([tool.to_string()]);
    let mut out = Vec::new();

    while let Some(current) = queue.pop_front() {
        if !seen.insert(current.clone()) {
            continue;
        }
        let source = if let Some(source) = generated_selfhost_source(&current) {
            source
        } else {
            let path = dir.join(&current);
            std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?
        };
        for import in selfhost_imports(&format!("selfhost/{current}"), &source) {
            queue.push_back(import);
        }
        out.push((format!("selfhost/{current}"), source));
    }

    Ok(out)
}

fn compile_selfhost_tool(tool: &str, label: &str) -> Result<RegVmExecutable, String> {
    let sources = tool_sources(tool)?;
    let source_refs = sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    reg_vm_compile_sources(&source_refs)
        .map_err(|e| format!("rss {label} failed to compile: {e:?}"))
}

/// Compile `selfhost/lexer.rss` with the shared scanner once for reuse.
fn compile_lexer() -> Result<RegVmExecutable, String> {
    compile_selfhost_tool("lexer.rss", "lexer")
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

fn is_corpus_excluded_dir(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name == "target"
        || name == ".git"
        || path
            .components()
            .any(|component| component.as_os_str() == ".claude")
}

#[test]
fn corpus_excludes_local_agent_worktrees() {
    assert!(is_corpus_excluded_dir(std::path::Path::new(
        "/repo/.claude/worktrees/review"
    )));
    assert!(!is_corpus_excluded_dir(std::path::Path::new(
        "/repo/tests/fixtures"
    )));
}

/// Recursively collect `*.rss` files under `root`, skipping build output and
/// local agent worktrees. The self-host corpus must be hermetic to this checkout;
/// mirrored worktrees under `.claude/` duplicate fixtures and make gate counts
/// depend on local tooling state.
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
                if is_corpus_excluded_dir(&path) {
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
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
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
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
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
    compile_selfhost_tool("parser.rss", "parser")
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
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
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

/// Dev-loop optimization: extra target codes from `RSS_CHECKER_EXTRA_CODES`
/// (comma-separated) are unioned into the target set at runtime.
/// `SELFHOST_CHECKER_TARGET_CODES` is compiled, so adding a code to it forces a
/// rebuild; `check.rss` is read from disk at runtime. While developing a new
/// code, wire it into `check.rss` and run
/// `RSS_CHECKER_EXTRA_CODES=RS0XXX cargo test … checker_parity_corpus` to
/// iterate without baking it into the target table yet.
fn extra_target_codes() -> &'static Vec<String> {
    static EXTRA: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    EXTRA.get_or_init(|| {
        std::env::var("RSS_CHECKER_EXTRA_CODES")
            .map(|s| {
                s.split(',')
                    .map(|c| c.trim().to_string())
                    .filter(|c| !c.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn is_target_code(code: &str) -> bool {
    SELFHOST_CHECKER_TARGET_CODES.contains(&code) || extra_target_codes().iter().any(|c| c == code)
}

#[test]
fn checker_target_codes_are_known_and_unique() {
    let mut seen = BTreeSet::new();
    for code in SELFHOST_CHECKER_TARGET_CODES {
        assert!(
            code.starts_with("RS") && code.len() == 6,
            "self-host checker target code must be an RS diagnostic code: {code}"
        );
        assert!(
            seen.insert(*code),
            "self-host checker target code must not be duplicated: {code}"
        );
    }
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
    compile_selfhost_tool("check.rss", "checker")
}

/// Run the rss checker; parse the target codes it reports (`CLEAN` => none).
fn run_checker(exe: &RegVmExecutable, source: &str) -> Result<Vec<String>, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss checker failed to run: {e:?}"))?;
    parse_checker_output(&output.stdout)
}

fn parse_checker_output(stdout: &str) -> Result<Vec<String>, String> {
    let mut codes = Vec::new();
    let mut clean = false;
    for line in stdout.lines() {
        let code = line.trim();
        if code.is_empty() {
            continue;
        }
        if code == "CLEAN" {
            clean = true;
        } else if is_target_code(code) {
            codes.push(code.to_string());
        } else {
            return Err(format!(
                "rss checker emitted an unknown diagnostic line: {line:?}"
            ));
        }
    }
    if clean && !codes.is_empty() {
        return Err("rss checker emitted CLEAN together with diagnostics".to_string());
    }
    codes.sort();
    codes.dedup();
    Ok(codes)
}

#[test]
fn checker_output_parser_rejects_unknown_lines() {
    assert_eq!(
        parse_checker_output("RS0005\nRS0207\n").unwrap(),
        vec!["RS0005".to_string(), "RS0207".to_string()]
    );
    assert!(parse_checker_output("debug\n").is_err());
    assert!(parse_checker_output("CLEAN\nRS0005\n").is_err());
}

#[test]
fn type_helpers_detect_prefixed_and_late_generic_args() {
    let mut sources = tool_sources("types.rss").expect("selfhost types deps should load");
    sources.push((
        "selfhost/type_helpers_test.rss".to_string(),
        r#"
module selfhost.type_helpers_test

use selfhost.types.*

fn main() -> Unit {
    if str_is_unresolved_generic(s: read "owned T") {
        Log.write(message: read "owned")
    }
    if str_is_unresolved_generic(s: read "Triple<Int, Int, T>") {
        Log.write(message: read "third")
    }
}
"#
        .to_string(),
    ));
    let source_refs = sources
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let exe = reg_vm_compile_sources(&source_refs).expect("type helper test should compile");
    let output = exe
        .eval_main_with_args(std::iter::empty::<String>())
        .expect("type helper test should run");
    assert_eq!(output.stdout.trim(), "owned\nthird");
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
    let source = "fn dup() -> Unit {\n    return Unit\n}\nfn dup() -> Unit {\n    return Unit\n}\n";
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
    let all_files = collect_rss_files(&root);
    // Slow-test gate. A handful of ~4k-line self-hosted tools (check.rss ~220KB,
    // astdump.rss ~180KB, …) dominate the wall time: the checker's per-file cost is
    // super-linear and the reg-VM is an interpreter, so those few files take minutes
    // EACH and no fan-out can split a single file. By default we skip files above a
    // byte threshold for a ~1-min iteration gate; set RSS_SELFHOST_FULL=1 for the
    // exhaustive run. The skipped files are logged (no silent truncation).
    let full = std::env::var("RSS_SELFHOST_FULL").is_ok();
    // Tightest inner loop: RSS_SELFHOST_DEV=1 runs only tests/fixtures/ (all small,
    // where nearly every oracle-positive lives) in ~10s. Use it while iterating a code;
    // fall back to the full FAST gate (615 files) before commit.
    let dev = std::env::var("RSS_SELFHOST_DEV").is_ok();
    const FAST_MAX_BYTES: u64 = 40_000;
    let (files, skipped): (Vec<_>, Vec<_>) = if full {
        (all_files, Vec::new())
    } else if dev {
        all_files
            .into_iter()
            .partition(|f| f.to_string_lossy().contains("/tests/fixtures/"))
    } else {
        all_files.into_iter().partition(|f| {
            std::fs::metadata(f)
                .map(|m| m.len() <= FAST_MAX_BYTES)
                .unwrap_or(true)
        })
    };
    if !full {
        let mode = if dev { "DEV (fixtures only)" } else { "FAST" };
        eprintln!(
            "[gate] {mode} mode ({} files; {} skipped — RSS_SELFHOST_FULL=1 for all)",
            files.len(),
            skipped.len()
        );
        if !dev {
            for f in &skipped {
                eprintln!(
                    "[gate] skipped (large): {}",
                    f.strip_prefix(&root).unwrap_or(f).display()
                );
            }
        }
    }
    let total = files.len();
    // Each file is independent, so fan the corpus out across cores. `RegVmExecutable`
    // holds an `Rc` (not `Sync`), so we can't share one exe across threads — instead
    // each worker compiles its own checker (cheap vs. hundreds of file runs) and
    // processes one chunk. Cuts the wall time from ~30 min to a few minutes.
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, total.max(1));
    // Work-stealing over a shared atomic cursor rather than static chunks: a few
    // files (the ~4k-line selfhost tools) are far slower than the rest, so static
    // chunking would leave one worker straggling while the others idle. Each worker
    // owns its own exe and pulls the next file index when free.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let (mut ok, mut run_failures, mut mismatches) = (0usize, Vec::new(), Vec::new());
    let partials: Vec<(usize, Vec<String>, Vec<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let (root, files, next) = (&root, &files, &next);
                scope.spawn(move || {
                    let exe = compile_checker().expect("rss checker should compile");
                    let mut ok = 0usize;
                    let mut run_failures: Vec<String> = Vec::new();
                    let mut mismatches: Vec<String> = Vec::new();
                    loop {
                        let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= files.len() {
                            break;
                        }
                        let file = &files[i];
                        let rel = file
                            .strip_prefix(root)
                            .unwrap_or(file)
                            .display()
                            .to_string();
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
                                    mismatches
                                        .push(format!("{rel}: oracle={oracle:?} rss={actual:?}"));
                                }
                            }
                        }
                    }
                    (ok, run_failures, mismatches)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    for (o, rf, mm) in partials {
        ok += o;
        run_failures.extend(rf);
        mismatches.extend(mm);
    }
    eprintln!(
        "\n=== checker_parity_corpus (codes {SELFHOST_CHECKER_TARGET_CODES:?}) ===\n  files: {total}\n  \
         ok: {ok}\n  run-failures: {}\n  code-mismatches: {}\n",
        run_failures.len(),
        mismatches.len()
    );
    for line in run_failures.iter().take(20) {
        eprintln!("[run-fail] {line}");
    }
    for line in mismatches.iter().take(100) {
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
// `docs/self-hosting.md`; this oracle emits it from the surface-preserving
// tree (`crate::syntax::parse_source_raw`, NOT the desugared `parse_source`).
// Byte-identical dumps = AST parity. This ships BEFORE `parser.rss` builds an
// AST, exactly as the token dump contract + oracle preceded the rss lexer.
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

/// AST-dump span tier from `RSS_SELFHOST_AST_TIER` (default 0). 0 = structure +
/// payload only (spans omitted); 1 = append ` @line:col` to every spanned node
/// head line; 2 = append ` @line:col:len`. Mirrors the lexer/parser tier ladders.
/// Cached once per process (a corpus run is single-tier).
fn ast_tier() -> u8 {
    static T: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *T.get_or_init(
        || match std::env::var("RSS_SELFHOST_AST_TIER").ok().as_deref() {
            Some("1") => 1,
            Some("2") => 2,
            _ => 0,
        },
    )
}

/// Span suffix for a node head line at the active AST tier (empty at tier 0).
fn sp(span: &crate::diagnostic::Span) -> String {
    match ast_tier() {
        1 => format!(" @{}:{}", span.line, span.column),
        2 => format!(" @{}:{}:{}", span.line, span.column, span.length),
        _ => String::new(),
    }
}

/// Push a spanned node head line: `content` plus the tier's span suffix.
fn push_node(out: &mut String, depth: usize, content: &str, span: &crate::diagnostic::Span) {
    let mut c = String::from(content);
    c.push_str(&sp(span));
    push_line(out, depth, &c);
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
            &format!(
                "protocol-impl protocol={} type={}",
                pi.protocol, pi.type_name
            ),
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
        push_line(
            &mut out,
            1,
            &format!("duplicate-feature {}", escape(&f.name)),
        );
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
            push_node(
                out,
                depth,
                &format!("module path={}", m.path.join(".")),
                &m.span,
            );
        }
        ast::Item::Use(u) => {
            let mut line = format!("use path={} glob={}", u.path.join("."), u.glob);
            if let Some(a) = &u.alias {
                line.push_str(&format!(" alias={a}"));
            }
            push_node(out, depth, &line, &u.span);
        }
        ast::Item::Type(t) => {
            push_node(
                out,
                depth,
                &format!(
                    "type kind={} name={} public={} opaque={}",
                    type_kind_str(t.kind),
                    t.name,
                    t.is_public,
                    t.is_opaque
                ),
                &t.span,
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
            push_node(
                out,
                depth,
                &format!("sum name={} public={}", s.name, s.is_public),
                &s.span,
            );
            for g in &s.type_params {
                dump_generic(out, depth + 1, g);
            }
            for d in &s.derives {
                push_line(out, depth + 1, &format!("derive {d}"));
            }
            for v in &s.variants {
                push_node(out, depth + 1, &format!("variant name={}", v.name), &v.span);
                for f in &v.fields {
                    dump_field(out, depth + 2, f);
                }
            }
        }
        ast::Item::TypeAlias(a) => {
            push_node(
                out,
                depth,
                &format!("type-alias name={} public={}", a.name, a.is_public),
                &a.span,
            );
            for g in &a.type_params {
                dump_generic(out, depth + 1, g);
            }
            dump_type_ref(out, depth + 1, &a.target, "target", None);
        }
        ast::Item::Const(c) => {
            push_node(
                out,
                depth,
                &format!("const name={} public={}", c.name, c.is_public),
                &c.span,
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
    push_node(out, depth, &line, &f.span);
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
    push_node(out, depth, &format!("generic name={}", g.name), &g.span);
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
    push_node(
        out,
        depth,
        &format!(
            "field name={} handle={} weak={}",
            f.name, f.is_handle, f.is_weak
        ),
        &f.span,
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
    push_node(out, depth, &line, &p.span);
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
    push_node(out, depth, &line, &tr.span);
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
    head_span: &crate::diagnostic::Span,
) {
    let mut line = String::from(tag);
    if let Some(e) = eff {
        line.push_str(&format!(" effect={}", e.as_str()));
    }
    push_node(out, depth, &line, head_span);
    push_line(out, depth + 1, "value");
    dump_expr(out, depth + 2, value);
    for arm in arms {
        push_node(out, depth + 1, "arm", &arm.span);
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
            push_node(out, depth, &line, &l.span);
            if let Some(t) = &l.type_annotation {
                dump_type_ref(out, depth + 1, t, "type", None);
            }
            if let Some(v) = &l.value {
                push_line(out, depth + 1, "value");
                dump_expr(out, depth + 2, v);
            }
        }
        ast::Stmt::Return(r) => {
            push_node(out, depth, "return", &r.span);
            if let Some(v) = &r.value {
                push_line(out, depth + 1, "value");
                dump_expr(out, depth + 2, v);
            }
        }
        ast::Stmt::With(w) => {
            push_node(out, depth, &format!("with binding={}", w.binding), &w.span);
            push_line(out, depth + 1, "resource");
            dump_expr(out, depth + 2, &w.resource);
            dump_block(out, depth + 1, &w.body);
        }
        ast::Stmt::MalformedWith(s) => push_node(out, depth, "malformed-with", s),
        ast::Stmt::If(i) => {
            push_node(out, depth, "if", &i.span);
            push_line(out, depth + 1, "cond");
            dump_expr(out, depth + 2, &i.condition);
            push_line(out, depth + 1, "then");
            dump_block(out, depth + 2, &i.then_body);
            if let Some(e) = &i.else_body {
                push_line(out, depth + 1, "else");
                dump_block(out, depth + 2, e);
            }
        }
        ast::Stmt::MalformedIf(s) => push_node(out, depth, "malformed-if", s),
        ast::Stmt::Loop(l) => {
            push_node(out, depth, "loop", &l.span);
            if let Some(c) = &l.condition {
                push_line(out, depth + 1, "cond");
                dump_expr(out, depth + 2, c);
            }
            dump_block(out, depth + 1, &l.body);
        }
        ast::Stmt::MalformedLoop(s) => push_node(out, depth, "malformed-loop", s),
        ast::Stmt::For(f) => {
            push_node(
                out,
                depth,
                &format!("for binding={} async={}", f.binding, f.is_async),
                &f.span,
            );
            push_line(out, depth + 1, "iter");
            dump_expr(out, depth + 2, &f.iterable);
            dump_block(out, depth + 1, &f.body);
        }
        ast::Stmt::MalformedFor(s) => push_node(out, depth, "malformed-for", s),
        ast::Stmt::Match(m) => dump_match(
            out,
            depth,
            &m.value,
            m.scrutinee_effect,
            &m.arms,
            m.malformed_arm_spans.len(),
            "match",
            &m.span,
        ),
        ast::Stmt::MalformedMatch(s) => push_node(out, depth, "malformed-match", s),
        ast::Stmt::TaskGroup(t) => {
            push_node(out, depth, "task-group", &t.span);
            dump_block(out, depth + 1, &t.body);
        }
        ast::Stmt::Select(s) => {
            push_node(out, depth, "select", &s.span);
            for arm in &s.arms {
                push_line(
                    out,
                    depth + 1,
                    &format!("select-arm binding={}", arm.binding),
                );
                push_line(out, depth + 2, "operation");
                dump_expr(out, depth + 3, &arm.operation);
                dump_block(out, depth + 2, &arm.body);
            }
        }
        ast::Stmt::Break(s) => push_node(out, depth, "break", s),
        ast::Stmt::Continue(s) => push_node(out, depth, "continue", s),
        ast::Stmt::LetElse(l) => {
            push_node(
                out,
                depth,
                &format!("let-else binding={}", l.binding_name),
                &l.span,
            );
            push_line(out, depth + 1, "pattern");
            dump_pattern(out, depth + 2, &l.pattern);
            push_line(out, depth + 1, "value");
            dump_expr(out, depth + 2, &l.value);
            push_line(out, depth + 1, "else");
            dump_block(out, depth + 2, &l.else_body);
        }
        ast::Stmt::Assign(a) => {
            push_node(out, depth, "assign", &a.span);
            push_line(out, depth + 1, "target");
            dump_expr(out, depth + 2, &a.target);
            push_line(out, depth + 1, "value");
            dump_expr(out, depth + 2, &a.value);
        }
        ast::Stmt::Expr(e) => {
            push_node(out, depth, "expr-stmt", e.span());
            dump_expr(out, depth + 1, e);
        }
        ast::Stmt::Unknown(s) => push_node(out, depth, "unknown-stmt", s),
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
    let es = e.span();
    match e {
        ast::Expr::Ident(s, _) => push_node(out, depth, &format!("ident {}", escape(s)), es),
        ast::Expr::Number(s, _) => push_node(out, depth, &format!("number {}", escape(s)), es),
        ast::Expr::String(s, _) => push_node(out, depth, &format!("string {}", escape(s)), es),
        ast::Expr::CharLiteral(s, _) => push_node(out, depth, &format!("char {}", escape(s)), es),
        ast::Expr::MultilineString(s, _) => {
            push_node(out, depth, &format!("multiline {}", escape(s)), es)
        }
        ast::Expr::ObjectLiteral { fields, .. } => {
            push_node(out, depth, "object", es);
            for f in fields {
                push_line(out, depth + 1, &format!("object-field name={}", f.name));
                dump_expr(out, depth + 2, &f.value);
            }
        }
        ast::Expr::MapLiteral { entries, .. } => {
            push_node(out, depth, "map", es);
            for en in entries {
                push_line(out, depth + 1, "map-entry");
                push_line(out, depth + 2, "key");
                dump_expr(out, depth + 3, &en.key);
                push_line(out, depth + 2, "value");
                dump_expr(out, depth + 3, &en.value);
            }
        }
        ast::Expr::ArrayLiteral { items, .. } => {
            push_node(out, depth, "array", es);
            for it in items {
                dump_expr(out, depth + 1, it);
            }
        }
        ast::Expr::Binary {
            op, left, right, ..
        } => {
            push_node(out, depth, &format!("binary op={}", binop_name(*op)), es);
            dump_expr(out, depth + 1, left);
            dump_expr(out, depth + 1, right);
        }
        ast::Expr::Field { base, name, .. } => {
            push_node(out, depth, &format!("field-access name={name}"), es);
            dump_expr(out, depth + 1, base);
        }
        ast::Expr::Index { base, index, .. } => {
            push_node(out, depth, "index", es);
            dump_expr(out, depth + 1, base);
            dump_expr(out, depth + 1, index);
        }
        ast::Expr::Call { callee, args, .. } => {
            push_node(out, depth, "call", es);
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
            push_node(out, depth, &format!("effect kind={}", effect.as_str()), es);
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Manage { value, .. } => {
            push_node(out, depth, "manage", es);
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Spawn { value, .. } => {
            push_node(out, depth, "spawn", es);
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Await { value, .. } => {
            push_node(out, depth, "await", es);
            dump_expr(out, depth + 1, value);
        }
        ast::Expr::Try { value, .. } => {
            push_node(out, depth, "try", es);
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
            push_node(out, depth, &format!("closure explicit={explicit}"), es);
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
            span,
        } => dump_match(
            out,
            depth,
            value,
            *scrutinee_effect,
            arms,
            malformed_arm_spans.len(),
            "match-expr",
            span,
        ),
        ast::Expr::Unknown(_) => push_node(out, depth, "unknown-expr", es),
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
/// the format contract in `docs/self-hosting.md` is locked BEFORE `parser.rss`
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
    assert_eq!(
        dump, expected,
        "AST dump golden mismatch\n--- actual ---\n{dump}"
    );
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
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
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
    assert!(
        empty.is_empty(),
        "{} files produced a degenerate dump",
        empty.len()
    );
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
    compile_selfhost_tool("astdump.rss", "astdump")
}

/// Run the precompiled rss AST-dump producer; its stdout IS the dump.
///
/// The producer always emits the RICHEST span suffix (` @line:col:len`) on every
/// node head (mirroring the lexer producer, which always emits `line:col:len`).
/// Here we project each line down to the active AST tier so the byte-exact
/// comparison against the (tier-gated) oracle holds: tier 0 drops the suffix
/// entirely, tier 1 keeps ` @line:col`, tier 2 keeps ` @line:col:len`. Lines
/// without a numeric ` @L:C:N` suffix (synthetic labels) are passed through.
fn run_astdump(exe: &RegVmExecutable, source: &str) -> Result<String, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss astdump failed to run: {e:?}"))?;
    let t = ast_tier();
    if t == 2 {
        return Ok(output.stdout);
    }
    let mut out = String::with_capacity(output.stdout.len());
    for line in output.stdout.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(b) => (b, "\n"),
            None => (line, ""),
        };
        out.push_str(&project_ast_line(body, t));
        out.push_str(nl);
    }
    Ok(out)
}

/// Project a producer line's ` @line:col:len` suffix down to `tier` (1 → keep
/// `line:col`, 0 → drop the suffix). Only a trailing, strictly-numeric
/// ` @d+:d+:d+` is treated as a span (so a payload containing `@` is untouched).
fn project_ast_line(line: &str, tier: u8) -> String {
    if let Some(at) = line.rfind(" @") {
        let suffix = &line[at + 2..];
        let parts: Vec<&str> = suffix.split(':').collect();
        if parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        {
            let head = &line[..at];
            return match tier {
                0 => head.to_string(),
                _ => format!("{head} @{}:{}", parts[0], parts[1]),
            };
        }
    }
    line.to_string()
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
const AST_CORPUS_PARITY_FLOOR: usize = 619;

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
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
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
                    let first_diff = oracle
                        .lines()
                        .zip(actual.lines())
                        .find(|(o, a)| o != a)
                        .map(|(o, a)| format!("\n    oracle: {o:?}\n    rss:    {a:?}"))
                        .unwrap_or_else(|| {
                            format!(
                                "\n    (line count: oracle {} vs rss {})",
                                oracle.lines().count(),
                                actual.lines().count()
                            )
                        });
                    sample_mismatches.push(format!("{rel}{first_diff}"));
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
