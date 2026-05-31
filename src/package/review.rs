use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::analyzer::{
    analyze_source_with_interfaces, analyze_sources_with_interfaces, core_interfaces,
};
use crate::diagnostic::{Diagnostic, code};
use crate::lint::lint_source;
use crate::review::{ReviewMap, ReviewMapClassification, review_map_sources_with_interfaces};
use crate::runtime_abi;
use crate::syntax::ast::{Block, Callee, Expr, Item, MatchPattern, Stmt, TypeKind};
use crate::syntax::parse_source;

use super::contract::{
    PackageFunctionContract, collect_package_function_contracts, collect_package_type_contracts,
    package_contract_has_resource_boundary, package_interface_contract_diagnostics,
    package_interface_diagnostic_exports, package_interface_environment_diagnostics,
    package_review_exports,
};
use super::native::{
    native_binding_interface_sources, package_native_binding_diagnostics, package_native_bindings,
    package_native_rust_review,
};
use super::source_set::{Manifest, PackageSource, load_package};
use super::{
    PackageNativeRustReview, PackageProviderImplementation, PackageReview,
    PackageReviewAwaitBoundary, PackageReviewAwaitSite, PackageReviewFile, PackageReviewFileKind,
    PackageReviewSummary, PackageRisk, collect_dependency_interface_sources, dedup_diagnostics,
    package_feature_may_change_boundary_risk, package_feature_resolution_diagnostics,
};

pub fn review_package_dir(package_dir: &Path) -> Result<PackageReview, String> {
    let package = load_package(package_dir)?;
    let manifest = &package.manifest;
    let sources = &package.sources;
    let dependency_interfaces = collect_dependency_interface_sources(package_dir, manifest)?;
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
    let interface_frontend_diagnostics = interface_refs
        .iter()
        .flat_map(|(path, contents)| {
            analyze_source_with_interfaces(path, contents, &contract_external_interfaces)
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
    diagnostics.extend(interface_frontend_diagnostics);
    diagnostics.extend(analyze_sources_with_interfaces(
        &source_refs,
        &source_interfaces,
    ));
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

    let mut reasons = Vec::new();
    collect_manifest_review_reasons(manifest, &mut reasons);
    collect_native_reasons(native_rust.as_ref(), &mut reasons);
    collect_review_map_reasons(&review_map, &mut reasons);
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
        unknown_apis: api_summary.unknown_apis + interface_diagnostic_exports.len(),
    };
    let files = sources
        .iter()
        .map(|source| PackageReviewFile {
            path: source.path.clone(),
            kind: source.kind,
        })
        .collect();
    let features = manifest.features.keys().cloned().collect::<Vec<_>>();
    let implements = package_provider_implementations(manifest);
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
        implements,
        summary,
        files,
        exports,
        await_sites,
        native_rust,
        review_map,
        diagnostics,
    })
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PackageApiSummary {
    public_types: usize,
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
    let source_contracts;
    let source_type_contracts;
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
    let resource_types = type_contracts
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();

    PackageApiSummary {
        public_types: type_contracts.len(),
        public_functions: contracts.len(),
        public_apis: type_contracts.len() + contracts.len(),
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
                    Item::Module(_) | Item::Use(_) | Item::SumType(_) | Item::TypeAlias(_) | Item::Const(_) => Vec::new(),
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
                context,
                sites,
            );
            collect_await_sites_from_block(
                function,
                &stmt.then_body,
                live_after,
                scoped_live,
                context,
                sites,
            );
            if let Some(else_body) = &stmt.else_body {
                collect_await_sites_from_block(
                    function,
                    else_body,
                    live_after,
                    scoped_live,
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
                    context,
                    sites,
                );
            }
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                scoped_live,
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
                context,
                sites,
            );
        }
        Stmt::TaskGroup(stmt) => {
            collect_await_sites_from_block(
                function,
                &stmt.body,
                live_after,
                scoped_live,
                context,
                sites,
            );
        }
        Stmt::Match(stmt) => {
            collect_await_sites_from_expr(
                function,
                &stmt.value,
                live_after,
                scoped_live,
                context,
                sites,
            );
            for arm in &stmt.arms {
                let mut arm_scoped_live = scoped_live.clone();
                if let MatchPattern::Variant {
                    binding: Some(binding),
                    ..
                } = &arm.pattern
                {
                    arm_scoped_live.insert(binding.clone());
                }
                collect_await_sites_from_block(
                    function,
                    &arm.body,
                    live_after,
                    &arm_scoped_live,
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
                context,
                sites,
            );
            collect_await_sites_from_block(
                function,
                &stmt.else_body,
                live_after,
                scoped_live,
                context,
                sites,
            );
        }
        Stmt::Expr(expr) => {
            collect_await_sites_from_expr(function, expr, live_after, scoped_live, context, sites)
        }
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
    context: &AwaitSiteContext,
    sites: &mut Vec<PackageReviewAwaitSite>,
) {
    let live_after_statements = block_live_after_statements(block, continuation_uses);
    for (index, statement) in block.statements.iter().enumerate() {
        let live_after = live_after_statements
            .get(index)
            .unwrap_or(continuation_uses);
        collect_await_sites_from_stmt(function, statement, live_after, scoped_live, context, sites);
    }
}

fn collect_await_sites_from_expr(
    function: &str,
    expr: &Expr,
    live_after: &BTreeSet<String>,
    scoped_live: &BTreeSet<String>,
    context: &AwaitSiteContext,
    sites: &mut Vec<PackageReviewAwaitSite>,
) {
    match expr {
        Expr::Await { value, span } => {
            let mut live_across_await = scoped_live.clone();
            live_across_await.extend(live_after.iter().cloned());
            collect_expr_uses(value, &mut live_across_await);
            let callee = awaited_callee(value);
            sites.push(PackageReviewAwaitSite {
                function: function.to_string(),
                boundary: await_boundary(callee.as_deref(), context),
                callee,
                live_across_await: live_across_await.into_iter().collect(),
                span: span.clone(),
            });
            collect_await_sites_from_expr(function, value, live_after, scoped_live, context, sites);
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Try { value, .. } => {
            collect_await_sites_from_expr(function, value, live_after, scoped_live, context, sites)
        }
        Expr::Binary { left, right, .. } => {
            let mut left_live_after = live_after.clone();
            collect_expr_uses(right, &mut left_live_after);
            collect_await_sites_from_expr(
                function,
                left,
                &left_live_after,
                scoped_live,
                context,
                sites,
            );
            collect_await_sites_from_expr(function, right, live_after, scoped_live, context, sites);
        }
        Expr::Field { base, .. } => {
            collect_await_sites_from_expr(function, base, live_after, scoped_live, context, sites)
        }
        Expr::Index { base, index, .. } => {
            let mut base_live_after = live_after.clone();
            collect_expr_uses(index, &mut base_live_after);
            collect_await_sites_from_expr(
                function,
                base,
                &base_live_after,
                scoped_live,
                context,
                sites,
            );
            collect_await_sites_from_expr(function, index, live_after, scoped_live, context, sites);
        }
        Expr::Call { args, .. } => {
            let mut arg_live_after = live_after.clone();
            for arg in args.iter().rev() {
                collect_await_sites_from_expr(
                    function,
                    &arg.value,
                    &arg_live_after,
                    scoped_live,
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
            context,
            sites,
        ),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
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
        Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
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
        Stmt::Match(stmt) => {
            for arm in &stmt.arms {
                if let MatchPattern::Variant {
                    binding: Some(binding),
                    ..
                } = &arm.pattern
                {
                    uses.remove(binding);
                }
            }
        }
        Stmt::LetElse(stmt) => {
            if !stmt.binding_name.is_empty() {
                uses.remove(&stmt.binding_name);
            }
        }
        Stmt::Return(_)
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

fn awaited_callee(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Call { callee, .. } => Some(callee_label(callee)),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => awaited_callee(value),
        _ => None,
    }
}

fn callee_label(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
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
) -> PackageRisk {
    if manifest
        .review
        .as_ref()
        .and_then(|review| review.expect.risk.as_deref())
        == Some("unknown")
        || review_map.summary.unknown.functions > 0
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
