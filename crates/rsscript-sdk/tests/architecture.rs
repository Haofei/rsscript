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

    let language_service = metadata_normal_dependencies(&metadata, "rsscript-language-service");
    assert_eq!(
        language_service,
        BTreeSet::from([
            "rsscript-diagnostics".to_string(),
            "rsscript-operation".to_string(),
            "rsscript-semantics".to_string(),
            "rsscript-syntax".to_string(),
        ]),
        "language service must depend only on frontend, diagnostics, and shared operation contracts"
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
fn selfhost_parity_is_an_explicit_research_feature_not_a_release_gate() {
    let root = workspace_root();
    let compiler_manifest = read(&root.join("crates/rsscript-compiler/Cargo.toml"));
    let compiler = read(&root.join("crates/rsscript-compiler/src/lib.rs"));
    let selfhost_workflow = read(&root.join(".github/workflows/selfhost.yml"));
    let release_workflow = read(&root.join(".github/workflows/release.yml"));

    assert!(
        compiler_manifest.contains("selfhost-parity = [\"dep:rsscript-vm\"]"),
        "the Research harness must require an explicit compiler feature"
    );
    assert!(
        compiler.contains("feature = \"selfhost-parity\""),
        "self-host test modules must be gated at their compilation boundary"
    );
    assert!(
        selfhost_workflow.contains("--features execution,selfhost-parity"),
        "the dedicated Research workflow must opt in explicitly"
    );
    assert!(
        !release_workflow.contains("selfhost_parity::"),
        "Research parity must not block the supported release path"
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
fn core_manifests_do_not_pull_test_generation_into_default_tests() {
    let metadata = cargo_metadata(&workspace_root());
    for package in ["rsscript-sdk", "rsscript-compiler"] {
        assert!(
            !metadata_direct_dependencies(&metadata, package).contains("rss-testgen"),
            "Core package `{package}` must not depend on the experimental test generator"
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
fn compiler_checks_only_construct_nonsemantic_boundary_diagnostics() {
    let root = workspace_root();
    let checks_root = root.join("crates/rsscript-compiler/src/checks");
    let direct_diagnostic_owners = rust_files_below(&checks_root)
        .into_iter()
        .filter_map(|path| {
            read(&path).contains("Diagnostic::").then(|| {
                path.strip_prefix(&root)
                    .unwrap_or(&path)
                    .display()
                    .to_string()
            })
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        direct_diagnostic_owners,
        BTreeSet::new(),
        "compiler must not retain frontend checks or construct language diagnostics"
    );
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
    assert!(runner.contains("validate_response_profile"));
    assert!(runner.contains("command.current_dir(\"/\")"));
    let process_guard = read(&root.join("crates/process-guard/src/lib.rs"));
    assert!(process_guard.contains("PR_SET_NO_NEW_PRIVS"));
    assert!(process_guard.contains("configure_strict_platform"));
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
    assert!(metadata.contains("rsscript_semantics::BUILTIN_TYPE_NAMES"));
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
    for source_type in ["FileId", "SourceRevision", "ModuleId", "InterfaceId"] {
        assert!(
            source_model.contains("stable_id!(")
                && source_model.contains(&format!("{source_type},")),
            "source model must own `{source_type}`"
        );
    }
    for source_type in ["pub struct TextRange", "pub struct Span"] {
        assert!(
            source_model.contains(source_type),
            "source model must own `{source_type}"
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
    assert_eq!(
        loader_dependencies,
        BTreeSet::from([
            "rsscript-operation".to_string(),
            "serde".to_string(),
            "sha2".to_string(),
            "toml".to_string(),
        ])
    );
    let workspace_loader = read(&root.join("crates/rsscript-workspace-loader/src/lib.rs"));
    for boundary in [
        "pub struct WorkspaceSnapshot",
        "pub fn snapshot_from",
        "pub fn snapshot_from_with_operation",
        "pub fn load_from",
        "pub fn content_digest",
        "pub logical_path: String",
        "pub struct WorkspaceManifestV1",
        "pub struct WorkspacePathDependencyV1",
    ] {
        assert!(
            workspace_loader.contains(boundary),
            "workspace loader must retain explicit input boundary {boundary}"
        );
    }
    for forbidden in [
        "std::env::current_dir",
        "pub fn snapshot(",
        "pub fn snapshot_with_operation(",
        "pub fn load(",
    ] {
        assert!(
            !workspace_loader.contains(forbidden),
            "workspace loader must not retain ambient-current-directory compatibility API `{forbidden}`"
        );
    }

    let description = language_manifest["package"]["description"]
        .as_str()
        .expect("language-service description");
    assert!(description.to_ascii_lowercase().contains("incremental"));
    for boundary in [
        "lint_cache",
        "format_cache",
        "symbol_cache",
        "workspace_module_graph",
    ] {
        assert!(
            language_service.contains(boundary),
            "language service must retain query boundary `{boundary}`"
        );
    }
    assert!(
        !language_service.contains("diagnostic_cache:"),
        "language-service semantic diagnostics must use only the shared CompilationSession cache"
    );
    assert!(
        language_service.contains("CompilationSession"),
        "language-service dependency queries must consume the shared frontend session"
    );
    for query in [
        "symbol_index_from_program",
        "document_symbols_from_program",
        "workspace_module_graph",
    ] {
        assert!(
            language_service.contains(query),
            "language-service editor query must consume parsed syntax through {query}"
        );
    }
    for forbidden in [
        "parse_source",
        "fn document_dependencies",
        "fn interface_modules",
        "dependency_cache:",
        "fn dependency_matches_module",
    ] {
        assert!(
            !language_service.contains(forbidden),
            "language-service must delegate parsed module facts to CompilationSession, not `{forbidden}`"
        );
    }
    assert!(
        !language_service.contains("fn declaration_target"),
        "language-service must not derive module graph edges from text lines"
    );
}

#[test]
fn artifact_persistence_is_an_execution_only_adapter() {
    let root = workspace_root();
    let adapter_manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-artifact-store/Cargo.toml"),
    ))
    .expect("artifact-store manifest should parse");
    let dependencies = dependency_packages(&adapter_manifest);
    assert_eq!(
        dependencies,
        BTreeSet::from(["fs2".to_string(), "rustix".to_string(), "uuid".to_string(),]),
        "artifact persistence adapter must not depend on compiler, VM, package, or Provider crates"
    );

    let compiler_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    let store = compiler_manifest["dependencies"]["rsscript-artifact-store"]
        .as_table()
        .expect("compiler package compatibility must declare the persistence adapter");
    assert_eq!(
        store.get("optional").and_then(toml::Value::as_bool),
        Some(true),
        "compiler persistence remains confined to its explicit package feature"
    );
    let package_feature = compiler_manifest["features"]["package"]
        .as_array()
        .expect("compiler package feature should be declared");
    assert!(
        package_feature
            .iter()
            .filter_map(toml::Value::as_str)
            .any(|entry| entry == "dep:rsscript-artifact-store"),
        "compiler package compatibility must opt in explicitly to persistence"
    );

    let package = read(&root.join("crates/rsscript-compiler/src/package.rs"));
    assert!(
        package
            .contains("#[cfg(test)]\nuse rsscript_artifact_store::write_package_artifact_atomic")
    );
    assert!(
        !package.contains("pub use rsscript_artifact_store"),
        "compiler package compatibility code must not make persistence a compiler API"
    );
    let compiler = read(&root.join("crates/rsscript-compiler/src/lib.rs"));
    assert!(!compiler.contains("ArtifactStore,"));
    assert!(!compiler.contains("write_package_artifact_atomic,"));
    let sdk = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    assert!(sdk.contains("pub use rsscript_artifact_store::{ArtifactStore"));
    assert!(
        !root
            .join("crates/rsscript-compiler/src/package/artifact_store.rs")
            .exists(),
        "the compiler must not retain a second persistence implementation"
    );
}

#[test]
fn semantic_diff_is_an_artifact_contract_not_sdk_implementation() {
    let root = workspace_root();
    let artifact = read(&root.join("crates/rsscript-artifact/src/lib.rs"));
    let sdk = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    assert!(artifact.contains("mod semantic_diff;"));
    assert!(artifact.contains("SemanticDiffV1"));
    assert!(artifact.contains("SEMANTIC_DIFF_SCHEMA"));
    assert!(
        !root
            .join("crates/rsscript-sdk/src/semantic_diff.rs")
            .exists(),
        "SDK must compose the semantic-diff contract rather than own it"
    );
    assert!(sdk.contains("pub use rsscript_artifact"));
    assert!(sdk.contains("SemanticDiffV1"));
}

#[test]
fn native_package_dependency_model_is_not_owned_by_aot_lowering() {
    let root = workspace_root();
    let package_types = read(&root.join("crates/rsscript-compiler/src/package/types.rs"));
    assert!(
        package_types.contains("pub struct NativeRustDependency"),
        "the package snapshot must own native dependency identity"
    );

    let native_loader = read(&root.join("crates/rsscript-compiler/src/package/native.rs"));
    assert!(
        native_loader.contains("use super::NativeRustDependency;"),
        "native package loading must consume the package-owned dependency model"
    );
    assert!(
        !native_loader.contains("crate::rust_lower::NativeRustDependency"),
        "package loading must not depend on the experimental Rust lowerer"
    );

    let lower_types = read(&root.join("crates/rsscript-compiler/src/rust_lower/types.rs"));
    assert!(
        !lower_types.contains("pub struct NativeRustDependency"),
        "the experimental Rust lowerer must not define package dependency identity"
    );

    let lowerer = read(&root.join("crates/rsscript-compiler/src/rust_lower/mod.rs"));
    assert!(
        lowerer.contains("pub use crate::package::NativeRustDependency;"),
        "the Rust lowerer may retain a compatibility re-export while it consumes package input"
    );
}

#[test]
fn rust_aot_lowering_is_explicitly_feature_gated() {
    let root = workspace_root();
    let manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    let execution = manifest["features"]["execution"]
        .as_array()
        .expect("compiler execution feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(
        !execution.contains("aot-rust"),
        "ordinary execution must not select the experimental Rust/AOT lowerer"
    );
    let aot = manifest["features"]["aot-rust"]
        .as_array()
        .expect("compiler aot-rust feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        aot,
        BTreeSet::from(["execution"]),
        "Rust/AOT lowering must be an explicit extension of execution"
    );

    let compiler = read(&root.join("crates/rsscript-compiler/src/lib.rs"));
    assert!(compiler.contains("#[cfg(feature = \"aot-rust\")]\nmod rust_lower;"));
    assert!(compiler.contains("#[cfg(feature = \"execution\")]\nmod lower_names;"));

    let symbols = read(&root.join("crates/rsscript-compiler/src/symbols.rs"));
    assert!(symbols.contains("crate::lower_names::lowered_symbol_name"));
    assert!(
        !symbols.contains("crate::rust_lower::"),
        "execution metadata must not require compiling the Rust/AOT lowerer"
    );

    let sdk: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-sdk/Cargo.toml")))
        .expect("SDK manifest should parse");
    let sdk_execution = sdk["features"]["execution"]
        .as_array()
        .expect("SDK execution feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(
        !sdk_execution.contains("rsscript_compiler/aot-rust"),
        "reviewed SDK execution must not select AOT by default"
    );
    let sdk_aot = sdk["features"]["aot-rust"]
        .as_array()
        .expect("SDK aot-rust feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(sdk_aot.contains("execution"));
    assert!(sdk_aot.contains("rsscript_compiler/aot-rust"));

    let cli: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-cli/Cargo.toml")))
        .expect("CLI manifest should parse");
    let cli_execution = cli["features"]["execution"]
        .as_array()
        .expect("CLI execution feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(
        !cli_execution.contains("rsscript-compiler/aot-rust"),
        "ordinary CLI execution must not select the experimental Rust/AOT lowerer"
    );
    let cli_aot = cli["features"]["aot-rust"]
        .as_array()
        .expect("CLI aot-rust feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(cli_aot.contains("execution"));
    assert!(cli_aot.contains("rsscript-compiler/aot-rust"));

    let run_command = read(&root.join("crates/rsscript-cli/src/cli/run_cmd.rs"));
    assert!(
        run_command.contains("#[cfg(not(feature = \"aot-rust\"))]"),
        "an ordinary CLI build must reject the unavailable experimental AOT path explicitly"
    );
    let cli_help = read(&root.join("crates/rsscript-cli/src/cli/mod.rs"));
    assert!(
        cli_help.contains("#[cfg(feature = \"aot-rust\")]\nconst AOT_USAGE"),
        "experimental AOT help must be feature-gated rather than advertised by the default CLI"
    );
}

#[test]
fn compilation_session_owns_workspace_type_facts() {
    let root = workspace_root();
    let database = read(&root.join("crates/rsscript-semantics/src/database.rs"));
    for required in [
        "workspace_type_cache: Option<Arc<SemanticTypeFacts>>",
        "pub fn workspace_type_facts(&mut self) -> Arc<SemanticTypeFacts>",
        "pub fn workspace_type_facts_with_operation(",
        "self.workspace_type_cache = None;",
    ] {
        assert!(
            database.contains(required),
            "CompilationSession must own workspace type-fact caching through `{required}`"
        );
    }

    let language_service = read(&root.join("crates/rsscript-language-service/src/lib.rs"));
    assert!(
        !language_service.contains("SemanticTypeFacts::from_programs"),
        "language-service must not rebuild a competing type-fact model"
    );
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
#[ignore = "superseded by the physical semantic-checker ownership guard below"]
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
        "isolate_module_namespaces",
        "isolate_sources_with_interfaces",
        "cyclic_type_alias_diagnostics",
        "declaration_item_surface_diagnostics",
        "declaration_surface_diagnostics",
        "block_surface_diagnostics",
        "by_value_callback_parameter_diagnostic",
        "duplicate_declaration_diagnostics",
        "derive_syntax_diagnostics",
        "derive_field_diagnostics",
        "call_argument_diagnostics",
        "receiver_call_effect_diagnostics",
        "await_placement_diagnostics",
        "await_operand_diagnostic",
        "async_call_consumption_diagnostic",
        "async_fn_lowering_diagnostic",
        "await_live_value_diagnostics",
        "cancellation_token_outside_task_group_diagnostic",
        "weak_field_upgrade_diagnostic",
        "try_operand_diagnostic",
        "try_error_type_diagnostics",
        "integer_literal_range_diagnostic",
        "item_body_surface_diagnostics",
        "char_literal_scalar_diagnostic",
        "match_char_literal_scalar_diagnostic",
        "FreshReturnIssue",
        "FreshReturnIssueKind",
        "ManagedToLocalUse",
        "MovedUse",
        "ResourceEscape",
        "ResourceEscapeKind",
        "RetainedClosureCapture",
        "RetainedLocalUse",
        "TakeHandleField",
        "bool_condition_diagnostic",
        "for_iterable_diagnostic",
        "match_expression_arm_type_diagnostics",
        "match_scrutinee_diagnostic",
        "match_literal_type_diagnostic",
        "match_pattern_type_diagnostic",
        "match_variant_family_diagnostic",
        "variant_pattern_arity_diagnostic",
        "structured_match_effect_diagnostic",
        "match_guard_mutation_diagnostic",
        "managed_pattern_field_effect_diagnostic",
        "weakened_pattern_field_effect_diagnostic",
        "conflicting_pattern_field_effect_diagnostic",
        "duplicate_pattern_field_diagnostic",
        "unknown_pattern_field_diagnostic",
        "omitted_pattern_fields_diagnostic",
        "moved_use_diagnostic",
        "managed_to_local_diagnostic",
        "retained_local_diagnostic",
        "retained_closure_capture_diagnostic",
        "take_handle_field_diagnostic",
        "managed_closure_local_capture_diagnostic",
        "resource_escape_diagnostic",
        "resource_capture_diagnostic",
        "resource_producer_escape_diagnostic",
        "resource_producer_missing_try_diagnostic",
        "local_class_binding_diagnostic",
        "invalid_manage_operand_diagnostic",
        "invalid_take_operand_diagnostic",
        "fresh_return_not_clean_diagnostic",
        "freshness_unknown_diagnostic",
        "invalid_fresh_return_type_diagnostic",
        "fresh_requires_local_binding_diagnostic",
        "weak_field_requires_weak_handle_diagnostic",
        "constructor_field_effect_diagnostic",
        "managed_inline_constructor_field_diagnostic",
        "spawn_local_capture_diagnostic",
        "function_fallthrough_diagnostics",
        "forbidden_surface_syntax_diagnostics",
        "module_use_layout_diagnostics",
        "type_ref_surface_diagnostics",
        "unsupported_syntax_diagnostic",
        "external_binding_type_diagnostics",
        "unknown_type_name_diagnostic",
        "protocol_impl_mismatch_diagnostic",
        "protocol_declaration_diagnostics",
        "protocol_method_names",
        "protocol_signature_mismatch",
        "generic_constraint_diagnostics",
        "hir_block_identifier_uses",
        "hir_block_inline_capture_uses",
        "hir_expr_path",
        "hir_expr_type_name",
        "Flow",
        "merge_non_fallthrough",
        "LocalFlowState",
        "path_root",
        "hir_stmt_effect_events",
        "hir_stmt_identifier_uses",
        "managed_closure_uses_by_statement",
        "resource_escapes_by_with_statement",
        "retained_closure_argument",
        "is_copy_type_name",
        "is_cross_isolate_transferable",
        "take_handle_fields",
        "fresh_field_access_base",
        "fresh_handle_or_weak_field_path",
        "fresh_return_value_span",
        "fresh_match_binding",
        "FreshMatchBinding",
        "LocalBindingValueFacts",
        "local_binding_value_facts",
        "fd_surface_diagnostics",
        "unknown_binding_diagnostics",
        "unknown_field_diagnostics",
        "resource_field_diagnostics",
        "resource_generic_diagnostics",
        "resource_producer_context_diagnostic",
        "resource_producer_diagnostics",
        "resource_producer_kind",
        "result_resource_with_try_diagnostic",
        "protocol_bound_diagnostics",
        "signature_diagnostics",
        "weak_field_diagnostics",
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

    let compiler_local_facts = read(&root.join("crates/rsscript-compiler/src/checks/local.rs"));
    assert!(compiler_local_facts.contains("pub(crate) use rsscript_semantics"));
    for forbidden in [
        "pub(crate) struct MovedUse",
        "pub(crate) struct ManagedToLocalUse",
        "pub(crate) struct RetainedLocalUse",
        "pub(crate) struct RetainedClosureCapture",
        "pub(crate) struct TakeHandleField",
        "pub(crate) struct FreshReturnIssue",
        "pub(crate) enum FreshReturnIssueKind",
        "pub(crate) struct ResourceEscape",
        "pub(crate) enum ResourceEscapeKind",
    ] {
        assert!(
            !compiler_local_facts.contains(forbidden),
            "compiler must not re-own local-flow fact `{forbidden}`"
        );
    }

    let semantic_declarations = read(&root.join("crates/rsscript-semantics/src/declarations.rs"));
    assert!(semantic_declarations.contains("pub fn duplicate_declaration_diagnostics"));
    assert!(semantic_declarations.contains("DuplicateSymbolKind"));
    assert!(semantic_declarations.contains("pub fn unknown_field_diagnostics"));
    let compiler_declarations =
        read(&root.join("crates/rsscript-compiler/src/checks/declarations/duplicate_decls.rs"));
    assert!(
        compiler_declarations.contains("rsscript_semantics::duplicate_declaration_diagnostics")
    );
    assert!(
        !compiler_declarations.contains("duplicate_symbols()"),
        "compiler declaration checks must consume semantic duplicate diagnostics instead of reinterpreting HIR identity facts"
    );
    let compiler_unknowns = read(&root.join("crates/rsscript-compiler/src/analyzer/unknowns.rs"));
    assert!(compiler_unknowns.contains("rsscript_semantics::unknown_field_diagnostics"));
    assert!(compiler_unknowns.contains("rsscript_semantics::unknown_binding_diagnostics"));
    assert!(compiler_unknowns.contains("rsscript_semantics::unknown_type_diagnostics"));
    assert!(!compiler_unknowns.contains("check_unknown_bindings_in_block"));
    assert!(!compiler_unknowns.contains("unknown_binding_diagnostic("));

    let semantic_derives = read(&root.join("crates/rsscript-semantics/src/derives.rs"));
    assert!(semantic_derives.contains("pub fn derive_syntax_diagnostics"));
    let compiler_syntax_support =
        read(&root.join("crates/rsscript-compiler/src/analyzer/syntax_support.rs"));
    assert!(compiler_syntax_support.contains("rsscript_semantics::derive_syntax_diagnostics"));
    assert!(!compiler_syntax_support.contains("fn check_module_use_layout"));
    assert!(
        compiler_syntax_support.contains("rsscript_semantics::declaration_surface_diagnostics")
    );
    assert!(
        compiler_syntax_support
            .contains("rsscript_semantics::declaration_item_surface_diagnostics")
    );
    assert!(compiler_syntax_support.contains("rsscript_semantics::type_ref_surface_diagnostics"));
    assert!(!compiler_syntax_support.contains("fn check_unsupported_syntax_type_ref"));
    assert!(compiler_syntax_support.contains("rsscript_semantics::item_body_surface_diagnostics"));
    assert!(
        compiler_syntax_support
            .contains("rsscript_semantics::by_value_callback_parameter_diagnostic")
    );
    for forbidden in [
        "fn check_unsupported_syntax_block",
        "fn check_unsupported_syntax_stmt",
        "fn check_unsupported_syntax_expr",
        "in_task_group",
    ] {
        assert!(
            !compiler_syntax_support.contains(forbidden),
            "compiler must not own source-body syntax rule `{forbidden}`"
        );
    }
    for forbidden in [
        "fn check_reserved_protocol_generics",
        "fn check_reserved_declaration_names",
    ] {
        assert!(
            !compiler_syntax_support.contains(forbidden),
            "compiler must not own declaration surface rule `{forbidden}`"
        );
    }
    assert!(!compiler_syntax_support.contains("Diagnostic::"));
    let compiler_declaration_diagnostics =
        read(&root.join("crates/rsscript-compiler/src/analyzer/diagnostics.rs"));
    assert!(
        compiler_declaration_diagnostics
            .contains("rsscript_semantics::unknown_type_name_diagnostic")
    );
    assert!(
        compiler_declaration_diagnostics
            .contains("rsscript_semantics::protocol_impl_mismatch_diagnostic")
    );
    assert!(!compiler_declaration_diagnostics.contains("Diagnostic::"));
    let compiler_resource_rules =
        read(&root.join("crates/rsscript-compiler/src/checks/body/resources.rs"));
    assert!(compiler_resource_rules.contains("rsscript_semantics::resource_producer_diagnostics"));
    for forbidden in [
        "fn expr_is_resource_producer",
        "fn expr_type_is_resource",
        "fn result_resource_ok_type",
        "fn result_ok_type_name",
        "fn check_resource_producer_children",
        "fn check_resource_producer_stmt",
    ] {
        assert!(
            !compiler_resource_rules.contains(forbidden),
            "compiler must not own resource producer rule `{forbidden}`"
        );
    }
    let compiler_local_flow = read(&root.join("crates/rsscript-compiler/src/checks/local/flow.rs"));
    let compiler_local_analysis = read(&root.join("crates/rsscript-compiler/src/checks/local.rs"));
    let compiler_body = read(&root.join("crates/rsscript-compiler/src/checks/body/mod.rs"));
    assert!(
        compiler_local_analysis.contains("rsscript_semantics::managed_closure_uses_by_statement")
    );
    assert!(
        compiler_local_analysis.contains("rsscript_semantics::resource_escapes_by_with_statement")
    );
    let semantic_local_flow_builder =
        read(&root.join("crates/rsscript-semantics/src/local_flow_builder.rs"));
    assert!(semantic_local_flow_builder.contains("hir_stmt_identifier_uses"));
    assert!(semantic_local_flow_builder.contains("hir_stmt_effect_events"));
    assert!(compiler_body.contains("rsscript_semantics::is_copy_type_name"));
    assert!(semantic_local_flow_builder.contains("hir_expr_type_name"));
    assert!(
        !compiler_local_flow.contains("fn hir_expr_type_name"),
        "compiler must not re-own the HIR type projection"
    );
    assert!(compiler_body.contains("pub(crate) use rsscript_semantics::Flow"));
    assert!(
        !compiler_body.contains("pub(crate) enum Flow"),
        "compiler must not re-own the structured flow-state enum"
    );
    let semantic_local_flow_solver =
        read(&root.join("crates/rsscript-semantics/src/local_flow_solver.rs"));
    assert!(semantic_local_flow_solver.contains("merge_non_fallthrough"));
    assert!(
        compiler_local_analysis.contains("use rsscript_semantics::LocalFlowState as BodyState")
    );
    assert!(compiler_local_analysis.contains("rsscript_semantics::local_flow_entry_states"));
    assert!(compiler_local_flow.contains("rsscript_semantics::local_flow_graph"));
    assert!(
        !compiler_local_analysis.contains("pub(crate) struct BodyState"),
        "compiler must not re-own the local-flow state lattice"
    );
    for forbidden in [
        "enum LocalFlowStepKind",
        "struct LocalFlowStep",
        "struct LocalFlowBinding",
        "struct LocalFlowResourceBinding",
        "struct LocalFlowEdge",
    ] {
        assert!(
            !compiler_local_analysis.contains(forbidden),
            "compiler must not re-own local-flow graph model `{forbidden}`"
        );
    }
    assert!(
        !compiler_local_flow.contains("impl BodyState"),
        "compiler must not re-own local-flow state transitions"
    );
    for forbidden in [
        "fn collect_block_local_flow",
        "fn collect_stmt_local_flow",
        "fn collect_match_local_flow",
        "fn collect_select_local_flow",
        "fn push_local_flow_step",
        "fn collect_flow_entry_states",
        "fn transfer_flow_step",
        "fn merge_flow_entry_state",
        "fn merge_flow_states",
    ] {
        assert!(
            !compiler_local_flow.contains(forbidden),
            "compiler must not re-own local-flow solver `{forbidden}`"
        );
    }
    assert!(compiler_local_analysis.contains("rsscript_semantics::initial_local_flow_state"));
    assert!(semantic_local_flow_solver.contains("path_root"));
    assert!(
        !root
            .join("crates/rsscript-compiler/src/checks/local/ownership.rs")
            .exists(),
        "compiler must not retain moved-use or ownership flow traversal"
    );
    let compiler_assign = read(&root.join("crates/rsscript-compiler/src/analyzer/assign.rs"));
    assert!(compiler_assign.contains("rsscript_semantics::is_copy_type_name"));
    let compiler_calls = read(&root.join("crates/rsscript-compiler/src/checks/calls.rs"));
    assert!(compiler_calls.contains("rsscript_semantics::is_cross_isolate_transferable"));
    assert!(compiler_local_analysis.contains("rsscript_semantics::take_handle_fields"));
    let semantic_fresh_return_flow =
        read(&root.join("crates/rsscript-semantics/src/fresh_return_flow.rs"));
    assert!(semantic_fresh_return_flow.contains("fresh_return_issues_from_flow"));
    assert!(semantic_fresh_return_flow.contains("fresh_return_value_span"));
    assert!(compiler_local_analysis.contains("rsscript_semantics::fresh_return_issues_from_flow"));
    let semantic_retained_closure_flow =
        read(&root.join("crates/rsscript-semantics/src/retained_closure_flow.rs"));
    assert!(semantic_retained_closure_flow.contains("retained_closure_captures_from_flow"));
    assert!(
        compiler_local_analysis.contains("rsscript_semantics::retained_closure_captures_from_flow")
    );
    let semantic_moved_use_flow =
        read(&root.join("crates/rsscript-semantics/src/moved_use_flow.rs"));
    assert!(semantic_moved_use_flow.contains("moved_uses_from_flow"));
    assert!(semantic_moved_use_flow.contains("collect_closure_local_moved_uses_from_block"));
    assert!(semantic_moved_use_flow.contains("apply_match_take_move"));
    assert!(compiler_local_analysis.contains("rsscript_semantics::moved_uses_from_flow"));
    assert!(semantic_local_flow_builder.contains("fresh_match_binding"));
    for forbidden in [
        "fn fresh_payload_type_for_variant",
        "fn is_fresh_match_scrutinee",
        "fn hir_expr_ident_name",
    ] {
        assert!(
            !semantic_local_flow_builder.contains(forbidden),
            "compiler must not re-own fresh-match fact rule `{forbidden}`"
        );
    }
    assert!(semantic_local_flow_builder.contains("local_binding_value_facts"));
    for forbidden in [
        "fn hir_expr_is_fresh_value",
        "fn local_binding_source_ident",
        "fn local_binding_handle_field_source",
        "fn local_binding_wrapper_callee",
    ] {
        assert!(
            !semantic_local_flow_builder.contains(forbidden),
            "compiler must not re-own local-binding HIR fact `{forbidden}`"
        );
    }
    for forbidden in ["fn is_copy_type_name", "fn is_cross_isolate_transferable"] {
        assert!(
            !compiler_local_flow.contains(forbidden),
            "compiler must not re-own semantic value property `{forbidden}`"
        );
    }
    assert!(semantic_retained_closure_flow.contains("retained_closure_argument"));
    assert!(semantic_local_flow_builder.contains("retained_closure_argument"));
    assert!(
        !root
            .join("crates/rsscript-compiler/src/checks/local/resources.rs")
            .exists(),
        "compiler must not retain migrated semantic resource traversal"
    );
    let compiler_protocol_rules = read(&root.join("crates/rsscript-compiler/src/analyzer.rs"));
    assert!(
        !compiler_protocol_rules.contains("fn protocol_signature_mismatch"),
        "compiler must not own protocol implementation signature comparison"
    );
    let compiler_protocol_signatures =
        read(&root.join("crates/rsscript-compiler/src/checks/declarations/signatures.rs"));
    assert!(
        compiler_protocol_signatures.contains("rsscript_semantics::protocol_signature_mismatch")
    );
    assert!(
        compiler_protocol_signatures
            .contains("rsscript_semantics::protocol_declaration_diagnostics")
    );
    assert!(compiler_protocol_signatures.contains("rsscript_semantics::protocol_method_names"));
    for forbidden in [
        "fn protocol_method_names",
        "fn function_body_belongs_to_protocol",
        "fn function_belongs_to_protocol",
    ] {
        assert!(
            !compiler_protocol_rules.contains(forbidden),
            "compiler must not own protocol declaration rule `{forbidden}`"
        );
    }
    let compiler_derives = read(&root.join("crates/rsscript-compiler/src/analyzer/derives.rs"));
    assert!(!compiler_derives.contains("fn supported_compiler_derive"));
    assert!(!compiler_derives.contains("fn check_supported_derives"));
    assert!(!compiler_derives.contains("fn check_resource_derives"));
    let semantic_derive_fields = read(&root.join("crates/rsscript-semantics/src/derive_fields.rs"));
    assert!(semantic_derive_fields.contains("pub fn derive_field_diagnostics"));
    assert!(compiler_derives.contains("rsscript-semantics"));
    for forbidden in [
        "fn check_derive_field_requirements",
        "fn collect_local_value_types",
        "fn field_supports_derive",
    ] {
        assert!(
            !compiler_derives.contains(forbidden),
            "compiler must not re-own derive field rule `{forbidden}`"
        );
    }
    let compiler_analyzer = read(&root.join("crates/rsscript-compiler/src/analyzer.rs"));
    assert!(compiler_analyzer.contains("rsscript_semantics::derive_field_diagnostics"));

    let semantic_control_flow = read(&root.join("crates/rsscript-semantics/src/control_flow.rs"));
    assert!(semantic_control_flow.contains("pub fn function_fallthrough_diagnostics"));
    let compiler_calls = read(&root.join("crates/rsscript-compiler/src/checks/calls.rs"));
    assert!(compiler_calls.contains("rsscript_semantics::function_fallthrough_diagnostics"));
    for forbidden in [
        "fn check_function_fallthrough",
        "fn block_may_fall_through",
        "fn statement_may_fall_through",
    ] {
        assert!(
            !compiler_calls.contains(forbidden),
            "compiler must not re-own control-flow rule `{forbidden}`"
        );
    }
    assert!(semantic_control_flow.contains("pub fn non_exhaustive_match_diagnostic"));
    let compiler_exhaustiveness =
        read(&root.join("crates/rsscript-compiler/src/analyzer/exhaustiveness.rs"));
    assert!(
        compiler_exhaustiveness.contains("rsscript_semantics::non_exhaustive_match_diagnostic")
    );
    assert!(
        !compiler_exhaustiveness.contains("Diagnostic::"),
        "compiler must not construct non-exhaustive match diagnostics"
    );

    let semantic_async_lowering =
        read(&root.join("crates/rsscript-semantics/src/await_placement.rs"));
    assert!(semantic_async_lowering.contains("pub fn async_fn_lowering_diagnostic"));
    assert!(semantic_async_lowering.contains("pub fn async_function_cancellation_diagnostics"));
    assert!(semantic_async_lowering.contains("pub fn async_function_lowering_diagnostics"));
    assert!(
        semantic_async_lowering.contains("pub fn cancellation_token_outside_task_group_diagnostic")
    );
    let semantic_task_groups = read(&root.join("crates/rsscript-semantics/src/task_groups.rs"));
    assert!(semantic_task_groups.contains("pub fn task_group_async_let_diagnostics"));
    assert!(
        compiler_analyzer.contains("rsscript_semantics::async_function_cancellation_diagnostics")
    );
    assert!(compiler_analyzer.contains("rsscript_semantics::async_function_lowering_diagnostics"));
    assert!(!compiler_analyzer.contains("fn async_not_lowerable_diagnostic"));
    assert!(!compiler_analyzer.contains("fn cancellation_token_outside_task_group_diagnostic"));
    assert!(!compiler_analyzer.contains("fn block_first_cancellation_token"));
    assert!(!compiler_analyzer.contains("fn expr_first_cancellation_token"));
    assert!(!compiler_analyzer.contains("fn async_block_nonlinear_await"));
    assert!(!compiler_analyzer.contains("fn expr_first_await"));
    assert!(
        !root
            .join("crates/rsscript-compiler/src/analyzer/task_group.rs")
            .exists(),
        "compiler must not retain task-group async-let rule traversal"
    );
    assert!(compiler_syntax_support.contains("rsscript_semantics::item_body_surface_diagnostics"));

    let semantic_call_arguments =
        read(&root.join("crates/rsscript-semantics/src/call_arguments.rs"));
    assert!(semantic_call_arguments.contains("pub fn call_argument_diagnostics"));
    assert!(compiler_calls.contains("rsscript_semantics::call_argument_diagnostics"));
    for forbidden in [
        "fn check_argument_naming",
        "fn check_argument_completeness",
        "fn check_argument_effects",
    ] {
        assert!(
            !compiler_calls.contains(forbidden),
            "compiler must not re-own call argument rule `{forbidden}`"
        );
    }
    assert!(semantic_call_arguments.contains("pub fn receiver_call_effect_diagnostics"));
    assert!(compiler_calls.contains("rsscript_semantics::receiver_call_effect_diagnostics"));
    let compiler_generic_constraints =
        read(&root.join("crates/rsscript-compiler/src/checks/calls/generic_constraints.rs"));
    assert!(
        !compiler_generic_constraints.contains("fn check_receiver_call_self_effect"),
        "compiler must not re-own receiver-call effect diagnostics"
    );
    for exported in [
        "pub fn return_type_mismatch_diagnostic",
        "pub fn return_payload_type_mismatch_diagnostic",
    ] {
        assert!(semantic_call_arguments.contains(exported));
    }
    assert!(compiler_calls.contains("rsscript_semantics::return_type_mismatch_diagnostic"));
    assert!(compiler_calls.contains("rsscript_semantics::return_payload_type_mismatch_diagnostic"));
    assert!(
        !compiler_calls.contains("RSScript return types are part of the review contract"),
        "compiler must not re-own return type diagnostic text"
    );
    let semantic_callbacks = read(&root.join("crates/rsscript-semantics/src/callbacks.rs"));
    for exported in [
        "pub fn callback_operator_type_mismatch_diagnostic",
        "pub fn callback_return_type_mismatch_diagnostic",
        "pub fn callback_fresh_return_not_clean_diagnostic",
        "pub fn callback_fresh_return_unknown_diagnostic",
        "pub fn callback_arity_mismatch_diagnostic",
        "pub fn callback_call_arity_mismatch_diagnostic",
        "pub fn callback_call_argument_type_mismatch_diagnostic",
        "pub fn callback_call_site_argument_type_mismatch_diagnostic",
    ] {
        assert!(semantic_callbacks.contains(exported));
    }
    let compiler_closure_contracts =
        read(&root.join("crates/rsscript-compiler/src/checks/calls/closure_contracts.rs"));
    for delegated in [
        "rsscript_semantics::callback_operator_type_mismatch_diagnostic",
        "rsscript_semantics::callback_return_type_mismatch_diagnostic",
        "rsscript_semantics::callback_fresh_return_not_clean_diagnostic",
        "rsscript_semantics::callback_fresh_return_unknown_diagnostic",
        "rsscript_semantics::callback_arity_mismatch_diagnostic",
        "rsscript_semantics::callback_call_arity_mismatch_diagnostic",
        "rsscript_semantics::callback_call_argument_type_mismatch_diagnostic",
        "rsscript_semantics::callback_call_site_argument_type_mismatch_diagnostic",
        "rsscript_semantics::retained_local_diagnostic",
    ] {
        assert!(compiler_closure_contracts.contains(delegated));
    }
    assert!(
        !compiler_closure_contracts
            .contains("callback parameter counts are part of the call signature"),
        "compiler must not re-own callback contract diagnostic text"
    );
    let semantic_closure_escape =
        read(&root.join("crates/rsscript-semantics/src/closure_escape.rs"));
    assert!(semantic_closure_escape.contains("pub enum ClosureEscapeContext"));
    assert!(semantic_closure_escape.contains("pub fn noescape_escape_diagnostic"));
    assert!(semantic_closure_escape.contains("pub fn local_closure_escape_diagnostic"));
    assert!(
        compiler_closure_contracts
            .contains("rsscript_semantics::ClosureEscapeContext as NoescapeEscapeContext")
    );
    assert!(compiler_closure_contracts.contains("rsscript_semantics::noescape_escape_diagnostic"));
    assert!(
        compiler_closure_contracts.contains("rsscript_semantics::local_closure_escape_diagnostic")
    );
    assert!(
        !compiler_closure_contracts.contains("noescape callback `{name}` cannot be stored"),
        "compiler must not re-own closure escape diagnostic text"
    );
    let semantic_type_compatibility =
        read(&root.join("crates/rsscript-semantics/src/type_compatibility.rs"));
    for exported in [
        "pub fn binding_type_mismatch_diagnostic",
        "pub fn binding_payload_type_mismatch_diagnostic",
        "pub fn argument_payload_type_mismatch_diagnostic",
        "pub fn argument_type_mismatch_diagnostic",
        "pub fn map_literal_entry_type_mismatch_diagnostic",
        "pub fn list_literal_item_type_mismatch_diagnostic",
        "pub fn unknown_callee_diagnostic",
        "pub fn ambiguous_receiver_call_diagnostic",
        "pub fn message_payload_not_transferable_diagnostic",
        "pub fn type_compatible",
        "pub fn contains_unresolved_generic_type",
        "pub fn type_contains_unresolved_generic",
    ] {
        assert!(semantic_type_compatibility.contains(exported));
    }
    for delegated in [
        "rsscript_semantics::binding_type_mismatch_diagnostic",
        "rsscript_semantics::binding_payload_type_mismatch_diagnostic",
        "rsscript_semantics::argument_payload_type_mismatch_diagnostic",
        "rsscript_semantics::argument_type_mismatch_diagnostic",
        "rsscript_semantics::unknown_callee_diagnostic",
        "rsscript_semantics::ambiguous_receiver_call_diagnostic",
        "rsscript_semantics::message_payload_not_transferable_diagnostic",
    ] {
        assert!(compiler_calls.contains(delegated));
    }
    let compiler_type_compatibility =
        read(&root.join("crates/rsscript-compiler/src/checks/calls/type_compatibility.rs"));
    assert!(
        compiler_type_compatibility
            .contains("rsscript_semantics::map_literal_entry_type_mismatch_diagnostic")
    );
    assert!(
        compiler_type_compatibility
            .contains("rsscript_semantics::list_literal_item_type_mismatch_diagnostic")
    );
    assert!(compiler_type_compatibility.contains("rsscript_semantics::type_compatible"));
    for forbidden in [
        "fn argument_type_matches",
        "fn function_type_matches",
        "fn unresolved_generic_type",
        "fn type_contains_unresolved_generic",
    ] {
        assert!(
            !compiler_type_compatibility.contains(forbidden),
            "compiler must not re-own structural type compatibility rule `{forbidden}`"
        );
    }
    assert!(
        compiler_type_compatibility
            .contains("rsscript_semantics::contains_unresolved_generic_type")
    );
    let compiler_assignment = read(&root.join("crates/rsscript-compiler/src/analyzer/assign.rs"));
    assert!(compiler_assignment.contains("rsscript_semantics::contains_unresolved_generic_type"));
    assert!(
        !compiler_assignment.contains("root.len() == 1"),
        "assignment checking must not re-own unresolved-generic inference"
    );
    let semantic_assignment = read(&root.join("crates/rsscript-semantics/src/assignment.rs"));
    for exported in [
        "pub fn invalid_assignment_diagnostic",
        "pub fn local_assignment_type_mismatch_diagnostic",
        "pub fn place_assignment_type_mismatch_diagnostic",
        "pub fn deferred_index_assignment_diagnostic",
    ] {
        assert!(semantic_assignment.contains(exported));
    }
    for delegated in [
        "rsscript_semantics::invalid_assignment_diagnostic",
        "rsscript_semantics::local_assignment_type_mismatch_diagnostic",
        "rsscript_semantics::place_assignment_type_mismatch_diagnostic",
        "rsscript_semantics::deferred_index_assignment_diagnostic",
    ] {
        assert!(compiler_assignment.contains(delegated));
    }
    assert!(
        !compiler_assignment.contains("Diagnostic::"),
        "compiler must not construct assignment language diagnostics"
    );
    assert!(
        !compiler_calls.contains("The callee is not a user function"),
        "compiler must not re-own resolved call diagnostic text"
    );
    let semantic_generic_constraints =
        read(&root.join("crates/rsscript-semantics/src/generic_constraints.rs"));
    for exported in [
        "pub fn protocol_bound_not_satisfied_diagnostic",
        "pub fn dyn_from_diagnostic",
        "pub fn unnamed_variant_field_diagnostic",
        "pub fn unknown_variant_field_diagnostic",
        "pub fn too_many_variant_fields_diagnostic",
        "pub fn duplicate_variant_field_diagnostic",
        "pub fn variant_field_type_mismatch_diagnostic",
        "pub fn missing_variant_field_diagnostic",
        "pub fn conventional_variant_form_diagnostic",
        "pub fn protocol_receiver_not_satisfied_diagnostic",
    ] {
        assert!(semantic_generic_constraints.contains(exported));
    }
    for delegated in [
        "rsscript_semantics::protocol_bound_not_satisfied_diagnostic",
        "rsscript_semantics::dyn_from_diagnostic",
        "rsscript_semantics::unnamed_variant_field_diagnostic",
        "rsscript_semantics::unknown_variant_field_diagnostic",
        "rsscript_semantics::too_many_variant_fields_diagnostic",
        "rsscript_semantics::duplicate_variant_field_diagnostic",
        "rsscript_semantics::variant_field_type_mismatch_diagnostic",
        "rsscript_semantics::missing_variant_field_diagnostic",
        "rsscript_semantics::conventional_variant_form_diagnostic",
        "rsscript_semantics::protocol_receiver_not_satisfied_diagnostic",
    ] {
        assert!(compiler_generic_constraints.contains(delegated));
    }
    assert!(semantic_generic_constraints.contains("pub fn protocol_satisfaction_facts"));
    assert!(semantic_generic_constraints.contains("pub fn type_satisfies_protocol_bound"));
    assert!(semantic_generic_constraints.contains("pub trait SubstitutionBudget"));
    assert!(semantic_generic_constraints.contains("pub fn substitute_type_params"));
    assert!(
        compiler_generic_constraints.contains("rsscript_semantics::protocol_satisfaction_facts")
    );
    assert!(
        compiler_generic_constraints.contains("rsscript_semantics::type_satisfies_protocol_bound")
    );
    for forbidden in [
        "fn type_satisfies_protocol_bound",
        "fn type_derives_protocol",
        "fn builtin_type_is_hashable",
        "fn builtin_type_is_clone",
        "fn substitute_type_params_bounded",
    ] {
        assert!(
            !compiler_generic_constraints.contains(forbidden),
            "compiler must not re-own generic protocol satisfaction rule `{forbidden}`"
        );
    }
    assert!(compiler_generic_constraints.contains("impl rsscript_semantics::SubstitutionBudget"));
    assert!(compiler_generic_constraints.contains("rsscript_semantics::substitute_type_params"));
    assert!(
        !compiler_generic_constraints.contains("external_binding protocol not satisfied"),
        "compiler must not re-own resolved protocol and variant diagnostic text"
    );
    let semantic_operators = read(&root.join("crates/rsscript-semantics/src/operators.rs"));
    assert!(semantic_operators.contains("pub fn builtin_operator_diagnostics"));
    assert!(semantic_operators.contains("pub fn operator_overload_attempt_diagnostic"));
    assert!(semantic_operators.contains("pub fn operator_type_mismatch_diagnostic"));
    let compiler_forbidden = read(&root.join("crates/rsscript-compiler/src/checks/forbidden.rs"));
    assert!(compiler_forbidden.contains("rsscript_semantics::builtin_operator_diagnostics"));
    for forbidden in [
        "fn check_operator_overload_attempts",
        "fn inferred_operand_type",
        "fn is_numeric_type",
        "fn operator_label",
    ] {
        assert!(
            !compiler_forbidden.contains(forbidden),
            "compiler must not re-own builtin operator rule {forbidden}"
        );
    }
    assert!(
        !compiler_forbidden.contains("RSScript does not support user-defined operator overloads"),
        "compiler must not re-own builtin operator diagnostic text"
    );

    let semantic_await_placement =
        read(&root.join("crates/rsscript-semantics/src/await_placement.rs"));
    assert!(semantic_await_placement.contains("pub fn await_placement_diagnostics"));
    let compiler_body = read(&root.join("crates/rsscript-compiler/src/checks/body/mod.rs"));
    assert!(
        !root
            .join("crates/rsscript-compiler/src/checks/body/async_checks.rs")
            .exists(),
        "compiler async diagnostics module must stay removed after semantic migration"
    );
    assert!(compiler_body.contains("rsscript_semantics::await_placement_diagnostics"));
    assert!(semantic_await_placement.contains("pub fn await_operand_diagnostic"));
    let compiler_body_semantics =
        read(&root.join("crates/rsscript-compiler/src/checks/body/semantics.rs"));
    assert!(compiler_body_semantics.contains("rsscript_semantics::await_operand_diagnostic"));
    assert!(semantic_await_placement.contains("pub fn async_call_consumption_diagnostic"));
    assert!(
        compiler_body_semantics.contains("rsscript_semantics::async_call_consumption_diagnostic")
    );
    assert!(semantic_await_placement.contains("pub fn await_live_value_diagnostics"));
    assert!(compiler_body_semantics.contains("rsscript_semantics::await_live_value_diagnostics"));
    let semantic_weak_fields = read(&root.join("crates/rsscript-semantics/src/weak_fields.rs"));
    assert!(semantic_weak_fields.contains("pub fn weak_field_upgrade_diagnostic"));
    assert!(compiler_body_semantics.contains("rsscript_semantics::weak_field_upgrade_diagnostic"));
    let compiler_fresh = read(&root.join("crates/rsscript-compiler/src/checks/body/fresh.rs"));
    for forbidden in [
        "fn weak_field_access_requiring_upgrade",
        "fn weak_field_access_requiring_upgrade_in_stmt",
    ] {
        assert!(
            !compiler_fresh.contains(forbidden),
            "compiler must not re-own weak-field rule `{forbidden}`"
        );
    }
    let semantic_try_checks = read(&root.join("crates/rsscript-semantics/src/try_checks.rs"));
    assert!(semantic_try_checks.contains("pub fn try_operand_diagnostic"));
    assert!(compiler_body_semantics.contains("rsscript_semantics::try_operand_diagnostic"));
    let compiler_try_checks =
        read(&root.join("crates/rsscript-compiler/src/checks/body/try_checks.rs"));
    assert!(
        !compiler_try_checks.contains("fn check_try_value_is_result"),
        "compiler must not re-own try operand diagnostics"
    );
    assert!(semantic_try_checks.contains("pub fn try_error_type_diagnostics"));
    assert!(compiler_body.contains("rsscript_semantics::try_error_type_diagnostics"));
    for forbidden in [
        "fn check_try_error_types",
        "fn check_try_error_types_stmt",
        "fn check_try_error_types_expr",
    ] {
        assert!(
            !compiler_try_checks.contains(forbidden),
            "compiler must not re-own try error compatibility rule `{forbidden}`"
        );
    }

    let semantic_literals = read(&root.join("crates/rsscript-semantics/src/literals.rs"));
    assert!(semantic_literals.contains("pub fn integer_literal_range_diagnostic"));
    assert!(semantic_literals.contains("pub fn char_literal_scalar_diagnostic"));
    assert!(semantic_literals.contains("pub fn match_char_literal_scalar_diagnostic"));
    assert!(
        compiler_body_semantics.contains("rsscript_semantics::integer_literal_range_diagnostic")
    );
    assert!(compiler_body_semantics.contains("rsscript_semantics::char_literal_scalar_diagnostic"));
    assert!(
        compiler_body_semantics
            .contains("rsscript_semantics::match_char_literal_scalar_diagnostic")
    );
    for forbidden in [
        "fn check_integer_literal_range",
        "fn check_char_literal_scalar",
    ] {
        assert!(
            !compiler_body_semantics.contains(forbidden),
            "compiler must not re-own literal validity rule `{forbidden}`"
        );
    }
    assert!(
        !compiler_body_semantics.contains("char_literal_scalar_count"),
        "compiler must not re-own character scalar validation for match patterns"
    );

    let semantic_control_flow = read(&root.join("crates/rsscript-semantics/src/control_flow.rs"));
    assert!(semantic_control_flow.contains("pub fn bool_condition_diagnostic"));
    assert!(compiler_body_semantics.contains("rsscript_semantics::bool_condition_diagnostic"));
    assert!(
        !compiler_body_semantics.contains("fn check_bool_condition"),
        "compiler must not re-own boolean control-flow condition diagnostics"
    );
    assert!(semantic_control_flow.contains("pub fn for_iterable_diagnostic"));
    assert!(compiler_body_semantics.contains("rsscript_semantics::for_iterable_diagnostic"));
    assert!(
        !compiler_body_semantics.contains("fn check_for_iterable_type"),
        "compiler must not re-own for iterable diagnostics"
    );
    assert!(semantic_control_flow.contains("pub fn match_expression_arm_type_diagnostics"));
    assert!(
        compiler_body_semantics
            .contains("rsscript_semantics::match_expression_arm_type_diagnostics")
    );
    for forbidden in [
        "fn check_match_expression_arm_types",
        "fn match_arm_value_type",
    ] {
        assert!(
            !compiler_body_semantics.contains(forbidden),
            "compiler must not re-own match arm result rule `{forbidden}`"
        );
    }
    assert!(semantic_control_flow.contains("pub fn match_scrutinee_diagnostic"));
    assert!(compiler_body_semantics.contains("rsscript_semantics::match_scrutinee_diagnostic"));
    assert!(
        !compiler_body_semantics.contains("match scrutinee has type"),
        "compiler must not re-own match scrutinee diagnostic text"
    );
    assert!(semantic_control_flow.contains("pub fn match_literal_type_diagnostic"));
    assert!(compiler_body_semantics.contains("rsscript_semantics::match_literal_type_diagnostic"));
    assert!(
        !compiler_body_semantics.contains("literal match pattern cannot match"),
        "compiler must not re-own literal match-pattern diagnostics"
    );
    for exported in [
        "pub fn match_pattern_type_diagnostic",
        "pub fn match_variant_family_diagnostic",
        "pub fn variant_pattern_arity_diagnostic",
    ] {
        assert!(semantic_control_flow.contains(exported));
    }
    for delegated in [
        "rsscript_semantics::match_pattern_type_diagnostic",
        "rsscript_semantics::match_variant_family_diagnostic",
        "rsscript_semantics::variant_pattern_arity_diagnostic",
    ] {
        assert!(compiler_body_semantics.contains(delegated));
    }
    for forbidden in [
        "fn push_variant_or_struct_cannot_match",
        "fn push_variant_pattern_arity_mismatch",
        "fn push_match_variant_type_mismatch",
    ] {
        assert!(
            !compiler_body_semantics.contains(forbidden),
            "compiler must not re-own match pattern rule `{forbidden}`"
        );
    }
    assert!(semantic_control_flow.contains("pub fn structured_match_effect_diagnostic"));
    assert!(
        compiler_body_semantics.contains("rsscript_semantics::structured_match_effect_diagnostic")
    );
    assert!(
        !compiler_body_semantics.contains("structured match patterns require an explicit"),
        "compiler must not re-own structured match effect diagnostics"
    );
    assert!(semantic_control_flow.contains("pub fn match_guard_mutation_diagnostic"));
    assert!(
        compiler_body_semantics.contains("rsscript_semantics::match_guard_mutation_diagnostic")
    );
    assert!(
        !compiler_body_semantics.contains("match guard cannot use"),
        "compiler must not re-own match guard mutation diagnostics"
    );
    for exported in [
        "pub fn managed_pattern_field_effect_diagnostic",
        "pub fn weakened_pattern_field_effect_diagnostic",
    ] {
        assert!(semantic_control_flow.contains(exported));
    }
    for delegated in [
        "rsscript_semantics::managed_pattern_field_effect_diagnostic",
        "rsscript_semantics::weakened_pattern_field_effect_diagnostic",
    ] {
        assert!(compiler_body_semantics.contains(delegated));
    }
    for forbidden in [
        "managed pattern field is read-only",
        "field pattern `{}` requests `{}` from a weaker match scrutinee.",
    ] {
        assert!(
            !compiler_body_semantics.contains(forbidden),
            "compiler must not re-own pattern field effect diagnostic `{forbidden}`"
        );
    }
    for exported in [
        "pub fn conflicting_pattern_field_effect_diagnostic",
        "pub fn duplicate_pattern_field_diagnostic",
    ] {
        assert!(semantic_control_flow.contains(exported));
    }
    for delegated in [
        "rsscript_semantics::conflicting_pattern_field_effect_diagnostic",
        "rsscript_semantics::duplicate_pattern_field_diagnostic",
    ] {
        assert!(compiler_body_semantics.contains(delegated));
    }
    for forbidden in ["pattern field conflict", "duplicate pattern field"] {
        assert!(
            !compiler_body_semantics.contains(forbidden),
            "compiler must not re-own duplicate pattern field diagnostic `{forbidden}`"
        );
    }
    for exported in [
        "pub fn unknown_pattern_field_diagnostic",
        "pub fn omitted_pattern_fields_diagnostic",
    ] {
        assert!(semantic_control_flow.contains(exported));
    }
    for delegated in [
        "rsscript_semantics::unknown_pattern_field_diagnostic",
        "rsscript_semantics::omitted_pattern_fields_diagnostic",
    ] {
        assert!(compiler_body_semantics.contains(delegated));
    }
    for forbidden in [
        "Structured match patterns may only project",
        "pattern omits fields",
    ] {
        assert!(
            !compiler_body_semantics.contains(forbidden),
            "compiler must not re-own structured pattern diagnostic `{forbidden}`"
        );
    }

    let semantic_ownership = read(&root.join("crates/rsscript-semantics/src/ownership.rs"));
    for exported in [
        "pub fn moved_use_diagnostic",
        "pub fn managed_to_local_diagnostic",
        "pub fn retained_local_diagnostic",
        "pub fn retained_closure_capture_diagnostic",
        "pub fn take_handle_field_diagnostic",
        "pub fn read_view_mutation_diagnostic",
        "pub fn noescape_consumes_capture_diagnostic",
        "pub fn explicit_closure_missing_capture_diagnostic",
        "pub fn explicit_closure_unused_capture_diagnostic",
        "pub fn explicit_closure_capture_contract_diagnostic",
        "pub fn uninferable_binding_type_diagnostic",
    ] {
        assert!(semantic_ownership.contains(exported));
    }
    let compiler_body_effects =
        read(&root.join("crates/rsscript-compiler/src/checks/body/effects.rs"));
    for delegated in [
        "rsscript_semantics::moved_use_diagnostic",
        "rsscript_semantics::managed_to_local_diagnostic",
        "rsscript_semantics::retained_local_diagnostic",
        "rsscript_semantics::retained_closure_capture_diagnostic",
        "rsscript_semantics::take_handle_field_diagnostic",
        "rsscript_semantics::fresh_return_not_clean_diagnostic",
        "rsscript_semantics::freshness_unknown_diagnostic",
        "rsscript_semantics::invalid_fresh_return_type_diagnostic",
    ] {
        assert!(compiler_body_effects.contains(delegated));
    }
    for forbidden in [
        "fn moved_use_diagnostic",
        "fn managed_to_local_diagnostic",
        "fn retained_local_diagnostic",
        "fn retained_closure_capture_diagnostic",
        "fn take_handle_field_diagnostic",
        "fn fresh_return_diagnostic",
        "fn freshness_unknown_diagnostic",
        "fn invalid_fresh_return_type_diagnostic",
    ] {
        assert!(
            !compiler_body_effects.contains(forbidden),
            "compiler must not re-own local-flow ownership diagnostic `{forbidden}`"
        );
    }
    let compiler_body_binding =
        read(&root.join("crates/rsscript-compiler/src/checks/body/binding.rs"));
    assert!(
        compiler_body_binding.contains("rsscript_semantics::uninferable_binding_type_diagnostic")
    );
    assert!(
        !compiler_body_binding.contains("the type of `{name}` cannot be inferred"),
        "compiler must not re-own uninferable binding diagnostic text"
    );

    let compiler_body_resources =
        read(&root.join("crates/rsscript-compiler/src/checks/body/resources.rs"));
    let compiler_body_closures =
        read(&root.join("crates/rsscript-compiler/src/checks/body/closure_captures.rs"));
    let compiler_body_fresh = read(&root.join("crates/rsscript-compiler/src/checks/body/fresh.rs"));
    for delegated in [
        "rsscript_semantics::managed_closure_local_capture_diagnostic",
        "rsscript_semantics::resource_escape_diagnostic",
        "rsscript_semantics::resource_capture_diagnostic",
        "rsscript_semantics::resource_producer_diagnostics",
    ] {
        assert!(compiler_body_resources.contains(delegated));
    }
    for delegated in [
        "rsscript_semantics::invalid_manage_operand_diagnostic",
        "rsscript_semantics::invalid_take_operand_diagnostic",
        "rsscript_semantics::resource_escape_diagnostic",
    ] {
        assert!(compiler_body_effects.contains(delegated));
    }
    assert!(compiler_body_closures.contains("rsscript_semantics::local_class_binding_diagnostic"));
    assert!(compiler_body_fresh.contains("rsscript_semantics::resource_escape_diagnostic"));
    for delegated in [
        "rsscript_semantics::weak_field_requires_weak_handle_diagnostic",
        "rsscript_semantics::constructor_field_effect_diagnostic",
        "rsscript_semantics::managed_inline_constructor_field_diagnostic",
        "rsscript_semantics::spawn_local_capture_diagnostic",
    ] {
        assert!(compiler_body_fresh.contains(delegated));
    }
    assert!(
        compiler_body_semantics
            .contains("rsscript_semantics::fresh_requires_local_binding_diagnostic")
    );
    assert!(compiler_body_effects.contains("rsscript_semantics::read_view_mutation_diagnostic"));
    assert!(compiler_body_fresh.contains("rsscript_semantics::read_view_mutation_diagnostic"));
    let compiler_body_places =
        read(&root.join("crates/rsscript-compiler/src/checks/body/place.rs"));
    assert!(
        compiler_body_places.contains("rsscript_semantics::noescape_consumes_capture_diagnostic")
    );
    assert!(
        compiler_body_closures
            .contains("rsscript_semantics::explicit_closure_missing_capture_diagnostic")
    );
    assert!(
        compiler_body_closures
            .contains("rsscript_semantics::explicit_closure_unused_capture_diagnostic")
    );
    assert!(
        compiler_body_closures
            .contains("rsscript_semantics::explicit_closure_capture_contract_diagnostic")
    );
    let semantic_places = read(&root.join("crates/rsscript-semantics/src/place.rs"));
    for exported in [
        "pub fn managed_field_split_conflict_diagnostic",
        "pub fn field_partial_access_conflict_diagnostic",
        "pub fn field_prefix_conflict_diagnostic",
        "pub fn indexed_place_conflict_diagnostic",
        "pub fn move_base_field_conflict_diagnostic",
    ] {
        assert!(semantic_places.contains(exported));
    }
    for delegated in [
        "rsscript_semantics::managed_field_split_conflict_diagnostic",
        "rsscript_semantics::field_partial_access_conflict_diagnostic",
        "rsscript_semantics::field_prefix_conflict_diagnostic",
        "rsscript_semantics::indexed_place_conflict_diagnostic",
        "rsscript_semantics::move_base_field_conflict_diagnostic",
    ] {
        assert!(compiler_body_places.contains(delegated));
    }
    for forbidden in [
        "fn resource_escape_diagnostic",
        "fn resource_capture_diagnostic",
        "fn resource_producer_escape_diagnostic",
        "fn local_class_binding_diagnostic",
        "fn invalid_manage_operand_diagnostic",
        "fn invalid_take_operand_diagnostic",
    ] {
        assert!(
            !compiler_body_resources.contains(forbidden),
            "compiler must not re-own resource-boundary diagnostic `{forbidden}`"
        );
    }
    for forbidden in [
        "fn read_view_mutation_diagnostic",
        "fn noescape_consumes_capture_diagnostic",
        "fn explicit_closure_missing_capture_diagnostic",
        "fn explicit_closure_unused_capture_diagnostic",
        "fn explicit_closure_capture_contract_diagnostic",
    ] {
        assert!(
            !compiler_body_effects.contains(forbidden),
            "compiler must not re-own closure or read-view diagnostic `{forbidden}`"
        );
    }
    for forbidden in [
        "fn managed_field_split_conflict_diagnostic",
        "fn field_partial_access_conflict_diagnostic",
        "fn field_prefix_conflict_diagnostic",
        "fn indexed_place_conflict_diagnostic",
        "fn move_base_field_conflict_diagnostic",
    ] {
        assert!(
            !compiler_body_places.contains(forbidden),
            "compiler must not re-own place conflict diagnostic `{forbidden}`"
        );
    }
    for forbidden in [
        "fn fresh_requires_local_binding_diagnostic",
        "fn constructor_field_effect_diagnostic",
        "fn managed_inline_constructor_field_diagnostic",
        "Weak fields are non-owning handles",
        "spawn cannot capture local value",
    ] {
        assert!(
            !compiler_body_fresh.contains(forbidden),
            "compiler must not re-own fresh or constructor diagnostic `{forbidden}`"
        );
    }

    let semantic_generic_constraints =
        read(&root.join("crates/rsscript-semantics/src/generic_constraints.rs"));
    assert!(semantic_generic_constraints.contains("pub fn generic_constraint_diagnostics"));
    let compiler_declaration_checks =
        read(&root.join("crates/rsscript-compiler/src/checks/declarations.rs"));
    assert!(
        compiler_declaration_checks.contains("rsscript_semantics::generic_constraint_diagnostics")
    );
    let compiler_signatures =
        read(&root.join("crates/rsscript-compiler/src/checks/declarations/signatures.rs"));
    assert!(!compiler_signatures.contains("fn check_generic_constraints"));
    let compiler_unknowns = read(&root.join("crates/rsscript-compiler/src/analyzer/unknowns.rs"));
    assert!(!compiler_unknowns.contains("fn check_fresh_generic_return_bound"));
    assert!(!compiler_unknowns.contains("fn check_resource_type_param_field"));

    let semantic_signatures = read(&root.join("crates/rsscript-semantics/src/signatures.rs"));
    assert!(semantic_signatures.contains("pub fn signature_diagnostics"));
    assert!(compiler_declaration_checks.contains("rsscript_semantics::signature_diagnostics"));
    for forbidden in [
        "fn check_signature_explicitness",
        "fn check_return_type_explicit",
        "fn check_retains_parameters",
        "fn invalid_self_parameter_diagnostic",
    ] {
        assert!(
            !compiler_signatures.contains(forbidden),
            "compiler must not re-own `{forbidden}`"
        );
    }

    let semantic_protocol_bounds =
        read(&root.join("crates/rsscript-semantics/src/protocol_bounds.rs"));
    assert!(semantic_protocol_bounds.contains("pub fn protocol_bound_diagnostics"));
    assert!(compiler_signatures.contains("rsscript_semantics::protocol_bound_diagnostics"));
    assert!(!compiler_signatures.contains("fn check_protocol_bound"));

    let semantic_resources = read(&root.join("crates/rsscript-semantics/src/resource_types.rs"));
    for query in [
        "pub fn fd_surface_diagnostics",
        "pub fn resource_field_diagnostics",
        "pub fn resource_generic_diagnostics",
        "pub fn weak_field_diagnostics",
    ] {
        assert!(semantic_resources.contains(query));
    }
    let compiler_resource_types =
        read(&root.join("crates/rsscript-compiler/src/analyzer/resource_types.rs"));
    for forbidden in [
        "fn check_resource_fields",
        "fn check_fd_surface",
        "fn check_weak_fields",
        "fn check_resource_generic_type_ref",
        "fn check_resource_generic_calls_in_block",
        "fn check_resource_generic_calls_in_expr",
    ] {
        assert!(
            !compiler_resource_types.contains(forbidden),
            "compiler must not re-own `{forbidden}`"
        );
    }
    assert!(!compiler_unknowns.contains("unknown_field_diagnostic(&access.name"));

    let semantic_aliases = read(&root.join("crates/rsscript-semantics/src/type_aliases.rs"));
    assert!(semantic_aliases.contains("pub fn cyclic_type_alias_diagnostics"));
    assert!(semantic_aliases.contains("AliasCycleDefinition"));
    let compiler_types = read(&root.join("crates/rsscript-compiler/src/checks/types.rs"));
    assert!(compiler_types.contains("rsscript_semantics::cyclic_type_alias_diagnostics"));
    for forbidden in [
        "fn check_alias_cycles",
        "fn alias_cycle(",
        "AliasCycleDefinition",
    ] {
        assert!(
            !compiler_types.contains(forbidden),
            "compiler type checks must not re-own semantic alias-cycle rule `{forbidden}`"
        );
    }

    let semantic_source_rules = read(&root.join("crates/rsscript-semantics/src/source_rules.rs"));
    assert!(semantic_source_rules.contains("pub fn forbidden_surface_syntax_diagnostics"));
    let semantic_forbidden = read(&root.join("crates/rsscript-semantics/src/checks/forbidden.rs"));
    assert!(semantic_forbidden.contains("forbidden_surface_syntax_diagnostics"));
    assert!(
        !root
            .join("crates/rsscript-compiler/src/checks/forbidden.rs")
            .exists(),
        "compiler must not retain a token-local semantic-rule implementation"
    );

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
        ["crates/rsscript-semantics/src/analyzer.rs"],
        "only the semantic-owned analyzer may assemble checked database parts"
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
fn complete_frontend_checker_is_owned_by_semantics() {
    let root = workspace_root();
    let compiler = root.join("crates/rsscript-compiler/src");
    let semantics = root.join("crates/rsscript-semantics/src");

    for obsolete in [compiler.join("analyzer.rs")] {
        assert!(
            !obsolete.exists(),
            "compiler must not retain frontend-checker implementation at {}",
            obsolete.display()
        );
    }
    assert!(
        rust_files_below(&compiler.join("checks")).is_empty(),
        "compiler must not retain frontend-checker source below checks/"
    );
    for required in [
        semantics.join("analyzer.rs"),
        semantics.join("checks/mod.rs"),
        semantics.join("checks/body/mod.rs"),
        semantics.join("checks/calls.rs"),
        semantics.join("checks/declarations.rs"),
    ] {
        assert!(
            required.is_file(),
            "semantics must own complete frontend-checker source at {}",
            required.display()
        );
    }

    let semantic_lib = read(&semantics.join("lib.rs"));
    assert!(semantic_lib.contains("mod analyzer;"));
    assert!(semantic_lib.contains("mod checks;"));
    assert!(semantic_lib.contains("analyze_frontend_input_snapshot_with_operation"));

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
        ["crates/rsscript-semantics/src/analyzer.rs"],
        "only the semantic-owned analyzer may assemble checked database parts"
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
fn namespace_isolation_and_workspace_hir_are_semantic_queries() {
    let root = workspace_root();
    let semantics = read(&root.join("crates/rsscript-semantics/src/database.rs"));
    for query in [
        "pub fn workspace_hir(",
        "pub fn workspace_hir_with_operation(",
    ] {
        assert!(
            semantics.contains(query),
            "CompilationSession must own the `{query}` workspace semantic query"
        );
    }
    assert!(semantics.contains("workspace_hir_cache"));
    assert!(semantics.contains("crate::isolate_sources_with_interfaces"));

    let isolation = read(&root.join("crates/rsscript-semantics/src/module_isolation.rs"));
    assert!(isolation.contains("pub fn isolate_module_namespaces"));
    assert!(isolation.contains("pub fn isolate_sources_with_interfaces"));
    assert!(
        !root
            .join("crates/rsscript-compiler/src/syntax/module_isolation.rs")
            .exists(),
        "compiler must not retain a second namespace-isolation implementation"
    );
    let analyzer = read(&root.join("crates/rsscript-semantics/src/analyzer.rs"));
    assert!(analyzer.contains("isolate_sources_with_interfaces"));
    assert!(
        rust_files_below(&root.join("crates/rsscript-compiler/src/checks")).is_empty(),
        "compiler must not retain a second semantic-check implementation"
    );
}

#[test]
fn workspace_diagnostic_query_contract_is_semantic_owned() {
    let root = workspace_root();
    let semantics = read(&root.join("crates/rsscript-semantics/src/database.rs"));
    assert!(semantics.contains("pub trait WorkspaceDiagnosticQuery"));
    assert!(semantics.contains("query: &dyn WorkspaceDiagnosticQuery"));

    let language_service = read(&root.join("crates/rsscript-language-service/src/lib.rs"));
    assert!(language_service.contains("Arc<dyn WorkspaceDiagnosticQuery>"));
    assert!(
        !language_service.contains("trait WorkspaceDiagnosticAnalyzer"),
        "language-service must consume the semantic query contract rather than declare a competing one"
    );
}

#[test]
fn compiler_legacy_package_review_and_aot_exports_are_quarantined() {
    let root = workspace_root();
    let compiler = read(&root.join("crates/rsscript-compiler/src/lib.rs"));
    assert!(compiler.contains("pub mod compatibility"));
    for legacy_root_export in [
        "pub use package::{",
        "pub use review::{",
        "pub use rust_lower::{",
    ] {
        assert!(
            !compiler.contains(legacy_root_export),
            "compiler legacy API must be explicit compatibility-only: {legacy_root_export}"
        );
    }
    assert!(compiler.contains("pub use crate::package::{"));
    assert!(compiler.contains("pub use crate::review::{"));
    assert!(compiler.contains("pub use crate::rust_lower::{"));

    let sdk = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    assert!(sdk.contains("pub use rsscript_compiler::compatibility::{"));
    let cli_aot = read(&root.join("crates/rsscript-cli/src/cli/mod.rs"));
    assert!(cli_aot.contains("use rsscript_compiler::compatibility::{"));
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
            .is_none(),
        "language service must not select the transitional compiler adapter"
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
    assert_eq!(
        compiler_manifest["dependencies"]["rsscript-bytecode"]["optional"].as_bool(),
        Some(true),
        "compiler bytecode emission must remain outside the language-service closure"
    );
    assert_eq!(
        compiler_manifest["dependencies"]["rsscript-vm"]["optional"].as_bool(),
        Some(true),
        "the legacy VM is permitted only through the research-only selfhost feature"
    );
    assert!(
        compiler_manifest["features"]["selfhost-parity"]
            .as_array()
            .is_some_and(|features| features
                .iter()
                .any(|feature| feature.as_str() == Some("dep:rsscript-vm"))),
        "the optional VM must be selected only by selfhost-parity"
    );
}

#[test]
fn embedding_facade_exposes_only_product_level_objects() {
    let root = workspace_root();
    let mut source = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    source.push_str(&read(&root.join("crates/rsscript-artifact/src/lib.rs")));
    for object in [
        "pub struct Compiler",
        "pub struct BuiltArtifact",
        "pub struct VerifiedArtifact",
        "pub struct AdmittedArtifact",
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
fn runtime_link_requires_explicit_host_artifact_admission() {
    let root = workspace_root();
    let sdk = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    assert!(
        sdk.contains("artifact: &'artifact AdmittedArtifact"),
        "Runtime::link must only accept host-admitted Artifacts"
    );
    assert!(
        sdk.contains("pub trait ArtifactAdmissionPolicy"),
        "hosts must be able to define non-trusted artifact admission"
    );
    assert!(
        sdk.contains("pub fn admit_trusted_input(self) -> AdmittedArtifact"),
        "trusted input admission must remain explicit in the API name"
    );
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
        "rsscript-mir",
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
    assert!(
        codegen.contains("mir: &VerifiedMir"),
        "MIR codegen must accept only verifier-admitted MIR"
    );
    let lowering = read(&root.join("crates/rsscript-lowering/src/mir.rs"));
    assert!(
        lowering.contains("Result<VerifiedMir, MirLoweringError>"),
        "the checked-HIR lowerer must return verifier-admitted MIR"
    );
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
    let emit_mir = function_source(&adapter, "fn emit_mir");
    assert!(
        compile_mir.contains("emit_mir")
            && emit_mir.contains("rsscript_codegen_vm::emit_artifact")
            && emit_mir.contains("BytecodeVerifier::default")
            && emit_mir.contains("RegVmExecutable::from_verified_bytecode"),
        "SDK MIR compilation must pass through codegen, verifier, then the VM token boundary"
    );

    let sdk = read(&root.join("crates/rsscript-sdk/src/lib.rs"));
    for signature in [
        "pub fn compile_snapshot(",
        "pub fn compile_snapshot_with_operation(",
    ] {
        let build = function_source(&sdk, signature);
        assert!(
            build.contains("compile_validated_to_bytecode"),
            "reviewed SDK build `{signature}` must emit an Artifact through the compiler bytecode boundary"
        );
        assert!(
            !build.contains("vm_adapter") && !build.contains("reg_vm_compile"),
            "reviewed SDK build `{signature}` must not call an SDK/VM compile helper"
        );
    }
    for signature in [
        "pub fn compile(&self, file: &str, source: &str)",
        "pub fn compile_with_interfaces(",
    ] {
        let wrapper = function_source(&sdk, signature);
        assert!(
            wrapper.contains("compile_snapshot"),
            "compatibility SDK compile helper `{signature}` must delegate to the immutable input snapshot"
        );
    }

    let compiler_start = sdk
        .find("impl Compiler {")
        .expect("SDK Compiler implementation must exist");
    let compiler_end = sdk[compiler_start..]
        .find("#[cfg(feature = \"execution\")]\n#[derive(Debug)]\npub struct BuiltArtifact")
        .map(|offset| compiler_start + offset)
        .expect("SDK Compiler implementation must end before BuiltArtifact");
    let compiler = &sdk[compiler_start..compiler_end];
    for forbidden in [
        "WorkspaceSnapshot",
        "load_workspace_snapshot",
        "compile_package",
        "std::path::Path",
    ] {
        assert!(
            !compiler.contains(forbidden),
            "reviewed Compiler must remain an in-memory frontend boundary, not a project loader: `{forbidden}`"
        );
    }
    let project = &sdk[sdk
        .find("pub mod project {")
        .expect("SDK project convenience module must exist")
        ..compiler_start];
    for required in [
        "pub struct ProjectCompiler",
        "pub struct CapturedProjectSnapshot",
        "pub fn capture_frontend_from",
        "pub fn capture_frontend_from_with_operation",
        "pub fn frontend_digest",
        "pub fn build_captured",
        "pub fn build_captured_with_operation",
        "WorkspaceLoader::default()",
        "pub fn compile_package",
    ] {
        assert!(
            project.contains(required),
            "project convenience adapter must own `{required}`"
        );
    }
    assert!(
        project.contains("pub mod legacy")
            && project.contains("pub struct PackageCompatibility")
            && project.contains("load_workspace_snapshot"),
        "legacy package capture must remain isolated behind the compatibility module"
    );
}

#[test]
fn legacy_executable_ir_lowering_is_an_explicit_vm_compatibility_feature() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-vm/Cargo.toml")))
        .expect("VM manifest should parse");
    let exec_ir = manifest["dependencies"]["rsscript-exec-ir"]
        .as_table()
        .expect("legacy executable IR dependency should be declared as a table");
    assert_eq!(
        exec_ir.get("optional").and_then(toml::Value::as_bool),
        Some(true),
        "the default VM closure must not link source-shaped executable IR"
    );
    let legacy_feature = manifest["features"]["legacy-exec-ir"]
        .as_array()
        .expect("legacy executable IR feature should be declared");
    assert!(
        legacy_feature
            .iter()
            .any(|entry| entry.as_str() == Some("dep:rsscript-exec-ir")),
        "only the explicit compatibility feature may activate executable-IR lowering"
    );

    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(
        vm.contains(
            "#[cfg(feature = \"legacy-exec-ir\")]\n#[doc(hidden)]\npub fn compile_executable_ir"
        ) && vm.contains("#[cfg(feature = \"legacy-exec-ir\")]\nmod lower;"),
        "VM source-shaped lowering must remain behind its explicit compatibility feature"
    );
    let bytecode = read(&root.join("crates/rsscript-vm/src/reg_vm/bytecode.rs"));
    assert!(
        bytecode.contains("#[cfg(feature = \"legacy-exec-ir\")]\npub(super) fn encode_and_verify")
            && bytecode.contains(
                "#[cfg(feature = \"legacy-exec-ir\")]\npub(super) fn encode_and_verify_with_imports"
            )
            && bytecode
                .contains("#[cfg(feature = \"legacy-exec-ir\")]\npub(super) fn verify_bytes"),
        "legacy register-unit Artifact encoding and raw-byte verification must not enter the default VM closure"
    );

    let sdk_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-sdk/Cargo.toml")))
            .expect("SDK manifest should parse");
    let execution = sdk_manifest["features"]["execution"]
        .as_array()
        .expect("SDK execution feature should be declared");
    assert!(
        execution
            .iter()
            .all(|entry| entry.as_str() != Some("rsscript-vm/legacy-exec-ir")),
        "reviewed SDK execution must not select legacy lowering"
    );
    let legacy = sdk_manifest["features"]["legacy-exec-ir"]
        .as_array()
        .expect("SDK legacy-exec-ir feature should be declared");
    assert!(
        legacy
            .iter()
            .any(|entry| entry.as_str() == Some("rsscript-vm/legacy-exec-ir")),
        "only the explicit SDK compatibility feature may select legacy lowering"
    );
    let compatibility = sdk_manifest["features"]["compatibility"]
        .as_array()
        .expect("SDK compatibility feature should be declared");
    assert!(
        compatibility
            .iter()
            .any(|entry| entry.as_str() == Some("legacy-exec-ir")),
        "legacy root APIs must opt into the explicit compatibility feature"
    );
}

#[test]
fn default_compiler_lowering_excludes_the_legacy_executable_ir_crate() {
    let root = workspace_root();
    let lowering: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-lowering/Cargo.toml")))
            .expect("lowering manifest should parse");
    assert_eq!(
        lowering["dependencies"]["rsscript-exec-ir"]["optional"].as_bool(),
        Some(true),
        "source-shaped executable IR must be optional in the lowering crate"
    );
    assert!(
        lowering["features"]["default"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "direct checked-HIR MIR lowering must be the lowering crate default"
    );
    assert!(
        lowering["features"]["legacy-exec-ir"]
            .as_array()
            .is_some_and(|items| items
                .iter()
                .any(|item| item.as_str() == Some("dep:rsscript-exec-ir"))),
        "only the explicit lowering compatibility feature may enable executable IR"
    );

    let compiler: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    assert_eq!(
        compiler["dependencies"]["rsscript-lowering"]["default-features"].as_bool(),
        Some(false),
        "compiler must not inherit the lowering compatibility default"
    );
    assert!(
        compiler["features"]["package"]
            .as_array()
            .is_some_and(|items| items
                .iter()
                .any(|item| item.as_str() == Some("rsscript-lowering/legacy-exec-ir"))),
        "only compiler package compatibility may enable executable-IR lowering"
    );
}

#[test]
fn checked_hir_mir_is_the_default_compiler_and_sdk_path() {
    let root = workspace_root();
    let compiler_output = read(&root.join("crates/rsscript-compiler/src/compiler_output.rs"));
    let mir = function_source(&compiler_output, "pub fn mir(&self)");
    assert!(mir.contains("self.checked_hir_mir()"));
    assert!(
        !mir.contains("lower_executable_ir_to_mir"),
        "the default compiler MIR query must not retry the legacy executable-IR bridge"
    );
    assert!(compiler_output.contains("pub fn legacy_executable("));
    assert!(compiler_output.contains("pub fn into_legacy_executable("));

    let adapter = read(&root.join("crates/rsscript-sdk/src/vm_adapter.rs"));
    let emit = function_source(&adapter, "fn emit_ir(compiled: &CompiledIr)");
    assert!(emit.contains("compiled.checked_hir_mir()"));
    assert!(emit.contains("MirLoweringError::Unsupported"));
    assert!(emit.contains("emit_legacy_executable_ir(compiled)"));
    assert!(
        !emit.contains("compiled.mir()"),
        "SDK execution must make the direct-MIR/legacy boundary explicit"
    );
    let artifact = function_source(&adapter, "pub(crate) fn emit_compiled_artifact(");
    assert!(artifact.contains("compiled.checked_hir_mir()"));
    assert!(artifact.contains("emit_legacy_compiled_artifact(compiled, snapshot_digest)"));
}

#[test]
fn vm_public_loader_requires_a_verifier_token() {
    let root = workspace_root();
    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(vm.contains("pub fn from_verified_bytecode("));
    for forbidden in [
        "pub fn from_bytecode(",
        "pub fn from_bytecode_with_operation(",
    ] {
        assert!(
            !vm.contains(forbidden),
            "VM must not expose raw-byte constructor `{forbidden}`"
        );
    }
}

#[test]
fn bytecode_verifier_is_the_only_payload_validation_owner() {
    let root = workspace_root();
    let vm_bytecode = read(&root.join("crates/rsscript-vm/src/reg_vm/bytecode.rs"));
    for duplicate in [
        "fn verify_payload(",
        "fn verify_wire_unit(",
        "fn verify_instruction(",
        "fn verify_register_field(",
    ] {
        assert!(
            !vm_bytecode.contains(duplicate),
            "VM must not restore duplicate bytecode validation `{duplicate}`"
        );
    }
    assert!(vm_bytecode.contains("VerifiedBytecode"));
    assert!(vm_bytecode.contains("decode_executable_payload"));
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
    let selfhost_vm = manifest["dependencies"]["rsscript-vm"]
        .as_table()
        .expect("self-host parity VM dependency must be declared explicitly");
    assert_eq!(
        selfhost_vm.get("optional").and_then(toml::Value::as_bool),
        Some(true),
        "the compiler must not select the VM outside the research-only selfhost feature"
    );

    let package = manifest["features"]["package"]
        .as_array()
        .expect("compiler package feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    let lowering = manifest["features"]["lowering"]
        .as_array()
        .expect("compiler lowering feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    let execution = manifest["features"]["execution"]
        .as_array()
        .expect("compiler execution compatibility feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(execution.contains("package"));
    let selfhost = manifest["features"]["selfhost-parity"]
        .as_array()
        .expect("compiler self-host feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(selfhost.contains("dep:rsscript-vm"));
    assert!(package.contains("bytecode"));
    let bytecode = manifest["features"]["bytecode"]
        .as_array()
        .expect("compiler bytecode feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(bytecode.contains("lowering"));
    for dependency in ["rsscript-bytecode", "rsscript-codegen-vm"] {
        let feature = format!("dep:{dependency}");
        assert!(
            bytecode.contains(feature.as_str()),
            "bytecode feature must explicitly select `{dependency}`"
        );
    }
    for dependency in [
        "rsscript-lowering",
        "rsscript-mir",
        "rsscript-provider-api",
        "sha2",
    ] {
        let feature = format!("dep:{dependency}");
        assert!(
            lowering.contains(feature.as_str()),
            "lowering feature must explicitly select `{dependency}`"
        );
    }
    for dependency in [
        "fs2",
        "hex",
        "libc",
        "rsscript-artifact-store",
        "rustix",
        "sha2",
        "tempfile",
        "toml",
        "uuid",
    ] {
        let specification = manifest["dependencies"][dependency]
            .as_table()
            .unwrap_or_else(|| panic!("compiler must declare `{dependency}`"));
        assert_eq!(
            specification.get("optional").and_then(toml::Value::as_bool),
            Some(true),
            "compiler default closure must not select package dependency `{dependency}`"
        );
        let feature = format!("dep:{dependency}");
        assert!(
            package.contains(feature.as_str()),
            "package feature must explicitly select package dependency `{dependency}`"
        );
    }
    let sdk_execution = facade["features"]["execution"]
        .as_array()
        .expect("SDK execution feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(sdk_execution.contains("rsscript_compiler/bytecode"));
    for removed in [
        "dep:rsscript-codegen-vm",
        "dep:rsscript-lowering",
        "dep:rsscript-mir",
    ] {
        assert!(
            !sdk_execution.contains(removed),
            "reviewed SDK execution must use compiler bytecode rather than `{removed}`"
        );
    }
    assert!(
        !sdk_execution.contains("rsscript_compiler/package"),
        "reviewed in-memory SDK execution must not select compiler package capture"
    );
    let sdk_project = facade["features"]["project"]
        .as_array()
        .expect("SDK project feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(
        !sdk_project.contains("rsscript_compiler/package"),
        "reviewed project capture must remain a loader-to-in-memory-compiler path"
    );
    assert!(
        sdk_project.contains("dep:rsscript-workspace-loader"),
        "project capture must select the dedicated OS workspace loader rather than widening normal execution"
    );
    for removed in [
        "base64",
        "chrono",
        "flate2",
        "hmac",
        "percent-encoding",
        "rand",
        "regex",
        "semver",
        "serde_yaml_ng",
        "sha3",
        "toml_edit",
    ] {
        assert!(
            manifest["dependencies"].get(removed).is_none(),
            "unused compiler package dependency `{removed}` must not widen the frontend closure"
        );
    }
}

#[test]
fn compiler_manifest_does_not_retain_research_or_fuzz_dev_dependencies() {
    let root = workspace_root();
    let compiler_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest");
    let dev_dependencies = compiler_manifest
        .get("dev-dependencies")
        .and_then(toml::Value::as_table);
    assert!(
        dev_dependencies.is_none_or(|dependencies| dependencies.is_empty()),
        "compiler tests must not pull REIR, review adapters, fuzz frameworks, or legacy VM paths into the Core manifest"
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
fn bytecode_backends_cannot_reintroduce_frontend_dependencies() {
    let root = workspace_root();
    let backends = [
        ("VM", root.join("crates/rsscript-vm/src/reg_vm")),
        ("MIR codegen", root.join("crates/rsscript-codegen-vm/src")),
        ("JIT lab", root.join("experiments/vm-jit/src")),
    ];
    let forbidden_source = [
        "rsscript_compiler",
        "rsscript_syntax",
        "rsscript_semantics",
        "rsscript_lowering",
        "crate::hir",
        "crate::syntax",
        "crate::semantic",
        "typed_hir()",
    ];
    for (name, directory) in backends {
        let mut sources = Vec::new();
        collect_rust_sources(&directory, &mut sources);
        for source in sources {
            let contents = read(&source);
            for forbidden in forbidden_source {
                assert!(
                    !contents.contains(forbidden),
                    "{name} backend `{}` must consume MIR/verified bytecode, not frontend `{forbidden}`",
                    source.strip_prefix(&root).unwrap_or(&source).display(),
                );
            }
        }
    }

    let metadata = cargo_metadata(&root);
    for package in ["rsscript-vm", "rsscript-codegen-vm"] {
        let dependencies = metadata_direct_dependencies(&metadata, package);
        for forbidden in [
            "rsscript-compiler",
            "rsscript-syntax",
            "rsscript-semantics",
            "rsscript-lowering",
        ] {
            assert!(
                !dependencies.contains(forbidden),
                "{package} must not depend on frontend package `{forbidden}`"
            );
        }
    }
    let jit_manifest: toml::Value =
        toml::from_str(&read(&root.join("experiments/vm-jit/Cargo.toml")))
            .expect("JIT lab manifest should parse");
    let jit_dependencies = dependency_packages(&jit_manifest);
    for forbidden in [
        "rsscript-compiler",
        "rsscript-syntax",
        "rsscript-semantics",
        "rsscript-lowering",
    ] {
        assert!(
            !jit_dependencies.contains(forbidden),
            "vm-jit must not depend on frontend package `{forbidden}`"
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
        "crates/rsscript-semantics/src/task_groups.rs",
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
