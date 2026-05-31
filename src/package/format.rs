use crate::package::{
    PackageCheck, PackageDependencyKind, PackageDiff, PackageLock, PackageLockDiff,
    PackageMetadataReport, PackagePublishDryRun, PackageReview, PackageReviewExport, PackageTree,
    PackageTreeNode, PackageVendorReport, package_risk_label,
};
use crate::review::format_review_human;

pub fn format_package_review_json(review: &PackageReview) -> String {
    serde_json::to_string(review).expect("package review JSON serialization should not fail")
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

pub fn format_package_publish_json(publish: &PackagePublishDryRun) -> String {
    serde_json::to_string(publish).expect("package publish JSON serialization should not fail")
}

pub fn format_package_vendor_json(vendor: &PackageVendorReport) -> String {
    serde_json::to_string(vendor).expect("package vendor JSON serialization should not fail")
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
        "summary: {} interface files; {} source files; {} dependencies; {} package features; {} public types; {} public functions; {} public APIs; {} mutating APIs; {} retaining APIs; {} resource APIs; {} fresh-returning APIs; {} guarantee APIs; {} native guarantee APIs; {} native APIs; {} parallel APIs; {} unsafe APIs; {} unknown APIs; {} diagnostics ({} errors)\n",
        review.summary.interface_files,
        review.summary.source_files,
        review.summary.dependencies,
        review.summary.package_features,
        review.summary.public_types,
        review.summary.public_functions,
        review.summary.public_apis,
        review.summary.mutating_apis,
        review.summary.retaining_apis,
        review.summary.resource_apis,
        review.summary.fresh_returning_apis,
        review.summary.guarantee_apis,
        review.summary.native_guarantee_apis,
        review.summary.native_apis,
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
    if !review.features.is_empty() {
        output.push_str(&format!(
            "package features: {}\n",
            review.features.join(", ")
        ));
    }
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
    output.push_str(&format_package_review_exports_human(&review.exports));
    output
}

pub fn format_package_metadata_human(metadata: &PackageMetadataReport) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package metadata {} {} {} risk {}\n",
        metadata.package.name,
        metadata.package.version,
        if metadata.dry_run { "dry-run" } else { "wrote" },
        package_risk_label(metadata.risk)
    ));
    output.push_str(&format!("metadata path: {}\n", metadata.metadata_path));
    output.push_str(&format!(
        "summary: {} interface files; {} source files; {} public types; {} public functions; {} public APIs; {} mutating APIs; {} retaining APIs; {} resource APIs; {} fresh-returning APIs; {} guarantee APIs; {} native guarantee APIs; {} native APIs; {} parallel APIs; {} unsafe APIs; {} unknown APIs; {} diagnostics ({} errors)\n",
        metadata.metadata.summary.interface_files,
        metadata.metadata.summary.source_files,
        metadata.metadata.summary.public_types,
        metadata.metadata.summary.public_functions,
        metadata.metadata.summary.public_apis,
        metadata.metadata.summary.mutating_apis,
        metadata.metadata.summary.retaining_apis,
        metadata.metadata.summary.resource_apis,
        metadata.metadata.summary.fresh_returning_apis,
        metadata.metadata.summary.guarantee_apis,
        metadata.metadata.summary.native_guarantee_apis,
        metadata.metadata.summary.native_apis,
        metadata.metadata.summary.parallel_apis,
        metadata.metadata.summary.unsafe_apis,
        metadata.metadata.summary.unknown_apis,
        metadata.metadata.summary.diagnostics,
        metadata.metadata.summary.errors
    ));
    for reason in &metadata.reasons {
        output.push_str(&format!("reason: {reason}\n"));
    }
    if !metadata.metadata.features.is_empty() {
        output.push_str(&format!(
            "package features: {}\n",
            metadata.metadata.features.join(", ")
        ));
    }
    output.push_str(&format_package_review_exports_human(
        &metadata.metadata.exports,
    ));
    output
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
        "summary: {} interface files; {} source files; {} dependencies; {} package features; {} public types; {} public functions; {} public APIs; {} mutating APIs; {} retaining APIs; {} resource APIs; {} fresh-returning APIs; {} native APIs; {} parallel APIs; {} unsafe APIs; {} unknown APIs; {} diagnostics ({} errors)\n",
        check.summary.interface_files,
        check.summary.source_files,
        check.summary.dependencies,
        check.summary.package_features,
        check.summary.public_types,
        check.summary.public_functions,
        check.summary.public_apis,
        check.summary.mutating_apis,
        check.summary.retaining_apis,
        check.summary.resource_apis,
        check.summary.fresh_returning_apis,
        check.summary.native_apis,
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

pub fn format_package_publish_human(publish: &PackagePublishDryRun) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package publish dry-run {} {} {} risk {}\n",
        publish.package.name,
        publish.package.version,
        if publish.ready { "ready" } else { "blocked" },
        package_risk_label(publish.risk)
    ));
    output.push_str(&format!(
        "archive: {} {} files {}\n",
        publish.archive_format,
        publish.archive_files.len(),
        publish.archive_hash
    ));
    output.push_str(&format!(
        "registry index: {} {} {} risk {} native={} unsafe={}\n",
        publish.registry_index.schema,
        publish.registry_index.name,
        publish.registry_index.version,
        package_risk_label(publish.registry_index.risk),
        publish.registry_index.native,
        publish.registry_index.unsafe_boundary
    ));
    if let Some(target) = &publish.registry_target {
        output.push_str(&format!(
            "registry target: {} index={} archive_manifest={}\n",
            target.registry_dir, target.index_path, target.archive_manifest_path
        ));
    }
    for check in &publish.checks {
        output.push_str(&format!(
            "{}: {} ({}) {}\n",
            check.name,
            if check.ok { "ok" } else { "failed" },
            package_risk_label(check.risk),
            check.detail
        ));
    }
    if !publish.reasons.is_empty() {
        output.push_str("reasons:\n");
        for reason in &publish.reasons {
            output.push_str(&format!("  - {reason}\n"));
        }
    }
    output
}

pub fn format_package_vendor_human(vendor: &PackageVendorReport) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "package vendor {} {} {} risk {}\n",
        vendor.package.name,
        vendor.package.version,
        if vendor.dry_run { "dry-run" } else { "wrote" },
        package_risk_label(vendor.risk)
    ));
    output.push_str(&format!("vendor dir: {}\n", vendor.vendor_dir));
    for entry in &vendor.entries {
        output.push_str(&format!(
            "vendored {} {} -> {} {}\n",
            entry.name, entry.version, entry.vendor_path, entry.checksum
        ));
    }
    for dependency in &vendor.unresolved {
        output.push_str(&format!(
            "unresolved {} {} ({})\n",
            dependency.name, dependency.source, dependency.reason
        ));
    }
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
