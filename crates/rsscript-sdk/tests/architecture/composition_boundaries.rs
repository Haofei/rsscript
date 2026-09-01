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
    let database = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-semantics/src/database.rs")),
        read(&root.join("crates/rsscript-semantics/src/database/session.rs"))
    );
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
    let semantics = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-semantics/src/database.rs")),
        read(&root.join("crates/rsscript-semantics/src/database/session.rs"))
    );
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
    let semantics = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-semantics/src/database.rs")),
        read(&root.join("crates/rsscript-semantics/src/database/session.rs"))
    );
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
    let semantics = format!(
        "{}\n{}",
        read(&root.join("crates/rsscript-semantics/src/database.rs")),
        read(&root.join("crates/rsscript-semantics/src/database/session.rs"))
    );
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
