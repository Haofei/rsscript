use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "public_api_architecture.rs"]
mod public_api_architecture;

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

fn cargo_metadata(root: &Path) -> serde_json::Value {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .expect("cargo metadata should start");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("cargo metadata output should be JSON")
}

fn metadata_direct_dependencies(metadata: &serde_json::Value, package: &str) -> BTreeSet<String> {
    metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .find(|candidate| candidate["name"].as_str() == Some(package))
        .unwrap_or_else(|| panic!("metadata package `{package}`"))["dependencies"]
        .as_array()
        .expect("metadata dependencies")
        .iter()
        .filter_map(|dependency| dependency["name"].as_str().map(str::to_string))
        .collect()
}

fn metadata_normal_dependencies(metadata: &serde_json::Value, package: &str) -> BTreeSet<String> {
    metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .find(|candidate| candidate["name"].as_str() == Some(package))
        .unwrap_or_else(|| panic!("metadata package `{package}`"))["dependencies"]
        .as_array()
        .expect("metadata dependencies")
        .iter()
        .filter(|dependency| {
            dependency["kind"].is_null() && dependency["optional"].as_bool() != Some(true)
        })
        .filter_map(|dependency| dependency["name"].as_str().map(str::to_string))
        .collect()
}

fn metadata_workspace_dependencies(
    metadata: &serde_json::Value,
    package: &str,
) -> BTreeSet<String> {
    let workspace_packages = metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .filter_map(|candidate| candidate["name"].as_str().map(str::to_string))
        .collect::<BTreeSet<_>>();
    metadata_direct_dependencies(metadata, package)
        .intersection(&workspace_packages)
        .cloned()
        .collect()
}

fn metadata_default_members(metadata: &serde_json::Value) -> BTreeSet<String> {
    let packages = metadata["packages"]
        .as_array()
        .expect("metadata packages")
        .iter()
        .map(|package| {
            (
                package["id"].as_str().expect("package id"),
                package["name"].as_str().expect("package name"),
            )
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    metadata["workspace_default_members"]
        .as_array()
        .expect("workspace default members")
        .iter()
        .map(|member| {
            let id = member.as_str().expect("default member id");
            packages
                .get(id)
                .unwrap_or_else(|| panic!("default member `{id}` is not a workspace package"))
                .to_string()
        })
        .collect()
}

fn collect_rust_sources(path: &Path, sources: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(path)
        .unwrap_or_else(|error| panic!("read source directory {}: {error}", path.display()))
    {
        let entry = entry.expect("source directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, sources);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            sources.push(path);
        }
    }
}

#[test]
fn cargo_metadata_enforces_composition_dependency_direction() {
    let root = workspace_root();
    let metadata = cargo_metadata(&root);

    let compiler = metadata_normal_dependencies(&metadata, "rsscript-compiler");
    assert!(compiler.contains("rsscript-syntax"));
    assert!(compiler.contains("rsscript-semantics"));
    assert!(!compiler.contains("rsscript-vm"));
    assert!(!compiler.contains("rsscript-sdk"));
    let sdk = metadata_normal_dependencies(&metadata, "rsscript-sdk");
    assert!(
        sdk.contains("rsscript-compiler"),
        "embedding SDK must consume the compiler implementation"
    );
    assert!(metadata_direct_dependencies(&metadata, "rsscript-sdk").contains("rsscript-vm"));
    for forbidden in [
        "rsscript-cli",
        "rsscript-aot-runtime",
        "rsscript-review-reir",
        "reir",
        "rss-native-abi",
        "rss-process-guard",
        "vm-jit",
        "rsscript-provider-fs",
        "rsscript-provider-env",
        "rsscript-provider-process",
        "rsscript-provider-http",
    ] {
        assert!(
            !compiler.contains(forbidden),
            "compiler implementation must not depend on composition package `{forbidden}`"
        );
    }

    let language_service = metadata_workspace_dependencies(&metadata, "rsscript-language-service");
    assert_eq!(
        language_service,
        BTreeSet::from([
            "rsscript-compiler".to_string(),
            "rsscript-operation".to_string(),
        ]),
        "language service must depend only on frontend and shared operation contracts"
    );

    for package in metadata["packages"].as_array().expect("metadata packages") {
        let name = package["name"].as_str().expect("package name");
        if name != "rsscript-cli" {
            assert!(
                !metadata_direct_dependencies(&metadata, name).contains("rsscript-cli"),
                "composition root must remain a dependency leaf; `{name}` depends on it"
            );
        }
    }
}

#[test]
fn workspace_tiers_are_exhaustive_and_define_default_members() {
    let root = workspace_root();
    let metadata = cargo_metadata(&root);
    let inventory = read(&root.join("docs/architecture/workspace-tiers.toml"));
    let inventory: toml::Value = toml::from_str(&inventory).expect("workspace tier inventory");
    assert_eq!(inventory["schema"].as_integer(), Some(1));

    let tier_names = [
        "core",
        "applications",
        "runner",
        "providers",
        "integrations",
        "experimental",
        "research",
        "tooling",
        "examples",
    ];
    let mut classified = BTreeSet::new();
    let mut defaults = BTreeSet::new();
    for tier in tier_names {
        for package in inventory[tier]
            .as_array()
            .unwrap_or_else(|| panic!("tier `{tier}` must be an array"))
        {
            let package = package
                .as_str()
                .unwrap_or_else(|| panic!("tier `{tier}` contains a non-string package"));
            assert!(
                classified.insert(package.to_string()),
                "workspace package `{package}` occurs in more than one maturity tier"
            );
            if matches!(tier, "core" | "applications" | "runner") {
                defaults.insert(package.to_string());
            }
        }
    }

    let workspace = metadata["workspace_members"]
        .as_array()
        .expect("workspace members")
        .iter()
        .map(|member| {
            let member = member.as_str().expect("workspace member id");
            metadata["packages"]
                .as_array()
                .expect("metadata packages")
                .iter()
                .find(|package| package["id"].as_str() == Some(member))
                .and_then(|package| package["name"].as_str())
                .unwrap_or_else(|| panic!("workspace member `{member}` has no package metadata"))
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        classified, workspace,
        "every workspace package must occur in exactly one maturity tier"
    );
    assert_eq!(
        metadata_default_members(&metadata),
        defaults,
        "root default-members must contain only Core, applications, and the runner"
    );
}

#[test]
fn root_workspace_excludes_experimental_packages() {
    let root = workspace_root();
    let metadata = cargo_metadata(&root);
    let packages = metadata["packages"].as_array().expect("metadata packages");
    let root_members = metadata["workspace_members"]
        .as_array()
        .expect("workspace members")
        .iter()
        .map(|member| member.as_str().expect("workspace member id"))
        .collect::<BTreeSet<_>>();

    for experimental in [
        "rsscript-aot-runtime",
        "rss-native-abi",
        "reir",
        "rss-testgen",
        "rsscript-review-reir",
        "vm-jit",
    ] {
        assert!(
            !root_members.iter().any(|member| {
                packages
                    .iter()
                    .find(|package| package["id"].as_str() == Some(*member))
                    .and_then(|package| package["name"].as_str())
                    == Some(experimental)
            }),
            "Core workspace must not own experimental package `{experimental}`"
        );
    }
}

#[test]
fn migration_boundary_rejects_disabled_cemetery_code_and_root_glob_exports() {
    let root = workspace_root();
    let mut sources = Vec::new();
    collect_rust_sources(&root.join("crates"), &mut sources);
    let disabled = sources
        .iter()
        .filter_map(|path| {
            let source = read(path);
            source.contains(&["#[cfg(", "any())]"].concat()).then(|| {
                path.strip_prefix(&root)
                    .unwrap_or(path)
                    .display()
                    .to_string()
            })
        })
        .collect::<Vec<_>>();
    assert!(
        disabled.is_empty(),
        "delete disabled cemetery code instead of hiding it behind cfg(any()): {}",
        disabled.join(", ")
    );

    let sdk = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    for implementation in ["rsscript_compiler", "rsscript_bytecode", "rsscript_vm"] {
        assert!(
            !sdk.contains(&format!("pub use {implementation}::*")),
            "the stable SDK root must explicitly inventory exports from `{implementation}`"
        );
    }
}

#[test]
fn rss_check_default_cargo_closure_is_frontend_only() {
    let root = workspace_root();
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            "rsscript-cli",
            "--edges",
            "normal",
            "--prefix",
            "none",
        ])
        .current_dir(&root)
        .output()
        .expect("cargo tree should start");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let closure = String::from_utf8(output.stdout).expect("cargo tree should be UTF-8");
    for forbidden in [
        "rsscript-runtime ",
        "rsscript-aot-runtime ",
        "rsscript-bytecode ",
        "rsscript-provider-api ",
        "rss-process-guard ",
        "vm-jit ",
        "reqwest ",
        "tokio ",
        "tungstenite ",
    ] {
        assert!(
            !closure.contains(forbidden),
            "frontend-only `rss check` dependency closure contains `{forbidden}`:\n{closure}"
        );
    }
}

#[test]
fn intrinsic_catalog_is_the_only_generated_registry_source() {
    let root = workspace_root();
    let build = read(&root.join("crates/rsscript-build-support/src/lib.rs"));
    assert!(
        build.contains("intrinsics.toml"),
        "the intrinsic generator must consume the structured catalog"
    );
    assert!(
        !build.contains("src/reg_vm/lower.rs") && !build.contains("src/runtime_abi.rs"),
        "the intrinsic generator must not scrape Rust implementation source"
    );

    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    let catalog: toml::Value = toml::from_str(&catalog).expect("intrinsic catalog should parse");
    assert_eq!(catalog["schema"].as_integer(), Some(1));
    let intrinsics = catalog["intrinsic"]
        .as_array()
        .expect("intrinsic catalog must contain an intrinsic array");
    let bindings = catalog["binding"]
        .as_array()
        .expect("intrinsic catalog must contain a binding array");
    assert!(
        !intrinsics.is_empty(),
        "the intrinsic catalog must not be empty"
    );
    assert!(
        !bindings.is_empty(),
        "the binding catalog must not be empty"
    );

    let mut intrinsic_ids = BTreeSet::new();
    let mut derived_intrinsic_ids = BTreeSet::new();
    for entry in intrinsics {
        let id = entry["id"]
            .as_str()
            .expect("every intrinsic entry must have a string id");
        assert!(intrinsic_ids.insert(id), "duplicate intrinsic id `{id}`");
        if entry.get("derived_from").is_some() {
            derived_intrinsic_ids.insert(id);
        }
    }

    let mut binding_names = BTreeSet::new();
    let mut referenced_intrinsic_ids = BTreeSet::new();
    for entry in bindings {
        let namespace = entry["namespace"]
            .as_str()
            .expect("every binding entry must have a string namespace");
        let name = entry["name"]
            .as_str()
            .expect("every binding entry must have a string name");
        let qualified_name = format!("{namespace}.{name}");
        assert!(
            binding_names.insert(qualified_name.clone()),
            "duplicate binding name `{qualified_name}`"
        );
        if let Some(vm_id) = entry.get("vm_id") {
            let vm_id = vm_id
                .as_str()
                .expect("a binding vm_id must be a string when present");
            assert!(
                intrinsic_ids.contains(vm_id),
                "binding `{qualified_name}` references orphan intrinsic `{vm_id}`"
            );
            referenced_intrinsic_ids.insert(vm_id);
        }
    }
    referenced_intrinsic_ids.extend(derived_intrinsic_ids);
    assert_eq!(
        referenced_intrinsic_ids, intrinsic_ids,
        "every VM intrinsic implementation must have at least one catalog binding"
    );
}

#[test]
fn cli_defaults_to_verified_vm_and_requires_explicit_aot() {
    let root = workspace_root();
    let run = read(&root.join("crates/rsscript-cli/src/cli/run_cmd.rs"));
    let runner = read(&root.join("crates/rsscript-cli/src/cli/runner.rs"));
    assert!(run.contains("super::runner::run_isolated"));
    assert!(run.contains("options.trusted_in_process"));
    assert!(runner.contains("ArtifactVerifier.verify_bytes"));
    assert!(runner.contains("spawn_guarded_child_strict"));
    assert!(run.contains("arg == \"--aot\""));
    assert!(!run.contains("arg == \"--vm\""));

    let selfhost = read(&root.join("docs/self-hosting.md"));
    assert!(selfhost.contains("Self-hosting is frozen Research"));
    assert!(selfhost.contains("Do not expand the C emitter"));
}

#[test]
fn selfhost_known_type_sets_are_generated() {
    let root = workspace_root();
    let checker = read(&root.join("selfhost/check.rss"));
    assert!(
        !checker.contains("fn is_builtin_type(") && !checker.contains("fn is_stdlib_type("),
        "self-host type knowledge must come from generated interface metadata"
    );
    let metadata = read(&root.join("crates/rsscript-compiler/src/interface_metadata.rs"));
    assert!(metadata.contains("crate::analyzer::BUILTIN_TYPE_NAMES"));
    assert!(metadata.contains("for name in &metadata.types"));
}

#[test]
fn selfhost_checker_entry_is_orchestration_only() {
    let root = workspace_root();
    let checker = read(&root.join("selfhost/check.rss"));
    let declarations = checker
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("fn ") || line.starts_with("struct ") || line.starts_with("const ")
        })
        .collect::<Vec<_>>();

    assert_eq!(declarations, ["fn main(args: read List<String>) -> Unit {"]);
    assert!(checker.lines().count() < 1_000);
    for import in [
        "use selfhost.checker.support.*",
        "use selfhost.checker.output.*",
        "use selfhost.checker.type_model.*",
        "use selfhost.checker.diagnostics.syntax_declarations.*",
        "use selfhost.checker.diagnostics.effects_calls.*",
    ] {
        assert!(
            checker.contains(import),
            "checker entry must retain {import}"
        );
    }

    for (path, module) in [
        (
            "selfhost/checker/support.rss",
            "module selfhost.checker.support",
        ),
        (
            "selfhost/checker/output.rss",
            "module selfhost.checker.output",
        ),
        (
            "selfhost/checker/type_model.rss",
            "module selfhost.checker.type_model",
        ),
        (
            "selfhost/checker/diagnostics/syntax_declarations.rss",
            "module selfhost.checker.diagnostics.syntax_declarations",
        ),
        (
            "selfhost/checker/diagnostics/effects_calls.rss",
            "module selfhost.checker.diagnostics.effects_calls",
        ),
    ] {
        assert!(
            read(&root.join(path)).contains(module),
            "{path} must declare {module}"
        );
    }
}

#[test]
fn hir_inference_uses_structural_type_queries() {
    let root = workspace_root();
    let inference = read(&root.join("crates/rsscript-semantics/src/hir/infer.rs"));
    for parser in [
        "strip_prefix(\"Fn(\")",
        "strip_prefix(\"Result<\")",
        "strip_prefix(\"Option<\")",
        "strip_prefix(\"List<\")",
        "strip_prefix(\"Stream<\")",
        "strip_prefix(\"Task<\")",
        "strip_prefix(\"Dyn<\")",
    ] {
        assert!(
            !inference.contains(parser),
            "HIR inference must query ResolvedType instead of parsing {parser}"
        );
    }
}

#[test]
fn hir_signatures_store_structural_types() {
    let root = workspace_root();
    let hir = read(&root.join("crates/rsscript-semantics/src/hir/mod.rs"));
    assert!(hir.contains("pub ty: ResolvedType"));
    assert!(hir.contains("pub return_ty: Option<ResolvedType>"));
    assert!(!hir.contains("pub type_name: String"));
    assert!(!hir.contains("pub return_type: Option<String>"));

    let inference = read(&root.join("crates/rsscript-semantics/src/hir/infer.rs"));
    assert!(
        !inference.contains("field.type_name")
            && !inference.contains("param.type_name")
            && !inference.contains("signature.return_type"),
        "HIR inference must not reconstruct signature or field types from rendered strings"
    );
}

#[test]
fn native_tier_state_machines_have_explicit_module_boundaries() {
    let root = workspace_root();
    let tier = read(&root.join("crates/rsscript-vm/src/reg_vm/tier.rs"));
    assert!(tier.contains("mod deopt_resume;"));
    assert!(tier.contains("mod jit_entry;"));
    assert!(!tier.contains("fn restore_native_deopt_live_regs("));
    assert!(!tier.contains("fn run_jit_pure_leaf("));

    let deopt = read(&root.join("crates/rsscript-vm/src/reg_vm/tier/deopt_resume.rs"));
    assert!(deopt.contains("fn try_resume_native_child_deopt_chain("));
    assert!(deopt.contains("fn restore_native_deopt_live_regs("));

    let entry = read(&root.join("crates/rsscript-vm/src/reg_vm/tier/jit_entry.rs"));
    assert!(entry.contains("fn run_jit("));
    assert!(entry.contains("fn run_jit_self_recursive_int("));
}

#[test]
fn register_vm_execution_policy_is_snapshotted_before_running() {
    let root = workspace_root();
    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(vm.contains("mod execution_plan;"));
    assert!(vm.contains("NativeExecutionPlan::from_environment("));
    assert!(vm.contains("NativeState::new_with_plan(native)"));

    let plan = read(&root.join("crates/rsscript-vm/src/reg_vm/execution_plan.rs"));
    assert!(plan.contains("struct ExecutionPlan"));
    assert!(plan.contains("enum TierPlan"));
    assert!(plan.contains("struct NativeAdmissionPolicy"));
    assert!(plan.contains("max_code_bytes"));
    assert!(plan.contains("max_compile_millis"));
    assert!(plan.contains("optimize_work_threshold"));
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

fn normal_dependency_packages(manifest: &toml::Value) -> BTreeSet<String> {
    manifest["dependencies"]
        .as_table()
        .into_iter()
        .flatten()
        .map(|(name, specification)| {
            specification
                .as_table()
                .and_then(|table| table.get("package"))
                .and_then(toml::Value::as_str)
                .unwrap_or(name)
                .to_string()
        })
        .collect()
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
fn register_vm_has_no_disabled_test_composition_tree() {
    let root = workspace_root();
    assert!(
        !root.join("crates/rsscript-vm/src/reg_vm/tests.rs").exists(),
        "disabled VM test aggregators must be deleted instead of cfg-disabled"
    );
    assert!(
        !root.join("crates/rsscript-vm/src/reg_vm/tests").exists(),
        "disabled VM test trees must be deleted instead of retained as cemetery code"
    );
    for active in ["exec.rs", "execution_plan.rs", "tier.rs", "value_ops.rs"] {
        let source = read(&root.join("crates/rsscript-vm/src/reg_vm").join(active));
        assert!(
            source.contains("mod tests"),
            "active VM domain `{active}` must keep colocated executable tests"
        );
    }
}

#[test]
fn jit_acceptance_domains_remain_separate_modules() {
    let root = workspace_root();
    let aggregator = root.join("crates/rsscript-sdk/tests/jit_acceptance.rs");
    let source = read(&aggregator);
    let expected = ["core", "optimization", "limits"];

    assert!(
        source.lines().count() <= expected.len() + 12,
        "jit_acceptance.rs must remain a composition root"
    );
    for domain in expected {
        assert!(
            source.contains(&format!("jit_acceptance/{domain}.rs")),
            "JIT acceptance composition root is missing `{domain}`"
        );
        assert!(
            root.join(format!(
                "crates/rsscript-sdk/tests/jit_acceptance/{domain}.rs"
            ))
            .is_file(),
            "JIT acceptance domain `{domain}` must have its own module"
        );
    }
}

#[test]
fn selfhost_parity_domains_remain_separate_modules() {
    let root = workspace_root();
    let aggregator = root.join("crates/rsscript-compiler/src/selfhost_parity.rs");
    let source = read(&aggregator);
    let expected = [
        "lexer",
        "parser",
        "checker",
        "package_contract",
        "ast_oracle",
        "ast_parity",
    ];

    assert!(
        source.lines().count() <= expected.len() + 10,
        "selfhost_parity.rs must remain a composition root"
    );
    for domain in expected {
        assert!(
            source.contains(&format!("selfhost_parity/{domain}.rs")),
            "self-host parity composition root is missing `{domain}`"
        );
        assert!(
            root.join(format!(
                "crates/rsscript-compiler/src/selfhost_parity/{domain}.rs"
            ))
            .is_file(),
            "self-host parity domain `{domain}` must have its own module"
        );
    }
}

#[test]
fn syntax_sources_do_not_reference_later_layers() {
    let root = workspace_root();
    let files = rust_files_below(&root.join("crates/rsscript-syntax/src"));

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
fn syntax_model_is_owned_by_the_boundary_crate() {
    let root = workspace_root();
    assert!(root.join("crates/rsscript-syntax/src/ast.rs").is_file());
    assert!(root.join("crates/rsscript-syntax/src/lexer.rs").is_file());
    assert!(
        root.join("crates/rsscript-syntax/src/parser/mod.rs")
            .is_file()
    );
    assert!(
        !root
            .join("crates/rsscript-compiler/src/syntax/ast.rs")
            .exists()
    );
    assert!(!root.join("crates/rsscript-compiler/src/lexer.rs").exists());
    assert!(
        !root
            .join("crates/rsscript-compiler/src/syntax/parser")
            .exists()
    );

    let manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-syntax/Cargo.toml")))
            .expect("syntax manifest should parse");
    let dependencies = dependency_packages(&manifest);
    for forbidden in [
        "rsscript",
        "rsscript-semantics",
        "rsscript-runtime",
        "rsscript-aot-runtime",
        "rsscript-provider-api",
        "reir",
        "vm-jit",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "syntax model must not depend on `{forbidden}`"
        );
    }
}

#[test]
fn source_coordinates_are_not_owned_by_budget_accounting() {
    let root = workspace_root();
    let source_model = read(&root.join("crates/rsscript-source-model/src/lib.rs"));
    for source_type in [
        "pub struct FileId",
        "pub struct SourceRevision",
        "pub struct TextRange",
        "pub struct Span",
    ] {
        assert!(
            source_model.contains(source_type),
            "source model must own `{source_type}`"
        );
    }

    let budget = read(&root.join("crates/rsscript-work-budget/src/lib.rs"));
    assert!(!budget.contains("pub struct Span"));
    assert!(budget.contains("pub use rsscript_source_model::Span"));

    let syntax = read(&root.join("crates/rsscript-syntax/src/lib.rs"));
    assert!(syntax.contains("pub use rsscript_source_model"));
}

#[test]
fn cancellation_and_deadlines_share_one_operation_contract() {
    let root = workspace_root();
    let operation = read(&root.join("crates/rsscript-operation/src/lib.rs"));
    assert!(operation.contains("pub struct CancellationToken"));
    assert!(operation.contains("pub struct MonotonicDeadline"));

    let provider = read(&root.join("crates/rsscript-provider-api/src/lib.rs"));
    assert!(provider.contains("Option<&'a CancellationToken>"));
    assert!(provider.contains("Option<MonotonicDeadline>"));

    let language_service = read(&root.join("crates/rsscript-language-service/src/lib.rs"));
    assert!(language_service.contains("Option<&'a CancellationToken>"));
    assert!(language_service.contains("Option<MonotonicDeadline>"));

    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(vm.contains("Option<rsscript_operation::CancellationToken>"));
    assert!(vm.contains("Option<rsscript_operation::MonotonicDeadline>"));
}

#[test]
fn provider_and_workspace_boundaries_use_structured_errors() {
    let root = workspace_root();
    let provider = read(&root.join("crates/rsscript-provider-api/src/lib.rs"));
    assert!(!provider.contains("impl From<String> for ProviderError"));
    assert!(!provider.contains("impl From<&str> for ProviderError"));
    assert!(provider.contains("pub enum ProviderErrorCode"));
    assert!(provider.contains("pub fn from_io"));

    for provider_name in [
        "fs", "env", "process", "http", "time", "entropy", "log", "cli",
    ] {
        let implementation = read(
            &root
                .join("providers")
                .join(provider_name)
                .join("src/lib.rs"),
        );
        assert!(
            !implementation.contains("return Err(\"")
                && !implementation.contains("Result<(), String>"),
            "provider `{provider_name}` must return ProviderError with a stable code"
        );
    }

    let workspace_loader = read(&root.join("crates/rsscript-workspace-loader/src/lib.rs"));
    assert!(workspace_loader.contains("pub enum WorkspaceLoadErrorCode"));
    assert!(workspace_loader.contains("pub struct WorkspaceLoadError"));
    assert!(!workspace_loader.contains("Result<Vec<WorkspaceSourceFile>, String>"));
}

#[test]
fn language_engine_does_not_read_the_operating_system() {
    let root = workspace_root();
    let language_service = read(&root.join("crates/rsscript-language-service/src/lib.rs"));
    for forbidden in [
        "std::fs",
        "std::path",
        "current_dir",
        "read_dir",
        "read_to_string",
        "WorkspaceLoader",
    ] {
        assert!(
            !language_service.contains(forbidden),
            "language engine must not contain OS loader operation `{forbidden}`"
        );
    }
    let language_manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-language-service/Cargo.toml"),
    ))
    .unwrap();
    assert!(language_manifest["dependencies"].get("toml").is_none());

    let loader_manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-workspace-loader/Cargo.toml"),
    ))
    .unwrap();
    let loader_dependencies = dependency_packages(&loader_manifest);
    assert_eq!(loader_dependencies, BTreeSet::from(["toml".to_string()]));

    let description = language_manifest["package"]["description"]
        .as_str()
        .expect("language-service description");
    assert!(description.to_ascii_lowercase().contains("incremental"));
    for boundary in [
        "dependency_cache",
        "lint_cache",
        "format_cache",
        "symbol_cache",
        "invalidate_interface_dependents",
    ] {
        assert!(
            language_service.contains(boundary),
            "language service must retain query boundary `{boundary}`"
        );
    }
}

#[test]
fn linked_provider_contracts_reach_the_invocation_path() {
    let root = workspace_root();
    let provider_api = read(&root.join("crates/rsscript-provider-api/src/lib.rs"));
    for contract in [
        "ProviderInvocationContract",
        "ResolvedProviderFunction",
        "blocking_allowed",
        "async_allowed",
        "into_resolved_functions",
    ] {
        assert!(
            provider_api.contains(contract),
            "provider API must retain `{contract}`"
        );
    }
    assert!(
        !provider_api.contains("pub fn into_functions"),
        "provider registry must not discard resolved descriptor metadata"
    );
    assert!(
        !provider_api.contains("pub fn functions(&self)"),
        "provider registry must not expose descriptor-free registered entries"
    );
    let execution = read(&root.join("crates/rsscript-vm/src/eval_types.rs"));
    assert!(execution.contains("ExternalFunction::from_resolved"));
    assert!(execution.contains("requires a blocking execution lane"));
    assert!(execution.contains("requires an async execution lane"));
    assert!(execution.contains("without registering a resource"));

    let vm_calls = read(&root.join("crates/rsscript-vm/src/reg_vm/calls.rs"));
    assert!(vm_calls.contains("let blocking_allowed = self.limits.allow_blocking_provider_calls"));
    assert!(!vm_calls.contains("blocking_allowed: true"));
    assert!(vm_calls.contains("async_allowed: false"));
    assert!(vm_calls.contains("AsyncProviderCallContext"));
    assert!(vm_calls.contains("function.start_async"));

    let scheduler = read(&root.join("crates/rsscript-vm/src/reg_vm/scheduler.rs"));
    assert!(scheduler.contains("poll_provider_futures"));
    assert!(scheduler.contains("Wait::Provider"));
}

#[test]
fn execution_termination_does_not_classify_message_text() {
    let root = workspace_root();
    let facade = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    assert!(!facade.contains("fn classify_runtime_error"));
    assert!(!facade.contains("reason: classify_runtime_error"));
    assert!(facade.contains("EvalError::Execution { kind, message }"));
    assert!(facade.contains("EvalError::Provider(error)"));

    let eval = read(&root.join("crates/rsscript-vm/src/eval_types.rs"));
    assert!(eval.contains("pub enum ExecutionFailureKind"));
    assert!(eval.contains("Provider(ProviderError)"));
}

#[test]
fn allocation_budget_is_not_mislabeled_as_live_memory() {
    let root = workspace_root();
    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(vm.contains("pub allocation_budget: Option<usize>"));
    assert!(vm.contains("allocated_bytes: usize"));
    assert!(vm.contains("This is not a live-memory measurement"));
    assert!(!vm.contains("pub mem_budget"));
    assert!(vm.contains("pub live_memory_limit: Option<usize>"));
    assert!(vm.contains("live_memory_bytes: usize"));
    let storage = read(&root.join("crates/rsscript-vm/src/reg_vm/exec/storage_accounting.rs"));
    assert!(storage.contains("refresh_live_memory_usage"));
    assert!(storage.contains("visited: &mut HashSet<usize>"));

    let facade = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    assert!(facade.contains("allocation_budget: Option<usize>"));
    assert!(facade.contains("live_memory_limit: Option<usize>"));
    assert!(facade.contains("pub fn with_allocation_budget"));
    assert!(facade.contains("pub fn with_live_memory_limit"));
    assert!(!facade.contains("pub allocation_budget: Option<usize>"));
    assert!(!facade.contains("pub live_memory_limit: Option<usize>"));
    assert!(!facade.contains("pub memory_budget"));
}

#[test]
fn public_execution_defaults_are_bounded_without_compatibility_aliases() {
    let root = workspace_root();
    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    let sdk = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    assert!(!vm.contains("pub fn safe_default"));
    for bounded in [
        "step_budget: Some(",
        "allocation_budget: Some(",
        "live_memory_limit: Some(",
        "stdout_budget: Some(",
        "intrinsic_call_budget: Some(",
        "provider_call_budget: Some(",
        "resource_limit: Some(",
        "allow_blocking_provider_calls: false",
    ] {
        assert!(
            vm.contains(bounded),
            "VmLimits::default must retain finite `{bounded}`"
        );
    }
    assert!(sdk.contains("VmLimits::default().into()"));
    assert!(sdk.contains("Self::new(ProviderRegistry::default())"));
    assert!(sdk.contains("limits: RunLimits::bounded()"));
    assert!(vm.contains("pub fn unbounded_for_trusted_host"));
}

#[test]
fn structural_semantics_are_owned_by_the_semantics_crate() {
    let root = workspace_root();
    let types = root.join("crates/rsscript-semantics/src/types.rs");
    assert!(types.is_file());
    assert!(
        root.join("crates/rsscript-semantics/src/database.rs")
            .is_file(),
        "semantic phase types and immutable facts must be owned by semantics"
    );
    assert!(
        !root
            .join("crates/rsscript-compiler/src/semantic_types.rs")
            .exists()
    );
    assert!(
        root.join("crates/rsscript-semantics/src/hir/mod.rs")
            .is_file()
    );
    assert!(
        rust_files_below(&root.join("crates/rsscript-compiler/src/hir"))
            .iter()
            .all(|path| path.ends_with("tests.rs")),
        "the compiler façade must not retain HIR implementation files"
    );

    let semantics = read(&root.join("crates/rsscript-semantics/src/lib.rs"));
    for exported in [
        "AnalysisResult",
        "FrontendCompletion",
        "FrontendStopReason",
        "ResolvedParamEffect",
        "ResolvedType",
        "ResolvedTypeKind",
        "SemanticDatabase",
        "SemanticTypeFacts",
        "SourceSnapshot",
        "TypeArena",
        "TypeId",
        "TypeQualifiers",
        "ValidatedProgram",
    ] {
        assert!(
            semantics.contains(exported),
            "semantics must export structural model `{exported}`"
        );
    }

    let database = read(&root.join("crates/rsscript-semantics/src/database.rs"));
    for owned in [
        "pub struct SourceSnapshot",
        "pub struct SemanticDatabase",
        "pub struct AnalysisResult",
        "pub struct ValidatedProgram",
        "pub fn into_validated",
    ] {
        assert!(
            database.contains(owned),
            "semantics must own phase contract `{owned}`"
        );
    }
    let compiler_projection = read(&root.join("crates/rsscript-compiler/src/semantic.rs"));
    for forbidden in [
        "pub struct SourceSnapshot",
        "pub struct SemanticDatabase",
        "pub struct AnalysisResult",
        "pub struct ValidatedProgram",
        "pub enum FrontendCompletion",
    ] {
        assert!(
            !compiler_projection.contains(forbidden),
            "compiler semantic compatibility module must not re-own `{forbidden}`"
        );
    }
    assert!(compiler_projection.contains("pub use rsscript_semantics"));

    assert!(
        !root
            .join("crates/rsscript-compiler/src/call_binding.rs")
            .exists(),
        "compiler must not retain a call-binding compatibility module"
    );
    for path in rust_files_below(&root.join("crates/rsscript-compiler/src")) {
        assert!(
            !read(&path).contains("crate::call_binding::"),
            "{} must consume canonical call binding directly from semantics",
            path.display()
        );
    }

    let mut constructor_users = Vec::new();
    for path in rust_files_below(&root.join("crates")) {
        let source = read(&path);
        if source.contains(&["SemanticDatabase::", "from_frontend_parts"].concat()) {
            constructor_users.push(
                path.strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }
    assert_eq!(
        constructor_users,
        ["crates/rsscript-compiler/src/analyzer.rs"],
        "only the migrating semantic analyzer may assemble checked database parts"
    );

    let manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-semantics/Cargo.toml")))
            .expect("semantics manifest should parse");
    let dependencies = dependency_packages(&manifest);
    for forbidden in [
        "rsscript",
        "rsscript-runtime",
        "rsscript-aot-runtime",
        "rsscript-provider-api",
        "rss-native-abi",
        "rss-process-guard",
        "reir",
        "vm-jit",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "semantics must not depend on `{forbidden}`"
        );
    }
}

#[test]
fn reir_is_a_one_way_optional_integration() {
    let root = workspace_root();
    let compiler_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    let compiler_dependencies = normal_dependency_packages(&compiler_manifest);
    assert!(
        !compiler_dependencies.contains("reir")
            && !compiler_dependencies.contains("rsscript-review-reir"),
        "normal compiler builds must not depend on review integrations"
    );

    let integration_manifest: toml::Value = toml::from_str(&read(
        &root.join("experiments/rsscript-review-reir/Cargo.toml"),
    ))
    .expect("REIR integration manifest should parse");
    let integration_dependencies = normal_dependency_packages(&integration_manifest);
    assert_eq!(
        integration_dependencies,
        BTreeSet::from(["reir".to_string(), "serde_json".to_string(),])
    );
    let integration_library = read(&root.join("experiments/rsscript-review-reir/src/lib.rs"));
    assert!(integration_library.contains("package_analysis"));
    assert!(!integration_library.contains("PackageReview"));
    assert!(!integration_library.contains("format_package_review"));

    let compiler_library = read(&root.join("crates/rsscript-compiler/src/lib.rs"));
    assert!(
        !compiler_library.contains("reir"),
        "the compiler façade must not expose REIR formatting APIs"
    );
    let package_cli = read(&root.join("crates/rsscript-cli/src/cli/mod.rs"));
    assert!(
        !package_cli.contains("mod package;")
            && !package_cli.contains("\"pkg\"")
            && !package_cli.contains("rss pkg"),
        "repository/review package commands must stay out of the product CLI"
    );
}

#[test]
fn compiler_does_not_embed_a_native_plugin_loader() {
    let root = workspace_root();
    let manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    assert!(
        manifest["features"]["default"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "frontend CLI commands must not select execution dependencies by default"
    );
    assert!(
        manifest["dependencies"].get("rss-native-abi").is_none(),
        "compiler must not depend on the native plugin ABI"
    );
    assert!(manifest["features"].get("native-plugin").is_none());
    let library = read(&root.join("crates/rsscript-compiler/src/lib.rs"));
    assert!(!library.contains("native_plugin"));
    assert!(
        !root
            .join("crates/rsscript-compiler/src/native_plugin/mod.rs")
            .exists()
    );
}

#[test]
fn lsp_dependency_closure_selects_frontend_only() {
    let root = workspace_root();
    let language_service: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-language-service/Cargo.toml"),
    ))
    .unwrap();
    assert!(language_service["dependencies"].get("rsscript").is_none());
    assert!(
        language_service["dependencies"]
            .get("rsscript_compiler")
            .is_some(),
        "language service must consume the frontend-only compiler API"
    );

    let compiler_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml"))).unwrap();
    for forbidden in ["rsscript-runtime", "rsscript-aot-runtime"] {
        assert!(
            compiler_manifest["dependencies"].get(forbidden).is_none(),
            "language-service compiler dependency must not select `{forbidden}`"
        );
    }
    for dependency in ["rsscript-lowering", "rsscript-provider-api"] {
        assert_eq!(
            compiler_manifest["dependencies"][dependency]["optional"].as_bool(),
            Some(true),
            "LSP-excluded dependency `{dependency}` must remain optional"
        );
    }
    for forbidden in ["rsscript-bytecode", "rsscript-vm"] {
        assert!(
            compiler_manifest["dependencies"].get(forbidden).is_none(),
            "compiler must not depend on execution crate `{forbidden}`"
        );
    }
}

#[test]
fn embedding_facade_exposes_only_product_level_objects() {
    let root = workspace_root();
    let mut source = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    source.push_str(&read(
        &root.join("crates/rsscript-sdk/src/artifact_bundle.rs"),
    ));
    for object in [
        "pub struct Compiler",
        "pub struct BuiltArtifact",
        "pub struct VerifiedArtifact",
        "pub struct LinkedArtifact",
        "pub struct ArtifactBundle",
        "pub struct ArtifactVerifier",
        "pub struct ExecutionRequest",
        "pub struct Runtime",
        "pub struct ProviderRegistry",
        "pub struct RunLimits",
        "pub struct ExecutionReport",
    ] {
        assert!(
            source.contains(object),
            "missing stable façade object `{object}`"
        );
    }
    for forbidden in [
        "JitPlan",
        "RegInstr",
        "RustSourceMapEntry",
        "ReviewFinding",
        "reir",
    ] {
        assert!(
            !source.contains(forbidden),
            "stable embedding façade must not expose `{forbidden}`"
        );
    }
}

#[test]
fn vm_core_consumes_owned_ir_not_frontend_internals() {
    let workspace = workspace_root();
    let root = workspace.join("crates/rsscript-vm/src/reg_vm");
    for relative in [
        "bytecode.rs",
        "calls.rs",
        "exec.rs",
        "lower.rs",
        "mir_codegen.rs",
        "model.rs",
        "scheduler.rs",
    ] {
        let source = read(&root.join(relative));
        for forbidden in [
            "crate::hir",
            "crate::syntax",
            "crate::semantic",
            "ValidatedProgram",
        ] {
            assert!(
                !source.contains(forbidden),
                "VM core `{relative}` must not consume frontend symbol `{forbidden}`; keep that dependency in compile.rs"
            );
        }
    }

    let vm_manifest: toml::Value =
        toml::from_str(&read(&workspace.join("crates/rsscript-vm/Cargo.toml"))).unwrap();
    let vm_dependencies = dependency_packages(&vm_manifest);
    for required in [
        "rsscript-bytecode",
        "rsscript-exec-ir",
        "rsscript-mir",
        "rsscript-provider-api",
    ] {
        assert!(
            vm_dependencies.contains(required),
            "VM must depend on `{required}`"
        );
    }
    for forbidden in [
        "rsscript",
        "rsscript-compiler",
        "rsscript-lowering",
        "rsscript-semantics",
        "rsscript-syntax",
    ] {
        assert!(
            !vm_dependencies.contains(forbidden),
            "VM must not depend on frontend package `{forbidden}`"
        );
    }

    let ir_manifest: toml::Value =
        toml::from_str(&read(&workspace.join("crates/rsscript-exec-ir/Cargo.toml"))).unwrap();
    assert_eq!(
        dependency_packages(&ir_manifest),
        BTreeSet::from(["rsscript-abi-model".to_string()]),
        "owned executable IR must remain independent of frontend and runtime crates"
    );
}

#[test]
fn mir_codegen_is_a_vm_independent_verified_bytecode_boundary() {
    let root = workspace_root();
    let codegen = read(&root.join("crates/rsscript-codegen-vm/src/lib.rs"));
    for forbidden in ["rsscript_exec_ir", "ExecutableIr", "ExecutableStmt", "Hir"] {
        assert!(
            !codegen.contains(forbidden),
            "MIR codegen must not reconstruct legacy frontend/executable state `{forbidden}`"
        );
    }
    for required in ["MirModule", "MirInstruction", "MirTerminator", "CallKnown"] {
        assert!(
            codegen.contains(required),
            "MIR codegen must lower verified MIR fact `{required}`"
        );
    }
    let manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-codegen-vm/Cargo.toml"))).unwrap();
    assert_eq!(
        normal_dependency_packages(&manifest),
        BTreeSet::from([
            "rsscript-abi-model".to_string(),
            "rsscript-bytecode".to_string(),
            "rsscript-mir".to_string(),
            "serde_json".to_string(),
        ]),
        "MIR codegen must stay independent from VM, compiler, syntax, package, and SDK"
    );
    let adapter = read(&root.join("crates/rsscript-sdk/src/vm_adapter.rs"));
    let compile_mir = function_source(&adapter, "pub fn reg_vm_compile_mir");
    assert!(
        compile_mir.contains("rsscript_codegen_vm::emit_artifact")
            && compile_mir.contains("BytecodeVerifier::default")
            && compile_mir.contains("RegVmExecutable::from_verified_bytecode"),
        "SDK MIR compilation must pass through codegen, verifier, then the VM token boundary"
    );
}

#[test]
fn compiler_default_dependency_closure_is_host_neutral() {
    let root = workspace_root();
    let facade: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-sdk/Cargo.toml")))
        .expect("embedding compiler manifest should parse");
    assert!(
        facade["features"]["default"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "the compiler facade must be frontend-only unless execution is explicitly enabled"
    );
    assert_eq!(
        facade["dependencies"]["rsscript_compiler"]["default-features"].as_bool(),
        Some(false)
    );
    assert_eq!(
        facade["dependencies"]["rsscript-provider-api"]["optional"].as_bool(),
        Some(true)
    );
    assert_eq!(
        facade["package"]["publish"].as_bool(),
        Some(false),
        "the alpha SDK must not advertise a broken crates.io package graph"
    );

    let manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    assert_eq!(
        manifest["package"]["name"].as_str(),
        Some("rsscript-compiler")
    );
    assert_eq!(manifest["package"]["publish"].as_bool(), Some(false));
    for forbidden in ["rsscript-runtime", "rsscript-aot-runtime"] {
        assert!(
            manifest["dependencies"].get(forbidden).is_none(),
            "compiler/VM core must not depend on generated-Rust runtime `{forbidden}`"
        );
    }
    assert!(
        manifest["dependencies"].get("rss-native-abi").is_none(),
        "compiler must not depend on the native plugin ABI"
    );
    assert!(
        manifest["dependencies"].get("rss-process-guard").is_none(),
        "compiler must not own child-process execution"
    );
    assert!(manifest["dependencies"].get("vm-jit").is_none());
    assert!(
        manifest["dependencies"].get("rsscript-vm").is_none(),
        "compiler must not depend on the VM"
    );
}

#[test]
fn concrete_host_providers_are_leaf_composition_packages() {
    let root = workspace_root();
    let compiler_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml"))).unwrap();
    let compiler_dependencies = normal_dependency_packages(&compiler_manifest);
    let providers = [
        "fs", "env", "process", "http", "time", "entropy", "log", "cli",
    ];
    for provider in providers {
        let manifest_path = root.join("providers").join(provider).join("Cargo.toml");
        let manifest: toml::Value = toml::from_str(&read(&manifest_path)).unwrap();
        let package = package_name(&manifest);
        let dependencies = normal_dependency_packages(&manifest);
        assert!(dependencies.contains("rsscript-provider-api"));
        for forbidden in [
            "rsscript",
            "rsscript-runtime",
            "rsscript-aot-runtime",
            "rsscript-semantics",
            "reir",
            "vm-jit",
        ] {
            assert!(
                !dependencies.contains(forbidden),
                "provider `{package}` must not depend on `{forbidden}`"
            );
        }
        assert!(
            !compiler_dependencies.contains(package),
            "compiler must not select concrete provider `{package}`"
        );
    }
}

#[test]
fn provider_contracts_can_be_generated_without_the_engine_or_runtime() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-provider-bindgen/Cargo.toml"),
    ))
    .expect("Provider bindgen manifest should parse");
    let dependencies = dependency_packages(&manifest);
    assert!(dependencies.contains("rsscript-abi-model"));
    assert!(dependencies.contains("rsscript-semantics"));
    for forbidden in [
        "rsscript-compiler",
        "rsscript-sdk",
        "rsscript-runtime",
        "rsscript-aot-runtime",
        "rsscript-provider-api",
        "vm-jit",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "Provider bindgen must not depend on `{forbidden}`"
        );
    }

    for provider in [
        "cli", "entropy", "env", "fs", "http", "log", "process", "time",
    ] {
        let source = read(&root.join(format!("providers/{provider}/src/lib.rs")));
        assert!(
            source.contains("provider_contract.rs"),
            "Provider `{provider}` must include its generated contract"
        );
        assert!(
            !source.contains("FunctionSignature"),
            "Provider `{provider}` must not hand-author ABI signatures"
        );
        assert!(
            root.join(format!("providers/{provider}/interface/lib.rssi"))
                .is_file(),
            "Provider `{provider}` must own a canonical .rssi interface"
        );
        let provider_manifest: toml::Value = toml::from_str(&read(
            &root.join(format!("providers/{provider}/Cargo.toml")),
        ))
        .unwrap();
        assert_eq!(
            provider_manifest["build-dependencies"]["rsscript-provider-bindgen"]["path"].as_str(),
            Some("../../crates/rsscript-provider-bindgen")
        );
    }
}

#[test]
fn provider_bindgen_consumes_semantic_descriptors_not_syntax() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-provider-bindgen/Cargo.toml"),
    ))
    .expect("provider bindgen manifest should parse");
    let dependencies = dependency_packages(&manifest);
    assert!(dependencies.contains("rsscript-semantics"));
    assert!(!dependencies.contains("rsscript-syntax"));
    let source = read(&root.join("crates/rsscript-provider-bindgen/src/lib.rs"));
    assert!(source.contains("from_descriptor"));
    assert!(!source.contains("parse_source("));
}

#[test]
fn interface_catalog_is_platform_neutral() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-interface-catalog/Cargo.toml"),
    ))
    .expect("interface catalog manifest should parse");
    let dependencies = dependency_packages(&manifest);
    assert!(
        dependencies.is_empty(),
        "the interface catalog must remain data-only"
    );

    let catalog = read(&root.join("crates/rsscript-interface-catalog/src/lib.rs"));
    for forbidden in ["host/", "provider", "policy", "capability"] {
        assert!(
            !catalog.to_ascii_lowercase().contains(forbidden),
            "interface catalog must not contain `{forbidden}`"
        );
    }

    for removed in [
        "stdlib/clock/clock.rssi",
        "stdlib/env/env.rssi",
        "stdlib/fs/directory.rssi",
        "stdlib/fs/file.rssi",
        "stdlib/http/client.rssi",
        "stdlib/process/process.rssi",
        "stdlib/random/random.rssi",
        "stdlib/tempdir/tempdir.rssi",
        "stdlib/workspace/workspace.rssi",
        "packages/async/interface/file.rssi",
        "packages/async/interface/http.rssi",
        "packages/async/interface/process.rssi",
        "packages/async/interface/timer.rssi",
    ] {
        assert!(
            !root.join(removed).exists(),
            "legacy host façade must not return at `{removed}`"
        );
    }
}

#[test]
fn runtime_does_not_depend_on_the_compiler_package() {
    let root = workspace_root();
    let manifest_path = root.join("experiments/aot-runtime/Cargo.toml");
    let manifest: toml::Value =
        toml::from_str(&read(&manifest_path)).expect("runtime Cargo.toml should parse");
    let dependencies = dependency_packages(&manifest);
    assert_eq!(
        manifest["package"]["name"].as_str(),
        Some("rsscript-aot-runtime"),
        "the generated-Rust runtime must be named as an AOT-only integration"
    );

    let default_features = manifest["features"]["default"]
        .as_array()
        .expect("runtime default features should be an array")
        .iter()
        .map(|feature| feature.as_str().expect("feature should be a string"))
        .collect::<BTreeSet<_>>();
    assert_eq!(
        default_features,
        BTreeSet::from(["core"]),
        "the default runtime must not enable concrete network services"
    );

    let runtime_source_dir = root.join("experiments/aot-runtime/src");
    for host_module in [
        "domain.rs",
        "env.rs",
        "fs.rs",
        "network/mod.rs",
        "process.rs",
        "process/capture.rs",
        "process/environment.rs",
        "process/policy.rs",
        "process/supervisor.rs",
        "random.rs",
        "socket.rs",
        "tempdir.rs",
        "websocket.rs",
    ] {
        assert!(
            !runtime_source_dir.join(host_module).exists(),
            "AOT runtime must not contain legacy host service `{host_module}`"
        );
    }
    for removed_feature in ["host-compat", "net"] {
        assert!(
            manifest["features"].get(removed_feature).is_none(),
            "AOT runtime must not retain legacy `{removed_feature}` feature"
        );
    }
    for removed_dependency in [
        "rand",
        "reqwest",
        "rss-process-guard",
        "tokio-tungstenite",
        "toml",
        "uuid",
    ] {
        assert!(
            manifest["dependencies"].get(removed_dependency).is_none(),
            "AOT runtime must not depend on concrete host crate `{removed_dependency}`"
        );
    }

    assert!(
        !dependencies.contains("rsscript"),
        "{} must not depend on the rsscript compiler/package",
        manifest_path.strip_prefix(&root).unwrap().display()
    );

    for path in rust_files_below(&root.join("experiments/aot-runtime/src")) {
        let source = read(&path);
        assert!(
            !source.contains("rsscript_sdk::"),
            "{} must not import the rsscript compiler/package",
            path.strip_prefix(&root).unwrap().display()
        );
    }
}

#[test]
fn abi_and_provider_crates_keep_one_way_dependencies() {
    let root = workspace_root();
    let abi_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-abi-model/Cargo.toml")))
            .expect("ABI model manifest should parse");
    let provider_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-provider-api/Cargo.toml")))
            .expect("provider API manifest should parse");
    let abi_dependencies = dependency_packages(&abi_manifest);
    let provider_dependencies = dependency_packages(&provider_manifest);

    for forbidden in [
        "rsscript",
        "rsscript-runtime",
        "rsscript-aot-runtime",
        "rss-native-abi",
        "rss-process-guard",
        "reir",
        "vm-jit",
    ] {
        assert!(
            !abi_dependencies.contains(forbidden),
            "ABI model must not depend on `{forbidden}`"
        );
        assert!(
            !provider_dependencies.contains(forbidden),
            "provider API must not depend on `{forbidden}`"
        );
    }
    assert!(
        provider_dependencies.contains("rsscript-abi-model"),
        "provider API must consume the shared ABI model"
    );
    let provider_source = read(&root.join("crates/rsscript-provider-api/src/lib.rs"));
    assert!(provider_source.contains("pub enum NativeValue"));
    assert!(provider_source.contains("pub struct NativeInterpreterFn"));
    let native_source = read(&root.join("experiments/native-abi/src/lib.rs"));
    assert!(
        native_source.contains("pub use rsscript_provider_api"),
        "the native adapter must reuse provider runtime values rather than own them"
    );
}

#[test]
fn artifact_verifier_owns_instruction_validation() {
    let root = workspace_root();
    let manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-bytecode/Cargo.toml")))
            .expect("bytecode manifest should parse");
    let dependencies = dependency_packages(&manifest);
    for forbidden in [
        "rsscript-sdk",
        "rsscript-compiler",
        "rsscript-semantics",
        "rsscript-runtime",
        "rsscript-aot-runtime",
        "vm-jit",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "artifact verifier must not depend on `{forbidden}`"
        );
    }
    let verifier = read(&root.join("crates/rsscript-bytecode/src/lib.rs"));
    for invariant in [
        "verify_executable_payload",
        "max_functions",
        "max_registers_per_function",
        "max_instructions",
        "unknown opcode",
        "external call table mismatch",
    ] {
        assert!(
            verifier.contains(invariant),
            "artifact verifier must enforce `{invariant}`"
        );
    }
}

#[test]
fn sdk_passes_verified_bytecode_to_the_vm_loader() {
    let root = workspace_root();
    let sdk = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    assert!(sdk.contains("BytecodeVerifier::default()"));
    assert!(sdk.contains("RegVmExecutable::from_verified_bytecode"));
    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(vm.contains("pub fn from_verified_bytecode"));
}

#[test]
fn bytecode_language_compatibility_is_not_inferred_from_compiler_version() {
    let root = workspace_root();
    let verifier = read(&root.join("crates/rsscript-bytecode/src/lib.rs"));
    let compatibility = verifier
        .split("impl Default for BytecodeCompatibility")
        .nth(1)
        .and_then(|source| source.split("impl BytecodeVerifier").next())
        .expect("bytecode compatibility default");
    assert!(compatibility.contains("SUPPORTED_LANGUAGE_SEMANTICS"));
    assert!(
        !compatibility.contains("CARGO_PKG_VERSION"),
        "language compatibility must not be derived from compiler provenance"
    );

    let emitter = read(&root.join("crates/rsscript-vm/src/reg_vm/bytecode.rs"));
    assert!(emitter.contains("LANGUAGE_SEMANTICS_VERSION"));
    assert!(emitter.contains("env!(\"CARGO_PKG_VERSION\")"));
    assert!(verifier.contains("BYTECODE_CONTAINER_FORMAT_VERSION"));

    let analysis = read(&root.join("crates/rsscript-compiler/src/package/analysis.rs"));
    assert!(analysis.contains("rsscript_abi_model::LANGUAGE_SEMANTICS_VERSION"));
    assert!(
        !analysis.contains("language_version: env!(\"CARGO_PKG_VERSION\")"),
        "neutral analysis must carry language semantics rather than compiler provenance"
    );
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
    let compiler_output = read(&root.join("crates/rsscript-compiler/src/compiler_output.rs"));
    let compile_source = function_source(&compiler_output, "pub fn compile_source_to_ir");
    assert!(
        compile_source.contains("validate_source(file, source)")
            && compile_source.contains("compile_validated_to_ir(&validated)"),
        "compiler source lowering must consume a ValidatedProgram"
    );
    let compile_adapter = read(&root.join("crates/rsscript-sdk/src/vm_adapter.rs"));
    let compile_validated = function_source(&compile_adapter, "pub fn reg_vm_compile_validated");
    assert!(
        compile_validated.contains("compile_validated_to_ir")
            && compile_validated.contains("emit_ir(&compiled)"),
        "register VM lowering must consume checked executable IR"
    );
    let vm_sources = [
        read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs")),
        read(&root.join("crates/rsscript-vm/src/reg_vm/lower.rs")),
        read(&root.join("crates/rsscript-vm/src/reg_vm/model.rs")),
        read(&root.join("crates/rsscript-vm/src/reg_vm/bytecode.rs")),
    ]
    .join("\n");
    for forbidden in ["crate::hir", "crate::syntax::ast", "typed_hir()"] {
        assert!(
            !vm_sources.contains(forbidden),
            "VM instruction lowering must not consume frontend representation `{forbidden}`"
        );
    }
    let executable_ir = read(&root.join("crates/rsscript-exec-ir/src/lib.rs"));
    assert!(executable_ir.contains("program: ExecutableProgram"));
    assert!(!executable_ir.contains("pub fn typed_hir"));

    let rust_lower = read(&root.join("crates/rsscript-compiler/src/rust_lower/mod.rs"));
    let lower_source = function_source(&rust_lower, "pub fn lower_source_to_rust_with_map");
    assert!(
        lower_source.contains("validate_source(file, source)")
            && lower_source.contains("validated.database()"),
        "Rust source lowering must consume a ValidatedProgram"
    );

    let helpers = read(&root.join("crates/rsscript-compiler/src/rust_lower/helpers.rs"));
    assert!(
        !helpers.contains("parse_source"),
        "lowering declaration projections must reuse parsed semantic inputs"
    );
    assert!(
        rust_lower.contains("lower_validated_hir")
            && rust_lower.contains("RustLowerer::new_validated"),
        "Rust AOT lowering must consume the same checked executable IR"
    );

    let lowering_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-lowering/Cargo.toml")))
            .expect("lowering manifest should parse");
    let dependencies = dependency_packages(&lowering_manifest);
    assert!(dependencies.contains("rsscript-semantics"));
    assert!(dependencies.contains("rsscript-mir"));
    for forbidden in [
        "rsscript",
        "rsscript-runtime",
        "rsscript-aot-runtime",
        "rsscript-provider-api",
        "rss-native-abi",
        "rss-process-guard",
        "reir",
        "vm-jit",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "executable IR must not depend on `{forbidden}`"
        );
    }
}

#[test]
fn typed_mir_has_a_frontend_free_dependency_boundary() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-mir/Cargo.toml")))
        .expect("MIR manifest should parse");
    let dependencies = dependency_packages(&manifest);

    for forbidden in [
        "rsscript",
        "rsscript-compiler",
        "rsscript-syntax",
        "rsscript-semantics",
        "rsscript-lowering",
        "rsscript-vm",
        "rsscript-provider-api",
        "rsscript-runtime",
        "reir",
        "vm-jit",
    ] {
        assert!(
            !dependencies.contains(forbidden),
            "rsscript-mir must not depend on {forbidden}"
        );
    }

    let mir = read(&root.join("crates/rsscript-mir/src/lib.rs"));
    for required in [
        "mir_id!(FunctionId)",
        "mir_id!(BlockId)",
        "mir_id!(ValueId)",
        "mir_id!(PlaceId)",
        "pub struct BasicBlock",
        "pub enum MirInstruction",
        "pub enum MirTerminator",
        "BorrowRead",
        "pub enum MirCallArgument",
        "pub enum MirParameterMode",
        "pub enum MirCallTarget",
        "pub struct MirFunctionSignature",
        "pub struct MirModule",
        "pub fn verify",
    ] {
        assert!(mir.contains(required), "MIR is missing {required}");
    }
    for forbidden in ["rsscript_syntax", "rsscript_semantics", "Unknown"] {
        assert!(
            !mir.contains(forbidden),
            "MIR must not expose source-shaped escape hatch {forbidden}"
        );
    }
}

#[test]
fn compiler_and_vm_do_not_embed_execution_authority() {
    let root = workspace_root();
    let vm_model = read(&root.join("crates/rsscript-vm/src/reg_vm/model.rs"));
    assert!(
        !vm_model.contains("host_authority"),
        "VM instructions must not carry runner authority policy"
    );

    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(
        !vm.contains("execution_context"),
        "VM core must not own an execution policy context"
    );
    let intrinsics = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/mod.rs"));
    assert!(
        !intrinsics.contains("authorize_intrinsic_host_access"),
        "intrinsic dispatch must be independent of runner policy"
    );
    assert!(
        !root
            .join("crates/rsscript-vm/src/reg_vm/host_adapters.rs")
            .exists()
    );
}

#[test]
fn provider_contract_uses_a_neutral_host_call_context() {
    let root = workspace_root();
    let provider_api = read(&root.join("crates/rsscript-provider-api/src/lib.rs"));
    assert!(provider_api.contains("pub struct HostCallContext"));
    assert!(provider_api.contains("pub host_context: &'a HostCallContext"));
    assert!(
        !provider_api.contains("ProviderAuthority"),
        "Core Provider ABI must not restore policy-shaped authority types"
    );
}

#[test]
fn vm_core_does_not_embed_filesystem_intrinsics() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    for forbidden in [
        "Directory",
        "FileError",
        "HashSha256File",
        "JsonParseFile",
        "PathReadString",
        "PathWriteString",
        "TempDir",
        "TomlParseFile",
        "YamlParseFile",
    ] {
        assert!(
            !catalog.contains(forbidden),
            "filesystem operation `{forbidden}` must be supplied by an external provider"
        );
    }

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        assert!(
            !source.contains("std::fs"),
            "VM core must not access the filesystem directly: {}",
            path.display()
        );
    }
}

#[test]
fn vm_core_does_not_embed_process_intrinsics() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    assert!(!catalog.contains("{ id = \"Process"));
    assert!(!catalog.contains("{ namespace = \"Process\""));

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        for forbidden in ["std::process::Command", "RegIntrinsic::Process"] {
            assert!(
                !source.contains(forbidden),
                "VM core must not execute child processes directly: {} contains `{forbidden}`",
                path.display()
            );
        }
    }
}

#[test]
fn vm_core_does_not_embed_network_intrinsics() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    for prefix in ["Http", "Tcp", "WebSocket"] {
        assert!(!catalog.contains(&format!("{{ id = \"{prefix}")));
        assert!(!catalog.contains(&format!("{{ namespace = \"{prefix}")));
    }

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        for forbidden in [
            "std::net",
            "TcpStream",
            "RegIntrinsic::Http",
            "RegIntrinsic::WebSocket",
        ] {
            assert!(
                !source.contains(forbidden),
                "VM core must not access the network directly: {} contains `{forbidden}`",
                path.display()
            );
        }
    }
}

#[test]
fn vm_core_does_not_embed_time_logging_or_os_intrinsics() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    for prefix in ["Deadline", "InstantElapsed", "Log", "OsClose", "Timer"] {
        assert!(
            !catalog.contains(&format!("{{ id = \"{prefix}")),
            "host operation `{prefix}` must be supplied by an external provider"
        );
    }
    for namespace in ["Deadline", "Instant", "Log", "OS", "Timer"] {
        assert!(!catalog.contains(&format!("{{ namespace = \"{namespace}\"")));
    }

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        for forbidden in [
            "SystemTime",
            "std::thread::sleep",
            "RegIntrinsic::Log",
            "RegIntrinsic::Timer",
        ] {
            assert!(
                !source.contains(forbidden),
                "VM core must not read ambient time or emit host logs: {} contains `{forbidden}`",
                path.display()
            );
        }
    }
}

#[test]
fn rust_aot_lowering_does_not_restore_removed_host_abi_types() {
    let root = workspace_root();
    let lowering_root = root.join("crates/rsscript-compiler/src/rust_lower");
    let forbidden = [
        "rsscript_runtime::File",
        "rsscript_runtime::Http",
        "rsscript_runtime::Process",
        "rsscript_runtime::RssTcp",
        "rsscript_runtime::RssWebSocket",
        "rsscript_runtime::TempDir",
        "rsscript_runtime::RssInstant",
        "rsscript_runtime::RssDeadline",
        "runtime_struct_constructor",
        "is_file_open_expr",
    ];
    for path in rust_files_below(&lowering_root) {
        let source = read(&path);
        for symbol in forbidden {
            assert!(
                !source.contains(symbol),
                "experimental AOT lowering must not restore host ABI `{symbol}` in {}",
                path.display()
            );
        }
    }
}

#[test]
fn generated_aot_abi_does_not_expose_wall_clock_or_timer_services() {
    let root = workspace_root();
    let runtime = read(&root.join("experiments/aot-runtime/src/lib.rs"));
    let abi_macro = runtime
        .split("macro_rules! runtime_abi_exports")
        .nth(1)
        .and_then(|source| source.split("/// Exact compatibility surface").next())
        .expect("generated AOT ABI macro");
    for forbidden in [
        "RssInstant",
        "clock_now",
        "clock_system_unix_ms",
        "instant_elapsed",
        "RssDeadline",
        "deadline_after",
        "TimerError",
        "TimerSleepPending",
        "timer_sleep",
        "OperationContext",
    ] {
        assert!(
            !abi_macro.contains(forbidden),
            "generated AOT ABI must obtain host time through a provider: `{forbidden}`"
        );
    }
    assert!(
        runtime.contains("pub mod host"),
        "execution deadlines remain explicit host controls"
    );
}

#[test]
fn program_arguments_enter_through_the_explicit_main_abi() {
    let root = workspace_root();
    let catalog = read(&root.join("crates/rsscript-compiler/intrinsics.toml"));
    assert!(!catalog.contains("{ id = \"Args"));
    assert!(!catalog.contains("{ namespace = \"Args\""));

    let vm_root = root.join("crates/rsscript-vm/src/reg_vm");
    for path in rust_files_below(&vm_root) {
        if path
            .components()
            .any(|component| component.as_os_str() == "tests")
        {
            continue;
        }
        let source = read(&path);
        for forbidden in ["std::env::args", "RegIntrinsic::Args"] {
            assert!(
                !source.contains(forbidden),
                "VM core must not read ambient program arguments: {} contains `{forbidden}`",
                path.display()
            );
        }
    }

    let scheduler = read(&root.join("crates/rsscript-vm/src/reg_vm/scheduler.rs"));
    assert!(scheduler.contains("List<String>"));
    assert!(scheduler.contains("self.entry_args"));
}

#[test]
fn high_risk_state_machines_keep_dedicated_module_owners() {
    let root = workspace_root();
    let required = [
        "crates/rsscript-compiler/src/analyzer/task_group.rs",
        "crates/rsscript-compiler/src/package/native/bindings.rs",
        "crates/rsscript-vm/src/reg_vm/tier/admission.rs",
        "crates/rsscript-vm/src/reg_vm/tier/call_scratch.rs",
        "crates/rsscript-vm/src/reg_vm/tier/recursion.rs",
        "crates/rsscript-compiler/src/rust_lower/helpers/executable_declarations.rs",
        "crates/rsscript-compiler/src/rust_lower/helpers/semantic_projection.rs",
        "experiments/aot-runtime/src/json.rs",
        "experiments/vm-jit/src/analysis.rs",
        "experiments/vm-jit/src/executable_memory.rs",
        "experiments/reir/src/reconciliation/engine.rs",
        "experiments/reir/src/cli/safe_io.rs",
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

#[test]
fn workspace_analysis_does_not_flow_through_optional_review() {
    let root = workspace_root();
    let authorization = read(&root.join("crates/rsscript-compiler/src/package/authorization.rs"));
    let authorization = authorization
        .split("#[cfg(test)]")
        .next()
        .expect("production authorization source");
    let analysis = read(&root.join("crates/rsscript-compiler/src/package/analysis.rs"));
    let types = read(&root.join("crates/rsscript-compiler/src/package/types.rs"));

    assert!(authorization.contains("analyze_package_dir_captured"));
    assert!(!authorization.contains("review_package_dir_captured_with_features"));
    for forbidden in ["crate::review", "super::review", "PackageRisk"] {
        assert!(
            !analysis.contains(forbidden),
            "neutral package analysis must not depend on `{forbidden}`"
        );
    }
    assert!(
        !types.contains("impl From<&PackageReview> for PackageAnalysis"),
        "review output must not be the constructor for neutral package analysis"
    );
}

#[test]
fn selfhost_frontend_does_not_restore_retired_language_contracts() {
    let root = workspace_root();
    let checker = read(&root.join("selfhost/check.rss"));
    let syntax_declarations =
        read(&root.join("selfhost/checker/diagnostics/syntax_declarations.rss"));
    for retired_code in [
        "RS0004", "RS0006", "RS0009", "RS0010", "RS0011", "RS0012", "RS0014", "RS0016", "RS0017",
        "RS0018", "RS0019", "RS0020", "RS0101",
    ] {
        assert!(
            !checker.contains(retired_code),
            "self-hosted checker must not emit retired diagnostic `{retired_code}`"
        );
    }

    let scanner = read(&root.join("selfhost/scan.rss"));
    for retired_mapping in [
        "word == \"features\"",
        "word == \"profile\"",
        "word == \"native\"",
        "word == \"effects\"",
        "word == \"unsafe\"",
    ] {
        assert!(
            !scanner.contains(retired_mapping),
            "self-hosted scanner must not restore retired keyword mapping `{retired_mapping}`"
        );
    }

    for retired_feature_check in [
        "RS0101 FEATURE_VIOLATION",
        "collect_feature_use_tokens",
        "file_local_use",
        "file_async_use",
        "file_unsafe_use",
    ] {
        assert!(
            !syntax_declarations.contains(retired_feature_check),
            "self-hosted diagnostics must not retain retired feature check `{retired_feature_check}`"
        );
    }
}

#[test]
fn github_workflows_follow_current_workspace_boundaries() {
    let root = workspace_root();
    let workflow_dir = root.join(".github/workflows");
    let workflows = fs::read_dir(&workflow_dir)
        .expect("workflow directory should exist")
        .map(|entry| entry.expect("workflow entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "yml"))
        .map(|path| read(&path))
        .collect::<Vec<_>>()
        .join("\n");

    for retired in [
        "rsscript-engine",
        "crates/rsscript-compiler/src/cli/run_cmd.rs",
        "crates/rsscript-compiler/src/native_plugin/",
        "crates/runtime/src/fs.rs",
        "crates/runtime/src/process.rs",
        "crates/runtime/src/socket.rs",
        "crates/runtime/src/websocket.rs",
    ] {
        assert!(
            !workflows.contains(retired),
            "GitHub workflows must not reference retired workspace boundary `{retired}`"
        );
    }

    for current in [
        "-p rsscript-compiler",
        "crates/rsscript-bytecode/**",
        "crates/rsscript-provider-api/**",
        "crates/rsscript-provider-conformance/**",
        "fuzz run bytecode_artifact",
        "fuzz run binding_descriptor",
        "fuzz run execution_report",
        "providers/**",
    ] {
        assert!(
            workflows.contains(current),
            "GitHub workflows must cover current workspace boundary `{current}`"
        );
    }

    let release = read(&workflow_dir.join("release.yml"));
    assert!(release.contains("for PACKAGE in rsscript-cli reir"));
    assert!(!release.contains("for PACKAGE in rsscript reir"));
    for target in [
        "x86_64-unknown-linux-gnu",
        "aarch64-apple-darwin",
        "x86_64-pc-windows-msvc",
    ] {
        assert!(
            release.contains(target),
            "release dry-run must build supported target `{target}`"
        );
    }
    assert!(release.contains("merge-multiple: true"));
    assert!(release.contains("prerelease: ${{ contains(github.ref_name, '-') }}"));
}
