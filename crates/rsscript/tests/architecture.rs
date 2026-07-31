use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

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

#[test]
fn intrinsic_catalog_is_the_only_generated_registry_source() {
    let root = workspace_root();
    let build = read(&root.join("crates/rsscript/build.rs"));
    assert!(
        build.contains("intrinsics.toml"),
        "the intrinsic generator must consume the structured catalog"
    );
    assert!(
        !build.contains("src/reg_vm/lower.rs") && !build.contains("src/runtime_abi.rs"),
        "the intrinsic generator must not scrape Rust implementation source"
    );

    let catalog = read(&root.join("crates/rsscript/intrinsics.toml"));
    let catalog: toml::Value = toml::from_str(&catalog).expect("intrinsic catalog should parse");
    assert_eq!(catalog["schema"].as_integer(), Some(1));
    assert!(
        catalog["intrinsic"]
            .as_array()
            .is_some_and(|entries| entries.len() >= 500),
        "the catalog must retain internal VM-only intrinsic identities"
    );
    assert!(
        catalog["binding"]
            .as_array()
            .is_some_and(|entries| entries.len() >= 500),
        "the catalog must retain the public runtime and VM surface"
    );
}

#[test]
fn selfhost_known_type_sets_are_generated() {
    let root = workspace_root();
    let checker = read(&root.join("selfhost/check.rss"));
    assert!(
        !checker.contains("fn is_builtin_type(") && !checker.contains("fn is_stdlib_type("),
        "self-host type knowledge must come from generated interface metadata"
    );
    let metadata = read(&root.join("crates/rsscript/src/interface_metadata.rs"));
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

    assert_eq!(declarations, ["fn main() -> Unit {"]);
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
    let inference = read(&root.join("crates/rsscript/src/hir/infer.rs"));
    for parser in [
        "strip_prefix(\"Fn(\")",
        "strip_prefix(\"Result<\")",
        "strip_prefix(\"Option<\")",
        "strip_prefix(\"List<\")",
        "strip_prefix(\"Stream<\")",
        "strip_prefix(\"Task<\")",
        "strip_prefix(\"Capability<\")",
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
    let hir = read(&root.join("crates/rsscript/src/hir.rs"));
    assert!(hir.contains("pub ty: ResolvedType"));
    assert!(hir.contains("pub return_ty: Option<ResolvedType>"));
    assert!(!hir.contains("pub type_name: String"));
    assert!(!hir.contains("pub return_type: Option<String>"));

    let inference = read(&root.join("crates/rsscript/src/hir/infer.rs"));
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
    let tier = read(&root.join("crates/rsscript/src/reg_vm/tier.rs"));
    assert!(tier.contains("mod deopt_resume;"));
    assert!(tier.contains("mod jit_entry;"));
    assert!(!tier.contains("fn restore_native_deopt_live_regs("));
    assert!(!tier.contains("fn run_jit_pure_leaf("));

    let deopt = read(&root.join("crates/rsscript/src/reg_vm/tier/deopt_resume.rs"));
    assert!(deopt.contains("fn try_resume_native_child_deopt_chain("));
    assert!(deopt.contains("fn restore_native_deopt_live_regs("));

    let entry = read(&root.join("crates/rsscript/src/reg_vm/tier/jit_entry.rs"));
    assert!(entry.contains("fn run_jit("));
    assert!(entry.contains("fn run_jit_self_recursive_int("));
}

#[test]
fn register_vm_execution_policy_is_snapshotted_before_running() {
    let root = workspace_root();
    let vm = read(&root.join("crates/rsscript/src/reg_vm/mod.rs"));
    assert!(vm.contains("mod execution_plan;"));
    assert!(vm.contains("NativeExecutionPlan::from_environment("));
    assert!(vm.contains("NativeState::new_with_plan(native)"));

    let plan = read(&root.join("crates/rsscript/src/reg_vm/execution_plan.rs"));
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
fn register_vm_test_domains_remain_separate_modules() {
    let root = workspace_root();
    let aggregator = root.join("crates/rsscript/src/reg_vm/tests.rs");
    let source = read(&aggregator);
    let expected = [
        "intrinsic_registry",
        "resource_boundary",
        "register_window",
        "closure_cache",
        "j1_profiling",
    ];

    assert!(
        source.lines().count() <= expected.len() + 2,
        "reg_vm/tests.rs must remain a composition root"
    );
    for domain in expected {
        assert!(
            source.contains(&format!("tests/{domain}.rs")),
            "reg_vm test composition root is missing `{domain}`"
        );
        assert!(
            root.join(format!("crates/rsscript/src/reg_vm/tests/{domain}.rs"))
                .is_file(),
            "reg_vm test domain `{domain}` must have its own module"
        );
    }

    let register_window = read(&root.join("crates/rsscript/src/reg_vm/tests/register_window.rs"));
    let register_window_domains = [
        "lowering",
        "translation",
        "tiering_and_memo",
        "abi_and_heap",
        "osr_collections",
        "closures",
        "deopt_and_transactions",
    ];
    assert!(
        register_window.lines().count() <= 600,
        "register_window.rs must remain a helper and composition root"
    );
    for domain in register_window_domains {
        assert!(
            register_window.contains(&format!("register_window/{domain}.rs")),
            "register-window composition root is missing `{domain}`"
        );
        assert!(
            root.join(format!(
                "crates/rsscript/src/reg_vm/tests/register_window/{domain}.rs"
            ))
            .is_file(),
            "register-window test domain `{domain}` must have its own module"
        );
    }
}

#[test]
fn jit_acceptance_domains_remain_separate_modules() {
    let root = workspace_root();
    let aggregator = root.join("crates/rsscript/tests/jit_acceptance.rs");
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
            root.join(format!("crates/rsscript/tests/jit_acceptance/{domain}.rs"))
                .is_file(),
            "JIT acceptance domain `{domain}` must have its own module"
        );
    }
}

#[test]
fn selfhost_parity_domains_remain_separate_modules() {
    let root = workspace_root();
    let aggregator = root.join("crates/rsscript/src/selfhost_parity.rs");
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
            root.join(format!("crates/rsscript/src/selfhost_parity/{domain}.rs"))
                .is_file(),
            "self-host parity domain `{domain}` must have its own module"
        );
    }
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
            && vm_model.contains("HostAuthority::Process"),
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
        dispatch.contains("self.authorize_intrinsic_host_access(intrinsic, args, base)?"),
        "authority must be checked before intrinsic dispatch"
    );
    let adapters = read(&root.join("crates/rsscript/src/reg_vm/host_adapters.rs"));
    assert!(
        adapters.contains("intrinsic.host_authority()")
            && adapters.contains(".filesystem_path(&authorized)")
            && adapters.contains(".network_endpoint(&authorized)")
            && adapters.contains(".process_executable(&authorized)"),
        "restricted dispatch must consume exact scope-bound host capabilities"
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
