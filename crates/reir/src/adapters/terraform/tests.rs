#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terraform_dir_to_bundle_reads_inline_iam_policy_grants() {
        let temp_dir =
            std::env::temp_dir().join(format!("reir-terraform-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        std::fs::write(
            temp_dir.join("main.tf"),
            r#"resource "aws_iam_role_policy" "report_uploader" {
  role = "report-uploader"
  policy = <<POLICY
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": "s3:GetObject",
      "Resource": "arn:aws:s3:::reports-prod/*"
    }
  ]
}
POLICY
}
"#,
        )
        .expect("Terraform fixture should be written");

        let bundle = terraform_dir_to_bundle(&temp_dir).expect("Terraform should collect");
        let _ = std::fs::remove_dir_all(&temp_dir);

        assert_eq!(bundle.producers.len(), 1);
        assert_eq!(
            bundle.producers[0].adapter.as_deref(),
            Some("reir.adapters.terraform")
        );
        assert_eq!(bundle.producers[0].source.as_deref(), Some("terraform_iac"));
        assert!(bundle.facts.iter().any(|fact| {
            fact.role == Some(FactRole::Granted)
                && fact.capability.as_ref().is_some_and(|capability| {
                    capability.action.as_deref() == Some("s3:GetObject")
                        && capability.resource.as_deref() == Some("arn:aws:s3:::reports-prod/*")
                })
        }));
        assert!(bundle.facts.iter().all(|fact| {
            fact.value == FactValue::Unknown
                && fact.confidence.level == ConfidenceLevel::Scanned
                && fact.acquisition_mode == AcquisitionMode::SourceScan
                && fact.evidence.iter().all(|evidence| {
                    evidence.kind == EvidenceKind::SourceTemplatePointer
                        && evidence
                            .reason
                            .as_deref()
                            .is_some_and(|reason| reason.contains("not proof"))
                })
        }));

        let mut required = bundle.facts[0].clone();
        required.id = "required.s3.get".to_owned();
        required.role = Some(FactRole::Required);
        required.value = FactValue::True;
        required.unknown_reason = None;
        required.confidence.level = ConfidenceLevel::Declared;
        required.acquisition_mode = AcquisitionMode::CompilerContract;
        let reconciliation = crate::reconcile_capabilities(&[required], &bundle.facts);
        assert!(
            reconciliation
                .iter()
                .all(|item| item.kind != ReconciliationKind::Covered),
            "source-template evidence must never prove deployed authorization"
        );
        assert!(
            reconciliation
                .iter()
                .any(|item| item.kind == ReconciliationKind::UnknownCoverage)
        );
    }

    #[test]
    fn terraform_dir_to_bundle_reads_s3_bucket_policy_grants() {
        let temp_dir = std::env::temp_dir().join(format!(
            "reir-terraform-bucket-policy-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        std::fs::write(
            temp_dir.join("bucket.tf"),
            r#"resource "aws_s3_bucket_policy" "reports" {
  bucket = "reports-prod"
  policy = <<POLICY
{
  "Version": "2012-10-17",
  "Statement": {
    "Effect": "Allow",
    "Action": ["s3:PutObject", "s3:DeleteObject"],
    "Resource": "arn:aws:s3:::reports-prod/*"
  }
}
POLICY
}
"#,
        )
        .expect("Terraform fixture should be written");

        let bundle = terraform_dir_to_bundle(&temp_dir).expect("Terraform should collect");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let actions = bundle
            .facts
            .iter()
            .filter_map(|fact| fact.capability.as_ref()?.action.as_deref())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(actions.contains("s3:PutObject"));
        assert!(actions.contains("s3:DeleteObject"));
        assert!(bundle.facts.iter().all(|fact| {
            fact.value == FactValue::Unknown
                && fact
                    .unknown_reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("principal"))
        }));
    }

    #[test]
    fn conditional_iam_allow_is_not_reported_as_effective_grant() {
        let block = TerraformResourceBlock {
            file: "main.tf".to_owned(),
            resource_type: "aws_iam_role_policy".to_owned(),
            name: "reader".to_owned(),
            body: "role = \"prod-reader\"".to_owned(),
            line: 1,
        };
        let policy = serde_json::json!({
            "Statement": {
                "Effect": "Allow",
                "Action": "s3:GetObject",
                "Resource": "*",
                "Condition": {"IpAddress": {"aws:SourceIp": "10.0.0.0/8"}}
            }
        });

        let facts = policy_grant_facts(&block, &policy);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].value, FactValue::Unknown);
        assert_eq!(facts[0].confidence.level, ConfidenceLevel::Unknown);
        assert!(
            facts[0]
                .capability
                .as_ref()
                .unwrap()
                .constraints
                .contains_key("iam.condition")
        );
    }

    #[test]
    fn explicit_iam_deny_is_preserved() {
        let block = TerraformResourceBlock {
            file: "main.tf".to_owned(),
            resource_type: "aws_iam_role_policy".to_owned(),
            name: "reader".to_owned(),
            body: "role = \"prod-reader\"".to_owned(),
            line: 1,
        };
        let policy = serde_json::json!({
            "Statement": {"Effect": "Deny", "Action": "s3:GetObject", "Resource": "*"}
        });

        let facts = policy_grant_facts(&block, &policy);
        assert_eq!(facts[0].role, Some(FactRole::Denied));
        assert_eq!(facts[0].value, FactValue::True);
    }

    #[test]
    fn standalone_iam_policy_is_not_proof_of_attachment() {
        let block = TerraformResourceBlock {
            file: "main.tf".to_owned(),
            resource_type: "aws_iam_policy".to_owned(),
            name: "reader".to_owned(),
            body: String::new(),
            line: 1,
        };
        let policy = serde_json::json!({
            "Statement": {"Effect": "Allow", "Action": "s3:GetObject", "Resource": "*"}
        });

        let facts = policy_grant_facts(&block, &policy);
        assert_eq!(facts[0].value, FactValue::Unknown);
        assert!(
            facts[0]
                .unknown_reason
                .as_deref()
                .unwrap()
                .contains("attachment")
        );
    }

    #[test]
    fn terraform_state_walks_child_modules() {
        let plan = serde_json::json!({
            "values": {"root_module": {"child_modules": [{"resources": [{
                "type": "aws_iam_role_policy",
                "name": "reader",
                "address": "module.app.aws_iam_role_policy.reader",
                "values": {
                    "role": "prod-reader",
                    "policy": serde_json::json!({"Statement": {
                        "Effect": "Allow", "Action": "s3:GetObject", "Resource": "*"
                    }}).to_string()
                }
            }]}]}}
        });

        let bundle = terraform_plan_json_to_bundle(&plan.to_string()).unwrap();
        assert_eq!(bundle.facts.len(), 1);
        assert_eq!(bundle.facts[0].value, FactValue::True);
        assert_eq!(
            bundle.facts[0].acquisition_mode,
            AcquisitionMode::TerraformPlan
        );
        assert_eq!(
            bundle.facts[0].evidence[0].principal.as_deref(),
            Some("prod-reader")
        );
    }

    #[test]
    fn terraform_plan_rejects_input_over_byte_budget_before_parsing() {
        let limits = TerraformPlanLimits {
            max_input_bytes: 8,
            ..TerraformPlanLimits::default()
        };
        let error = terraform_plan_json_to_bundle_with_limits(r#"{"resource_changes":[]}"#, limits)
            .expect_err("oversized plan must be rejected");
        assert!(error.contains("byte limit"));
    }

    #[test]
    fn terraform_plan_rejects_excessive_json_depth_and_nodes() {
        let depth_limits = TerraformPlanLimits {
            max_json_depth: 3,
            ..TerraformPlanLimits::default()
        };
        let error = terraform_plan_json_to_bundle_with_limits(
            r#"{"values":{"root_module":{"child_modules":[]}}}"#,
            depth_limits,
        )
        .expect_err("deep JSON must be rejected");
        assert!(error.contains("depth limit"));

        let node_limits = TerraformPlanLimits {
            max_json_nodes: 3,
            ..TerraformPlanLimits::default()
        };
        let error = terraform_plan_json_to_bundle_with_limits(
            r#"{"resource_changes":[null,null,null]}"#,
            node_limits,
        )
        .expect_err("node-heavy JSON must be rejected");
        assert!(error.contains("node limit"));
    }

    #[test]
    fn terraform_plan_rejects_resource_and_fact_budget_overflow() {
        let plan = serde_json::json!({
            "resource_changes": [{
                "type": "postgresql_grant",
                "name": "reader",
                "address": "postgresql_grant.reader",
                "change": {"after": {
                    "database": "app",
                    "schema": "public",
                    "role": "reader",
                    "objects": ["first", "second"],
                    "privileges": ["SELECT"]
                }}
            }]
        })
        .to_string();

        let resource_error = terraform_plan_json_to_bundle_with_limits(
            &plan,
            TerraformPlanLimits {
                max_resources: 0,
                ..TerraformPlanLimits::default()
            },
        )
        .expect_err("resource budget must be enforced");
        assert!(resource_error.contains("resource limit"));

        let fact_error = terraform_plan_json_to_bundle_with_limits(
            &plan,
            TerraformPlanLimits {
                max_facts: 1,
                ..TerraformPlanLimits::default()
            },
        )
        .expect_err("fact budget must be enforced");
        assert!(fact_error.contains("fact limit"));
    }

    #[test]
    fn terraform_dir_to_bundle_reads_postgresql_grants() {
        let temp_dir = std::env::temp_dir().join(format!(
            "reir-terraform-postgresql-grant-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        std::fs::write(
            temp_dir.join("postgres.tf"),
            r#"resource "postgresql_grant" "report_writer_audit_events" {
  database    = "reports"
  role        = "report_writer"
  schema      = "public"
  object_type = "table"
  objects     = ["audit_events"]
  privileges  = ["SELECT", "INSERT"]
}
"#,
        )
        .expect("Terraform fixture should be written");

        let bundle = terraform_dir_to_bundle(&temp_dir).expect("Terraform should collect");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let insert = bundle
            .facts
            .iter()
            .find(|fact| {
                fact.capability
                    .as_ref()
                    .is_some_and(|capability| capability.action.as_deref() == Some("INSERT"))
            })
            .expect("INSERT privilege should be present");
        let capability = insert
            .capability
            .as_ref()
            .expect("INSERT fact should carry a capability");
        assert_eq!(capability.category, CapabilityCategory::DatabaseWrite);
        assert_eq!(capability.provider.as_deref(), Some("postgres"));
        assert_eq!(capability.service.as_deref(), Some("postgres"));
        assert_eq!(
            capability.resource.as_deref(),
            Some("postgres://reports/public/audit_events")
        );

        let select = bundle.facts.iter().find(|fact| {
            fact.capability
                .as_ref()
                .is_some_and(|capability| capability.action.as_deref() == Some("SELECT"))
        });
        assert!(select.is_some(), "SELECT privilege should be present");
        assert_eq!(
            select.unwrap().capability.as_ref().unwrap().category,
            CapabilityCategory::DatabaseRead
        );
    }

    #[test]
    fn terraform_dir_to_bundle_reads_multiline_postgresql_grant_arrays() {
        let temp_dir = std::env::temp_dir().join(format!(
            "reir-terraform-postgresql-multiline-grant-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        std::fs::write(
            temp_dir.join("postgres.tf"),
            r#"resource "postgresql_grant" "report_writer_audit_events" {
  database    = "reports"
  role        = "report_writer"
  schema      = "public"
  object_type = "table"
  objects = [
    "audit_events",
  ]
  privileges = [
    "SELECT",
    "INSERT",
  ]
}
"#,
        )
        .expect("Terraform fixture should be written");

        let bundle = terraform_dir_to_bundle(&temp_dir).expect("Terraform should collect");
        let _ = std::fs::remove_dir_all(&temp_dir);

        let actions = bundle
            .facts
            .iter()
            .filter_map(|fact| fact.capability.as_ref()?.action.as_deref())
            .collect::<std::collections::BTreeSet<_>>();
        assert!(actions.contains("SELECT"));
        assert!(actions.contains("INSERT"));
    }

    #[cfg(unix)]
    #[test]
    fn terraform_source_traversal_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temp_dir = std::env::temp_dir().join(format!(
            "reir-terraform-symlink-test-{}",
            std::process::id()
        ));
        let outside = temp_dir.with_extension("outside");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("escaped.tf"), "resource \"x\" \"y\" {}").unwrap();
        symlink(&outside, temp_dir.join("linked")).unwrap();

        let error = terraform_dir_to_bundle(&temp_dir).expect_err("symlink must be rejected");

        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::remove_dir_all(&outside);
        assert!(error.contains("refusing to follow symlink"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn terraform_source_traversal_rejects_symlink_loops() {
        use std::os::unix::fs::symlink;

        let temp_dir = std::env::temp_dir().join(format!(
            "reir-terraform-symlink-loop-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(temp_dir.join("nested")).unwrap();
        symlink(&temp_dir, temp_dir.join("nested/loop")).unwrap();

        let error = terraform_dir_to_bundle(&temp_dir).expect_err("symlink loop must be rejected");

        let _ = std::fs::remove_dir_all(&temp_dir);
        assert!(error.contains("refusing to follow symlink"), "{error}");
    }

    #[test]
    fn terraform_source_traversal_enforces_file_and_byte_budgets() {
        let temp_dir =
            std::env::temp_dir().join(format!("reir-terraform-budget-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();
        std::fs::write(temp_dir.join("a.tf"), "aaaa").unwrap();
        std::fs::write(temp_dir.join("b.tf"), "bbbb").unwrap();

        let file_error = terraform_dir_to_bundle_with_limits(
            &temp_dir,
            TerraformSourceLimits {
                max_files: 1,
                max_depth: 4,
                max_file_bytes: 16,
                max_total_bytes: 32,
            },
        )
        .expect_err("file count must be bounded");
        let byte_error = terraform_dir_to_bundle_with_limits(
            &temp_dir,
            TerraformSourceLimits {
                max_files: 4,
                max_depth: 4,
                max_file_bytes: 3,
                max_total_bytes: 32,
            },
        )
        .expect_err("individual file bytes must be bounded");
        let total_error = terraform_dir_to_bundle_with_limits(
            &temp_dir,
            TerraformSourceLimits {
                max_files: 4,
                max_depth: 4,
                max_file_bytes: 16,
                max_total_bytes: 7,
            },
        )
        .expect_err("aggregate bytes must be bounded");

        let _ = std::fs::remove_dir_all(&temp_dir);
        assert!(file_error.contains("file limit"), "{file_error}");
        assert!(byte_error.contains("exceeding"), "{byte_error}");
        assert!(total_error.contains("byte limit"), "{total_error}");
    }

    #[test]
    fn terraform_source_traversal_enforces_depth_budget() {
        let temp_dir =
            std::env::temp_dir().join(format!("reir-terraform-depth-test-{}", std::process::id()));
        let nested = temp_dir.join("nested");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("main.tf"), "").unwrap();

        let error = terraform_dir_to_bundle_with_limits(
            &temp_dir,
            TerraformSourceLimits {
                max_files: 4,
                max_depth: 0,
                max_file_bytes: 16,
                max_total_bytes: 32,
            },
        )
        .expect_err("depth must be bounded");

        let _ = std::fs::remove_dir_all(&temp_dir);
        assert!(error.contains("maximum depth"), "{error}");
    }

    #[test]
    fn terraform_plan_surfaces_embedded_policy_parse_failure_as_diagnostic() {
        let bundle = terraform_plan_json_to_bundle(
            r#"{
                "resource_changes": [{
                    "address": "aws_iam_role_policy.invalid",
                    "type": "aws_iam_role_policy",
                    "name": "invalid",
                    "change": {
                        "after": {
                            "role": "role.prod",
                            "policy": "{not-json"
                        }
                    }
                }]
            }"#,
        )
        .expect("outer plan JSON should parse");

        let diagnostic = bundle
            .facts
            .iter()
            .find(|fact| fact.kind == FactKind::Diagnostic)
            .expect("embedded policy failure should be retained");
        assert_eq!(diagnostic.value, FactValue::Unknown);
        assert!(diagnostic.unknown_reason.is_some());
        assert_eq!(
            diagnostic.evidence[0].kind,
            EvidenceKind::TerraformPlanPointer
        );
        assert_eq!(
            diagnostic.evidence[0].json_pointer.as_deref(),
            Some("/resource_changes/0/change/after/policy")
        );
        let decision =
            crate::decide_validated_gate(&[], &bundle.facts, &[], crate::GatePolicy::production());
        assert_eq!(decision.status, crate::GateStatus::Fail);
        assert!(decision.blockers.iter().any(|blocker| {
            blocker.fact_id.as_deref() == Some(diagnostic.id.as_str())
                && blocker.kind == crate::GateIssueKind::InvalidEvidence
        }));
    }

    #[test]
    fn unsupported_plan_resource_is_explicit_unknown_coverage() {
        let bundle = terraform_plan_json_to_bundle(
            r#"{
                "resource_changes": [{
                    "address": "aws_lambda_function.worker",
                    "type": "aws_lambda_function",
                    "name": "worker",
                    "change": { "after": {} }
                }]
            }"#,
        )
        .expect("plan should parse");

        let diagnostic = bundle
            .facts
            .iter()
            .find(|fact| fact.id.contains("unsupported"))
            .expect("unsupported resource must be retained");
        assert_eq!(
            bundle.producers[0].adapter.as_deref(),
            Some("reir.adapters.terraform_plan")
        );
        assert_eq!(
            bundle.producers[0].source.as_deref(),
            Some("terraform_plan_json")
        );
        assert_eq!(diagnostic.kind, FactKind::Diagnostic);
        assert_eq!(diagnostic.value, FactValue::Unknown);
        assert!(
            diagnostic
                .unknown_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("coverage is unknown"))
        );
        assert_eq!(
            diagnostic.evidence[0].value.as_deref(),
            Some("unsupported_resource_type")
        );

        let decision =
            crate::decide_validated_gate(&[], &bundle.facts, &[], crate::GatePolicy::production());
        assert_eq!(decision.status, crate::GateStatus::Fail);
    }

    #[test]
    fn unsupported_source_resource_is_explicit_unknown_coverage() {
        let temp_dir = std::env::temp_dir().join(format!(
            "reir-terraform-unsupported-source-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).expect("temp dir");
        std::fs::write(
            temp_dir.join("main.tf"),
            r#"resource "aws_lambda_function" "worker" {
  function_name = "worker"
}
"#,
        )
        .expect("fixture");

        let bundle = terraform_dir_to_bundle(&temp_dir).expect("source should scan");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let diagnostic = bundle
            .facts
            .iter()
            .find(|fact| fact.id.contains("unsupported"))
            .expect("unsupported source resource must be retained");
        assert_eq!(diagnostic.kind, FactKind::Diagnostic);
        assert_eq!(diagnostic.value, FactValue::Unknown);
        assert_eq!(
            diagnostic.evidence[0].kind,
            EvidenceKind::SourceTemplatePointer
        );
    }

    #[test]
    fn unsupported_state_resource_is_explicit_unknown_coverage() {
        let bundle = terraform_plan_json_to_bundle(
            r#"{
                "values": {
                    "root_module": {
                        "resources": [{
                            "address": "google_storage_bucket.assets",
                            "type": "google_storage_bucket",
                            "name": "assets",
                            "values": {}
                        }]
                    }
                }
            }"#,
        )
        .expect("state should parse");

        let diagnostic = bundle
            .facts
            .iter()
            .find(|fact| fact.id.contains("unsupported"))
            .expect("unsupported state resource must be retained");
        assert_eq!(diagnostic.value, FactValue::Unknown);
        assert_eq!(diagnostic.acquisition_mode, AcquisitionMode::TerraformState);
        assert_eq!(
            diagnostic.evidence[0].kind,
            EvidenceKind::TerraformStatePointer
        );
    }

    #[test]
    fn terraform_state_surfaces_embedded_policy_parse_failure_as_diagnostic() {
        let bundle = terraform_plan_json_to_bundle(
            r#"{
                "values": {
                    "root_module": {
                        "resources": [{
                            "address": "aws_s3_bucket_policy.invalid",
                            "type": "aws_s3_bucket_policy",
                            "name": "invalid",
                            "values": { "policy": "[" }
                        }]
                    }
                }
            }"#,
        )
        .expect("outer state JSON should parse");

        let diagnostic = bundle
            .facts
            .iter()
            .find(|fact| fact.kind == FactKind::Diagnostic)
            .expect("embedded policy failure should be retained");
        assert_eq!(diagnostic.acquisition_mode, AcquisitionMode::TerraformState);
        assert_eq!(
            diagnostic.evidence[0].kind,
            EvidenceKind::TerraformStatePointer
        );
        assert!(diagnostic.unknown_reason.is_some());
    }
}
