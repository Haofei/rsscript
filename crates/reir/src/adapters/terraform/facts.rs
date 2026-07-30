// Candidate capability fact construction for supported Terraform resources.

fn policy_grant_facts(block: &TerraformResourceBlock, policy: &Value) -> Vec<Fact> {
    let subject = Subject {
        kind: SubjectKind::CloudPolicy,
        id: format!("terraform::{}.{}", block.resource_type, block.name),
        name: Some(format!("{}.{}", block.resource_type, block.name)),
        package: Some("terraform".to_owned()),
    };
    let mut facts = Vec::new();
    for (statement_index, statement) in statements(policy).into_iter().enumerate() {
        let effect = statement
            .get("Effect")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !effect.eq_ignore_ascii_case("Allow") && !effect.eq_ignore_ascii_case("Deny") {
            continue;
        }
        let principal = policy_principal(block, statement);
        let condition = statement.get("Condition");
        let attached = block.resource_type != "aws_iam_policy" && principal.is_some();
        let conclusive = effect.eq_ignore_ascii_case("Deny") || (attached && condition.is_none());
        let actions = string_or_array(statement.get("Action"));
        let resources = string_or_array(statement.get("Resource"));
        for action in actions {
            for resource in &resources {
                facts.push(capability_grant_fact(
                    block,
                    subject.clone(),
                    statement_index,
                    &action,
                    resource,
                    effect,
                    principal.as_deref(),
                    condition,
                    conclusive,
                ));
            }
        }
    }
    facts
}

fn capability_grant_fact(
    block: &TerraformResourceBlock,
    subject: Subject,
    statement_index: usize,
    action: &str,
    resource: &str,
    effect: &str,
    principal: Option<&str>,
    condition: Option<&Value>,
    conclusive: bool,
) -> Fact {
    let denied = effect.eq_ignore_ascii_case("Deny");
    let mut constraints = HashMap::new();
    if let Some(condition) = condition {
        constraints.insert("iam.condition".to_owned(), condition.to_string());
    }
    Fact {
        schema: FACT_SCHEMA.to_owned(),
        id: format!(
            "fact.terraform.{}.{}.statement_{}.{}.{}",
            sanitize_id(&block.resource_type),
            sanitize_id(&block.name),
            statement_index,
            sanitize_id(action),
            sanitize_id(resource)
        ),
        kind: FactKind::Capability,
        role: Some(if denied {
            FactRole::Denied
        } else {
            FactRole::Granted
        }),
        subject,
        capability: Some(Capability {
            category: capability_category_for_action(action),
            provider: Some("aws".to_owned()),
            service: Some(service_for_action(action).to_owned()),
            action: Some(action.to_owned()),
            resource: Some(resource.to_owned()),
            constraints,
        }),
        value: if conclusive {
            FactValue::True
        } else {
            FactValue::Unknown
        },
        confidence: Confidence {
            level: if conclusive {
                ConfidenceLevel::Scanned
            } else {
                ConfidenceLevel::Unknown
            },
            source: Some(PRODUCER_SOURCE.to_owned()),
        },
        acquisition_mode: AcquisitionMode::TerraformPlan,
        precision: Precision::ResourceScoped,
        evidence: vec![Evidence {
            kind: EvidenceKind::TerraformPlanPointer,
            file: Some(block.file.clone()),
            line: Some(block.line),
            column: None,
            length: None,
            symbol: Some(format!("{}.{}", block.resource_type, block.name)),
            reason: Some(format!(
                "Terraform/OpenTofu {}.{} {effect} statement for {action} on {resource}",
                block.resource_type, block.name
            )),
            json_pointer: Some(format!("/Statement/{statement_index}")),
            resource: Some(resource.to_owned()),
            provider: Some("aws".to_owned()),
            value: None,
            event_id: None,
            time: None,
            source: Some(PRODUCER_SOURCE.to_owned()),
            event_name: None,
            principal: principal.map(str::to_owned),
            account: None,
            policy_arn: None,
            statement_index: Some(statement_index),
            action: Some(action.to_owned()),
        }],
        unknown_reason: (!conclusive).then(|| {
            if condition.is_some() {
                "conditional IAM policy requires effective-permission evaluation".to_owned()
            } else if block.resource_type == "aws_iam_policy" {
                "standalone IAM policy is not proof of attachment to a principal".to_owned()
            } else {
                "IAM policy principal could not be resolved".to_owned()
            }
        }),
    }
}

fn postgresql_grant_facts(block: &TerraformResourceBlock) -> Vec<Fact> {
    let database = hcl_string_attr(&block.body, "database").unwrap_or_default();
    let schema = hcl_string_attr(&block.body, "schema").unwrap_or_else(|| "public".to_owned());
    let role = hcl_string_attr(&block.body, "role").unwrap_or_default();
    let mut objects = hcl_string_array_attr(&block.body, "objects");
    if objects.is_empty() {
        objects.push("*".to_owned());
    }
    let privileges = hcl_string_array_attr(&block.body, "privileges");

    let subject = Subject {
        kind: SubjectKind::CloudPolicy,
        id: format!("terraform::{}.{}", block.resource_type, block.name),
        name: Some(format!("{}.{}", block.resource_type, block.name)),
        package: Some("terraform".to_owned()),
    };

    let mut facts = Vec::new();
    for (privilege_index, privilege) in privileges.iter().enumerate() {
        let normalized = privilege.to_ascii_uppercase();
        let category = postgres_privilege_category(&normalized);
        for object in &objects {
            let resource = format!("postgres://{database}/{schema}/{object}");
            facts.push(Fact {
                schema: FACT_SCHEMA.to_owned(),
                id: format!(
                    "fact.terraform.{}.{}.privilege_{}.{}.{}",
                    sanitize_id(&block.resource_type),
                    sanitize_id(&block.name),
                    privilege_index,
                    sanitize_id(&normalized),
                    sanitize_id(&resource)
                ),
                kind: FactKind::Capability,
                role: Some(FactRole::Granted),
                subject: subject.clone(),
                capability: Some(Capability {
                    category: category.clone(),
                    provider: Some("postgres".to_owned()),
                    service: Some("postgres".to_owned()),
                    action: Some(normalized.clone()),
                    resource: Some(resource.clone()),
                    constraints: HashMap::new(),
                }),
                value: FactValue::True,
                confidence: Confidence {
                    level: ConfidenceLevel::Scanned,
                    source: Some(PRODUCER_SOURCE.to_owned()),
                },
                acquisition_mode: AcquisitionMode::TerraformPlan,
                precision: Precision::ResourceScoped,
                evidence: vec![Evidence {
                    kind: EvidenceKind::TerraformPlanPointer,
                    file: Some(block.file.clone()),
                    line: Some(block.line),
                    column: None,
                    length: None,
                    symbol: Some(format!("{}.{}", block.resource_type, block.name)),
                    reason: Some(format!(
                        "Terraform/OpenTofu {}.{} grants {normalized} on {resource} to role {role}",
                        block.resource_type, block.name
                    )),
                    json_pointer: Some(format!("/privileges/{privilege_index}")),
                    resource: Some(resource.clone()),
                    provider: Some("postgres".to_owned()),
                    value: None,
                    event_id: None,
                    time: None,
                    source: Some(PRODUCER_SOURCE.to_owned()),
                    event_name: None,
                    principal: if role.is_empty() {
                        None
                    } else {
                        Some(role.clone())
                    },
                    account: None,
                    policy_arn: None,
                    statement_index: Some(privilege_index),
                    action: Some(normalized.clone()),
                }],
                unknown_reason: None,
            });
        }
    }
    facts
}

fn postgresql_grant_plan_facts(
    name: &str,
    address: &str,
    values: &Value,
    change_index: usize,
) -> Vec<Fact> {
    let database = values
        .get("database")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let schema = values
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or("public");
    let role = values
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let objects = values
        .get("objects")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .filter(|items| !items.is_empty())
        .unwrap_or_else(|| vec!["*"]);
    let privileges = values
        .get("privileges")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let subject = Subject {
        kind: SubjectKind::CloudPolicy,
        id: format!("terraform::postgresql_grant.{name}"),
        name: Some(format!("postgresql_grant.{name}")),
        package: Some("terraform".to_owned()),
    };

    privileges
        .iter()
        .enumerate()
        .flat_map(|(privilege_index, privilege)| {
            let normalized = privilege.to_ascii_uppercase();
            let subject = subject.clone();
            objects.iter().map(move |object| {
                let resource = format!("postgres://{database}/{schema}/{object}");
                Fact {
                    schema: FACT_SCHEMA.to_owned(),
                    id: format!(
                        "fact.terraform.postgresql_grant.{}.privilege_{}.{}.{}",
                        sanitize_id(name),
                        privilege_index,
                        sanitize_id(&normalized),
                        sanitize_id(&resource)
                    ),
                    kind: FactKind::Capability,
                    role: Some(FactRole::Granted),
                    subject: subject.clone(),
                    capability: Some(Capability {
                        category: postgres_privilege_category(&normalized),
                        provider: Some("postgres".to_owned()),
                        service: Some("postgres".to_owned()),
                        action: Some(normalized.clone()),
                        resource: Some(resource.clone()),
                        constraints: HashMap::new(),
                    }),
                    value: FactValue::True,
                    confidence: Confidence {
                        level: ConfidenceLevel::Scanned,
                        source: Some("terraform_plan_json".to_owned()),
                    },
                    acquisition_mode: AcquisitionMode::TerraformPlan,
                    precision: Precision::ResourceScoped,
                    evidence: vec![Evidence {
                        kind: EvidenceKind::TerraformPlanPointer,
                        file: Some("terraform-plan".to_owned()),
                        line: Some(0),
                        column: None,
                        length: None,
                        symbol: Some(address.to_owned()),
                        reason: Some(format!(
                            "Terraform/OpenTofu plan grants {normalized} on {resource} to role {role}"
                        )),
                        json_pointer: Some(format!(
                            "/resource_changes/{change_index}/change/after/privileges/{privilege_index}"
                        )),
                        resource: Some(resource.clone()),
                        provider: Some("postgres".to_owned()),
                        value: None,
                        event_id: None,
                        time: None,
                        source: Some("terraform_plan_json".to_owned()),
                        event_name: None,
                        principal: (!role.is_empty()).then(|| role.to_owned()),
                        account: None,
                        policy_arn: None,
                        statement_index: Some(privilege_index),
                        action: Some(normalized.clone()),
                    }],
                    unknown_reason: None,
                }
            })
        })
        .collect()
}

fn policy_grant_facts_with_address(
    block: &TerraformResourceBlock,
    policy: &Value,
    address: &str,
) -> Vec<Fact> {
    let mut facts = Vec::new();
    let statement_values = statements(policy);
    for (statement_index, statement_value) in statement_values.iter().enumerate() {
        let effect = statement_value
            .get("Effect")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !effect.eq_ignore_ascii_case("Allow") && !effect.eq_ignore_ascii_case("Deny") {
            continue;
        }
        let principal = policy_principal(block, statement_value);
        let condition = statement_value.get("Condition");
        let attached = block.resource_type != "aws_iam_policy" && principal.is_some();
        let conclusive = effect.eq_ignore_ascii_case("Deny") || (attached && condition.is_none());
        let actions = json_string_or_array(statement_value, "Action");
        let resources = json_string_or_array(statement_value, "Resource");

        for action in &actions {
            for resource in &resources {
                let subject_id = format!("terraform::{}.{}", block.resource_type, block.name);
                let subject = Subject {
                    kind: SubjectKind::CloudPolicy,
                    id: subject_id.clone(),
                    name: Some(format!("{}.{}", block.resource_type, block.name)),
                    package: Some("terraform".to_owned()),
                };
                let mut fact = capability_grant_fact(
                    block,
                    subject,
                    statement_index,
                    action,
                    resource,
                    effect,
                    principal.as_deref(),
                    condition,
                    conclusive,
                );
                fact.id = format!(
                    "{}::{}::statement::{}::{}",
                    subject_id,
                    statement_index,
                    sanitize_id(action),
                    sanitize_id(resource)
                );
                fact.acquisition_mode = AcquisitionMode::TerraformPlan;
                fact.confidence.source = Some("terraform_plan_json".to_owned());
                if let Some(evidence) = fact.evidence.first_mut() {
                    evidence.kind = EvidenceKind::Extension("terraform_plan_resource".to_owned());
                    evidence.symbol = Some(address.to_owned());
                    evidence.source = Some("terraform_plan_json".to_owned());
                }
                facts.push(fact);
            }
        }
    }
    facts
}

fn principal_body(resource_type: &str, values: &Value) -> String {
    let key = match resource_type {
        "aws_iam_role_policy" => "role",
        "aws_iam_user_policy" => "user",
        "aws_iam_group_policy" => "group",
        _ => return String::new(),
    };
    values
        .get(key)
        .and_then(Value::as_str)
        .map(|principal| format!("{key} = \"{principal}\""))
        .unwrap_or_default()
}
