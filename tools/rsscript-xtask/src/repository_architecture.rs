use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

const UNSAFE_BOUNDARIES: &[(&str, &str, &str)] = &[
    ("rss-process-guard", "rss_process_guard", "process-guard"),
    ("rsscript-jit-cranelift", "vm_jit", "rsscript-jit-cranelift"),
];
const MAX_ARCHITECTURE_TEST_MODULE_BYTES: u64 = 45_000;

pub(super) fn validate(root: &Path) -> Result<(), Box<dyn Error>> {
    validate_architecture_test_module_sizes(root)?;
    validate_root_workspace_ownership(root)?;
    validate_compiler_manifest(root)?;
    validate_aot_runtime_boundary(root)?;
    validate_unsafe_crate_boundaries(root)?;
    validate_backend_dependency_direction(root)?;
    Ok(())
}

fn validate_architecture_test_module_sizes(root: &Path) -> Result<(), Box<dyn Error>> {
    let tests = root.join("crates/rsscript-sdk/tests");
    let mut paths = rust_files_below(&tests.join("architecture"))?;
    paths.extend([
        tests.join("architecture.rs"),
        tests.join("public_api_architecture.rs"),
    ]);

    for path in paths {
        let size = fs::metadata(&path)?.len();
        if size > MAX_ARCHITECTURE_TEST_MODULE_BYTES {
            return Err(format!(
                "architecture audit module {} is {size} bytes; split audit responsibilities below the {MAX_ARCHITECTURE_TEST_MODULE_BYTES}-byte limit",
                path.strip_prefix(root).unwrap_or(&path).display()
            )
            .into());
        }
    }
    Ok(())
}

fn validate_root_workspace_ownership(root: &Path) -> Result<(), Box<dyn Error>> {
    let members = workspace_members(root)?;
    let forbidden = BTreeSet::from([
        "rsscript-aot-backend",
        "rsscript-aot-model",
        "rsscript-aot-runtime",
        "rss-native-abi",
        "reir",
        "rss-testgen",
        "rsscript-review-reir",
    ]);
    let experiments = root.join("experiments").canonicalize()?;

    for member in members {
        let manifest_path = member.join("Cargo.toml");
        let manifest = read_toml(&manifest_path)?;
        let package = package_name(&manifest)?;
        if forbidden.contains(package) {
            return Err(format!("root workspace owns experimental package `{package}`").into());
        }
        for dependency_path in dependency_paths(&manifest) {
            let resolved = member.join(dependency_path).canonicalize()?;
            if resolved.starts_with(&experiments) {
                return Err(format!(
                    "root package `{package}` path-depends on experiments at {}",
                    resolved.display()
                )
                .into());
            }
        }
    }
    Ok(())
}

fn validate_compiler_manifest(root: &Path) -> Result<(), Box<dyn Error>> {
    let manifest = read_toml(&root.join("crates/rsscript-compiler/Cargo.toml"))?;
    if manifest
        .get("dev-dependencies")
        .and_then(toml::Value::as_table)
        .is_some_and(|dependencies| !dependencies.is_empty())
    {
        return Err("Core compiler must not retain research/fuzz dev-dependencies".into());
    }
    Ok(())
}

fn validate_aot_runtime_boundary(root: &Path) -> Result<(), Box<dyn Error>> {
    let directory = root.join("experiments/aot-runtime");
    let manifest = read_toml(&directory.join("Cargo.toml"))?;
    if package_name(&manifest)? != "rsscript-aot-runtime" {
        return Err("AOT runtime package identity drifted".into());
    }
    let dependencies = dependency_packages(&manifest);
    for forbidden in [
        "rsscript",
        "rsscript-sdk",
        "rand",
        "reqwest",
        "rss-process-guard",
        "tokio-tungstenite",
        "toml",
        "uuid",
    ] {
        if dependencies.contains(forbidden) {
            return Err(format!("AOT runtime depends on forbidden `{forbidden}`").into());
        }
    }
    for feature in ["host-compat", "net"] {
        if manifest["features"].get(feature).is_some() {
            return Err(format!("AOT runtime retains legacy feature `{feature}`").into());
        }
    }
    for path in rust_files_below(&directory.join("src"))? {
        if fs::read_to_string(&path)?.contains("rsscript_sdk::") {
            return Err(format!(
                "AOT runtime source {} imports the product SDK",
                path.strip_prefix(root).unwrap_or(&path).display()
            )
            .into());
        }
    }
    Ok(())
}

fn validate_unsafe_crate_boundaries(root: &Path) -> Result<(), Box<dyn Error>> {
    for member in workspace_members(root)? {
        let manifest = read_toml(&member.join("Cargo.toml"))?;
        let package = package_name(&manifest)?;
        let dependencies = dependency_packages(&manifest);
        let source_root = member.join("src");
        if !source_root.is_dir() {
            continue;
        }
        for path in rust_files_below(&source_root)? {
            // The validator necessarily names the boundaries it enforces; those
            // string literals are not Rust imports or source inclusion.
            if package == "rsscript-xtask"
                && path
                    .file_name()
                    .is_some_and(|name| name == "repository_architecture.rs")
            {
                continue;
            }
            let source = fs::read_to_string(&path)?;
            for (boundary_package, rust_name, directory) in UNSAFE_BOUNDARIES {
                if package != *boundary_package
                    && source.contains(rust_name)
                    && !dependencies.contains(*boundary_package)
                {
                    return Err(format!(
                        "{} references `{rust_name}` without `{boundary_package}` dependency",
                        path.strip_prefix(root).unwrap_or(&path).display()
                    )
                    .into());
                }
                if package != *boundary_package
                    && (source.contains("#[path") || source.contains("include!"))
                    && source.contains(directory)
                {
                    return Err(format!(
                        "{} imports unsafe boundary `{boundary_package}` by source path",
                        path.strip_prefix(root).unwrap_or(&path).display()
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

fn validate_backend_dependency_direction(root: &Path) -> Result<(), Box<dyn Error>> {
    let forbidden = [
        "rsscript_compiler",
        "rsscript_syntax",
        "rsscript_semantics",
        "rsscript_lowering",
        "crate::hir",
        "crate::syntax",
        "crate::semantic",
        "typed_hir()",
    ];
    for (name, directory) in [
        ("VM", root.join("crates/rsscript-vm/src/reg_vm")),
        ("MIR codegen", root.join("crates/rsscript-codegen-vm/src")),
        (
            "optional JIT engine",
            root.join("crates/rsscript-jit-cranelift/src"),
        ),
    ] {
        for path in rust_files_below(&directory)? {
            let source = fs::read_to_string(&path)?;
            for token in forbidden {
                if source.contains(token) {
                    return Err(format!(
                        "{name} backend {} references frontend token `{token}`",
                        path.strip_prefix(root).unwrap_or(&path).display()
                    )
                    .into());
                }
            }
        }
    }
    for manifest in [
        "crates/rsscript-vm/Cargo.toml",
        "crates/rsscript-codegen-vm/Cargo.toml",
        "crates/rsscript-jit-cranelift/Cargo.toml",
    ] {
        let dependencies = dependency_packages(&read_toml(&root.join(manifest))?);
        for package in [
            "rsscript-compiler",
            "rsscript-syntax",
            "rsscript-semantics",
            "rsscript-lowering",
        ] {
            if dependencies.contains(package) {
                return Err(format!("backend `{manifest}` depends on frontend `{package}`").into());
            }
        }
    }
    Ok(())
}

fn workspace_members(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let manifest = read_toml(&root.join("Cargo.toml"))?;
    manifest["workspace"]["members"]
        .as_array()
        .ok_or_else(|| "workspace.members must be an array".into())
        .and_then(|members| {
            members
                .iter()
                .map(|member| {
                    member
                        .as_str()
                        .map(|member| root.join(member))
                        .ok_or_else(|| "workspace member must be a string".into())
                })
                .collect()
        })
}

fn read_toml(path: &Path) -> Result<toml::Value, Box<dyn Error>> {
    Ok(toml::from_str(&fs::read_to_string(path)?)?)
}

fn package_name(manifest: &toml::Value) -> Result<&str, Box<dyn Error>> {
    manifest["package"]["name"]
        .as_str()
        .ok_or_else(|| "package.name must be a string".into())
}

fn dependency_packages(manifest: &toml::Value) -> BTreeSet<String> {
    let mut packages = BTreeSet::new();
    collect_dependency_tables(manifest, &mut |name, value| {
        packages.insert(
            value
                .as_table()
                .and_then(|table| table.get("package"))
                .and_then(toml::Value::as_str)
                .unwrap_or(name)
                .to_owned(),
        );
    });
    packages
}

fn dependency_paths(manifest: &toml::Value) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    collect_dependency_tables(manifest, &mut |_name, value| {
        if let Some(path) = value
            .as_table()
            .and_then(|table| table.get("path"))
            .and_then(toml::Value::as_str)
        {
            paths.push(PathBuf::from(path));
        }
    });
    paths
}

fn collect_dependency_tables(value: &toml::Value, visit: &mut impl FnMut(&str, &toml::Value)) {
    let Some(table) = value.as_table() else {
        return;
    };
    for (key, value) in table {
        if matches!(
            key.as_str(),
            "dependencies" | "dev-dependencies" | "build-dependencies"
        ) {
            if let Some(dependencies) = value.as_table() {
                for (name, specification) in dependencies {
                    visit(name, specification);
                }
            }
        } else {
            collect_dependency_tables(value, visit);
        }
    }
}

fn rust_files_below(root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
        let mut entries = fs::read_dir(directory)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                visit(&path, files)?;
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(root, &mut files)?;
    Ok(files)
}
