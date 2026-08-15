#![allow(
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::derivable_impls,
    clippy::doc_lazy_continuation,
    clippy::if_same_then_else,
    clippy::items_after_test_module,
    clippy::let_and_return,
    clippy::manual_contains,
    clippy::manual_slice_fill,
    clippy::mutable_key_type,
    clippy::needless_borrow,
    clippy::needless_lifetimes,
    clippy::needless_range_loop,
    clippy::nonminimal_bool,
    clippy::op_ref,
    clippy::ptr_arg,
    clippy::question_mark,
    clippy::redundant_closure,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_lazy_evaluations,
    clippy::useless_conversion
)]
// Compatibility package/review tooling keeps its lint debt local to this module.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::diagnostic::Diagnostic;
#[cfg(test)]
pub(super) use rsscript_project::relative_project_path_label as relative_path;
pub(crate) use rsscript_project::{
    ProjectTreeLimits as TreeLimits, read_project_utf8_file_bounded as read_utf8_file_bounded,
};
pub(super) use rsscript_project::{
    canonical_project_path_label as canonical_path_label,
    project_path_source as package_path_source,
};

mod analysis {
    pub(super) use rsscript_package_review::analyze_package_dir_captured;
}
mod authorization;
mod check;
mod dependency {
    pub(super) use rsscript_package_review::*;
}
mod graph;
// Legacy composition only: package lock semantics and hashing are owned by
// `rsscript-package-review`; the compiler supplies native-wrapper callbacks.
mod lock {
    use std::path::Path;

    use rsscript_package_model::{PackageLock, PackageLockDiff};

    pub(super) use rsscript_package_review::{
        compare_locked_packages, effective_interface_hash, package_lock_diff_reasons,
        parse_package_lock, read_package_lock,
    };
    pub(super) const PACKAGE_LOCK_MAX_BYTES: u64 = rsscript_package_review::PACKAGE_LOCK_MAX_BYTES;

    pub(super) fn lock_package_dir_captured(package_dir: &Path) -> Result<PackageLock, String> {
        rsscript_package_review::lock_package_dir_captured(
            package_dir,
            super::native::package_native_rust_review,
            super::native::confined_native_rust_path,
        )
    }

    pub(super) fn lock_package_dir(package_dir: &Path) -> Result<PackageLock, String> {
        let snapshot = super::authorization::snapshot_package_graph_inputs(package_dir)?;
        let mut lock = lock_package_dir_captured(snapshot.root())
            .map_err(|error| snapshot.remap_error(error))?;
        super::authorization::remap_lock(&snapshot, &mut lock);
        Ok(lock)
    }

    pub(super) fn diff_package_locks(
        old_path: &Path,
        new_path: &Path,
    ) -> Result<PackageLockDiff, String> {
        rsscript_package_review::diff_package_locks(old_path, new_path)
    }
}
mod lock_format;
mod metadata;
mod native;
mod policy {
    pub(super) use rsscript_package_review::*;
}
// Legacy composition only: review evidence is package-review-owned. Native
// Rust inspection remains an opt-in compiler compatibility adapter.
mod review {
    use std::path::Path;

    use rsscript_package_model::PackageReview;

    pub(super) fn review_package_dir_captured_with_features(
        package_dir: &Path,
        selected_features: Option<&[String]>,
    ) -> Result<PackageReview, String> {
        rsscript_package_review::review_package_dir_captured_with_features(
            package_dir,
            selected_features,
            super::native::package_native_rust_review,
        )
    }
}
// The legacy package compatibility façade keeps this module name only so its
// remaining callers can migrate incrementally. Captured manifests and source
// sets are physically owned by the independent package-review boundary.
mod source_set {
    pub(super) use rsscript_package_review::*;
}

const PACKAGE_MANIFEST_MAX_BYTES: u64 = 1024 * 1024;

pub fn analyze_package_dir(package_dir: &Path) -> Result<PackageAnalysis, String> {
    authorization::load_workspace_snapshot(package_dir).map(|snapshot| snapshot.analysis().clone())
}
pub fn review_package_dir(package_dir: &Path) -> Result<PackageReview, String> {
    let snapshot = authorization::snapshot_package_graph_inputs(package_dir)?;
    let mut review = review::review_package_dir_captured_with_features(snapshot.root(), None)
        .map_err(|error| snapshot.remap_error(error))?;
    authorization::remap_review(&snapshot, &mut review);
    Ok(review)
}
pub fn diff_package_dirs(old_dir: &Path, new_dir: &Path) -> Result<PackageDiff, String> {
    rsscript_package_review::diff_package_dirs_with_native_review(
        old_dir,
        new_dir,
        native::package_native_rust_review,
    )
}
pub fn lock_package_dir(package_dir: &Path) -> Result<PackageLock, String> {
    lock::lock_package_dir(package_dir)
}
pub fn diff_package_locks(old_path: &Path, new_path: &Path) -> Result<PackageLockDiff, String> {
    lock::diff_package_locks(old_path, new_path)
}
pub use authorization::{
    ExecutablePackageSnapshot, PreparedPackage, WorkspaceSnapshot, load_workspace_snapshot,
    load_workspace_snapshot_with_operation, prepare_executable_package,
    prepare_package_for_execution,
};
pub use check::check_package_dir;
use dependency::{
    PackageDependencySpec, collect_dependency_interface_sources,
    collect_dependency_lowering_sources,
};
pub use graph::package_tree;
pub(super) use lock_format::package_lock_toml;
pub use metadata::package_lowering_input;
pub(crate) use native::package_native_plugin_build_dependencies;
pub use rsscript_package_model::*;
use source_set::{Manifest, ManifestNativeRust, PackageSource};

pub fn package_sources(package_dir: &Path) -> Result<Vec<PackageSourceFile>, String> {
    let package = source_set::load_package(package_dir)?;
    Ok(package_source_files(package.sources))
}

pub fn package_sources_with_dependency_interfaces(
    package_dir: &Path,
) -> Result<Vec<PackageSourceFile>, String> {
    let package = source_set::load_package(package_dir)?;
    let mut sources = package.sources;
    sources.extend(collect_dependency_interface_sources(
        package_dir,
        &package.manifest,
    )?);
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(package_source_files(sources))
}

fn package_source_files(sources: Vec<PackageSource>) -> Vec<PackageSourceFile> {
    sources
        .into_iter()
        .map(|source| PackageSourceFile {
            path: source.path,
            relative_path: source.relative_path,
            contents: source.contents,
            kind: source.kind,
        })
        .collect()
}

fn collect_regular_files(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    files.extend(
        rsscript_project::collect_project_regular_files(
            path,
            rsscript_project::ProjectTreeLimits::default(),
            "package file scan",
            |_, name| matches!(name, "target" | ".git" | ".DS_Store" | "Cargo.lock"),
        )?
        .into_iter()
        .map(|file| file.path),
    );
    Ok(())
}

pub(super) fn dedup_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    let mut seen = BTreeSet::new();
    diagnostics.retain(|diagnostic| {
        seen.insert((
            diagnostic.code.clone(),
            diagnostic.summary.clone(),
            diagnostic.span.file.clone(),
            diagnostic.span.line,
            diagnostic.span.column,
            diagnostic.span.length,
        ))
    });
}

fn package_identity(manifest: &Manifest) -> PackageIdentity {
    PackageIdentity {
        name: manifest.package.name.clone(),
        version: manifest.package.version.clone(),
        edition: manifest.package.edition.clone(),
    }
}

#[cfg(test)]
mod preparation_limit_tests {
    use super::*;
    use rsscript_project::collect_project_regular_files as collect_bounded_regular_files;
    use std::fs;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rss-package-preparation-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    #[test]
    fn bounded_tree_scan_enforces_file_byte_and_depth_limits() {
        let root = test_dir("tree-limits");
        fs::create_dir_all(root.join("one/two")).expect("fixture directories");
        fs::write(root.join("a"), b"1234").expect("first fixture");
        fs::write(root.join("one/b"), b"5678").expect("second fixture");
        fs::write(root.join("one/two/c"), b"9").expect("deep fixture");

        let file_error = collect_bounded_regular_files(
            &root,
            TreeLimits {
                max_files: 2,
                max_entries: 10,
                max_bytes: 100,
                max_depth: 10,
            },
            "test scan",
            |_, _| false,
        )
        .expect_err("third file must exceed file budget");
        assert!(file_error.contains("file count limit"), "{file_error}");

        let byte_error = collect_bounded_regular_files(
            &root,
            TreeLimits {
                max_files: 10,
                max_entries: 10,
                max_bytes: 7,
                max_depth: 10,
            },
            "test scan",
            |_, _| false,
        )
        .expect_err("eight bytes must exceed byte budget");
        assert!(byte_error.contains("total byte limit"), "{byte_error}");

        let depth_error = collect_bounded_regular_files(
            &root,
            TreeLimits {
                max_files: 10,
                max_entries: 10,
                max_bytes: 100,
                max_depth: 1,
            },
            "test scan",
            |_, _| false,
        )
        .expect_err("nested fixture must exceed depth budget");
        assert!(
            depth_error.contains("directory depth limit"),
            "{depth_error}"
        );
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn bounded_tree_scan_counts_directory_entries_before_sorting() {
        let root = test_dir("entry-limits");
        fs::create_dir_all(root.join("z")).expect("first directory");
        fs::create_dir_all(root.join("a")).expect("second directory");
        fs::create_dir_all(root.join("m")).expect("third directory");

        let error = collect_bounded_regular_files(
            &root,
            TreeLimits {
                max_files: 10,
                max_entries: 2,
                max_bytes: 100,
                max_depth: 10,
            },
            "test scan",
            |_, _| false,
        )
        .expect_err("third directory entry must exceed entry budget");
        assert!(error.contains("directory entry limit"), "{error}");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn bounded_utf8_read_rejects_oversized_input_before_reading_it_whole() {
        let root = test_dir("bounded-text");
        fs::create_dir_all(&root).expect("fixture directory");
        let path = root.join("Cargo.toml");
        fs::write(&path, b"12345").expect("fixture file");

        let error = read_utf8_file_bounded(&path, 4, "test manifest read")
            .expect_err("oversized manifest must be rejected");
        assert!(error.contains("byte limit of 4"), "{error}");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn bounded_tree_scan_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let root = test_dir("tree-symlink");
        fs::create_dir_all(&root).expect("fixture directory");
        fs::write(root.join("target"), b"x").expect("target fixture");
        symlink(root.join("target"), root.join("link")).expect("fixture symlink");

        let error =
            collect_bounded_regular_files(&root, TreeLimits::default(), "test scan", |_, _| false)
                .expect_err("symlink must be rejected");
        assert!(error.contains("rejects symlinks"), "{error}");
        fs::remove_dir_all(root).expect("fixture cleanup");
    }
}
