use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::diagnostic::{Diagnostic, code};

use super::source_set::{
    Manifest, ManifestReviewFeaturePolicy, PackageSource, load_package_manifest,
    load_package_with_features, resolve_package_features,
};
use super::{PackageReviewFileKind, canonical_path_label, toml_value_label};

#[derive(Debug, Clone)]
pub(super) struct PackageDependencySpec {
    pub(super) name: String,
    pub(super) requirement: Option<String>,
    pub(super) path: Option<String>,
    pub(super) git: Option<String>,
    pub(super) features: Vec<String>,
    pub(super) compile_only: bool,
    pub(super) test_only: bool,
    pub(super) platform_provided: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DependencyResolutionScope {
    Production,
    Development,
}

#[derive(Debug)]
pub(super) struct ResolvedDependencyGraph {
    pub(super) root: String,
    pub(super) nodes: BTreeMap<String, ResolvedDependencyNode>,
}

#[derive(Debug)]
pub(super) struct ResolvedDependencyNode {
    pub(super) package_dir: PathBuf,
    pub(super) manifest: Manifest,
    pub(super) features: Vec<String>,
    pub(super) dependencies: Vec<ResolvedDependencyEdge>,
}

#[derive(Debug, Clone)]
pub(super) struct ResolvedDependencyEdge {
    pub(super) spec: PackageDependencySpec,
    pub(super) kind: super::PackageDependencyKind,
    pub(super) target: Option<String>,
}

pub(super) fn resolve_dependency_graph(
    package_dir: &Path,
    scope: DependencyResolutionScope,
) -> Result<ResolvedDependencyGraph, String> {
    let root = canonical_path_label(package_dir);
    let root_manifest = load_package_manifest(package_dir)?;
    let root_features = super::source_set::selected_root_package_features(&root_manifest);
    let mut graph = ResolvedDependencyGraph {
        root: root.clone(),
        nodes: BTreeMap::new(),
    };
    let mut expanded = BTreeSet::new();
    let mut stack = Vec::new();
    resolve_dependency_node(
        package_dir,
        &root_features,
        true,
        scope,
        &mut graph,
        &mut expanded,
        &mut stack,
    )?;
    Ok(graph)
}

fn resolve_dependency_node(
    package_dir: &Path,
    requested_features: &[String],
    is_root: bool,
    scope: DependencyResolutionScope,
    graph: &mut ResolvedDependencyGraph,
    expanded: &mut BTreeSet<String>,
    stack: &mut Vec<String>,
) -> Result<String, String> {
    let canonical = canonical_path_label(package_dir);
    if let Some(cycle_start) = stack.iter().position(|entry| entry == &canonical) {
        let mut cycle = stack[cycle_start..]
            .iter()
            .map(|key| resolved_node_label(graph, key))
            .collect::<Vec<_>>();
        cycle.push(resolved_node_label(graph, &canonical));
        return Err(format!("dependency cycle detected: {}", cycle.join(" -> ")));
    }

    if !graph.nodes.contains_key(&canonical) {
        let manifest = load_package_manifest(package_dir)?;
        graph.nodes.insert(
            canonical.clone(),
            ResolvedDependencyNode {
                package_dir: package_dir.to_path_buf(),
                manifest,
                features: Vec::new(),
                dependencies: Vec::new(),
            },
        );
    }
    {
        let node = graph
            .nodes
            .get_mut(&canonical)
            .expect("resolved node exists");
        let mut requested = node.features.clone();
        requested.extend(requested_features.iter().cloned());
        node.features = resolve_package_features(&node.manifest, &requested).selected;
    }
    if expanded.contains(&canonical) {
        return Ok(canonical);
    }

    stack.push(canonical.clone());
    let manifest = &graph.nodes[&canonical].manifest;
    let mut declared = manifest
        .dependencies
        .iter()
        .map(|(name, value)| {
            (
                package_dependency_spec(name, value),
                super::PackageDependencyKind::Normal,
            )
        })
        .collect::<Vec<_>>();
    if is_root && scope == DependencyResolutionScope::Development {
        declared.extend(manifest.dev_dependencies.iter().map(|(name, value)| {
            (
                package_dependency_spec(name, value),
                super::PackageDependencyKind::Dev,
            )
        }));
    }

    let mut dependencies = Vec::with_capacity(declared.len());
    for (spec, kind) in declared {
        let target = match &spec.path {
            Some(path) => {
                let dependency_dir = package_dir.join(path);
                if dependency_dir.join("rsspkg.toml").exists() {
                    Some(resolve_dependency_node(
                        &dependency_dir,
                        &spec.features,
                        false,
                        scope,
                        graph,
                        expanded,
                        stack,
                    )?)
                } else {
                    None
                }
            }
            None => None,
        };
        dependencies.push(ResolvedDependencyEdge { spec, kind, target });
    }
    stack.pop();
    graph
        .nodes
        .get_mut(&canonical)
        .expect("resolved node exists")
        .dependencies = dependencies;
    expanded.insert(canonical.clone());
    Ok(canonical)
}

fn resolved_node_label(graph: &ResolvedDependencyGraph, key: &str) -> String {
    graph
        .nodes
        .get(key)
        .map(|node| node.manifest.package.name.clone())
        .unwrap_or_else(|| key.to_string())
}

impl ResolvedDependencyGraph {
    pub(super) fn dependency_order(&self) -> Vec<&ResolvedDependencyNode> {
        let mut seen = BTreeSet::new();
        let mut ordered = Vec::new();
        self.collect_dependency_order(&self.root, &mut seen, &mut ordered);
        ordered
    }

    fn collect_dependency_order<'a>(
        &'a self,
        key: &str,
        seen: &mut BTreeSet<String>,
        ordered: &mut Vec<&'a ResolvedDependencyNode>,
    ) {
        if !seen.insert(key.to_string()) {
            return;
        }
        let node = &self.nodes[key];
        for edge in &node.dependencies {
            if let Some(target) = &edge.target {
                self.collect_dependency_order(target, seen, ordered);
            }
        }
        if key != self.root {
            ordered.push(node);
        }
    }
}

pub(super) fn collect_dependency_interface_sources(
    package_dir: &Path,
    _manifest: &Manifest,
) -> Result<Vec<PackageSource>, String> {
    let graph = resolve_dependency_graph(package_dir, DependencyResolutionScope::Production)?;
    let mut sources = Vec::new();
    collect_resolved_sources(
        &graph,
        PackageReviewFileKind::Interface,
        false,
        &mut sources,
    )?;
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(sources)
}

pub(super) fn collect_dependency_interface_sources_for_tests(
    package_dir: &Path,
    _manifest: &Manifest,
) -> Result<Vec<PackageSource>, String> {
    let graph = resolve_dependency_graph(package_dir, DependencyResolutionScope::Development)?;
    let mut sources = Vec::new();
    collect_resolved_sources(
        &graph,
        PackageReviewFileKind::Interface,
        false,
        &mut sources,
    )?;
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(sources)
}

pub(super) fn collect_dependency_lowering_sources(
    package_dir: &Path,
    _manifest: &Manifest,
) -> Result<Vec<PackageSource>, String> {
    let graph = resolve_dependency_graph(package_dir, DependencyResolutionScope::Production)?;
    let mut sources = Vec::new();
    collect_resolved_sources(&graph, PackageReviewFileKind::Source, true, &mut sources)?;
    sources.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(sources)
}

fn collect_resolved_sources(
    graph: &ResolvedDependencyGraph,
    kind: PackageReviewFileKind,
    lowering: bool,
    sources: &mut Vec<PackageSource>,
) -> Result<(), String> {
    let included = if lowering {
        lowering_reachable_nodes(graph)
    } else {
        graph.nodes.keys().cloned().collect()
    };
    for node in graph.dependency_order() {
        let canonical = canonical_path_label(&node.package_dir);
        if !included.contains(&canonical) {
            continue;
        }
        let package = load_package_with_features(&node.package_dir, Some(&node.features))?;
        sources.extend(
            package
                .sources
                .into_iter()
                .filter(|source| source.kind == kind),
        );
    }
    Ok(())
}

fn lowering_reachable_nodes(graph: &ResolvedDependencyGraph) -> BTreeSet<String> {
    let mut reachable = BTreeSet::new();
    let mut pending = vec![graph.root.clone()];
    while let Some(key) = pending.pop() {
        for edge in &graph.nodes[&key].dependencies {
            if edge.spec.platform_provided || edge.spec.test_only {
                continue;
            }
            if let Some(target) = &edge.target
                && reachable.insert(target.clone())
            {
                pending.push(target.clone());
            }
        }
    }
    reachable
}

pub(super) fn package_feature_resolution_diagnostics(
    package_dir: &Path,
    manifest: &Manifest,
) -> Result<Vec<Diagnostic>, String> {
    let graph = resolve_dependency_graph(package_dir, DependencyResolutionScope::Development)?;
    let mut diagnostics = Vec::new();
    let feature_policy = manifest
        .review
        .as_ref()
        .map(|review| &review.feature_policy);
    for node in graph.nodes.values() {
        let declared = node
            .manifest
            .dependencies
            .iter()
            .chain(node.manifest.dev_dependencies.iter());
        for (name, value) in declared {
            let Some(edge) = node
                .dependencies
                .iter()
                .find(|edge| edge.spec.name == *name)
            else {
                // Non-root dev dependencies are intentionally outside this resolution.
                continue;
            };
            diagnostics.extend(package_dependency_unknown_key_diagnostics(
                &node.package_dir,
                name,
                value,
            ));
            if edge.spec.git.is_some() {
                diagnostics.push(package_unsupported_dependency_source_diagnostic(
                    &node.package_dir,
                    name,
                    "git",
                ));
            }
            let Some(target) = &edge.target else {
                continue;
            };
            let dependency = &graph.nodes[target];
            let requested = resolve_package_features(&dependency.manifest, &edge.spec.features);
            for feature in requested.unknown {
                diagnostics.push(package_unknown_feature_diagnostic(
                    &node.package_dir,
                    name,
                    &feature,
                ));
            }
            if let Some(feature_policy) = feature_policy {
                for feature in &dependency.features {
                    if package_feature_denied(feature_policy, name, feature) {
                        diagnostics.push(package_denied_feature_diagnostic(
                            &node.package_dir,
                            name,
                            feature,
                        ));
                    }
                }
            }
        }
    }
    Ok(diagnostics)
}

fn package_dependency_unknown_key_diagnostics(
    package_dir: &Path,
    dependency: &str,
    value: &toml::Value,
) -> Vec<Diagnostic> {
    let Some(table) = value.as_table() else {
        return Vec::new();
    };
    const ALLOWED_KEYS: &[&str] = &[
        "version",
        "path",
        "git",
        "features",
        "compile_only",
        "test_only",
        "platform_provided",
    ];
    table
        .keys()
        .filter(|key| !ALLOWED_KEYS.contains(&key.as_str()))
        .map(|key| {
            Diagnostic::error(
                code::PACKAGE_REVIEW_POLICY_VIOLATION,
                format!("dependency `{dependency}` has unknown key `{key}`."),
                super::package_dependency_span(package_dir, dependency),
                "unknown dependency key",
            )
            .with_cause("Package dependency metadata is review-critical and unknown keys cannot be ignored.")
            .with_fix(
                "remove_unknown_dependency_key",
                format!("Remove `{key}` or replace it with a supported dependency key."),
                "manual",
            )
        })
        .collect()
}

fn package_unsupported_dependency_source_diagnostic(
    package_dir: &Path,
    dependency: &str,
    source: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::PACKAGE_UNSUPPORTED_DEPENDENCY_SOURCE,
        format!("dependency `{dependency}` uses unsupported package source `{source}`."),
        super::package_dependency_span(package_dir, dependency),
        "unsupported dependency source",
    )
    .with_cause("Git dependencies are not part of the v0.6 accepted dependency-source grammar.")
    .with_fix(
        "use_supported_dependency_source",
        "Use a registry version requirement or a local path dependency.",
        "manual",
    )
}

fn package_feature_denied(
    feature_policy: &ManifestReviewFeaturePolicy,
    package: &str,
    feature: &str,
) -> bool {
    feature_policy
        .deny
        .iter()
        .any(|pattern| package_feature_deny_pattern_matches(pattern, package, feature))
}

fn package_feature_deny_pattern_matches(pattern: &str, package: &str, feature: &str) -> bool {
    let Some((package_pattern, feature_pattern)) = pattern.split_once('/') else {
        return pattern == feature;
    };
    (package_pattern == "*" || package_pattern == package) && feature_pattern == feature
}

fn package_unknown_feature_diagnostic(
    package_dir: &Path,
    dependency: &str,
    feature: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::PACKAGE_FEATURE_RESOLUTION,
        format!("dependency `{dependency}` selects unknown package feature `{feature}`."),
        super::package_dependency_span(package_dir, dependency),
        "unknown package feature",
    )
    .with_cause("Selected dependency features must be declared by the dependency package.")
    .with_fix(
        "fix_dependency_features",
        format!("Remove `{feature}` from the dependency feature list, or declare it in the dependency package."),
        "manual",
    )
}

fn package_denied_feature_diagnostic(
    package_dir: &Path,
    dependency: &str,
    feature: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::PACKAGE_REVIEW_POLICY_VIOLATION,
        format!("dependency `{dependency}` selects denied package feature `{feature}`."),
        super::package_dependency_span(package_dir, dependency),
        "denied package feature",
    )
    .with_cause("Selected dependency features must satisfy `[review.feature_policy]`.")
    .with_fix(
        "fix_dependency_features",
        format!(
            "Remove `{feature}` from the dependency feature list, or choose another dependency."
        ),
        "manual",
    )
}

pub(super) fn package_dependency_spec(name: &str, value: &toml::Value) -> PackageDependencySpec {
    if let Some(requirement) = value.as_str() {
        return PackageDependencySpec {
            name: name.to_string(),
            requirement: Some(requirement.to_string()),
            path: None,
            git: None,
            features: Vec::new(),
            compile_only: false,
            test_only: false,
            platform_provided: false,
        };
    }
    let Some(table) = value.as_table() else {
        return PackageDependencySpec {
            name: name.to_string(),
            requirement: Some(toml_value_label(value)),
            path: None,
            git: None,
            features: Vec::new(),
            compile_only: false,
            test_only: false,
            platform_provided: false,
        };
    };
    PackageDependencySpec {
        name: name.to_string(),
        requirement: table
            .get("version")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        path: table
            .get("path")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        git: table
            .get("git")
            .and_then(toml::Value::as_str)
            .map(str::to_string),
        features: table
            .get("features")
            .and_then(toml::Value::as_array)
            .map(|features| {
                features
                    .iter()
                    .filter_map(toml::Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        compile_only: table
            .get("compile_only")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
        test_only: table
            .get("test_only")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
        platform_provided: table
            .get("platform_provided")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{DependencyResolutionScope, canonical_path_label, resolve_dependency_graph};

    struct TestPackages(PathBuf);

    impl TestPackages {
        fn new(name: &str) -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "rsscript-package-resolution-{}-{name}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create test package root");
            Self(root)
        }

        fn package(&self, relative: &str, manifest_body: &str) -> PathBuf {
            let dir = self.0.join(relative);
            fs::create_dir_all(&dir).expect("create test package");
            let name = relative.replace('/', "-");
            fs::write(
                dir.join("rsspkg.toml"),
                format!(
                    "[package]\nname = \"{name}\"\nversion = \"1.0.0\"\nedition = \"2024\"\n\n{manifest_body}"
                ),
            )
            .expect("write test manifest");
            dir
        }
    }

    impl Drop for TestPackages {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn node_features<'a>(graph: &'a super::ResolvedDependencyGraph, path: &Path) -> &'a [String] {
        &graph.nodes[&canonical_path_label(path)].features
    }

    #[test]
    fn resolver_unions_features_across_diamond_deterministically() {
        let packages = TestPackages::new("feature-union");
        let shared = packages.package("root/shared", "[features]\nalpha = []\nbeta = []\n");
        packages.package(
            "root/a",
            "[dependencies]\nshared = { path = \"../shared\", features = [\"alpha\"] }\n",
        );
        packages.package(
            "root/b",
            "[dependencies]\nshared = { path = \"../shared\", features = [\"beta\"] }\n",
        );
        let root = packages.package(
            "root",
            "[dependencies]\na = { path = \"a\" }\nb = { path = \"b\" }\n",
        );

        let graph = resolve_dependency_graph(&root, DependencyResolutionScope::Production)
            .expect("resolve diamond");

        assert_eq!(node_features(&graph, &shared), &["alpha", "beta"]);
        assert_eq!(graph.nodes.len(), 4);
    }

    #[test]
    fn resolver_rejects_cycles_with_package_path() {
        let packages = TestPackages::new("cycle");
        packages.package("root/a", "[dependencies]\nroot = { path = \"..\" }\n");
        let root = packages.package("root", "[dependencies]\na = { path = \"a\" }\n");

        let error = resolve_dependency_graph(&root, DependencyResolutionScope::Production)
            .expect_err("cycle must fail");

        assert_eq!(error, "dependency cycle detected: root -> root-a -> root");
    }

    #[test]
    fn development_scope_adds_only_root_dev_dependencies() {
        let packages = TestPackages::new("dev-scope");
        packages.package("root/prod/transitive", "");
        packages.package("root/dev", "");
        packages.package("root/prod/dev-only", "");
        packages.package(
            "root/prod",
            "[dependencies]\ntransitive = { path = \"transitive\" }\n\n[dev-dependencies]\ndev-only = { path = \"dev-only\" }\n",
        );
        let root = packages.package(
            "root",
            "[dependencies]\nprod = { path = \"prod\" }\n\n[dev-dependencies]\ndev = { path = \"dev\" }\n",
        );

        let production = resolve_dependency_graph(&root, DependencyResolutionScope::Production)
            .expect("resolve production");
        let development = resolve_dependency_graph(&root, DependencyResolutionScope::Development)
            .expect("resolve development");

        assert_eq!(production.nodes.len(), 3);
        assert_eq!(development.nodes.len(), 4);
        assert!(
            !development
                .nodes
                .values()
                .any(|node| { node.manifest.package.name == "root-prod-dev-only" })
        );
    }
}
