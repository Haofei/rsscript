use std::fs;
use std::path::Path;

use super::native::{
    native_binding_interface_sources, package_native_bindings, package_native_rust_dependencies,
};
use super::source_set::load_package;
use super::{
    PackageIdentity, PackageLoweringInput, PackageMetadataReport, PackageReview,
    PackageReviewFileKind, PackageReviewMetadata, PackageRisk,
    collect_dependency_interface_sources, review_package_dir,
};

pub fn package_metadata(
    package_dir: &Path,
    dry_run: bool,
) -> Result<PackageMetadataReport, String> {
    let review = review_package_dir(package_dir)?;
    let metadata_path = package_dir.join("review").join("package-review.json");
    let metadata = package_review_metadata_from_review(&review);
    let ok = review.summary.errors == 0 && review.risk != PackageRisk::Unknown;

    if !dry_run {
        let parent = metadata_path
            .parent()
            .ok_or_else(|| format!("metadata path has no parent: {}", metadata_path.display()))?;
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
        let json = serde_json::to_string_pretty(&metadata)
            .expect("package metadata JSON serialization should not fail");
        fs::write(&metadata_path, json)
            .map_err(|error| format!("failed to write {}: {error}", metadata_path.display()))?;
    }

    Ok(PackageMetadataReport {
        package: review.package,
        package_dir: package_dir.display().to_string(),
        metadata_path: metadata_path.display().to_string(),
        dry_run,
        written: !dry_run,
        ok,
        risk: review.risk,
        reasons: review.reasons,
        metadata,
    })
}

pub fn package_lowering_input(package_dir: &Path) -> Result<PackageLoweringInput, String> {
    let package = load_package(package_dir)?;
    let dependency_interfaces =
        collect_dependency_interface_sources(package_dir, &package.manifest)?;
    let native_dependencies = package_native_rust_dependencies(package_dir, &package.manifest)?;
    let native_bindings = package_native_bindings(package_dir)?;
    let native_binding_interfaces =
        native_binding_interface_sources(&package.sources, &native_bindings);
    let interfaces = dependency_interfaces
        .iter()
        .chain(native_binding_interfaces.iter())
        .map(|source| (source.path.clone(), source.contents.clone()))
        .collect::<Vec<_>>();

    let source_files = package
        .sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .collect::<Vec<_>>();
    let source = select_package_runnable_source(&source_files)?;
    let sources = source_files
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
        schema: "rss.review.package.v1".to_string(),
        package: review.package.clone(),
        risk: review.risk,
        reasons: review.reasons.clone(),
        features: review.features.clone(),
        summary: review.summary.clone(),
        files: review.files.clone(),
        exports: review.exports.clone(),
        native_rust: review.native_rust.clone(),
        review_map: review.review_map.clone(),
        diagnostics: review.diagnostics.clone(),
    }
}
