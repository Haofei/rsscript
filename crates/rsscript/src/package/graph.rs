use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::dependency::{
    DependencyResolutionScope, ResolvedDependencyEdge, ResolvedDependencyGraph,
    resolve_dependency_graph,
};
use super::lock::effective_interface_hash;
use super::review::review_package_dir_captured_with_features;
use super::source_set::{
    ManifestDependencyBudget, ManifestProviderChoice, load_package_manifest,
    load_package_with_features,
};
use super::{
    PackageDependencyKind, PackageDependencySpec, PackageGraphCheck, PackageRisk, PackageTree,
    PackageTreeNode, PackageTreeSummary, package_identity,
};

pub fn package_tree(package_dir: &Path) -> Result<PackageTree, String> {
    let snapshot = super::authorization::snapshot_package_graph_inputs(package_dir)?;
    let mut tree =
        package_tree_captured(snapshot.root()).map_err(|error| snapshot.remap_error(error))?;
    snapshot.remap_tree(&mut tree);
    Ok(tree)
}

pub(super) fn package_tree_captured(package_dir: &Path) -> Result<PackageTree, String> {
    let graph = resolve_dependency_graph(package_dir, DependencyResolutionScope::Development)?;
    let root = package_tree_node(
        &graph,
        &graph.root,
        PackageDependencyKind::Root,
        None,
        &mut BTreeMap::new(),
    )?;
    let mut summary = PackageTreeSummary::default();
    collect_package_tree_summary(&root, &mut summary, &mut BTreeSet::new());
    Ok(PackageTree { root, summary })
}

pub(super) fn check_package_graph(package_dir: &Path) -> Result<PackageGraphCheck, String> {
    let root_manifest = load_package_manifest(package_dir)?;
    let tree = package_tree_captured(package_dir)?;
    let mut packages_by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    collect_package_graph_identities(&tree.root, &mut packages_by_name);

    let mut reasons = Vec::new();
    collect_missing_provider_reasons(
        &tree.root,
        &tree.root,
        &root_manifest.providers,
        &mut reasons,
    );
    if tree.summary.unresolved_dependencies > 0 {
        reasons.push(format!(
            "dependency graph contains {} unresolved dependencies",
            tree.summary.unresolved_dependencies
        ));
    }
    for (name, identities) in packages_by_name {
        if identities.len() > 1 {
            reasons.push(format!(
                "dependency `{name}` resolves to multiple package identities: {}",
                identities.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    if let Some(dependency_policy) = root_manifest.dependency.as_ref() {
        collect_graph_budget_reasons(
            &tree.summary,
            tree.root.dependencies.len(),
            &dependency_policy.budget,
            &mut reasons,
        );
    }
    reasons.sort();
    reasons.dedup();

    let ok = reasons.is_empty();
    let risk = if ok {
        PackageRisk::Low
    } else if tree.summary.unresolved_dependencies > 0 {
        PackageRisk::Unknown
    } else {
        PackageRisk::High
    };

    Ok(PackageGraphCheck { ok, risk, reasons })
}

fn collect_missing_provider_reasons(
    root: &PackageTreeNode,
    node: &PackageTreeNode,
    provider_choices: &BTreeMap<String, ManifestProviderChoice>,
    reasons: &mut Vec<String>,
) {
    if node.interface_only
        && node.dependency_kind == PackageDependencyKind::Normal
        && !node.compile_only
        && !node.test_only
        && !node.platform_provided
    {
        collect_missing_provider_reason(root, node, provider_choices, reasons);
    }
    for dependency in &node.dependencies {
        collect_missing_provider_reasons(root, dependency, provider_choices, reasons);
    }
}

fn collect_missing_provider_reason(
    root: &PackageTreeNode,
    node: &PackageTreeNode,
    provider_choices: &BTreeMap<String, ManifestProviderChoice>,
    reasons: &mut Vec<String>,
) {
    let Some(choice) = provider_choice_for_node(node, provider_choices) else {
        reasons.push(format!(
            "interface-only dependency `{}` requires an implementation provider for executable builds",
            node.name
        ));
        return;
    };
    let Some(provider_package) = choice.package.as_deref() else {
        reasons.push(format!(
            "provider choice for interface-only dependency `{}` is missing package",
            node.name
        ));
        return;
    };
    let Some(provider) = find_package_tree_node_by_name(root, provider_package) else {
        reasons.push(format!(
            "provider `{provider_package}` for interface-only dependency `{}` is not resolved in dependency graph",
            node.name
        ));
        return;
    };
    if choice
        .version
        .as_deref()
        .is_some_and(|version| provider.version.as_deref() != Some(version))
    {
        reasons.push(format!(
            "provider `{provider_package}` for interface-only dependency `{}` does not match requested version",
            node.name
        ));
        return;
    }
    let Some(declared_hash) = provider
        .implements
        .iter()
        .find(|implementation| implementation.interface_package == node.name)
        .and_then(|implementation| implementation.interface_effective_hash.as_deref())
    else {
        reasons.push(format!(
            "provider `{provider_package}` does not declare implementation for interface-only dependency `{}`",
            node.name
        ));
        return;
    };
    if declared_hash != node.interface_effective_hash {
        reasons.push(format!(
            "provider `{provider_package}` interface hash for `{}` is stale or mismatched",
            node.name
        ));
    }
}

fn find_package_tree_node_by_name<'a>(
    node: &'a PackageTreeNode,
    name: &str,
) -> Option<&'a PackageTreeNode> {
    if node.name == name {
        return Some(node);
    }
    for dependency in &node.dependencies {
        if let Some(found) = find_package_tree_node_by_name(dependency, name) {
            return Some(found);
        }
    }
    None
}

fn provider_choice_for_node<'a>(
    node: &'a PackageTreeNode,
    provider_choices: &'a BTreeMap<String, ManifestProviderChoice>,
) -> Option<&'a ManifestProviderChoice> {
    provider_choices.get(&node.name).or_else(|| {
        node.virtual_package
            .as_ref()
            .and_then(|virtual_package| virtual_package.provider.as_deref())
            .and_then(|provider| provider_choices.get(provider))
    })
}

fn collect_graph_budget_reasons(
    summary: &PackageTreeSummary,
    direct_dependencies: usize,
    budget: &ManifestDependencyBudget,
    reasons: &mut Vec<String>,
) {
    push_budget_reason(
        reasons,
        "direct dependencies",
        direct_dependencies,
        budget.max_direct_dependencies,
    );
    push_budget_reason(
        reasons,
        "total packages",
        summary.packages,
        budget.max_total_packages,
    );
    push_budget_reason(
        reasons,
        "native packages",
        summary.native_packages,
        budget.max_native_packages,
    );
    push_budget_reason(
        reasons,
        "high-risk packages",
        summary.high_risk_packages,
        budget.max_high_risk_packages,
    );
    push_budget_reason(
        reasons,
        "unknown packages",
        summary.unknown_risk_packages,
        budget.max_unknown_packages,
    );
    push_budget_reason(
        reasons,
        "build-execution packages",
        summary.build_execution_packages,
        budget.max_build_execution_packages,
    );
}

fn push_budget_reason(
    reasons: &mut Vec<String>,
    label: &str,
    actual: usize,
    maximum: Option<usize>,
) {
    if let Some(maximum) = maximum
        && actual > maximum
    {
        reasons.push(format!(
            "dependency graph exceeds {label} budget: {actual} > {maximum}"
        ));
    }
}

fn collect_package_graph_identities(
    node: &PackageTreeNode,
    packages_by_name: &mut BTreeMap<String, BTreeSet<String>>,
) {
    if let Some(version) = &node.version {
        packages_by_name
            .entry(node.name.clone())
            .or_default()
            .insert(format!(
                "{version} {}",
                canonical_graph_source(&node.source)
            ));
    }
    for dependency in &node.dependencies {
        collect_package_graph_identities(dependency, packages_by_name);
    }
}

fn canonical_graph_source(source: &str) -> String {
    let Some(path) = source.strip_prefix("path+") else {
        return source.to_string();
    };
    format!("path+{}", super::canonical_path_label(Path::new(path)))
}

fn package_tree_node(
    graph: &ResolvedDependencyGraph,
    key: &str,
    dependency_kind: PackageDependencyKind,
    incoming: Option<&ResolvedDependencyEdge>,
    cache: &mut BTreeMap<(String, PackageDependencyKind), PackageTreeNode>,
) -> Result<PackageTreeNode, String> {
    let cache_key = (key.to_string(), dependency_kind);
    if let Some(cached) = cache.get(&cache_key) {
        let mut reference = cached.clone();
        apply_incoming_edge(&mut reference, incoming);
        reference.reference = Some(key.to_string());
        reference.dependencies.clear();
        return Ok(reference);
    }
    let resolved = &graph.nodes[key];
    let package_dir = &resolved.package_dir;
    let features = resolved.features.clone();
    let package = load_package_with_features(package_dir, Some(&features))?;
    let review = review_package_dir_captured_with_features(package_dir, Some(&features))?;
    let interface_effective_hash = effective_interface_hash(&package.sources, &features);
    let identity = package_identity(&package.manifest);
    let mut dependencies = Vec::new();
    for edge in &resolved.dependencies {
        let child_kind = if dependency_kind == PackageDependencyKind::Dev {
            PackageDependencyKind::Dev
        } else {
            edge.kind
        };
        dependencies.push(match &edge.target {
            Some(target) => package_tree_node(graph, target, child_kind, Some(edge), cache)?,
            None => {
                unresolved_dependency_node(edge.spec.clone(), child_kind, unresolved_reasons(edge))
            }
        });
    }

    let spec = incoming.map(|edge| &edge.spec);

    let node = PackageTreeNode {
        name: identity.name,
        version: Some(identity.version),
        requirement: spec.and_then(|spec| spec.requirement.clone()),
        source: super::package_path_source(package_dir),
        risk: review.risk,
        features,
        native: review.native_rust.is_some(),
        virtual_package: package_virtual(&package.manifest),
        interface_only: package_is_interface_only(&package.sources),
        compile_only: spec.is_some_and(|spec| spec.compile_only),
        test_only: spec.is_some_and(|spec| spec.test_only),
        platform_provided: spec.is_some_and(|spec| spec.platform_provided),
        interface_effective_hash,
        implements: package_provider_implementations(&package.manifest),
        dependency_kind,
        reasons: review.reasons,
        reference: None,
        dependencies,
    };
    cache.insert(cache_key, node.clone());
    Ok(node)
}

fn apply_incoming_edge(node: &mut PackageTreeNode, incoming: Option<&ResolvedDependencyEdge>) {
    let Some(edge) = incoming else {
        return;
    };
    node.requirement = edge.spec.requirement.clone();
    node.compile_only = edge.spec.compile_only;
    node.test_only = edge.spec.test_only;
    node.platform_provided = edge.spec.platform_provided;
}

fn unresolved_reasons(edge: &ResolvedDependencyEdge) -> Vec<String> {
    if edge.spec.path.is_some() {
        vec!["path dependency manifest missing".to_string()]
    } else {
        vec!["dependency resolver not implemented for this source".to_string()]
    }
}

fn unresolved_dependency_node(
    spec: PackageDependencySpec,
    dependency_kind: PackageDependencyKind,
    reasons: Vec<String>,
) -> PackageTreeNode {
    let source = if let Some(path) = &spec.path {
        format!("path+{path}")
    } else if let Some(git) = &spec.git {
        format!("git+{git}")
    } else {
        "registry".to_string()
    };
    PackageTreeNode {
        name: spec.name,
        version: None,
        requirement: spec.requirement,
        source,
        risk: PackageRisk::Unknown,
        features: spec.features,
        native: false,
        virtual_package: None,
        interface_only: false,
        compile_only: spec.compile_only,
        test_only: spec.test_only,
        platform_provided: spec.platform_provided,
        interface_effective_hash: String::new(),
        implements: Vec::new(),
        dependency_kind,
        reasons,
        reference: None,
        dependencies: Vec::new(),
    }
}

fn package_provider_implementations(
    manifest: &super::Manifest,
) -> Vec<super::PackageProviderImplementation> {
    manifest
        .implements
        .iter()
        .map(
            |(interface_package, implementation)| super::PackageProviderImplementation {
                interface_package: interface_package.clone(),
                version: implementation.version.clone(),
                interface_features: implementation.interface_features.clone(),
                interface_effective_hash: implementation.interface_effective_hash.clone(),
            },
        )
        .collect()
}

fn package_virtual(manifest: &super::Manifest) -> Option<super::PackageVirtual> {
    manifest
        .virtual_package
        .as_ref()
        .map(|virtual_package| super::PackageVirtual {
            has_default: virtual_package.has_default,
            provider: virtual_package.provider.clone(),
        })
}

fn package_is_interface_only(sources: &[super::PackageSource]) -> bool {
    let has_interface = sources
        .iter()
        .any(|source| source.kind == super::PackageReviewFileKind::Interface);
    let has_source = sources
        .iter()
        .any(|source| source.kind == super::PackageReviewFileKind::Source);
    has_interface && !has_source
}

fn collect_package_tree_summary(
    node: &PackageTreeNode,
    summary: &mut PackageTreeSummary,
    seen: &mut BTreeSet<String>,
) {
    let identity = format!(
        "{}\0{}\0{}",
        node.name,
        node.version.as_deref().unwrap_or("<unresolved>"),
        canonical_graph_source(&node.source)
    );
    if !seen.insert(identity) {
        return;
    }
    summary.packages += 1;
    if node.source.starts_with("path+") && node.dependency_kind != PackageDependencyKind::Root {
        summary.path_dependencies += 1;
    }
    if node.risk == PackageRisk::Unknown {
        summary.unknown_risk_packages += 1;
    }
    if node.risk == PackageRisk::High {
        summary.high_risk_packages += 1;
    }
    if node.version.is_none() {
        summary.unresolved_dependencies += 1;
    }
    if node.native {
        summary.native_packages += 1;
    }
    if package_tree_node_uses_build_execution(node) {
        summary.build_execution_packages += 1;
    }
    for dependency in &node.dependencies {
        collect_package_tree_summary(dependency, summary, seen);
    }
}

fn package_tree_node_uses_build_execution(node: &PackageTreeNode) -> bool {
    node.reasons.iter().any(|reason| {
        reason.contains("build script")
            || reason.contains("build scripts")
            || reason.contains("proc macro")
            || reason.contains("proc macros")
            || reason.contains("build-time execution")
    })
}
