// Pure candidate fact and edge construction from normalized inputs.

pub fn review_map_to_facts(input: &RsScriptReviewMapInput) -> Vec<Fact> {
    let mut facts = module_use_facts(input);
    facts.extend(
        input
            .regions
            .iter()
            .filter_map(|region| match region.classification {
                RsScriptClassification::Foldable => None,
                RsScriptClassification::Unknown => Some(Fact {
                    schema: FACT_SCHEMA.to_owned(),
                    id: format!(
                        "fact.review_map.{}.unknown",
                        normalized_id(&function_subject_id(
                            &input.package_name,
                            &region.function_name
                        ))
                    ),
                    kind: FactKind::Unknown,
                    role: None,
                    subject: function_subject(&input.package_name, &region.function_name),
                    capability: None,
                    value: FactValue::Unknown,
                    confidence: confidence(ConfidenceLevel::Unknown, REVIEW_MAP_SOURCE),
                    acquisition_mode: AcquisitionMode::CompilerContract,
                    precision: Precision::Exact,
                    evidence: vec![source_span(
                        &region.file,
                        region.line,
                        &region.function_name,
                        joined_reason(&region.reasons),
                        REVIEW_MAP_SOURCE,
                    )],
                    unknown_reason: joined_reason(&region.reasons),
                }),
                RsScriptClassification::ReviewRequired => {
                    let kind = classify_review_required(&region.reasons);
                    let kind_name: String = kind.clone().into();
                    Some(Fact {
                        schema: FACT_SCHEMA.to_owned(),
                        id: format!(
                            "fact.review_map.{}.{}",
                            normalized_id(&function_subject_id(
                                &input.package_name,
                                &region.function_name
                            )),
                            normalized_id(&kind_name)
                        ),
                        kind,
                        role: None,
                        subject: function_subject(&input.package_name, &region.function_name),
                        capability: None,
                        value: FactValue::True,
                        confidence: confidence(ConfidenceLevel::Authoritative, REVIEW_MAP_SOURCE),
                        acquisition_mode: AcquisitionMode::CompilerContract,
                        precision: Precision::Exact,
                        evidence: vec![source_span(
                            &region.file,
                            region.line,
                            &region.function_name,
                            joined_reason(&region.reasons),
                            REVIEW_MAP_SOURCE,
                        )],
                        unknown_reason: None,
                    })
                }
            }),
    );
    facts
}

/// Convert RSScript package review into REIR facts.
pub fn package_review_to_facts(input: &RsScriptPackageReviewInput) -> Vec<Fact> {
    let mut facts = Vec::new();
    let package_subject = package_subject(&input.package_name, &input.version);
    let package_slug = normalized_id(&package_subject.id);
    let package_summary = package_review_summary(input);

    facts.push(Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package.{}.risk", package_slug),
        kind: FactKind::PackageRisk,
        role: None,
        subject: package_subject.clone(),
        capability: None,
        value: package_risk_value(&input.risk),
        confidence: confidence(package_risk_confidence(&input.risk), PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Category,
        evidence: vec![package_metadata(
            &input.package_name,
            &input.version,
            Some(input.risk.as_str().to_owned()),
            Some(package_summary.clone()),
        )],
        unknown_reason: matches!(input.risk, RsScriptPackageRisk::Unknown)
            .then(|| "package risk could not be determined".to_owned()),
    });

    if input.native_apis > 0 {
        facts.push(capability_fact(
            format!("fact.package.{}.capability.runtime_native", package_slug),
            package_subject.clone(),
            CapabilityCategory::RuntimeNative,
            input.native_apis,
            &input.package_name,
            &input.version,
            package_summary.clone(),
        ));
    }

    if input.unsafe_apis > 0 {
        facts.push(capability_fact(
            format!("fact.package.{}.capability.runtime_unsafe", package_slug),
            package_subject.clone(),
            CapabilityCategory::RuntimeUnsafe,
            input.unsafe_apis,
            &input.package_name,
            &input.version,
            package_summary.clone(),
        ));
    }
    if let Some(scan) = &input.native_source_scan {
        facts.extend(native_source_scan_facts(
            input,
            scan,
            &package_subject,
            &package_slug,
        ));
    }
    facts.extend(native_author_declaration_facts(
        input,
        &package_subject,
        &package_slug,
    ));
    facts.extend(native_cargo_feature_facts(input, &package_slug));
    facts.extend(await_site_facts(input, &package_slug));
    facts.extend(diagnostic_facts(input, &package_subject, &package_slug));
    facts.extend(package_feature_facts(input, &package_slug));
    facts.extend(provider_implementation_facts(input, &package_slug));
    facts.extend(package_capability_facts(input, &package_slug));

    for dependency in &input.dependencies {
        let dependency_subject = dependency_subject(dependency);
        let dependency_slug = normalized_id(&dependency_subject.id);
        facts.push(build_dependency_risk_fact(
            format!("fact.dependency.{}.risk", dependency_slug),
            dependency_subject,
            dependency,
            &input.package_name,
            &input.version,
        ));
    }

    for export in &input.exports {
        let export_subject = public_contract_subject(&input.package_name, export);
        facts.push(build_public_contract_fact(
            format!(
                "fact.public_contract.{}.{}",
                package_slug,
                normalized_id(&export_subject.id)
            ),
            export_subject.clone(),
            export,
            &input.package_name,
            &input.version,
        ));
        if let Some(protocol_fact) = protocol_impl_fact(
            export,
            &export_subject,
            &input.package_name,
            &input.version,
            &package_slug,
        ) {
            facts.push(protocol_fact);
        }
        if let Some(protocol_fact) = protocol_declaration_fact(
            export,
            &export_subject,
            &input.package_name,
            &input.version,
            &package_slug,
        ) {
            facts.push(protocol_fact);
        }
        for category in standard_library_export_capabilities(export) {
            facts.push(export_capability_fact(
                format!(
                    "fact.public_contract.{}.{}.capability.{}",
                    package_slug,
                    normalized_id(&export_subject.id),
                    normalized_id(&String::from(category.clone()))
                ),
                export_subject.clone(),
                export,
                category,
                &input.package_name,
                &input.version,
            ));
        }
    }
    facts.extend(protocol_method_contract_facts(
        input,
        &package_slug,
        &input
            .exports
            .iter()
            .filter(|export| export.kind == "protocol")
            .map(|export| export.name.clone())
            .collect::<Vec<_>>(),
    ));

    for boundary in &input.native_boundaries {
        let boundary_subject = native_boundary_subject(&input.package_name, &boundary.module_name);
        facts.push(build_native_boundary_fact(
            format!(
                "fact.native_module_declaration.{}",
                normalized_id(&boundary_subject.id)
            ),
            FactKind::NativeModuleDeclaration,
            boundary_subject.clone(),
            boundary,
            native_module_declaration_reason(boundary),
        ));
        facts.push(build_native_boundary_fact(
            format!(
                "fact.native_boundary.{}",
                normalized_id(&boundary_subject.id)
            ),
            FactKind::NativeBoundary,
            boundary_subject,
            boundary,
            native_boundary_reason(boundary),
        ));
    }

    facts
}

/// Convert RSScript semantic lockfile entries into supply-chain REIR facts.
pub fn package_lock_to_facts(input: &RsScriptPackageLockInput) -> Vec<Fact> {
    let mut facts = Vec::new();
    let lockfile_path = input.lockfile_path.as_deref().unwrap_or("rsspkg.lock");
    for (index, package) in input.packages.iter().enumerate() {
        let subject = package_subject(&package.name, &package.version);
        let package_slug = normalized_id(&subject.id);
        facts.push(lockfile_supply_chain_fact(
            &package_slug,
            "checksum",
            "checksum",
            &subject,
            package,
            index,
            &package.checksum,
            lockfile_path,
            format!(
                "package checksum for {}@{} source={}",
                package.name, package.version, package.source
            ),
        ));
        facts.push(lockfile_supply_chain_fact(
            &package_slug,
            "effective_interface_hash",
            "interface_hash",
            &subject,
            package,
            index,
            &package.interface_hash,
            lockfile_path,
            format!(
                "effective_interface_hash={} features={}",
                package.interface_hash,
                lockfile_features_summary(&package.features)
            ),
        ));
        facts.push(lockfile_supply_chain_fact(
            &package_slug,
            "review_hash",
            "review_hash",
            &subject,
            package,
            index,
            &package.review_hash,
            lockfile_path,
            "review metadata hash".to_owned(),
        ));
        if let Some(native_hash) = &package.native_hash {
            facts.push(lockfile_supply_chain_fact(
                &package_slug,
                "native_hash",
                "native_hash",
                &subject,
                package,
                index,
                native_hash,
                lockfile_path,
                "native wrapper source hash".to_owned(),
            ));
        }
    }
    facts
}

/// Convert RSScript package check output into REIR policy facts.
pub fn package_check_to_facts(input: &RsScriptPackageCheckInput) -> Vec<Fact> {
    let subject = package_subject(&input.package.name, &input.package.version);
    let package_slug = normalized_id(&subject.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_check.{}.status", package_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if input.ok {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![package_check_evidence(
            input,
            None,
            Some(package_check_summary(input)),
            Some("/ok"),
        )],
        unknown_reason: (!input.ok).then(|| "package check failed".to_owned()),
    }];

    facts.push(package_check_policy_fact(
        &package_slug,
        "graph",
        &subject,
        input,
        input.graph.ok,
        &input.graph.risk,
        check_reasons_summary("graph", &input.graph.reasons),
        "/graph",
    ));
    facts.push(package_check_lock_policy_fact(
        &package_slug,
        &subject,
        input,
        input.lock.matches,
        &input.lock.risk,
        check_reasons_summary("lock", &input.lock.reasons),
    ));

    for (index, change) in input.lock.package_changes.iter().enumerate() {
        let package_subject = lock_diff_package_subject(change);
        let change_slug = normalized_id(&change.name);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.package_check.{}.lock_change.{}",
                package_slug, change_slug
            ),
            kind: FactKind::DependencyRisk,
            role: None,
            subject: package_subject,
            capability: None,
            value: package_risk_value(&change.risk),
            confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
            acquisition_mode: AcquisitionMode::Lockfile,
            precision: Precision::Category,
            evidence: vec![package_check_lock_evidence(
                input,
                Some(change.risk.as_str().to_owned()),
                Some(lock_diff_package_summary(change)),
                &format!("/lock/package_changes/{index}"),
            )],
            unknown_reason: matches!(change.risk, RsScriptPackageRisk::Unknown)
                .then(|| format!("lock change for package `{}` is unknown", change.name)),
        });
        for (field_index, field) in change.changes.iter().enumerate() {
            facts.push(Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package_check.{}.lock_change.{}.field.{}",
                    package_slug,
                    change_slug,
                    normalized_id(&field.field)
                ),
                kind: lock_diff_field_fact_kind(&field.field),
                role: None,
                subject: lock_diff_package_subject(change),
                capability: None,
                value: package_risk_value(&field.risk),
                confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
                acquisition_mode: AcquisitionMode::Lockfile,
                precision: Precision::Exact,
                evidence: vec![package_check_lock_evidence(
                    input,
                    field.after.clone().or_else(|| field.before.clone()),
                    Some(lock_diff_field_summary(field)),
                    &format!("/lock/package_changes/{index}/changes/{field_index}"),
                )],
                unknown_reason: matches!(field.risk, RsScriptPackageRisk::Unknown).then(|| {
                    format!(
                        "stale lock field `{}` for package `{}` is unknown",
                        field.field, change.name
                    )
                }),
            });
        }
    }

    facts.extend(package_check_provider_implementation_facts(
        input,
        &package_slug,
    ));

    if let Some(native) = &input.native_rust {
        facts.push(package_check_policy_fact(
            &package_slug,
            "native",
            &subject,
            input,
            native.ok,
            &native.risk,
            package_check_native_summary(native),
            "/native_rust",
        ));
        if native.unsafe_detected {
            facts.push(package_check_boundary_fact(
                &package_slug,
                "unsafe",
                &subject,
                input,
                FactKind::UnsafeBoundary,
                "native Rust unsafe detected",
                "/native_rust/unsafe_detected",
            ));
        }
        if native.build_env_detected || native.build_download_detected {
            facts.push(package_check_boundary_fact(
                &package_slug,
                "build_time",
                &subject,
                input,
                FactKind::BuildTimeExecution,
                "native Rust build script risk detected",
                "/native_rust",
            ));
        }
    }

    for (index, diagnostic) in input.diagnostics.iter().enumerate() {
        let span = diagnostic.spans.first();
        let diagnostic_subject = span
            .filter(|span| !span.file.is_empty())
            .map(|span| Subject {
                kind: SubjectKind::CodeFile,
                id: format!("{}::{}", input.package.name, span.file),
                name: Some(span.file.clone()),
                package: Some(input.package.name.clone()),
            })
            .unwrap_or_else(|| subject.clone());
        let diagnostic_unknown = matches!(diagnostic_fact_value(diagnostic), FactValue::Unknown);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.package_check.{}.diagnostic.{}.{}",
                package_slug,
                index,
                normalized_id(&diagnostic.code)
            ),
            kind: FactKind::Diagnostic,
            role: None,
            subject: diagnostic_subject,
            capability: None,
            value: if diagnostic_unknown {
                FactValue::Unknown
            } else {
                FactValue::True
            },
            confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_CHECK_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![package_check_diagnostic_evidence(input, diagnostic, span)],
            unknown_reason: diagnostic_unknown.then(|| diagnostic.summary.clone()),
        });
    }

    facts
}

/// Convert RSScript semantic lockfile update reviews into REIR facts.
pub fn package_lock_diff_to_facts(input: &RsScriptPackageLockDiffInput) -> Vec<Fact> {
    let subject = lock_diff_subject(input);
    let diff_slug = normalized_id(&subject.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.lock_update.{}.risk", diff_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: package_risk_value(&input.risk),
        confidence: confidence(ConfidenceLevel::Computed, LOCKFILE_SOURCE),
        acquisition_mode: AcquisitionMode::Lockfile,
        precision: Precision::Category,
        evidence: vec![lock_diff_evidence(
            input,
            input.new_lock_path.clone(),
            Some(input.risk.as_str().to_owned()),
            Some(lock_diff_summary(input)),
            "/risk",
        )],
        unknown_reason: matches!(input.risk, RsScriptPackageRisk::Unknown)
            .then(|| "lock update risk is unknown".to_owned()),
    }];

    for (package_index, package) in input.package_changes.iter().enumerate() {
        let package_subject = lock_diff_package_subject(package);
        let package_slug = normalized_id(&format!("{}::{}", subject.id, package.name));
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.lock_update.{}.package.{}.risk",
                diff_slug, package_slug
            ),
            kind: FactKind::DependencyRisk,
            role: None,
            subject: package_subject.clone(),
            capability: None,
            value: package_risk_value(&package.risk),
            confidence: confidence(ConfidenceLevel::Computed, LOCKFILE_SOURCE),
            acquisition_mode: AcquisitionMode::Lockfile,
            precision: Precision::Category,
            evidence: vec![lock_diff_evidence(
                input,
                lock_diff_package_evidence_file(input, package),
                Some(package.risk.as_str().to_owned()),
                Some(lock_diff_package_summary(package)),
                &format!("/package_changes/{package_index}"),
            )],
            unknown_reason: matches!(package.risk, RsScriptPackageRisk::Unknown)
                .then(|| format!("lock update package `{}` risk is unknown", package.name)),
        });
        for (field_index, field) in package.changes.iter().enumerate() {
            facts.push(Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.lock_update.{}.package.{}.field.{}",
                    diff_slug,
                    package_slug,
                    normalized_id(&field.field)
                ),
                kind: lock_diff_field_fact_kind(&field.field),
                role: None,
                subject: package_subject.clone(),
                capability: None,
                value: package_risk_value(&field.risk),
                confidence: confidence(ConfidenceLevel::Computed, LOCKFILE_SOURCE),
                acquisition_mode: AcquisitionMode::Lockfile,
                precision: Precision::Exact,
                evidence: vec![lock_diff_evidence(
                    input,
                    lock_diff_field_evidence_file(input, field),
                    field.after.clone().or_else(|| field.before.clone()),
                    Some(lock_diff_field_summary(field)),
                    &format!("/package_changes/{package_index}/changes/{field_index}"),
                )],
                unknown_reason: matches!(field.risk, RsScriptPackageRisk::Unknown).then(|| {
                    format!(
                        "lock update field `{}` for package `{}` is unknown",
                        field.field, package.name
                    )
                }),
            });
        }
    }
    facts
}

/// Convert RSScript dependency tree nodes into graph-risk REIR facts.
pub fn package_tree_to_facts(input: &RsScriptPackageTreeInput) -> Vec<Fact> {
    let mut facts = Vec::new();
    collect_package_tree_facts(&input.root, "root", &mut facts);
    facts
}

/// Convert RSScript dependency tree relationships into REIR dependency edges.
pub fn package_tree_to_edges(input: &RsScriptPackageTreeInput) -> Vec<Edge> {
    let mut edges = Vec::new();
    collect_package_tree_edges(&input.root, "root", &mut edges);
    edges
}

/// Convert RSScript package metadata write/verify output into REIR facts.
pub fn package_metadata_report_to_facts(input: &RsScriptPackageMetadataInput) -> Vec<Fact> {
    let subject = package_subject(&input.package.name, &input.package.version);
    let package_slug = normalized_id(&subject.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.metadata.{}.status", package_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if input.ok {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_METADATA_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![metadata_artifact_evidence(
            input,
            Some(input.package_dir.clone()),
            None,
            Some(package_metadata_report_summary(input)),
            Some("/ok"),
        )],
        unknown_reason: (!input.ok)
            .then(|| "package metadata write/verify result is not ok".to_owned()),
    }];

    facts.push(metadata_artifact_fact(
        &package_slug,
        "package_review_artifact",
        &subject,
        input,
        &input.metadata_path,
        "package review metadata artifact",
        "/metadata_path",
    ));
    facts.push(metadata_artifact_fact(
        &package_slug,
        "reir_artifact",
        &subject,
        input,
        &input.reir_path,
        "package REIR bundle artifact",
        "/reir_path",
    ));

    for (index, mismatch) in input.mismatches.iter().enumerate() {
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!(
                "fact.metadata.{}.mismatch.{}.{}",
                package_slug,
                index,
                normalized_id(&mismatch.path)
            ),
            kind: FactKind::PolicyResult,
            role: None,
            subject: subject.clone(),
            capability: None,
            value: FactValue::Unknown,
            confidence: confidence(ConfidenceLevel::Computed, PACKAGE_METADATA_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Exact,
            evidence: vec![metadata_artifact_evidence(
                input,
                Some(mismatch.path.clone()),
                Some(metadata_mismatch_value(mismatch)),
                Some(metadata_mismatch_reason(mismatch)),
                Some(&format!("/mismatches/{index}")),
            )],
            unknown_reason: Some(format!(
                "metadata artifact `{}` is {}{}",
                mismatch.path,
                mismatch.kind,
                metadata_mismatch_hash_suffix(mismatch)
            )),
        });
    }

    facts
}

fn metadata_mismatch_value(mismatch: &RsScriptPackageMetadataMismatch) -> String {
    if mismatch.expected_sha256.is_empty() {
        mismatch.path.clone()
    } else if let Some(actual_sha256) = &mismatch.actual_sha256 {
        format!(
            "{} expected={} actual={}",
            mismatch.path, mismatch.expected_sha256, actual_sha256
        )
    } else {
        format!("{} expected={}", mismatch.path, mismatch.expected_sha256)
    }
}

fn metadata_mismatch_reason(mismatch: &RsScriptPackageMetadataMismatch) -> String {
    let artifact = if mismatch.artifact.is_empty() {
        "artifact"
    } else {
        mismatch.artifact.as_str()
    };
    format!(
        "metadata {artifact} {}: {}{}",
        mismatch.kind,
        mismatch.message,
        metadata_mismatch_hash_suffix(mismatch)
    )
}

fn metadata_mismatch_hash_suffix(mismatch: &RsScriptPackageMetadataMismatch) -> String {
    if mismatch.expected_sha256.is_empty() {
        String::new()
    } else if let Some(actual_sha256) = &mismatch.actual_sha256 {
        format!(
            " (expected {}, actual {})",
            mismatch.expected_sha256, actual_sha256
        )
    } else {
        format!(" (expected {})", mismatch.expected_sha256)
    }
}

/// Convert RSScript package review relationships into REIR edges.
pub fn native_boundaries_to_edges(input: &RsScriptPackageReviewInput) -> Vec<Edge> {
    let mut edges = Vec::new();
    let package_subject = package_subject(&input.package_name, &input.version);

    for dependency in &input.dependencies {
        let dependency_subject = dependency_subject(dependency);
        let dependency_slug = normalized_id(&dependency_subject.id);
        edges.push(Edge {
            schema: EDGE_SCHEMA.to_owned(),
            id: format!(
                "edge.package.{}.depends_on.{}",
                normalized_id(&package_subject.id),
                dependency_slug
            ),
            kind: EdgeKind::DependsOn,
            from: package_subject.clone(),
            to: dependency_subject,
            confidence: confidence(dependency_confidence(dependency), PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::PackageMetadata,
            precision: Precision::Category,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                dependency.requirement.clone(),
                Some(dependency_summary(dependency)),
            )],
        });
    }

    for boundary in &input.native_boundaries {
        let to = native_boundary_subject(&input.package_name, &boundary.module_name);

        if boundary.functions.is_empty() {
            edges.push(native_edge(
                format!(
                    "edge.crosses_native.{}.{}",
                    normalized_id(&package_subject.id),
                    normalized_id(&to.id)
                ),
                package_subject.clone(),
                to.clone(),
                &boundary.file,
                boundary.line,
                &boundary.module_name,
            ));
            continue;
        }

        for function_name in &boundary.functions {
            let from = function_subject(&input.package_name, function_name);
            edges.push(Edge {
                schema: EDGE_SCHEMA.to_owned(),
                id: format!(
                    "edge.normalizes_to_native_fn.{}.{}",
                    normalized_id(&to.id),
                    normalized_id(&from.id)
                ),
                kind: EdgeKind::NormalizesToNativeFn,
                from: to.clone(),
                to: from.clone(),
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::CompilerContract,
                precision: Precision::Exact,
                evidence: vec![source_span(
                    &boundary.file,
                    boundary.line,
                    function_name,
                    Some(format!(
                        "native module {} normalizes to native function {}",
                        boundary.module_name, function_name
                    )),
                    PACKAGE_REVIEW_SOURCE,
                )],
            });
            edges.push(native_edge(
                format!(
                    "edge.crosses_native.{}.{}",
                    normalized_id(&from.id),
                    normalized_id(&to.id)
                ),
                from,
                to.clone(),
                &boundary.file,
                boundary.line,
                function_name,
            ));
        }
    }

    for export in input
        .exports
        .iter()
        .filter(|export| export.kind == "protocol_impl")
    {
        let Some((protocol, _type_name)) = protocol_impl_parts(export) else {
            continue;
        };
        let from = public_contract_subject(&input.package_name, export);
        let to = protocol_subject(&input.package_name, &protocol);
        edges.push(Edge {
            schema: EDGE_SCHEMA.to_owned(),
            id: format!(
                "edge.protocol_impl.{}.implements.{}",
                normalized_id(&from.id),
                normalized_id(&to.id)
            ),
            kind: EdgeKind::ImplementsProtocol,
            from,
            to,
            confidence: confidence(public_contract_confidence(export), PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                Some(export.kind.clone()),
                Some(package_export_summary(export)),
            )],
        });
    }

    edges
}

fn classify_review_required(reasons: &[String]) -> FactKind {
    let lower_reasons = reasons
        .iter()
        .map(|reason| reason.to_ascii_lowercase())
        .collect::<Vec<_>>();

    if lower_reasons
        .iter()
        .any(|reason| matches!(reason.as_str(), "native boundary" | "native bridge"))
    {
        FactKind::NativeBoundary
    } else if lower_reasons.iter().any(|reason| reason.contains("retain")) {
        FactKind::Retention
    } else if lower_reasons.iter().any(|reason| reason.contains("mut")) {
        FactKind::Mutation
    } else {
        FactKind::Extension(REVIEW_REQUIRED_KIND.to_owned())
    }
}

fn function_subject(package_name: &str, function_name: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeFunction,
        id: function_subject_id(package_name, function_name),
        name: Some(function_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn module_use_facts(input: &RsScriptReviewMapInput) -> Vec<Fact> {
    let mut facts = Vec::new();
    for module in &input.modules {
        let module_subject = module_subject(&input.package_name, &module.module_path);
        let module_slug = normalized_id(&module_subject.id);
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.module.{module_slug}.declaration"),
            kind: FactKind::ModuleDeclaration,
            role: None,
            subject: module_subject.clone(),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Authoritative, REVIEW_MAP_SOURCE),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![source_span(
                &module.file,
                module.line,
                &module.module_path,
                Some("RSScript module declaration".to_owned()),
                REVIEW_MAP_SOURCE,
            )],
            unknown_reason: None,
        });

        for use_decl in &module.uses {
            facts.push(Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.module.{}.uses.{}",
                    module_slug,
                    normalized_id(&use_decl.path)
                ),
                kind: FactKind::UseDeclaration,
                role: None,
                subject: module_subject.clone(),
                capability: None,
                value: FactValue::True,
                confidence: confidence(ConfidenceLevel::Authoritative, REVIEW_MAP_SOURCE),
                acquisition_mode: AcquisitionMode::CompilerContract,
                precision: Precision::Exact,
                evidence: vec![source_span(
                    &module.file,
                    use_decl.line,
                    &use_decl.path,
                    Some("RSScript use declaration; no implicit method resolution".to_owned()),
                    REVIEW_MAP_SOURCE,
                )],
                unknown_reason: None,
            });
        }
    }
    facts
}

fn module_subject(package_name: &str, module_path: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeModule,
        id: format!("{package_name}::module::{module_path}"),
        name: Some(module_path.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn function_subject_id(package_name: &str, function_name: &str) -> String {
    format!("{package_name}::{function_name}")
}

fn package_subject(package_name: &str, version: &str) -> Subject {
    Subject {
        kind: SubjectKind::Package,
        id: format!("{package_name}@{version}"),
        name: Some(package_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn dependency_subject(dependency: &RsScriptPackageDependency) -> Subject {
    let identity = dependency
        .requirement
        .as_ref()
        .map(|requirement| format!("{}@{}", dependency.name, requirement))
        .unwrap_or_else(|| format!("{}@{}", dependency.name, dependency.source));
    Subject {
        kind: SubjectKind::Package,
        id: identity,
        name: Some(dependency.name.clone()),
        package: Some(dependency.name.clone()),
    }
}

fn native_boundary_subject(package_name: &str, module_name: &str) -> Subject {
    Subject {
        kind: SubjectKind::NativeBoundary,
        id: format!("{package_name}::native::{module_name}"),
        name: Some(module_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn public_contract_subject(package_name: &str, export: &RsScriptPackageExport) -> Subject {
    let kind = match export.kind.as_str() {
        "type" | "sum_type" | "type_alias" => SubjectKind::CodeType,
        "protocol" => SubjectKind::CodeProtocol,
        "protocol_impl" => SubjectKind::CodeProtocolImpl,
        "function" | "const" => SubjectKind::CodePublicApi,
        _ => SubjectKind::CodeInterfaceSymbol,
    };
    Subject {
        kind,
        id: format!("{}::public::{}::{}", package_name, export.kind, export.name),
        name: Some(export.name.clone()),
        package: Some(package_name.to_owned()),
    }
}

fn package_function_subject(package_name: &str, function: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeFunction,
        id: format!("{package_name}::function::{function}"),
        name: Some(function.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn protocol_declaration_fact(
    export: &RsScriptPackageExport,
    export_subject: &Subject,
    package_name: &str,
    version: &str,
    package_slug: &str,
) -> Option<Fact> {
    (export.kind == "protocol").then(|| Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.protocol_declaration.{}.{}",
            package_slug,
            normalized_id(&export_subject.id)
        ),
        kind: FactKind::ProtocolDeclaration,
        role: None,
        subject: export_subject.clone(),
        capability: None,
        value: public_contract_value(export),
        confidence: confidence(public_contract_confidence(export), PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(export.kind.clone()),
            Some(package_export_summary(export)),
        )],
        unknown_reason: matches!(public_contract_value(export), FactValue::Unknown)
            .then(|| public_contract_unknown_reason(export)),
    })
}

fn protocol_impl_fact(
    export: &RsScriptPackageExport,
    export_subject: &Subject,
    package_name: &str,
    version: &str,
    package_slug: &str,
) -> Option<Fact> {
    (export.kind == "protocol_impl").then(|| Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.protocol_impl.{}.{}",
            package_slug,
            normalized_id(&export_subject.id)
        ),
        kind: FactKind::ProtocolImpl,
        role: None,
        subject: export_subject.clone(),
        capability: None,
        value: public_contract_value(export),
        confidence: confidence(public_contract_confidence(export), PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(export.kind.clone()),
            Some(package_export_summary(export)),
        )],
        unknown_reason: matches!(public_contract_value(export), FactValue::Unknown)
            .then(|| public_contract_unknown_reason(export)),
    })
}

fn protocol_method_contract_facts(
    input: &RsScriptPackageReviewInput,
    package_slug: &str,
    protocol_names: &[String],
) -> Vec<Fact> {
    input
        .exports
        .iter()
        .filter(|export| export.kind == "function")
        .filter_map(|export| {
            let (namespace, method) = export.name.split_once('.')?;
            protocol_names
                .iter()
                .any(|protocol| protocol == namespace)
                .then(|| {
                    let subject = Subject {
                        kind: SubjectKind::CodeProtocolMethod,
                        id: format!(
                            "{}::protocol::{}::method::{}",
                            input.package_name, namespace, method
                        ),
                        name: Some(export.name.clone()),
                        package: Some(input.package_name.clone()),
                    };
                    Fact {
                        schema: FACT_SCHEMA.to_owned(),
                        id: format!(
                            "fact.protocol_method_contract.{}.{}",
                            package_slug,
                            normalized_id(&subject.id)
                        ),
                        kind: FactKind::ProtocolMethodContract,
                        role: None,
                        subject,
                        capability: None,
                        value: public_contract_value(export),
                        confidence: confidence(
                            public_contract_confidence(export),
                            PACKAGE_REVIEW_SOURCE,
                        ),
                        acquisition_mode: AcquisitionMode::CompilerContract,
                        precision: Precision::Exact,
                        evidence: vec![package_metadata(
                            &input.package_name,
                            &input.version,
                            Some(export.kind.clone()),
                            Some(package_export_summary(export)),
                        )],
                        unknown_reason: matches!(public_contract_value(export), FactValue::Unknown)
                            .then(|| public_contract_unknown_reason(export)),
                    }
                })
        })
        .collect()
}

fn protocol_subject(package_name: &str, protocol: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeProtocol,
        id: format!("{package_name}::public::protocol::{protocol}"),
        name: Some(protocol.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn protocol_impl_parts(export: &RsScriptPackageExport) -> Option<(String, String)> {
    let protocol = export
        .reasons
        .iter()
        .find_map(|reason| backtick_value_after_prefix(reason, "protocol "))
        .or_else(|| {
            export
                .name
                .split_once(" for ")
                .map(|(protocol, _)| protocol.to_owned())
        })?;
    let type_name = export
        .reasons
        .iter()
        .find_map(|reason| backtick_value_after_prefix(reason, "type "))
        .or_else(|| {
            export
                .name
                .split_once(" for ")
                .map(|(_, type_name)| type_name.to_owned())
        })?;
    Some((protocol, type_name))
}

fn backtick_value_after_prefix(reason: &str, prefix: &str) -> Option<String> {
    let rest = reason.strip_prefix(prefix)?;
    let value = rest.strip_prefix('`')?.split_once('`')?.0;
    (!value.is_empty()).then(|| value.to_owned())
}

fn public_contract_value(export: &RsScriptPackageExport) -> FactValue {
    if export.classification == "unknown" || export.kind == "contract_diagnostic" {
        FactValue::Unknown
    } else {
        FactValue::True
    }
}

fn public_contract_confidence(export: &RsScriptPackageExport) -> ConfidenceLevel {
    if matches!(public_contract_value(export), FactValue::Unknown) {
        ConfidenceLevel::Unknown
    } else {
        ConfidenceLevel::Authoritative
    }
}

fn public_contract_unknown_reason(export: &RsScriptPackageExport) -> String {
    let reason = if export.reasons.is_empty() {
        "package public contract could not be classified".to_owned()
    } else {
        export.reasons.join("; ")
    };
    format!(
        "public contract export `{}` is unknown: {}",
        export.name, reason
    )
}

fn package_risk_value(risk: &RsScriptPackageRisk) -> FactValue {
    match risk {
        RsScriptPackageRisk::Unknown => FactValue::Unknown,
        _ => FactValue::True,
    }
}

fn package_risk_confidence(risk: &RsScriptPackageRisk) -> ConfidenceLevel {
    match risk {
        RsScriptPackageRisk::Unknown => ConfidenceLevel::Unknown,
        _ => ConfidenceLevel::Authoritative,
    }
}

fn dependency_risk_value(dependency: &RsScriptPackageDependency) -> FactValue {
    if dependency.source.starts_with("path+") || dependency.platform_provided {
        FactValue::True
    } else {
        FactValue::Unknown
    }
}

fn dependency_confidence(dependency: &RsScriptPackageDependency) -> ConfidenceLevel {
    if dependency.source.starts_with("path+") || dependency.platform_provided {
        ConfidenceLevel::Scanned
    } else {
        ConfidenceLevel::Unknown
    }
}

fn dependency_summary(dependency: &RsScriptPackageDependency) -> String {
    let requirement = dependency.requirement.as_deref().unwrap_or("unspecified");
    let features = if dependency.features.is_empty() {
        "none".to_owned()
    } else {
        dependency.features.join(",")
    };
    format!(
        "dependency {} requirement={} source={} kind={} features={} compile_only={} test_only={} platform_provided={}",
        dependency.name,
        requirement,
        dependency.source,
        dependency.dependency_kind,
        features,
        dependency.compile_only,
        dependency.test_only,
        dependency.platform_provided
    )
}

fn package_check_policy_fact(
    package_slug: &str,
    field: &str,
    subject: &Subject,
    input: &RsScriptPackageCheckInput,
    ok: bool,
    risk: &RsScriptPackageRisk,
    reason: String,
    json_pointer: &str,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_check.{}.{}", package_slug, field),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if ok {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![package_check_evidence(
            input,
            Some(risk.as_str().to_owned()),
            Some(reason),
            Some(json_pointer),
        )],
        unknown_reason: (!ok).then(|| format!("package check `{field}` failed")),
    }
}

fn package_check_lock_policy_fact(
    package_slug: &str,
    subject: &Subject,
    input: &RsScriptPackageCheckInput,
    ok: bool,
    risk: &RsScriptPackageRisk,
    reason: String,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_check.{}.lock", package_slug),
        kind: FactKind::PolicyResult,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if ok {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
        acquisition_mode: AcquisitionMode::Lockfile,
        precision: Precision::Category,
        evidence: vec![package_check_lock_evidence(
            input,
            Some(risk.as_str().to_owned()),
            Some(reason),
            "/lock",
        )],
        unknown_reason: (!ok).then(|| "package check `lock` failed".to_owned()),
    }
}

fn package_check_boundary_fact(
    package_slug: &str,
    field: &str,
    subject: &Subject,
    input: &RsScriptPackageCheckInput,
    kind: FactKind,
    reason: &str,
    json_pointer: &str,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_check.{}.{}", package_slug, field),
        kind,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_CHECK_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![package_check_evidence(
            input,
            None,
            Some(reason.to_owned()),
            Some(json_pointer),
        )],
        unknown_reason: None,
    }
}

fn metadata_artifact_fact(
    package_slug: &str,
    field: &str,
    subject: &Subject,
    input: &RsScriptPackageMetadataInput,
    path: &str,
    reason: &str,
    json_pointer: &str,
) -> Fact {
    let path_has_mismatch = input
        .mismatches
        .iter()
        .any(|mismatch| mismatch.path == path);
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.metadata.{}.{}", package_slug, field),
        kind: FactKind::SupplyChain,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: if path_has_mismatch {
            FactValue::Unknown
        } else {
            FactValue::True
        },
        confidence: confidence(ConfidenceLevel::Computed, PACKAGE_METADATA_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Exact,
        evidence: vec![metadata_artifact_evidence(
            input,
            Some(path.to_owned()),
            Some(path.to_owned()),
            Some(format!("{reason} {path}")),
            Some(json_pointer),
        )],
        unknown_reason: path_has_mismatch
            .then(|| format!("metadata artifact `{path}` is missing, stale, or unreadable")),
    }
}

fn metadata_artifact_evidence(
    input: &RsScriptPackageMetadataInput,
    file: Option<String>,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: Option<&str>,
) -> Evidence {
    Evidence {
        file,
        reason,
        json_pointer: json_pointer.map(str::to_owned),
        resource: Some(format!("{}@{}", input.package.name, input.package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        source: Some(PACKAGE_METADATA_SOURCE.to_owned()),
        ..rsscript_evidence(EvidenceKind::PackageMetadata)
    }
}

fn package_check_evidence(
    input: &RsScriptPackageCheckInput,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: Option<&str>,
) -> Evidence {
    Evidence {
        file: package_check_evidence_file(input, json_pointer),
        reason,
        json_pointer: json_pointer.map(str::to_owned),
        resource: Some(format!("{}@{}", input.package.name, input.package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        source: Some(PACKAGE_CHECK_SOURCE.to_owned()),
        ..rsscript_evidence(EvidenceKind::PackageMetadata)
    }
}

fn package_check_evidence_file(
    input: &RsScriptPackageCheckInput,
    json_pointer: Option<&str>,
) -> Option<String> {
    if json_pointer.is_some_and(|pointer| pointer.starts_with("/native_rust")) {
        if let Some(native) = &input.native_rust {
            if !native.path.is_empty() {
                return Some(package_check_native_evidence_file(input, &native.path));
            }
        }
    }
    if json_pointer.is_some_and(|pointer| pointer.starts_with("/implements")) {
        return Some(
            Path::new(&input.package_dir)
                .join("rsspkg.toml")
                .display()
                .to_string(),
        );
    }
    Some(input.package_dir.clone())
}

fn package_check_native_evidence_file(
    input: &RsScriptPackageCheckInput,
    native_path: &str,
) -> String {
    let path = Path::new(native_path);
    if path.is_absolute() {
        native_path.to_owned()
    } else {
        Path::new(&input.package_dir)
            .join(path)
            .display()
            .to_string()
    }
}

fn package_check_lock_evidence(
    input: &RsScriptPackageCheckInput,
    value: Option<String>,
    reason: Option<String>,
    json_pointer: &str,
) -> Evidence {
    Evidence {
        file: Some(input.lock.path.clone()),
        reason,
        json_pointer: Some(json_pointer.to_owned()),
        resource: Some(format!("{}@{}", input.package.name, input.package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        source: Some(PACKAGE_CHECK_SOURCE.to_owned()),
        ..rsscript_evidence(EvidenceKind::LockfileEntry)
    }
}

fn package_check_diagnostic_evidence(
    input: &RsScriptPackageCheckInput,
    diagnostic: &RsScriptDiagnosticInput,
    span: Option<&RsScriptDiagnosticSpan>,
) -> Evidence {
    let mut evidence = diagnostic_evidence(diagnostic, span);
    if let Some(file) = evidence.file.as_deref() {
        let path = Path::new(file);
        if !path.is_absolute() {
            evidence.file = Some(
                Path::new(&input.package_dir)
                    .join(path)
                    .display()
                    .to_string(),
            );
        }
    }
    evidence.source = Some(PACKAGE_CHECK_SOURCE.to_owned());
    evidence
}

fn capability_fact(
    id: String,
    subject: Subject,
    category: CapabilityCategory,
    count: usize,
    package_name: &str,
    version: &str,
    summary: String,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject: subject.clone(),
        capability: Some(Capability {
            category,
            provider: Some("rsscript".to_owned()),
            service: None,
            action: None,
            resource: Some(subject.id.clone()),
            constraints: HashMap::new(),
        }),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Presence,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(count.to_string()),
            Some(summary),
        )],
        unknown_reason: None,
    }
}

fn build_dependency_risk_fact(
    id: String,
    subject: Subject,
    dependency: &RsScriptPackageDependency,
    package_name: &str,
    version: &str,
) -> Fact {
    let value = dependency_risk_value(dependency);
    let unknown = matches!(value, FactValue::Unknown);
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::DependencyRisk,
        role: None,
        subject,
        capability: None,
        value,
        confidence: confidence(dependency_confidence(dependency), PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::PackageMetadata,
        precision: Precision::Category,
        evidence: vec![package_metadata(
            package_name,
            version,
            dependency.requirement.clone(),
            Some(dependency_summary(dependency)),
        )],
        unknown_reason: unknown
            .then(|| "dependency source is unresolved by package review".to_owned()),
    }
}

fn build_public_contract_fact(
    id: String,
    subject: Subject,
    export: &RsScriptPackageExport,
    package_name: &str,
    version: &str,
) -> Fact {
    let value = public_contract_value(export);
    let unknown = matches!(value, FactValue::Unknown);
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::PublicContract,
        role: None,
        subject,
        capability: None,
        value,
        confidence: confidence(public_contract_confidence(export), PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(export.kind.clone()),
            Some(package_export_summary(export)),
        )],
        unknown_reason: unknown.then(|| public_contract_unknown_reason(export)),
    }
}

fn build_native_boundary_fact(
    id: String,
    kind: FactKind,
    subject: Subject,
    boundary: &RsScriptNativeBoundary,
    reason: String,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind,
        role: None,
        subject,
        capability: None,
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![source_span(
            &boundary.file,
            boundary.line,
            &boundary.module_name,
            Some(reason),
            PACKAGE_REVIEW_SOURCE,
        )],
        unknown_reason: None,
    }
}

fn native_edge(
    id: String,
    from: Subject,
    to: Subject,
    file: &str,
    line: usize,
    symbol: &str,
) -> Edge {
    Edge {
        schema: EDGE_SCHEMA.to_owned(),
        id,
        kind: EdgeKind::CrossesNative,
        from,
        to,
        confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![source_span(file, line, symbol, None, PACKAGE_REVIEW_SOURCE)],
    }
}

fn export_capability_fact(
    id: String,
    subject: Subject,
    export: &RsScriptPackageExport,
    category: CapabilityCategory,
    package_name: &str,
    version: &str,
) -> Fact {
    let mut constraints = HashMap::new();
    constraints.insert("symbol".to_owned(), export.name.clone());
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject: subject.clone(),
        capability: Some(Capability {
            category: category.clone(),
            provider: Some("rsscript".to_owned()),
            service: Some("stdlib".to_owned()),
            action: Some(export.name.clone()),
            resource: Some(subject.id.clone()),
            constraints,
        }),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Category,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some(String::from(category)),
            Some(package_export_summary(export)),
        )],
        unknown_reason: None,
    }
}

fn package_capability_facts(input: &RsScriptPackageReviewInput, package_slug: &str) -> Vec<Fact> {
    input
        .capabilities
        .iter()
        .map(|capability| {
            let subject = package_function_subject(&input.package_name, &capability.function);
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package_capability.{}.{}.{}",
                    package_slug,
                    normalized_id(&subject.id),
                    normalized_id(&capability.binding_symbol)
                ),
                kind: FactKind::Capability,
                role: Some(FactRole::Required),
                subject,
                capability: Some(Capability {
                    category: capability.category.clone(),
                    provider: capability
                        .provider
                        .clone()
                        .or_else(|| Some("rsscript".to_owned())),
                    service: capability.service.clone(),
                    action: capability.action.clone(),
                    resource: capability.resource.clone(),
                    // Binding symbol and call chain are provenance, not
                    // authorization constraints. They remain in the evidence
                    // reason below and must not prevent a deployment grant
                    // from covering the capability.
                    constraints: HashMap::new(),
                }),
                value: if capability.unknown_reason.is_some() {
                    FactValue::Unknown
                } else {
                    FactValue::True
                },
                confidence: confidence(
                    if capability.unknown_reason.is_some() {
                        ConfidenceLevel::Unknown
                    } else {
                        ConfidenceLevel::Inferred
                    },
                    PACKAGE_REVIEW_SOURCE,
                ),
                acquisition_mode: AcquisitionMode::BindingManifest,
                precision: if capability.resource.is_some() {
                    Precision::ResourceScoped
                } else {
                    Precision::Category
                },
                evidence: vec![binding_manifest_evidence(input, capability)],
                unknown_reason: capability.unknown_reason.clone(),
            }
        })
        .collect()
}

fn binding_manifest_evidence(
    input: &RsScriptPackageReviewInput,
    capability: &RsScriptPackageCapability,
) -> Evidence {
    Evidence {
        file: capability
            .span
            .as_ref()
            .and_then(|span| (!span.file.is_empty()).then(|| span.file.clone())),
        line: capability.span.as_ref().map(|span| span.line.max(1)),
        column: capability.span.as_ref().map(|span| span.column.max(1)),
        length: capability.span.as_ref().map(|span| span.length),
        symbol: Some(capability.function.clone()),
        reason: Some(format!(
            "{} {} propagated through {}",
            if capability.unknown_reason.is_some() {
                "unknown capability binding"
            } else {
                "capability binding"
            },
            capability.binding_symbol,
            if capability.call_chain.is_empty() {
                capability.function.clone()
            } else {
                capability.call_chain.join(" -> ")
            }
        )),
        resource: capability
            .resource
            .clone()
            .or_else(|| Some(format!("{}@{}", input.package_name, input.version))),
        provider: capability.provider.clone(),
        value: Some(String::from(capability.category.clone())),
        source: Some(PACKAGE_REVIEW_SOURCE.to_owned()),
        action: capability.action.clone(),
        ..rsscript_evidence(EvidenceKind::BindingManifest)
    }
}

fn native_source_scan_facts(
    input: &RsScriptPackageReviewInput,
    scan: &RsScriptNativeSourceScan,
    package_subject: &Subject,
    package_slug: &str,
) -> Vec<Fact> {
    let mut facts = Vec::new();
    let scan_summary = native_source_scan_summary(scan);
    if scan.unsafe_detected {
        let subject = Subject {
            kind: SubjectKind::UnsafeBoundary,
            id: format!("{}::unsafe::native_rust", input.package_name),
            name: Some("native_rust_unsafe".to_owned()),
            package: Some(input.package_name.clone()),
        };
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.unsafe_boundary.{}", normalized_id(&subject.id)),
            kind: FactKind::UnsafeBoundary,
            role: None,
            subject: subject.clone(),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Scanned, PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::SourceScan,
            precision: Precision::Presence,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                Some("unsafe_detected".to_owned()),
                Some(scan_summary.clone()),
            )],
            unknown_reason: None,
        });
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.runtime_unsafe", package_slug),
            package_subject.clone(),
            CapabilityCategory::RuntimeUnsafe,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.ffi_detected {
        let subject = Subject {
            kind: SubjectKind::NativeBoundary,
            id: format!("{}::native::ffi", input.package_name),
            name: Some("ffi".to_owned()),
            package: Some(input.package_name.clone()),
        };
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.native_boundary.{}", normalized_id(&subject.id)),
            kind: FactKind::NativeBoundary,
            role: None,
            subject: subject.clone(),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Scanned, PACKAGE_REVIEW_SOURCE),
            acquisition_mode: AcquisitionMode::SourceScan,
            precision: Precision::Presence,
            evidence: vec![package_metadata(
                &input.package_name,
                &input.version,
                Some("ffi_detected".to_owned()),
                Some(scan_summary.clone()),
            )],
            unknown_reason: None,
        });
        facts.push(native_scan_capability_fact(
            format!(
                "fact.package.{}.native_scan.runtime_native_ffi",
                package_slug
            ),
            package_subject.clone(),
            CapabilityCategory::RuntimeNative,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.filesystem_detected {
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.filesystem", package_slug),
            package_subject.clone(),
            CapabilityCategory::FilesystemRead,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.network_detected {
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.network", package_slug),
            package_subject.clone(),
            CapabilityCategory::NetworkClient,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.worker_thread_parallelism_detected {
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.process_spawn", package_slug),
            package_subject.clone(),
            CapabilityCategory::ProcessSpawn,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    if scan.build_script_present {
        facts.push(native_build_time_execution_fact(
            input,
            package_slug,
            scan_summary.clone(),
        ));
        facts.push(native_scan_capability_fact(
            format!("fact.package.{}.native_scan.build_execute", package_slug),
            package_subject.clone(),
            CapabilityCategory::BuildExecute,
            &input.package_name,
            &input.version,
            scan_summary.clone(),
        ));
    }
    facts
}

fn native_build_time_execution_fact(
    input: &RsScriptPackageReviewInput,
    package_slug: &str,
    summary: String,
) -> Fact {
    let subject = Subject {
        kind: SubjectKind::BuildStep,
        id: format!(
            "{}@{}::build::native_rust_build_script",
            input.package_name, input.version
        ),
        name: Some("native_rust_build_script".to_owned()),
        package: Some(input.package_name.clone()),
    };
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.package.{}.build_time_execution.build_script",
            package_slug
        ),
        kind: FactKind::BuildTimeExecution,
        role: None,
        subject,
        capability: None,
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Scanned, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::SourceScan,
        precision: Precision::Presence,
        evidence: vec![package_metadata(
            &input.package_name,
            &input.version,
            Some("build_script_present".to_owned()),
            Some(summary),
        )],
        unknown_reason: None,
    }
}

fn await_site_facts(input: &RsScriptPackageReviewInput, package_slug: &str) -> Vec<Fact> {
    input
        .await_sites
        .iter()
        .enumerate()
        .map(|(index, site)| {
            let subject = function_subject(&input.package_name, &site.function);
            let boundary = if site.boundary.is_empty() {
                "unknown"
            } else {
                site.boundary.as_str()
            };
            let summary = await_site_summary(site, boundary);
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.await_site.{}.{}.{}",
                    package_slug,
                    normalized_id(&subject.id),
                    site.line,
                    index
                ),
                kind: FactKind::AsyncBoundary,
                role: None,
                subject,
                capability: None,
                value: FactValue::True,
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::CompilerContract,
                precision: Precision::Exact,
                evidence: vec![await_site_evidence(site, summary)],
                unknown_reason: (boundary == "unknown")
                    .then(|| "await boundary target could not be classified".to_owned()),
            }
        })
        .collect()
}

fn await_site_summary(site: &RsScriptPackageAwaitSite, boundary: &str) -> String {
    let callee = site.callee.as_deref().unwrap_or("unknown");
    let live = if site.live_across_await.is_empty() {
        "none".to_owned()
    } else {
        site.live_across_await.join(",")
    };
    format!(
        "await boundary={} callee={} live_across_await={}",
        boundary, callee, live
    )
}

fn await_site_evidence(site: &RsScriptPackageAwaitSite, summary: String) -> Evidence {
    Evidence {
        file: (!site.file.is_empty()).then(|| site.file.clone()),
        line: Some(site.line.max(1)),
        column: Some(site.column.max(1)),
        symbol: Some(site.function.clone()),
        reason: Some(summary),
        source: Some(PACKAGE_REVIEW_SOURCE.to_owned()),
        ..rsscript_evidence(EvidenceKind::SourceSpan)
    }
}

fn diagnostic_facts(
    input: &RsScriptPackageReviewInput,
    package_subject: &Subject,
    package_slug: &str,
) -> Vec<Fact> {
    input
        .diagnostics
        .iter()
        .enumerate()
        .map(|(index, diagnostic)| {
            let span = diagnostic.spans.first();
            let subject = span
                .filter(|span| !span.file.is_empty())
                .map(|span| Subject {
                    kind: SubjectKind::CodeFile,
                    id: format!("{}::{}", input.package_name, span.file),
                    name: Some(span.file.clone()),
                    package: Some(input.package_name.clone()),
                })
                .unwrap_or_else(|| package_subject.clone());
            let code = if diagnostic.code.is_empty() {
                "unknown"
            } else {
                diagnostic.code.as_str()
            };
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.diagnostic.{}.{}",
                    package_slug,
                    normalized_id(code),
                    index
                ),
                kind: FactKind::Diagnostic,
                role: None,
                subject,
                capability: None,
                value: diagnostic_fact_value(diagnostic),
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::CompilerContract,
                precision: Precision::Exact,
                evidence: vec![diagnostic_evidence(diagnostic, span)],
                unknown_reason: (diagnostic.severity == "error")
                    .then(|| diagnostic.summary.clone()),
            }
        })
        .collect()
}

fn diagnostic_fact_value(diagnostic: &RsScriptDiagnosticInput) -> FactValue {
    if diagnostic.severity.eq_ignore_ascii_case("error") {
        FactValue::Unknown
    } else {
        FactValue::True
    }
}

fn diagnostic_evidence(
    diagnostic: &RsScriptDiagnosticInput,
    span: Option<&RsScriptDiagnosticSpan>,
) -> Evidence {
    let reason = if diagnostic.summary.is_empty() {
        format!(
            "diagnostic {} severity={}",
            diagnostic.code, diagnostic.severity
        )
    } else {
        format!(
            "diagnostic {} severity={} summary={}",
            diagnostic.code, diagnostic.severity, diagnostic.summary
        )
    };
    Evidence {
        file: span.and_then(|span| (!span.file.is_empty()).then(|| span.file.clone())),
        line: span.map(|span| span.line.max(1)),
        column: span.map(|span| span.column.max(1)),
        length: span.map(|span| span.length),
        symbol: Some(diagnostic.code.clone()),
        reason: Some(reason),
        value: Some(diagnostic.severity.clone()),
        source: Some(PACKAGE_REVIEW_SOURCE.to_owned()),
        ..rsscript_evidence(EvidenceKind::SourceSpan)
    }
}

fn package_feature_facts(input: &RsScriptPackageReviewInput, package_slug: &str) -> Vec<Fact> {
    input
        .features
        .iter()
        .map(|feature| {
            let subject = Subject {
                kind: SubjectKind::PackageFeature,
                id: format!(
                    "{}@{}#feature:{}",
                    input.package_name, input.version, feature
                ),
                name: Some(feature.clone()),
                package: Some(input.package_name.clone()),
            };
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.feature.{}",
                    package_slug,
                    normalized_id(feature)
                ),
                kind: FactKind::PackageFeature,
                role: None,
                subject,
                capability: None,
                value: FactValue::True,
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::PackageMetadata,
                precision: Precision::Exact,
                evidence: vec![package_metadata(
                    &input.package_name,
                    &input.version,
                    Some(feature.clone()),
                    Some(format!("package feature {} selected", feature)),
                )],
                unknown_reason: None,
            }
        })
        .collect()
}

fn provider_implementation_facts(
    input: &RsScriptPackageReviewInput,
    package_slug: &str,
) -> Vec<Fact> {
    input
        .implements
        .iter()
        .map(|implementation| {
            let subject = Subject {
                kind: SubjectKind::Package,
                id: format!(
                    "{}@{}::implements::{}",
                    input.package_name, input.version, implementation.interface_package
                ),
                name: Some(implementation.interface_package.clone()),
                package: Some(input.package_name.clone()),
            };
            let has_hash = implementation.interface_effective_hash.is_some();
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.provider_implementation.{}",
                    package_slug,
                    normalized_id(&implementation.interface_package)
                ),
                kind: FactKind::ProviderImplementation,
                role: None,
                subject,
                capability: None,
                value: if has_hash {
                    FactValue::True
                } else {
                    FactValue::Unknown
                },
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::PackageMetadata,
                precision: Precision::Exact,
                evidence: vec![package_metadata(
                    &input.package_name,
                    &input.version,
                    implementation.interface_effective_hash.clone(),
                    Some(provider_implementation_summary(implementation)),
                )],
                unknown_reason: (!has_hash).then(|| {
                    "provider implementation is missing interface_effective_hash".to_owned()
                }),
            }
        })
        .collect()
}

fn provider_implementation_summary(implementation: &RsScriptProviderImplementation) -> String {
    let version = implementation.version.as_deref().unwrap_or("unspecified");
    let features = if implementation.interface_features.is_empty() {
        "none".to_owned()
    } else {
        implementation.interface_features.join(",")
    };
    let hash = implementation
        .interface_effective_hash
        .as_deref()
        .unwrap_or("missing");
    format!(
        "implements {} version={} interface_features={} interface_effective_hash={}",
        implementation.interface_package, version, features, hash
    )
}

fn package_check_provider_implementation_facts(
    input: &RsScriptPackageCheckInput,
    package_slug: &str,
) -> Vec<Fact> {
    input
        .implements
        .iter()
        .enumerate()
        .map(|(index, implementation)| {
            let subject = Subject {
                kind: SubjectKind::Package,
                id: format!(
                    "{}@{}::implements::{}",
                    input.package.name, input.package.version, implementation.interface_package
                ),
                name: Some(implementation.interface_package.clone()),
                package: Some(input.package.name.clone()),
            };
            let has_hash = implementation.interface_effective_hash.is_some();
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package_check.{}.provider_implementation.{}",
                    package_slug,
                    normalized_id(&implementation.interface_package)
                ),
                kind: FactKind::ProviderImplementation,
                role: None,
                subject,
                capability: None,
                value: if has_hash {
                    FactValue::True
                } else {
                    FactValue::Unknown
                },
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_CHECK_SOURCE),
                acquisition_mode: AcquisitionMode::PackageMetadata,
                precision: Precision::Exact,
                evidence: vec![package_check_evidence(
                    input,
                    implementation.interface_effective_hash.clone(),
                    Some(provider_implementation_summary(implementation)),
                    Some(&format!("/implements/{index}")),
                )],
                unknown_reason: (!has_hash).then(|| {
                    "provider implementation is missing interface_effective_hash".to_owned()
                }),
            }
        })
        .collect()
}

fn native_author_declaration_facts(
    input: &RsScriptPackageReviewInput,
    package_subject: &Subject,
    package_slug: &str,
) -> Vec<Fact> {
    let Some(declaration) = &input.native_author_declaration else {
        return Vec::new();
    };
    let mut facts = Vec::new();
    if declaration.worker_thread_parallelism {
        let summary = native_author_declaration_summary(declaration);
        facts.push(native_author_capability_fact(
            format!("fact.package.{}.native_author.process_spawn", package_slug),
            package_subject.clone(),
            CapabilityCategory::ProcessSpawn,
            &input.package_name,
            &input.version,
            summary,
        ));
    }
    facts
}

fn native_cargo_feature_facts(input: &RsScriptPackageReviewInput, package_slug: &str) -> Vec<Fact> {
    input
        .native_cargo_features
        .iter()
        .map(|feature| {
            let subject = Subject {
                kind: SubjectKind::PackageFeature,
                id: format!(
                    "{}@{}#native-cargo-feature:{}",
                    input.package_name, input.version, feature
                ),
                name: Some(feature.clone()),
                package: Some(input.package_name.clone()),
            };
            Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.package.{}.native_cargo_feature.{}",
                    package_slug,
                    normalized_id(feature)
                ),
                kind: FactKind::NativeCargoFeature,
                role: None,
                subject,
                capability: None,
                value: FactValue::True,
                confidence: confidence(ConfidenceLevel::Authoritative, PACKAGE_REVIEW_SOURCE),
                acquisition_mode: AcquisitionMode::PackageMetadata,
                precision: Precision::Exact,
                evidence: vec![package_metadata(
                    &input.package_name,
                    &input.version,
                    Some(feature.clone()),
                    Some(format!("selected native Cargo feature {}", feature)),
                )],
                unknown_reason: None,
            }
        })
        .collect()
}

fn native_author_capability_fact(
    id: String,
    subject: Subject,
    category: CapabilityCategory,
    package_name: &str,
    version: &str,
    summary: String,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject: subject.clone(),
        capability: Some(Capability {
            category,
            provider: Some("rsscript".to_owned()),
            service: Some("native_rust_author_declaration".to_owned()),
            action: None,
            resource: Some(subject.id.clone()),
            constraints: HashMap::new(),
        }),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Declared, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::ManualDeclaration,
        precision: Precision::Presence,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some("author_declaration".to_owned()),
            Some(summary),
        )],
        unknown_reason: None,
    }
}

fn native_author_declaration_summary(declaration: &RsScriptNativeAuthorDeclaration) -> String {
    let backend = declaration
        .native_parallel_backend
        .as_deref()
        .unwrap_or("unspecified");
    let reasons = if declaration.risk_reasons.is_empty() {
        "none".to_owned()
    } else {
        declaration.risk_reasons.join(";")
    };
    format!(
        "author_declaration worker_thread_parallelism={} native_parallel_backend={} risk_reasons={}",
        declaration.worker_thread_parallelism, backend, reasons
    )
}

fn native_scan_capability_fact(
    id: String,
    subject: Subject,
    category: CapabilityCategory,
    package_name: &str,
    version: &str,
    summary: String,
) -> Fact {
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id,
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject: subject.clone(),
        capability: Some(Capability {
            category,
            provider: Some("rsscript".to_owned()),
            service: Some("native_rust_source_scan".to_owned()),
            action: None,
            resource: Some(subject.id.clone()),
            constraints: HashMap::new(),
        }),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Scanned, PACKAGE_REVIEW_SOURCE),
        acquisition_mode: AcquisitionMode::SourceScan,
        precision: Precision::Presence,
        evidence: vec![package_metadata(
            package_name,
            version,
            Some("source_scan_best_effort".to_owned()),
            Some(summary),
        )],
        unknown_reason: None,
    }
}

fn standard_library_export_capabilities(export: &RsScriptPackageExport) -> Vec<CapabilityCategory> {
    if export.kind != "function" {
        return Vec::new();
    }
    let name = export.name.as_str();
    let mut categories = Vec::new();
    match name {
        "Env.get" | "Env.get_or_default" | "Env.current_dir" | "Env.home_dir" | "Env.temp_dir" => {
            categories.push(CapabilityCategory::EnvRead)
        }
        "Env.set" | "Env.set_current_dir" => categories.push(CapabilityCategory::EnvWrite),
        "Http.get" | "Http.post_json" | "Http.post_form" => {
            categories.push(CapabilityCategory::NetworkClient);
        }
        "Process.run"
        | "Process.run_timeout"
        | "Process.run_request"
        | "Process.run_async"
        | "Process.run_timeout_async"
        | "Process.run_request_async"
        | "Process.run_request_cancellable_async"
        | "Process.stream"
        | "Process.run_stdout"
        | "Process.run_stdout_timeout"
        | "Process.run_stdout_async"
        | "Process.run_stdout_timeout_async"
        | "Process.run_many_stdout"
        | "Process.run_many_stdout_timeout"
        | "Process.run_many_stdout_async"
        | "Process.run_many_stdout_timeout_async" => {
            categories.push(CapabilityCategory::ProcessSpawn);
        }
        "Args.count" | "Args.get_or_default" => {
            categories.push(CapabilityCategory::ProcessArgs);
        }
        "Clock.now" | "Clock.system_unix_ms" | "Instant.elapsed" => {
            categories.push(CapabilityCategory::TimeRead);
        }
        "Uuid.new_v4" | "Random.int" | "Random.bool" | "Random.float" | "Random.bytes"
        | "Random.string" => {
            categories.push(CapabilityCategory::RandomRead);
        }
        "Hash.sha256_string"
        | "Hash.sha256_bytes"
        | "Hash.sha3_224_bytes"
        | "Hash.sha3_256_bytes"
        | "Hash.shake128_bytes" => {
            categories.push(CapabilityCategory::ComputeHash);
        }
        "Hash.sha256_file" => {
            categories.push(CapabilityCategory::ComputeHash);
            categories.push(CapabilityCategory::FilesystemRead);
        }
        "Regex.compile" | "Regex.is_match" | "Regex.find" | "Regex.captures"
        | "Regex.replace_all" | "Regex.split" => categories.push(CapabilityCategory::ComputeRegex),
        "Log.write" => categories.push(CapabilityCategory::TelemetryEmit),
        "Directory.exists"
        | "Directory.is_file"
        | "Directory.is_dir"
        | "Directory.list_files"
        | "Directory.metadata"
        | "Directory.read_string"
        | "Config.load"
        | "RuleLoader.load_rules"
        | "Csv.open_read"
        | "Csv.read_into"
        | "Image.load"
        | "Json.parse_file"
        | "Toml.parse_file"
        | "Yaml.parse_file"
        | "File.open_read"
        | "File.read_all"
        | "File.read_all_string"
        | "File.read_into"
        | "TempDir.path" => categories.push(CapabilityCategory::FilesystemRead),
        "Directory.create_all"
        | "Directory.remove_file"
        | "Directory.remove_dir_all"
        | "Directory.write_string"
        | "File.open_write"
        | "File.write"
        | "File.write_string"
        | "File.write_buffer"
        | "Image.save"
        | "TempDir.new"
        | "TempDir.new_in"
        | "TempDir.keep" => categories.push(CapabilityCategory::FilesystemWrite),
        "File.open" => {
            categories.push(CapabilityCategory::FilesystemRead);
            categories.push(CapabilityCategory::FilesystemWrite);
        }
        "Directory.copy_file" | "Directory.rename" => {
            categories.push(CapabilityCategory::FilesystemRead);
            categories.push(CapabilityCategory::FilesystemWrite);
        }
        _ => {}
    }
    categories.sort_by_key(|category| String::from(category.clone()));
    categories.dedup();
    categories
}

fn native_source_scan_summary(scan: &RsScriptNativeSourceScan) -> String {
    format!(
        "native_source_scan tool={} graph={} unsafe_detected={} ffi_detected={} filesystem_detected={} network_detected={} build_script_present={} worker_thread_parallelism_detected={}",
        scan.tool,
        scan.selected_graph,
        scan.unsafe_detected,
        scan.ffi_detected,
        scan.filesystem_detected,
        scan.network_detected,
        scan.build_script_present,
        scan.worker_thread_parallelism_detected
    )
}
