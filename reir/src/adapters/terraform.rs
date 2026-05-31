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
            if !matches!(
                block.resource_type.as_str(),
                "aws_iam_role_policy" | "aws_s3_bucket_policy"
            ) {
                continue;
            }
            for policy_json in terraform_policy_jsons(&block.body) {
                let policy: Value = serde_json::from_str(&policy_json).map_err(|error| {
                    format!(
                        "failed to parse IAM policy JSON in {} resource {}.{}: {error}",
                        block.file, block.resource_type, block.name
                    )
                })?;
                facts.extend(policy_grant_facts(&block, &policy));
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
}
