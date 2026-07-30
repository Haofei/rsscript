// Evidence metadata, subjects, summaries, and producer provenance.

fn confidence(level: ConfidenceLevel, source: &str) -> Confidence {
    Confidence {
        level,
        source: Some(source.to_owned()),
    }
}

/// Base [`Evidence`] for RSScript-sourced facts.
///
/// RSScript evidence never carries the cloud/runtime correlation fields
/// (`event_id`, `time`, `event_name`, `principal`, `account`, `policy_arn`,
/// `statement_index`, `action`), so this helper sets the `kind` and leaves
/// every other field `None`. Callers fill in the fields they need with struct
/// update syntax, e.g. `Evidence { file, .. rsscript_evidence(kind) }`, which
/// produces the same value as spelling out every field by hand.
fn rsscript_evidence(kind: EvidenceKind) -> Evidence {
    Evidence {
        kind,
        file: None,
        line: None,
        column: None,
        length: None,
        symbol: None,
        reason: None,
        json_pointer: None,
        resource: None,
        provider: None,
        value: None,
        event_id: None,
        time: None,
        source: None,
        event_name: None,
        principal: None,
        account: None,
        policy_arn: None,
        statement_index: None,
        action: None,
    }
}

fn source_span(
    file: &str,
    line: usize,
    symbol: &str,
    reason: Option<String>,
    source: &str,
) -> Evidence {
    Evidence {
        file: Some(file.to_owned()),
        line: Some(line),
        symbol: Some(symbol.to_owned()),
        reason,
        source: Some(source.to_owned()),
        ..rsscript_evidence(EvidenceKind::SourceSpan)
    }
}

fn package_metadata(
    package_name: &str,
    version: &str,
    value: Option<String>,
    reason: Option<String>,
) -> Evidence {
    Evidence {
        reason,
        resource: Some(format!("{package_name}@{version}")),
        provider: Some("rsscript".to_owned()),
        value,
        source: Some(PACKAGE_REVIEW_SOURCE.to_owned()),
        ..rsscript_evidence(EvidenceKind::PackageMetadata)
    }
}

fn lockfile_supply_chain_fact(
    package_slug: &str,
    field: &str,
    json_field: &str,
    subject: &Subject,
    package: &RsScriptPackageLockPackage,
    package_index: usize,
    hash: &str,
    lockfile_path: &str,
    reason: String,
) -> Fact {
    let missing_hash = hash.is_empty();
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.lockfile.{}.{}", package_slug, field),
        kind: FactKind::SupplyChain,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if missing_hash {
            FactValue::Unknown
        } else {
            FactValue::True
        },
        confidence: confidence(ConfidenceLevel::Computed, LOCKFILE_SOURCE),
        acquisition_mode: AcquisitionMode::Lockfile,
        precision: Precision::Exact,
        evidence: vec![lockfile_entry(
            package,
            if missing_hash {
                None
            } else {
                Some(hash.to_owned())
            },
            reason,
            format!("/package/{package_index}/{json_field}"),
            lockfile_path,
        )],
        unknown_reason: missing_hash.then(|| format!("lockfile `{json_field}` is missing")),
    }
}

fn lockfile_entry(
    package: &RsScriptPackageLockPackage,
    value: Option<String>,
    reason: String,
    json_pointer: String,
    lockfile_path: &str,
) -> Evidence {
    Evidence {
        file: Some(lockfile_path.to_owned()),
        reason: Some(reason),
        json_pointer: Some(json_pointer),
        resource: Some(format!("{}@{}", package.name, package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        source: Some(LOCKFILE_SOURCE.to_owned()),
        ..rsscript_evidence(EvidenceKind::LockfileEntry)
    }
}

fn lockfile_features_summary(features: &[String]) -> String {
    if features.is_empty() {
        "default".to_owned()
    } else {
        features.join(",")
    }
}

fn lock_diff_subject(input: &RsScriptPackageLockDiffInput) -> Subject {
    Subject {
        kind: SubjectKind::DependencyEdge,
        id: format!(
            "lock-update:{}->{}",
            input.old_lock_path, input.new_lock_path
        ),
        name: Some("rsspkg.lock update".to_owned()),
        package: None,
    }
}

fn lock_diff_package_subject(package: &RsScriptPackageLockPackageChange) -> Subject {
    let version = package
        .after_version
        .as_ref()
        .or(package.before_version.as_ref())
        .map(String::as_str)
        .unwrap_or("unresolved");
    Subject {
        kind: SubjectKind::Package,
        id: format!("{}@{}", package.name, version),
        name: Some(package.name.clone()),
        package: Some(package.name.clone()),
    }
}

fn lock_diff_field_fact_kind(field: &str) -> FactKind {
    match field {
        "checksum" | "interface_hash" | "review_hash" | "native_hash" | "features" => {
            FactKind::SupplyChain
        }
        "package" | "version" | "source" => FactKind::DependencyRisk,
        _ => FactKind::PolicyResult,
    }
}

fn lock_diff_summary(input: &RsScriptPackageLockDiffInput) -> String {
    format!(
        "lock update old={} new={} old_packages={} new_packages={} changed_packages={} reasons={}",
        input.old_lock_path,
        input.new_lock_path,
        input.old_packages,
        input.new_packages,
        input.package_changes.len(),
        if input.reasons.is_empty() {
            "none".to_owned()
        } else {
            input.reasons.join("; ")
        }
    )
}

fn lock_diff_package_summary(package: &RsScriptPackageLockPackageChange) -> String {
    format!(
        "package {} version {} -> {} risk={} changes={}",
        package.name,
        package.before_version.as_deref().unwrap_or("<none>"),
        package.after_version.as_deref().unwrap_or("<none>"),
        package.risk.as_str(),
        package.changes.len()
    )
}

fn lock_diff_field_summary(field: &RsScriptPackageLockFieldChange) -> String {
    format!(
        "field {} {} -> {} risk={}",
        field.field,
        field.before.as_deref().unwrap_or("<none>"),
        field.after.as_deref().unwrap_or("<none>"),
        field.risk.as_str()
    )
}

fn lock_diff_package_evidence_file(
    input: &RsScriptPackageLockDiffInput,
    package: &RsScriptPackageLockPackageChange,
) -> String {
    if package.after_version.is_none() {
        input.old_lock_path.clone()
    } else {
        input.new_lock_path.clone()
    }
}

fn lock_diff_field_evidence_file(
    input: &RsScriptPackageLockDiffInput,
    field: &RsScriptPackageLockFieldChange,
) -> String {
    if field.after.is_none() {
        input.old_lock_path.clone()
    } else {
        input.new_lock_path.clone()
    }
}

fn lock_diff_evidence(
    input: &RsScriptPackageLockDiffInput,
    file: String,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: &str,
) -> Evidence {
    Evidence {
        file: Some(file),
        reason,
        json_pointer: Some(json_pointer.to_owned()),
        resource: Some(format!(
            "{} -> {}",
            input.old_lock_path, input.new_lock_path
        )),
        provider: Some("rsscript".to_owned()),
        value,
        source: Some(LOCKFILE_SOURCE.to_owned()),
        ..rsscript_evidence(EvidenceKind::LockfileEntry)
    }
}

fn package_review_summary(input: &RsScriptPackageReviewInput) -> String {
    format!(
        "public_apis={}, mutating_apis={}, retaining_apis={}, resource_apis={}, native_apis={}, unsafe_apis={}, unknown_apis={}, native_boundaries={}",
        input.public_apis,
        input.mutating_apis,
        input.retaining_apis,
        input.resource_apis,
        input.native_apis,
        input.unsafe_apis,
        input.unknown_apis,
        input.native_boundaries.len()
    )
}

fn package_check_summary(input: &RsScriptPackageCheckInput) -> String {
    format!(
        "package check ok={} risk={} diagnostics={} errors={} dependencies={} native_apis={} unsafe_apis={} unknown_apis={}",
        input.ok,
        input.risk.as_str(),
        input.summary.diagnostics,
        input.summary.errors,
        input.summary.dependencies,
        input.summary.native_apis,
        input.summary.unsafe_apis,
        input.summary.unknown_apis
    )
}

fn check_reasons_summary(label: &str, reasons: &[String]) -> String {
    if reasons.is_empty() {
        format!("{label} check ok")
    } else {
        format!("{label} check reasons={}", reasons.join("; "))
    }
}

fn package_check_native_summary(native: &RsScriptPackageNativeRustCheckInput) -> String {
    format!(
        "native check ok={} risk={} path={} cargo_toml={} cargo_metadata={} unsafe={} links={} build_env={} build_download={} files={} reasons={}",
        native.ok,
        native.risk.as_str(),
        native.path,
        native.cargo_toml_present,
        native.cargo_metadata_ok,
        native.unsafe_detected,
        if native.linked_libraries.is_empty() {
            "none".to_owned()
        } else {
            native.linked_libraries.join(",")
        },
        native.build_env_detected,
        native.build_download_detected,
        native.file_count,
        if native.reasons.is_empty() {
            "none".to_owned()
        } else {
            native.reasons.join("; ")
        }
    )
}

fn package_metadata_report_summary(input: &RsScriptPackageMetadataInput) -> String {
    format!(
        "package metadata {} package={} version={} risk={} mismatches={}",
        if input.verified {
            "verify"
        } else if input.written {
            "write"
        } else if input.dry_run {
            "dry-run"
        } else {
            "report"
        },
        input.package.name,
        input.package.version,
        input.risk.as_str(),
        input.mismatches.len()
    )
}

fn package_vendor_summary(input: &RsScriptPackageVendorInput) -> String {
    format!(
        "package vendor {} package={} version={} risk={} entries={} unresolved={}",
        if input.dry_run { "dry-run" } else { "write" },
        input.package.name,
        input.package.version,
        input.risk.as_str(),
        input.entries.len(),
        input.unresolved.len()
    )
}

fn package_export_summary(export: &RsScriptPackageExport) -> String {
    let mut parts = vec![
        format!("kind={}", export.kind),
        format!("classification={}", export.classification),
    ];
    if !export.reasons.is_empty() {
        parts.push(format!("reasons={}", export.reasons.join("; ")));
    }
    if !export.normalized_effects.is_empty() {
        parts.push(format!("effects={}", export.normalized_effects.join(", ")));
    }
    parts.join(", ")
}

fn native_boundary_reason(boundary: &RsScriptNativeBoundary) -> String {
    if boundary.functions.is_empty() {
        format!("native boundary in module {}", boundary.module_name)
    } else {
        format!(
            "native boundary in module {} for functions {}",
            boundary.module_name,
            boundary.functions.join(", ")
        )
    }
}

fn native_module_declaration_reason(boundary: &RsScriptNativeBoundary) -> String {
    if boundary.functions.is_empty() {
        format!("native module {} declared", boundary.module_name)
    } else {
        format!(
            "native module {} declares functions {}",
            boundary.module_name,
            boundary.functions.join(", ")
        )
    }
}

fn joined_reason(reasons: &[String]) -> Option<String> {
    (!reasons.is_empty()).then(|| reasons.join("; "))
}

fn normalized_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

const fn rsscript_provenance(adapter: &'static str, source: &'static str) -> ProducerProvenance {
    ProducerProvenance {
        name: "rssc",
        version: PRODUCER_VERSION,
        adapter,
        adapter_version: ADAPTER_VERSION,
        source,
    }
}
