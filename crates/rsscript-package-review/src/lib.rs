//! Captured package-review compatibility inputs.
//!
//! This crate owns the manifest/source-set representation used by optional
//! package review tooling. It is deliberately separate from the compiler's
//! normal in-memory frontend path: callers capture project input before asking
//! the compiler for semantic facts.

mod analysis;
mod await_facts;
mod bindings;
mod check;
mod contract;
mod dependency;
mod diff;
mod execution_facts;
mod graph;
mod lock;
mod lock_format;
mod policy;
mod review;
mod runtime_catalog;
mod source_set;

pub use analysis::*;
pub use await_facts::*;
pub use bindings::*;
pub use check::*;
pub use contract::*;
pub use dependency::*;
pub use diff::*;
pub use execution_facts::*;
pub use graph::*;
pub use lock::*;
pub use lock_format::*;
pub use policy::*;
pub use review::*;
pub use source_set::*;

use std::collections::BTreeSet;
use std::sync::Arc;

use rsscript_semantics::{AnalysisResult, CompilationSession, core_interfaces};

/// Analyze already-captured package sources through the same semantic session
/// boundary used by normal compiler and language-service callers.
pub fn session_analysis(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> Arc<AnalysisResult> {
    let core_paths = core_interfaces()
        .iter()
        .map(|(path, _)| *path)
        .collect::<BTreeSet<_>>();
    let mut session = CompilationSession::default();
    for (path, contents) in interfaces {
        if !core_paths.contains(path) {
            session
                .set_interface(*path, *contents)
                .expect("captured package interfaces have unique normalized paths");
        }
    }
    for (path, contents) in sources {
        session
            .set_file(*path, *contents)
            .expect("captured package sources have unique normalized paths");
    }
    session.workspace_analysis()
}

/// Return the package's captured source and interface files in the legacy
/// presentation model without routing through compiler compatibility code.
pub fn package_sources(
    package_dir: &std::path::Path,
) -> Result<Vec<rsscript_package_model::PackageSourceFile>, String> {
    let package = load_package(package_dir)?;
    Ok(package_source_files(package.sources))
}

/// Return captured package files plus resolved dependency interfaces for
/// compatibility tools that still present the expanded source set.
pub fn package_sources_with_dependency_interfaces(
    package_dir: &std::path::Path,
) -> Result<Vec<rsscript_package_model::PackageSourceFile>, String> {
    let package = load_package(package_dir)?;
    let mut sources = package.sources;
    sources.extend(collect_dependency_interface_sources(
        package_dir,
        &package.manifest,
    )?);
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(package_source_files(sources))
}

fn package_source_files(
    sources: Vec<PackageSource>,
) -> Vec<rsscript_package_model::PackageSourceFile> {
    sources
        .into_iter()
        .map(|source| rsscript_package_model::PackageSourceFile {
            path: source.path,
            relative_path: source.relative_path,
            contents: source.contents,
            kind: source.kind,
        })
        .collect()
}
