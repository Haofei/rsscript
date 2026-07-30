use reir::{
    FactKind, FactValue,
    adapters::{
        rsscript::rsscript_lock_json_to_bundle,
        terraform::{TerraformPlanLimits, terraform_plan_json_to_bundle_with_limits},
    },
};

const UNSUPPORTED_PLAN: &str = r#"{
    "resource_changes": [{
        "address": "aws_lambda_function.worker",
        "type": "aws_lambda_function",
        "name": "worker",
        "change": { "after": {} }
    }]
}"#;

#[test]
fn terraform_unsupported_resources_are_explicit_unknown_coverage() {
    let bundle =
        terraform_plan_json_to_bundle_with_limits(UNSUPPORTED_PLAN, TerraformPlanLimits::default())
            .expect("plan should convert");

    let fact = bundle
        .facts
        .iter()
        .find(|fact| fact.id.contains("unsupported"))
        .expect("unsupported resources must produce evidence");
    assert_eq!(fact.kind, FactKind::Diagnostic);
    assert_eq!(fact.value, FactValue::Unknown);
    assert!(
        fact.unknown_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("coverage is unknown"))
    );
    assert_eq!(
        fact.evidence[0].value.as_deref(),
        Some("unsupported_resource_type")
    );
}

#[test]
fn terraform_fact_budget_fails_before_returning_partial_evidence() {
    let error = terraform_plan_json_to_bundle_with_limits(
        UNSUPPORTED_PLAN,
        TerraformPlanLimits {
            max_facts: 0,
            ..TerraformPlanLimits::default()
        },
    )
    .expect_err("zero fact budget must reject the conversion");

    assert!(
        error.contains("fact") && error.contains("limit"),
        "{error}"
    );
}

#[test]
fn adapter_bundles_include_complete_producer_provenance() {
    let rsscript = rsscript_lock_json_to_bundle(
        r#"{
            "version": 1,
            "lockfile_path": "rsspkg.lock",
            "package": [{
                "name": "demo",
                "version": "1.0.0",
                "source": "path+demo",
                "checksum": "sha256:package",
                "interface_hash": "sha256:interface",
                "review_hash": "sha256:review",
                "features": []
            }]
        }"#,
    )
    .expect("RSScript lock should convert");
    let terraform =
        terraform_plan_json_to_bundle_with_limits(UNSUPPORTED_PLAN, TerraformPlanLimits::default())
            .expect("Terraform plan should convert");

    for bundle in [&rsscript, &terraform] {
        let producer = bundle.producers.first().expect("producer");
        assert!(!producer.name.is_empty());
        assert!(!producer.version.is_empty());
        assert!(
            producer
                .adapter
                .as_deref()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(
            producer
                .adapter_version
                .as_deref()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(
            producer
                .source
                .as_deref()
                .is_some_and(|value| !value.is_empty())
        );
    }
}
