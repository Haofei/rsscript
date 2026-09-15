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

/// The workspace root, from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should exist")
}

/// Every `.rssi` under `directory`, recursively, sorted so failures report
/// stably.
fn interface_files(directory: &Path, found: &mut Vec<PathBuf>) {
    let mut entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| entry.expect("directory entry").path())
        .collect::<Vec<_>>();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            interface_files(&path, found);
        } else if path
            .extension()
            .is_some_and(|extension| extension == "rssi")
        {
            found.push(path);
        }
    }
}

/// A signature-level rule holds wherever the signature is written. An `.rssi`
/// declaration has no body, but it has a contract, and an interface that
/// escaped the rule could export one the language does not have — `pub fn
/// make_default<T>() -> fresh T` was accepted in an interface while the same
/// signature in a `.rss` file was `RS0603`.
///
/// The diagnostic must also land on the interface, not on the source that
/// supplied it: the reader has to be sent to the file they can fix.
#[test]
fn invalid_interface_signatures_are_diagnosed_against_the_interface_file() {
    const SOURCE: &str = "fn main() -> Unit {\n    return Unit\n}\n";
    const INTERFACE: &str = "pub fn make_default<T>() -> fresh T\n";

    let mut interfaces = standard_package_interfaces().to_vec();
    interfaces.push(("host/defaults.rssi", INTERFACE));
    let diagnostics = analyze_sources_with_interfaces(&[("main.rss", SOURCE)], &interfaces);

    let invalid_fresh = diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "RS0603")
        .unwrap_or_else(|| {
            panic!(
                "an invalid `.rssi` signature must be diagnosed; got {:?}",
                diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.code.as_str())
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        invalid_fresh.span.file, "host/defaults.rssi",
        "the diagnostic must point at the interface that declares the signature"
    );
    assert!(
        invalid_fresh.summary.contains("make_default"),
        "the diagnostic must name the interface declaration: {}",
        invalid_fresh.summary
    );

    // The bounded form of the same signature is clean, so the rule is not
    // rejecting every generic `fresh` return in an interface.
    let mut bounded = standard_package_interfaces().to_vec();
    bounded.push((
        "host/defaults.rssi",
        "pub fn make_default<T: Struct>() -> fresh T\n",
    ));
    assert!(
        analyze_sources_with_interfaces(&[("main.rss", SOURCE)], &bounded).is_empty(),
        "a correctly bounded interface signature must stay clean"
    );
}

/// The prelude is the one interface set every program sees, so a regression in
/// it would be invisible in ordinary fixtures until it reached users. Check
/// every `.rssi` that ships, read from disk rather than from the embedded
/// catalog, so an interface added to `stdlib/` or `packages/` is covered the
/// day it lands.
#[test]
fn every_shipped_interface_passes_the_signature_checks() {
    const SOURCE: &str = "fn main() -> Unit {\n    return Unit\n}\n";

    let root = workspace_root();
    let mut paths = Vec::new();
    interface_files(&root.join("stdlib"), &mut paths);
    for package in {
        let mut packages = fs::read_dir(root.join("packages"))
            .expect("packages directory should exist")
            .map(|entry| entry.expect("packages entry").path())
            .collect::<Vec<_>>();
        packages.sort();
        packages
    } {
        let interface = package.join("interface");
        if interface.is_dir() {
            interface_files(&interface, &mut paths);
        }
    }
    assert!(
        paths.len() >= 30,
        "the shipped interface set should not have shrunk to {} files",
        paths.len()
    );

    // Supply the whole set at once: the interfaces reference each other's
    // protocols (`Ord`, `Eq`, `Hashable`), so checking one in isolation would
    // report a missing protocol that the prelude does in fact declare. Each
    // diagnostic still carries its own interface's path, so a failure names
    // the file to fix.
    let sources = paths
        .iter()
        .map(|path| {
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .to_string_lossy()
                .into_owned();
            let text = fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            (relative, text)
        })
        .collect::<Vec<_>>();
    let interfaces = sources
        .iter()
        .map(|(file, text)| (file.as_str(), text.as_str()))
        .collect::<Vec<_>>();

    let failures = analyze_sources_with_interfaces(&[("main.rss", SOURCE)], &interfaces)
        .into_iter()
        .map(|diagnostic| {
            format!(
                "{}:{}:{}: {}",
                diagnostic.span.file, diagnostic.span.line, diagnostic.span.column, diagnostic.code
            )
        })
        .collect::<Vec<_>>();

    assert!(
        failures.is_empty(),
        "{} diagnostics across the {} shipped interfaces:\n{}",
        failures.len(),
        paths.len(),
        failures.join("\n")
    );
}
