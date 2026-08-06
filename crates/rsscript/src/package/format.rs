use crate::package::{
    PackageAnalysis, PackageCheck, PackageDependencyKind, PackageDiff, PackageLock,
    PackageLockDiff, PackageMetadataReport, PackageReview, PackageReviewAwaitBoundary,
    PackageReviewAwaitSite, PackageReviewDependency, PackageReviewExport, PackageTree,
    PackageTreeNode, package_risk_label,
};
use crate::review::format_review_human;

pub fn format_package_review_json(review: &PackageReview) -> String {
    serde_json::to_string(review).expect("package review JSON serialization should not fail")
}

pub fn format_package_analysis_json(analysis: &PackageAnalysis) -> String {
    serde_json::to_string(analysis).expect("package analysis JSON serialization should not fail")
}

pub fn format_package_metadata_json(metadata: &PackageMetadataReport) -> String {
    serde_json::to_string(metadata).expect("package metadata JSON serialization should not fail")
}

pub fn format_package_diff_json(diff: &PackageDiff) -> String {
    serde_json::to_string(diff).expect("package diff JSON serialization should not fail")
}

pub fn format_package_check_json(check: &PackageCheck) -> String {
    serde_json::to_string(check).expect("package check JSON serialization should not fail")
}

pub fn format_package_tree_json(tree: &PackageTree) -> String {
    serde_json::to_string(tree).expect("package tree JSON serialization should not fail")
}

pub fn format_package_lock_json(lock: &PackageLock) -> String {
    serde_json::to_string(lock).expect("package lock JSON serialization should not fail")
}

pub fn format_package_lock_toml(lock: &PackageLock) -> String {
    toml::to_string_pretty(lock).expect("package lock TOML serialization should not fail")
}

pub fn format_package_lock_diff_json(diff: &PackageLockDiff) -> String {
    serde_json::to_string(diff).expect("package lock diff JSON serialization should not fail")
}

pub fn format_package_review_human(review: &PackageReview) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package {} {} ({}) risk {}\n",
        review.package.name,
        review.package.version,
        review.package.edition,
        package_risk_label(review.risk)
    ));
    output.push_str(&format!(
        "summary: {} interface files; {} source files; {} dependencies; {} package features; {} public types; {} public sum types; {} public type aliases; {} public consts; {} public functions; {} public APIs; {} mutating APIs; {} retaining APIs; {} resource APIs; {} fresh-returning APIs; {} guarantee APIs; {} native guarantee APIs; {} native APIs; {} async APIs; {} await sites; {} parallel APIs; {} unsafe APIs; {} unknown APIs; {} diagnostics ({} errors)\n",
        review.summary.interface_files,
        review.summary.source_files,
        review.summary.dependencies,
        review.summary.package_features,
        review.summary.public_types,
        review.summary.public_sum_types,
        review.summary.public_type_aliases,
        review.summary.public_consts,
        review.summary.public_functions,
        review.summary.public_apis,
        review.summary.mutating_apis,
        review.summary.retaining_apis,
        review.summary.resource_apis,
        review.summary.fresh_returning_apis,
        review.summary.guarantee_apis,
        review.summary.native_guarantee_apis,
        review.summary.native_apis,
        review.summary.async_apis,
        review.summary.await_sites,
        review.summary.parallel_apis,
        review.summary.unsafe_apis,
        review.summary.unknown_apis,
        review.summary.diagnostics,
        review.summary.errors
    ));
    if !review.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &review.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    output.push_str(&format_package_review_dependencies_human(
        &review.dependencies,
    ));
    if let Some(native) = &review.native_rust {
        output.push_str(&format!("native rust: {}", native.path));
        if let Some(crate_name) = &native.crate_name {
            output.push_str(&format!(" crate {crate_name}"));
        }
        if !native
            .semantic
            .source_scan_best_effort
            .native_parallel_backends
            .is_empty()
        {
            output.push_str(&format!(
                " parallel_backend {}",
                native
                    .semantic
                    .source_scan_best_effort
                    .native_parallel_backends
                    .join(",")
            ));
        }
        output.push('\n');
    }
    output.push_str(&format_package_review_await_sites_human(
        &review.await_sites,
    ));
    output.push_str(&format_package_review_capabilities_human(
        &review.external_bindings,
    ));
    output.push_str(&format_package_review_exports_human(&review.exports));
    output
}

/// Render a package review as a PR-facing Markdown report: the powers it needs
/// (ranked by risk, with provider and author-declared/unknown flags), native
/// risk, reviewable boundaries, and diagnostics.
pub fn format_package_review_markdown(review: &PackageReview) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "## RSScript review: `{}` {} — risk **{}**",
        review.package.name,
        review.package.version,
        package_risk_label(review.risk)
    );
    if !review.reasons.is_empty() {
        let _ = writeln!(out, "\n**Why:**");
        for reason in &review.reasons {
            let _ = writeln!(out, "- {reason}");
        }
    }

    let mut external_bindings = review.external_bindings.clone();
    external_bindings.sort_by_key(|external_binding| match external_binding.risk {
        crate::package::types::PackageRisk::High => 0u8,
        crate::package::types::PackageRisk::Elevated => 1,
        crate::package::types::PackageRisk::Low => 2,
        crate::package::types::PackageRisk::Unknown => 3,
    });
    let mut seen = std::collections::BTreeSet::new();
    let mut rows = Vec::new();
    for external_binding in &external_bindings {
        if !seen.insert((
            external_binding.category.clone(),
            external_binding.binding_symbol.clone(),
        )) {
            continue;
        }
        let risk = match external_binding.risk {
            crate::package::types::PackageRisk::High => "high",
            crate::package::types::PackageRisk::Elevated => "medium",
            crate::package::types::PackageRisk::Low => "low",
            crate::package::types::PackageRisk::Unknown => "unknown",
        };
        let note = external_binding
            .unknown_reason
            .as_deref()
            .map(|reason| format!(" ⚠️ {reason}"))
            .unwrap_or_default();
        rows.push(format!(
            "| {} | {} | {} | `{}`{} |",
            risk,
            external_binding.category,
            external_binding.provider.as_deref().unwrap_or("—"),
            external_binding.binding_symbol,
            note
        ));
    }
    if rows.is_empty() {
        let _ = writeln!(out, "\nNo declared external_bindings.");
    } else {
        let _ = writeln!(out, "\n### Capabilities (by risk)\n");
        let _ = writeln!(out, "| risk | external_binding | provider | via |");
        let _ = writeln!(out, "|------|------------|----------|-----|");
        for row in rows {
            let _ = writeln!(out, "{row}");
        }
    }

    if let Some(native) = &review.native_rust {
        let _ = writeln!(out, "\n### Native boundary\n");
        let _ = writeln!(
            out,
            "- crate: `{}`",
            native.crate_name.as_deref().unwrap_or("?")
        );
        let _ = writeln!(out, "- path: `{}`", native.path);
    }

    let errors = review
        .diagnostics
        .iter()
        .filter(|d| d.severity.is_error())
        .count();
    if errors > 0 {
        let _ = writeln!(
            out,
            "\n### Diagnostics\n\n**{errors} error(s)** — this review is not valid for gating."
        );
    }
    out
}

/// Distinct external_bindings the package requires, ranked high-risk first, so a
/// reviewer sees the powers (and any unrecognized ones) at a glance.
fn format_package_review_capabilities_human(
    external_bindings: &[crate::package::types::PackageExternalBinding],
) -> String {
    if external_bindings.is_empty() {
        return String::new();
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut rows: Vec<(u8, String)> = Vec::new();
    for external_binding in external_bindings {
        if !seen.insert((
            external_binding.category.clone(),
            external_binding.binding_symbol.clone(),
        )) {
            continue;
        }
        let (rank, label) = match external_binding.risk {
            crate::package::types::PackageRisk::High => (0u8, "high"),
            crate::package::types::PackageRisk::Elevated => (1, "medium"),
            crate::package::types::PackageRisk::Low => (2, "low"),
            crate::package::types::PackageRisk::Unknown => (3, "unknown"),
        };
        let mut line = format!(
            "  [{label}] {} via {}",
            external_binding.category, external_binding.binding_symbol
        );
        if let Some(provider) = &external_binding.provider {
            line.push_str(&format!(" (provider {provider})"));
        }
        if let Some(reason) = &external_binding.unknown_reason {
            line.push_str(&format!("  -- {reason}"));
        }
        rows.push((rank, line));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut output = String::from("external_bindings (by risk):\n");
    for (_, line) in rows {
        output.push_str(&line);
        output.push('\n');
    }
    output
}

pub fn format_package_metadata_human(metadata: &PackageMetadataReport) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package metadata {} {} {} risk {}\n",
        metadata.package.name,
        metadata.package.version,
        if metadata.verified {
            "verified"
        } else if metadata.dry_run {
            "dry-run"
        } else {
            "wrote"
        },
        package_risk_label(metadata.risk)
    ));
    output.push_str(&format!("metadata path: {}\n", metadata.metadata_path));
    if !metadata.mismatches.is_empty() {
        output.push_str("metadata mismatches:\n");
        for mismatch in &metadata.mismatches {
            let actual = mismatch
                .actual_sha256
                .as_deref()
                .map(|actual| format!(" actual={actual}"))
                .unwrap_or_default();
            output.push_str(&format!(
                "  - {} {} {}: {} expected={}{}\n",
                mismatch.artifact,
                mismatch.kind,
                mismatch.path,
                mismatch.message,
                mismatch.expected_sha256,
                actual
            ));
        }
    }
    output.push_str(&format!(
        "summary: {} interface files; {} source files; {} dependencies; {} package features; {} public types; {} public sum types; {} public type aliases; {} public consts; {} public functions; {} public APIs; {} mutating APIs; {} retaining APIs; {} resource APIs; {} fresh-returning APIs; {} guarantee APIs; {} native guarantee APIs; {} native APIs; {} async APIs; {} await sites; {} parallel APIs; {} unsafe APIs; {} unknown APIs; {} diagnostics ({} errors)\n",
        metadata.metadata.summary.interface_files,
        metadata.metadata.summary.source_files,
        metadata.metadata.summary.dependencies,
        metadata.metadata.summary.package_features,
        metadata.metadata.summary.public_types,
        metadata.metadata.summary.public_sum_types,
        metadata.metadata.summary.public_type_aliases,
        metadata.metadata.summary.public_consts,
        metadata.metadata.summary.public_functions,
        metadata.metadata.summary.public_apis,
        metadata.metadata.summary.mutating_apis,
        metadata.metadata.summary.retaining_apis,
        metadata.metadata.summary.resource_apis,
        metadata.metadata.summary.fresh_returning_apis,
        metadata.metadata.summary.guarantee_apis,
        metadata.metadata.summary.native_guarantee_apis,
        metadata.metadata.summary.native_apis,
        metadata.metadata.summary.async_apis,
        metadata.metadata.summary.await_sites,
        metadata.metadata.summary.parallel_apis,
        metadata.metadata.summary.unsafe_apis,
        metadata.metadata.summary.unknown_apis,
        metadata.metadata.summary.diagnostics,
        metadata.metadata.summary.errors
    ));
    for reason in &metadata.reasons {
        output.push_str(&format!("reason: {reason}\n"));
    }
    output.push_str(&format_package_review_dependencies_human(
        &metadata.metadata.dependencies,
    ));
    output.push_str(&format_package_review_await_sites_human(
        &metadata.metadata.await_sites,
    ));
    output.push_str(&format_package_review_exports_human(
        &metadata.metadata.exports,
    ));
    output
}

fn format_package_review_dependencies_human(dependencies: &[PackageReviewDependency]) -> String {
    if dependencies.is_empty() {
        return String::new();
    }
    let mut output = String::from("dependencies:\n");
    for dependency in dependencies {
        output.push_str(&format!(
            "  - {} {} {}",
            dependency_kind_label(dependency.dependency_kind),
            dependency.name,
            dependency.source
        ));
        if let Some(requirement) = &dependency.requirement {
            output.push_str(&format!(" requirement {requirement}"));
        }
        if !dependency.features.is_empty() {
            output.push_str(&format!(" features {}", dependency.features.join(",")));
        }
        if dependency.compile_only {
            output.push_str(" compile_only");
        }
        if dependency.test_only {
            output.push_str(" test_only");
        }
        if dependency.platform_provided {
            output.push_str(" platform_provided");
        }
        output.push('\n');
    }
    output
}

fn dependency_kind_label(kind: PackageDependencyKind) -> &'static str {
    match kind {
        PackageDependencyKind::Root => "root",
        PackageDependencyKind::Normal => "dependency",
        PackageDependencyKind::Dev => "dev-dependency",
    }
}

fn format_package_review_await_sites_human(await_sites: &[PackageReviewAwaitSite]) -> String {
    if await_sites.is_empty() {
        return String::new();
    }
    let mut output = String::from("await sites:\n");
    for site in await_sites {
        let callee = site.callee.as_deref().unwrap_or("<unknown>");
        output.push_str(&format!(
            "  - {} awaits {} ({}) at {}:{}:{}",
            site.function,
            callee,
            await_boundary_label(site.boundary),
            site.span.file,
            site.span.line,
            site.span.column
        ));
        if !site.live_across_await.is_empty() {
            output.push_str(&format!(
                " live_across [{}]",
                site.live_across_await.join(", ")
            ));
        }
        output.push('\n');
    }
    output
}

fn await_boundary_label(boundary: PackageReviewAwaitBoundary) -> &'static str {
    match boundary {
        PackageReviewAwaitBoundary::RuntimePending => "runtime_pending",
        PackageReviewAwaitBoundary::NativePending => "native_pending",
        PackageReviewAwaitBoundary::RssCall => "rss_call",
        PackageReviewAwaitBoundary::Unknown => "unknown",
    }
}

fn format_package_review_exports_human(exports: &[PackageReviewExport]) -> String {
    if exports.is_empty() {
        return String::new();
    }
    let mut output = String::from("exports:\n");
    for export in exports {
        output.push_str(&format!(
            "  - {} {}: {}",
            export.kind, export.name, export.classification
        ));
        if !export.reasons.is_empty() {
            output.push_str(&format!(" ({})", export.reasons.join(", ")));
        }
        output.push('\n');
    }
    output
}

pub fn format_package_diff_human(diff: &PackageDiff) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package diff {} {} -> {} risk {}\n",
        diff.new_package.name,
        diff.old_package.version,
        diff.new_package.version,
        package_risk_label(diff.risk)
    ));
    if !diff.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &diff.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    if diff.old_review.await_sites != diff.new_review.await_sites {
        output.push_str(&format!(
            "await sites: {} -> {}\n",
            diff.old_review.await_sites, diff.new_review.await_sites
        ));
    }
    if !diff.external_binding_changes.is_empty() {
        output.push_str("external_binding changes:\n");
        for change in &diff.external_binding_changes {
            let sign = match change.change {
                crate::package::types::PackageExternalBindingChangeKind::Added => "+",
                crate::package::types::PackageExternalBindingChangeKind::Removed => "-",
            };
            let risk = match change.risk {
                crate::package::types::PackageRisk::High => "high",
                crate::package::types::PackageRisk::Elevated => "medium",
                crate::package::types::PackageRisk::Low => "low",
                crate::package::types::PackageRisk::Unknown => "unknown",
            };
            output.push_str(&format!(
                "  {sign} [{risk}] {} via {}\n",
                change.category, change.binding_symbol
            ));
        }
    }
    for change in &diff.manifest_changes {
        output.push_str(&format!(
            "{} {}: {} -> {} ({})\n",
            change.kind,
            change.name,
            change.before.as_deref().unwrap_or("<none>"),
            change.after.as_deref().unwrap_or("<none>"),
            package_risk_label(change.risk)
        ));
    }
    for change in &diff.interface_changes {
        output.push_str(&format!(
            "interface {} {:?} ({})\n",
            change.file,
            change.change,
            package_risk_label(change.risk)
        ));
        if !change.findings.is_empty() {
            output.push_str(&format_review_human(&change.findings));
        }
    }
    output
}

pub fn format_package_check_human(check: &PackageCheck) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package check {} {} ({}) {} risk {}\n",
        check.package.name,
        check.package.version,
        check.package.edition,
        if check.ok { "ok" } else { "failed" },
        package_risk_label(check.risk)
    ));
    output.push_str(&format!(
        "summary: {} interface files; {} source files; {} dependencies; {} package features; {} public types; {} public sum types; {} public type aliases; {} public consts; {} public functions; {} public APIs; {} mutating APIs; {} retaining APIs; {} resource APIs; {} fresh-returning APIs; {} native APIs; {} async APIs; {} await sites; {} parallel APIs; {} unsafe APIs; {} unknown APIs; {} diagnostics ({} errors)\n",
        check.summary.interface_files,
        check.summary.source_files,
        check.summary.dependencies,
        check.summary.package_features,
        check.summary.public_types,
        check.summary.public_sum_types,
        check.summary.public_type_aliases,
        check.summary.public_consts,
        check.summary.public_functions,
        check.summary.public_apis,
        check.summary.mutating_apis,
        check.summary.retaining_apis,
        check.summary.resource_apis,
        check.summary.fresh_returning_apis,
        check.summary.native_apis,
        check.summary.async_apis,
        check.summary.await_sites,
        check.summary.parallel_apis,
        check.summary.unsafe_apis,
        check.summary.unknown_apis,
        check.summary.diagnostics,
        check.summary.errors
    ));
    output.push_str(&format!(
        "graph: {} ({})\n",
        if check.graph.ok { "ok" } else { "failed" },
        package_risk_label(check.graph.risk)
    ));
    output.push_str(&format!(
        "lock: {} {}\n",
        check.lock.path,
        if check.lock.matches {
            "matches"
        } else if check.lock.present {
            "stale"
        } else {
            "missing"
        }
    ));
    if let Some(native) = &check.native_rust {
        output.push_str(&format!(
            "native rust: {} cargo_toml={} cargo_metadata={} package={} targets={} unsafe={} links={} build_env={} build_download={} files={}\n",
            native.path,
            native.cargo_toml_present,
            native.cargo_metadata_ok,
            native.cargo_package_name.as_deref().unwrap_or("<unknown>"),
            if native.target_kinds.is_empty() {
                "<none>".to_string()
            } else {
                native.target_kinds.join(",")
            },
            native.unsafe_detected,
            if native.linked_libraries.is_empty() {
                "<none>".to_string()
            } else {
                native.linked_libraries.join(",")
            },
            native.build_env_detected,
            native.build_download_detected,
            native.file_count
        ));
    }
    if !check.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &check.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    output
}

pub fn format_package_tree_human(tree: &PackageTree) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package tree: {} packages; {} path deps; {} unresolved; {} native; {} build execution; {} high risk; {} unknown\n",
        tree.summary.packages,
        tree.summary.path_dependencies,
        tree.summary.unresolved_dependencies,
        tree.summary.native_packages,
        tree.summary.build_execution_packages,
        tree.summary.high_risk_packages,
        tree.summary.unknown_risk_packages
    ));
    format_package_tree_node_human(&tree.root, "", true, &mut output);
    output
}

pub fn format_package_lock_diff_human(diff: &PackageLockDiff) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package lock update {} -> {} risk {}\n",
        diff.old_lock_path,
        diff.new_lock_path,
        package_risk_label(diff.risk)
    ));
    if !diff.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &diff.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    for package in &diff.package_changes {
        output.push_str(&format!(
            "package {}: {} -> {} ({})\n",
            package.name,
            package.before_version.as_deref().unwrap_or("<none>"),
            package.after_version.as_deref().unwrap_or("<none>"),
            package_risk_label(package.risk)
        ));
        for change in &package.changes {
            output.push_str(&format!(
                "  {}: {} -> {} ({})\n",
                change.field,
                change.before.as_deref().unwrap_or("<none>"),
                change.after.as_deref().unwrap_or("<none>"),
                package_risk_label(change.risk)
            ));
        }
    }
    output
}

fn format_package_tree_node_human(
    node: &PackageTreeNode,
    prefix: &str,
    is_last: bool,
    output: &mut String,
) {
    let connector = if node.dependency_kind == PackageDependencyKind::Root {
        ""
    } else if is_last {
        "`-- "
    } else {
        "|-- "
    };
    output.push_str(prefix);
    output.push_str(connector);
    output.push_str(&node.name);
    if let Some(version) = &node.version {
        output.push(' ');
        output.push_str(version);
    }
    if let Some(requirement) = &node.requirement {
        output.push_str(" req ");
        output.push_str(requirement);
    }
    output.push_str(" [");
    output.push_str(package_risk_label(node.risk));
    if node.native {
        output.push_str(", native");
    }
    if !node.features.is_empty() {
        output.push_str(", features ");
        output.push_str(&node.features.join(","));
    }
    output.push_str("]\n");

    let child_prefix = if node.dependency_kind == PackageDependencyKind::Root {
        String::new()
    } else if is_last {
        format!("{prefix}    ")
    } else {
        format!("{prefix}|   ")
    };
    for (index, dependency) in node.dependencies.iter().enumerate() {
        format_package_tree_node_human(
            dependency,
            &child_prefix,
            index + 1 == node.dependencies.len(),
            output,
        );
    }
}
