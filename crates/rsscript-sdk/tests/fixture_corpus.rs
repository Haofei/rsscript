//! Executes the `tests/fixtures/{pass,fail}` corpus through the frontend
//! checker.
//!
//! Until this test existed the corpus was inert: 125 `pass` files and 202
//! `fail` files that nothing ran, so nothing noticed when the checker's
//! accept/reject boundary moved away from them.
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
//!
//! Every fixture is checked the way `rss check <file>` checks it
//! (`crates/rsscript-cli/src/cli/check.rs`): the standard package interfaces
//! plus any `// interface:` siblings, and no `--lint` pass.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use rsscript_semantics::{analyze_sources_with_interfaces, standard_package_interfaces};

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Fixture `.rss` files in `directory`, sorted so failures report stably.
fn fixture_sources(directory: &Path) -> Vec<PathBuf> {
    let mut paths = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read fixture directory {}: {error}", directory.display()))
        .map(|entry| entry.expect("fixture directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rss"))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

#[derive(Default)]
struct Header {
    expected: BTreeSet<String>,
    has_expect: bool,
    interfaces: Vec<String>,
    sources: Vec<String>,
}

/// Parse the leading comment block of a fixture.
///
/// Scanning stops at the first line that is neither blank nor a `//` comment so
/// a directive cannot hide in the middle of a program.
fn parse_header(source: &str) -> Header {
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
fn companions(path: &Path, names: &[String], kind: &str) -> Vec<(String, String)> {
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

/// Check one fixture the way `rss check` would and return the emitted codes.
fn check(path: &Path, header: &Header) -> BTreeSet<String> {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()));
    let interface_sources = companions(path, &header.interfaces, "interface");
    let companion_sources = companions(path, &header.sources, "source");

    let mut interfaces = standard_package_interfaces().to_vec();
    interfaces.extend(
        interface_sources
            .iter()
            .map(|(file, contents)| (file.as_str(), contents.as_str())),
    );

    let file = path.to_string_lossy().into_owned();
    let mut sources = vec![(file.as_str(), source.as_str())];
    sources.extend(
        companion_sources
            .iter()
            .map(|(file, contents)| (file.as_str(), contents.as_str())),
    );

    analyze_sources_with_interfaces(&sources, &interfaces)
        .into_iter()
        .map(|diagnostic| diagnostic.code)
        .collect()
}

fn name(path: &Path) -> String {
    path.file_name()
        .expect("fixture file name")
        .to_string_lossy()
        .into_owned()
}

fn join(codes: &BTreeSet<String>) -> String {
    if codes.is_empty() {
        "<none>".to_string()
    } else {
        codes.iter().cloned().collect::<Vec<_>>().join(" ")
    }
}

#[test]
fn pass_fixtures_are_diagnostic_free() {
    let directory = fixtures_root().join("pass");
    let paths = fixture_sources(&directory);
    assert!(!paths.is_empty(), "the pass corpus should not be empty");

    let mut failures = Vec::new();
    for path in &paths {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()));
        let header = parse_header(&source);
        assert!(
            !header.has_expect,
            "{}: a `pass` fixture must not carry an `// expect:` directive",
            name(path)
        );
        let emitted = check(path, &header);
        if !emitted.is_empty() {
            failures.push(format!("{}: emitted {}", name(path), join(&emitted)));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} pass fixtures emitted diagnostics:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
}

#[test]
fn fail_fixtures_emit_exactly_their_expected_codes() {
    let directory = fixtures_root().join("fail");
    let paths = fixture_sources(&directory);
    assert!(!paths.is_empty(), "the fail corpus should not be empty");

    let mut failures = Vec::new();
    for path in &paths {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()));
        let header = parse_header(&source);
        assert!(
            header.has_expect,
            "{}: a `fail` fixture must carry an `// expect: RSxxxx` directive",
            name(path)
        );
        assert!(
            !header.expected.is_empty(),
            "{}: `// expect:` must name at least one diagnostic code",
            name(path)
        );
        let emitted = check(path, &header);
        if emitted != header.expected {
            failures.push(format!(
                "{}: expected {}, emitted {}",
                name(path),
                join(&header.expected),
                join(&emitted)
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} fail fixtures did not emit exactly their expected codes:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
}
