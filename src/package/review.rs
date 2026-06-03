use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::analyzer::{
    analyze_source_with_interfaces, analyze_sources_with_interfaces_without_core, core_interfaces,
};
use crate::diagnostic::{Diagnostic, Span, code};
use crate::hir::{CallResolution, Hir};
use crate::lint::lint_source;
use crate::review::{ReviewMap, ReviewMapClassification, review_map_sources_with_interfaces};
use crate::runtime_abi;
use crate::syntax::ast::{Block, Callee, Expr, Item, Stmt, TypeKind, merge_programs};
use crate::syntax::parse_source;

use super::contract::{
    PackageFunctionContract, collect_package_const_contracts, collect_package_function_contracts,
    collect_package_sum_type_contracts, collect_package_type_alias_contracts,
    collect_package_type_contracts, package_contract_has_resource_boundary,
    package_interface_contract_diagnostics, package_interface_diagnostic_exports,
    package_interface_environment_diagnostics, package_review_exports,
};
use super::native::{
    native_binding_interface_sources, package_native_binding_diagnostics, package_native_bindings,
    package_native_rust_review,
};
use super::source_set::{Manifest, PackageSource, load_package, load_package_with_features};
use super::{
    PackageDependencyKind, PackageNativeRustReview, PackageProviderImplementation, PackageReview,
    PackageReviewAwaitBoundary, PackageReviewAwaitSite, PackageReviewCapability,
    PackageReviewDependency, PackageReviewFile, PackageReviewFileKind, PackageReviewSummary,
    PackageRisk, PackageVirtual, collect_dependency_interface_sources,
    collect_dependency_interface_sources_for_tests, dedup_diagnostics, package_dependency_spec,
    package_feature_may_change_boundary_risk, package_feature_resolution_diagnostics,
};

pub fn review_package_dir(package_dir: &Path) -> Result<PackageReview, String> {
    review_package_dir_with_features(package_dir, None)
}

pub(super) fn review_package_dir_with_features(
    package_dir: &Path,
    selected_features: Option<&[String]>,
) -> Result<PackageReview, String> {
    let package = match selected_features {
        Some(features) => load_package_with_features(package_dir, Some(features))?,
        None => load_package(package_dir)?,
    };
    let manifest = &package.manifest;
    let sources = &package.sources;
    let dependency_interfaces = collect_dependency_interface_sources(package_dir, manifest)?;
    let test_dependency_interfaces =
        collect_dependency_interface_sources_for_tests(package_dir, manifest)?;
    let native_bindings = package_native_bindings(package_dir)?;
    let native_binding_interfaces = native_binding_interface_sources(sources, &native_bindings);

    let interface_refs = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let dependency_interface_refs = dependency_interfaces
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let core_interface_refs = core_interfaces().to_vec();
    let contract_external_interfaces = dependency_interface_refs.clone();
    let mut external_interfaces = core_interface_refs;
    external_interfaces.extend(dependency_interface_refs);
    let mut combined_interfaces = contract_external_interfaces.clone();
    combined_interfaces.extend(interface_refs.clone());
    let native_binding_interface_refs = native_binding_interfaces
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let mut source_interfaces = external_interfaces.clone();
    source_interfaces.extend(native_binding_interface_refs);
    let source_refs = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let test_refs = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Test)
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();
    let interface_frontend_diagnostics = interface_refs
        .iter()
        .flat_map(|(path, contents)| {
            let mut visible_interfaces = contract_external_interfaces.clone();
            visible_interfaces.extend(
                interface_refs
                    .iter()
                    .filter(|(interface_path, _)| interface_path != path)
                    .copied(),
            );
            analyze_source_with_interfaces(path, contents, &visible_interfaces)
        })
        .collect::<Vec<_>>();
    let interface_diagnostic_exports =
        package_interface_diagnostic_exports(sources, &interface_frontend_diagnostics);
    let mut diagnostics = package_interface_environment_diagnostics(&combined_interfaces);
    diagnostics.extend(package_feature_resolution_diagnostics(
        package_dir,
        manifest,
    )?);
    diagnostics.extend(package_provider_implementation_diagnostics(
        package_dir,
        manifest,
    ));
    diagnostics.extend(package_virtual_diagnostics(package_dir, manifest, sources));
    diagnostics.extend(interface_frontend_diagnostics);
    diagnostics.extend(analyze_sources_with_interfaces_without_core(
        &source_refs,
        &source_interfaces,
    ));
    if !test_refs.is_empty() {
        let mut test_interfaces = source_interfaces.clone();
        let mut seen_test_interfaces = test_interfaces
            .iter()
            .map(|(path, _)| (*path).to_string())
            .collect::<BTreeSet<_>>();
        test_interfaces.extend(
            test_dependency_interfaces
                .iter()
                .map(|source| (source.path.as_str(), source.contents.as_str()))
                .filter(|(path, _)| seen_test_interfaces.insert((*path).to_string())),
        );
        if source_refs.is_empty() {
            test_interfaces.extend(interface_refs.clone());
        } else {
            test_interfaces.extend(source_refs.clone());
        }
        diagnostics.extend(analyze_sources_with_interfaces_without_core(
            &test_refs,
            &test_interfaces,
        ));
    }
    diagnostics.extend(package_interface_contract_diagnostics(
        sources,
        &native_bindings,
    ));
    diagnostics.extend(package_native_binding_diagnostics(
        package_dir,
        sources,
        &native_bindings,
        manifest
            .native
            .as_ref()
            .and_then(|native| native.rust.as_ref()),
    ));
    diagnostics.extend(package_lint_diagnostics(sources));
    dedup_diagnostics(&mut diagnostics);
    let review_map = review_map_sources_with_interfaces(
        sources
            .iter()
            .map(|source| (source.path.as_str(), source.contents.as_str()))
            .collect(),
        &source_interfaces,
    );

    let native_rust = manifest
        .native
        .as_ref()
        .and_then(|native| native.rust.as_ref())
        .filter(|native| native.enabled)
        .map(|native| package_native_rust_review(package_dir, manifest, sources, native));

    let capabilities = package_review_capabilities(manifest, sources);
    let unknown_capability_bindings = capabilities
        .iter()
        .filter(|capability| capability.unknown_reason.is_some())
        .map(|capability| capability.binding_symbol.as_str())
        .collect::<BTreeSet<_>>()
        .len();

    let mut reasons = Vec::new();
    collect_manifest_review_reasons(manifest, &mut reasons);
    collect_native_reasons(native_rust.as_ref(), &mut reasons);
    collect_review_map_reasons(&review_map, &mut reasons);
    collect_unknown_capability_binding_reasons(&capabilities, &mut reasons);
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        reasons.push("package contains frontend errors".to_string());
    } else if !diagnostics.is_empty() {
        reasons.push("package contains frontend warnings".to_string());
    }
    if !interface_diagnostic_exports.is_empty() {
        reasons.push("public .rssi contract contains frontend errors".to_string());
    }
    reasons.sort();
    reasons.dedup();

    let await_sites = collect_package_await_sites(sources);
    let api_summary = package_api_effect_summary(sources, &review_map, &await_sites);
    let risk = if interface_diagnostic_exports.is_empty() {
        package_risk(
            manifest,
            native_rust.as_ref(),
            &review_map,
            &diagnostics,
            api_summary.native_apis,
            unknown_capability_bindings,
        )
    } else {
        PackageRisk::Unknown
    };
    let summary = PackageReviewSummary {
        interface_files: sources
            .iter()
            .filter(|source| source.kind == PackageReviewFileKind::Interface)
            .count(),
        source_files: sources
            .iter()
            .filter(|source| source.kind == PackageReviewFileKind::Source)
            .count(),
        diagnostics: diagnostics.len(),
        errors: diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity.is_error())
            .count(),
        dependencies: manifest.dependencies.len(),
        dev_dependencies: manifest.dev_dependencies.len(),
        package_features: manifest.features.len(),
        public_types: api_summary.public_types,
        public_sum_types: api_summary.public_sum_types,
        public_type_aliases: api_summary.public_type_aliases,
        public_consts: api_summary.public_consts,
        public_functions: api_summary.public_functions,
        public_apis: api_summary.public_apis,
        mutating_apis: api_summary.mutating_apis,
        retaining_apis: api_summary.retaining_apis,
        resource_apis: api_summary.resource_apis,
        fresh_returning_apis: api_summary.fresh_returning_apis,
        guarantee_apis: api_summary.guarantee_apis,
        native_guarantee_apis: api_summary.native_guarantee_apis,
        native_apis: api_summary.native_apis,
        async_apis: api_summary.async_apis,
        await_sites: api_summary.await_sites,
        parallel_apis: api_summary.parallel_apis,
        unsafe_apis: api_summary.unsafe_apis,
        unknown_apis: api_summary.unknown_apis
            + interface_diagnostic_exports.len()
            + unknown_capability_bindings,
    };
    let files = sources
        .iter()
        .map(|source| PackageReviewFile {
            path: source.path.clone(),
            kind: source.kind,
        })
        .collect();
    let features = manifest.features.keys().cloned().collect::<Vec<_>>();
    let virtual_package = package_virtual(manifest);
    let implements = package_provider_implementations(manifest);
    let dependencies = package_review_dependencies(manifest);
    let mut exports = package_review_exports(sources, &review_map);
    exports.extend(interface_diagnostic_exports);
    exports.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });

    Ok(PackageReview {
        package: super::package_identity(manifest),
        manifest_path: package.manifest_path.display().to_string(),
        risk,
        reasons,
        features,
        virtual_package,
        implements,
        dependencies,
        summary,
        files,
        exports,
        capabilities,
        await_sites,
        native_rust,
        review_map,
        diagnostics,
    })
}

fn package_virtual(manifest: &Manifest) -> Option<PackageVirtual> {
    manifest
        .virtual_package
        .as_ref()
        .map(|virtual_package| PackageVirtual {
            has_default: virtual_package.has_default,
            provider: virtual_package.provider.clone(),
        })
}

fn package_review_dependencies(manifest: &Manifest) -> Vec<PackageReviewDependency> {
    let mut dependencies = manifest
        .dependencies
        .iter()
        .map(|(name, value)| package_review_dependency(name, value, PackageDependencyKind::Normal))
        .chain(manifest.dev_dependencies.iter().map(|(name, value)| {
            package_review_dependency(name, value, PackageDependencyKind::Dev)
        }))
        .collect::<Vec<_>>();
    dependencies.sort_by(|left, right| {
        left.dependency_kind
            .cmp(&right.dependency_kind)
            .then_with(|| left.name.cmp(&right.name))
    });
    dependencies
}

fn package_review_dependency(
    name: &str,
    value: &toml::Value,
    dependency_kind: PackageDependencyKind,
) -> PackageReviewDependency {
    let spec = package_dependency_spec(name, value);
    let source = if let Some(path) = &spec.path {
        format!("path+{path}")
    } else if let Some(git) = &spec.git {
        format!("git+{git}")
    } else {
        "registry".to_string()
    };

    PackageReviewDependency {
        name: spec.name,
        requirement: spec.requirement,
        source,
        features: spec.features,
        dependency_kind,
        compile_only: spec.compile_only,
        test_only: spec.test_only,
        platform_provided: spec.platform_provided,
    }
}

fn package_lint_diagnostics(sources: &[PackageSource]) -> Vec<Diagnostic> {
    sources
        .iter()
        .flat_map(|source| lint_source(&source.path, &source.contents))
        .collect()
}

fn package_provider_implementations(manifest: &Manifest) -> Vec<PackageProviderImplementation> {
    manifest
        .implements
        .iter()
        .map(
            |(interface_package, implementation)| PackageProviderImplementation {
                interface_package: interface_package.clone(),
                version: implementation.version.clone(),
                interface_features: implementation.interface_features.clone(),
                interface_effective_hash: implementation.interface_effective_hash.clone(),
            },
        )
        .collect()
}

fn package_provider_implementation_diagnostics(
    package_dir: &Path,
    manifest: &Manifest,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    for (interface_package, implementation) in &manifest.implements {
        if implementation.version.as_deref().is_none_or(str::is_empty) {
            diagnostics.push(package_provider_implementation_diagnostic(
                package_dir,
                interface_package,
                "version",
                "provider implementation is missing `version`.",
                "`version` must declare the interface package version requirement this provider implements.",
            ));
        }
        if implementation
            .interface_effective_hash
            .as_deref()
            .is_none_or(str::is_empty)
        {
            diagnostics.push(package_provider_implementation_diagnostic(
                package_dir,
                interface_package,
                "interface_effective_hash",
                "provider implementation is missing `interface_effective_hash`.",
                "`interface_effective_hash` must bind the provider to one normalized interface contract.",
            ));
        }
    }
    diagnostics
}

fn package_virtual_diagnostics(
    package_dir: &Path,
    manifest: &Manifest,
    sources: &[PackageSource],
) -> Vec<Diagnostic> {
    let Some(virtual_package) = manifest.virtual_package.as_ref() else {
        return Vec::new();
    };
    let has_interface = sources
        .iter()
        .any(|source| source.kind == PackageReviewFileKind::Interface);
    let has_source = sources
        .iter()
        .any(|source| source.kind == PackageReviewFileKind::Source);
    let mut diagnostics = Vec::new();
    if !has_interface {
        diagnostics.push(package_virtual_diagnostic(
            package_dir,
            "virtual",
            "virtual package must declare an interface.",
            "`[virtual]` packages define a contract that implementations bind to, so at least one `.rssi` file must be present.",
        ));
    }
    if virtual_package.has_default && !has_source {
        diagnostics.push(package_virtual_diagnostic(
            package_dir,
            "has_default",
            "virtual package with `has_default = true` must provide source files.",
            "A default implementation is linkable only when the virtual package includes RSScript source or native bindings.",
        ));
    }
    if !virtual_package.has_default && has_source {
        diagnostics.push(package_virtual_diagnostic(
            package_dir,
            "has_default",
            "virtual package without a default implementation cannot include source files.",
            "Set `has_default = true`, or move the implementation into a provider package declared with `[implements]`.",
        ));
    }
    diagnostics
}

fn package_virtual_diagnostic(
    package_dir: &Path,
    label: &str,
    summary: impl Into<String>,
    cause: impl Into<String>,
) -> Diagnostic {
    Diagnostic::error(
        code::PACKAGE_PROVIDER_DECLARATION,
        summary,
        super::package_manifest_key_span(package_dir, "virtual"),
        label,
    )
    .with_cause(cause)
    .with_fix(
        "fix_virtual_package",
        "Use `[virtual] has_default = true` with a default implementation, or keep the virtual package interface-only and select a provider with `[providers]`.",
        "manual",
    )
}

fn package_provider_implementation_diagnostic(
    package_dir: &Path,
    interface_package: &str,
    key: &str,
    summary: impl Into<String>,
    cause: impl Into<String>,
) -> Diagnostic {
    Diagnostic::error(
        code::PACKAGE_PROVIDER_DECLARATION,
        summary,
        super::package_manifest_key_span(package_dir, interface_package),
        key,
    )
    .with_cause(cause)
    .with_fix(
        "fix_provider_declaration",
        format!("Add `[implements.\"{interface_package}\"].{key}` with the reviewed value."),
        "manual",
    )
}

fn collect_manifest_review_reasons(manifest: &Manifest, reasons: &mut Vec<String>) {
    if !manifest.features.is_empty() {
        reasons.push("package declares selectable package features".to_string());
    }
    super::collect_package_feature_boundary_reasons(&manifest.features, reasons);
    if let Some(review) = &manifest.review {
        if review.expect.risk.as_deref() == Some("unknown") {
            reasons.push("manifest declares unknown package risk".to_string());
        }
        if review.policy.deny_unknown == Some(true) {
            reasons.push("package policy denies unknown review risk".to_string());
        }
        if review.policy.deny_native == Some(true) {
            reasons.push("package policy denies native boundaries".to_string());
        }
        if review.policy.deny_unsafe_apis == Some(true) {
            reasons.push("package policy denies unsafe APIs".to_string());
        }
    }
}

fn collect_native_reasons(native: Option<&PackageNativeRustReview>, reasons: &mut Vec<String>) {
    let Some(native) = native else {
        return;
    };
    reasons.push("native Rust wrapper enabled".to_string());
    if native
        .build_scripts
        .as_deref()
        .is_some_and(|policy| policy != "forbid")
    {
        reasons.push("native Rust build scripts require review".to_string());
    }
    if native
        .proc_macros
        .as_deref()
        .is_some_and(|policy| policy != "forbid")
    {
        reasons.push("native Rust proc macros require review".to_string());
    }
    if native
        .unsafe_policy
        .as_deref()
        .is_some_and(|policy| policy != "forbid")
    {
        reasons.push("native Rust unsafe policy requires review".to_string());
    }
    if !native.links.is_empty()
        && native
            .native_links_policy
            .as_deref()
            .is_none_or(|policy| policy != "allow")
    {
        if native.native_links_policy.as_deref() == Some("forbid") {
            reasons.push("native Rust links external libraries forbidden".to_string());
        } else {
            reasons.push("native Rust links external libraries".to_string());
        }
    }
    if native.semantic.source_scan_best_effort.ffi_detected
        && native
            .ffi_policy
            .as_deref()
            .is_none_or(|policy| policy != "allow")
    {
        if native.ffi_policy.as_deref() == Some("forbid") {
            reasons.push("native Rust FFI usage forbidden".to_string());
        } else {
            reasons.push("native Rust FFI usage requires review".to_string());
        }
    }
    if native.semantic.author_declaration.worker_thread_parallelism {
        reasons.push("native Rust parallel worker execution requires review".to_string());
    }
    if !native
        .semantic
        .source_scan_best_effort
        .native_parallel_backends
        .is_empty()
    {
        reasons.push("native Rust parallel backend detected".to_string());
    }
    if !native.cargo_features.is_empty() {
        reasons.push("native Rust Cargo features require review".to_string());
    }
}

fn collect_review_map_reasons(review_map: &ReviewMap, reasons: &mut Vec<String>) {
    if review_map.summary.unknown.functions > 0 {
        reasons.push("review map contains unknown functions".to_string());
    }
    if review_map.summary.review_required.functions > 0 {
        reasons.push("review map contains must-review functions".to_string());
    }
}

fn collect_unknown_capability_binding_reasons(
    capabilities: &[PackageReviewCapability],
    reasons: &mut Vec<String>,
) {
    if capabilities
        .iter()
        .any(|capability| capability.unknown_reason.is_some())
    {
        reasons.push("native/external capability binding unknown".to_string());
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PackageApiSummary {
    public_types: usize,
    public_sum_types: usize,
    public_type_aliases: usize,
    public_consts: usize,
    public_functions: usize,
    public_apis: usize,
    mutating_apis: usize,
    retaining_apis: usize,
    resource_apis: usize,
    fresh_returning_apis: usize,
    guarantee_apis: usize,
    native_guarantee_apis: usize,
    native_apis: usize,
    async_apis: usize,
    await_sites: usize,
    parallel_apis: usize,
    unsafe_apis: usize,
    unknown_apis: usize,
}

fn package_api_effect_summary(
    sources: &[PackageSource],
    review_map: &ReviewMap,
    await_sites: &[PackageReviewAwaitSite],
) -> PackageApiSummary {
    let interface_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let interface_type_contracts =
        collect_package_type_contracts(sources, PackageReviewFileKind::Interface);
    let interface_sum_type_contracts =
        collect_package_sum_type_contracts(sources, PackageReviewFileKind::Interface);
    let interface_type_alias_contracts =
        collect_package_type_alias_contracts(sources, PackageReviewFileKind::Interface);
    let interface_const_contracts =
        collect_package_const_contracts(sources, PackageReviewFileKind::Interface);
    let source_contracts;
    let source_type_contracts;
    let source_sum_type_contracts;
    let source_type_alias_contracts;
    let source_const_contracts;
    let contracts = if interface_contracts.is_empty() {
        source_contracts =
            collect_package_function_contracts(sources, PackageReviewFileKind::Source);
        &source_contracts
    } else {
        &interface_contracts
    };
    let type_contracts = if interface_type_contracts.is_empty() {
        source_type_contracts =
            collect_package_type_contracts(sources, PackageReviewFileKind::Source);
        &source_type_contracts
    } else {
        &interface_type_contracts
    };
    let sum_type_contracts = if interface_sum_type_contracts.is_empty() {
        source_sum_type_contracts =
            collect_package_sum_type_contracts(sources, PackageReviewFileKind::Source);
        &source_sum_type_contracts
    } else {
        &interface_sum_type_contracts
    };
    let type_alias_contracts = if interface_type_alias_contracts.is_empty() {
        source_type_alias_contracts =
            collect_package_type_alias_contracts(sources, PackageReviewFileKind::Source);
        &source_type_alias_contracts
    } else {
        &interface_type_alias_contracts
    };
    let const_contracts = if interface_const_contracts.is_empty() {
        source_const_contracts =
            collect_package_const_contracts(sources, PackageReviewFileKind::Source);
        &source_const_contracts
    } else {
        &interface_const_contracts
    };
    let resource_types = type_contracts
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();

    PackageApiSummary {
        public_types: type_contracts.len(),
        public_sum_types: sum_type_contracts.len(),
        public_type_aliases: type_alias_contracts.len(),
        public_consts: const_contracts.len(),
        public_functions: contracts.len(),
        public_apis: type_contracts.len()
            + sum_type_contracts.len()
            + type_alias_contracts.len()
            + const_contracts.len()
            + contracts.len(),
        mutating_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .params
                    .iter()
                    .any(|param| param.effect == Some("mut"))
            })
            .count(),
        retaining_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.starts_with("retains("))
            })
            .count(),
        resource_apis: contracts
            .values()
            .filter(|contract| package_contract_has_resource_boundary(contract, &resource_types))
            .count(),
        fresh_returning_apis: contracts
            .values()
            .filter(|contract| contract.returns_fresh)
            .count(),
        guarantee_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| is_guarantee_effect(effect))
            })
            .count(),
        native_guarantee_apis: contracts
            .values()
            .filter(|contract| {
                contract.effects.iter().any(|effect| effect == "native")
                    && contract
                        .effects
                        .iter()
                        .any(|effect| is_guarantee_effect(effect))
            })
            .count(),
        native_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.as_str() == "native")
            })
            .count(),
        async_apis: contracts
            .values()
            .filter(|contract| contract.is_async)
            .count(),
        await_sites: await_sites.len(),
        parallel_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.as_str() == "parallel")
            })
            .count(),
        unsafe_apis: contracts
            .values()
            .filter(|contract| {
                contract
                    .effects
                    .iter()
                    .any(|effect| effect.as_str() == "unsafe")
            })
            .count(),
        unknown_apis: package_unknown_api_count(contracts, review_map),
    }
}

fn package_review_capabilities(
    manifest: &Manifest,
    sources: &[PackageSource],
) -> Vec<PackageReviewCapability> {
    let call_graph = collect_package_call_graph(sources);
    let mut capabilities = BTreeMap::new();
    let mut best_chain_lengths = BTreeMap::new();

    if let Some(review) = manifest.review.as_ref() {
        for binding in &review.capability_bindings {
            propagate_package_capability(
                &call_graph,
                &mut capabilities,
                &mut best_chain_lengths,
                PackageCapabilitySeed {
                    binding_symbol: binding.symbol.clone(),
                    category: binding.category.clone(),
                    provider: binding.provider.clone(),
                    service: binding.service.clone(),
                    action: binding.action.clone(),
                    resource: binding.resource.clone(),
                    span: call_graph.function_spans.get(&binding.symbol).cloned(),
                    unknown_reason: None,
                },
            );
        }
    }

    let bound_symbols = manifest
        .review
        .as_ref()
        .map(|review| {
            review
                .capability_bindings
                .iter()
                .map(|binding| binding.symbol.as_str())
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    let mut native_contracts =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    for (name, contract) in
        collect_package_function_contracts(sources, PackageReviewFileKind::Source)
    {
        native_contracts.entry(name).or_insert(contract);
    }
    for contract in native_contracts.values() {
        if !contract.effects.contains("native") || bound_symbols.contains(contract.name.as_str()) {
            continue;
        }
        propagate_package_capability(
            &call_graph,
            &mut capabilities,
            &mut best_chain_lengths,
            PackageCapabilitySeed {
                binding_symbol: contract.name.clone(),
                category: "unknown".to_string(),
                provider: None,
                service: None,
                action: Some(contract.name.clone()),
                resource: None,
                span: Some(contract.span.clone()),
                unknown_reason: Some(
                    "native/external facade has no review.capability_bindings entry".to_string(),
                ),
            },
        );
    }

    let mut capabilities = capabilities.into_values().collect::<Vec<_>>();
    capabilities.sort_by(|left, right| {
        left.binding_symbol
            .cmp(&right.binding_symbol)
            .then_with(|| left.function.cmp(&right.function))
            .then_with(|| left.call_chain.cmp(&right.call_chain))
    });
    capabilities
}

struct PackageCapabilitySeed {
    binding_symbol: String,
    category: String,
    provider: Option<String>,
    service: Option<String>,
    action: Option<String>,
    resource: Option<String>,
    span: Option<Span>,
    unknown_reason: Option<String>,
}

fn propagate_package_capability(
    call_graph: &PackageCallGraph,
    capabilities: &mut BTreeMap<(String, String), PackageReviewCapability>,
    best_chain_lengths: &mut BTreeMap<(String, String), usize>,
    seed: PackageCapabilitySeed,
) {
    let mut queue = vec![(
        seed.binding_symbol.clone(),
        vec![seed.binding_symbol.clone()],
        seed.span.clone(),
    )];
    while let Some((function, call_chain, span)) = queue.pop() {
        let key = (seed.binding_symbol.clone(), function.clone());
        let chain_len = call_chain.len();
        if best_chain_lengths
            .get(&key)
            .is_some_and(|best_len| *best_len >= chain_len)
        {
            continue;
        }
        best_chain_lengths.insert(key.clone(), chain_len);
        capabilities.insert(
            key,
            PackageReviewCapability {
                function: function.clone(),
                binding_symbol: seed.binding_symbol.clone(),
                category: seed.category.clone(),
                provider: seed.provider.clone(),
                service: seed.service.clone(),
                action: seed.action.clone(),
                resource: seed.resource.clone(),
                call_chain: call_chain.clone(),
                span,
                unknown_reason: seed.unknown_reason.clone(),
            },
        );
        if let Some(callers) = call_graph.reverse_calls.get(&function) {
            for caller in callers.iter().rev() {
                if call_chain.contains(&caller.function) {
                    continue;
                }
                let mut caller_chain = Vec::with_capacity(call_chain.len() + 1);
                caller_chain.push(caller.function.clone());
                caller_chain.extend(call_chain.iter().cloned());
                queue.push((
                    caller.function.clone(),
                    caller_chain,
                    Some(caller.span.clone()),
                ));
            }
        }
    }
}

struct PackageCallGraph {
    reverse_calls: BTreeMap<String, Vec<PackageCallSite>>,
    function_spans: BTreeMap<String, Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageCallSite {
    function: String,
    span: Span,
}

fn collect_package_call_graph(sources: &[PackageSource]) -> PackageCallGraph {
    let mut reverse_calls = BTreeMap::new();
    let mut function_spans = BTreeMap::new();
    let relative_paths = sources
        .iter()
        .map(|source| (source.path.clone(), source.relative_path.clone()))
        .collect::<BTreeMap<_, _>>();
    let source_program = merge_programs(
        sources
            .iter()
            .filter(|source| source.kind == PackageReviewFileKind::Source)
            .map(|source| parse_source(&source.path, &source.contents)),
    );
    let interface_programs = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Interface)
        .map(|source| parse_source(&source.path, &source.contents))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&source_program, &interface_programs);

    for source in sources {
        let program = parse_source(&source.path, &source.contents);
        for item in &program.items {
            let Item::Function(function) = item else {
                continue;
            };
            function_spans.insert(
                function.name.clone(),
                relative_span(&function.span, &source.relative_path),
            );
        }
    }
    for call_site in hir.call_sites() {
        let callee = package_call_site_label(call_site);
        let span = relative_paths.get(&call_site.span.file).map_or_else(
            || call_site.span.clone(),
            |relative_path| relative_span(&call_site.span, relative_path),
        );
        reverse_calls
            .entry(callee)
            .or_insert_with(Vec::new)
            .push(PackageCallSite {
                function: call_site.function_name.clone(),
                span,
            });
    }
    for callers in reverse_calls.values_mut() {
        callers.sort_by(|left, right| {
            left.function
                .cmp(&right.function)
                .then_with(|| left.span.file.cmp(&right.span.file))
                .then_with(|| left.span.line.cmp(&right.span.line))
                .then_with(|| left.span.column.cmp(&right.span.column))
        });
        callers.dedup();
    }
    PackageCallGraph {
        reverse_calls,
        function_spans,
    }
}

fn package_call_site_label(call_site: &crate::hir::HirCallSite) -> String {
    match &call_site.resolution {
        CallResolution::Resolved { signature, .. } => match &signature.namespace {
            Some(namespace) => format!("{namespace}.{}", signature.name),
            None => signature.name.clone(),
        },
        CallResolution::EnumVariant
        | CallResolution::Ambiguous { .. }
        | CallResolution::Unknown => callee_label(&call_site.callee),
    }
}

fn relative_span(span: &Span, relative_path: &str) -> Span {
    Span {
        file: relative_path.to_owned(),
        line: span.line,
        column: span.column,
        length: span.length,
    }
}

fn collect_package_await_sites(sources: &[PackageSource]) -> Vec<PackageReviewAwaitSite> {
    let context = collect_await_site_context(sources);
    let mut await_sites = sources
        .iter()
        .filter(|source| source.kind == PackageReviewFileKind::Source)
        .flat_map(|source| {
            let program = parse_source(&source.path, &source.contents);
            program
                .items
                .iter()
                .flat_map(|item| match item {
                    Item::Function(function) => {
                        collect_await_sites_in_block(&function.name, &function.body, &context)
                    }
                    Item::Type(type_decl) => {
                        type_decl.drop_body.as_ref().map_or_else(Vec::new, |body| {
                            collect_await_sites_in_block(
                                &format!("drop {}", type_decl.name),
                                body,
                                &context,
                            )
                        })
                    }
                    Item::Module(_)
                    | Item::Use(_)
                    | Item::SumType(_)
                    | Item::TypeAlias(_)
                    | Item::Const(_) => Vec::new(),
                })
                .map(|mut site| {
                    site.span.file = source.relative_path.clone();
                    site
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    await_sites.sort_by(|left, right| {
        left.span
            .file
            .cmp(&right.span.file)
            .then_with(|| left.span.line.cmp(&right.span.line))
            .then_with(|| left.span.column.cmp(&right.span.column))
            .then_with(|| left.function.cmp(&right.function))
    });
    await_sites
}

struct AwaitSiteContext {
    async_native_callees: BTreeSet<String>,
    async_rss_callees: BTreeSet<String>,
}

fn collect_await_site_context(sources: &[PackageSource]) -> AwaitSiteContext {
    let mut context = AwaitSiteContext {
        async_native_callees: BTreeSet::new(),
        async_rss_callees: BTreeSet::new(),
    };
    for source in sources {
        let program = parse_source(&source.path, &source.contents);
        for item in &program.items {
            let Item::Function(function) = item else {
                continue;
            };
            if !function.is_async {
                continue;
            }
            if function.is_native {
                context.async_native_callees.insert(function.name.clone());
            } else {
                context.async_rss_callees.insert(function.name.clone());
            }
        }
    }
    context
}

fn collect_await_sites_in_block(
    function: &str,
    block: &Block,
    context: &AwaitSiteContext,
) -> Vec<PackageReviewAwaitSite> {
    let mut sites = Vec::new();
    collect_await_sites_from_block(
        function,
        block,
        &BTreeSet::new(),
        &BTreeSet::new(),
        &BTreeMap::new(),
        context,
        &mut sites,
    );
    sites
}

fn collect_await_sites_from_stmt(
    function: &str,
    statement: &Stmt,
    live_after: &BTreeSet<String>,
    scoped_live: &BTreeSet<String>,
    pending_callees: &BTreeMap<String, String>,
    context: &AwaitSiteContext,
    sites: &mut Vec<PackageReviewAwaitSite>,
) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_await_sites_from_expr(
                    function,
                    value,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_await_sites_from_expr(
                    function,
                    value,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::With(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.resource,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            let mut body_scoped_live = scoped_live.clone();
            body_scoped_live.insert(stmt.binding.clone());
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                &body_scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::If(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.condition,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_block(
                function,
                &stmt.then_body,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            if let Some(else_body) = &stmt.else_body {
                collect_await_sites_from_block(
                    function,
                    else_body,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_await_sites_from_expr(
                    function,
                    condition,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::For(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.iterable,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            let mut body_scoped_live = scoped_live.clone();
            body_scoped_live.insert(stmt.binding.clone());
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                &body_scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::TaskGroup(stmt) => {
            let mut task_group_pending_callees = pending_callees.clone();
            collect_task_group_async_let_callees(&stmt.body, &mut task_group_pending_callees);
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                scoped_live,
                &task_group_pending_callees,
                context,
                sites,
            );
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_await_sites_from_expr(
                    function,
                    &arm.operation,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
                let mut arm_scoped_live = scoped_live.clone();
                if arm.binding != "_" {
                    arm_scoped_live.insert(arm.binding.clone());
                }
                collect_await_sites_from_block(
                    function,
                    &arm.body,
                    live_after,
                    &arm_scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::Match(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.value,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            for arm in &stmt.arms {
                let mut arm_scoped_live = scoped_live.clone();
                for binding in arm.pattern.binding_names() {
                    arm_scoped_live.insert(binding.to_string());
                }
                collect_await_sites_from_block(
                    function,
                    &arm.body,
                    live_after,
                    &arm_scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Stmt::LetElse(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.value,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_block(
                function,
                &stmt.else_body,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::Assign(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.target,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_expr(
                function,
                &stmt.value,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Stmt::Expr(expr) => collect_await_sites_from_expr(
            function,
            expr,
            live_after,
            scoped_live,
            pending_callees,
            context,
            sites,
        ),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

fn collect_await_sites_from_block(
    function: &str,
    block: &Block,
    continuation_uses: &BTreeSet<String>,
    scoped_live: &BTreeSet<String>,
    pending_callees: &BTreeMap<String, String>,
    context: &AwaitSiteContext,
    sites: &mut Vec<PackageReviewAwaitSite>,
) {
    let live_after_statements = block_live_after_statements(block, continuation_uses);
    for (index, statement) in block.statements.iter().enumerate() {
        let live_after = live_after_statements
            .get(index)
            .unwrap_or(continuation_uses);
        collect_await_sites_from_stmt(
            function,
            statement,
            live_after,
            scoped_live,
            pending_callees,
            context,
            sites,
        );
    }
}

fn collect_await_sites_from_expr(
    function: &str,
    expr: &Expr,
    live_after: &BTreeSet<String>,
    scoped_live: &BTreeSet<String>,
    pending_callees: &BTreeMap<String, String>,
    context: &AwaitSiteContext,
    sites: &mut Vec<PackageReviewAwaitSite>,
) {
    match expr {
        Expr::Await { value, span } => {
            let mut live_across_await = scoped_live.clone();
            live_across_await.extend(live_after.iter().cloned());
            collect_expr_uses(value, &mut live_across_await);
            let callee = awaited_callee(value, pending_callees);
            sites.push(PackageReviewAwaitSite {
                function: function.to_string(),
                boundary: await_boundary(callee.as_deref(), context),
                callee,
                live_across_await: live_across_await.into_iter().collect(),
                span: span.clone(),
            });
            collect_await_sites_from_expr(
                function,
                value,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Try { value, .. } => collect_await_sites_from_expr(
            function,
            value,
            live_after,
            scoped_live,
            pending_callees,
            context,
            sites,
        ),
        Expr::Binary { left, right, .. } => {
            let mut left_live_after = live_after.clone();
            collect_expr_uses(right, &mut left_live_after);
            collect_await_sites_from_expr(
                function,
                left,
                &left_live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_expr(
                function,
                right,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Expr::Field { base, .. } => collect_await_sites_from_expr(
            function,
            base,
            live_after,
            scoped_live,
            pending_callees,
            context,
            sites,
        ),
        Expr::Index { base, index, .. } => {
            let mut base_live_after = live_after.clone();
            collect_expr_uses(index, &mut base_live_after);
            collect_await_sites_from_expr(
                function,
                base,
                &base_live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            collect_await_sites_from_expr(
                function,
                index,
                live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
        }
        Expr::Call { args, .. } => {
            let mut arg_live_after = live_after.clone();
            for arg in args.iter().rev() {
                collect_await_sites_from_expr(
                    function,
                    &arg.value,
                    &arg_live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
                collect_expr_uses(&arg.value, &mut arg_live_after);
            }
        }
        Expr::Closure { body, .. } => collect_await_sites_from_block(
            function,
            body,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &BTreeMap::new(),
            context,
            sites,
        ),
        Expr::Match { value, arms, .. } => {
            let mut value_live_after = live_after.clone();
            for arm in arms {
                collect_block_uses(&arm.body, &mut value_live_after);
            }
            collect_await_sites_from_expr(
                function,
                value,
                &value_live_after,
                scoped_live,
                pending_callees,
                context,
                sites,
            );
            for arm in arms {
                collect_await_sites_from_block(
                    function,
                    &arm.body,
                    live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
            }
        }
        Expr::MapLiteral { entries, .. } => {
            let mut entry_live_after = live_after.clone();
            for entry in entries.iter().rev() {
                collect_await_sites_from_expr(
                    function,
                    &entry.value,
                    &entry_live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
                collect_expr_uses(&entry.value, &mut entry_live_after);
                collect_await_sites_from_expr(
                    function,
                    &entry.key,
                    &entry_live_after,
                    scoped_live,
                    pending_callees,
                    context,
                    sites,
                );
                collect_expr_uses(&entry.key, &mut entry_live_after);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn block_live_after_statements(
    block: &Block,
    continuation_uses: &BTreeSet<String>,
) -> Vec<BTreeSet<String>> {
    let mut live_after = vec![BTreeSet::new(); block.statements.len()];
    let mut used = continuation_uses.clone();
    for (index, statement) in block.statements.iter().enumerate().rev() {
        live_after[index] = used.clone();
        collect_stmt_uses(statement, &mut used);
        remove_stmt_bindings(statement, &mut used);
    }
    live_after
}

fn collect_task_group_async_let_callees(
    block: &Block,
    pending_callees: &mut BTreeMap<String, String>,
) {
    for statement in &block.statements {
        if let Stmt::Let(stmt) = statement
            && stmt.is_async
            && let Some(value) = &stmt.value
            && let Some(callee) = awaited_callee(value, pending_callees)
        {
            pending_callees.insert(stmt.name.clone(), callee);
        }
    }
}

fn collect_stmt_uses(statement: &Stmt, uses: &mut BTreeSet<String>) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_uses(value, uses);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_uses(value, uses);
            }
        }
        Stmt::With(stmt) => {
            collect_expr_uses(&stmt.resource, uses);
            collect_block_uses(&stmt.body, uses);
        }
        Stmt::If(stmt) => {
            collect_expr_uses(&stmt.condition, uses);
            collect_block_uses(&stmt.then_body, uses);
            if let Some(else_body) = &stmt.else_body {
                collect_block_uses(else_body, uses);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_expr_uses(condition, uses);
            }
            collect_block_uses(&stmt.body, uses);
        }
        Stmt::For(stmt) => {
            collect_expr_uses(&stmt.iterable, uses);
            collect_block_uses(&stmt.body, uses);
        }
        Stmt::TaskGroup(stmt) => {
            collect_block_uses(&stmt.body, uses);
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_expr_uses(&arm.operation, uses);
                collect_block_uses(&arm.body, uses);
            }
        }
        Stmt::Match(stmt) => {
            collect_expr_uses(&stmt.value, uses);
            for arm in &stmt.arms {
                collect_block_uses(&arm.body, uses);
            }
        }
        Stmt::LetElse(stmt) => {
            collect_expr_uses(&stmt.value, uses);
            collect_block_uses(&stmt.else_body, uses);
        }
        Stmt::Assign(stmt) => {
            collect_expr_uses(&stmt.target, uses);
            collect_expr_uses(&stmt.value, uses);
        }
        Stmt::Expr(expr) => collect_expr_uses(expr, uses),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

fn collect_block_uses(block: &Block, uses: &mut BTreeSet<String>) {
    let mut block_uses = BTreeSet::new();
    for statement in block.statements.iter().rev() {
        collect_stmt_uses(statement, &mut block_uses);
        remove_stmt_bindings(statement, &mut block_uses);
    }
    uses.extend(block_uses);
}

fn collect_expr_uses(expr: &Expr, uses: &mut BTreeSet<String>) {
    match expr {
        Expr::Ident(name, _) => {
            if !is_builtin_value_ident(name) {
                uses.insert(name.clone());
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_expr_uses(left, uses);
            collect_expr_uses(right, uses);
        }
        Expr::Field { base, .. } => collect_expr_uses(base, uses),
        Expr::Index { base, index, .. } => {
            collect_expr_uses(base, uses);
            collect_expr_uses(index, uses);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_uses(&arg.value, uses);
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => collect_expr_uses(value, uses),
        Expr::Closure { body, .. } => collect_block_uses(body, uses),
        Expr::Match { value, arms, .. } => {
            collect_expr_uses(value, uses);
            for arm in arms {
                collect_block_uses(&arm.body, uses);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_uses(&entry.key, uses);
                collect_expr_uses(&entry.value, uses);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn is_builtin_value_ident(name: &str) -> bool {
    matches!(name, "Unit" | "true" | "false")
}

fn await_boundary(callee: Option<&str>, context: &AwaitSiteContext) -> PackageReviewAwaitBoundary {
    let Some(callee) = callee else {
        return PackageReviewAwaitBoundary::Unknown;
    };
    if runtime_intrinsic_label(callee) {
        return PackageReviewAwaitBoundary::RuntimePending;
    }
    if context.async_native_callees.contains(callee) {
        return PackageReviewAwaitBoundary::NativePending;
    }
    if context.async_rss_callees.contains(callee) {
        return PackageReviewAwaitBoundary::RssCall;
    }
    PackageReviewAwaitBoundary::Unknown
}

fn runtime_intrinsic_label(callee: &str) -> bool {
    let Some((namespace, name)) = callee.rsplit_once('.') else {
        return false;
    };
    runtime_abi::lookup_runtime_intrinsic(namespace, name).is_some()
}

fn remove_stmt_bindings(statement: &Stmt, uses: &mut BTreeSet<String>) {
    match statement {
        Stmt::Let(stmt) => {
            uses.remove(&stmt.name);
        }
        Stmt::With(stmt) => {
            uses.remove(&stmt.binding);
        }
        Stmt::For(stmt) => {
            uses.remove(&stmt.binding);
        }
        Stmt::TaskGroup(_) => {}
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                if arm.binding != "_" {
                    uses.remove(&arm.binding);
                }
            }
        }
        Stmt::Match(stmt) => {
            for arm in &stmt.arms {
                for binding in arm.pattern.binding_names() {
                    uses.remove(binding);
                }
            }
        }
        Stmt::LetElse(stmt) => {
            if !stmt.binding_name.is_empty() {
                uses.remove(&stmt.binding_name);
            }
        }
        Stmt::Assign(_)
        | Stmt::Return(_)
        | Stmt::If(_)
        | Stmt::Loop(_)
        | Stmt::Expr(_)
        | Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

fn awaited_callee(expr: &Expr, pending_callees: &BTreeMap<String, String>) -> Option<String> {
    match expr {
        Expr::Call { callee, .. } => Some(callee_label(callee)),
        Expr::Ident(name, _) => pending_callees.get(name).cloned(),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => {
            awaited_callee(value, pending_callees)
        }
        _ => None,
    }
}

fn callee_label(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => format!(
            "{} {}.{method}",
            effect.as_str(),
            package_expr_label(receiver)
        ),
    }
}

fn package_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::MultilineString(value, _) => format!("{value:?}"),
        Expr::Field { base, name, .. } => format!("{}.{}", package_expr_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", package_expr_label(base)),
        Expr::Call { callee, .. } => format!("{}()", callee_label(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            package_expr_label(value)
        }
        _ => "<expr>".to_string(),
    }
}

fn is_guarantee_effect(effect: &str) -> bool {
    matches!(effect, "no_panic" | "noalloc" | "no_block" | "pure")
}

fn package_unknown_api_count(
    contracts: &BTreeMap<String, PackageFunctionContract>,
    review_map: &ReviewMap,
) -> usize {
    contracts
        .keys()
        .filter(|function| {
            review_map.files.iter().any(|file| {
                file.regions.iter().any(|region| {
                    &region.function == *function
                        && region.classification == ReviewMapClassification::Unknown
                })
            })
        })
        .count()
}

fn package_risk(
    manifest: &Manifest,
    native: Option<&PackageNativeRustReview>,
    review_map: &ReviewMap,
    diagnostics: &[Diagnostic],
    native_apis: usize,
    unknown_capability_bindings: usize,
) -> PackageRisk {
    if manifest
        .review
        .as_ref()
        .and_then(|review| review.expect.risk.as_deref())
        == Some("unknown")
        || review_map.summary.unknown.functions > 0
        || unknown_capability_bindings > 0
    {
        return PackageRisk::Unknown;
    }
    if diagnostics
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        return PackageRisk::High;
    }
    if manifest
        .features
        .iter()
        .any(|(name, values)| package_feature_may_change_boundary_risk(name, values))
    {
        return PackageRisk::High;
    }
    if let Some(native) = native
        && (native
            .build_scripts
            .as_deref()
            .is_some_and(|policy| policy != "forbid")
            || native
                .proc_macros
                .as_deref()
                .is_some_and(|policy| policy != "forbid")
            || native
                .unsafe_policy
                .as_deref()
                .is_some_and(|policy| policy != "forbid")
            || (!native.links.is_empty()
                && native
                    .native_links_policy
                    .as_deref()
                    .is_none_or(|policy| policy != "allow"))
            || (native.semantic.source_scan_best_effort.ffi_detected
                && native
                    .ffi_policy
                    .as_deref()
                    .is_none_or(|policy| policy != "allow")))
    {
        return PackageRisk::High;
    }
    if native_apis > 0 {
        return package_native_api_risk(manifest);
    }
    if native.is_some() || review_map.summary.review_required.functions > 0 {
        return PackageRisk::Elevated;
    }
    PackageRisk::Low
}

fn package_native_api_risk(manifest: &Manifest) -> PackageRisk {
    match manifest
        .review
        .as_ref()
        .and_then(|review| review.policy.native_api_risk.as_deref())
    {
        Some("high") => PackageRisk::High,
        Some("elevated") => PackageRisk::Elevated,
        _ => PackageRisk::High,
    }
}
