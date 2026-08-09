// Pure candidate fact construction from neutral package analysis.

/// Convert provider- and review-neutral package analysis into REIR facts.
///
/// External imports deliberately remain structural facts. Capability
/// classification requires binding/provider metadata and must not be inferred
/// from a symbol name alone.
pub fn package_analysis_to_facts(input: &RsScriptPackageAnalysisInput) -> Vec<Fact> {
    let package = package_subject(&input.package.name, &input.package.version);
    let package_slug = normalized_id(&package.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_analysis.{package_slug}.identity"),
        kind: FactKind::Extension("package_analysis".to_owned()),
        role: None,
        subject: package,
        capability: None,
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![package_analysis_metadata(
            input,
            input.module_digest.clone(),
            Some(format!(
                "schema={}; language={}; snapshot={}; module={}; interfaces={}",
                input.schema,
                input.language_version,
                input.snapshot_digest,
                input.module_digest.as_deref().unwrap_or("not-built"),
                input.interface_catalog_digest
            )),
        )],
        unknown_reason: None,
    }];

    for export in &input.exports {
        facts.extend(export_facts(input, export));
    }
    for (index, external) in input.external_imports.iter().enumerate() {
        facts.push(external_import_fact(input, external, index));
    }
    for (index, await_site) in input.await_sites.iter().enumerate() {
        facts.push(await_site_fact(input, await_site, index));
    }
    for (index, diagnostic) in input.diagnostics.iter().enumerate() {
        facts.push(diagnostic_fact(input, diagnostic, index, &package_slug));
    }
    facts
}

fn export_facts(
    input: &RsScriptPackageAnalysisInput,
    export: &RsScriptPackageAnalysisExport,
) -> Vec<Fact> {
    let subject = package_analysis_export_subject(&input.package.name, export);
    let export_slug = normalized_id(&subject.id);
    let mut facts = vec![Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!("fact.package_analysis.{export_slug}.public_contract"),
        kind: FactKind::PublicContract,
        role: None,
        subject: subject.clone(),
        capability: None,
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![package_analysis_metadata(
            input,
            Some(export.kind.clone()),
            Some(format!(
                "public {} `{}`{}",
                export.kind,
                export.name,
                export
                    .function_kind
                    .as_deref()
                    .map(|kind| format!(" ({kind})"))
                    .unwrap_or_default()
            )),
        )],
        unknown_reason: None,
    }];
    let semantic_facts = export
        .semantic_facts
        .iter()
        .cloned()
        .chain(
            export
                .retained_params
                .iter()
                .map(|param| format!("retains({param})")),
        )
        .collect::<BTreeSet<_>>();
    for (index, semantic_fact) in semantic_facts.into_iter().enumerate() {
        facts.push(Fact {
            schema: FACT_SCHEMA.to_owned(),
            id: format!("fact.package_analysis.{export_slug}.semantic.{index}"),
            kind: semantic_kind(&semantic_fact),
            role: None,
            subject: subject.clone(),
            capability: None,
            value: FactValue::True,
            confidence: confidence(ConfidenceLevel::Authoritative),
            acquisition_mode: AcquisitionMode::CompilerContract,
            precision: Precision::Exact,
            evidence: vec![package_analysis_metadata(
                input,
                Some(semantic_fact.clone()),
                Some(format!("{}: {semantic_fact}", export.name)),
            )],
            unknown_reason: None,
        });
    }
    facts
}

fn external_import_fact(
    input: &RsScriptPackageAnalysisInput,
    external: &RsScriptPackageAnalysisExternalImport,
    index: usize,
) -> Fact {
    let subject = function_subject(&input.package.name, &external.function);
    let reason = if external.call_chain.is_empty() {
        format!("external symbol `{}`", external.symbol)
    } else {
        format!(
            "external symbol `{}` via {}",
            external.symbol,
            external.call_chain.join(" -> ")
        )
    };
    let evidence = external
        .span
        .as_ref()
        .map(|span| source_evidence(span, &external.symbol, reason.clone()))
        .unwrap_or_else(|| {
            package_analysis_metadata(input, Some(external.symbol.clone()), Some(reason))
        });
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.package_analysis.{}.external_import.{index}",
            normalized_id(&subject.id)
        ),
        kind: FactKind::Extension("external_import".to_owned()),
        role: None,
        subject,
        capability: None,
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![evidence],
        unknown_reason: None,
    }
}

fn await_site_fact(
    input: &RsScriptPackageAnalysisInput,
    await_site: &RsScriptPackageAnalysisAwaitSite,
    index: usize,
) -> Fact {
    let subject = function_subject(&input.package.name, &await_site.function);
    let callee = await_site.callee.as_deref().unwrap_or("unknown callee");
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.package_analysis.{}.await.{index}",
            normalized_id(&subject.id)
        ),
        kind: FactKind::AsyncBoundary,
        role: None,
        subject,
        capability: None,
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![source_evidence(
            &await_site.span,
            &await_site.function,
            format!(
                "awaits {callee}; live across await: {}",
                if await_site.live_across_await.is_empty() {
                    "none".to_owned()
                } else {
                    await_site.live_across_await.join(", ")
                }
            ),
        )],
        unknown_reason: None,
    }
}

fn diagnostic_fact(
    input: &RsScriptPackageAnalysisInput,
    diagnostic: &RsScriptPackageAnalysisDiagnostic,
    index: usize,
    package_slug: &str,
) -> Fact {
    let is_error = diagnostic.severity.eq_ignore_ascii_case("error");
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.package_analysis.{package_slug}.diagnostic.{}.{index}",
            normalized_id(&diagnostic.code)
        ),
        kind: FactKind::Diagnostic,
        role: None,
        subject: package_subject(&input.package.name, &input.package.version),
        capability: None,
        value: if is_error {
            FactValue::Unknown
        } else {
            FactValue::True
        },
        confidence: confidence(ConfidenceLevel::Authoritative),
        acquisition_mode: AcquisitionMode::CompilerContract,
        precision: Precision::Exact,
        evidence: vec![source_evidence(
            &diagnostic.span,
            &diagnostic.code,
            format!(
                "{}: {}{}",
                diagnostic.severity,
                diagnostic.summary,
                if diagnostic.label.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", diagnostic.label)
                }
            ),
        )],
        unknown_reason: is_error.then(|| diagnostic.summary.clone()),
    }
}

fn package_analysis_export_subject(
    package_name: &str,
    export: &RsScriptPackageAnalysisExport,
) -> Subject {
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

fn semantic_kind(semantic_fact: &str) -> FactKind {
    let semantic_fact = semantic_fact.to_ascii_lowercase();
    if semantic_fact.contains("resource")
        || semantic_fact.contains("handle")
        || semantic_fact.contains("weak field")
    {
        FactKind::Resource
    } else if semantic_fact.starts_with("retains(") {
        FactKind::Retention
    } else if semantic_fact.starts_with("mut ") || semantic_fact.starts_with("take ") {
        FactKind::Mutation
    } else if semantic_fact.contains("async") {
        FactKind::AsyncBoundary
    } else if semantic_fact.contains("fresh") {
        FactKind::Extension("freshness".to_owned())
    } else {
        FactKind::Extension("semantic_fact".to_owned())
    }
}
