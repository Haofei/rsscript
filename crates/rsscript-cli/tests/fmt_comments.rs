//! `rss fmt` keeps every `//` comment, and formatting its output again changes
//! nothing, for every `.rss` file checked into the repository's source corpora.
//!
//! `rss fmt` runs inside the eval generation loop and its output is read by
//! agents, so a dropped comment is lost information and a second format that
//! differs is churn. The formatter printed the AST and dropped every comment
//! until ADR 0245.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use rsscript_syntax::lexer::lex_with_comments;

const CORPORA: &[&str] = &[
    "examples",
    "stdlib",
    "packages",
    "crates/rsscript-sdk/tests/fixtures",
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the CLI crate sits two levels below the repository root")
        .to_path_buf()
}

fn rss_files(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rss_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "rss") {
            out.push(path);
        }
    }
}

/// `rss fmt` on `path`: `Some(output)`, or `None` when it refuses a file with
/// a syntax error.
fn fmt(path: &Path) -> Option<String> {
    let output = Command::new(env!("CARGO_BIN_EXE_rss"))
        .arg("fmt")
        .arg(path)
        .output()
        .expect("rss fmt runs");
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).expect("rss fmt prints UTF-8"))
}

/// Every comment's text, counted, as the lexer reads it.
fn comments(path: &str, source: &str) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for comment in lex_with_comments(path, source).1 {
        *counts.entry(comment.text).or_default() += 1;
    }
    counts
}

#[test]
fn rss_fmt_keeps_every_comment_and_is_a_fixpoint_over_the_source_corpora() {
    let root = repository_root();
    let mut files = Vec::new();
    for corpus in CORPORA {
        rss_files(&root.join(corpus), &mut files);
    }
    files.sort();
    assert!(files.len() > 100, "found only {} .rss files", files.len());

    let scratch = tempfile::tempdir().expect("temp dir");
    let mut formatted_files = 0usize;
    let mut commented_files = 0usize;
    let mut failures = Vec::new();
    for file in &files {
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string();
        let source = fs::read_to_string(file).expect("corpus file is UTF-8");
        let Some(once) = fmt(file) else {
            // Only a fixture that exists to be a syntax error may be refused.
            if !relative.contains("fixtures/fail/") {
                failures.push(format!("{relative}: rss fmt refused a file outside fail/"));
            }
            continue;
        };
        formatted_files += 1;

        let before = comments(&relative, &source);
        if !before.is_empty() {
            commented_files += 1;
        }
        let after = comments(&relative, &once);
        for (text, count) in &before {
            let kept = after.get(text).copied().unwrap_or(0);
            if kept < *count {
                failures.push(format!("{relative}: lost comment {text:?}"));
            }
        }

        let again_path = scratch
            .path()
            .join(file.file_name().expect("corpus file has a name"));
        fs::write(&again_path, &once).expect("write formatted copy");
        match fmt(&again_path) {
            Some(twice) if twice == once => {}
            Some(_) => failures.push(format!("{relative}: a second rss fmt changed the output")),
            None => failures.push(format!("{relative}: rss fmt's own output does not parse")),
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} formatted files failed:\n{}",
        failures.len(),
        formatted_files,
        failures.join("\n")
    );
    assert!(
        commented_files > 50,
        "the corpora should exercise comments; only {commented_files} files have any"
    );
}
