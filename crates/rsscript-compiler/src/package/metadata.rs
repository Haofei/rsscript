use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::native::{
    native_binding_interface_sources, package_external_bindings, package_native_rust_dependencies,
};
use super::source_set::load_package;
use super::{
    collect_dependency_interface_sources, collect_dependency_lowering_sources, PackageIdentity,
    PackageLoweringInput, PackageReviewFileKind,
};

pub fn package_lowering_input(package_dir: &Path) -> Result<PackageLoweringInput, String> {
    let package = load_package(package_dir)?;
    let dependency_interfaces =
        collect_dependency_interface_sources(package_dir, &package.manifest)?;
    let dependency_sources = collect_dependency_lowering_sources(package_dir, &package.manifest)?;
    let native_dependencies = package_native_rust_dependencies(package_dir, &package.manifest)?;
    let external_bindings = package_external_bindings(package_dir)?;
    let native_binding_interfaces =
        native_binding_interface_sources(&package.sources, &external_bindings);
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
