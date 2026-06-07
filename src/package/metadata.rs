use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::format::format_package_review_reir_json;
use super::native::{
    native_binding_interface_sources, package_native_bindings, package_native_rust_dependencies,
};
use super::source_set::load_package;
use super::{
    PACKAGE_REVIEW_METADATA_SCHEMA, PackageIdentity, PackageLoweringInput, PackageMetadataMismatch,
    PackageMetadataReport, PackageReview, PackageReviewFileKind, PackageReviewMetadata,
    PackageRisk, collect_dependency_interface_sources, collect_dependency_lowering_sources,
    review_package_dir,
};

pub fn package_metadata(
    package_dir: &Path,
    dry_run: bool,
) -> Result<PackageMetadataReport, String> {
    package_metadata_inner(package_dir, dry_run, false)
}

pub fn package_metadata_verify(package_dir: &Path) -> Result<PackageMetadataReport, String> {
    package_metadata_inner(package_dir, true, true)
}

fn package_metadata_inner(
    package_dir: &Path,
    dry_run: bool,
    verify: bool,
) -> Result<PackageMetadataReport, String> {
    let review = review_package_dir(package_dir)?;
    let metadata_path = package_dir.join("review").join("package-review.json");
    let reir_path = package_dir
        .join("review")
        .join("reir")
        .join("rsscript.json");
    let metadata = package_review_metadata_from_review(&review);
    let metadata_json = serde_json::to_string_pretty(&metadata)
        .expect("package metadata JSON serialization should not fail");
    let reir_json = format_package_review_reir_json(&review);
    let mut mismatches = Vec::new();

    // Fail closed: a package with error diagnostics must not produce authoritative
    // review/REIR artifacts on disk — downstream gates consume those facts and
    // would treat invalid source as evidence.
    let error_count = review.summary.errors;
    let blocked_by_errors = error_count > 0;

    if verify {
        verify_artifact(
            "package_review",
            &metadata_path,
            &metadata_json,
            &mut mismatches,
        );
        verify_artifact("reir_bundle", &reir_path, &reir_json, &mut mismatches);
    } else if !dry_run && !blocked_by_errors {
        create_parent_dir(&metadata_path, "metadata path")?;
        create_parent_dir(&reir_path, "REIR path")?;
        fs::write(&metadata_path, metadata_json)
            .map_err(|error| format!("failed to write {}: {error}", metadata_path.display()))?;
        fs::write(&reir_path, reir_json)
            .map_err(|error| format!("failed to write {}: {error}", reir_path.display()))?;
    }

    let wrote = !dry_run && !verify && !blocked_by_errors;
    let mut reasons = review.reasons;
    if blocked_by_errors && !dry_run && !verify {
        reasons.push(format!(
            "refused to write authoritative review/REIR artifacts: package has {error_count} \
             error diagnostic(s); fix them and re-run"
        ));
    }

    let ok = error_count == 0 && review.risk != PackageRisk::Unknown && mismatches.is_empty();
    Ok(PackageMetadataReport {
        package: review.package,
        package_dir: package_dir.display().to_string(),
        metadata_path: metadata_path.display().to_string(),
        reir_path: reir_path.display().to_string(),
        dry_run,
        written: wrote,
        verified: verify && mismatches.is_empty(),
        ok,
        risk: review.risk,
        reasons,
        mismatches,
        metadata,
    })
}

fn verify_artifact(
    artifact: &str,
    path: &Path,
    expected: &str,
    mismatches: &mut Vec<PackageMetadataMismatch>,
) {
    let expected_sha256 = sha256_label(expected.as_bytes());
    match fs::read_to_string(path) {
        Ok(actual) => {
            if actual == expected {
                return;
            }
            mismatches.push(PackageMetadataMismatch {
                artifact: artifact.to_string(),
                path: path.display().to_string(),
                kind: "stale".to_string(),
                message: "artifact does not match the current package review result".to_string(),
                expected_sha256,
                actual_sha256: Some(sha256_label(actual.as_bytes())),
            });
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            mismatches.push(PackageMetadataMismatch {
                artifact: artifact.to_string(),
                path: path.display().to_string(),
                kind: "missing".to_string(),
                message: "artifact is missing".to_string(),
                expected_sha256,
                actual_sha256: None,
            });
        }
        Err(error) => mismatches.push(PackageMetadataMismatch {
            artifact: artifact.to_string(),
            path: path.display().to_string(),
            kind: "unreadable".to_string(),
            message: format!("failed to read artifact: {error}"),
            expected_sha256,
            actual_sha256: None,
        }),
    }
}

fn sha256_label(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::from("sha256:");
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn create_parent_dir(path: &Path, label: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{label} has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create {}: {error}", parent.display()))
}

pub fn package_lowering_input(package_dir: &Path) -> Result<PackageLoweringInput, String> {
    let package = load_package(package_dir)?;
    let dependency_interfaces =
        collect_dependency_interface_sources(package_dir, &package.manifest)?;
    let dependency_sources = collect_dependency_lowering_sources(package_dir, &package.manifest)?;
    let native_dependencies = package_native_rust_dependencies(package_dir, &package.manifest)?;
    let native_bindings = package_native_bindings(package_dir)?;
    let native_binding_interfaces =
        native_binding_interface_sources(&package.sources, &native_bindings);
    let source_dependency_roots = dependency_sources
        .iter()
        .filter_map(package_source_root)
        .collect::<BTreeSet<_>>();
    let executable_dependency_interfaces = dependency_interfaces
        .iter()
        .filter(|source| {
            package_source_root(source)
                .map(|root| !source_dependency_roots.contains(&root))
                .unwrap_or(true)
        })
        .collect::<Vec<_>>();
    let interfaces = executable_dependency_interfaces
        .iter()
        .copied()
        .chain(native_binding_interfaces.iter())
        .map(|source| (source.path.clone(), source.contents.clone()))
        .collect::<Vec<_>>();

    let source_files = package
        .sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .collect::<Vec<_>>();
    let source = select_package_runnable_source(&source_files)?;
    let lowering_sources = dependency_sources
        .iter()
        .chain(source_files.iter().copied())
        .collect::<Vec<_>>();
    let sources = lowering_sources
        .iter()
        .map(|source| (source.path.clone(), source.contents.clone()))
        .collect::<Vec<_>>();
    Ok(PackageLoweringInput {
        package: PackageIdentity {
            name: package.manifest.package.name.clone(),
            version: package.manifest.package.version.clone(),
            edition: package.manifest.package.edition.clone(),
        },
        package_dir: package_dir.display().to_string(),
        source_path: source.path.clone(),
        source_relative_path: source.relative_path.clone(),
        source: source.contents.clone(),
        sources,
        interfaces,
        native_dependencies,
    })
}

fn package_source_root(source: &super::PackageSource) -> Option<PathBuf> {
    let mut root = Path::new(&source.path);
    for _ in Path::new(&source.relative_path).components() {
        root = root.parent()?;
    }
    Some(root.to_path_buf())
}

fn select_package_runnable_source<'a>(
    source_files: &[&'a super::PackageSource],
) -> Result<&'a super::PackageSource, String> {
    if source_files.is_empty() {
        return Err("rss run requires one package source file under `src`.".to_string());
    }

    let main_sources = source_files
        .iter()
        .copied()
        .filter(|source| source.relative_path == "src/main.rss")
        .collect::<Vec<_>>();
    if source_files.len() == 1 {
        return Ok(source_files[0]);
    }
    if main_sources.len() == 1 {
        return Ok(main_sources[0]);
    }

    Err(
        "rss run package lowering requires `src/main.rss` when a package has multiple `.rss` source files."
            .to_string(),
    )
}

fn package_review_metadata_from_review(review: &PackageReview) -> PackageReviewMetadata {
    PackageReviewMetadata {
        schema: PACKAGE_REVIEW_METADATA_SCHEMA.to_string(),
        package: review.package.clone(),
        risk: review.risk,
        reasons: review.reasons.clone(),
        features: review.features.clone(),
        virtual_package: review.virtual_package.clone(),
        implements: review.implements.clone(),
        dependencies: review.dependencies.clone(),
        summary: review.summary.clone(),
        files: review.files.clone(),
        exports: review.exports.clone(),
        capabilities: review.capabilities.clone(),
        await_sites: review.await_sites.clone(),
        native_rust: review.native_rust.clone(),
        review_map: review.review_map.clone(),
        diagnostics: review.diagnostics.clone(),
    }
}
