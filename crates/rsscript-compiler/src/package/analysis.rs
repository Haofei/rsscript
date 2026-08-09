use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use crate::analyzer::{
    analyze_source_with_interfaces_result, analyze_sources_with_interfaces_without_core_result,
    core_interfaces,
};
use crate::diagnostic::{Diagnostic, Span, code};
use crate::hir::CallResolution;
use crate::lint::lint_source;
use crate::syntax::ast::TypeKind;

use super::analysis_await::collect_package_await_sites;
use super::contract::{
    collect_package_const_contracts, collect_package_function_contracts,
    collect_package_protocol_contracts, collect_package_protocol_impl_contracts,
    collect_package_sum_type_contracts, collect_package_type_alias_contracts,
    collect_package_type_contracts, package_contract_has_resource_boundary,
    package_interface_contract_diagnostics, package_interface_diagnostic_exports,
    package_interface_environment_diagnostics,
};
use super::dependency::{
    collect_dependency_interface_sources, collect_dependency_interface_sources_for_tests,
    package_feature_resolution_diagnostics,
};
use super::source_set::{PackageSource, load_package};
use super::{
    PACKAGE_ANALYSIS_SCHEMA, PackageAnalysis, PackageAnalysisAwaitSite, PackageAnalysisExport,
    PackageAnalysisExternalImport, PackageAnalysisFile, PackageAnalysisParameter,
    PackageAnalysisProducer, PackageAnalysisSummary, PackageReviewFileKind, dedup_diagnostics,
    package_identity,
};

/// Analyze one already-captured package graph without consulting review policy,
/// provider metadata, native implementations, or deployment evidence.
pub(super) fn analyze_package_dir_captured(package_dir: &Path) -> Result<PackageAnalysis, String> {
    let package = load_package(package_dir)?;
    let sources = &package.sources;
    let dependency_interfaces =
        collect_dependency_interface_sources(package_dir, &package.manifest)?;
    let test_dependency_interfaces =
        collect_dependency_interface_sources_for_tests(package_dir, &package.manifest)?;
    let interface_refs = source_refs(sources, PackageReviewFileKind::Interface);
    let program_refs = source_refs(sources, PackageReviewFileKind::Source);
    let test_refs = source_refs(sources, PackageReviewFileKind::Test);
    let dependency_interface_refs = dependency_interfaces
        .iter()
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect::<Vec<_>>();

    let mut source_interfaces = core_interfaces().to_vec();
    source_interfaces.extend(dependency_interface_refs.iter().copied());
    let mut combined_interfaces = dependency_interface_refs.clone();
    combined_interfaces.extend(interface_refs.iter().copied());

    let interface_frontend_diagnostics = interface_refs
        .iter()
        .flat_map(|(path, contents)| {
            let mut visible_interfaces = dependency_interface_refs.clone();
            visible_interfaces.extend(
                interface_refs
                    .iter()
                    .filter(|(interface_path, _)| interface_path != path)
                    .copied(),
            );
            analyze_source_with_interfaces_result(path, contents, &visible_interfaces)
                .into_diagnostics()
        })
        .collect::<Vec<_>>();
    let interface_diagnostic_exports =
        package_interface_diagnostic_exports(sources, &interface_frontend_diagnostics);

    let mut diagnostics = package_interface_environment_diagnostics(&combined_interfaces);
    diagnostics.extend(package_feature_resolution_diagnostics(
        package_dir,
        &package.manifest,
    )?);
    diagnostics.extend(interface_frontend_diagnostics);
    diagnostics.extend(
        analyze_sources_with_interfaces_without_core_result(&program_refs, &source_interfaces)
            .into_diagnostics(),
    );
    if !test_refs.is_empty() {
        let mut test_interfaces = source_interfaces.clone();
        let mut seen = test_interfaces
            .iter()
            .map(|(path, _)| (*path).to_string())
            .collect::<BTreeSet<_>>();
        test_interfaces.extend(
            test_dependency_interfaces
                .iter()
                .map(|source| (source.path.as_str(), source.contents.as_str()))
                .filter(|(path, _)| seen.insert((*path).to_string())),
        );
        if program_refs.is_empty() {
            test_interfaces.extend(interface_refs.iter().copied());
        } else {
            test_interfaces.extend(program_refs.iter().copied());
        }
        diagnostics.extend(
            analyze_sources_with_interfaces_without_core_result(&test_refs, &test_interfaces)
                .into_diagnostics(),
        );
    }
    diagnostics.extend(package_interface_contract_diagnostics(
        sources,
        &BTreeMap::new(),
    ));
    diagnostics.extend(package_lint_diagnostics(sources));
    dedup_diagnostics(&mut diagnostics);

    let semantic_analysis =
        analyze_sources_with_interfaces_without_core_result(&program_refs, &source_interfaces);
    diagnostics.extend(
        semantic_analysis
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == code::ANALYSIS_INCOMPLETE)
            .cloned(),
    );
    dedup_diagnostics(&mut diagnostics);

    let database = semantic_analysis.database();
    let review_await_sites = collect_package_await_sites(sources, database);
    let await_sites = review_await_sites
        .iter()
        .map(|site| PackageAnalysisAwaitSite {
            function: site.function.clone(),
            callee: site.callee.clone(),
            live_across_await: site.live_across_await.clone(),
            span: site.span.clone(),
        })
        .collect::<Vec<_>>();
    let mut exports = package_analysis_exports(sources);
    exports.extend(
        interface_diagnostic_exports
            .into_iter()
            .map(|export| PackageAnalysisExport {
                name: export.name,
                kind: export.kind,
                function_kind: export.function_kind,
                parameters: Vec::new(),
                return_type: None,
                retained_params: export.retained_params,
                semantic_facts: export.reasons,
            }),
    );
    exports.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });

    let summary = package_analysis_summary(sources, await_sites.len(), &diagnostics);
    Ok(PackageAnalysis {
        schema: PACKAGE_ANALYSIS_SCHEMA.to_string(),
        producer: PackageAnalysisProducer::current(),
        language_version: rsscript_abi_model::LANGUAGE_SEMANTICS_VERSION.to_string(),
        interface_catalog_digest: crate::interfaces::interface_catalog_digest(),
        snapshot_digest: String::new(),
        module_digest: None,
        package: package_identity(&package.manifest),
        files: sources
            .iter()
            .map(|source| PackageAnalysisFile {
                path: source.path.clone(),
                kind: source.kind,
            })
            .collect(),
        summary,
        exports,
        external_imports: package_external_imports(sources, database),
        await_sites,
        diagnostics,
    })
}

pub fn analyze_package_dir(package_dir: &Path) -> Result<PackageAnalysis, String> {
    super::authorization::load_workspace_snapshot(package_dir)
        .map(|snapshot| snapshot.analysis().clone())
}

fn source_refs(sources: &[PackageSource], kind: PackageReviewFileKind) -> Vec<(&str, &str)> {
    sources
        .iter()
        .filter(|source| source.kind == kind)
        .map(|source| (source.path.as_str(), source.contents.as_str()))
        .collect()
}

fn package_lint_diagnostics(sources: &[PackageSource]) -> Vec<Diagnostic> {
    sources
        .iter()
        .filter(|source| source.kind != PackageReviewFileKind::Interface)
        .flat_map(|source| lint_source(&source.path, &source.contents))
        .collect()
}

fn package_analysis_exports(sources: &[PackageSource]) -> Vec<PackageAnalysisExport> {
    let interface_types = collect_package_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_types;
    let types = if interface_types.is_empty() {
        source_types = collect_package_type_contracts(sources, PackageReviewFileKind::Source);
        &source_types
    } else {
        &interface_types
    };
    let resource_types = types
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();
    let interface_functions =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let source_functions;
    let functions = if interface_functions.is_empty() {
        source_functions =
            collect_package_function_contracts(sources, PackageReviewFileKind::Source);
        &source_functions
    } else {
        &interface_functions
    };

    let mut exports = types
        .values()
        .map(|contract| {
            let mut semantic_facts = Vec::new();
            if contract.kind == TypeKind::Resource {
                semantic_facts.push("resource type".to_string());
            }
            if contract.fields.iter().any(|field| field.is_handle) {
                semantic_facts.push("handle field".to_string());
            }
            if contract.fields.iter().any(|field| field.is_weak) {
                semantic_facts.push("weak field".to_string());
            }
            PackageAnalysisExport {
                name: contract.name.clone(),
                kind: "type".to_string(),
                function_kind: None,
                parameters: Vec::new(),
                return_type: None,
                retained_params: Vec::new(),
                semantic_facts,
            }
        })
        .collect::<Vec<_>>();
    exports.extend(functions.values().map(|contract| {
        let mut semantic_facts = Vec::new();
        if contract.is_async {
            semantic_facts.push("async boundary".to_string());
        }
        for param in &contract.params {
            if matches!(param.effect, Some("mut" | "take")) {
                semantic_facts.push(format!(
                    "{} parameter `{}`",
                    param.effect.expect("effect matched"),
                    param.name
                ));
            }
        }
        if package_contract_has_resource_boundary(contract, &resource_types) {
            semantic_facts.push("resource boundary".to_string());
        }
        if contract.returns_fresh {
            semantic_facts.push("returns fresh value".to_string());
        }
        let mut retained_params = contract.retained_params.iter().cloned().collect::<Vec<_>>();
        retained_params.sort();
        semantic_facts.extend(
            retained_params
                .iter()
                .map(|param| format!("retains({param})")),
        );
        semantic_facts.sort();
        semantic_facts.dedup();
        PackageAnalysisExport {
            name: contract.name.clone(),
            kind: "function".to_string(),
            function_kind: Some(if contract.is_async { "async" } else { "sync" }.to_string()),
            parameters: contract
                .params
                .iter()
                .map(|parameter| PackageAnalysisParameter {
                    name: parameter.name.clone(),
                    effect: parameter.effect.unwrap_or("read").to_string(),
                    ty: parameter.type_name.clone(),
                    retained: contract.retained_params.contains(&parameter.name),
                })
                .collect(),
            return_type: contract.return_type.clone(),
            retained_params,
            semantic_facts,
        }
    }));
    exports.extend(public_contract_names(
        sources,
        "sum_type",
        collect_package_sum_type_contracts,
    ));
    exports.extend(public_contract_names(
        sources,
        "type_alias",
        collect_package_type_alias_contracts,
    ));
    exports.extend(public_contract_names(
        sources,
        "const",
        collect_package_const_contracts,
    ));
    exports.extend(public_contract_names(
        sources,
        "protocol_impl",
        collect_package_protocol_impl_contracts,
    ));
    exports.extend(public_contract_names(
        sources,
        "protocol",
        collect_package_protocol_contracts,
    ));
    exports.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.name.cmp(&right.name))
    });
    exports
}

fn public_contract_names<T>(
    sources: &[PackageSource],
    kind: &str,
    collect: fn(&[PackageSource], PackageReviewFileKind) -> BTreeMap<String, T>,
) -> Vec<PackageAnalysisExport> {
    let interfaces = collect(sources, PackageReviewFileKind::Interface);
    let contracts = if interfaces.is_empty() {
        collect(sources, PackageReviewFileKind::Source)
    } else {
        interfaces
    };
    contracts
        .into_keys()
        .map(|name| PackageAnalysisExport {
            name,
            kind: kind.to_string(),
            function_kind: None,
            parameters: Vec::new(),
            return_type: None,
            retained_params: Vec::new(),
            semantic_facts: Vec::new(),
        })
        .collect()
}

fn package_analysis_summary(
    sources: &[PackageSource],
    await_sites: usize,
    diagnostics: &[Diagnostic],
) -> PackageAnalysisSummary {
    let interface_functions =
        collect_package_function_contracts(sources, PackageReviewFileKind::Interface);
    let source_functions;
    let functions = if interface_functions.is_empty() {
        source_functions =
            collect_package_function_contracts(sources, PackageReviewFileKind::Source);
        &source_functions
    } else {
        &interface_functions
    };
    let interface_types = collect_package_type_contracts(sources, PackageReviewFileKind::Interface);
    let source_types;
    let types = if interface_types.is_empty() {
        source_types = collect_package_type_contracts(sources, PackageReviewFileKind::Source);
        &source_types
    } else {
        &interface_types
    };
    let resource_types = types
        .values()
        .filter(|contract| contract.kind == TypeKind::Resource)
        .map(|contract| contract.name.as_str())
        .collect::<BTreeSet<_>>();
    PackageAnalysisSummary {
        interface_files: sources
            .iter()
            .filter(|source| source.kind == PackageReviewFileKind::Interface)
            .count(),
        source_files: sources
            .iter()
            .filter(|source| source.kind == PackageReviewFileKind::Source)
            .count(),
        public_types: types.len(),
        public_sum_types: public_contract_count(sources, collect_package_sum_type_contracts),
        public_type_aliases: public_contract_count(sources, collect_package_type_alias_contracts),
        public_consts: public_contract_count(sources, collect_package_const_contracts),
        public_functions: functions.len(),
        mutating_apis: functions
            .values()
            .filter(|contract| {
                contract
                    .params
                    .iter()
                    .any(|param| param.effect == Some("mut"))
            })
            .count(),
        retaining_apis: functions
            .values()
            .filter(|contract| !contract.retained_params.is_empty())
            .count(),
        resource_apis: functions
            .values()
            .filter(|contract| package_contract_has_resource_boundary(contract, &resource_types))
            .count(),
        fresh_returning_apis: functions
            .values()
            .filter(|contract| contract.returns_fresh)
            .count(),
        async_apis: functions
            .values()
            .filter(|contract| contract.is_async)
            .count(),
        await_sites,
        diagnostics: diagnostics.len(),
        errors: diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity.is_error())
            .count(),
    }
}

fn public_contract_count<T>(
    sources: &[PackageSource],
    collect: fn(&[PackageSource], PackageReviewFileKind) -> BTreeMap<String, T>,
) -> usize {
    let interfaces = collect(sources, PackageReviewFileKind::Interface);
    if interfaces.is_empty() {
        collect(sources, PackageReviewFileKind::Source).len()
    } else {
        interfaces.len()
    }
}

fn package_external_imports(
    sources: &[PackageSource],
    database: &crate::semantic::SemanticDatabase,
) -> Vec<PackageAnalysisExternalImport> {
    #[derive(Clone)]
    struct Caller {
        function: String,
        span: Span,
    }

    let relative_paths = sources
        .iter()
        .map(|source| (source.path.clone(), source.relative_path.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut reverse_calls: BTreeMap<String, Vec<Caller>> = BTreeMap::new();
    let mut external_symbols = BTreeSet::new();
    for call_site in database.hir().call_sites() {
        let CallResolution::Resolved { signature, .. } = &call_site.resolution else {
            continue;
        };
        let symbol = match &signature.namespace {
            Some(namespace) => format!("{namespace}.{}", signature.name),
            None => signature.name.clone(),
        };
        if signature.is_external {
            external_symbols.insert(symbol.clone());
        }
        let mut span = call_site.span.clone();
        if let Some(relative) = relative_paths.get(&span.file) {
            span.file = relative.clone();
        }
        reverse_calls.entry(symbol).or_default().push(Caller {
            function: call_site.function_name.clone(),
            span,
        });
    }
    for callers in reverse_calls.values_mut() {
        callers.sort_by(|left, right| left.function.cmp(&right.function));
    }

    let mut imports = BTreeMap::new();
    for symbol in external_symbols {
        let mut queue = VecDeque::from([(symbol.clone(), vec![symbol.clone()])]);
        while let Some((callee, chain)) = queue.pop_front() {
            let Some(callers) = reverse_calls.get(&callee) else {
                continue;
            };
            for caller in callers {
                if chain.contains(&caller.function) {
                    continue;
                }
                let mut caller_chain = Vec::with_capacity(chain.len() + 1);
                caller_chain.push(caller.function.clone());
                caller_chain.extend(chain.iter().cloned());
                let key = (symbol.clone(), caller.function.clone());
                let replace =
                    imports
                        .get(&key)
                        .is_none_or(|existing: &PackageAnalysisExternalImport| {
                            caller_chain.len() < existing.call_chain.len()
                        });
                if replace {
                    imports.insert(
                        key,
                        PackageAnalysisExternalImport {
                            function: caller.function.clone(),
                            symbol: symbol.clone(),
                            call_chain: caller_chain.clone(),
                            span: Some(caller.span.clone()),
                        },
                    );
                    queue.push_back((caller.function.clone(), caller_chain));
                }
            }
        }
    }
    imports.into_values().collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn neutral_analysis_does_not_require_review_metadata_for_external_imports() {
        let root = tempfile::tempdir().expect("package fixture");
        fs::create_dir_all(root.path().join("src")).expect("source directory");
        fs::create_dir_all(root.path().join("deps/clock/interfaces")).expect("interface directory");
        fs::write(
            root.path().join("rsspkg.toml"),
            "[package]\nname = \"neutral\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[dependencies.clock]\npath = \"deps/clock\"\n",
        )
        .expect("manifest");
        fs::write(
            root.path().join("deps/clock/rsspkg.toml"),
            "[package]\nname = \"clock\"\nversion = \"0.1.0\"\nedition = \"2026\"\n\n[interfaces]\npaths = [\"interfaces\"]\n",
        )
        .expect("dependency manifest");
        fs::write(
            root.path().join("deps/clock/interfaces/clock.rssi"),
            "fn HostClock.now() -> Int\n",
        )
        .expect("interface");
        fs::write(
            root.path().join("src/main.rss"),
            "fn main() -> Int { return HostClock.now() }\n",
        )
        .expect("source");

        let analysis = analyze_package_dir_captured(root.path()).expect("neutral analysis");
        assert_eq!(analysis.summary.errors, 0, "{:?}", analysis.diagnostics);
        assert!(
            analysis
                .external_imports
                .iter()
                .any(|import| { import.function == "main" && import.symbol == "HostClock.now" })
        );
    }
}
