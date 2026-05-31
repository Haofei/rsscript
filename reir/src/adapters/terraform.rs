//! Terraform/OpenTofu IaC producer adapter for REIR.
//! Converts rendered `.tf` IAM policy resources into granted capability facts.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::*;

const FACT_SCHEMA: &str = "reir.fact.v0.1";
const PRODUCER_VERSION: &str = "0.1.0";
const ADAPTER_VERSION: &str = "0.1";
const PRODUCER_SOURCE: &str = "terraform_iac";

pub fn terraform_dir_to_bundle(root: &Path) -> Result<Bundle, String> {
    let mut files = Vec::new();
    collect_tf_files(root, &mut files)?;
    files.sort();

    let mut facts = Vec::new();
    for file in files {
        let text = fs::read_to_string(&file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let relative = file
            .strip_prefix(root)
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
                _ => {}
            }
        }
    }

    let mut bundle = Bundle::new();
    bundle.producers.push(crate::subject::Producer {
        name: "terraform".to_owned(),
        version: PRODUCER_VERSION.to_owned(),
        adapter: Some("reir.adapters.terraform".to_owned()),
        adapter_version: Some(ADAPTER_VERSION.to_owned()),
        source: Some(PRODUCER_SOURCE.to_owned()),
    });
    bundle.facts = facts;
    bundle.subjects = bundle
        .facts
        .iter()
        .map(|fact| fact.subject.clone())
        .collect();
    bundle.slices = crate::slice_by_kind(&bundle);
    Ok(bundle)
}

#[derive(Debug, Clone)]
struct TerraformResourceBlock {
    file: String,
    resource_type: String,
    name: String,
    body: String,
    line: usize,
}

fn collect_tf_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if root.is_file() {
        if root.extension().is_some_and(|extension| extension == "tf") {
            files.push(root.to_path_buf());
        }
        return Ok(());
    }
    let entries = fs::read_dir(root).map_err(|error| {
        format!(
            "failed to read Terraform directory {}: {error}",
            root.display()
        )
    })?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to read Terraform directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_tf_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "tf") {
            files.push(path);
        }
    }
    Ok(())
}

fn terraform_resource_blocks(file: &str, text: &str) -> Vec<TerraformResourceBlock> {
    let mut blocks = Vec::new();
    let mut offset = 0;
    while let Some(relative) = text[offset..].find("resource \"") {
        let start = offset + relative;
        let Some((resource_type, type_end)) = quoted_after(text, start + "resource ".len()) else {
            offset = start + "resource ".len();
            continue;
        };
        let Some((name, name_end)) = quoted_after(text, type_end) else {
            offset = type_end;
            continue;
        };
        let Some(open_relative) = text[name_end..].find('{') else {
            offset = name_end;
            continue;
        };
        let open = name_end + open_relative;
        let Some(close) = matching_brace(text, open) else {
            offset = open + 1;
            continue;
        };
        blocks.push(TerraformResourceBlock {
            file: file.to_owned(),
            resource_type,
            name,
            body: text[open + 1..close].to_owned(),
            line: line_for_offset(text, start),
        });
        offset = close + 1;
    }
    blocks
}

fn quoted_after(text: &str, start: usize) -> Option<(String, usize)> {
    let quote = text[start..].find('"')? + start;
    let end = text[quote + 1..].find('"')? + quote + 1;
    Some((text[quote + 1..end].to_owned(), end + 1))
}

fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[open..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn terraform_policy_jsons(block: &str) -> Vec<String> {
    let mut policies = Vec::new();
    let lines = block.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index].trim();
        if let Some(marker) = line.strip_prefix("policy").and_then(policy_heredoc_marker) {
            let mut json = String::new();
            index += 1;
            while index < lines.len() && lines[index].trim() != marker {
                json.push_str(lines[index]);
                json.push('\n');
                index += 1;
            }
            policies.push(json);
        } else if line.starts_with("policy") && line.contains("jsonencode(") {
            if let Some(json) = extract_jsonencode_object(&lines, index) {
                policies.push(json.0);
                index = json.1;
            }
        }
        index += 1;
    }
    policies
}

fn policy_heredoc_marker(line: &str) -> Option<&str> {
    let (_, value) = line.split_once('=')?;
    let value = value.trim();
    value
        .strip_prefix("<<-")
        .or_else(|| value.strip_prefix("<<"))
        .map(str::trim)
}

fn extract_jsonencode_object(lines: &[&str], start: usize) -> Option<(String, usize)> {
    let mut text = String::new();
    for (index, line) in lines.iter().enumerate().skip(start) {
        text.push_str(line);
        text.push('\n');
        if line.contains(')') {
            let begin = text.find("jsonencode(")? + "jsonencode(".len();
            let end = text.rfind(')')?;
            return Some((text[begin..end].trim().to_owned(), index));
        }
    }
    None
}

fn policy_grant_facts(block: &TerraformResourceBlock, policy: &Value) -> Vec<Fact> {
    let subject = Subject {
        kind: SubjectKind::CloudPolicy,
        id: format!("terraform::{}.{}", block.resource_type, block.name),
        name: Some(format!("{}.{}", block.resource_type, block.name)),
        package: Some("terraform".to_owned()),
    };
    let mut facts = Vec::new();
    for (statement_index, statement) in statements(policy).into_iter().enumerate() {
        if statement
            .get("Effect")
            .and_then(Value::as_str)
            .is_some_and(|effect| !effect.eq_ignore_ascii_case("Allow"))
        {
            continue;
        }
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
                ));
            }
        }
    }
    facts
}

fn statements(policy: &Value) -> Vec<&Value> {
    match policy.get("Statement") {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(statement) => vec![statement],
        None => Vec::new(),
    }
}

fn string_or_array(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(value)) => vec![value.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn capability_grant_fact(
    block: &TerraformResourceBlock,
    subject: Subject,
    statement_index: usize,
    action: &str,
    resource: &str,
) -> Fact {
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
        role: Some(FactRole::Granted),
        subject,
        capability: Some(Capability {
            category: capability_category_for_action(action),
            provider: Some("aws".to_owned()),
            service: Some(service_for_action(action).to_owned()),
            action: Some(action.to_owned()),
            resource: Some(resource.to_owned()),
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
                "Terraform/OpenTofu {}.{} grants {action} on {resource}",
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
            principal: None,
            account: None,
            policy_arn: None,
            statement_index: Some(statement_index),
            action: Some(action.to_owned()),
        }],
        unknown_reason: None,
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
                    principal: if role.is_empty() { None } else { Some(role.clone()) },
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

fn postgres_privilege_category(privilege: &str) -> CapabilityCategory {
    match privilege {
        "SELECT" | "REFERENCES" => CapabilityCategory::DatabaseRead,
        "INSERT" | "UPDATE" | "DELETE" | "TRUNCATE" => CapabilityCategory::DatabaseWrite,
        _ => CapabilityCategory::Extension("database".to_owned()),
    }
}

fn hcl_string_attr(body: &str, key: &str) -> Option<String> {
    for line in body.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim();
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_owned());
        }
    }
    None
}

fn hcl_string_array_attr(body: &str, key: &str) -> Vec<String> {
    for line in body.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(key) else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim();
        let Some(rest) = rest.strip_prefix('[') else {
            continue;
        };
        let Some(end) = rest.find(']') else {
            continue;
        };
        return rest[..end]
            .split(',')
            .map(|item| item.trim().trim_matches('"').to_owned())
            .filter(|item| !item.is_empty())
            .collect();
    }
    Vec::new()
}

fn capability_category_for_action(action: &str) -> CapabilityCategory {
    match action {
        "s3:GetObject" => CapabilityCategory::ObjectStorageRead,
        "s3:PutObject" => CapabilityCategory::ObjectStorageWrite,
        "s3:DeleteObject" => CapabilityCategory::ObjectStorageDelete,
        _ if action.starts_with("s3:") => {
            CapabilityCategory::Extension("object_storage".to_owned())
        }
        _ => CapabilityCategory::Unknown,
    }
}

fn service_for_action(action: &str) -> &str {
    action
        .split_once(':')
        .map(|(service, _)| service)
        .unwrap_or("unknown")
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn line_for_offset(text: &str, offset: usize) -> usize {
    text[..offset].bytes().filter(|byte| *byte == b'\n').count() + 1
}

/// Parse `terraform plan -json` (or `terraform show -json`) output into REIR grant facts.
/// This handles the structured JSON plan format (resource_changes with after values).
pub fn terraform_plan_json_to_bundle(plan_json: &str) -> Result<Bundle, String> {
    let plan: Value = serde_json::from_str(plan_json)
        .map_err(|e| format!("failed to parse terraform plan JSON: {e}"))?;

    let mut facts = Vec::new();

    // Handle both `terraform show -json` (has .values.root_module.resources)
    // and `terraform plan -json` (has .resource_changes)
    if let Some(changes) = plan.get("resource_changes").and_then(|v| v.as_array()) {
        for change in changes {
            let resource_type = change.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let name = change.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let address = change.get("address").and_then(|v| v.as_str()).unwrap_or("");

            if !matches!(
                resource_type,
                "aws_iam_role_policy"
                    | "aws_iam_policy"
                    | "aws_s3_bucket_policy"
                    | "aws_iam_user_policy"
                    | "aws_iam_group_policy"
            ) {
                continue;
            }

            let after = change
                .get("change")
                .and_then(|c| c.get("after"))
                .unwrap_or(&Value::Null);

            if let Some(policy_str) = after.get("policy").and_then(|v| v.as_str()) {
                if let Ok(policy) = serde_json::from_str::<Value>(policy_str) {
                    let block = TerraformResourceBlock {
                        file: "terraform-plan".to_owned(),
                        resource_type: resource_type.to_owned(),
                        name: name.to_owned(),
                        body: String::new(),
                        line: 0,
                    };
                    facts.extend(policy_grant_facts_with_address(&block, &policy, address));
                }
            }
        }
    }

    // Also handle `terraform show -json` state format
    if let Some(values) = plan.get("values") {
        if let Some(resources) = values
            .get("root_module")
            .and_then(|m| m.get("resources"))
            .and_then(|r| r.as_array())
        {
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
                    continue;
                }

                let values_obj = resource.get("values").unwrap_or(&Value::Null);
                if let Some(policy_str) = values_obj.get("policy").and_then(|v| v.as_str()) {
                    if let Ok(policy) = serde_json::from_str::<Value>(policy_str) {
                        let block = TerraformResourceBlock {
                            file: "terraform-state".to_owned(),
                            resource_type: resource_type.to_owned(),
                            name: name.to_owned(),
                            body: String::new(),
                            line: 0,
                        };
                        facts.extend(policy_grant_facts_with_address(&block, &policy, address));
                    }
                }
            }
        }
    }

    let mut bundle = Bundle::new();
    bundle.producers.push(crate::subject::Producer {
        name: "terraform-plan".to_owned(),
        version: PRODUCER_VERSION.to_owned(),
        adapter: Some("reir.adapters.terraform_plan".to_owned()),
        adapter_version: Some(ADAPTER_VERSION.to_owned()),
        source: Some("terraform_plan_json".to_owned()),
    });
    bundle.facts = facts;
    bundle.subjects = bundle
        .facts
        .iter()
        .map(|fact| fact.subject.clone())
        .collect();
    bundle.slices = crate::slice_by_kind(&bundle);
    Ok(bundle)
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
        if effect != "Allow" {
            continue;
        }
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
                facts.push(Fact {
                    schema: FACT_SCHEMA.to_owned(),
                    id: format!(
                        "{}::{}::grant::{}::{}",
                        subject_id,
                        statement_index,
                        sanitize_id(action),
                        sanitize_id(resource)
                    ),
                    kind: FactKind::Capability,
                    role: Some(FactRole::Granted),
                    subject,
                    capability: Some(Capability {
                        category: capability_category_for_action(action),
                        provider: Some("aws".to_owned()),
                        service: Some(service_for_action(action).to_owned()),
                        action: Some(action.clone()),
                        resource: Some(resource.clone()),
                        constraints: HashMap::new(),
                    }),
                    value: FactValue::True,
                    confidence: Confidence {
                        level: ConfidenceLevel::Computed,
                        source: Some("terraform_plan_json".to_owned()),
                    },
                    acquisition_mode: AcquisitionMode::CloudPolicy,
                    precision: Precision::ResourceScoped,
                    evidence: vec![Evidence {
                        kind: EvidenceKind::Extension("terraform_plan_resource".to_owned()),
                        file: Some(block.file.clone()),
                        line: Some(block.line),
                        column: None,
                        length: None,
                        symbol: Some(address.to_owned()),
                        reason: Some(format!("{address} grants {action} on {resource}")),
                        json_pointer: None,
                        resource: Some(resource.clone()),
                        provider: Some("aws".to_owned()),
                        value: None,
                        event_id: None,
                        time: None,
                        source: Some("terraform_plan".to_owned()),
                        event_name: None,
                        principal: None,
                        account: None,
                        policy_arn: None,
                        statement_index: Some(statement_index),
                        action: Some(action.clone()),
                    }],
                    unknown_reason: None,
                });
            }
        }
    }
    facts
}

fn json_string_or_array(obj: &Value, key: &str) -> Vec<String> {
    match obj.get(key) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => vec![],
    }
}

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

        assert!(bundle.facts.iter().any(|fact| {
            fact.role == Some(FactRole::Granted)
                && fact.capability.as_ref().is_some_and(|capability| {
                    capability.action.as_deref() == Some("s3:GetObject")
                        && capability.resource.as_deref() == Some("arn:aws:s3:::reports-prod/*")
                })
        }));
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
}
