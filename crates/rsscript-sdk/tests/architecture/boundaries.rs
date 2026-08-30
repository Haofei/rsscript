use std::collections::BTreeSet;

use crate::support::*;

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
fn selfhost_parity_domains_remain_separate_modules() {
    let root = workspace_root();
    let aggregator = root.join("experiments/selfhost-parity/src/selfhost_parity.rs");
    let source = read(&aggregator);
    let expected = ["lexer", "parser", "checker", "ast_oracle", "ast_parity"];

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
                "experiments/selfhost-parity/src/selfhost_parity/{domain}.rs"
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
        "rsscript-jit-cranelift",
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

    let vm = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs")),
        read(&root.join("crates/rsscript-vm/src/reg_vm/state.rs"))
    );
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
            "rustix".to_string(),
            "serde".to_string(),
            "sha2".to_string(),
            "tempfile".to_string(),
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
        "CompilationSession",
        "workspace_module_graph",
        "format_file_with_operation",
        "lint_file_with_operation",
        "symbol_index_file_with_operation",
        "document_symbols_file_with_operation",
        "operation_for_request",
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
    for forbidden in [
        "parse_source",
        "lint_cache:",
        "format_cache:",
        "symbol_cache:",
        "document_symbol_cache:",
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
fn semantic_diff_is_an_artifact_contract_not_sdk_implementation() {
    let root = workspace_root();
    let artifact = read(&root.join("crates/rsscript-artifact/src/lib.rs"));
    let sdk = sdk_source(&root);
    assert!(artifact.contains("mod semantic_diff;"));
    assert!(artifact.contains("SemanticDiffV2"));
    assert!(artifact.contains("SEMANTIC_DIFF_SCHEMA"));
    assert!(
        !root
            .join("crates/rsscript-sdk/src/semantic_diff.rs")
            .exists(),
        "SDK must compose the semantic-diff contract rather than own it"
    );
    assert!(sdk.contains("pub use rsscript_artifact"));
    assert!(sdk.contains("SemanticDiffV2"));
}

#[test]
fn package_analysis_schema_is_an_artifact_contract_not_compiler_implementation() {
    let root = workspace_root();
    let artifact = read(&root.join("crates/rsscript-artifact/src/lib.rs"));
    let package_types = read(&root.join("crates/rsscript-package-model/src/lib.rs"));

    for contract_type in [
        "PackageAnalysisV1",
        "PackageAnalysisProducerV1",
        "PackageAnalysisSummaryV1",
        "PackageAnalysisExportV1",
        "PackageAnalysisExternalImportV1",
        "PackageAnalysisCallEdgeV1",
        "PackageAnalysisResourceLifetimeV1",
        "PackageAnalysisTaskGroupV1",
    ] {
        assert!(
            artifact.contains(contract_type),
            "Artifact must own the neutral package-analysis type `{contract_type}`"
        );
    }
    assert!(
        package_types.contains("PackageAnalysisV1 as PackageAnalysis"),
        "compiler compatibility must re-export the Artifact-owned analysis type"
    );
    assert!(
        artifact.contains("pub enum AnalysisEnvelopeV1")
            && artifact.contains("Package {")
            && artifact.contains("package: PackageAnalysisV1"),
        "Bundle analysis must encode source/package evidence as mutually exclusive typed states"
    );
    assert!(
        !artifact.contains("source: Option<SourceAnalysisV1>")
            && !artifact.contains("package: Option<PackageAnalysisV1>"),
        "Bundle analysis must not represent mutually exclusive evidence variants with optional fields"
    );
    assert!(
        artifact.contains("Self::package(package)"),
        "package analysis JSON must be decoded through the Artifact-owned type"
    );
    assert!(
        !package_types.contains("pub struct PackageAnalysis {"),
        "compiler must not define a second package-analysis wire model"
    );
    assert!(
        !root
            .join("crates/rsscript-compiler/src/package/format.rs")
            .exists(),
        "compiler must not retain package evidence presentation"
    );
    let package_format = read(&root.join("crates/rsscript-package-review/src/format.rs"));
    assert!(
        !package_format.contains("format_package_analysis_json"),
        "review adapter must not define a second presentation for Artifact-owned analysis evidence"
    );
    let review_manifest: toml::Value = toml::from_str(&read(
        &root.join("crates/rsscript-package-review/Cargo.toml"),
    ))
    .expect("package review manifest should parse");
    let review_dependencies = normal_dependency_packages(&review_manifest);
    assert!(
        review_dependencies.contains("rsscript-package-model"),
        "review presentation must consume the compiler-independent package evidence model"
    );
    assert!(
        !review_dependencies.contains("rsscript-compiler"),
        "review presentation must not pull the compiler compatibility closure"
    );
    assert!(
        root.join("crates/rsscript-package-model/src/lib.rs")
            .is_file()
            && !root
                .join("crates/rsscript-compiler/src/package/types.rs")
                .exists(),
        "package review types must be physically owned by rsscript-package-model"
    );
    let compiler_manifest: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    assert!(
        !normal_dependency_packages(&compiler_manifest).contains("rsscript-review"),
        "compiler must not depend on optional review presentation"
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
        "let analysis = match operation {",
        "analysis.database().hir().clone()",
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
    assert!(execution.contains("non-wire async Provider cannot enter the wire async dispatcher"));
    assert!(execution.contains("is_wire_async"));
    assert!(execution.contains("is_wire_async_mut"));
    assert!(execution.contains("without registering a resource"));

    let vm_calls = read(&root.join("crates/rsscript-vm/src/reg_vm/calls.rs"));
    assert!(vm_calls.contains("let blocking_allowed = self.limits.allow_blocking_provider_calls"));
    assert!(!vm_calls.contains("blocking_allowed: true"));
    assert!(vm_calls.contains("async_allowed: false"));
    assert!(vm_calls.contains("AsyncProviderCallContext"));
    assert!(vm_calls.contains("function.start_wire_async"));

    let scheduler = read(&root.join("crates/rsscript-vm/src/reg_vm/scheduler.rs"));
    assert!(scheduler.contains("poll_provider_futures"));
    assert!(scheduler.contains("Wait::WireProvider"));
    assert!(scheduler.contains("Wait::WireMutationProvider"));
}

#[test]
fn execution_termination_does_not_classify_message_text() {
    let root = workspace_root();
    let facade = sdk_source(&root);
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
    let vm = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs")),
        read(&root.join("crates/rsscript-vm/src/reg_vm/state.rs"))
    );
    assert!(vm.contains("pub allocation_budget: Option<usize>"));
    assert!(vm.contains("allocated_bytes: usize"));
    assert!(vm.contains("This is not a live-memory measurement"));
    assert!(!vm.contains("pub mem_budget"));
    assert!(vm.contains("pub live_memory_limit: Option<usize>"));
    assert!(vm.contains("live_memory_bytes: usize"));
    let storage = read(&root.join("crates/rsscript-vm/src/reg_vm/exec/storage_accounting.rs"));
    assert!(storage.contains("refresh_live_memory_usage"));
    assert!(storage.contains("visited: &mut HashSet<usize>"));

    let facade = sdk_source(&root);
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
    let vm = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs")),
        read(&root.join("crates/rsscript-vm/src/reg_vm/state.rs"))
    );
    let sdk = sdk_source(&root);
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
fn complete_frontend_checker_is_owned_by_semantics() {
    let root = workspace_root();
    let compiler = root.join("crates/rsscript-compiler/src");
    let semantics = root.join("crates/rsscript-semantics/src");

    let obsolete = compiler.join("analyzer.rs");
    assert!(
        !obsolete.exists(),
        "compiler must not retain frontend-checker implementation at {}",
        obsolete.display()
    );
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
        "rsscript-jit-cranelift",
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
    assert!(
        semantics.contains("self.workspace_analysis_with_operation(operation)?")
            && semantics.contains("analysis.database().hir().clone()"),
        "workspace HIR must be projected from the session-owned checked analysis rather than rebuilt from a parallel isolation path"
    );

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
    assert!(semantics.contains("pub fn semantic_workspace_diagnostics_with_operation"));
    assert!(semantics.contains("self.workspace_analysis_with_operation(operation)?"));
    assert!(
        !semantics.contains("analyze_frontend_input_snapshot_with_operation"),
        "workspace diagnostics must project the session-owned analysis rather than select a second analyzer path"
    );

    let language_service = read(&root.join("crates/rsscript-language-service/src/lib.rs"));
    assert!(language_service.contains("semantic_document_diagnostics"));
    assert!(
        !language_service.contains("WorkspaceDiagnosticQuery"),
        "language-service must not select or inject a competing semantic query"
    );
}

#[test]
fn session_owns_the_core_interface_policy() {
    let root = workspace_root();
    let semantics = read(&root.join("crates/rsscript-semantics/src/database.rs"));
    assert!(semantics.contains("pub enum SessionInterfacePolicy"));
    assert!(semantics.contains("pub fn without_core() -> Self"));
    assert!(semantics.contains("pub fn with_standard_packages() -> Self"));
    assert!(semantics.contains("SessionInterfacePolicy::WithoutCore"));
    assert!(semantics.contains("SessionInterfacePolicy::WithStandardPackages"));

    let cli_check = read(&root.join("crates/rsscript-cli/src/cli/check.rs"));
    assert!(cli_check.contains("CompilationSession::without_core()"));

    let sdk = sdk_source(&root);
    assert!(sdk.contains("CompilationSession::default()"));
    assert!(sdk.contains("fn analyze_snapshot_with_session"));

    let session_boundary = sdk
        .split("fn analyze_snapshot_with_session")
        .nth(1)
        .and_then(|rest| rest.split("fn session_for_snapshot").next())
        .expect("reviewed snapshot session boundary must exist");
    for direct_analyzer_call in [
        "validate_sources_with_interfaces(",
        "analyze_sources_with_interfaces(",
        "rsscript_compiler::analyze_source_result",
    ] {
        assert!(
            !session_boundary.contains(direct_analyzer_call),
            "ordinary SDK paths must not select the legacy direct analyzer: {direct_analyzer_call}"
        );
    }
}

#[test]
fn production_frontend_callers_share_the_compilation_session_boundary() {
    let root = workspace_root();
    let compiler_output = read(&root.join("crates/rsscript-compiler/src/compiler_output.rs"));
    let sdk = sdk_source(&root);
    let project = sdk_source(&root);
    let cli_check = read(&root.join("crates/rsscript-cli/src/cli/check.rs"));
    let cli_fmt = read(&root.join("crates/rsscript-cli/src/cli/fmt.rs"));
    let cli_fix = read(&root.join("crates/rsscript-cli/src/cli/fix.rs"));
    let cli_artifact = read(&root.join("crates/rsscript-cli/src/cli/artifact.rs"));

    for required in [
        "CompilationSession::default()",
        "session.workspace_validated()?",
    ] {
        assert!(
            compiler_output.contains(required),
            "pure compiler lowering must use the shared session query: {required}"
        );
    }
    for required in [
        "CompilationSession::default()",
        "fn analyze_snapshot_with_session",
    ] {
        assert!(
            sdk.contains(required),
            "ordinary SDK analysis must use the shared session boundary: {required}"
        );
    }
    for (name, source) in [("check", cli_check), ("fmt", cli_fmt), ("fix", cli_fix)] {
        assert!(
            source.contains("CompilationSession"),
            "CLI {name} must use CompilationSession rather than construct a frontend analyzer"
        );
    }
    assert!(
        cli_artifact.contains("ProjectCompiler::new()")
            && cli_artifact.contains(".compile_package(Path::new(input))"),
        "normal artifact commands must capture once through the project boundary before pure compilation"
    );
    assert!(
        project.contains("Compiler.compile_snapshot(snapshot.frontend())"),
        "project convenience compilation must pass its immutable frontend snapshot to the pure compiler"
    );
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
    assert!(
        compiler_manifest["dependencies"]
            .get("rsscript-provider-api")
            .is_none(),
        "the frontend compiler must not declare the provider runtime API"
    );
    let dependency = "rsscript-lowering";
    assert_eq!(
        compiler_manifest["dependencies"][dependency]["optional"].as_bool(),
        Some(true),
        "LSP-excluded dependency `{dependency}` must remain optional"
    );
    assert_eq!(
        compiler_manifest["dependencies"]["rsscript-bytecode"]["optional"].as_bool(),
        Some(true),
        "compiler bytecode emission must remain outside the language-service closure"
    );
    assert!(
        compiler_manifest["dependencies"]
            .get("rsscript-vm")
            .is_none()
            && compiler_manifest["features"]
                .get("selfhost-parity")
                .is_none(),
        "frontend-only compiler closure must not retain the self-host VM adapter"
    );
}

#[test]
fn embedding_facade_exposes_only_product_level_objects() {
    let root = workspace_root();
    let mut source = sdk_source(&root);
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
    for forbidden in ["JitPlan", "RegInstr", "ReviewFinding", "reir"] {
        assert!(
            !source.contains(forbidden),
            "stable embedding façade must not expose `{forbidden}`"
        );
    }
}

#[test]
fn runtime_link_requires_explicit_host_artifact_admission() {
    let root = workspace_root();
    let sdk = sdk_source(&root);
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
    for required in ["rsscript-bytecode", "rsscript-provider-api"] {
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
}

#[test]
fn source_shaped_executable_ir_is_physically_deleted() {
    let root = workspace_root();
    let manifest: toml::Value = toml::from_str(&read(&root.join("crates/rsscript-vm/Cargo.toml")))
        .expect("VM manifest should parse");
    assert_eq!(
        manifest["dependencies"].get("rsscript-exec-ir"),
        None,
        "the execution-only VM must not retain the source-shaped IR dependency"
    );
    assert!(manifest["features"].get("legacy-exec-ir").is_none());

    let vm = read(&root.join("crates/rsscript-vm/src/reg_vm/mod.rs"));
    assert!(!vm.contains("legacy-exec-ir") && !vm.contains("rsscript_exec_ir"));
    let bytecode = read(&root.join("crates/rsscript-vm/src/reg_vm/bytecode.rs"));
    assert!(!bytecode.contains("encode_and_verify") && !bytecode.contains("verify_bytes("));

    assert!(!root.join("crates/rsscript-exec-ir").exists());
    assert!(!root.join("crates/rsscript-vm/src/reg_vm/lower.rs").exists());
}

#[test]
fn compiler_lowering_has_no_source_shaped_ir_compatibility_path() {
    let root = workspace_root();
    let lowering: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-lowering/Cargo.toml")))
            .expect("lowering manifest should parse");
    assert!(
        lowering["dependencies"].get("rsscript-exec-ir").is_none()
            && lowering
                .get("features")
                .is_none_or(|features| features.get("legacy-exec-ir").is_none()),
        "the lowering boundary must have no source-shaped compatibility closure"
    );

    let compiler: toml::Value =
        toml::from_str(&read(&root.join("crates/rsscript-compiler/Cargo.toml")))
            .expect("compiler manifest should parse");
    assert!(compiler["features"].get("legacy-exec-ir").is_none());
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
    assert!(
        manifest["dependencies"]
            .get("rsscript-jit-cranelift")
            .is_none()
    );
    assert!(
        manifest["dependencies"].get("rsscript-vm").is_none(),
        "self-host execution belongs to the independent experiments workspace"
    );

    assert!(
        manifest["features"].get("package").is_none(),
        "the pure compiler must not retain a package/persistence compatibility feature"
    );
    let lowering = manifest["features"]["lowering"]
        .as_array()
        .expect("compiler lowering feature should be declared")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect::<BTreeSet<_>>();
    assert!(manifest["features"].get("execution").is_none());
    assert!(manifest["features"].get("legacy-exec-ir").is_none());
    assert!(
        manifest["features"].get("selfhost-parity").is_none(),
        "compiler must not expose a research self-host feature"
    );
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
    for dependency in ["rsscript-lowering", "rsscript-mir", "sha2"] {
        let feature = format!("dep:{dependency}");
        assert!(
            lowering.contains(feature.as_str()),
            "lowering feature must explicitly select `{dependency}`"
        );
    }
    for dependency in ["fs2", "hex", "libc", "rustix", "tempfile", "toml", "uuid"] {
        assert!(
            manifest["dependencies"].get(dependency).is_none(),
            "pure compiler must not retain package/persistence dependency `{dependency}`"
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
        sdk_project.contains("dep:rsscript-project"),
        "project capture must select the dedicated project/input boundary rather than widening normal execution"
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
            "rsscript-jit-cranelift",
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
        "rsscript-jit-cranelift",
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
        "rsscript-jit-cranelift",
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
    assert!(
        !provider_source.contains("pub enum NativeValue")
            && !provider_source.contains("pub struct NativeInterpreterFn"),
        "the canonical Provider contract must not retain the retired dynamic compatibility API"
    );
    assert!(
        provider_source.contains("pub details: Option<WireValue>"),
        "canonical Provider errors must not reopen a serde_json ABI escape hatch"
    );
    assert!(
        !provider_source.contains("pub details: Option<serde_json::Value>"),
        "Provider error details must remain structural wire values"
    );
    let provider_manifest = read(&root.join("crates/rsscript-provider-api/Cargo.toml"));
    assert!(
        !provider_manifest.contains("compatibility = []"),
        "the retired Provider compatibility feature must remain deleted"
    );
    let vm_manifest = read(&root.join("crates/rsscript-vm/Cargo.toml"));
    assert!(
        !vm_manifest.contains("features = [\"compatibility\"]"),
        "the register VM must use only the canonical Provider wire API"
    );
}

#[test]
fn official_providers_use_canonical_wire_callables() {
    let root = workspace_root();
    for provider in [
        "cli", "entropy", "env", "fs", "http", "log", "process", "time",
    ] {
        let source = read(&root.join(format!("providers/{provider}/src/lib.rs")));
        assert!(
            source.contains("WireInterpreterFn"),
            "official provider `{provider}` must use the canonical wire callable"
        );
        assert!(
            !source.contains("NativeInterpreterFn") && !source.contains("NativeValue"),
            "official provider `{provider}` must not regress to the legacy dynamic value boundary"
        );
    }
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
        "rsscript-jit-cranelift",
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
    let sdk = sdk_source(&root);
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

    let emitter = read(&root.join("crates/rsscript-codegen-vm/src/lib.rs"));
    assert!(emitter.contains("LANGUAGE_SEMANTICS_VERSION"));
    assert!(emitter.contains("compiler_provenance"));
    assert!(verifier.contains("BYTECODE_CONTAINER_FORMAT_VERSION"));

    let analysis = read(&root.join("crates/rsscript-package-review/src/analysis.rs"));
    assert!(analysis.contains("rsscript_abi_model::LANGUAGE_SEMANTICS_VERSION"));
    assert!(
        !analysis.contains("language_version: env!(\"CARGO_PKG_VERSION\")"),
        "neutral analysis must carry language semantics rather than compiler provenance"
    );
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
        "rsscript-jit-cranelift",
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
        "mir_id!(BuiltinId)",
        "mir_id!(ExternalSymbolId)",
        "pub struct MirClosureCapture",
        "MakeClosure",
        "CallClosure",
        "pub struct MirFunctionSignature",
        "pub struct MirModule",
        "pub fn verify",
    ] {
        assert!(mir.contains(required), "MIR is missing {required}");
    }
    for forbidden in [
        "rsscript_syntax",
        "rsscript_semantics",
        "Unknown",
        "Executable",
    ] {
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
    let lowering_root = root.join("experiments/aot-backend/src/rust_lower");
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
        "crates/rsscript-package-review/src/bindings.rs",
        "crates/rsscript-vm/src/reg_vm/tier/admission.rs",
        "crates/rsscript-vm/src/reg_vm/tier/call_scratch.rs",
        "crates/rsscript-vm/src/reg_vm/tier/recursion.rs",
        "experiments/aot-backend/src/rust_lower/helpers/executable_declarations.rs",
        "experiments/aot-backend/src/rust_lower/helpers/semantic_projection.rs",
        "experiments/aot-runtime/src/json.rs",
        "crates/rsscript-jit-cranelift/src/analysis.rs",
        "crates/rsscript-jit-cranelift/src/executable_memory.rs",
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
fn selfhost_frontend_does_not_restore_retired_language_contracts() {
    let root = workspace_root();
    let checker = read(&root.join("experiments/fixtures/selfhost/check.rss"));
    let syntax_declarations = read(
        &root.join("experiments/fixtures/selfhost/checker/diagnostics/syntax_declarations.rss"),
    );
    for retired_code in [
        "RS0004", "RS0006", "RS0009", "RS0010", "RS0011", "RS0012", "RS0014", "RS0016", "RS0017",
        "RS0018", "RS0019", "RS0020", "RS0101",
    ] {
        assert!(
            !checker.contains(retired_code),
            "self-hosted checker must not emit retired diagnostic `{retired_code}`"
        );
    }

    let scanner = read(&root.join("experiments/fixtures/selfhost/scan.rss"));
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
