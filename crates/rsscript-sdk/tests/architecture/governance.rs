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
            "rsscript-core-types".to_string(),
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
        "the SDK test closure must not path-depend on archived experiments"
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
    // Archived backends live on a branch, but an optional dependency can
    // still accidentally pull one back into the normal product path. Check
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
