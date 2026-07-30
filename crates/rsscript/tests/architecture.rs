use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

struct BoundaryCrate {
    package: &'static str,
    rust_name: &'static str,
    directory: &'static str,
}

const UNSAFE_BOUNDARY_CRATES: &[BoundaryCrate] = &[
    BoundaryCrate {
        package: "rss-native-abi",
        rust_name: "rss_native_abi",
        directory: "native-abi",
    },
    BoundaryCrate {
        package: "rss-process-guard",
        rust_name: "rss_process_guard",
        directory: "process-guard",
    },
    BoundaryCrate {
        package: "rss-metal-compute",
        rust_name: "rss_metal_compute",
        directory: "metal-compute",
    },
    BoundaryCrate {
        package: "vm-jit",
        rust_name: "vm_jit",
        directory: "vm-jit",
    },
];

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should exist")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

fn rust_files_below(root: &Path) -> Vec<PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        let mut entries = fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("read directory {}: {error}", directory.display()))
            .map(|entry| entry.expect("directory entry should be readable").path())
            .collect::<Vec<_>>();
        entries.sort();

        for path in entries {
            if path.is_dir() {
                visit(&path, files);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }

    let mut files = Vec::new();
    visit(root, &mut files);
    files
}

fn dependency_packages(manifest: &toml::Value) -> BTreeSet<String> {
    fn collect(value: &toml::Value, dependencies: &mut BTreeSet<String>) {
        let Some(table) = value.as_table() else {
            return;
        };

        for (key, value) in table {
            if matches!(
                key.as_str(),
                "dependencies" | "dev-dependencies" | "build-dependencies"
            ) {
                let Some(dependency_table) = value.as_table() else {
                    continue;
                };
                for (name, specification) in dependency_table {
                    let package = specification
                        .as_table()
                        .and_then(|table| table.get("package"))
                        .and_then(toml::Value::as_str)
                        .unwrap_or(name);
                    dependencies.insert(package.to_string());
                }
            } else {
                collect(value, dependencies);
            }
        }
    }

    let mut dependencies = BTreeSet::new();
    collect(manifest, &mut dependencies);
    dependencies
}

fn workspace_members(root: &Path) -> Vec<PathBuf> {
    let manifest: toml::Value =
        toml::from_str(&read(&root.join("Cargo.toml"))).expect("workspace Cargo.toml should parse");
    manifest["workspace"]["members"]
        .as_array()
        .expect("workspace.members should be an array")
        .iter()
        .map(|member| {
            root.join(
                member
                    .as_str()
                    .expect("workspace member should be a string"),
            )
        })
        .collect()
}

fn package_name(manifest: &toml::Value) -> &str {
    manifest["package"]["name"]
        .as_str()
        .expect("workspace member should declare package.name")
}

fn function_source<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("missing function signature `{signature}`"));
    let body_start = source[start..]
        .find('{')
        .map(|offset| start + offset)
        .unwrap_or_else(|| panic!("missing function body for `{signature}`"));
    let mut depth = 0usize;
    for (offset, character) in source[body_start..].char_indices() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[start..=body_start + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated function body for `{signature}`");
}

#[test]
fn syntax_sources_do_not_reference_later_layers() {
    let root = workspace_root();
    let mut files = rust_files_below(&root.join("crates/rsscript/src/syntax"));
    files.push(root.join("crates/rsscript/src/lexer.rs"));

    let forbidden = [
        ("package", "crate::package"),
        ("package", "package::"),
        ("runtime", "rsscript_runtime"),
        ("runtime", "crate::runtime"),
        ("REIR", "reir::"),
        ("VM JIT", "vm_jit"),
    ];
    let mut violations = Vec::new();

    for path in files {
        for (line_index, line) in read(&path).lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            for (layer, needle) in forbidden {
                if code.contains(needle) {
                    violations.push(format!(
                        "{}:{} references {layer} via `{needle}`",
                        path.strip_prefix(&root).unwrap_or(&path).display(),
                        line_index + 1
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "syntax may only depend on syntax/front-end primitives:\n{}",
        violations.join("\n")
    );
}

#[test]
fn runtime_does_not_depend_on_the_compiler_package() {
    let root = workspace_root();
    let manifest_path = root.join("crates/runtime/Cargo.toml");
    let manifest: toml::Value =
        toml::from_str(&read(&manifest_path)).expect("runtime Cargo.toml should parse");
    let dependencies = dependency_packages(&manifest);

    assert!(
        !dependencies.contains("rsscript"),
        "{} must not depend on the rsscript compiler/package",
        manifest_path.strip_prefix(&root).unwrap().display()
    );

    for path in rust_files_below(&root.join("crates/runtime/src")) {
        let source = read(&path);
        assert!(
            !source.contains("rsscript::"),
            "{} must not import the rsscript compiler/package",
            path.strip_prefix(&root).unwrap().display()
        );
    }
}

#[test]
fn unsafe_boundary_crates_are_explicit_dependencies() {
    let root = workspace_root();
    let mut violations = Vec::new();

    for member in workspace_members(&root) {
        let manifest_path = member.join("Cargo.toml");
        let manifest: toml::Value = toml::from_str(&read(&manifest_path))
            .unwrap_or_else(|error| panic!("parse {}: {error}", manifest_path.display()));
        let member_name = package_name(&manifest);
        let dependencies = dependency_packages(&manifest);
        let source_root = member.join("src");
        if !source_root.is_dir() {
            continue;
        }

        for path in rust_files_below(&source_root) {
            let source = read(&path);
            let relative = path.strip_prefix(&root).unwrap_or(&path).display();

            for boundary in UNSAFE_BOUNDARY_CRATES {
                if member_name != boundary.package
                    && source.contains(boundary.rust_name)
                    && !dependencies.contains(boundary.package)
                {
                    violations.push(format!(
                        "{relative} references `{}` without a `{}` Cargo dependency",
                        boundary.rust_name, boundary.package
                    ));
                }

                let imports_source = source.contains("#[path") || source.contains("include!");
                if member_name != boundary.package
                    && imports_source
                    && source.contains(boundary.directory)
                {
                    violations.push(format!(
                        "{relative} imports `{}` source; use an explicit Cargo dependency",
                        boundary.package
                    ));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "unsafe implementation boundaries must remain crate boundaries:\n{}",
        violations.join("\n")
    );
}

#[test]
fn executable_backends_consume_validated_frontend_results() {
    let root = workspace_root();
    let reg_vm = read(&root.join("crates/rsscript/src/reg_vm/mod.rs"));
    let compile_source = function_source(&reg_vm, "pub fn reg_vm_compile_source");
    assert!(
        compile_source.contains("validate_source(file, source)")
            && compile_source.contains("reg_vm_compile_validated(&validated)"),
        "register VM source compilation must consume a ValidatedProgram"
    );
    let compile_validated = function_source(&reg_vm, "pub fn reg_vm_compile_validated");
    assert!(
        compile_validated.contains("validated.database().hir()"),
        "register VM lowering must consume the checked HIR"
    );

    let rust_lower = read(&root.join("crates/rsscript/src/rust_lower/mod.rs"));
    let lower_source = function_source(&rust_lower, "pub fn lower_source_to_rust_with_map");
    assert!(
        lower_source.contains("validate_source(file, source)")
            && lower_source.contains("validated.database()"),
        "Rust source lowering must consume a ValidatedProgram"
    );

    let helpers = read(&root.join("crates/rsscript/src/rust_lower/helpers.rs"));
    assert!(
        !helpers.contains("parse_source"),
        "lowering declaration projections must reuse parsed semantic inputs"
    );
}

#[test]
fn restricted_vm_authority_is_mandatory_and_precedes_intrinsic_dispatch() {
    let root = workspace_root();
    let vm_model = read(&root.join("crates/rsscript/src/reg_vm/model.rs"));
    assert!(
        vm_model.contains("fn host_authority")
            && vm_model.contains("HostAuthority::Filesystem")
            && vm_model.contains("HostAuthority::Network")
            && vm_model.contains("HostAuthority::Process")
            && vm_model.contains("HostAuthority::Database"),
        "host-touching intrinsics must carry an explicit authority classification"
    );

    let vm = read(&root.join("crates/rsscript/src/reg_vm/mod.rs"));
    assert!(
        vm.contains("execution_context: crate::ExecutionContext"),
        "every RegVm instance must own an execution context"
    );
    let intrinsics = read(&root.join("crates/rsscript/src/reg_vm/intrinsics/mod.rs"));
    let dispatch = function_source(&intrinsics, "pub(super) fn call_intrinsic");
    assert!(
        dispatch.contains("intrinsic.host_authority()")
            && dispatch.contains("self.authorize_host_authority(authority)?"),
        "authority must be checked before intrinsic dispatch"
    );
}

#[test]
fn high_risk_state_machines_keep_dedicated_module_owners() {
    let root = workspace_root();
    let required = [
        "crates/rsscript/src/analyzer/task_group.rs",
        "crates/rsscript/src/native_plugin/loader/cache.rs",
        "crates/rsscript/src/native_plugin/loader/shim.rs",
        "crates/rsscript/src/package/native/bindings.rs",
        "crates/rsscript/src/reg_vm/tier/admission.rs",
        "crates/rsscript/src/reg_vm/tier/call_scratch.rs",
        "crates/rsscript/src/reg_vm/tier/recursion.rs",
        "crates/rsscript/src/rust_lower/helpers/executable_declarations.rs",
        "crates/rsscript/src/rust_lower/helpers/semantic_projection.rs",
        "crates/runtime/src/json.rs",
        "crates/runtime/src/network/mod.rs",
        "crates/runtime/src/process/supervisor.rs",
        "crates/vm-jit/src/analysis.rs",
        "crates/vm-jit/src/executable_memory.rs",
        "crates/reir/src/reconciliation/engine.rs",
        "crates/reir/src/cli/safe_io.rs",
    ];
    let missing = required
        .iter()
        .filter(|relative| !root.join(relative).is_file())
        .copied()
        .collect::<Vec<_>>();
    assert!(
        missing.is_empty(),
        "refactoring module owners must remain explicit: {}",
        missing.join(", ")
    );
}
