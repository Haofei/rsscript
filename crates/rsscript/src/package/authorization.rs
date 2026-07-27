use std::path::{Path, PathBuf};

use super::{
    NativePluginBuildDependency, PackageLoweringInput, check_package_dir, package_lowering_input,
    package_native_plugin_build_dependencies,
};

/// A package whose review, lock, dependency graph, and native policy checks
/// succeeded.
///
/// Values can only be created by [`prepare_authorized_package`]. Native build
/// and load code consumes this type instead of accepting an unchecked path.
#[derive(Debug)]
pub struct AuthorizedPackage {
    package_dir: PathBuf,
    lowering_input: PackageLoweringInput,
    native_build_dependencies: Vec<NativePluginBuildDependency>,
}

impl AuthorizedPackage {
    pub fn package_dir(&self) -> &Path {
        &self.package_dir
    }

    /// Return the checked lowering snapshot captured during authorization.
    ///
    /// AOT callers must use this snapshot rather than re-reading the package
    /// path after authorization.
    pub fn lowering_input(&self) -> &PackageLoweringInput {
        &self.lowering_input
    }

    pub(crate) fn native_build_dependencies(&self) -> &[NativePluginBuildDependency] {
        &self.native_build_dependencies
    }
}

/// Review and authorize a package before any native build or dynamic load.
///
/// The returned value also captures the lowering and native dependency inputs
/// used by the loader, so the loader cannot independently rediscover an
/// unchecked package graph.
pub fn prepare_authorized_package(package_dir: &Path) -> Result<AuthorizedPackage, String> {
    let check = check_package_dir(package_dir)?;
    if !check.ok {
        let reasons = if check.reasons.is_empty() {
            "package check did not authorize native execution".to_string()
        } else {
            check.reasons.join("; ")
        };
        return Err(format!(
            "native build/load denied because package review or policy did not authorize execution: {reasons}"
        ));
    }

    let package_dir = package_dir.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize authorized package {}: {error}",
            package_dir.display()
        )
    })?;
    let lowering_input = package_lowering_input(&package_dir)?;
    let native_build_dependencies = package_native_plugin_build_dependencies(&package_dir)?;

    Ok(AuthorizedPackage {
        package_dir,
        lowering_input,
        native_build_dependencies,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::native_plugin::load_authorized_package_native_bindings;
    use crate::package::{format_package_lock_toml, lock_package_dir};

    fn pure_package_fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "rss-authorized-package-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("src")).expect("fixture source directory");
        fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"authorized-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n",
        )
        .expect("fixture manifest");
        fs::write(
            root.join("src/main.rss"),
            "fn main() -> Unit { return Unit }\n",
        )
        .expect("fixture source");
        root
    }

    fn add_native_dependency(root: &Path) {
        fs::write(
            root.join("rsspkg.toml"),
            "[package]\nname = \"authorized-test\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[sources]\npaths = [\"src\"]\n\n[native.rust]\nenabled = true\npath = \"native/rust\"\ncrate = \"authorized_test_native\"\nbuild_scripts = \"forbid\"\nproc_macros = \"forbid\"\nunsafe = \"forbid\"\n",
        )
        .expect("fixture native manifest declaration");
        fs::create_dir_all(root.join("native/rust/src")).expect("fixture native source directory");
        fs::write(
            root.join("native/rust/Cargo.toml"),
            "[package]\nname = \"authorized_test_native\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .expect("fixture native Cargo manifest");
        fs::write(root.join("native/rust/src/lib.rs"), "pub fn unused() {}\n")
            .expect("fixture native source");
    }

    #[test]
    fn successful_check_is_the_only_authorized_package_constructor() {
        let root = pure_package_fixture();
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), format_package_lock_toml(&lock))
            .expect("fixture lockfile");

        let package = prepare_authorized_package(&root).expect("checked fixture should authorize");
        assert_eq!(
            package.package_dir(),
            root.canonicalize().expect("canonical fixture")
        );
        assert_eq!(package.lowering_input().package.name, "authorized-test");
        assert!(package.native_build_dependencies().is_empty());
        assert!(
            load_authorized_package_native_bindings(&package)
                .expect("pure authorized package should load without native work")
                .is_empty()
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_check_cannot_produce_an_authorized_package() {
        let root = pure_package_fixture();

        let error =
            prepare_authorized_package(&root).expect_err("missing lock must prevent authorization");
        assert!(error.contains("native build/load denied"), "{error}");
        assert!(error.contains("rsspkg.lock missing"), "{error}");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn successful_native_authorization_captures_checked_build_inputs() {
        let root = pure_package_fixture();
        add_native_dependency(&root);
        let lock = lock_package_dir(&root).expect("fixture lock");
        fs::write(root.join("rsspkg.lock"), format_package_lock_toml(&lock))
            .expect("fixture lockfile");

        let package =
            prepare_authorized_package(&root).expect("checked native fixture should authorize");
        assert_eq!(package.lowering_input().native_dependencies.len(), 1);
        assert_eq!(package.native_build_dependencies().len(), 1);
        assert_eq!(
            package.native_build_dependencies()[0].crate_name,
            "authorized_test_native"
        );

        let _ = fs::remove_dir_all(root);
    }
}
