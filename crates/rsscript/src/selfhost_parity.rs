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
use crate::reg_vm_eval_source_main_with_args;

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

/// Run the rss lexer over `source` and parse its dump.
fn rss_dump(source: &str) -> Result<Vec<CanonTok>, String> {
    let lexer_path = selfhost_dir().join("lexer.rss");
    let lexer_src = std::fs::read_to_string(&lexer_path)
        .map_err(|e| format!("cannot read {}: {e}", lexer_path.display()))?;
    let output =
        reg_vm_eval_source_main_with_args("selfhost/lexer.rss", &lexer_src, [source.to_string()])
            .map_err(|e| format!("rss lexer failed to run: {e:?}"))?;
    Ok(output
        .stdout
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(parse_line)
        .collect())
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
        match rss_dump(&source) {
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
