use std::collections::BTreeSet;
use std::process::Command;

use crate::support::*;

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
        "rsscript-jit-cranelift",
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

    let lowering_tree = cargo_tree_with_features(&root, "rsscript-compiler", "lowering");
    for forbidden in [
        "rsscript-artifact-store",
        "rsscript-provider-api",
        "rsscript-review",
        "rsscript-vm",
        "rsscript-workspace-loader",
        "rss-native-abi",
        "rss-process-guard",
        "rsscript-jit-cranelift",
        "fs2",
        "rustix",
        "tempfile",
    ] {
        assert!(
            !lowering_tree
                .lines()
                .any(|line| line.starts_with(forbidden)),
            "the provider-neutral lowering closure must not include `{forbidden}`:\n{lowering_tree}"
        );
    }
}

#[test]
fn reviewed_compiler_closure_excludes_host_and_persistence_adapters() {
    let root = workspace_root();
    let compiler_manifest = read(&root.join("crates/rsscript-compiler/Cargo.toml"));
    let manifest: toml::Value =
        toml::from_str(&compiler_manifest).expect("compiler manifest must remain valid TOML");
    assert!(
        manifest["features"]["default"]
            .as_array()
            .is_some_and(Vec::is_empty),
        "the reviewed compiler must not enable lowering, packages, or host adapters by default"
    );

    let tree = cargo_tree(&root, "rsscript-compiler");
    for forbidden in [
        "rsscript-artifact-store",
        "rsscript-bytecode",
        "rsscript-codegen-vm",
        "rsscript-lowering",
        "rsscript-mir",
        "rsscript-provider-api",
        "rsscript-vm",
        "rsscript-workspace-loader",
        "rss-native-abi",
        "rss-process-guard",
        "rsscript-jit-cranelift",
        "fs2",
        "rustix",
        "tempfile",
    ] {
        assert!(
            !tree.lines().any(|line| line.starts_with(forbidden)),
            "the default compiler dependency closure must not include `{forbidden}`:\n{tree}"
        );
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
        "optional_engines",
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
fn selfhost_parity_is_an_independent_research_package_not_a_release_gate() {
    let root = workspace_root();
    let compiler_manifest = read(&root.join("crates/rsscript-compiler/Cargo.toml"));
    let experiments_manifest = read(&root.join("experiments/Cargo.toml"));
    let selfhost_workflow = read(&root.join(".github/workflows/selfhost.yml"));
    let release_workflow = read(&root.join(".github/workflows/release.yml"));

    assert!(
        !compiler_manifest.contains("selfhost-parity")
            && !compiler_manifest.contains("rsscript-vm"),
        "the Core compiler must not retain the Research harness or a VM dependency"
    );
    assert!(
        experiments_manifest.contains("\"selfhost-parity\""),
        "the Research harness must be an independent experiments workspace member"
    );
    assert!(
        selfhost_workflow.contains("-p rsscript-selfhost-parity"),
        "the dedicated Research workflow must run the independent parity package"
    );
    assert!(
        !release_workflow.contains("selfhost_parity::"),
        "Research parity must not block the supported release path"
    );
}

#[test]
fn sdk_development_closure_does_not_compile_experimental_integrations() {
    let root = workspace_root();
    let manifest = read(&root.join("crates/rsscript-sdk/Cargo.toml"));
    let lockfile = read(&root.join("Cargo.lock"));
    let dev_dependencies = manifest
        .split("[dev-dependencies]")
        .nth(1)
        .and_then(|section| section.split("[[test]]").next())
        .expect("SDK manifest must contain a bounded dev-dependency section");

    assert!(
        !dev_dependencies.contains("../../experiments/"),
        "the SDK test closure must not pull experiments; integration tests belong to the experiments workspace"
    );
    for package in ["name = \"reir\"", "name = \"rsscript-review-reir\""] {
        assert!(
            !lockfile.contains(package),
            "Core lockfile must not retain SDK-only experimental package `{package}`"
        );
    }
}

#[test]
fn reviewed_execution_closures_do_not_resolve_experimental_backends() {
    let root = workspace_root();
    // Core has a separate experiments workspace, but an optional dependency can
    // still accidentally pull a lab back into the normal product path. Check
    // the feature closures that power the supported embedding, CLI, and VM
    // routes rather than merely checking default features or workspace members.
    for (package, features) in [
        ("rsscript-sdk", "execution"),
        ("rsscript-cli", "execution"),
        ("rsscript-vm", ""),
    ] {
        let closure = if features.is_empty() {
            cargo_tree(&root, package)
        } else {
            cargo_tree_with_features(&root, package, features)
        };
        for forbidden in [
            "rsscript-aot-backend ",
            "rsscript-aot-model ",
            "rsscript-aot-runtime ",
            "rsscript-jit-cranelift ",
            "rss-native-abi ",
            "experiments/",
        ] {
            assert!(
                !closure.contains(forbidden),
                "reviewed `{package}` closure ({features:?}) must not resolve experiment `{forbidden}`:\n{closure}"
            );
        }
    }
}

#[test]
fn research_fixtures_are_owned_by_the_experiments_boundary() {
    let root = workspace_root();
    let experiment_manifest = read(&root.join("experiments/Cargo.toml"));
    let root_manifest = read(&root.join("Cargo.toml"));

    for (retired_alias, owned_path) in [
        ("selfhost", "experiments/fixtures/selfhost"),
        (
            "packages/native-abi-fixture",
            "experiments/fixtures/native-abi-fixture",
        ),
    ] {
        assert!(
            !root.join(retired_alias).exists(),
            "retired Research fixture alias `{retired_alias}` must not return at the Core root"
        );
        assert!(
            root.join(owned_path).is_dir(),
            "Research fixture owner `{owned_path}` must exist under experiments"
        );
    }

    assert!(root_manifest.contains("experiments/fixtures/native-abi-fixture/native/rust"));
    assert!(!root_manifest.contains("packages/native-abi-fixture/native/rust"));
    assert!(
        experiment_manifest.contains("fixtures/native-abi-fixture/native/rust"),
        "the experiments workspace must also exclude the native fixture bridge"
    );
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

    let sdk = sdk_source(&root);
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
        "rsscript-compiler ",
        "rsscript-runtime ",
        "rsscript-aot-runtime ",
        "rsscript-bytecode ",
        "rsscript-provider-api ",
        "rss-process-guard ",
        "rsscript-jit-cranelift ",
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
fn selfhost_known_type_sets_are_generated() {
    let root = workspace_root();
    let checker = read(&root.join("experiments/fixtures/selfhost/check.rss"));
    assert!(
        !checker.contains("fn is_builtin_type(") && !checker.contains("fn is_stdlib_type("),
        "self-host type knowledge must come from generated interface metadata"
    );
    let metadata = read(&root.join("experiments/selfhost-parity/src/interface_metadata.rs"));
    assert!(metadata.contains("rsscript_semantics::BUILTIN_TYPE_NAMES"));
    assert!(metadata.contains("for name in &metadata.types"));
}

#[test]
fn selfhost_checker_entry_is_orchestration_only() {
    let root = workspace_root();
    let checker = read(&root.join("experiments/fixtures/selfhost/check.rss"));
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
            "experiments/fixtures/selfhost/checker/support.rss",
            "module selfhost.checker.support",
        ),
        (
            "experiments/fixtures/selfhost/checker/output.rss",
            "module selfhost.checker.output",
        ),
        (
            "experiments/fixtures/selfhost/checker/type_model.rss",
            "module selfhost.checker.type_model",
        ),
        (
            "experiments/fixtures/selfhost/checker/diagnostics/syntax_declarations.rss",
            "module selfhost.checker.diagnostics.syntax_declarations",
        ),
        (
            "experiments/fixtures/selfhost/checker/diagnostics/effects_calls.rss",
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
fn jit_planning_state_is_kept_out_of_verified_program_objects() {
    let root = workspace_root();
    let tier = read(&root.join("crates/rsscript-vm/src/reg_vm/tier.rs"));
    let state = read(&root.join("crates/rsscript-vm/src/reg_vm/tier/state.rs"));
    let model = read(&root.join("crates/rsscript-vm/src/reg_vm/model.rs"));
    let bytecode = read(&root.join("crates/rsscript-vm/src/reg_vm/bytecode.rs"));
    let exec = read(&root.join("crates/rsscript-vm/src/reg_vm/exec.rs"));
    let deopt = read(&root.join("crates/rsscript-vm/src/reg_vm/tier/deopt_resume.rs"));
    let entry = read(&root.join("crates/rsscript-vm/src/reg_vm/tier/jit_entry.rs"));

    assert!(tier.contains("mod state;"));
    for required in ["struct JitState", "functions: Vec<JitFunctionState>"] {
        assert!(
            state.contains(required),
            "JIT state side table must retain `{required}`"
        );
    }
    for retired in [
        "struct VerifiedProgramIdentity",
        "from_executable_digest",
        "ordinal_by_function_pointer",
    ] {
        assert!(
            !state.contains(retired),
            "retired JIT identity bridge must not return: `{retired}`"
        );
    }
    assert!(model.contains("pub(crate) ordinal: usize"));
    assert!(bytecode.contains(".enumerate()"));
    for forbidden in [
        "\n    jit_analysis:",
        "\n    jit_self_recursion_kind:",
        "\n    native_status:",
        "\n    call_count:",
        "\n    branch_count:",
        "\n    profile:",
        "\n    osr_state:",
    ] {
        assert!(
            !model.contains(forbidden),
            "verified VM program model must not retain JIT planning field `{forbidden}`"
        );
        assert!(
            !bytecode.contains(forbidden),
            "bytecode decoder must not initialize JIT planning field `{forbidden}`"
        );
    }
    assert!(exec.contains("JitState::for_verified_program"));
    assert!(exec.contains("self.jit_state.tier0_analysis"));
    assert!(state.contains("native_status: u8"));
    assert!(state.contains("call_count: u32"));
    assert!(state.contains("branch_count: u32"));
    assert!(state.contains("profile: Option<Box<FunctionProfile>>"));
    assert!(!state.contains("BTreeMap<JitFunctionKey"));
    assert!(!state.contains(".clone();\n        self.functions"));
    for (name, source) in [
        ("interpreter", exec),
        ("native tier", tier),
        ("deopt", deopt),
        ("native entry", entry),
    ] {
        assert!(
            !source.contains("as *const RegFunction"),
            "{name} must address native state through JitState ordinals, not RegFunction pointers"
        );
    }
}

#[test]
fn verified_jit_facts_are_native_only_evaluation_local_state() {
    let root = workspace_root();
    let vm = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs")),
        read(&root.join("crates/rsscript-vm/src/reg_vm/state.rs"))
    );
    let native = read(&root.join("crates/rsscript-vm/src/reg_vm/native/mod.rs"));
    let facts_path = root.join("crates/rsscript-vm/src/reg_vm/native/facts.rs");
    let facts = read(&facts_path);
    let model = read(&root.join("crates/rsscript-vm/src/reg_vm/model.rs"));
    let bytecode = read(&root.join("crates/rsscript-vm/src/reg_vm/bytecode.rs"));

    assert!(vm.contains("#[cfg(feature = \"native-jit\")]\nmod native;"));
    assert!(
        vm.contains("verified_facts: std::cell::OnceCell"),
        "verified facts must be derived lazily once per verified executable"
    );
    assert!(vm.contains("native_param_shape_with_fact"));
    assert!(vm.contains("NativeParamShape::StaticScalar"));
    assert!(native.contains("mod facts;"));
    for required in [
        "struct VerifiedExecutableFacts",
        "struct VerifiedFunctionFacts",
        "struct VerifiedFactsLimits",
        "enum VerifiedStorageType",
        "BytecodeLimits::default()",
        "fn derive(",
    ] {
        assert!(
            facts.contains(required),
            "the bounded native facts projection must expose `{required}`"
        );
    }
    assert!(
        facts.contains("RegUnit"),
        "native facts must be derived from the decoded, verified executable"
    );
    for serialized_model in [
        ("verified VM model", model),
        ("v1 bytecode model", bytecode),
    ] {
        assert!(
            !serialized_model.1.contains("VerifiedExecutableFacts")
                && !serialized_model.1.contains("VerifiedFunctionFacts")
                && !serialized_model.1.contains("VerifiedStorageType"),
            "{} must not persist evaluation-local native facts",
            serialized_model.0
        );
    }
    for serialization_marker in [
        "#[derive(Serialize",
        "#[derive(Deserialize",
        "serde::Serialize",
        "serde::Deserialize",
        "#[serde(",
    ] {
        assert!(
            !facts.contains(serialization_marker),
            "evaluation-local native facts must not acquire serialized contract marker `{serialization_marker}`"
        );
    }
}

#[test]
fn native_facts_do_not_pull_frontend_layers_into_the_jit() {
    let root = workspace_root();
    let native_closure = cargo_tree_with_features(&root, "rsscript-vm", "native-jit");
    for forbidden in [
        "rsscript-syntax ",
        "rsscript-semantics ",
        "rsscript-compiler ",
        "rsscript-sdk ",
    ] {
        assert!(
            !native_closure.contains(forbidden),
            "verified executable facts must not make the VM/JIT depend on frontend layer `{forbidden}`:\n{native_closure}"
        );
    }
}

#[test]
fn jit_translation_consumes_verified_facts_without_multiplying_inference_engines() {
    let root = workspace_root();
    let translate = read(&root.join("crates/rsscript-vm/src/reg_vm/native/translate.rs"));
    let native_sources = rust_files_below(&root.join("crates/rsscript-vm/src/reg_vm/native"));
    let all_native = native_sources
        .iter()
        .map(|path| read(path))
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        translate.contains("VerifiedFunctionFacts")
            || translate.contains("VerifiedExecutableFacts"),
        "whole-function, OSR, and continuation translation must consume the shared verified facts projection"
    );
    assert!(
        translate.contains("facts.native_type_seed(n_regs, false)?"),
        "ordinary whole-function scalar translation must seed storage types from verified facts"
    );
    assert!(
        translate.contains("facts: &VerifiedFunctionFacts"),
        "known-call translation must receive the verified call-site/type projection explicitly"
    );
    for inference_definition in ["fn native_set_ty(", "fn native_unify("] {
        assert!(
            all_native.matches(inference_definition).count() <= 1,
            "native translation must not grow another ad-hoc inference engine `{inference_definition}`"
        );
    }
}

#[test]
fn jit_static_facts_and_missed_optimization_telemetry_stay_structured() {
    let root = workspace_root();
    let vm = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs")),
        read(&root.join("crates/rsscript-vm/src/reg_vm/state.rs"))
    );
    let facts = read(&root.join("crates/rsscript-vm/src/reg_vm/native/facts.rs"));

    for field in [
        "verified_known_reg_types",
        "verified_unknown_reg_types",
        "verified_known_call_sites",
        "verified_instruction_effects",
        "interpreted_native_work",
        "native_barrier_counts",
        "native_decline_reasons",
        "shape_versions",
        "native_call_edges",
        "direct_list_bounds_checks_elided",
        "memoized_runtime_helper_call_sites",
        "runtime_helper_call_sites",
        "canonical_loops",
        "canonical_induction_variables",
    ] {
        assert!(
            vm.contains(field),
            "native evidence must retain structured telemetry field `{field}`"
        );
    }
    for summary_field in [
        "known_reg_types",
        "unknown_reg_types",
        "known_call_sites",
        "instruction_effects",
    ] {
        assert!(
            facts.contains(summary_field),
            "verified facts summary must expose `{summary_field}`"
        );
    }
    assert!(vm.contains("fn jit_missed_opt_report("));
    assert!(vm.contains("native_decline_reason_counts("));
    assert!(vm.contains("verified_facts"));
    assert!(vm.contains("native execution installs verified facts"));
    assert!(vm.contains("pub fn to_json(&self)"));
}

#[test]
fn native_execution_evidence_covers_compact_windows_steps_and_direct_calls() {
    let root = workspace_root();
    let regions =
        read(&root.join("crates/rsscript-vm/src/reg_vm/native/translate/loop_regions.rs"));
    let tier = read(&root.join("crates/rsscript-vm/src/reg_vm/tier.rs"));
    let compile_result = read(&root.join("crates/rsscript-vm/src/reg_vm/tier/compile_result.rs"));
    let jit_ir = read(&root.join("crates/rsscript-jit-cranelift/src/ir.rs"));
    let jit_module = read(&root.join("crates/rsscript-jit-cranelift/src/module.rs"));

    assert!(regions.contains("struct ContinuationSlot"));
    assert!(regions.contains("live_in_regs"));
    assert!(regions.contains("live_slots"));
    assert!(jit_ir.contains("compact_registers("));

    // Batched accounting is allowed only at conservative CFG/deopt segments;
    // an insufficient reservation must return at the first unpaid source IP.
    assert!(tier.contains("emit_step"));
    assert!(jit_module.contains("step_limit"));
    let codegen = read(&root.join("crates/rsscript-jit-cranelift/src/codegen.rs"));
    assert!(codegen.contains("fn step_segment_costs("));
    assert!(codegen.contains("first uncharged source instruction"));

    assert!(jit_ir.contains("CallNative"));
    assert!(codegen.contains("build_child_call_frame("));
    assert!(compile_result.contains("native_call_edges"));
    assert!(compile_result.contains("JitInstr::CallNative"));
}

#[test]
fn loop_optimizations_share_canonical_facts_and_keep_unrolling_research_only() {
    let root = workspace_root();
    let loops = read(&root.join("crates/rsscript-vm/src/reg_vm/native/translate/loop_regions.rs"));
    let jit_post = read(&root.join("crates/rsscript-vm/src/reg_vm/native/translate/jit_post.rs"));
    let contract = read(&root.join("docs/spec/native-jit-contract.md"));

    for fact in [
        "struct CanonicalLoopFacts",
        "preheader: Option<usize>",
        "condition: usize",
        "latches: Box<[usize]>",
        "exits: Box<[usize]>",
        "induction: Option<CanonicalInductionVariable>",
    ] {
        assert!(
            loops.contains(fact),
            "canonical loop fact `{fact}` is missing"
        );
    }
    assert!(jit_post.contains("detect_canonical_loops(code)"));
    assert!(jit_post.contains("facts: &CanonicalLoopFacts"));
    assert!(jit_post.contains("fn native_readonly_licm_eligible("));
    assert!(jit_post.contains("helper.heap_reads()"));
    assert!(loops.contains("#[cfg(all(test, feature = \"native-jit\"))]"));
    assert!(loops.contains("fn scalar_x2_unroll_research_decision("));
    assert!(loops.contains("fn simd_research_decision("));
    assert!(!loops.contains("fn native_unroll_scalar_loop_x2("));
    assert!(contract.contains("Scalar x2 unrolling is not enabled"));
    assert!(contract.contains("SIMD remains out of scope"));

    let backend_analysis = read(&root.join("crates/rsscript-jit-cranelift/src/analysis.rs"));
    assert!(backend_analysis.contains("fn add_canonical_induction_bounds("));
    assert!(backend_analysis.contains("WORK_PER_INSTRUCTION"));
    assert!(backend_analysis.contains("MAX_WORK"));
}

#[test]
fn jit_profiles_only_genuinely_dynamic_targets_and_branch_bias() {
    let root = workspace_root();
    let profile = read(&root.join("crates/rsscript-vm/src/reg_vm/model/profile.rs"));
    let inlining = read(&root.join("crates/rsscript-vm/src/reg_vm/native/passes/inlining.rs"));
    let state = read(&root.join("crates/rsscript-vm/src/reg_vm/tier/state.rs"));
    let exec = read(&root.join("crates/rsscript-vm/src/reg_vm/exec.rs"));

    assert!(profile.contains("call_sites: HashMap<usize, CallSiteFeedback>"));
    assert!(profile.contains("branch_sites: HashMap<usize, BranchFeedback>"));
    for forbidden_static_fact in ["reg_types:", "layout_types:", "field_types:"] {
        assert!(
            !profile.contains(forbidden_static_fact),
            "runtime profile must not rediscover static fact `{forbidden_static_fact}`"
        );
    }
    assert!(inlining.contains("#[cfg(feature = \"jit-speculation\")]"));
    assert!(inlining.contains("NativeGuardClosureId"));
    assert!(state.contains("#[cfg(feature = \"jit-speculation\")]\n    branch_count: u32"));
    assert!(state.contains("pub(crate) fn should_record_call"));
    assert!(!exec.contains("record_native_branch_feedback"));
}

#[test]
fn generic_jit_instances_flow_from_semantic_substitutions_not_runtime_guessing() {
    let root = workspace_root();
    let inference = read(&root.join("crates/rsscript-semantics/src/hir/infer.rs"));
    let hir = read(&root.join("crates/rsscript-semantics/src/hir/mod.rs"));
    let mir = read(&root.join("crates/rsscript-mir/src/lib.rs"));
    let lowering = read(&root.join("crates/rsscript-lowering/src/mir.rs"));
    let codegen = read(&root.join("crates/rsscript-codegen-vm/src/lib.rs"));
    let verifier = read(&root.join("crates/rsscript-bytecode/src/typed_facts.rs"));
    let vm_facts = read(&root.join("crates/rsscript-vm/src/reg_vm/native/facts.rs"));
    let tier = read(&root.join("crates/rsscript-vm/src/reg_vm/tier.rs"));

    assert!(inference.contains("fn infer_call_type_arguments("));
    assert!(hir.contains("type_arguments: Vec<ResolvedType>"));
    assert!(mir.contains("FunctionInstance"));
    assert!(mir.contains("type_substitutions: Box<[(TypeId, TypeId)]>"));
    assert!(lowering.contains("checked_type_to_wire(ty, &self.function_name)"));
    assert!(codegen.contains("TYPED_EXECUTABLE_FACTS_SCHEMA_V2"));
    assert!(verifier.contains("type_parameters.len() != call.type_arguments.len()"));
    assert!(vm_facts.contains("fn instantiate_parameter_storage("));
    assert!(tier.contains("JitInstanceKey::from_call_site(callee_key, call)"));
}

#[test]
fn aot_jit_matrix_schema_requires_honest_per_engine_evidence() {
    let root = workspace_root();
    let schema: serde_json::Value = serde_json::from_str(&read(
        &root.join("benchmarks/vm-jit/aot-jit-matrix.schema.json"),
    ))
    .expect("AOT/JIT matrix schema is valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("AOT/JIT matrix schema compiles");
    let unavailable = serde_json::json!({
        "status": "not_measured",
        "execution_ns": null,
        "compile_ns": null,
        "transitions": null,
        "host_helper_calls": null,
        "bounds_checks": null,
        "allocations_eliminated": null,
        "reason": "engine is outside this harness"
    });
    let document = serde_json::json!({
        "schema": "rsscript.aot_jit_matrix.v1",
        "workload": "static_calls",
        "semantic_match": true,
        "engines": {
            "interpreter": {
                "status": "measured",
                "execution_ns": 100,
                "compile_ns": 0,
                "transitions": 0,
                "host_helper_calls": null,
                "bounds_checks": null,
                "allocations_eliminated": null,
                "reason": null
            },
            "jit": {
                "status": "measured",
                "execution_ns": 50,
                "compile_ns": 20,
                "transitions": 1,
                "host_helper_calls": 0,
                "bounds_checks": null,
                "allocations_eliminated": null,
                "reason": null
            },
            "aot": unavailable
        }
    });
    assert!(validator.is_valid(&document));

    let mut incomplete = document;
    incomplete["engines"]
        .as_object_mut()
        .expect("engines object")
        .remove("aot");
    assert!(
        !validator.is_valid(&incomplete),
        "the matrix must not silently omit an unmeasured engine"
    );
}

#[test]
fn continuation_register_windows_remain_compact_and_liveness_driven() {
    let root = workspace_root();
    let regions =
        read(&root.join("crates/rsscript-vm/src/reg_vm/native/translate/loop_regions.rs"));
    let jit_ir = read(&root.join("crates/rsscript-jit-cranelift/src/ir.rs"));

    for required in [
        "struct ContinuationSlot",
        "live_in_regs",
        "live_slots",
        "ordered_vm_regs",
        "compact_registers(",
    ] {
        assert!(
            regions.contains(required) || jit_ir.contains(required),
            "continuation compact-window invariant is missing `{required}`"
        );
    }
    assert!(regions.contains("region.live_in_regs.len()"));
    assert!(jit_ir.contains("n_live_in"));
}

#[test]
fn typed_facts_migration_preserves_the_v1_reader_contract() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-sdk/Cargo.toml")))
        .expect("SDK manifest should parse");
    let compatibility = read(&root.join("crates/rsscript-sdk/tests/compatibility_corpus.rs"));
    let bytecode = read(&root.join("crates/rsscript-bytecode/src/lib.rs"));
    let typed_facts = read(&root.join("crates/rsscript-bytecode/src/typed_facts.rs"));
    let fuzz_manifest = read(&root.join("fuzz/Cargo.toml"));
    let hardening = read(&root.join(".github/workflows/jit-hardening.yml"));
    let fixture = root.join("crates/rsscript-bytecode/fixtures/v1/reference.rssbundle.base64");
    let tests = manifest["test"]
        .as_array()
        .expect("SDK must declare explicit test targets");
    let target = tests
        .iter()
        .find(|test| test["name"].as_str() == Some("compatibility_corpus"))
        .expect("v1 reader compatibility corpus must remain an explicit SDK target");

    assert_eq!(target["required-features"][0].as_str(), Some("execution"));
    assert!(
        fixture.is_file(),
        "the deployed v1 reader fixture must remain checked in"
    );
    assert!(compatibility.contains("deployed v1 bundle remains readable"));
    assert!(compatibility.contains("ArtifactVerifier"));
    assert!(bytecode.contains("pub const BYTECODE_SCHEMA: &str = \"rsscript.bytecode.v1\";"));
    for required in [
        "TypedExecutableFactsVerifierV1",
        "BoundTypedExecutableFactsV1",
        "TYPED_EXECUTABLE_FACTS_SCHEMA_V1",
        "TypedFactsBindingMismatch",
    ] {
        assert!(
            typed_facts.contains(required) || bytecode.contains(required),
            "optional typed facts contract is missing `{required}`"
        );
    }
    assert!(fuzz_manifest.contains("name = \"typed_executable_facts\""));
    assert!(hardening.contains("fuzz run typed_executable_facts"));
}

#[test]
fn default_vm_layout_excludes_native_jit_while_opt_in_closure_is_explicit() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-vm/Cargo.toml")))
        .expect("VM manifest should parse");
    assert_eq!(
        manifest["dependencies"]["vm-jit"]["optional"].as_bool(),
        Some(true),
        "the native backend must remain an optional dependency"
    );
    let native_feature = manifest["features"]["native-jit"]
        .as_array()
        .expect("native-jit feature should be explicit");
    assert!(
        native_feature
            .iter()
            .any(|entry| entry.as_str() == Some("dep:vm-jit"))
    );

    let closure = cargo_tree(&root, "rsscript-vm");
    assert!(
        !closure.contains("rsscript-jit-cranelift "),
        "default VM dependency closure must not include the optional native engine:\n{closure}"
    );
    let native_closure = cargo_tree_with_features(&root, "rsscript-vm", "native-jit");
    assert!(
        native_closure.contains("rsscript-jit-cranelift "),
        "the explicit native-jit feature must resolve the reviewed backend:\n{native_closure}"
    );
}

#[test]
fn builtin_registry_is_versioned_and_keeps_library_calls_out_of_provider_dispatch() {
    let root = workspace_root();
    let build_support = read(&root.join("crates/rsscript-build-support/src/lib.rs"));
    let mir = read(&root.join("crates/rsscript-mir/src/lib.rs"));
    let codegen = read(&root.join("crates/rsscript-codegen-vm/src/lib.rs"));
    let intrinsics = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/mod.rs"));
    let calls = read(&root.join("crates/rsscript-vm/src/reg_vm/calls.rs"));

    for required in [
        "BUILTIN_REGISTRY_SCHEMA",
        "BUILTIN_REGISTRY_DIGEST",
        "struct BuiltinDescriptor",
        "enum BuiltinDeterminism",
        "enum BuiltinCost",
        "enum BuiltinClass",
        "enum BuiltinSignatureSource",
        "pub fn builtin_descriptor",
    ] {
        assert!(
            mir.contains(required) || build_support.contains(required),
            "versioned builtin registry is missing `{required}`"
        );
    }
    assert!(build_support.contains("collect_interface_function_signatures"));
    assert!(build_support.contains("duplicate standard interface declaration"));
    assert!(build_support.contains("intrinsic catalog contains duplicate binding"));
    assert!(codegen.contains("builtin_descriptor(*id)"));
    assert!(intrinsics.contains("self.charge_intrinsic_call()?"));
    assert!(calls.contains("self.charge_provider_call()?"));
}

#[test]
fn deterministic_core_library_is_pure_and_the_vm_only_adapts_its_results() {
    let root = workspace_root();
    let corelib_manifest = read(&root.join("crates/rsscript-corelib/Cargo.toml"));
    let corelib = read(&root.join("crates/rsscript-corelib/src/lib.rs"));
    let vm_manifest = read(&root.join("crates/rsscript-vm/Cargo.toml"));
    let vm = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs")),
        read(&root.join("crates/rsscript-vm/src/reg_vm/state.rs"))
    );
    let intrinsics = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/mod.rs"));
    let hex = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/hex.rs"));
    let url = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/url.rs"));
    let list = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/list.rs"));
    let deque = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/deque.rs"));
    let set = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/set.rs"));
    let regex = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/regex.rs"));
    let date = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/date.rs"));
    let value_access = read(&root.join("crates/rsscript-vm/src/reg_vm/value_access.rs"));

    assert!(corelib_manifest.contains("name = \"rsscript-corelib\""));
    for forbidden in ["rsscript-vm", "rsscript-provider", "rsscript-bytecode"] {
        assert!(
            !corelib_manifest.contains(forbidden),
            "pure core library must not depend on `{forbidden}`"
        );
    }
    for required in [
        "pub fn base64_decode",
        "pub fn base64_encode",
        "pub fn hex_decode",
        "pub fn hex_encode",
        "pub fn url_decode_component",
        "pub fn url_encode_component",
        "pub fn dedup",
        "pub fn reverse",
        "pub fn skip",
        "pub fn take",
        "pub fn slice",
        "pub fn enumerate",
        "pub fn zip",
        "pub fn deque_to_vec",
        "pub fn map_difference",
        "pub fn map_intersection",
        "pub fn map_union",
        "pub fn map_is_subset",
        "pub fn map_keys",
        "pub fn map_values",
        "pub struct CompiledRegex",
        "pub fn captures",
        "pub fn replace_all",
        "pub fn format_iso",
        "pub fn parse_iso",
        "pub fn start_of_day",
        "pub fn sha256_hex",
        "pub fn sha3_224",
        "pub fn sha3_256",
        "pub fn shake128",
        "pub fn hmac_sha256_hex",
        "pub fn gzip_decompress",
        "pub fn yaml_to_json",
    ] {
        assert!(
            corelib.contains(required),
            "core library is missing `{required}`"
        );
    }
    assert!(vm_manifest.contains("rsscript-corelib"));
    let vm_runtime_dependencies = vm_manifest
        .split("[build-dependencies]")
        .next()
        .expect("VM manifest has a package dependency section");
    for removed in [
        "base64 =",
        "hex =",
        "percent-encoding =",
        "regex =",
        "chrono =",
        "sha2 =",
        "sha3 =",
        "hmac =",
        "flate2 =",
        "serde_yaml_ng =",
    ] {
        assert!(
            !vm_runtime_dependencies.contains(removed),
            "VM manifest must not directly own encoding implementation dependency `{removed}`"
        );
    }
    assert!(vm.contains("use rsscript_corelib::{"));
    assert!(vm.contains("encoding::{"));
    assert!(vm.contains("collections::{"));
    assert!(intrinsics.contains("base64_decode(text)"));
    assert!(intrinsics.contains("core_sha256_hex(value)"));
    assert!(intrinsics.contains("core_hmac_sha256_hex(key, value)"));
    assert!(intrinsics.contains("core_gzip_decompress(value)"));
    assert!(vm.contains("structured_data::yaml_to_json as core_yaml_to_json"));
    assert!(hex.contains("core_hex_decode(text)"));
    assert!(url.contains("url_decode_component(value)"));
    assert!(regex.contains("CompiledRegex::compile(pattern)"));
    assert!(value_access.contains("Result<CompiledRegex, EvalError>"));
    assert!(date.contains("core_date_format_iso(unix_ms)"));
    assert!(date.contains("core_date_start_of_day(unix_ms)"));
    for required in [
        "core_list_dedup(list.borrow().iter())",
        "core_list_reverse(list.borrow().iter())",
        "core_list_skip(list.borrow().iter(), count)",
        "core_list_slice(list.borrow().iter(), start, len)",
        "core_list_take(list.borrow().iter(), count)",
        "core_list_enumerate(list.borrow().iter())",
        "core_list_zip(left.iter(), right.iter())",
    ] {
        assert!(
            list.contains(required),
            "list adapter is missing `{required}`"
        );
    }
    assert!(deque.contains("core_deque_to_vec(&deque.borrow())"));
    for required in [
        "core_map_difference(&left.borrow(), &right.borrow())",
        "core_map_intersection(&left.borrow(), &right.borrow())",
        "core_map_union(&left.borrow(), &right.borrow())",
        "core_map_is_subset(",
        "core_map_keys(&set.borrow())",
    ] {
        assert!(
            set.contains(required),
            "set adapter is missing `{required}`"
        );
    }
    let map = read(&root.join("crates/rsscript-vm/src/reg_vm/intrinsics/map.rs"));
    for required in [
        "core_map_keys(&map.borrow())",
        "core_map_values(&map.borrow())",
    ] {
        assert!(
            map.contains(required),
            "map adapter is missing `{required}`"
        );
    }
}

#[test]
fn vm_runtime_dependency_inventory_prevents_library_implementation_regressions() {
    let root = workspace_root();
    let vm_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-vm/Cargo.toml")))
            .expect("VM manifest should parse");
    let inventory = read(&root.join("docs/architecture/vm-runtime-dependency-inventory.md"));
    let declared = normal_dependency_packages(&vm_manifest);
    let expected = BTreeSet::from_iter([
        "rsscript-abi-model".to_owned(),
        "rsscript-bytecode".to_owned(),
        "rsscript-corelib".to_owned(),
        "rsscript-diagnostics".to_owned(),
        "rsscript-operation".to_owned(),
        "rsscript-provider-api".to_owned(),
        "rsscript-text".to_owned(),
        "serde".to_owned(),
        "rsscript-jit-cranelift".to_owned(),
    ]);
    assert_eq!(
        declared, expected,
        "new VM runtime dependencies require an explicit inventory/ownership review"
    );
    for required in [
        "rsscript-corelib",
        "legacy JSON adapter",
        "P06.2/P06.4",
        "must not directly add algorithm crates",
    ] {
        assert!(
            inventory.contains(required),
            "VM dependency inventory is missing `{required}`"
        );
    }
}

#[test]
fn register_vm_execution_policy_is_snapshotted_before_running() {
    let root = workspace_root();
    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(vm.contains("mod execution_plan;"));
    assert!(vm.contains("NativeExecutionPlan::for_diagnostics("));
    assert!(!vm.contains("std::env::var_os(\"RSS_JIT_"));
    assert!(!vm.contains("std::env::var(\"RSS_JIT_"));
    assert!(vm.contains("NativeState::new_with_plan(native)"));
    assert!(
        !vm.starts_with("#![allow("),
        "the register VM root must not hide directory-wide lint debt"
    );

    let plan = read(&root.join("crates/rsscript-vm/src/reg_vm/execution_plan.rs"));
    assert!(plan.contains("struct ExecutionPlan"));
    assert!(plan.contains("enum TierPlan"));
    assert!(plan.contains("struct NativeAdmissionPolicy"));
    assert!(plan.contains("max_code_bytes"));
    assert!(plan.contains("max_compile_millis"));
    assert!(plan.contains("optimize_work_threshold"));
    assert!(plan.contains("jit-recursion-experimental"));
}

#[test]
fn cranelift_engine_uses_real_modules_instead_of_flattened_includes() {
    let root = workspace_root();
    for path in rust_files_below(&root.join("crates/rsscript-jit-cranelift/src")) {
        let source = read(&path);
        assert!(
            !source
                .lines()
                .any(|line| line.trim_start().starts_with("include!(")),
            "{} must use Rust modules instead of flattening source into one namespace",
            path.display()
        );
    }
}

#[test]
fn cranelift_engine_keeps_research_features_and_raw_abi_out_of_the_stable_surface() {
    let root = workspace_root();
    let manifest = read(&root.join("crates/rsscript-jit-cranelift/Cargo.toml"));
    let library = read(&root.join("crates/rsscript-jit-cranelift/src/lib.rs"));
    let vm_manifest = read(&root.join("crates/rsscript-vm/Cargo.toml"));

    assert!(manifest.contains("publish = false"));
    for feature in ["speculation = []", "recursion = []"] {
        assert!(
            manifest.contains(feature),
            "Cranelift manifest is missing isolated research feature `{feature}`"
        );
    }
    assert!(!manifest.contains("memoization = []"));
    assert!(!library.contains("pub use host_abi::*;"));
    assert!(!library.contains("pub use ir::*;"));
    assert!(!library.contains("pub use module::*;"));
    assert!(library.contains("JitCallFrame"));
    assert!(!library.contains("pub use host_abi::{\n    JitCallFrame"));
    assert!(library.contains("cfg(not(target_pointer_width = \"64\"))"));
    assert!(library.contains("native JIT currently requires a 64-bit target"));

    let native_jit = vm_manifest
        .lines()
        .find(|line| line.starts_with("native-jit ="))
        .expect("VM must declare the stable native-jit feature");
    assert!(!native_jit.contains("speculation"));
    assert!(!native_jit.contains("recursion"));
    assert!(!native_jit.contains("memoization"));
    assert!(vm_manifest.contains("jit-speculation"));
    assert!(vm_manifest.contains("jit-recursion-experimental"));
    assert!(!vm_manifest.contains("jit-memoization-experimental"));
    assert!(vm_manifest.contains("jit-struct-sr-experimental"));

    let scalar_replacement =
        read(&root.join("crates/rsscript-vm/src/reg_vm/native/passes/scalar_replacement.rs"));
    assert!(scalar_replacement.contains("feature = \"jit-struct-sr-experimental\""));
}
