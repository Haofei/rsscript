//! Shared walker and header parser for the `tests/fixtures` corpus.
//!
//! Two test targets read the same corpus with the same header conventions:
//! `fixture_corpus` runs it through the checker, and `fixture_build_corpus`
//! runs it through build and Artifact verification. The conventions are
//! documented once, here, so the two cannot drift apart.
//!
//! # Fixture header conventions
//!
//! A fixture is a single `.rss` file. Header comments at the top of the file
//! (before the first non-comment, non-blank line) configure the check:
//!
//! * `// expect: RS0201 RS0202` — the exact set of diagnostic codes the file
//!   must produce. Required for every `fail` fixture, rejected for `pass`
//!   fixtures. The set is compared order-insensitively; an unexpected extra
//!   code is a failure, so the corpus keeps pinning precise behaviour rather
//!   than "some error happened". The directive may be repeated; the expected
//!   sets are unioned.
//! * `// interface: ../interfaces/host-fs.rssi` — a sibling `.rssi` file, resolved
//!   relative to the fixture's own directory, that is supplied to the check in
//!   addition to the standard package interfaces, exactly as
//!   `rss check --interface` would. May be repeated.
//! * `// source: ../modules/gadgets.rss` — an additional `.rss` file compiled
//!   into the same compilation unit as the fixture. A file may declare at most
//!   one `module`, so this is how a fixture exercises a rule that spans
//!   modules. Companion sources live outside `pass/` and `fail/` so the corpus
//!   walker does not treat them as fixtures of their own. May be repeated.

#![allow(dead_code)]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Fixture `.rss` files in `directory`, sorted so failures report stably.
pub fn fixture_sources(directory: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read fixture directory {}: {error}", directory.display()))
        .map(|entry| entry.expect("fixture directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rss"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[derive(Default)]
pub struct Header {
    pub expected: BTreeSet<String>,
    pub has_expect: bool,
    pub interfaces: Vec<String>,
    pub sources: Vec<String>,
}

/// Parse the leading comment block of a fixture.
///
/// Scanning stops at the first line that is neither blank nor a `//` comment so
/// a directive cannot hide in the middle of a program.
pub fn parse_header(source: &str) -> Header {
    let mut header = Header::default();
    for line in source.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(comment) = line.strip_prefix("//") else {
            break;
        };
        let comment = comment.trim();
        if let Some(codes) = comment.strip_prefix("expect:") {
            header.has_expect = true;
            header
                .expected
                .extend(codes.split_whitespace().map(str::to_string));
        } else if let Some(interface) = comment.strip_prefix("interface:") {
            header.interfaces.push(interface.trim().to_string());
        } else if let Some(source) = comment.strip_prefix("source:") {
            header.sources.push(source.trim().to_string());
        }
    }
    header
}

/// Read every file a header directive names, relative to the fixture.
pub fn companions(path: &Path, names: &[String], kind: &str) -> Vec<(String, String)> {
    let directory = path.parent().expect("fixture path has a parent directory");
    names
        .iter()
        .map(|name| {
            let companion = directory.join(name);
            let contents = fs::read_to_string(&companion).unwrap_or_else(|error| {
                panic!(
                    "fixture {} names {kind} `{name}`, but {} could not be read: {error}",
                    path.display(),
                    companion.display()
                )
            });
            (companion.to_string_lossy().into_owned(), contents)
        })
        .collect()
}

pub fn name(path: &Path) -> String {
    path.file_name()
        .expect("fixture file name")
        .to_string_lossy()
        .into_owned()
}

pub fn join(codes: &BTreeSet<String>) -> String {
    if codes.is_empty() {
        "<none>".to_string()
    } else {
        codes.iter().cloned().collect::<Vec<_>>().join(" ")
    }
}
