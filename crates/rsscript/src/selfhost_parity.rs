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
use std::path::{Path, PathBuf};

use crate::diagnostic::{SELFHOST_CHECKER_TARGET_CODES, code};
use crate::interface_metadata::{
    collect_interface_metadata, format_selfhost_interface_metadata_rss,
};
use crate::interfaces::default_interfaces;
use crate::lexer::{TokenKind, lex};
use crate::reg_vm::reg_vm_compile_sources;
use crate::syntax::ast::Item;
use crate::syntax::parse_source_raw;
use crate::{RegVmExecutable, Severity, analyze_source, review_package_dir};

/// One token in the canonical dump. `len` is a Unicode-scalar span length,
/// matching the Rust lexer spans and the RSS scanner's `String.chars` cursor.
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

fn env_tier_u8(var: &str, default: u8, allowed: &[u8]) -> u8 {
    let Some(value) = std::env::var(var).ok() else {
        return default;
    };
    let parsed = value
        .parse::<u8>()
        .unwrap_or_else(|_| panic!("{var} must be one of {allowed:?}, got {value:?}"));
    assert!(
        allowed.contains(&parsed),
        "{var} must be one of {allowed:?}, got {value:?}"
    );
    parsed
}

/// Comparison tier from `RSS_SELFHOST_TIER` (default 0). 0 = kind+payload,
/// 1 = +position, 2 = +Unicode-scalar span length.
fn tier() -> u8 {
    static T: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *T.get_or_init(|| env_tier_u8("RSS_SELFHOST_TIER", 0, &[0, 1, 2]))
}

fn env_flag_tier(var: &str) -> bool {
    match std::env::var(var).ok().as_deref() {
        Some("1") => true,
        Some("0") | None => false,
        Some(value) => panic!("{var} must be unset, 0, or 1, got {value:?}"),
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

/// Parse a `L:C:N\tKIND\tPAYLOAD` line into a token.
fn parse_line(line: &str) -> Option<CanonTok> {
    let parts = line.split('\t').collect::<Vec<_>>();
    if parts.len() != 3 {
        return None;
    }
    let pos = parts[0];
    let kind = parts[1].to_string();
    let payload = parts[2].to_string();
    if kind.is_empty() {
        return None;
    }
    let mut nums = pos.split(':');
    let line_no = nums.next()?.parse().ok()?;
    let col = nums.next()?.parse().ok()?;
    let len = nums.next()?.parse().ok()?;
    if nums.next().is_some() {
        return None;
    }
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

#[test]
fn lexer_output_parser_rejects_malformed_lines() {
    assert!(parse_line("1:2:3\tIdent\tname").is_some());
    assert!(parse_line("1:2\tIdent\tname").is_none());
    assert!(parse_line("1:2:x\tIdent\tname").is_none());
    assert!(parse_line("1:2:3:4\tIdent\tname").is_none());
    assert!(parse_line("1:2:3\tIdent\tname\textra").is_none());
    assert!(parse_line("1:2:3\t\tname").is_none());
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

#[test]
fn corpus_manifest_matches_discovery() {
    let root = workspace_root();
    let manifest_path = selfhost_dir().join("corpus.txt");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", manifest_path.display()));
    let mut expected = manifest
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    expected.sort();
    let mut actual = collect_rss_files(&root)
        .expect("corpus discovery should succeed")
        .into_iter()
        .map(|path| {
            path.strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<Vec<_>>();
    actual.sort();
    assert_eq!(
        actual, expected,
        "selfhost/corpus.txt is stale; update it when adding/removing corpus .rss files"
    );
}

/// Recursively collect `*.rss` files under `root`, skipping build output and
/// local agent worktrees. The self-host corpus must be hermetic to this checkout;
/// mirrored worktrees under `.claude/` duplicate fixtures and make gate counts
/// depend on local tooling state.
fn collect_rss_files(root: &std::path::Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry =
                entry.map_err(|e| format!("cannot read entry in {}: {e}", dir.display()))?;
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
    Ok(out)
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
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
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
        let source =
            std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
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
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
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
        let source =
            std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
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
    env_flag_tier("RSS_SELFHOST_PARSE_TIER")
}

fn compile_parser() -> Result<RegVmExecutable, String> {
    compile_selfhost_tool("parser.rss", "parser")
}

/// Run the precompiled rss parser; parse its verdict line.
fn run_parser(exe: &RegVmExecutable, source: &str) -> Result<Option<(usize, usize)>, String> {
    let output = exe
        .eval_main_with_args([source.to_string()])
        .map_err(|e| format!("rss parser failed to run: {e:?}"))?;
    parse_parser_output(&output.stdout)
}

fn parse_parser_output(stdout: &str) -> Result<Option<(usize, usize)>, String> {
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>();
    if lines.len() != 1 {
        return Err(format!(
            "rss parser must emit exactly one non-empty verdict line, got {}",
            lines.len()
        ));
    }
    let verdict = lines[0];
    if verdict == "OK" {
        Ok(None)
    } else if let Some(rest) = verdict.strip_prefix("ERR ") {
        let mut nums = rest.split_whitespace();
        let line = nums
            .next()
            .ok_or_else(|| format!("missing parser error line in verdict: {verdict:?}"))?
            .parse::<usize>()
            .map_err(|_| format!("invalid parser error line in verdict: {verdict:?}"))?;
        let col = nums
            .next()
            .ok_or_else(|| format!("missing parser error column in verdict: {verdict:?}"))?
            .parse::<usize>()
            .map_err(|_| format!("invalid parser error column in verdict: {verdict:?}"))?;
        if nums.next().is_some() || line == 0 || col == 0 {
            return Err(format!("invalid parser error verdict: {verdict:?}"));
        }
        Ok(Some((line, col)))
    } else {
        Err(format!("unrecognized parser verdict: {verdict:?}"))
    }
}

#[test]
fn parser_output_parser_rejects_malformed_verdicts() {
    assert_eq!(parse_parser_output("OK\n").unwrap(), None);
    assert_eq!(parse_parser_output("ERR 2 3\n").unwrap(), Some((2, 3)));
    assert!(parse_parser_output("").is_err());
    assert!(parse_parser_output("debug\nOK\n").is_err());
    assert!(parse_parser_output("ERR\n").is_err());
    assert!(parse_parser_output("ERR bad\n").is_err());
    assert!(parse_parser_output("ERR 0 3\n").is_err());
    assert!(parse_parser_output("ERR 2 3 4\n").is_err());
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

#[test]
fn selfhost_top_level_ast_outline_is_deterministic() {
    let source = r#"features: local
module demo.core
use demo.util.*
struct Boxed {
    value: Int
}
sum Resultish {
    Good
}
type Name = String
const LIMIT: Int = 3
fn run() -> Unit {
    return Unit
}
pub struct PublicBox {
    value: Int
}
async fn async_run() -> Unit {
    return Unit
}
fn effectful() -> Unit effects(noalloc, pure) {
    return Unit
}
pub native fn external<T: Display, U>(value: take List<String>, count: mut Int = 1) -> fresh Result<Int, String>
#lower_name("lowered_named")
pub fn pinned_name() -> Unit {
    return Unit
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "top-level AST outline")
        .expect("top-level AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("top-level AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "features\tfeatures\t1:1:8\n",
            "module\tdemo\t2:1:6\n",
            "use\t\t3:1:3\n",
            "type\tBoxed\t4:1:6\n",
            "sum\tResultish\t7:1:3\n",
            "type-alias\tName\t10:1:4\n",
            "const\tLIMIT\t11:1:5\n",
            "function\trun\t12:1:2\n",
            "  header\tpublic=false\tasync=false\tnative=false\tbody=true\treturn=Unit\n",
            "  stmt\treturn\t\t\tname\tUnit\n",
            "type\tPublicBox\t15:1:3\n",
            "function\tasync_run\t18:1:5\n",
            "  header\tpublic=false\tasync=true\tnative=false\tbody=true\treturn=Unit\n",
            "  stmt\treturn\t\t\tname\tUnit\n",
            "function\teffectful\t21:1:2\n",
            "  header\tpublic=false\tasync=false\tnative=false\tbody=true\treturn=Unit\n",
            "  effect\tnoalloc\t21:32:7\n",
            "  effect\tpure\t21:41:4\n",
            "  stmt\treturn\t\t\tname\tUnit\n",
            "function\texternal\t24:1:3\n",
            "  header\tpublic=true\tasync=false\tnative=true\tbody=false\treturn=fresh Result<Int, String>\n",
            "  generic\tT\tDisplay\n",
            "  generic\tU\t\n",
            "  param\tvalue\ttake\tList<String>\n",
            "  param\tcount\tmut\tInt\n",
            "  default\t24:82:1\n",
            "function\tpinned_name\t25:1:1\n",
            "  header\tpublic=true\tasync=false\tnative=false\tbody=true\treturn=Unit\n",
            "  stmt\treturn\t\t\tname\tUnit\n",
        )
    );
}

#[test]
fn selfhost_function_body_ast_outline_is_deterministic() {
    let source = r#"fn consume(value: read Int) -> Unit {
    return Unit
}

fn work() -> Unit {
    let item: Int = 1
    consume(
        value: item
    )
    if item == 1 {
        return item
    } else {
        return Unit
    }
    return item
}
"#;
    let exe = compile_selfhost_tool("serialize/outline.rss", "function-body AST outline")
        .expect("function-body AST outline should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("function-body AST outline should run");
    assert_eq!(
        output.stdout,
        concat!(
            "function\tconsume\t1:1:2\n",
            "  header\tpublic=false\tasync=false\tnative=false\tbody=true\treturn=Unit\n",
            "  param\tvalue\tread\tInt\n",
            "  stmt\treturn\t\t\tname\tUnit\n",
            "function\twork\t5:1:2\n",
            "  header\tpublic=false\tasync=false\tnative=false\tbody=true\treturn=Unit\n",
            "  stmt\tlet\titem\tInt\tliteral\t1\n",
            "  stmt\texpr\t\t\tcall\tconsume\n",
            "  stmt\tif\t\t\tbinary\t==\tthen=1\telse=1\n",
            "  stmt\treturn\t\t\tname\titem\n",
        )
    );
}

#[test]
fn selfhost_function_context_infers_core_body_types() {
    let source = r#"fn consume(value: read Int) -> String {
    return "ok"
}

fn work(input: read Int) -> Unit {
    let number = 1
    let text: String = consume(value: input)
    if number == input {
        return Unit
    }
    while false {
        return Unit
    }
    return Unit
}
"#;
    let exe = compile_selfhost_tool("serialize/type_outline.rss", "function-context probe")
        .expect("function-context probe should compile");
    let output = exe
        .eval_main_with_args([source.to_string()])
        .expect("function-context probe should run");
    assert_eq!(
        output.stdout,
        concat!(
            "consume\treturn\tString\n",
            "work\tlet\tInt\n",
            "work\tlet\tString\n",
            "work\tif\tBool\n",
            "work\twhile\tBool\n",
            "work\treturn\tUnit\n",
        )
    );
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
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
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
        let source =
            std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SelfhostDiagnosticRecord {
    code: String,
    line: usize,
    column: usize,
    length: usize,
}

fn checker_oracle_records(
    file: &str,
    source: &str,
    target_code: &str,
) -> Vec<SelfhostDiagnosticRecord> {
    let mut records = analyze_source(file, source)
        .into_iter()
        .filter(|diagnostic| {
            diagnostic.severity == Severity::Error && diagnostic.code == target_code
        })
        .map(|diagnostic| SelfhostDiagnosticRecord {
            code: diagnostic.code,
            line: diagnostic.span.line,
            column: diagnostic.span.column,
            length: diagnostic.span.length,
        })
        .collect::<Vec<_>>();
    records.sort();
    records
}

fn diagnostic_records_for_code(
    records: Vec<SelfhostDiagnosticRecord>,
    target_code: &str,
) -> Vec<SelfhostDiagnosticRecord> {
    records
        .into_iter()
        .filter(|record| record.code == target_code)
        .collect()
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
    let mut clean_count = 0usize;
    for line in stdout.lines() {
        let code = line.trim();
        if code.is_empty() {
            continue;
        }
        if code == "CLEAN" {
            clean_count += 1;
        } else if is_target_code(code) {
            codes.push(code.to_string());
        } else {
            return Err(format!(
                "rss checker emitted an unknown diagnostic line: {line:?}"
            ));
        }
    }
    if clean_count > 1 {
        return Err("rss checker emitted duplicate CLEAN verdicts".to_string());
    }
    if clean_count == 1 && !codes.is_empty() {
        return Err("rss checker emitted CLEAN together with diagnostics".to_string());
    }
    if clean_count == 0 && codes.is_empty() {
        return Err("rss checker emitted no verdict".to_string());
    }
    codes.sort();
    codes.dedup();
    Ok(codes)
}

fn parse_checker_records(stdout: &str) -> Result<Vec<SelfhostDiagnosticRecord>, String> {
    let mut records = Vec::new();
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.as_slice() == ["CLEAN"] {
        return Ok(Vec::new());
    }
    if lines.iter().any(|line| *line == "CLEAN") {
        return Err("rss checker emitted CLEAN together with structured diagnostics".to_string());
    }
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        let [code, line, column, length] = fields.as_slice() else {
            return Err(format!("malformed structured diagnostic: {line:?}"));
        };
        if !is_target_code(code) {
            return Err(format!("unknown structured diagnostic code: {code:?}"));
        }
        let parse_number = |name: &str, value: &str| {
            value
                .parse::<usize>()
                .map_err(|_| format!("invalid diagnostic {name}: {value:?}"))
        };
        records.push(SelfhostDiagnosticRecord {
            code: (*code).to_string(),
            line: parse_number("line", line)?,
            column: parse_number("column", column)?,
            length: parse_number("length", length)?,
        });
    }
    if records.is_empty() {
        return Err("rss checker emitted no structured diagnostics".to_string());
    }
    records.sort();
    Ok(records)
}

type CheckerWorkerResponse = Result<String, String>;
type CheckerWorkerRequest = (String, std::sync::mpsc::Sender<CheckerWorkerResponse>);

/// Compile the large self-hosted checker once and keep both compilation and
/// execution on one worker thread. `RegVmExecutable` owns an `Rc<RegUnit>`, so
/// it cannot live in a process-global `Sync` cache or cross thread boundaries.
fn run_cached_checker_records(source: &str) -> Result<Vec<SelfhostDiagnosticRecord>, String> {
    static WORKER: std::sync::OnceLock<std::sync::mpsc::Sender<CheckerWorkerRequest>> =
        std::sync::OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (request_tx, request_rx) = std::sync::mpsc::channel::<CheckerWorkerRequest>();
        std::thread::Builder::new()
            .name("selfhost-checker".to_string())
            .spawn(move || {
                let checker = compile_checker();
                for (source, response_tx) in request_rx {
                    let response = match &checker {
                        Ok(exe) => exe
                            .eval_main_with_args([source, "records".to_string()])
                            .map(|output| output.stdout)
                            .map_err(|e| format!("rss structured checker failed to run: {e:?}")),
                        Err(error) => Err(error.clone()),
                    };
                    let _ = response_tx.send(response);
                }
            })
            .expect("self-host checker worker should start");
        request_tx
    });
    let (response_tx, response_rx) = std::sync::mpsc::channel();
    worker
        .send((source.to_string(), response_tx))
        .map_err(|_| "self-host checker worker stopped".to_string())?;
    let stdout = response_rx
        .recv()
        .map_err(|_| "self-host checker worker returned no result".to_string())??;
    parse_checker_records(&stdout)
}

#[test]
fn checker_output_parser_rejects_unknown_lines() {
    assert_eq!(
        parse_checker_output("RS0005\nRS0207\n").unwrap(),
        vec!["RS0005".to_string(), "RS0207".to_string()]
    );
    assert!(parse_checker_output("debug\n").is_err());
    assert!(parse_checker_output("CLEAN\nRS0005\n").is_err());
    assert!(parse_checker_output("").is_err());
    assert!(parse_checker_output("  \n\t\n").is_err());
    assert!(parse_checker_output("CLEAN\nCLEAN\n").is_err());
}

#[test]
fn checker_record_parser_is_strict_and_preserves_duplicates() {
    let records = parse_checker_records("RS0005\t2\t1\t2\nRS0005\t2\t1\t2\n")
        .expect("valid records should parse");
    assert_eq!(
        records.len(),
        2,
        "structured parity must retain occurrences"
    );
    assert_eq!(parse_checker_records("CLEAN\n").unwrap(), Vec::new());
    assert!(parse_checker_records("").is_err());
    assert!(parse_checker_records("CLEAN\nRS0005\t2\t1\t2\n").is_err());
    assert!(parse_checker_records("RS0005\t2\t1\n").is_err());
    assert!(parse_checker_records("RS0005\ttwo\t1\t2\n").is_err());
    assert!(parse_checker_records("RS9999\t2\t1\t2\n").is_err());
}

#[test]
fn checker_structured_clean_verdict() {
    let source = "fn clean(value: Int) -> Int {\n    return value\n}\n";
    let actual = run_cached_checker_records(source).expect("rss checker should emit records");
    assert!(
        actual.is_empty(),
        "clean source emitted records: {actual:?}"
    );
}

#[test]
fn checker_rs0005_structured_multiset_parity() {
    let source = r#"struct Response {
    status: Int
    status: String
}

struct Response {
    body: String
}

fn render() -> Unit {
    return Unit
}

fn render() -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0005.rss", source, "RS0005");
    assert!(
        oracle.len() > 1,
        "fixture must exercise duplicate occurrences"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0005",
    );
    assert_eq!(oracle, actual, "RS0005 structured diagnostics diverged");
}

#[test]
fn checker_rs0002_structured_multiset_parity() {
    let source = r#"fn first() {
    return Unit
}

fn second() {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0002.rss", source, "RS0002");
    assert_eq!(oracle.len(), 2, "fixture must exercise both functions");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0002",
    );
    assert_eq!(oracle, actual, "RS0002 structured diagnostics diverged");
}

#[test]
fn checker_rs0003_structured_multiset_parity() {
    let source = r#"fn combine(first, second, typed: Int) -> Unit {
    return Unit
}

"#;
    let oracle = checker_oracle_records("structured-rs0003.rss", source, "RS0003");
    assert_eq!(oracle.len(), 2, "fixture must exercise both parameters");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0003",
    );
    assert_eq!(oracle, actual, "RS0003 structured diagnostics diverged");
}

#[test]
fn checker_rs0004_structured_multiset_parity() {
    let source = r#"fn work() -> Unit
    effects(mystery, fresh)
{
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0004.rss", source, "RS0004");
    assert_eq!(oracle.len(), 2, "fixture must exercise both effects");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0004",
    );
    assert_eq!(oracle, actual, "RS0004 structured diagnostics diverged");
}

#[test]
fn checker_rs0006_structured_multiset_parity() {
    let source = "features: local\nfeatures: async\nfeatures: native\n";
    let oracle = checker_oracle_records("structured-rs0006.rss", source, "RS0006");
    assert_eq!(oracle.len(), 2, "fixture must exercise both extra headers");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0006",
    );
    assert_eq!(oracle, actual, "RS0006 structured diagnostics diverged");
}

#[test]
fn checker_rs0008_structured_multiset_parity() {
    let source = r#"fn combine(first: String, second: List<Int>) -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0008.rss", source, "RS0008");
    assert_eq!(oracle.len(), 2, "fixture must exercise both parameters");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0008",
    );
    assert_eq!(oracle, actual, "RS0008 structured diagnostics diverged");
}

#[test]
fn checker_rs0009_structured_multiset_parity() {
    let source = r#"features: local

resource File {
    fd: Int

    drop {
        Log.write(message: read "close")
    }
}

struct Image {
    value: Int
}

fn helper() -> Unit {
    return Unit
}

fn inspect(
    changed: mut Image,
    consumed: take Image,
    first: read String,
    second: read String,
    file: read File
) -> File
    effects(pure, retains(first), retains(second))
{
    with file as opened {
        helper()
    }
    local image = Image(value: 1)
    let shared = manage image
    return File(fd: 1)
}
"#;
    let oracle = checker_oracle_records("structured-rs0009.rss", source, "RS0009");
    assert_eq!(
        oracle.len(),
        8,
        "fixture must preserve every pure signature and body violation"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0009",
    );
    assert_eq!(oracle, actual, "RS0009 structured diagnostics diverged");
}

#[test]
fn checker_rs0007_structured_multiset_parity() {
    let source = r#"fn sample(count: Int, text: read String) -> Unit
    effects(retains(count), retains(missing))
{
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0007.rss", source, "RS0007");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both retains failures"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0007",
    );
    assert_eq!(oracle, actual, "RS0007 structured diagnostics diverged");
}

#[test]
fn checker_rs0010_structured_multiset_parity() {
    let source = "profile: managed\nprofile: managed\n";
    let oracle = checker_oracle_records("structured-rs0010.rss", source, "RS0010");
    assert_eq!(oracle.len(), 2, "fixture must exercise both profiles");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0010",
    );
    assert_eq!(oracle, actual, "RS0010 structured diagnostics diverged");
}

#[test]
fn checker_rs0012_structured_multiset_parity() {
    let source = r#"fn work() -> Unit
    effects(io, may_panic)
{
    return Unit
}

"#;
    let oracle = checker_oracle_records("structured-rs0012.rss", source, "RS0012");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both removed effects"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0012",
    );
    assert_eq!(oracle, actual, "RS0012 structured diagnostics diverged");
}

#[test]
fn checker_rs0013_structured_multiset_parity() {
    let source = r#"struct Image {
    value: Int
}

struct Config {
    value: Int
}

struct ConfigError {
    code: Int
}

struct AppError {
    code: Int
}

fn load_image() -> Image {
    return Image(value: 1)
}

fn load_config() -> Result<Config, ConfigError> {
    return Ok(Config(value: 1))
}

fn load_app() -> Result<Config, AppError> {
    return Ok(Config(value: 1))
}

fn scalar() -> Int {
    let first = load_image()?
    let second = load_image()?
    return 0
}

fn bad_value() -> Result<Image, AppError> {
    let image = load_image()?
    return Ok(image)
}

fn bad_error() -> Result<Config, AppError> {
    let config = load_config()?
    return Ok(config)
}

fn valid() -> Result<Config, AppError> {
    let config = load_app()?
    return Ok(config)
}
"#;
    let oracle = checker_oracle_records("structured-rs0013.rss", source, "RS0013");
    assert_eq!(
        oracle.len(),
        6,
        "fixture must preserve duplicate-span return/value failures and error mismatch"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0013",
    );
    assert_eq!(oracle, actual, "RS0013 structured diagnostics diverged");
}

#[test]
fn checker_rs0014_structured_multiset_parity() {
    let source = r#"features: local

struct Pair {
    left: Int
    right: Int
}

fn build() -> Pair
    effects(noalloc)
{
    local first = Pair(left: 1, right: 2)
    local second = Pair(left: 3, right: 4)
    return manage first
}
"#;
    let oracle = checker_oracle_records("structured-rs0014.rss", source, "RS0014");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve constructor and manage allocation sites"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0014",
    );
    assert_eq!(oracle, actual, "RS0014 structured diagnostics diverged");
}

#[test]
fn checker_rs0018_structured_multiset_parity() {
    let source = r#"fn may_block(value: Int) -> Int {
    return value
}

fn safe(value: Int) -> Int
    effects(no_block)
{
    return value
}

fn promised(value: Int) -> Int
    effects(no_block)
{
    let first = may_block(value: value)
    let second = may_block(value: first)
    return safe(value: second)
}
"#;
    let oracle = checker_oracle_records("structured-rs0018.rss", source, "RS0018");
    assert_eq!(oracle.len(), 2, "fixture must preserve both blocking calls");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0018",
    );
    assert_eq!(oracle, actual, "RS0018 structured diagnostics diverged");
}

#[test]
fn checker_rs0019_structured_multiset_parity() {
    let source = r#"fn may_panic(value: Int) -> Int {
    return value
}

fn safe(value: Int) -> Int
    effects(no_panic)
{
    return value
}

fn promised(value: Int) -> Int
    effects(no_panic)
{
    let first = may_panic(value: value)
    let second = may_panic(value: first)
    return safe(value: second)
}
"#;
    let oracle = checker_oracle_records("structured-rs0019.rss", source, "RS0019");
    assert_eq!(oracle.len(), 2, "fixture must preserve both panic calls");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0019",
    );
    assert_eq!(oracle, actual, "RS0019 structured diagnostics diverged");
}

#[test]
fn checker_rs0020_structured_multiset_parity() {
    let source = r#"sum Choice {
    One
}

struct Boxed {
    value: Int
}

fn first(value: read Int) -> Int {
    return value
}

fn second(value: read Int) -> Int {
    return value
}

fn allowed(value: read Int) -> Int effects(noalloc) {
    return value
}

fn Host.bad(value: read Int) -> Int {
    return value
}

fn Host.allowed(value: read Int) -> Int effects(noalloc) {
    return value
}

fn exercise(value: read Int) -> Int effects(noalloc) {
    let a = first(value: read value)
    let b = second(value: read a)
    let c = Host.bad(value: read b)
    let d = allowed(value: read c)
    let e = Host.allowed(value: read d)
    let variant = One
    let boxed = Boxed(value: e)
    return boxed.value
}
"#;
    let oracle = checker_oracle_records("structured-rs0020.rss", source, "RS0020");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve simple and qualified calls while exempting noalloc/variant/constructor"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0020",
    );
    assert_eq!(oracle, actual, "RS0020 structured diagnostics diverged");
}

#[test]
fn checker_rs0021_structured_multiset_parity() {
    let source = r#"fn statement_bad(value: read Option<Int>) -> Int {
    match value {
        Some(item) => return item
    }
}

fn expression_bad(name: read String) -> String {
    return match name {
        "read" => { "value" }
    }
}

fn exhaustive(value: read Option<Int>) -> Int {
    return match value {
        Some(item) => { item }
        None => { 0 }
    }
}
"#;
    let oracle = checker_oracle_records("structured-rs0021.rss", source, "RS0021");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve statement/expression mismatches and exempt exhaustive match"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0021",
    );
    assert_eq!(oracle, actual, "RS0021 structured diagnostics diverged");
}

#[test]
fn checker_rs0016_structured_multiset_parity() {
    let source = "features: mystery, other\n";
    let oracle = checker_oracle_records("structured-rs0016.rss", source, "RS0016");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both unknown features"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0016",
    );
    assert_eq!(oracle, actual, "RS0016 structured diagnostics diverged");
}

#[test]
fn checker_rs0017_structured_multiset_parity() {
    let source = "features: local, local, local\n";
    let oracle = checker_oracle_records("structured-rs0017.rss", source, "RS0017");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both duplicate features"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0017",
    );
    assert_eq!(oracle, actual, "RS0017 structured diagnostics diverged");
}

#[test]
fn checker_rs0011_structured_multiset_parity() {
    let source = r#"fn old(first: share String, second: share List<Int>) -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0011.rss", source, "RS0011");
    assert_eq!(oracle.len(), 2, "fixture must exercise both share types");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0011",
    );
    assert_eq!(oracle, actual, "RS0011 structured diagnostics diverged");
}

#[test]
fn checker_rs0028_structured_multiset_parity() {
    let source = r#"fn wrong(self: read String, other: Int, self: read String) -> Unit {
    return Unit
}

"#;
    let oracle = checker_oracle_records("structured-rs0028.rss", source, "RS0028");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both self parameters"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0028",
    );
    assert_eq!(oracle, actual, "RS0028 structured diagnostics diverged");
}

#[test]
fn checker_rs0029_structured_multiset_parity() {
    let source = r#"features: async

async fn fetch(value: read Int) -> Int {
    return value
}

fn exercise(value: read Int) -> Unit {
    let first = await fetch(value: read value)
    let second = await fetch(value: read first)
}

async fn valid(value: read Int) -> Int {
    return await fetch(value: read value)
}
"#;
    let oracle = checker_oracle_records("structured-rs0029.rss", source, "RS0029");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve two invalid awaits and exempt async functions"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0029",
    );
    assert_eq!(oracle, actual, "RS0029 structured diagnostics diverged");
}

#[test]
fn checker_rs0022_structured_multiset_parity() {
    let source = r#"features: async

async fn fetch(value: read Int) -> Int {
    return value
}

async fn exercise(value: read Int) -> Unit {
    let first = fetch(value: read value)
    fetch(value: read first)
    let consumed = await fetch(value: read value)
}
"#;
    let oracle = checker_oracle_records("structured-rs0022.rss", source, "RS0022");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve unconsumed calls and exempt await"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0022",
    );
    assert_eq!(oracle, actual, "RS0022 structured diagnostics diverged");
}

#[test]
fn checker_rs0023_structured_multiset_parity() {
    let source = r#"struct BadHandle {
    input: Fd
    output: Fd
}

resource AllowedHandle {
    fd: Fd

    drop {
        OS.close(fd: fd)
    }
}

native fn allowed(fd: Fd) -> Fd

fn exposed(first: Fd, second: Fd) -> Fd {
    return first
}
"#;
    let oracle = checker_oracle_records("structured-rs0023.rss", source, "RS0023");
    assert_eq!(
        oracle.len(),
        5,
        "fixture must preserve fields, parameters, and return surface failures"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0023",
    );
    assert_eq!(oracle, actual, "RS0023 structured diagnostics diverged");
}

#[test]
fn checker_rs0024_structured_multiset_parity() {
    let source = r#"struct Holder<T> {
    first: Missing
    second: List<Other>
    callback: Fn(Arg, T) -> ReturnMissing
}

fn exercise<T>(
    first: read UnknownParam,
    second: read Map<String, NestedUnknown>,
    known: read T,
    holder: read Holder<T>
) -> Result<UnknownReturn, ErrorUnknown> {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0024.rss", source, "RS0024");
    assert_eq!(
        oracle.len(),
        8,
        "fixture must preserve every unknown root while exempting generic and declared types"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0024",
    );
    assert_eq!(oracle, actual, "RS0024 structured diagnostics diverged");
}

#[test]
fn checker_rs0027_structured_multiset_parity() {
    let source = r#"struct Box<T: MissingBoxProtocol> {
    value: T
}

fn combine<A: MissingLeft, B: MissingRight>(left: read A, right: read B) -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0027.rss", source, "RS0027");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve type and function generic-bound failures"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0027",
    );
    assert_eq!(oracle, actual, "RS0027 structured diagnostics diverged");
}

#[test]
fn checker_rs0032_structured_multiset_parity() {
    let source = r#"features: local

struct Plain {
    value: Int
}

struct Hashable derives(Hash) {
    value: Int
}

struct Ordered derives(Ord) {
    value: Int
}

fn exercise(values: mut List<Plain>, ordered: mut List<Ordered>) -> Unit {
    let bad_set = Set.new<Plain>()
    let bad_map = Map.new<Plain, Int>()
    List.sort<Plain>(list: mut values)
    let good_set = Set.new<Hashable>()
    List.sort<Ordered>(list: mut ordered)
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0032.rss", source, "RS0032");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve Hashable/Ord failures and exempt derived implementations"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0032",
    );
    assert_eq!(oracle, actual, "RS0032 structured diagnostics diverged");
}

#[test]
fn checker_rs0033_structured_multiset_parity() {
    let source = r#"fn first() -> Int {
    return 9223372036854775808
}

fn second() -> Int {
    return 999999999999999999999999999999
}
"#;
    let oracle = checker_oracle_records("structured-rs0033.rss", source, "RS0033");
    assert_eq!(oracle.len(), 2, "fixture must exercise both integers");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0033",
    );
    assert_eq!(oracle, actual, "RS0033 structured diagnostics diverged");
}

#[test]
fn checker_rs0034_structured_multiset_parity() {
    let source = r#"fn main() -> Unit {
    let first = Ok(1)
    let second = Err("error")
    let third = None
    let used = Ok(2)
    let annotated: Result<Int, String> = Ok(3)
    let determined = Some(4)
    Log.debug(value: read used)
}
"#;
    let oracle = checker_oracle_records("structured-rs0034.rss", source, "RS0034");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise bare Ok, Err, and None"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0034",
    );
    assert_eq!(oracle, actual, "RS0034 structured diagnostics diverged");
}

#[test]
fn checker_rs0205_structured_multiset_parity() {
    let source = r#"fn target(a: Int, b: Int) -> Unit

fn exercise() -> Unit {
    target(a: 1, a: 2, a: 3, b: 4)
    target(a: 1, b: 2, b: 3)
    target(a: 1, b: 2)
}
"#;
    let oracle = checker_oracle_records("structured-rs0205.rss", source, "RS0205");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve every duplicate after the first argument"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0205",
    );
    assert_eq!(oracle, actual, "RS0205 structured diagnostics diverged");
}

#[test]
fn checker_rs0208_structured_multiset_parity() {
    let source = r#"class BuildError {
    code: Int
}

fn nested() -> Result<Option<String>, BuildError> {
    return Ok(Some(42))
}

fn direct() -> String {
    return 42
}

fn fallthrough() -> String {
    42
}
"#;
    let oracle = checker_oracle_records("structured-rs0208.rss", source, "RS0208");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise nested payload, direct return, and fallthrough anchors"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0208",
    );
    assert_eq!(oracle, actual, "RS0208 structured diagnostics diverged");
}

#[test]
fn checker_rs0210_structured_multiset_parity() {
    let source = r#"fn apply(callback: noescape Fn(Int) -> Bool) -> Unit {
    return Unit
}

fn direct() -> Unit {
    if 1 == "one" {
        return Unit
    }
    return Unit
}

fn callback() -> Unit {
    apply(callback: |value| value == "text")
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0210.rss", source, "RS0210");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise ordinary and callback operator spans"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0210",
    );
    assert_eq!(oracle, actual, "RS0210 structured diagnostics diverged");
}

#[test]
fn checker_rs0209_structured_multiset_parity() {
    let source = r#"fn maybe() -> Option<Int> {
    return Some(1)
}

fn conditions(value: read String) -> Unit {
    if value {
        return Unit
    }
    for item in value {
        Log.write(message: read "item")
    }
    return Unit
}

fn patterns() -> Unit {
    let value = maybe()
    match value {
        Ok(result) => return Unit
        Err(error) => return Unit
    }
    return Unit
}

"#;
    let oracle = checker_oracle_records("structured-rs0209.rss", source, "RS0209");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise condition, iterable, and both variant pattern occurrences"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0209",
    );
    assert_eq!(oracle, actual, "RS0209 structured diagnostics diverged");
}

#[test]
fn checker_rs0202_structured_multiset_parity() {
    let source = r#"sum Expr {
    Call(callee: String)
}

struct Item {
    name: String
}

struct Boxed {
    item: Item
}

fn Item.new() -> fresh Item {
    return Item(name: "item")
}

fn use_item(value: read Item) -> Unit {
    return Unit
}

fn Item.touch(self: mut Item, value: read String) -> Unit {
    return Unit
}

fn bad(expr: read Expr) -> Unit {
    let item = Item.new()
    use_item(value: item)
    let boxed = Boxed(item: read item)
    read item.touch(value: read "name")
    match expr {
        Call { callee } => return Unit
    }
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0202.rss", source, "RS0202");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise argument, constructor, receiver, and match-effect spans"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0202",
    );
    assert_eq!(oracle, actual, "RS0202 structured diagnostics diverged");
}

#[test]
fn checker_rs0207_structured_multiset_parity() {
    let source = r#"fn needs_text(value: read String) -> Unit {
    return Unit
}

fn bad() -> Unit {
    let value: String = 42
    needs_text(value: read 7)
    Log.write(message: read 9)
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0207.rss", source, "RS0207");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise annotated binding, same-file, and stdlib call argument anchors"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0207",
    );
    assert_eq!(oracle, actual, "RS0207 structured diagnostics diverged");
}

#[test]
fn checker_rs0035_structured_multiset_parity() {
    let source = r#"#lower_name("dup_symbol")
fn first() -> Unit {
    return Unit
}

#lower_name("dup_symbol")
fn second() -> Unit {
    return Unit
}

#lower_name("has-a-dash")
fn invalid() -> Unit {
    return Unit
}

#lower_name("plain")
fn pinned_plain() -> Unit {
    return Unit
}

fn plain() -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0035.rss", source, "RS0035");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve pin collision, invalid pin, and pinned/default collision"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0035",
    );
    assert_eq!(oracle, actual, "RS0035 structured diagnostics diverged");
}

#[test]
fn checker_rs0036_structured_multiset_parity() {
    let source = r#"features: async, native, local

async fn exercise() -> Result<Unit, ChannelError> {
    let first = Channel.message<List<Int>>(capacity: 4)?
    let second = Channel.message<Map<String, Int>>(capacity: 4)?
    let valid = Channel.message<String>(capacity: 4)?
    return Ok(Unit)
}

async fn generic<T>() -> Result<Unit, ChannelError> {
    let unresolved = Channel.message<T>(capacity: 4)?
    return Ok(Unit)
}
"#;
    let oracle = checker_oracle_records("structured-rs0036.rss", source, "RS0036");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve two non-transferable calls and exempt transferable/generic payloads"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0036",
    );
    assert_eq!(oracle, actual, "RS0036 structured diagnostics diverged");
}

#[test]
fn checker_rs0037_structured_multiset_parity() {
    let source = r#"sum Pairish {
    Pair(left: Int, right: Int)
    Empty
}

fn inspect(value: read Pairish) -> Int {
    match value {
        Pair(one) => return one
        Pair(one, two, three) => return one
        Pair(left, right) => return left + right
        Empty => return 0
    }
}
"#;
    let oracle = checker_oracle_records("structured-rs0037.rss", source, "RS0037");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve too-few and too-many positional bindings"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0037",
    );
    assert_eq!(oracle, actual, "RS0037 structured diagnostics diverged");
}

#[test]
fn checker_rs0101_structured_declaration_parity() {
    let source = r#"async fn async_entry() -> Unit {
    return Unit
}

fn unsafe_entry() -> Unit
    effects(unsafe)
{
    return Unit
}

fn native_effect_entry() -> Unit
    effects(native)
{
    return Unit
}

native fn native_missing_effect() -> Unit {
    return Unit
}

fn local_surface(pool: read ResourcePool<File>, value: take String) -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0101-declarations.rss", source, "RS0101");
    assert_eq!(
        oracle.len(),
        6,
        "fixture must preserve declaration, type, and parameter feature uses"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0101",
    );
    assert_eq!(oracle, actual, "RS0101 declaration diagnostics diverged");
}

#[test]
fn checker_rs0101_structured_body_parity() {
    let source = r#"async fn fetch() -> Int {
    return 1
}

fn dangerous() -> Unit
    effects(unsafe)
{
    return Unit
}

fn exercise() -> Unit {
    local value = 1
    let managed = manage value
    let first = await fetch()
    spawn fetch()
    dangerous()
}
"#;
    let oracle = checker_oracle_records("structured-rs0101-body.rss", source, "RS0101");
    assert_eq!(
        oracle.len(),
        9,
        "fixture must preserve nested local, async, and unsafe feature uses"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0101",
    );
    assert_eq!(oracle, actual, "RS0101 body diagnostics diverged");
}

#[test]
fn checker_rs0101_structured_feature_suppression_parity() {
    let source = r#"features: local, native, unsafe, async

async fn async_entry() -> Unit {
    return Unit
}

fn unsafe_entry() -> Unit effects(unsafe) {
    return Unit
}

fn native_entry(pool: read ResourcePool<File>, value: take String) -> Unit
    effects(native)
{
    return Unit
}

native fn still_missing_effect() -> Unit {
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0101-enabled.rss", source, "RS0101");
    assert_eq!(
        oracle.len(),
        1,
        "enabled features must suppress uses but not a native boundary missing its effect"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0101",
    );
    assert_eq!(oracle, actual, "RS0101 feature suppression diverged");
}

#[test]
fn checker_rs0101_structured_qualified_call_parity() {
    let source = r#"async fn Worker.fetch<T>(value: read T) -> T {
    return value
}

fn Host.danger() -> Unit effects(unsafe) {
    return Unit
}

fn exercise() -> Unit {
    let value = await Worker.fetch<Int>(value: read 1)
    Host.danger()
}
"#;
    let oracle = checker_oracle_records("structured-rs0101-qualified.rss", source, "RS0101");
    assert_eq!(
        oracle.len(),
        5,
        "fixture must preserve qualified declarations, await/call, and unsafe call uses"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0101",
    );
    assert_eq!(oracle, actual, "RS0101 qualified-call diagnostics diverged");
}

#[test]
fn checker_rs0201_structured_multiset_parity() {
    let source = r#"pub fn publish(first: Int, second: Int) -> Unit {
    return Unit
}

fn exercise() -> Unit {
    publish(1, 2)
    publish(first: 3, 4)
    publish(first: 5, second: 6)
}
"#;
    let oracle = checker_oracle_records("structured-rs0201.rss", source, "RS0201");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise each unnamed argument"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0201",
    );
    assert_eq!(oracle, actual, "RS0201 structured diagnostics diverged");
}

#[test]
fn checker_rs0212_structured_multiset_parity() {
    let source = r#"resource Connection derives(Clone, Eq, Hash) {
    id: Int
}
"#;
    let oracle = checker_oracle_records("structured-rs0212.rss", source, "RS0212");
    assert_eq!(oracle.len(), 3, "fixture must exercise all banned derives");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0212",
    );
    assert_eq!(oracle, actual, "RS0212 structured diagnostics diverged");
}

#[test]
fn checker_rs0211_structured_multiset_parity() {
    let source = r#"class Entity {
    value: Int
}

struct Bad derives(Eq, Hash) {
    score: Float
    target: handle Entity
}

struct DecodeBad derives(JsonDecode) {
    values: Map<Float, Int>
}

struct Good derives(Eq, Hash) {
    value: Int
}
"#;
    let oracle = checker_oracle_records("structured-rs0211.rss", source, "RS0211");
    assert_eq!(
        oracle.len(),
        5,
        "fixture must preserve per-field/per-derive violations and valid scalar fields"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0211",
    );
    assert_eq!(oracle, actual, "RS0211 structured diagnostics diverged");
}

#[test]
fn checker_rs0701_structured_multiset_parity() {
    let source = r#"resource Connection {
    id: Int
}

struct Holder {
    primary: Connection
    backup: Connection
}
"#;
    let oracle = checker_oracle_records("structured-rs0701.rss", source, "RS0701");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both resource fields"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0701",
    );
    assert_eq!(oracle, actual, "RS0701 structured diagnostics diverged");
}

#[test]
fn checker_rs0703_structured_multiset_parity() {
    let source = r#"features: local

struct Image {
    id: Int
}

fn typed(pool: mut ResourcePool<Image>) -> Unit {
    return Unit
}

fn generic<T: Managed>(pool: mut ResourcePool<T>) -> Unit {
    return Unit
}

fn constructed() -> Unit {
    local pool = ResourcePool<Image>.new(
        create: || Image(id: 1),
        max_size: 1,
    )
}
"#;
    let oracle = checker_oracle_records("structured-rs0703.rss", source, "RS0703");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise concrete, generic, and call-site pool types"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0703",
    );
    assert_eq!(oracle, actual, "RS0703 structured diagnostics diverged");
}

#[test]
fn checker_rs0704_structured_multiset_parity() {
    let source = r#"resource File {
    fd: Int

    drop {
        OS.close(fd: fd)
    }
}

struct Archive {
    files: List<File>
    backups: Option<File>
}

resource Unbounded<T> {
    id: Int

    drop {
        OS.close()
    }
}

resource Direct<T: Resource> {
    item: T

    drop {
        OS.close()
    }
}
"#;
    let oracle = checker_oracle_records("structured-rs0704.rss", source, "RS0704");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise resource arguments and declaration constraints"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0704",
    );
    assert_eq!(oracle, actual, "RS0704 structured diagnostics diverged");
}

#[test]
fn checker_rs0705_structured_multiset_parity() {
    let source = r#"features: local

resource Connection {
    id: Int
}

fn inspect(first: read ResourcePool<Connection>, second: read ResourcePool<Connection>) -> Unit {
    let managed = ResourcePool<Connection>.new(create: || Connection { id: 1 }, max_size: 1)
}

fn valid(pool: mut ResourcePool<Connection>, owned: take ResourcePool<Connection>) -> Unit {
    local direct = ResourcePool<Connection>.new(create: || Connection { id: 1 }, max_size: 1)
}
"#;
    let oracle = checker_oracle_records("structured-rs0705.rss", source, "RS0705");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise two parameters and one managed binding"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0705",
    );
    assert_eq!(oracle, actual, "RS0705 structured diagnostics diverged");
}

#[test]
fn checker_rs0708_structured_multiset_parity() {
    let source = r#"features: local

resource Connection {
    id: Int
}

const POOL_SIZE: Int = 2

fn zero() -> Unit {
    local pool = ResourcePool<Connection>.new(create: || Connection { id: 1 }, max_size: 0)
}

fn negative() -> Unit {
    local pool = ResourcePool<Connection>.new(create: || Connection { id: 1 }, max_size: -1)
}

fn dynamic(size: Int) -> Unit {
    local pool = ResourcePool<Connection>.new(create: || Connection { id: 1 }, max_size: size)
}

fn rebound_name_stays_moved() -> Unit {
    local literal = ResourcePool<Connection>.new(create: || Connection { id: 1 }, max_size: 1)
    local named = ResourcePool<Connection>.new(create: || Connection { id: 1 }, max_size: POOL_SIZE)
}
"#;
    let oracle = checker_oracle_records("structured-rs0708.rss", source, "RS0708");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise zero, negative, and dynamic sizes"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0708",
    );
    assert_eq!(oracle, actual, "RS0708 structured diagnostics diverged");
}

#[test]
fn checker_rs0706_structured_multiset_parity() {
    let source = r#"features: local

resource File {
    fd: Int
}

fn File.open_result(path: read Path) -> Result<File, IOError>
fn File.stat(file: read File) -> Unit

fn missing_first(path: read Path) -> Unit {
    with File.open_result(path: read path) as file {
        File.stat(file: read file)
    }
}

fn valid(path: read Path) -> Result<Unit, IOError> {
    with File.open_result(path: read path)? as file {
        File.stat(file: read file)
    }
    return Ok(Unit)
}

fn missing_second(path: read Path) -> Unit {
    with File.open_result(path: read path) as file {
        File.stat(file: read file)
    }
}
"#;
    let oracle = checker_oracle_records("structured-rs0706.rss", source, "RS0706");
    assert_eq!(oracle.len(), 2, "fixture must exercise both missing tries");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0706",
    );
    assert_eq!(oracle, actual, "RS0706 structured diagnostics diverged");
}

#[test]
fn checker_rs0707_structured_multiset_parity() {
    let source = r#"features: local

resource Connection {
    id: Int
}

fn Connection.open() -> Connection
fn Connection.try_open() -> Result<Connection, IOError>

fn bad_new() -> Unit {
    local pool = ResourcePool<Connection>.new(
        create: || Connection.try_open(),
        max_size: 1,
    )
}

fn bad_try_new() -> Result<Unit, IOError> {
    local pool = ResourcePool<Connection>.try_new(
        create: || Connection.open(),
        max_size: 1,
    )?
    return Ok(Unit)
}

fn valid() -> Result<Unit, IOError> {
    local direct = ResourcePool<Connection>.new(create: || Connection.open(), max_size: 1)
    local checked = ResourcePool<Connection>.try_new(create: || Connection.try_open(), max_size: 1)?
    return Ok(Unit)
}
"#;
    let oracle = checker_oracle_records("structured-rs0707.rss", source, "RS0707");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both constructor directions"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0707",
    );
    assert_eq!(oracle, actual, "RS0707 structured diagnostics diverged");
}

#[test]
fn checker_rs0709_structured_multiset_parity() {
    let source = r#"features: local

resource Connection {
    id: Int
}

fn Connection.open() -> Connection

fn exercise() -> Unit {
    local first = ResourcePool<Connection>.new(create: || Connection.open(), max_size: 2)
    local second = ResourcePool<Connection>.new(create: || Connection.open(), max_size: 1)

    with ResourcePool.borrow(pool: mut first) as lease {
        with ResourcePool.borrow(pool: mut first) as nested {
            Log.write(message: read "nested")
        }
        ResourcePool.reset(pool: mut first)
        let count = ResourcePool.stats(pool: read first)
        with ResourcePool.borrow(pool: mut second) as other {
            Log.write(message: read "different pool")
        }
    }
}
"#;
    let oracle = checker_oracle_records("structured-rs0709.rss", source, "RS0709");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise nested, mut, and read conflicts"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0709",
    );
    assert_eq!(oracle, actual, "RS0709 structured diagnostics diverged");
}

#[test]
fn checker_rs0710_structured_multiset_parity() {
    let source = r#"features: local

resource Connection {
    id: Int
}

fn Connection.open() -> Connection

fn invalid(first: mut Connection, second: mut Connection) -> Unit {
    ResourcePool.discard(lease: mut first)
    ResourcePool.discard(lease: mut second)
}

fn valid() -> Unit {
    local pool = ResourcePool<Connection>.new(create: || Connection.open(), max_size: 1)
    with ResourcePool.borrow(pool: mut pool) as lease {
        ResourcePool.discard(lease: mut lease)
    }
    ResourcePool.discard(lease: mut lease)
}
"#;
    let oracle = checker_oracle_records("structured-rs0710.rss", source, "RS0710");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise ordinary values and an expired lease name"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0710",
    );
    assert_eq!(oracle, actual, "RS0710 structured diagnostics diverged");
}

#[test]
fn checker_rs0711_structured_multiset_parity() {
    let source = r#"features: local

resource Session {
    id: Int
}

fn Session.open(host: read String) -> Session

fn build(host: read String) -> Unit {
    let managed = "managed"
    local owned = "owned"
    local first = ResourcePool<Session>.lazy(
        create: || {
            let suffix = "local"
            return Session.open(host: read host)
        },
        max_size: 2,
    )
    local second = ResourcePool<Session>.lazy(
        create: || Session.open(host: read managed),
        max_size: 2,
    )
    local valid = ResourcePool<Session>.lazy(
        create: || Session.open(host: read owned),
        max_size: 2,
    )
}
"#;
    let oracle = checker_oracle_records("structured-rs0711.rss", source, "RS0711");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must preserve one parameter use and one managed capture"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0711",
    );
    assert_eq!(oracle, actual, "RS0711 structured diagnostics diverged");
}

#[test]
fn checker_rs0805_structured_multiset_parity() {
    let source = r#"features: local

fn mismatch() -> Int {
    let mut count = 0
    local bump = fn() captures(read count) effects(pure) {
        count = count + 1
        return count
    }
    return bump()
}

fn missing() -> Int {
    let offset = 2
    local add = fn(value) captures() effects(pure) {
        return value + offset
    }
    return add(40)
}

fn unused() -> Int {
    let offset = 2
    local identity = fn(value) captures(read offset) effects(pure) {
        return value
    }
    return identity(40)
}

fn stronger_is_valid() -> Int {
    let offset = 2
    local add = fn(value) captures(take offset) effects(pure) {
        return value + offset
    }
    return add(40)
}
"#;
    let oracle = checker_oracle_records("structured-rs0805.rss", source, "RS0805");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must preserve mismatch, missing, and unused capture diagnostics"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0805",
    );
    assert_eq!(oracle, actual, "RS0805 structured diagnostics diverged");
}

#[test]
fn checker_closure_capture_structured_multiset_parity() {
    let source = r#"features: local

struct Image { id: Int }
class Scheduler

fn Image.inspect(image: read Image) -> Unit
fn schedule(scheduler: mut Scheduler, callback: read Fn()) -> Unit
    effects(retains(callback))
fn apply(callback: noescape Fn()) -> Unit {
    callback()
}
fn consume(image: take Image) -> Unit

fn managed() -> Unit {
    local image = Image(id: 1)
    let callback = || {
        Image.inspect(image: read image)
        Image.inspect(image: read image)
    }
}

fn retained(scheduler: mut Scheduler) -> Unit {
    local image = Image(id: 2)
    schedule(scheduler: mut scheduler, callback: read || {
        Image.inspect(image: read image)
    })
}

fn consuming() -> Unit {
    local first = Image(id: 3)
    local second = Image(id: 4)
    apply(callback: || {
        consume(image: take first)
        consume(image: take second)
    })
}
"#;

    let all_actual = run_cached_checker_records(source).expect("rss checker should emit records");
    for (code, expected) in [("RS0801", 3), ("RS0804", 2)] {
        let oracle = checker_oracle_records("structured-closure-capture.rss", source, code);
        assert_eq!(
            oracle.len(),
            expected,
            "fixture must preserve every {code} occurrence"
        );
        let actual = diagnostic_records_for_code(all_actual.clone(), code);
        assert_eq!(oracle, actual, "{code} structured diagnostics diverged");
    }
}

#[test]
fn checker_closure_escape_structured_multiset_parity() {
    let source = r#"features: local

struct Callback

fn store(callback: read Callback) -> Unit

fn invalid_signature(callback: noescape Fn()) -> Unit
    effects(retains(callback))
{
    callback()
}

fn noescape_escapes(callback: noescape Fn()) -> Fn {
    let stored = callback
    store(callback: read callback)
    let wrapper = || {
        callback()
    }
    return callback
}

fn local_escapes() -> Callback {
    local callback = || {
        return Unit
    }
    let stored = callback
    store(callback: read callback)
    return callback
}
"#;

    let all_actual = run_cached_checker_records(source).expect("rss checker should emit records");
    for (code, expected) in [("RS0802", 6), ("RS0803", 4)] {
        let oracle = checker_oracle_records("structured-closure-escape.rss", source, code);
        assert_eq!(
            oracle.len(),
            expected,
            "fixture must preserve every {code} occurrence"
        );
        let actual = diagnostic_records_for_code(all_actual.clone(), code);
        assert_eq!(oracle, actual, "{code} structured diagnostics diverged");
    }
}

#[test]
fn checker_rs0702_structured_multiset_parity() {
    let source = r#"features: local

resource File { fd: Int }
resource Conn { fd: Int }
class Registry

fn File.open(path: read Path) -> File
fn File.inspect(file: mut File) -> Unit
fn Conn.from_file(file: read File) -> Conn
fn consume(file: take File) -> Unit
fn register(registry: mut Registry, file: read File) -> Unit
    effects(retains(file))

fn lease(pool: mut ResourcePool<File>) -> Unit {
    local file = ResourcePool.borrow(pool: mut pool)
}

fn producer(path: read Path) -> Unit {
    let file = File.open(path: read path)
}

fn returned(path: read Path) -> File {
    with File.open(path: read path) as file {
        return read file
    }
}

fn bound(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        let saved = read file
    }
}

fn managed(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        let shared = manage file
    }
}

fn taken(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        consume(file: take file)
    }
}

fn retained(registry: mut Registry, path: read Path) -> Unit {
    with File.open(path: read path) as file {
        register(registry: mut registry, file: read file)
    }
}

fn captured(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        let callback = || {
            File.inspect(file: mut file)
        }
    }
}

fn factory(path: read Path) -> Unit {
    with File.open(path: read path) as file {
        local pool = ResourcePool<Conn>.new(
            create: || Conn.from_file(file: read file),
            max_size: 1,
        )
    }
}

fn viewed(data: read Bytes) -> BytesView {
    view item = Bytes.view(value: read data, start: 0, len: 1)
    return item
}
"#;

    let oracle = checker_oracle_records("structured-rs0702.rss", source, "RS0702");
    assert_eq!(
        oracle.len(),
        11,
        "fixture must preserve every resource escape path"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0702",
    );
    assert_eq!(oracle, actual, "RS0702 structured diagnostics diverged");
}

#[test]
fn checker_rs0902_structured_multiset_parity() {
    let source = r#"struct Value {
    id: Int
}

struct Holder {
    first: weak Value
    second: weak Int
}
"#;
    let oracle = checker_oracle_records("structured-rs0902.rss", source, "RS0902");
    assert_eq!(oracle.len(), 2, "fixture must exercise both weak fields");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0902",
    );
    assert_eq!(oracle, actual, "RS0902 structured diagnostics diverged");
}

#[test]
fn checker_rs0903_structured_multiset_parity() {
    let source = r#"class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn User.log(user: read User) -> Unit
fn User.rename(user: mut User) -> Unit

fn invalid(session: read Session) -> Unit {
    User.log(user: read session.owner)
    User.rename(user: mut session.owner)
}

fn valid(session: read Session) -> Option<User> {
    return Weak.upgrade(value: read session.owner)
}
"#;
    let oracle = checker_oracle_records("structured-rs0903.rss", source, "RS0903");
    assert_eq!(oracle.len(), 2, "fixture must exercise read and mut uses");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0903",
    );
    assert_eq!(oracle, actual, "RS0903 structured diagnostics diverged");
}

#[test]
fn checker_rs0904_structured_multiset_parity() {
    let source = r#"class User {
    id: Int
}

struct Session {
    owner: weak User
}

fn invalid(first: read User, second: read User) -> Unit {
    let a = Session(owner: read first)
    let b = Session(owner: read second)
}

fn valid(user: read User) -> Unit {
    let a = Session(owner: Weak.from(value: read user))
    let b = Session(owner: Weak.downgrade(value: read user))
}
"#;
    let oracle = checker_oracle_records("structured-rs0904.rss", source, "RS0904");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both bad initializers"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0904",
    );
    assert_eq!(oracle, actual, "RS0904 structured diagnostics diverged");
}

#[test]
fn checker_rs0901_structured_multiset_parity() {
    let source = r#"features: local

struct Rule {
    name: String
}

struct Config {
    first: handle List<Rule>
    second: handle List<Rule>
    owned: List<Rule>
}

fn consume(config: mut Config) -> Unit {
    List.consume(list: take config.first)
    List.consume(list: take config.owned)
    List.consume(list: take config.second)
}
"#;
    let oracle = checker_oracle_records("structured-rs0901.rss", source, "RS0901");
    assert_eq!(oracle.len(), 2, "fixture must exercise both handle fields");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0901",
    );
    assert_eq!(oracle, actual, "RS0901 structured diagnostics diverged");
}

#[test]
fn checker_rs1003_structured_multiset_parity() {
    let source = "own struct First\nown struct Second\n";
    let oracle = checker_oracle_records("structured-rs1003.rss", source, "RS1003");
    assert_eq!(oracle.len(), 2, "fixture must exercise both own structs");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS1003",
    );
    assert_eq!(oracle, actual, "RS1003 structured diagnostics diverged");
}

#[test]
fn checker_rs0306_structured_multiset_parity() {
    let source = r#"features: local

class Session

fn create() -> Unit {
    local first = Session()
    local second = Session()
    return Unit
}
"#;
    let oracle = checker_oracle_records("structured-rs0306.rss", source, "RS0306");
    assert_eq!(oracle.len(), 2, "fixture must exercise both local bindings");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0306",
    );
    assert_eq!(oracle, actual, "RS0306 structured diagnostics diverged");
}

#[test]
fn checker_rs0307_structured_multiset_parity() {
    let source = r#"features: local

class User {
    id: Int
}

struct Frame {
    id: Int
}

fn invalid(user: read User) -> Unit {
    let managed = User(id: 1)
    let first = manage user
    let second = manage managed
}

fn valid() -> Unit {
    local frame = Frame(id: 1)
    let promoted = manage frame
}
"#;
    let oracle = checker_oracle_records("structured-rs0307.rss", source, "RS0307");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise parameter and let values"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0307",
    );
    assert_eq!(oracle, actual, "RS0307 structured diagnostics diverged");
}

#[test]
fn checker_rs0308_structured_multiset_parity() {
    let source = r#"features: local

struct Buffer {
    id: Int
}

fn Buffer.consume(buffer: take Buffer) -> Unit

fn invalid(buffer: read Buffer) -> Unit {
    let managed = Buffer(id: 1)
    Buffer.consume(buffer: take buffer)
    Buffer.consume(buffer: take managed)
}

fn valid(owned: take Buffer) -> Unit {
    local direct = Buffer(id: 1)
    Buffer.consume(buffer: take direct)
    Buffer.consume(buffer: take owned)
}
"#;
    let oracle = checker_oracle_records("structured-rs0308.rss", source, "RS0308");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise parameter and let values"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0308",
    );
    assert_eq!(oracle, actual, "RS0308 structured diagnostics diverged");
}

#[test]
fn checker_rs0301_structured_multiset_parity() {
    let source = r#"features: local

struct Rules {
    id: Int
}

struct Holder {
    rules: handle Rules
}

fn Holder.create() -> fresh Holder

fn invalid() -> Unit {
    let shared = Rules(id: 1)
    local first = read shared
    local second = read Some(shared)
    local holder = Holder.create()
    local third = read holder.rules
    local fourth = read Ok(holder.rules)
}

fn valid() -> Unit {
    local owned = Rules(id: 1)
    local copy = read owned
    local number = 1
}
"#;
    let oracle = checker_oracle_records("structured-rs0301.rss", source, "RS0301");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise managed values, wrappers, and handle fields"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0301",
    );
    assert_eq!(oracle, actual, "RS0301 structured diagnostics diverged");
}

#[test]
fn checker_place_conflicts_structured_multiset_parity() {
    let source = r#"features: local

struct Cache { value: Int }
struct Inner { cache: Cache }
struct State { inner: Inner }
struct Buffer { value: Int }
struct LocalVec { length: Int }
struct Workspace { id: Int }
struct Config { workspace: Workspace }
struct SplitState { cache: Cache buffer: Buffer }

fn use_state(state: read State, cache: mut Cache) -> Unit
fn use_inner(inner: mut Inner, cache: mut Cache) -> Unit
fn use_buffers(a: mut Buffer, b: mut Buffer) -> Unit
fn use_config(config: take Config, workspace: read Workspace) -> Unit
fn use_parts(cache: mut Cache, buffer: mut Buffer) -> Unit
fn make_state() -> fresh State
fn make_buffers() -> fresh LocalVec
fn make_config() -> fresh Config

fn whole_base() -> Unit {
    local state = make_state()
    use_state(state: read state, cache: mut state.inner.cache)
}

fn prefix() -> Unit {
    local state = make_state()
    use_inner(inner: mut state.inner, cache: mut state.inner.cache)
    use_inner(inner: mut state.inner, cache: mut state.inner.cache)
}

fn indexed() -> Unit {
    local buffers = make_buffers()
    use_buffers(a: mut buffers[0], b: mut buffers[1])
}

fn moved() -> Unit {
    local config = make_config()
    use_config(config: take config, workspace: read config.workspace)
}

fn managed_split(state: mut SplitState) -> Unit {
    use_parts(cache: mut state.cache, buffer: mut state.buffer)
}
"#;

    let all_actual = run_cached_checker_records(source).expect("rss checker should emit records");
    for code in ["RS0302", "RS0303", "RS0304", "RS0305", "RS0309"] {
        let oracle = checker_oracle_records("structured-place-conflicts.rss", source, code);
        let expected = if code == "RS0303" { 2 } else { 1 };
        assert_eq!(
            oracle.len(),
            expected,
            "fixture must preserve every {code} occurrence"
        );
        let actual = diagnostic_records_for_code(all_actual.clone(), code);
        assert_eq!(oracle, actual, "{code} structured diagnostics diverged");
    }
}

#[test]
fn checker_rs0401_structured_multiset_parity() {
    let source = r#"features: local

struct Frame {
    id: Int
}

fn consume(value: take Frame) -> Unit

fn invalid_direct() -> Unit {
    local value = Frame(id: 1)
    consume(value: take value)
    Log.write(message: read "moved")
    let first = value.id
    let second = value.id
}

fn invalid_field() -> Unit {
    local holder = Frame(id: 2)
    let moved = manage holder.id
    let later = holder.id
}

fn valid() -> Unit {
    local value = Frame(id: 3)
    consume(value: take value)
    local value = Frame(id: 4)
    let id = value.id
}
"#;
    let oracle = checker_oracle_records("structured-rs0401.rss", source, "RS0401");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise repeated direct uses, a field path, and a rebound name"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0401",
    );
    assert_eq!(oracle, actual, "RS0401 structured diagnostics diverged");
}

#[test]
fn checker_rs0501_structured_multiset_parity() {
    let source = r#"features: local

struct Item {
    id: Int
}

fn Cache.store(value: read Item) -> Unit
    effects(retains(value))

fn Cache.store_option(value: read Option<Item>) -> Unit
    effects(retains(value))

fn exercise() -> Unit {
    local first = Item(id: 1)
    local second = Item(id: 2)
    let managed = Item(id: 3)
    Cache.store(value: read first)
    Cache.store(value: read second)
    Cache.store_option(value: read Some(first))
    Cache.store(value: read managed)
}
"#;
    let oracle = checker_oracle_records("structured-rs0501.rss", source, "RS0501");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise direct, repeated, and wrapped local retention"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0501",
    );
    assert_eq!(oracle, actual, "RS0501 structured diagnostics diverged");
}

#[test]
fn checker_rs0601_structured_multiset_parity() {
    let source = r#"features: local

struct Boxed {
    value: Int
}

struct Holder {
    boxed: handle Boxed
}

fn Holder.create() -> fresh Holder

fn bad_direct(value: read Boxed) -> fresh Boxed {
    return value
}

fn bad_wrapper(value: read Boxed) -> Option<fresh Boxed> {
    return Some(value)
}

fn bad_field() -> fresh Boxed {
    local holder = Holder.create()
    return holder.boxed
}
"#;
    let oracle = checker_oracle_records("structured-rs0601.rss", source, "RS0601");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise identifier, wrapper, and field-expression spans"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0601",
    );
    assert_eq!(oracle, actual, "RS0601 structured diagnostics diverged");
}

#[test]
fn checker_rs0604_structured_multiset_parity() {
    let source = r#"features: local

struct Image {
    width: Int
}

fn Image.load(width: read Int) -> fresh Image
fn mutate(image: mut Image) -> Unit
fn consume(image: take Image) -> Unit

fn exercise() -> Unit {
    mutate(image: mut Image.load(width: read 1))
    consume(image: take Image.load(width: read 2))
    local image = Image.load(width: read 3)
    mutate(image: mut image)
}
"#;
    let oracle = checker_oracle_records("structured-rs0604.rss", source, "RS0604");
    assert_eq!(oracle.len(), 2, "fixture must exercise mut and take ranges");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0604",
    );
    assert_eq!(oracle, actual, "RS0604 structured diagnostics diverged");
}

#[test]
fn checker_rs0603_structured_multiset_parity() {
    let source = r#"class User {
    name: String
}

class BuildError {
    code: Int
}

fn direct() -> fresh User
fn nested() -> Result<fresh User, BuildError>

fn generic<T>() -> fresh T {
    return value
}
"#;
    let oracle = checker_oracle_records("structured-rs0603.rss", source, "RS0603");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise direct, nested, and generic fresh targets"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0603",
    );
    assert_eq!(oracle, actual, "RS0603 structured diagnostics diverged");
}

#[test]
fn checker_rs0311_structured_multiset_parity() {
    let source = r#"features: local

struct State {
    value: Int
}

fn invalid(state: mut State, borrowed: read State) -> Unit {
    let count = 0
    count = 1
    count = 2
    state = State(value: 3)
    borrowed = State(value: 4)
}

fn valid(value: mut Int) -> Unit {
    let mut count = 0
    count = 1
    value = 2
}
"#;
    let oracle = checker_oracle_records("structured-rs0311.rss", source, "RS0311");
    assert_eq!(
        oracle.len(),
        4,
        "fixture must exercise repeated local and parameter assignments"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0311",
    );
    assert_eq!(oracle, actual, "RS0311 structured diagnostics diverged");
}

#[test]
fn checker_rs0313_structured_multiset_parity() {
    let source = r#"fn exercise() -> Unit {
    let mut count: Int = 0
    let mut label: String = "start"
    let mut enabled: Bool = false
    count = "wrong"
    label = true
    enabled = 1
    count = 2
    label = "valid"
    enabled = false
}
"#;
    let oracle = checker_oracle_records("structured-rs0313.rss", source, "RS0313");
    assert_eq!(oracle.len(), 2, "fixture must exercise scalar mismatches");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0313",
    );
    assert_eq!(oracle, actual, "RS0313 structured diagnostics diverged");
}

#[test]
fn checker_rs0312_structured_multiset_parity() {
    let source = r#"fn exercise() -> Unit {
    let mut values = Map<String, Int>.new()
    let mut queue = Deque<Int>.new()
    let mut items = List<Int>.new()
    values["first"] = 1
    values["second"] = 2
    queue[0] = 3
    items[0] = 4
}
"#;
    let oracle = checker_oracle_records("structured-rs0312.rss", source, "RS0312");
    assert_eq!(
        oracle.len(),
        3,
        "fixture must exercise repeated Map and Deque index assignments"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS0312",
    );
    assert_eq!(oracle, actual, "RS0312 structured diagnostics diverged");
}

#[test]
fn checker_rs1002_structured_multiset_parity() {
    let source = r#"fn convert(value: Int) -> Int {
    let first = value as String
    let second = value as Float
    return value
}
"#;
    let oracle = checker_oracle_records("structured-rs1002.rss", source, "RS1002");
    assert_eq!(oracle.len(), 2, "fixture must exercise both conversions");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS1002",
    );
    assert_eq!(oracle, actual, "RS1002 structured diagnostics diverged");
}

#[test]
fn checker_rs1001_structured_multiset_parity() {
    let source = r#"struct Point {
    x: Int
}

fn invalid(right: read Point) -> Unit {
    let first = Point + right
    let second = "left" - "right"
}

fn valid(left: Int, right: Int) -> Unit {
    let sum = left + right
    let shifted = left << 1
}
"#;
    let oracle = checker_oracle_records("structured-rs1001.rss", source, "RS1001");
    assert_eq!(
        oracle.len(),
        2,
        "fixture must exercise both overload attempts"
    );
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS1001",
    );
    assert_eq!(oracle, actual, "RS1001 structured diagnostics diverged");
}

#[test]
fn checker_rs1004_structured_multiset_parity() {
    let source = r#"fn first(value: &Int) -> Int {
    return 0
}

fn second() -> &String {
    return ""
}
"#;
    let oracle = checker_oracle_records("structured-rs1004.rss", source, "RS1004");
    assert_eq!(oracle.len(), 2, "fixture must exercise both references");
    let actual = diagnostic_records_for_code(
        run_cached_checker_records(source).expect("rss checker should emit records"),
        "RS1004",
    );
    assert_eq!(oracle, actual, "RS1004 structured diagnostics diverged");
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

#[test]
fn checker_rs0015_edge_parity() {
    let root = workspace_root();
    let cases = [
        (
            "comparison-before-generic.rss",
            "features: local\n\
             fn f(limit: Int) -> Unit {\n\
                 let mut i = 0\n\
                 while i < limit {\n\
                     let values = List<Int>.new()\n\
                     i = i + 1\n\
                 }\n\
                 return Unit\n\
             }\n"
            .to_string(),
            false,
        ),
        (
            "native-function-body.rss",
            "features: native\n\
             pub native fn host() -> Unit {\n\
                 return Unit\n\
             }\n"
            .to_string(),
            true,
        ),
        (
            "hostile-malformed/unicode-bidi.rss",
            std::fs::read_to_string(
                root.join("crates/rsscript/tests/hostile-malformed/unicode-bidi.rss"),
            )
            .expect("unicode fixture should be readable"),
            true,
        ),
        (
            "hostile-malformed/unterminated-string.rss",
            std::fs::read_to_string(
                root.join("crates/rsscript/tests/hostile-malformed/unterminated-string.rss"),
            )
            .expect("unterminated-string fixture should be readable"),
            true,
        ),
        (
            "samples/ast/async_let.rss",
            std::fs::read_to_string(root.join("selfhost/samples/ast/async_let.rss"))
                .expect("async-let sample should be readable"),
            true,
        ),
        (
            "core-properties/properties_result_option.rss",
            std::fs::read_to_string(
                root.join("packages/core-properties/src/properties_result_option.rss"),
            )
            .expect("result/option properties should be readable"),
            false,
        ),
    ];
    let exe = compile_checker().expect("rss checker should compile");
    for (file, source, expects_rs0015) in cases {
        let oracle = checker_oracle_codes(file, &source);
        let actual = run_checker(&exe, &source).expect("rss checker should run");
        assert_eq!(oracle, actual, "checker parity diverged for {file}");
        assert_eq!(
            oracle.contains(&"RS0015".to_string()),
            expects_rs0015,
            "unexpected Rust RS0015 result for {file}: {oracle:?}"
        );
    }
}

/// Phase-3 gate (ignored by default): the rss checker's target-code diagnostics
/// match the analyzer over the whole `.rss` corpus.
#[test]
#[ignore]
fn checker_parity_corpus() {
    let root = workspace_root();
    let all_files = collect_rss_files(&root).expect("corpus discovery should succeed");
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
                        let source = match std::fs::read_to_string(file) {
                            Ok(source) => source,
                            Err(e) => {
                                run_failures.push(format!("{rel}: unreadable: {e}"));
                                continue;
                            }
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
// Package-contract parity — RS1301 is not a single-source checker diagnostic.
// The self-hosted checker covers functions plus the package data-model and
// protocol declarations that production treats as one RS1301 contract surface.
// ---------------------------------------------------------------------------

fn compile_package_contract_checker() -> Result<RegVmExecutable, String> {
    compile_selfhost_tool("package_contract.rss", "package contract checker")
}

fn parse_package_contract_output(stdout: &str) -> Result<Vec<String>, String> {
    let lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    match lines.as_slice() {
        ["CLEAN"] => Ok(Vec::new()),
        [code::PACKAGE_INTERFACE_MISMATCH] => {
            Ok(vec![code::PACKAGE_INTERFACE_MISMATCH.to_string()])
        }
        [] => Err("rss package contract checker emitted no verdict".to_string()),
        _ => Err(format!(
            "rss package contract checker emitted malformed output: {lines:?}"
        )),
    }
}

fn run_package_contract_checker(
    exe: &RegVmExecutable,
    interface_source: &str,
    source: &str,
) -> Result<Vec<String>, String> {
    run_package_contract_checker_with_native(exe, interface_source, source, "")
}

fn run_package_contract_checker_with_native(
    exe: &RegVmExecutable,
    interface_source: &str,
    source: &str,
    native_bindings: &str,
) -> Result<Vec<String>, String> {
    let output = exe
        .eval_main_with_args([
            interface_source.to_string(),
            source.to_string(),
            native_bindings.to_string(),
        ])
        .map_err(|e| format!("rss package contract checker failed to run: {e:?}"))?;
    parse_package_contract_output(&output.stdout)
}

fn selfhost_unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()))
}

fn write_package_contract_fixture(
    dir: &Path,
    interface_source: &str,
    source: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(dir.join("interface"))
        .map_err(|e| format!("cannot create interface dir under {}: {e}", dir.display()))?;
    std::fs::create_dir_all(dir.join("src"))
        .map_err(|e| format!("cannot create src dir under {}: {e}", dir.display()))?;
    std::fs::write(
        dir.join("rsspkg.toml"),
        "[package]\nname = \"selfhost-contract-parity\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[interfaces]\npaths = [\"interface\"]\n",
    )
    .map_err(|e| format!("cannot write package manifest under {}: {e}", dir.display()))?;
    std::fs::write(dir.join("interface/lib.rssi"), interface_source).map_err(|e| {
        format!(
            "cannot write package interface under {}: {e}",
            dir.display()
        )
    })?;
    std::fs::write(dir.join("src/lib.rss"), source)
        .map_err(|e| format!("cannot write package source under {}: {e}", dir.display()))?;
    Ok(())
}

fn package_contract_oracle_codes(interface_source: &str, source: &str) -> Vec<String> {
    package_contract_oracle_codes_with_native(interface_source, source, &[])
}

fn package_contract_oracle_codes_with_native(
    interface_source: &str,
    source: &str,
    native_bindings: &[(&str, &str)],
) -> Vec<String> {
    let dir = selfhost_unique_temp_dir("rss-selfhost-package-contract");
    write_package_contract_fixture(&dir, interface_source, source)
        .expect("package contract fixture should be writable");
    if !native_bindings.is_empty() {
        std::fs::create_dir_all(dir.join("native"))
            .expect("native binding directory should be writable");
        let mut manifest = String::from("[bindings]\n");
        for (symbol, target) in native_bindings {
            manifest.push_str(&format!("\"{symbol}\" = \"{target}\"\n"));
        }
        std::fs::write(dir.join("native/bindings.rssbind.toml"), manifest)
            .expect("native binding manifest should be writable");
    }
    let review = review_package_dir(&dir).expect("package review should succeed");
    let _ = std::fs::remove_dir_all(&dir);
    let mut codes = review
        .diagnostics
        .into_iter()
        .filter(|diagnostic| {
            diagnostic.severity == Severity::Error
                && diagnostic.code == code::PACKAGE_INTERFACE_MISMATCH
        })
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    codes
}

fn package_contract_oracle_bundle_codes(
    interface_sources: &[(&str, &str)],
    source_files: &[(&str, &str)],
) -> Vec<String> {
    let dir = selfhost_unique_temp_dir("rss-selfhost-package-contract-bundle");
    std::fs::create_dir_all(dir.join("interface"))
        .expect("package interface directory should be writable");
    std::fs::create_dir_all(dir.join("src")).expect("package source directory should be writable");
    std::fs::write(
        dir.join("rsspkg.toml"),
        "[package]\nname = \"selfhost-contract-bundle\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[interfaces]\npaths = [\"interface\"]\n",
    )
    .expect("package manifest should be writable");
    for (path, contents) in interface_sources {
        std::fs::write(dir.join("interface").join(path), contents)
            .expect("package interface file should be writable");
    }
    for (path, contents) in source_files {
        std::fs::write(dir.join("src").join(path), contents)
            .expect("package source file should be writable");
    }
    let review = review_package_dir(&dir).expect("package bundle review should succeed");
    let _ = std::fs::remove_dir_all(&dir);
    let mut codes = review
        .diagnostics
        .into_iter()
        .filter(|diagnostic| {
            diagnostic.severity == Severity::Error
                && diagnostic.code == code::PACKAGE_INTERFACE_MISMATCH
        })
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();
    codes.sort();
    codes.dedup();
    codes
}

fn join_package_contract_sources(files: &[(&str, &str)]) -> String {
    let mut joined = String::new();
    for (_, contents) in files {
        joined.push_str(contents);
        if !contents.ends_with('\n') {
            joined.push('\n');
        }
    }
    joined
}

#[test]
fn package_contract_function_rs1301_parity_smoke() {
    let cases = [
        (
            "matching function",
            "pub fn render(body: read String) -> String\n",
            "pub fn render(body: read String) -> String {\n    return body\n}\n",
        ),
        (
            "missing implementation",
            "pub fn render(body: read String) -> String\n",
            "fn helper(body: read String) -> String {\n    return body\n}\n",
        ),
        (
            "signature mismatch",
            "pub fn render(body: read String) -> fresh String\n    effects(no_panic)\n",
            "pub fn render(body: read String) -> String {\n    return body\n}\n",
        ),
    ];
    let exe = compile_package_contract_checker().expect("rss package checker should compile");
    for (name, interface_source, source) in cases {
        let oracle = package_contract_oracle_codes(interface_source, source);
        let actual = run_package_contract_checker(&exe, interface_source, source)
            .expect("rss package contract checker should run");
        assert_eq!(
            oracle, actual,
            "package contract parity diverged for {name}"
        );
    }
}

#[test]
fn package_contract_declaration_rs1301_parity() {
    let cases = [
        (
            "matching struct",
            "struct Config {\n    retries: Int\n}\n",
            "pub struct Config {\n    retries: Int\n}\n",
        ),
        (
            "struct field mismatch",
            "struct Config {\n    retries: Int\n}\n",
            "pub struct Config {\n    retries: String\n}\n",
        ),
        (
            "opaque struct hides fields",
            "opaque struct Config\n",
            "pub struct Config {\n    retries: Int\n}\n",
        ),
        (
            "opaque type still checks kind",
            "opaque struct Config\n",
            "pub resource Config {\n    retries: Int\n}\n",
        ),
        (
            "matching sum",
            "sum PackageError {\n    Io(code: Int),\n    Invalid\n}\n",
            "pub sum PackageError {\n    Io(code: Int),\n    Invalid\n}\n",
        ),
        (
            "sum variant mismatch",
            "sum PackageError {\n    Io(code: Int),\n    Invalid\n}\n",
            "pub sum PackageError {\n    Io(code: String),\n    Invalid\n}\n",
        ),
        (
            "matching alias and const",
            "type PackageName = String\nconst MAX_RETRIES: Int = 3\n",
            "pub type PackageName = String\npub const MAX_RETRIES: Int = 3\n",
        ),
        (
            "const mismatch",
            "const MAX_RETRIES: Int = 3\n",
            "pub const MAX_RETRIES: Int = 4\n",
        ),
        (
            "matching protocol",
            "protocol Writer {\n    fn write(self: mut Self, message: read String) -> Unit\n        effects(retains(message))\n}\n",
            "protocol Writer {\n    fn write(self: mut Self, message: read String) -> Unit\n        effects(retains(message))\n}\n",
        ),
        (
            "protocol mismatch",
            "protocol Writer {\n    fn write(self: mut Self, message: read String) -> Unit\n        effects(retains(message))\n}\n",
            "protocol Writer {\n    fn write(self: mut Self, message: read String) -> Unit\n}\n",
        ),
        (
            "protocol impl mismatch",
            "protocol Writer {\n    fn write(self: mut Self) -> Unit\n}\nstruct Buffer\nimpl Writer for Buffer {\n    write = Buffer.write\n}\n",
            "protocol Writer {\n    fn write(self: mut Self) -> Unit\n}\npub struct Buffer\nimpl Writer for Buffer {\n    write = Buffer.audit\n}\n",
        ),
    ];
    let exe = compile_package_contract_checker().expect("rss package checker should compile");
    for (name, interface_source, source) in cases {
        let oracle = package_contract_oracle_codes(interface_source, source);
        let actual = run_package_contract_checker(&exe, interface_source, source)
            .expect("rss package contract checker should run");
        assert_eq!(
            oracle, actual,
            "package declaration contract parity diverged for {name}"
        );
    }
}

#[test]
fn package_contract_native_function_exemption_parity() {
    let interface_source = "features: native\n\nnative fn Native.echo(message: read String) -> String\n    effects(native)\n";
    let source = "fn helper() -> Unit {\n    return Unit\n}\n";
    let native_bindings = [("Native.echo", "rss_native::echo")];
    let oracle =
        package_contract_oracle_codes_with_native(interface_source, source, &native_bindings);
    let exe = compile_package_contract_checker().expect("rss package checker should compile");
    let actual =
        run_package_contract_checker_with_native(&exe, interface_source, source, "Native.echo")
            .expect("rss package contract checker should run");
    assert_eq!(oracle, actual, "native interface exemption diverged");
}

#[test]
fn package_contract_resolved_multifile_bundle_parity() {
    let interface_sources = [
        ("api.rssi", "fn render(body: read String) -> String\n"),
        (
            "model.rssi",
            "struct Config {\n    retries: Int\n}\ntype PackageName = String\n",
        ),
    ];
    let matching_sources = [
        (
            "api.rss",
            "pub fn render(body: read String) -> String {\n    return body\n}\n",
        ),
        (
            "model.rss",
            "pub struct Config {\n    retries: Int\n}\npub type PackageName = String\n",
        ),
    ];
    let missing_sources = [(
        "api.rss",
        "pub fn render(body: read String) -> String {\n    return body\n}\n",
    )];
    let exe = compile_package_contract_checker().expect("rss package checker should compile");
    let interface_bundle = join_package_contract_sources(&interface_sources);

    for (name, sources) in [
        ("matching bundle", matching_sources.as_slice()),
        ("missing model file", missing_sources.as_slice()),
    ] {
        let oracle = package_contract_oracle_bundle_codes(&interface_sources, sources);
        let source_bundle = join_package_contract_sources(sources);
        let actual = run_package_contract_checker(&exe, &interface_bundle, &source_bundle)
            .expect("rss package bundle checker should run");
        assert_eq!(
            oracle, actual,
            "resolved package bundle diverged for {name}"
        );
    }
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
    *T.get_or_init(|| env_tier_u8("RSS_SELFHOST_AST_TIER", 0, &[0, 1, 2]))
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
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
    let mut ok = 0usize;
    let mut empty: Vec<String> = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
        let source =
            std::fs::read_to_string(file).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));
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
// harness compares it byte-for-byte against `ast_oracle_dump`. The curated
// `samples/ast/*.rss` set is a non-ignored fast gate, while
// `ast_parity_corpus` is the full equality gate over the discovered corpus.
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

/// Step-2 corpus gate (ignored by default): the rss AST producer reproduces the
/// Rust AST oracle byte-for-byte for every discovered `.rss` file.
///
/// RUNTIME: each worker compiles one private rss producer (the executable uses
/// non-thread-safe Rc state) and reuses it over a size-descending share of the
/// corpus. Giant inputs start first to avoid a long single-worker tail. A debug
/// build is still slow; run the gate in release:
/// `cargo test -p rsscript --release --lib selfhost_parity::ast_parity_corpus -- --ignored --nocapture`.
/// The fast inner-loop gate is `ast_parity_samples` (non-ignored, curated subset).
#[test]
#[ignore]
fn ast_parity_corpus() {
    let root = workspace_root();
    let files = collect_rss_files(&root).expect("corpus discovery should succeed");
    // Start expensive inputs first so a giant self-hosted tool does not become
    // the sole straggler after every other worker has drained the small files.
    // Paths break equal-size ties to keep scheduling deterministic.
    let mut sized_files = files
        .into_iter()
        .map(|path| {
            let len = std::fs::metadata(&path)
                .unwrap_or_else(|e| panic!("cannot stat {}: {e}", path.display()))
                .len();
            (len, path)
        })
        .collect::<Vec<_>>();
    sized_files.sort_by(|(left_len, left), (right_len, right)| {
        right_len.cmp(left_len).then_with(|| left.cmp(right))
    });
    let files = sized_files
        .into_iter()
        .map(|(_, path)| path)
        .collect::<Vec<_>>();
    let total = files.len();
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, total.max(1));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let partials: Vec<(usize, Vec<String>, Vec<String>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                let (root, files, next) = (&root, &files, &next);
                scope.spawn(move || {
                    let exe = compile_astdump().expect("rss astdump should compile");
                    let mut ok = 0usize;
                    let mut run_failures = Vec::new();
                    let mut mismatches = Vec::new();
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
                        let source = match std::fs::read_to_string(file) {
                            Ok(source) => source,
                            Err(e) => {
                                run_failures.push(format!("{rel}: unreadable: {e}"));
                                continue;
                            }
                        };
                        let oracle = ast_oracle_dump(&rel, &source);
                        match run_astdump(&exe, &source) {
                            Err(e) => run_failures.push(format!("{rel}: {e}")),
                            Ok(actual) if actual == oracle => ok += 1,
                            Ok(actual) if mismatches.len() < 10 => {
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
                                mismatches.push(format!("{rel}{first_diff}"));
                            }
                            Ok(_) => {}
                        }
                    }
                    (ok, run_failures, mismatches)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect()
    });
    let mut ok = 0usize;
    let mut run_failures = Vec::new();
    let mut sample_mismatches = Vec::new();
    for (partial_ok, partial_failures, partial_mismatches) in partials {
        ok += partial_ok;
        run_failures.extend(partial_failures);
        sample_mismatches.extend(partial_mismatches);
    }
    run_failures.sort();
    sample_mismatches.sort();
    sample_mismatches.truncate(10);
    eprintln!(
        "\n=== ast_parity_corpus ===\n  files: {total}\n  checked: {}\n  byte-exact: {ok}\n  \
         run-failures: {}\n",
        total - run_failures.len(),
        run_failures.len()
    );
    for failure in &run_failures {
        eprintln!("[run-fail] {failure}");
    }
    for rel in &sample_mismatches {
        eprintln!("[mismatch] {rel}");
    }
    // The producer must never crash on corpus input — unsupported constructs are
    // expected to mismatch (partial/`unknown-*` output), not error. A run-failure
    // is a real regression even if `ok` still clears the floor.
    assert_eq!(
        run_failures.len(),
        0,
        "rss AST producer had {} run-failures over {total} corpus files \
         (it must degrade to a mismatch, never crash)",
        run_failures.len()
    );
    assert_eq!(
        total - run_failures.len(),
        total,
        "some corpus files were not readable or were not checked"
    );
    assert_eq!(
        ok,
        total,
        "AST dump must match every corpus file; first mismatches:\n{}",
        sample_mismatches.join("\n\n")
    );
}
