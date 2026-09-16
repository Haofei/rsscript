//! Takes the `tests/fixtures/pass` corpus all the way through build and
//! Artifact verification.
//!
//! `fixture_corpus` proves the checker accepts these files. That is not the
//! same claim as "these files run": every failure this task fixed checked
//! clean and then died in the checked-HIR-to-MIR lowerer, which is the worst
//! shape a failure can take for a language whose programs are written by models
//! and validated by the checker. A `pass` fixture that stops being buildable
//! now fails here instead of being discovered by a person running the program.
//!
//! Fixtures that name an `// interface:` sibling are out of scope: they model a
//! host contract whose implementation is not present, so there is nothing to
//! execute. Fixtures with no `fn main` are libraries and are likewise skipped.
//! Everything else must build and verify, or be named in `CANNOT_BUILD_YET`
//! with the reason.

#[path = "support/fixture_support.rs"]
mod fixture_support;

use std::fs;

use fixture_support::{companions, fixture_sources, fixtures_root, name, parse_header};
use rsscript_sdk::{ArtifactVerifier, Compiler};
use rsscript_semantics::{FrontendInputSnapshot, standard_package_interfaces};

/// `pass` fixtures the checker accepts but the MIR lowerer cannot yet build,
/// each with the lowering construct that refuses it. Every entry is a real gap
/// in the typed MIR subset, not a fixture defect: the program is valid RSScript
/// and `rss check` reports nothing, so the split is recorded here rather than
/// hidden. Removing an entry is the acceptance test for closing its gap.
const CANNOT_BUILD_YET: &[(&str, &str)] = &[
    (
        "core-receiver-call.rss",
        "`List.push with invalid checked call shape`: the receiver-call \
         spelling of a mutating core list method is not lowered.",
    ),
    (
        "list_patterns.rss",
        "`non-literal checked HIR match pattern`: list patterns have no MIR \
         match form.",
    ),
    (
        "positional-multifield-nested.rss",
        "`nested checked HIR variant match binding`: a variant pattern that \
         binds through another pattern is not lowered.",
    ),
    (
        "positional-multifield-variant.rss",
        "`non-literal checked HIR match pattern`: a positional multi-field \
         variant pattern is not lowered.",
    ),
];

/// Fixtures whose `main` is not the entry point of a self-contained program.
fn has_main(source: &str) -> bool {
    source
        .lines()
        .any(|line| line.trim_start().starts_with("fn main("))
}

#[test]
fn pass_fixtures_build_and_verify() {
    let directory = fixtures_root().join("pass");
    let paths = fixture_sources(&directory);
    assert!(!paths.is_empty(), "the pass corpus should not be empty");

    let mut failures = Vec::new();
    let mut unexpectedly_built = Vec::new();
    let mut considered = 0usize;
    for path in &paths {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("read fixture {}: {error}", path.display()));
        let header = parse_header(&source);
        if !header.interfaces.is_empty() || !has_main(&source) {
            continue;
        }
        considered += 1;

        let companion_sources = companions(path, &header.sources, "source");
        let file = path.to_string_lossy().into_owned();
        let mut sources = vec![(file.as_str(), source.as_str())];
        sources.extend(
            companion_sources
                .iter()
                .map(|(file, contents)| (file.as_str(), contents.as_str())),
        );
        let snapshot = FrontendInputSnapshot::from_sources(
            sources,
            standard_package_interfaces().iter().copied(),
        );

        let allowed = CANNOT_BUILD_YET
            .iter()
            .find(|(fixture, _)| *fixture == name(path));
        let outcome = Compiler
            .compile_snapshot(&snapshot)
            .map_err(|error| error.to_string())
            .and_then(|built| {
                ArtifactVerifier
                    .verify(built)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            });

        match (outcome, allowed) {
            (Ok(()), None) => {}
            (Ok(()), Some((fixture, reason))) => unexpectedly_built.push(format!(
                "{fixture}: builds and verifies now — remove it from CANNOT_BUILD_YET ({reason})"
            )),
            (Err(error), Some(_)) => {
                assert!(
                    !error.is_empty(),
                    "{}: a refusal must carry a reason",
                    name(path)
                );
            }
            (Err(error), None) => failures.push(format!("{}: {error}", name(path))),
        }
    }

    assert!(considered > 0, "no buildable pass fixture was considered");
    assert!(
        failures.is_empty() && unexpectedly_built.is_empty(),
        "{} of {considered} buildable pass fixtures did not build and verify:\n{}\n{}",
        failures.len(),
        failures.join("\n"),
        unexpectedly_built.join("\n")
    );
}
