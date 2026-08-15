//! Captured package-review compatibility inputs.
//!
//! This crate owns the manifest/source-set representation used by optional
//! package review tooling. It is deliberately separate from the compiler's
//! normal in-memory frontend path: callers capture project input before asking
//! the compiler for semantic facts.

mod await_facts;
mod contract;
mod dependency;
mod execution_facts;
mod runtime_catalog;
mod source_set;

pub use await_facts::*;
pub use contract::*;
pub use dependency::*;
pub use execution_facts::*;
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
