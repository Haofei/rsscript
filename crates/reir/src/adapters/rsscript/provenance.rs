// Evidence metadata, subjects, and producer provenance.

fn confidence(level: ConfidenceLevel) -> Confidence {
    Confidence {
        level,
        source: Some(PACKAGE_ANALYSIS_SOURCE.to_owned()),
    }
}

fn base_evidence(kind: EvidenceKind) -> Evidence {
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

fn package_analysis_metadata(
    input: &RsScriptPackageAnalysisInput,
    value: Option<String>,
    reason: Option<String>,
) -> Evidence {
    Evidence {
        reason,
        resource: Some(format!("{}@{}", input.package.name, input.package.version)),
        provider: Some("rsscript".to_owned()),
        value,
        source: Some(PACKAGE_ANALYSIS_SOURCE.to_owned()),
        ..base_evidence(EvidenceKind::PackageMetadata)
    }
}

fn source_evidence(span: &RsScriptDiagnosticSpan, symbol: &str, reason: String) -> Evidence {
    Evidence {
        file: (!span.file.is_empty()).then(|| span.file.clone()),
        line: Some(span.line.max(1)),
        column: Some(span.column.max(1)),
        length: Some(span.length),
        symbol: Some(symbol.to_owned()),
        reason: Some(reason),
        source: Some(PACKAGE_ANALYSIS_SOURCE.to_owned()),
        ..base_evidence(EvidenceKind::SourceSpan)
    }
}

fn package_subject(package_name: &str, version: &str) -> Subject {
    Subject {
        kind: SubjectKind::Package,
        id: format!("{package_name}@{version}"),
        name: Some(package_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn function_subject(package_name: &str, function_name: &str) -> Subject {
    Subject {
        kind: SubjectKind::CodeFunction,
        id: format!("{package_name}::function::{function_name}"),
        name: Some(function_name.to_owned()),
        package: Some(package_name.to_owned()),
    }
}

fn normalized_id(input: &str) -> String {
    input
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

const fn rsscript_provenance() -> ProducerProvenance {
    ProducerProvenance {
        name: "rssc",
        version: PRODUCER_VERSION,
        adapter: "rsscript-package-analysis",
        adapter_version: ADAPTER_VERSION,
        source: PACKAGE_ANALYSIS_SOURCE,
    }
}
