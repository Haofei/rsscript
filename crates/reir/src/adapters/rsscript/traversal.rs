// Recursive package-tree traversal kept separate from fact construction.

fn collect_package_tree_facts(node: &RsScriptPackageTreeNode, path: &str, facts: &mut Vec<Fact>) {
    let subject = package_tree_node_subject(node);
    let path_slug = normalized_id(path);
    let unknown = matches!(node.risk, RsScriptPackageRisk::Unknown);
    facts.push(Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_tree.{}.risk", path_slug),
        kind: if node.dependency_kind == "root" {
            FactKind::PackageRisk
        } else {
            FactKind::DependencyRisk
        },
        role: None,
        subject,
        capability: None,
        value: package_risk_value(&node.risk),
        confidence: confidence(package_tree_confidence(node), TREE_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![dependency_path_evidence(
            node,
            path,
            Some(node.risk.as_str().to_owned()),
            Some(package_tree_node_summary(node)),
        )],
        unknown_reason: unknown.then(|| package_tree_unknown_reason(node)),
    });
    if !node.interface_effective_hash.is_empty() {
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.package_tree.{}.effective_interface_hash", path_slug),
            kind: FactKind::SupplyChain,
            role: None,
            subject: package_tree_node_subject(node),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Computed, TREE_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Exact,
            evidence: vec![dependency_path_evidence(
                node,
                path,
                Some(node.interface_effective_hash.clone()),
                Some(format!(
                    "effective_interface_hash={} features={}",
                    node.interface_effective_hash,
                    tree_features_summary(&node.features)
                )),
            )],
            unknown_reason: None,
        });
    }
    for (index, dependency) in node.dependencies.iter().enumerate() {
        collect_package_tree_facts(dependency, &format!("{path}/{}", index + 1), facts);
    }
}

fn collect_package_tree_edges(node: &RsScriptPackageTreeNode, path: &str, edges: &mut Vec<Edge>) {
    let from = package_tree_node_subject(node);
    for (index, dependency) in node.dependencies.iter().enumerate() {
        let child_path = format!("{path}/{}", index + 1);
        edges.push(Edge {
            schema: EDGE_SCHEMA.to_owned(),
            id: format!(
                "edge.package_tree.{}.depends_on.{}",
                normalized_id(path),
                normalized_id(&child_path)
            ),
            kind: EdgeKind::DependsOn,
            from: from.clone(),
            to: package_tree_node_subject(dependency),
            confidence: confidence(package_tree_confidence(dependency), TREE_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Category,
            evidence: vec![dependency_path_evidence(
                dependency,
                &child_path,
                dependency.requirement.clone(),
                Some(package_tree_node_summary(dependency)),
            )],
        });
        collect_package_tree_edges(dependency, &child_path, edges);
    }
}

fn package_tree_node_subject(node: &RsScriptPackageTreeNode) -> Subject {
    if let Some(version) = &node.version {
        package_subject(&node.name, version)
    } else {
        let identity = node
            .requirement
            .as_ref()
            .map(|requirement| format!("{}@{}", node.name, requirement))
            .unwrap_or_else(|| format!("{}@{}", node.name, node.source));
        Subject {
            kind: SubjectKind::Dependency,
            id: identity,
            name: Some(node.name.clone()),
            package: Some(node.name.clone()),
        }
    }
}

fn package_tree_confidence(node: &RsScriptPackageTreeNode) -> ConfidenceLevel {
    if node.source.starts_with("path+") || node.platform_provided {
        ConfidenceLevel::Scanned
    } else {
        ConfidenceLevel::Unknown
    }
}

fn package_tree_unknown_reason(node: &RsScriptPackageTreeNode) -> String {
    if node.reasons.is_empty() {
        format!(
            "dependency `{}` could not be resolved by package tree",
            node.name
        )
    } else {
        node.reasons.join("; ")
    }
}

fn package_tree_node_summary(node: &RsScriptPackageTreeNode) -> String {
    format!(
        "dependency {} version={} requirement={} source={} kind={} features={} native={} interface_only={} compile_only={} test_only={} platform_provided={} reasons={}",
        node.name,
        node.version.as_deref().unwrap_or("unresolved"),
        node.requirement.as_deref().unwrap_or("unspecified"),
        node.source,
        node.dependency_kind,
        tree_features_summary(&node.features),
        node.native,
        node.interface_only,
        node.compile_only,
        node.test_only,
        node.platform_provided,
        if node.reasons.is_empty() {
            "none".to_owned()
        } else {
            node.reasons.join("; ")
        }
    )
}

fn dependency_path_evidence(
    node: &RsScriptPackageTreeNode,
    path: &str,
    value: Option<String>,
    reason: Option<String>,
) -> Evidence {
    Evidence {
        file: package_tree_evidence_file(node),
        reason,
        json_pointer: Some(format!("/root{}", dependency_path_json_pointer(path))),
        resource: Some(format!(
            "{}@{}",
            node.name,
            node.version
                .as_deref()
                .or(node.requirement.as_deref())
                .unwrap_or("unresolved")
        )),
        provider: Some("rsscript".to_owned()),
        value,
        source: Some(TREE_SOURCE.to_owned()),
        ..rsscript_evidence(EvidenceKind::DependencyPath)
    }
}

fn package_tree_evidence_file(node: &RsScriptPackageTreeNode) -> Option<String> {
    node.version.as_ref()?;
    node.source
        .strip_prefix("path+")
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
}

fn dependency_path_json_pointer(path: &str) -> String {
    path.strip_prefix("root")
        .unwrap_or(path)
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let index = part.parse::<usize>().unwrap_or(1).saturating_sub(1);
            format!("/dependencies/{index}")
        })
        .collect::<String>()
}

fn tree_features_summary(features: &[String]) -> String {
    if features.is_empty() {
        "none".to_owned()
    } else {
        features.join(",")
    }
}
