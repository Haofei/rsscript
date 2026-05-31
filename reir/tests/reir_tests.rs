use reir::*;
use serde_json::json;
use std::collections::HashMap;

fn empty_evidence(kind: EvidenceKind) -> Evidence {
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

fn source_span(file: &str, line: usize, symbol: &str) -> Evidence {
    Evidence {
        file: Some(file.to_owned()),
        line: Some(line),
        symbol: Some(symbol.to_owned()),
        ..empty_evidence(EvidenceKind::SourceSpan)
    }
}

fn manifest_pointer(file: &str, json_pointer: &str) -> Evidence {
    Evidence {
        file: Some(file.to_owned()),
        json_pointer: Some(json_pointer.to_owned()),
        ..empty_evidence(EvidenceKind::ManifestPointer)
    }
}

fn image_digest(value: &str) -> Evidence {
    Evidence {
        value: Some(value.to_owned()),
        ..empty_evidence(EvidenceKind::ImageDigest)
    }
}

fn confidence(level: ConfidenceLevel, source: &str) -> Confidence {
    Confidence {
        level,
        source: Some(source.to_owned()),
    }
}

fn subject(kind: SubjectKind, id: &str) -> Subject {
    Subject {
        kind,
        id: id.to_owned(),
        name: None,
        package: None,
    }
}

fn named_subject(kind: SubjectKind, id: &str, name: &str, package: &str) -> Subject {
    Subject {
        kind,
        id: id.to_owned(),
        name: Some(name.to_owned()),
        package: Some(package.to_owned()),
    }
}

fn capability(
    category: CapabilityCategory,
    provider: &str,
    action: &str,
    resource: &str,
) -> Capability {
    Capability {
        category,
        provider: Some(provider.to_owned()),
        service: None,
        action: Some(action.to_owned()),
        resource: Some(resource.to_owned()),
        constraints: HashMap::new(),
    }
}

fn required_capability_fact(id: &str, subject: Subject, capability: Capability) -> Fact {
    Fact {
        schema: "reir.fact.v0.1".to_owned(),
        id: id.to_owned(),
        kind: FactKind::Capability,
        role: Some(FactRole::Required),
        subject,
        capability: Some(capability),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Computed, "sdk_mapping"),
        acquisition_mode: AcquisitionMode::SourceScan,
        precision: Precision::ResourceScoped,
        evidence: vec![source_span("src/report_upload.rs", 88, "s3.put_object")],
        unknown_reason: None,
    }
}

fn granted_capability_fact(id: &str, subject: Subject, capability: Capability) -> Fact {
    Fact {
        schema: "reir.fact.v0.1".to_owned(),
        id: id.to_owned(),
        kind: FactKind::Capability,
        role: Some(FactRole::Granted),
        subject,
        capability: Some(capability),
        value: FactValue::True,
        confidence: confidence(ConfidenceLevel::Authoritative, "iam_policy"),
        acquisition_mode: AcquisitionMode::CloudPolicy,
        precision: Precision::ResourceScoped,
        evidence: vec![manifest_pointer(
            "terraform/prod/iam.tf",
            "/resource/aws_iam_role/checkout_prod",
        )],
        unknown_reason: None,
    }
}

fn producer() -> Producer {
    Producer {
        name: "rsscript".to_owned(),
        version: "0.5.0".to_owned(),
        adapter: Some("aws".to_owned()),
        adapter_version: Some("1.2.3".to_owned()),
        source: Some("git+https://example.com/rsscript".to_owned()),
    }
}

fn subject_chain() -> SubjectChain {
    SubjectChain {
        schema: "reir.subject_chain.v0.1".to_owned(),
        id: "chain.checkout.report_uploader.prod".to_owned(),
        kind: "execution_subject_chain".to_owned(),
        nodes: vec![
            subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
            subject(SubjectKind::Package, "checkout@1.4.2"),
            subject(
                SubjectKind::ContainerImage,
                "registry.example.com/checkout@sha256:abc...",
            ),
            subject(
                SubjectKind::K8sDeployment,
                "prod/apps/Deployment/checkout-api",
            ),
            subject(
                SubjectKind::K8sServiceAccount,
                "prod/ServiceAccount/checkout-api",
            ),
            subject(
                SubjectKind::CloudRole,
                "aws:arn:aws:iam::123456789012:role/checkout-prod",
            ),
        ],
        edges: vec![
            ChainEdge {
                kind: ChainEdgeKind::BuiltAs,
                from: 0,
                to: 2,
            },
            ChainEdge {
                kind: ChainEdgeKind::DeployedAs,
                from: 2,
                to: 3,
            },
            ChainEdge {
                kind: ChainEdgeKind::RunsAs,
                from: 3,
                to: 4,
            },
            ChainEdge {
                kind: ChainEdgeKind::AssumesRole,
                from: 4,
                to: 5,
            },
        ],
        evidence: vec![
            image_digest("sha256:abc..."),
            manifest_pointer(
                "rendered/prod/deployment.yaml",
                "/spec/template/spec/serviceAccountName",
            ),
            manifest_pointer(
                "terraform/prod/iam.tf",
                "/resource/aws_iam_role/checkout_prod",
            ),
        ],
    }
}

fn requires_capability_edge() -> Edge {
    Edge {
        schema: "reir.edge.v0.1".to_owned(),
        id: "edge.checkout.upload.requires.s3_put_object".to_owned(),
        kind: EdgeKind::RequiresCapability,
        from: subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
        to: subject(
            SubjectKind::Capability,
            "cap.aws.s3.PutObject.reports-prod.exports",
        ),
        confidence: confidence(ConfidenceLevel::Computed, "sdk_mapping"),
        acquisition_mode: AcquisitionMode::SourceScan,
        precision: Precision::ResourceScoped,
        evidence: vec![source_span("src/report_upload.rs", 88, "s3.put_object")],
    }
}

fn missing_profile() -> Profile {
    Profile {
        kind: "prod-deployment-profile".to_owned(),
        allow: HashMap::new(),
        budget: Some(ProfileBudget {
            max_missing_capabilities: 0,
            max_unknown_coverage: 0,
            max_excess_grants: 5,
        }),
    }
}

fn find_reconciliation<'a>(
    reconciliations: &'a [Reconciliation],
    kind: ReconciliationKind,
) -> &'a Reconciliation {
    reconciliations
        .iter()
        .find(|reconciliation| reconciliation.kind == kind)
        .unwrap_or_else(|| panic!("missing reconciliation: {kind:?}"))
}

fn find_policy_result<'a>(results: &'a [PolicyResult], suffix: &str) -> &'a PolicyResult {
    results
        .iter()
        .find(|result| result.id.ends_with(suffix))
        .unwrap_or_else(|| panic!("missing policy result suffix: {suffix}"))
}

#[test]
fn bundle_serialization_round_trip() {
    let chain = subject_chain();
    let required = required_capability_fact(
        "fact.checkout.report_uploader.requires.s3_put_object",
        subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/exports/*",
        ),
    );
    let granted = granted_capability_fact(
        "fact.checkout.role.grants.s3_put_object",
        subject(
            SubjectKind::CloudRole,
            "aws:arn:aws:iam::123456789012:role/checkout-prod",
        ),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/*",
        ),
    );
    let reconciliations = reconcile_capabilities(&[required.clone()], &[granted.clone()]);
    let policy_results = evaluate_policy(&missing_profile(), &reconciliations);

    let bundle_without_diff = Bundle {
        schema: "reir.bundle.v0.1".to_owned(),
        ontology: "reir.capability_ontology.v0.1".to_owned(),
        producers: vec![producer()],
        subjects: vec![required.subject.clone(), granted.subject.clone()],
        subject_chains: vec![chain],
        facts: vec![required, granted],
        edges: vec![requires_capability_edge()],
        reconciliations: reconciliations.clone(),
        slices: vec![Slice {
            schema: "reir.slice.v0.1".to_owned(),
            id: "slice.missing_capability".to_owned(),
            kind: SliceKind::MissingCapabilitySlice,
            facts: vec!["fact.checkout.report_uploader.requires.s3_put_object".to_owned()],
            edges: vec!["edge.checkout.upload.requires.s3_put_object".to_owned()],
            reconciliations: vec![
                "recon.covered.fact.checkout.report_uploader.requires.s3_put_object".to_owned(),
            ],
            evidence: vec![source_span("src/report_upload.rs", 88, "s3.put_object")],
        }],
        policy_results,
        diffs: Vec::new(),
        exceptions: vec![Exception {
            id: "exception.checkout.review.1".to_owned(),
            accepted_by: "reviewer@example.com".to_owned(),
            reason: "temporary rollout exception".to_owned(),
            expires: Some("2025-01-01T00:00:00Z".to_owned()),
            evidence: vec![manifest_pointer("review/exceptions.json", "/exceptions/0")],
        }],
    };

    let diff = compute_diff(&Bundle::new(), &bundle_without_diff);
    let bundle = Bundle {
        diffs: vec![diff],
        ..bundle_without_diff
    };

    let json = bundle.to_json().unwrap();
    let round_trip = Bundle::from_json(&json).unwrap();

    assert_eq!(round_trip, bundle);
}

#[test]
fn fact_with_capability_serializes_to_expected_json_structure() {
    let fact = required_capability_fact(
        "fact.checkout.report_uploader.requires.s3_put_object",
        subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/exports/*",
        ),
    );

    let value = serde_json::to_value(&fact).unwrap();

    assert_eq!(
        value,
        json!({
            "schema": "reir.fact.v0.1",
            "id": "fact.checkout.report_uploader.requires.s3_put_object",
            "kind": "capability",
            "role": "required",
            "subject": {
                "kind": "code.function",
                "id": "checkout::ReportUploader.upload"
            },
            "capability": {
                "category": "object_storage.write",
                "provider": "aws",
                "action": "s3:PutObject",
                "resource": "arn:aws:s3:::reports-prod/exports/*"
            },
            "value": true,
            "confidence": {
                "level": "computed",
                "source": "sdk_mapping"
            },
            "acquisition_mode": "source_scan",
            "precision": "resource_scoped",
            "evidence": [
                {
                    "kind": "source_span",
                    "file": "src/report_upload.rs",
                    "line": 88,
                    "symbol": "s3.put_object"
                }
            ]
        })
    );
}

#[test]
fn subject_with_all_fields_serializes_correctly() {
    let subject = named_subject(
        SubjectKind::CodeFunction,
        "my-app::ReportUploader.upload",
        "ReportUploader.upload",
        "my-app",
    );

    assert_eq!(
        serde_json::to_value(&subject).unwrap(),
        json!({
            "kind": "code.function",
            "id": "my-app::ReportUploader.upload",
            "name": "ReportUploader.upload",
            "package": "my-app"
        })
    );
}

#[test]
fn subject_chain_with_nodes_and_edges_serializes_correctly() {
    let chain = subject_chain();

    assert_eq!(
        serde_json::to_value(&chain).unwrap(),
        json!({
            "schema": "reir.subject_chain.v0.1",
            "id": "chain.checkout.report_uploader.prod",
            "kind": "execution_subject_chain",
            "nodes": [
                { "kind": "code.function", "id": "checkout::ReportUploader.upload" },
                { "kind": "package", "id": "checkout@1.4.2" },
                { "kind": "container.image", "id": "registry.example.com/checkout@sha256:abc..." },
                { "kind": "k8s.deployment", "id": "prod/apps/Deployment/checkout-api" },
                { "kind": "k8s.service_account", "id": "prod/ServiceAccount/checkout-api" },
                { "kind": "cloud.role", "id": "aws:arn:aws:iam::123456789012:role/checkout-prod" }
            ],
            "edges": [
                { "kind": "built_as", "from": 0, "to": 2 },
                { "kind": "deployed_as", "from": 2, "to": 3 },
                { "kind": "runs_as", "from": 3, "to": 4 },
                { "kind": "assumes_role", "from": 4, "to": 5 }
            ],
            "evidence": [
                { "kind": "image_digest", "value": "sha256:abc..." },
                {
                    "kind": "manifest_pointer",
                    "file": "rendered/prod/deployment.yaml",
                    "json_pointer": "/spec/template/spec/serviceAccountName"
                },
                {
                    "kind": "manifest_pointer",
                    "file": "terraform/prod/iam.tf",
                    "json_pointer": "/resource/aws_iam_role/checkout_prod"
                }
            ]
        })
    );
}

#[test]
fn edge_serializes_correctly() {
    let edge = requires_capability_edge();

    assert_eq!(
        serde_json::to_value(&edge).unwrap(),
        json!({
            "schema": "reir.edge.v0.1",
            "id": "edge.checkout.upload.requires.s3_put_object",
            "kind": "requires_capability",
            "from": {
                "kind": "code.function",
                "id": "checkout::ReportUploader.upload"
            },
            "to": {
                "kind": "capability",
                "id": "cap.aws.s3.PutObject.reports-prod.exports"
            },
            "confidence": {
                "level": "computed",
                "source": "sdk_mapping"
            },
            "acquisition_mode": "source_scan",
            "precision": "resource_scoped",
            "evidence": [
                {
                    "kind": "source_span",
                    "file": "src/report_upload.rs",
                    "line": 88,
                    "symbol": "s3.put_object"
                }
            ]
        })
    );
}

#[test]
fn reconcile_missing_capability() {
    let required = required_capability_fact(
        "fact.required.put",
        subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/exports/*",
        ),
    );
    let granted = granted_capability_fact(
        "fact.granted.get",
        subject(SubjectKind::CloudRole, "aws:role/checkout-prod"),
        capability(
            CapabilityCategory::ObjectStorageRead,
            "aws",
            "s3:GetObject",
            "arn:aws:s3:::reports-prod/*",
        ),
    );

    let reconciliations = reconcile_capabilities(&[required], &[granted]);
    let missing = find_reconciliation(&reconciliations, ReconciliationKind::MissingCapability);

    assert_eq!(missing.status, ReconciliationStatus::Fail);
    assert_eq!(missing.required_fact.as_deref(), Some("fact.required.put"));
    assert!(missing.granted_facts.is_empty());
    assert_eq!(
        missing.capability.as_ref().unwrap().action.as_deref(),
        Some("s3:PutObject")
    );
    assert_eq!(
        missing.risk.as_ref().unwrap().class,
        RiskClass::Availability
    );
}

#[test]
fn reconcile_covered_capability() {
    let required = required_capability_fact(
        "fact.required.put",
        subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/exports/*",
        ),
    );
    let granted = granted_capability_fact(
        "fact.granted.put",
        subject(SubjectKind::CloudRole, "aws:role/checkout-prod"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/*",
        ),
    );

    let reconciliations = reconcile_capabilities(&[required], &[granted]);
    let covered = find_reconciliation(&reconciliations, ReconciliationKind::Covered);

    assert_eq!(covered.status, ReconciliationStatus::Pass);
    assert_eq!(covered.required_fact.as_deref(), Some("fact.required.put"));
    assert_eq!(covered.granted_facts, vec!["fact.granted.put".to_owned()]);
    assert!(covered.risk.is_none());
}

#[test]
fn reconcile_excess_capability() {
    let granted = granted_capability_fact(
        "fact.granted.delete",
        subject(SubjectKind::CloudRole, "aws:role/checkout-prod"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:DeleteObject",
            "arn:aws:s3:::reports-prod/*",
        ),
    );

    let reconciliations = reconcile_capabilities(&[], &[granted]);
    let excess = find_reconciliation(&reconciliations, ReconciliationKind::ExcessCapability);

    assert_eq!(excess.status, ReconciliationStatus::Warn);
    assert!(excess.required_fact.is_none());
    assert_eq!(excess.granted_facts, vec!["fact.granted.delete".to_owned()]);
    assert_eq!(excess.risk.as_ref().unwrap().class, RiskClass::Security);
}

#[test]
fn reconcile_multiple_capabilities_mixed_results() {
    let required_put = required_capability_fact(
        "fact.required.put",
        subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/exports/*",
        ),
    );
    let required_get = required_capability_fact(
        "fact.required.get",
        subject(
            SubjectKind::CodeFunction,
            "checkout::ReportUploader.download",
        ),
        capability(
            CapabilityCategory::ObjectStorageRead,
            "aws",
            "s3:GetObject",
            "arn:aws:s3:::reports-prod/exports/*",
        ),
    );
    let granted_put = granted_capability_fact(
        "fact.granted.put",
        subject(SubjectKind::CloudRole, "aws:role/checkout-prod"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/*",
        ),
    );
    let granted_delete = granted_capability_fact(
        "fact.granted.delete",
        subject(SubjectKind::CloudRole, "aws:role/checkout-prod"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:DeleteObject",
            "arn:aws:s3:::reports-prod/*",
        ),
    );

    let reconciliations = reconcile_capabilities(
        &[required_put, required_get],
        &[granted_put, granted_delete],
    );

    assert_eq!(reconciliations.len(), 3);
    assert!(
        reconciliations
            .iter()
            .any(|reconciliation| reconciliation.kind == ReconciliationKind::Covered)
    );
    assert!(
        reconciliations
            .iter()
            .any(|reconciliation| reconciliation.kind == ReconciliationKind::MissingCapability)
    );
    assert!(
        reconciliations
            .iter()
            .any(|reconciliation| reconciliation.kind == ReconciliationKind::ExcessCapability)
    );
}

#[test]
fn diff_detects_added_fact() {
    let current = Bundle {
        facts: vec![required_capability_fact(
            "fact.required.put",
            subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
            capability(
                CapabilityCategory::ObjectStorageWrite,
                "aws",
                "s3:PutObject",
                "arn:aws:s3:::reports-prod/exports/*",
            ),
        )],
        ..Bundle::new()
    };

    let diff = compute_diff(&Bundle::new(), &current);

    assert!(
        diff.items
            .iter()
            .any(|item| item.kind == DiffItemKind::FactAdded && item.id == "fact.required.put")
    );
}

#[test]
fn diff_detects_removed_fact() {
    let baseline = Bundle {
        facts: vec![required_capability_fact(
            "fact.required.put",
            subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
            capability(
                CapabilityCategory::ObjectStorageWrite,
                "aws",
                "s3:PutObject",
                "arn:aws:s3:::reports-prod/exports/*",
            ),
        )],
        ..Bundle::new()
    };

    let diff = compute_diff(&baseline, &Bundle::new());

    assert!(
        diff.items
            .iter()
            .any(|item| item.kind == DiffItemKind::FactRemoved && item.id == "fact.required.put")
    );
}

#[test]
fn policy_fails_when_missing_capabilities_exceed_budget() {
    let required = required_capability_fact(
        "fact.required.put",
        subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/exports/*",
        ),
    );

    let reconciliations = reconcile_capabilities(&[required], &[]);
    let results = evaluate_policy(&missing_profile(), &reconciliations);
    let missing_budget = find_policy_result(&results, "missing_capabilities");

    assert_eq!(missing_budget.status, PolicyStatus::Fail);
    assert!(
        missing_budget
            .reason
            .as_deref()
            .unwrap()
            .contains("max_missing_capabilities exceeded")
    );
}

#[test]
fn policy_passes_within_budget() {
    let excess_one = Reconciliation {
        schema: "reir.reconciliation.v0.1".to_owned(),
        id: "recon.excess.1".to_owned(),
        kind: ReconciliationKind::ExcessCapability,
        status: ReconciliationStatus::Warn,
        subject_chain: None,
        required_fact: None,
        granted_facts: vec!["fact.granted.delete.1".to_owned()],
        observed_fact: None,
        capability: Some(capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:DeleteObject",
            "arn:aws:s3:::reports-prod/*",
        )),
        risk: None,
        evidence: vec![],
    };
    let excess_two = Reconciliation {
        id: "recon.excess.2".to_owned(),
        granted_facts: vec!["fact.granted.delete.2".to_owned()],
        ..excess_one.clone()
    };
    let profile = Profile {
        kind: "prod-deployment-profile".to_owned(),
        allow: HashMap::new(),
        budget: Some(ProfileBudget {
            max_missing_capabilities: 0,
            max_unknown_coverage: 0,
            max_excess_grants: 5,
        }),
    };

    let results = evaluate_policy(&profile, &[excess_one, excess_two]);
    let excess_budget = find_policy_result(&results, "excess_grants");

    assert_eq!(excess_budget.status, PolicyStatus::Pass);
    assert!(
        !results
            .iter()
            .any(|result| result.status == PolicyStatus::Fail)
    );
}

#[test]
fn slice_groups_missing_capabilities() {
    let required = required_capability_fact(
        "fact.required.put",
        subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/exports/*",
        ),
    );
    let reconciliations = reconcile_capabilities(&[required], &[]);
    let bundle = Bundle {
        reconciliations,
        ..Bundle::new()
    };

    let slices = slice_by_kind(&bundle);
    let missing_slice = slices
        .iter()
        .find(|slice| slice.kind == SliceKind::MissingCapabilitySlice)
        .expect("missing capability slice");

    assert_eq!(missing_slice.id, "slice.missing_capability");
    assert_eq!(
        missing_slice.reconciliations,
        vec!["recon.missing.fact.required.put".to_owned()]
    );
    assert_eq!(missing_slice.facts, vec!["fact.required.put".to_owned()]);
}

#[test]
fn mvp_cross_layer_capability_reconciliation() {
    let required = required_capability_fact(
        "fact.checkout.report_uploader.requires.s3_put_object",
        subject(SubjectKind::CodeFunction, "checkout::ReportUploader.upload"),
        capability(
            CapabilityCategory::ObjectStorageWrite,
            "aws",
            "s3:PutObject",
            "arn:aws:s3:::reports-prod/exports/*",
        ),
    );
    let granted = granted_capability_fact(
        "fact.checkout.role.grants.s3_get_object",
        subject(
            SubjectKind::CloudRole,
            "aws:arn:aws:iam::123456789012:role/checkout-prod",
        ),
        capability(
            CapabilityCategory::ObjectStorageRead,
            "aws",
            "s3:GetObject",
            "arn:aws:s3:::reports-prod/*",
        ),
    );
    let chain = subject_chain();

    let reconciliations = reconcile_capabilities(&[required.clone()], &[granted.clone()]);
    assert!(
        reconciliations
            .iter()
            .any(|reconciliation| reconciliation.kind == ReconciliationKind::MissingCapability)
    );

    let policy_results = evaluate_policy(&missing_profile(), &reconciliations);
    assert!(
        policy_results
            .iter()
            .any(|result| result.status == PolicyStatus::Fail)
    );

    let mut bundle = Bundle {
        producers: vec![producer()],
        subjects: vec![required.subject.clone(), granted.subject.clone()],
        subject_chains: vec![chain],
        facts: vec![required, granted],
        reconciliations,
        policy_results,
        ..Bundle::new()
    };
    bundle.slices = slice_by_kind(&bundle);

    assert!(
        bundle
            .slices
            .iter()
            .any(|slice| slice.kind == SliceKind::MissingCapabilitySlice)
    );

    let json = bundle.to_json().unwrap();
    let round_trip = Bundle::from_json(&json).unwrap();

    assert_eq!(round_trip, bundle);
}
