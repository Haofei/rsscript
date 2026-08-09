// Operation, resource, JSON-shape, and evidence budgets for fail-closed conversion.

struct TerraformPlanBudget {
    json_nodes: usize,
    resources: usize,
    evidence: BoundedEvidenceBuilder,
}

impl TerraformPlanBudget {
    fn new(limits: TerraformPlanLimits) -> Self {
        let max_operations = limits
            .max_json_nodes
            .saturating_add(limits.max_resources)
            .saturating_add(limits.max_facts);
        Self {
            json_nodes: 0,
            resources: 0,
            evidence: BoundedEvidenceBuilder::new(AdapterLimits::new(
                max_operations,
                limits.max_facts,
                limits.max_input_bytes.saturating_mul(8).max(1024 * 1024),
            )),
        }
    }
}

#[derive(Default)]
struct TerraformSourceBudget {
    files: usize,
    bytes: u64,
}

fn account_json_value(
    root: &Value,
    limits: TerraformPlanLimits,
    budget: &mut TerraformPlanBudget,
) -> Result<(), String> {
    let mut stack = vec![(root, 1_usize)];
    while let Some((value, depth)) = stack.pop() {
        if depth > limits.max_json_depth {
            return Err(format!(
                "Terraform JSON exceeds the {} level depth limit",
                limits.max_json_depth
            ));
        }
        budget.json_nodes = budget
            .json_nodes
            .checked_add(1)
            .ok_or_else(|| "Terraform JSON node count overflow".to_owned())?;
        if budget.json_nodes > limits.max_json_nodes {
            return Err(format!(
                "Terraform JSON exceeds the {} node limit",
                limits.max_json_nodes
            ));
        }
        budget
            .evidence
            .record_operation()
            .map_err(|error| format!("Terraform JSON conversion failed: {error}"))?;
        match value {
            Value::Array(values) => {
                stack.extend(values.iter().map(|value| (value, depth + 1)));
            }
            Value::Object(values) => {
                stack.extend(values.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(())
}

fn account_terraform_resource(
    limits: TerraformPlanLimits,
    budget: &mut TerraformPlanBudget,
) -> Result<(), String> {
    budget.resources = budget
        .resources
        .checked_add(1)
        .ok_or_else(|| "Terraform resource count overflow".to_owned())?;
    if budget.resources > limits.max_resources {
        return Err(format!(
            "Terraform JSON exceeds the {} resource limit",
            limits.max_resources
        ));
    }
    budget
        .evidence
        .record_operation()
        .map_err(|error| format!("Terraform JSON conversion failed: {error}"))?;
    Ok(())
}

fn append_terraform_facts(
    facts: Vec<Fact>,
    _limits: TerraformPlanLimits,
    budget: &mut TerraformPlanBudget,
) -> Result<(), String> {
    budget
        .evidence
        .extend_facts(facts)
        .map_err(|error| format!("Terraform plan conversion failed: {error}"))
}

fn ensure_fact_capacity(
    additional: usize,
    limits: TerraformPlanLimits,
    budget: &TerraformPlanBudget,
) -> Result<(), String> {
    budget
        .evidence
        .ensure_fact_capacity(additional)
        .map_err(|error| {
            format!(
                "Terraform plan conversion exceeds the {} fact limit: {error}",
                limits.max_facts
            )
        })
}

fn iam_policy_fact_upper_bound(policy: &Value) -> usize {
    statements(policy)
        .into_iter()
        .fold(0_usize, |total, statement| {
            let actions = json_value_string_count(statement.get("Action"));
            let resources = json_value_string_count(statement.get("Resource"));
            total.saturating_add(actions.saturating_mul(resources))
        })
}

fn postgresql_grant_fact_upper_bound(values: &Value) -> usize {
    let objects = values
        .get("objects")
        .and_then(Value::as_array)
        .map_or(1, |items| items.len().max(1));
    let privileges = values
        .get("privileges")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    objects.saturating_mul(privileges)
}

fn json_value_string_count(value: Option<&Value>) -> usize {
    match value {
        Some(Value::String(_)) => 1,
        Some(Value::Array(values)) => values.iter().filter(|value| value.is_string()).count(),
        _ => 0,
    }
}
