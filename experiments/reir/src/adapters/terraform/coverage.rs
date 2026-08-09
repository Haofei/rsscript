// Explicit Unknown coverage and bounded parse diagnostics.

fn mark_source_scan_unverified(fact: &mut Fact) {
    fact.value = FactValue::Unknown;
    fact.confidence = Confidence {
        level: ConfidenceLevel::Scanned,
        source: Some(PRODUCER_SOURCE.to_owned()),
    };
    fact.acquisition_mode = AcquisitionMode::SourceScan;
    fact.unknown_reason = Some(match fact.unknown_reason.take() {
        Some(reason) => format!("{reason}; {SOURCE_EVIDENCE_REASON}"),
        None => SOURCE_EVIDENCE_REASON.to_owned(),
    });
    for evidence in &mut fact.evidence {
        evidence.kind = EvidenceKind::SourceTemplatePointer;
        evidence.source = Some(PRODUCER_SOURCE.to_owned());
        evidence.reason = Some(match evidence.reason.take() {
            Some(reason) => format!("{reason}; {SOURCE_EVIDENCE_REASON}"),
            None => SOURCE_EVIDENCE_REASON.to_owned(),
        });
    }
}

fn unsupported_terraform_resource_fact(
    source: &str,
    acquisition_mode: AcquisitionMode,
    evidence_kind: EvidenceKind,
    resource_type: &str,
    name: &str,
    address: &str,
    json_pointer: Option<String>,
) -> Fact {
    let resource_id = if address.is_empty() {
        format!("{resource_type}.{name}")
    } else {
        address.to_owned()
    };
    let reason = format!(
        "Terraform resource `{resource_id}` has unsupported type `{resource_type}`; capability coverage is unknown"
    );
    UnknownCoverage {
        id: format!("fact.terraform.unsupported.{}", sanitize_id(&resource_id)),
        subject_kind: SubjectKind::TerraformResource,
        subject_id: format!("terraform::{resource_id}"),
        subject_name: resource_id,
        package: "terraform",
        reason,
        source,
        acquisition_mode,
        evidence_kind,
        evidence_file: source,
        evidence_pointer: json_pointer,
        evidence_value: "unsupported_resource_type",
    }
    .into_fact()
}

struct TerraformParseDiagnostic<'a> {
    source: &'a str,
    acquisition_mode: AcquisitionMode,
    evidence_kind: EvidenceKind,
    resource_type: &'a str,
    name: &'a str,
    address: &'a str,
    json_pointer: Option<String>,
    error: &'a serde_json::Error,
}

fn push_terraform_parse_diagnostic(
    diagnostics: &mut Vec<Fact>,
    omitted: &mut usize,
    input: TerraformParseDiagnostic<'_>,
) {
    if diagnostics.len() >= MAX_TERRAFORM_PARSE_DIAGNOSTICS - 1 {
        *omitted += 1;
        return;
    }
    let resource_id = if input.address.is_empty() {
        format!("{}.{}", input.resource_type, input.name)
    } else {
        input.address.to_owned()
    };
    let reason = format!(
        "failed to parse embedded IAM policy JSON for {}: {}",
        resource_id, input.error
    );
    diagnostics.push(Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.terraform.diagnostic.policy_parse.{}.{}",
            sanitize_id(&resource_id),
            diagnostics.len()
        ),
        kind: FactKind::Diagnostic,
        role: None,
        subject: Subject {
            kind: SubjectKind::TerraformResource,
            id: format!("terraform::{resource_id}"),
            name: Some(resource_id.clone()),
            package: Some("terraform".to_owned()),
        },
        capability: None,
        value: FactValue::Unknown,
        confidence: Confidence {
            level: ConfidenceLevel::Authoritative,
            source: Some("terraform_plan_json".to_owned()),
        },
        acquisition_mode: input.acquisition_mode,
        precision: Precision::Exact,
        evidence: vec![Evidence {
            kind: input.evidence_kind,
            file: Some(input.source.to_owned()),
            line: None,
            column: None,
            length: None,
            symbol: Some(resource_id),
            reason: Some(reason.clone()),
            json_pointer: input.json_pointer,
            resource: Some(input.address.to_owned()),
            provider: Some("terraform".to_owned()),
            value: Some("invalid_json".to_owned()),
            event_id: None,
            time: None,
            source: Some("terraform_plan_json".to_owned()),
            event_name: None,
            principal: None,
            account: None,
            policy_arn: None,
            statement_index: None,
            action: None,
        }],
        unknown_reason: Some(reason),
    });
}

fn terraform_diagnostic_budget_fact(omitted: usize) -> Fact {
    let reason = format!(
        "Terraform embedded policy parse diagnostics exceeded the {MAX_TERRAFORM_PARSE_DIAGNOSTICS} item budget; {omitted} additional failure(s) were omitted"
    );
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: "fact.terraform.diagnostic.policy_parse_budget".to_owned(),
        kind: FactKind::Diagnostic,
        role: None,
        subject: Subject {
            kind: SubjectKind::TerraformWorkspace,
            id: "terraform::workspace".to_owned(),
            name: Some("terraform workspace".to_owned()),
            package: Some("terraform".to_owned()),
        },
        capability: None,
        value: FactValue::Unknown,
        confidence: Confidence {
            level: ConfidenceLevel::Authoritative,
            source: Some("terraform_plan_json".to_owned()),
        },
        acquisition_mode: AcquisitionMode::TerraformPlan,
        precision: Precision::Exact,
        evidence: vec![Evidence {
            kind: EvidenceKind::UnknownReason,
            file: None,
            line: None,
            column: None,
            length: None,
            symbol: Some("terraform_policy_parse_diagnostic_budget".to_owned()),
            reason: Some(reason.clone()),
            json_pointer: None,
            resource: None,
            provider: Some("terraform".to_owned()),
            value: Some(omitted.to_string()),
            event_id: None,
            time: None,
            source: Some("terraform_plan_json".to_owned()),
            event_name: None,
            principal: None,
            account: None,
            policy_arn: None,
            statement_index: None,
            action: None,
        }],
        unknown_reason: Some(reason),
    }
}
