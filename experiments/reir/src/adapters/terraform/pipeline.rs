// Public source/plan conversion entry points and bounded bundle finalization.

pub fn terraform_dir_to_bundle(root: &Path) -> Result<Bundle, String> {
    terraform_dir_to_bundle_with_limits(root, TerraformSourceLimits::default())
}

pub fn terraform_dir_to_bundle_with_limits(
    root: &Path,
    limits: TerraformSourceLimits,
) -> Result<Bundle, String> {
    let root = canonical_terraform_root(root)?;
    let mut files = Vec::new();
    let mut budget = TerraformSourceBudget::default();
    let mut visited = HashSet::new();
    collect_tf_files(
        &root,
        &root,
        0,
        limits,
        &mut budget,
        &mut visited,
        &mut files,
    )?;
    files.sort();

    let mut facts = Vec::new();
    let mut actual_bytes = 0_u64;
    for file in files {
        let text = read_tf_file(&root, &file, limits.max_file_bytes)?;
        actual_bytes = actual_bytes
            .checked_add(text.len() as u64)
            .ok_or_else(|| "Terraform source byte count overflow".to_owned())?;
        if actual_bytes > limits.max_total_bytes {
            return Err(format!(
                "Terraform source traversal exceeded the {} byte limit while reading",
                limits.max_total_bytes
            ));
        }
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        for block in terraform_resource_blocks(&relative, &text) {
            match block.resource_type.as_str() {
                "aws_iam_role_policy" | "aws_s3_bucket_policy" => {
                    for policy_json in terraform_policy_jsons(&block.body) {
                        let policy: Value =
                            serde_json::from_str(&policy_json).map_err(|error| {
                                format!(
                                    "failed to parse IAM policy JSON in {} resource {}.{}: {error}",
                                    block.file, block.resource_type, block.name
                                )
                            })?;
                        facts.extend(policy_grant_facts(&block, &policy));
                    }
                }
                "postgresql_grant" => {
                    facts.extend(postgresql_grant_facts(&block));
                }
                _ => facts.push(unsupported_terraform_resource_fact(
                    "terraform-source",
                    AcquisitionMode::SourceScan,
                    EvidenceKind::SourceTemplatePointer,
                    &block.resource_type,
                    &block.name,
                    &format!("{}.{}", block.resource_type, block.name),
                    Some(format!("{}:{}", block.file, block.line)),
                )),
            }
        }
    }
    for fact in &mut facts {
        mark_source_scan_unverified(fact);
    }
    let max_operations = limits
        .max_files
        .saturating_add(facts.len())
        .saturating_add(1);
    let max_facts = (limits.max_total_bytes as usize).min(250_000);
    let mut evidence = BoundedEvidenceBuilder::new(AdapterLimits::new(
        max_operations,
        max_facts,
        (limits.max_total_bytes as usize)
            .saturating_mul(8)
            .max(1024 * 1024),
    ));
    evidence
        .record_operations(budget.files)
        .and_then(|()| evidence.extend_facts(facts))
        .map_err(|error| format!("Terraform source conversion failed: {error}"))?;
    evidence
        .finish_preserving_fact_subjects(terraform_source_provenance())
        .map_err(|error| format!("Terraform source conversion failed: {error}"))
}

pub fn terraform_plan_json_to_bundle(plan_json: &str) -> Result<Bundle, String> {
    terraform_plan_json_to_bundle_with_limits(plan_json, TerraformPlanLimits::default())
}

pub fn terraform_plan_json_to_bundle_with_limits(
    plan_json: &str,
    limits: TerraformPlanLimits,
) -> Result<Bundle, String> {
    if plan_json.len() > limits.max_input_bytes {
        return Err(format!(
            "Terraform plan JSON is {} bytes, exceeding the {} byte limit",
            plan_json.len(),
            limits.max_input_bytes
        ));
    }
    let plan: Value = serde_json::from_str(plan_json)
        .map_err(|e| format!("failed to parse terraform plan JSON: {e}"))?;
    let mut budget = TerraformPlanBudget::new(limits);
    account_json_value(&plan, limits, &mut budget)?;

    let mut diagnostics = Vec::new();
    let mut omitted_diagnostics = 0_usize;

    // Handle both `terraform show -json` (has .values.root_module.resources)
    // and `terraform plan -json` (has .resource_changes)
    if let Some(changes) = plan.get("resource_changes").and_then(|v| v.as_array()) {
        for (change_index, change) in changes.iter().enumerate() {
            account_terraform_resource(limits, &mut budget)?;
            let resource_type = change.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let name = change.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let address = change.get("address").and_then(|v| v.as_str()).unwrap_or("");
            let after = change
                .get("change")
                .and_then(|c| c.get("after"))
                .unwrap_or(&Value::Null);

            if resource_type == "postgresql_grant" {
                ensure_fact_capacity(postgresql_grant_fact_upper_bound(after), limits, &budget)?;
                append_terraform_facts(
                    postgresql_grant_plan_facts(name, address, after, change_index),
                    limits,
                    &mut budget,
                )?;
                continue;
            }

            if !matches!(
                resource_type,
                "aws_iam_role_policy"
                    | "aws_iam_policy"
                    | "aws_s3_bucket_policy"
                    | "aws_iam_user_policy"
                    | "aws_iam_group_policy"
            ) {
                append_terraform_facts(
                    vec![unsupported_terraform_resource_fact(
                        "terraform-plan",
                        AcquisitionMode::TerraformPlan,
                        EvidenceKind::TerraformPlanPointer,
                        resource_type,
                        name,
                        address,
                        Some(format!("/resource_changes/{change_index}")),
                    )],
                    limits,
                    &mut budget,
                )?;
                continue;
            }

            if let Some(policy_str) = after.get("policy").and_then(|v| v.as_str()) {
                match serde_json::from_str::<Value>(policy_str) {
                    Ok(policy) => {
                        account_json_value(&policy, limits, &mut budget)?;
                        ensure_fact_capacity(
                            iam_policy_fact_upper_bound(&policy),
                            limits,
                            &budget,
                        )?;
                        let block = TerraformResourceBlock {
                            file: "terraform-plan".to_owned(),
                            resource_type: resource_type.to_owned(),
                            name: name.to_owned(),
                            body: principal_body(resource_type, after),
                            line: 0,
                        };
                        append_terraform_facts(
                            policy_grant_facts_with_address(&block, &policy, address),
                            limits,
                            &mut budget,
                        )?;
                    }
                    Err(error) => push_terraform_parse_diagnostic(
                        &mut diagnostics,
                        &mut omitted_diagnostics,
                        TerraformParseDiagnostic {
                            source: "terraform-plan",
                            acquisition_mode: AcquisitionMode::TerraformPlan,
                            evidence_kind: EvidenceKind::TerraformPlanPointer,
                            resource_type,
                            name,
                            address,
                            json_pointer: Some(format!(
                                "/resource_changes/{change_index}/change/after/policy"
                            )),
                            error: &error,
                        },
                    ),
                }
            }
        }
    }

    // Also handle `terraform show -json` state format
    if let Some(root_module) = plan
        .get("values")
        .and_then(|values| values.get("root_module"))
    {
        let mut resources = Vec::new();
        collect_state_resources(root_module, &mut resources, limits, &mut budget)?;
        for resource in resources {
            let resource_type = resource.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let name = resource.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let address = resource
                .get("address")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if !matches!(
                resource_type,
                "aws_iam_role_policy"
                    | "aws_iam_policy"
                    | "aws_s3_bucket_policy"
                    | "aws_iam_user_policy"
                    | "aws_iam_group_policy"
            ) {
                append_terraform_facts(
                    vec![unsupported_terraform_resource_fact(
                        "terraform-state",
                        AcquisitionMode::TerraformState,
                        EvidenceKind::TerraformStatePointer,
                        resource_type,
                        name,
                        address,
                        Some("/values/root_module".to_owned()),
                    )],
                    limits,
                    &mut budget,
                )?;
                continue;
            }

            let values_obj = resource.get("values").unwrap_or(&Value::Null);
            if let Some(policy_str) = values_obj.get("policy").and_then(|v| v.as_str()) {
                match serde_json::from_str::<Value>(policy_str) {
                    Ok(policy) => {
                        account_json_value(&policy, limits, &mut budget)?;
                        ensure_fact_capacity(
                            iam_policy_fact_upper_bound(&policy),
                            limits,
                            &budget,
                        )?;
                        let block = TerraformResourceBlock {
                            file: "terraform-state".to_owned(),
                            resource_type: resource_type.to_owned(),
                            name: name.to_owned(),
                            body: principal_body(resource_type, values_obj),
                            line: 0,
                        };
                        append_terraform_facts(
                            policy_grant_facts_with_address(&block, &policy, address),
                            limits,
                            &mut budget,
                        )?;
                    }
                    Err(error) => push_terraform_parse_diagnostic(
                        &mut diagnostics,
                        &mut omitted_diagnostics,
                        TerraformParseDiagnostic {
                            source: "terraform-state",
                            acquisition_mode: AcquisitionMode::TerraformState,
                            evidence_kind: EvidenceKind::TerraformStatePointer,
                            resource_type,
                            name,
                            address,
                            json_pointer: Some("/values/root_module".to_owned()),
                            error: &error,
                        },
                    ),
                }
            }
        }
    }
    if omitted_diagnostics > 0 {
        diagnostics.push(terraform_diagnostic_budget_fact(omitted_diagnostics));
    }
    append_terraform_facts(diagnostics, limits, &mut budget)?;

    budget
        .evidence
        .finish_preserving_fact_subjects(terraform_plan_provenance())
        .map_err(|error| format!("Terraform plan conversion failed: {error}"))
}
