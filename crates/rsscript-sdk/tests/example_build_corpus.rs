//! Takes the checked-in example programs all the way through build and
//! Artifact verification.
//!
//! `examples/` is the product's front door: the README, the language card, and
//! the getting-started documentation all point at these files, and a reader's
//! first act is to run one. `fixture_build_corpus` makes that guarantee for the
//! `tests/fixtures/pass` corpus; this makes the same guarantee for the programs
//! a person actually opens. It is not a theoretical concern —
//! `examples/scripts/core/interpreter_pure_parity.rss` checked clean and then
//! died in the checked-HIR-to-MIR lowerer on its struct pattern, which is the
//! worst shape a failure can take for a published example.
//!
//! Two layouts are covered. `examples/scripts/**/*.rss` are self-contained
//! single-file programs. `examples/<name>/script/*.rss` are the embedding
//! examples, each of which is built against every `.rssi` in its own
//! `examples/<name>/interfaces/` directory, exactly as
//! `rss build --interface …` would. An example that cannot build is named in
//! `CANNOT_BUILD_YET` with the reason, and the test fails if one of those
//! starts building.

use std::fs;
use std::path::{Path, PathBuf};

use rsscript_sdk::{ArtifactVerifier, Compiler};
use rsscript_semantics::{FrontendInputSnapshot, standard_package_interfaces};

/// Examples the checker accepts but the build path cannot yet produce an
/// Artifact for, each with the construct that refuses it, keyed by the
/// example's path under `examples/`.
///
/// The list is empty: every checked-in example builds and verifies. Add an
/// entry only for a gap that is being recorded, never to make a regression
/// quiet — an entry that starts building fails this test.
const CANNOT_BUILD_YET: &[(&str, &str)] = &[];

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

/// Every `.rss` file under `directory`, recursively, sorted so failures report
/// stably.
fn scripts_under(directory: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        {
            let path = entry.expect("example directory entry").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rss") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    paths
}

/// The `.rssi` files an example directory publishes, if it has any.
fn interfaces_of(example: &Path) -> Vec<PathBuf> {
    let directory = example.join("interfaces");
    if !directory.is_dir() {
        return Vec::new();
    }
    let mut paths = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()))
        .map(|entry| entry.expect("interface directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "rssi")
        })
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

/// Every example program paired with the interfaces it is built against.
fn example_programs() -> Vec<(PathBuf, Vec<PathBuf>)> {
    let root = examples_root();
    let mut programs = scripts_under(&root.join("scripts"))
        .into_iter()
        .map(|path| (path, Vec::new()))
        .collect::<Vec<_>>();

    let mut directories = fs::read_dir(&root)
        .unwrap_or_else(|error| panic!("read {}: {error}", root.display()))
        .map(|entry| entry.expect("examples directory entry").path())
        .filter(|path| path.is_dir() && path.file_name().is_some_and(|name| name != "scripts"))
        .collect::<Vec<_>>();
    directories.sort();
    for example in directories {
        let scripts = example.join("script");
        if !scripts.is_dir() {
            continue;
        }
        let interfaces = interfaces_of(&example);
        programs.extend(
            scripts_under(&scripts)
                .into_iter()
                .map(|path| (path, interfaces.clone())),
        );
    }
    programs
}

/// The example's path relative to `examples/`, which is how it is named in
/// `CANNOT_BUILD_YET` and in a failure report.
fn name(path: &Path) -> String {
    path.strip_prefix(examples_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn build_and_verify(path: &Path, interfaces: &[PathBuf]) -> Result<(), String> {
    let source =
        fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let interface_sources = interfaces
        .iter()
        .map(|interface| {
            let contents = fs::read_to_string(interface)
                .unwrap_or_else(|error| panic!("read {}: {error}", interface.display()));
            (interface.to_string_lossy().into_owned(), contents)
        })
        .collect::<Vec<_>>();
    let mut assembled = standard_package_interfaces().to_vec();
    assembled.extend(
        interface_sources
            .iter()
            .map(|(path, contents)| (path.as_str(), contents.as_str())),
    );
    let file = path.to_string_lossy().into_owned();
    let snapshot =
        FrontendInputSnapshot::from_sources([(file.as_str(), source.as_str())], assembled);

    Compiler
        .compile_snapshot(&snapshot)
        .map_err(|error| error.to_string())
        .and_then(|built| {
            ArtifactVerifier
                .verify(built)
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
}

#[test]
fn examples_build_and_verify() {
    let programs = example_programs();
    assert!(
        programs.len() > 10,
        "the example corpus should not have shrunk to {} programs",
        programs.len()
    );

    let mut failures = Vec::new();
    let mut unexpectedly_built = Vec::new();
    for (path, interfaces) in &programs {
        let name = name(path);
        let allowed = CANNOT_BUILD_YET
            .iter()
            .find(|(example, _)| *example == name);
        match (build_and_verify(path, interfaces), allowed) {
            (Ok(()), None) => {}
            (Ok(()), Some((example, reason))) => unexpectedly_built.push(format!(
                "{example}: builds and verifies now — remove it from CANNOT_BUILD_YET ({reason})"
            )),
            (Err(error), Some(_)) => {
                assert!(!error.is_empty(), "{name}: a refusal must carry a reason");
            }
            (Err(error), None) => failures.push(format!("{name}: {error}")),
        }
    }

    assert!(
        failures.is_empty() && unexpectedly_built.is_empty(),
        "{} of {} examples did not build and verify:\n{}\n{}",
        failures.len(),
        programs.len(),
        failures.join("\n"),
        unexpectedly_built.join("\n")
    );
}
