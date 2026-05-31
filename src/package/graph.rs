use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::source_set::{
    ManifestDependencyBudget, load_package_manifest, load_package_with_features,
    resolve_package_features, selected_root_package_features,
};
use super::{
    PackageDependencyKind, PackageDependencySpec, PackageGraphCheck, PackageRisk, PackageTree,
    PackageTreeNode, PackageTreeSummary, package_dependency_spec, package_identity,
    review_package_dir,
};

pub fn package_tree(package_dir: &Path) -> Result<PackageTree, String> {
    let mut visiting = BTreeSet::new();
    let root = package_tree_node(
        package_dir,
        PackageDependencyKind::Root,
        None,
        &mut visiting,
    )?;
    let mut summary = PackageTreeSummary::default();
    collect_package_tree_summary(&root, &mut summary);
    Ok(PackageTree { root, summary })
}

pub(super) fn check_package_graph(package_dir: &Path) -> Result<PackageGraphCheck, String> {
    let root_manifest = load_package_manifest(package_dir)?;
    let tree = package_tree(package_dir)?;
    let mut packages_by_name: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    collect_package_graph_identities(&tree.root, &mut packages_by_name);

    let mut reasons = Vec::new();
    collect_missing_provider_reasons(&tree.root, &mut reasons);
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
            root_manifest.dependencies.len() + root_manifest.dev_dependencies.len(),
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

fn collect_missing_provider_reasons(node: &PackageTreeNode, reasons: &mut Vec<String>) {
    if node.interface_only && node.dependency_kind == PackageDependencyKind::Normal {
        reasons.push(format!(
            "interface-only dependency `{}` requires an implementation provider for executable builds",
            node.name
        ));
    }
    for dependency in &node.dependencies {
        collect_missing_provider_reasons(dependency, reasons);
    }
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
            .insert(format!("{version} {}", node.source));
    }
    for dependency in &node.dependencies {
        collect_package_graph_identities(dependency, packages_by_name);
    }
}

fn package_tree_node(
    package_dir: &Path,
    dependency_kind: PackageDependencyKind,
    selected_features: Option<&[String]>,
    visiting: &mut BTreeSet<String>,
) -> Result<PackageTreeNode, String> {
    let package = load_package_with_features(package_dir, selected_features)?;
    let review = review_package_dir(package_dir)?;
    let features = selected_features
        .map(|features| features.to_vec())
        .unwrap_or_else(|| selected_root_package_features(&package.manifest));
    let identity = package_identity(&package.manifest);
    let visit_key = super::canonical_path_label(package_dir);
    if !visiting.insert(visit_key.clone()) {
        return Ok(PackageTreeNode {
            name: identity.name,
            version: Some(identity.version),
            requirement: None,
            source: format!("path+{}", package_dir.display()),
            risk: PackageRisk::Elevated,
            features,
            native: review.native_rust.is_some(),
            interface_only: package_is_interface_only(&package.sources),
            dependency_kind,
            reasons: vec!["dependency cycle truncated".to_string()],
            dependencies: Vec::new(),
        });
    }

    let mut dependencies = Vec::new();
    dependencies.extend(package_tree_dependencies(
        package_dir,
        &package.manifest.dependencies,
        PackageDependencyKind::Normal,
        visiting,
    )?);
    dependencies.extend(package_tree_dependencies(
        package_dir,
        &package.manifest.dev_dependencies,
        PackageDependencyKind::Dev,
        visiting,
    )?);
    visiting.remove(&visit_key);

    Ok(PackageTreeNode {
        name: identity.name,
        version: Some(identity.version),
        requirement: None,
        source: format!("path+{}", package_dir.display()),
        risk: review.risk,
        features,
        native: review.native_rust.is_some(),
        interface_only: package_is_interface_only(&package.sources),
        dependency_kind,
        reasons: review.reasons,
        dependencies,
    })
}

fn package_tree_dependencies(
    package_dir: &Path,
    dependencies: &BTreeMap<String, toml::Value>,
    dependency_kind: PackageDependencyKind,
    visiting: &mut BTreeSet<String>,
) -> Result<Vec<PackageTreeNode>, String> {
    let mut nodes = Vec::new();
    for (name, value) in dependencies {
        let spec = package_dependency_spec(name, value);
        if let Some(path) = &spec.path {
            let dependency_dir = package_dir.join(path);
            if dependency_dir.join("rsspkg.toml").exists() {
                let dependency_manifest = load_package_manifest(&dependency_dir)?;
                let selected_features =
                    resolve_package_features(&dependency_manifest, &spec.features);
                let mut node = package_tree_node(
                    &dependency_dir,
                    dependency_kind,
                    Some(&selected_features.selected),
                    visiting,
                )?;
                node.requirement = spec.requirement.clone();
                node.features = selected_features.selected;
                node.source = format!("path+{}", dependency_dir.display());
                nodes.push(node);
            } else {
                nodes.push(unresolved_dependency_node(
                    spec,
                    dependency_kind,
                    vec!["path dependency manifest missing".to_string()],
                ));
            }
        } else {
            nodes.push(unresolved_dependency_node(
                spec,
                dependency_kind,
                vec!["dependency resolver not implemented for this source".to_string()],
            ));
        }
    }
    Ok(nodes)
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
        interface_only: false,
        dependency_kind,
        reasons,
        dependencies: Vec::new(),
    }
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

fn collect_package_tree_summary(node: &PackageTreeNode, summary: &mut PackageTreeSummary) {
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
        collect_package_tree_summary(dependency, summary);
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
