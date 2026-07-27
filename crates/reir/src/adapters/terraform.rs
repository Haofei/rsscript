//! Terraform/OpenTofu IaC producer adapter for REIR.
//! Converts rendered `.tf` IAM policy resources into granted capability facts.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::*;

const FACT_SCHEMA: &str = "reir.fact.v0.1";
const PRODUCER_VERSION: &str = "0.1.0";
const ADAPTER_VERSION: &str = "0.1";
const PRODUCER_SOURCE: &str = "terraform_iac";
const SOURCE_EVIDENCE_REASON: &str =
    "Terraform source scan is not proof of rendered, planned, or deployed authorization";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerraformSourceLimits {
    pub max_files: usize,
    pub max_depth: usize,
    pub max_file_bytes: u64,
    pub max_total_bytes: u64,
}

impl Default for TerraformSourceLimits {
    fn default() -> Self {
        Self {
            max_files: 1_024,
            max_depth: 32,
            max_file_bytes: 2 * 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Default)]
struct TerraformSourceBudget {
    files: usize,
    bytes: u64,
}

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
                _ => {}
            }
        }
    }
    for fact in &mut facts {
        mark_source_scan_unverified(fact);
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

#[derive(Debug, Clone)]
struct TerraformResourceBlock {
    file: String,
    resource_type: String,
    name: String,
    body: String,
    line: usize,
}

fn canonical_terraform_root(root: &Path) -> Result<PathBuf, String> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("failed to inspect {}: {error}", root.display()))?;
    if is_link_or_reparse_point(&metadata) {
        return Err(format!(
            "refusing Terraform source root that is a symlink or reparse point: {}",
            root.display()
        ));
    }
    fs::canonicalize(root)
        .map_err(|error| format!("failed to canonicalize {}: {error}", root.display()))
}

fn collect_tf_files(
    canonical_root: &Path,
    root: &Path,
    depth: usize,
    limits: TerraformSourceLimits,
    budget: &mut TerraformSourceBudget,
    visited: &mut HashSet<PathBuf>,
    files: &mut Vec<PathBuf>,
) -> Result<(), String> {
    if depth > limits.max_depth {
        return Err(format!(
            "Terraform source traversal exceeded maximum depth {} at {}",
            limits.max_depth,
            root.display()
        ));
    }
    let metadata = fs::symlink_metadata(root)
        .map_err(|error| format!("failed to inspect {}: {error}", root.display()))?;
    if is_link_or_reparse_point(&metadata) {
        return Err(format!(
            "refusing to follow symlink or reparse point in Terraform source tree: {}",
            root.display()
        ));
    }
    let canonical = fs::canonicalize(root)
        .map_err(|error| format!("failed to canonicalize {}: {error}", root.display()))?;
    ensure_beneath_root(canonical_root, &canonical)?;
    if metadata.is_file() {
        if canonical
            .extension()
            .is_some_and(|extension| extension == "tf")
        {
            account_tf_file(&canonical, metadata.len(), limits, budget)?;
            files.push(canonical);
        }
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(format!(
            "Terraform source root is neither a file nor directory: {}",
            root.display()
        ));
    }
    if !visited.insert(canonical.clone()) {
        return Err(format!(
            "Terraform source traversal encountered a directory more than once: {}",
            canonical.display()
        ));
    }

    let entries = fs::read_dir(&canonical).map_err(|error| {
        format!(
            "failed to read Terraform directory {}: {error}",
            canonical.display()
        )
    })?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("failed to read Terraform directory entry: {error}"))?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if is_link_or_reparse_point(&metadata) {
            return Err(format!(
                "refusing to follow symlink or reparse point in Terraform source tree: {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            collect_tf_files(
                canonical_root,
                &path,
                depth + 1,
                limits,
                budget,
                visited,
                files,
            )?;
        } else if metadata.is_file() && path.extension().is_some_and(|extension| extension == "tf")
        {
            let canonical = fs::canonicalize(&path)
                .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))?;
            ensure_beneath_root(canonical_root, &canonical)?;
            account_tf_file(&canonical, metadata.len(), limits, budget)?;
            files.push(canonical);
        }
    }
    Ok(())
}

fn ensure_beneath_root(root: &Path, path: &Path) -> Result<(), String> {
    if path == root || path.starts_with(root) {
        Ok(())
    } else {
        Err(format!(
            "Terraform source path escapes canonical root {}: {}",
            root.display(),
            path.display()
        ))
    }
}

fn is_link_or_reparse_point(metadata: &Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn account_tf_file(
    path: &Path,
    bytes: u64,
    limits: TerraformSourceLimits,
    budget: &mut TerraformSourceBudget,
) -> Result<(), String> {
    if bytes > limits.max_file_bytes {
        return Err(format!(
            "Terraform source file {} is {bytes} bytes, exceeding the {} byte limit",
            path.display(),
            limits.max_file_bytes
        ));
    }
    budget.files = budget
        .files
        .checked_add(1)
        .ok_or_else(|| "Terraform source file count overflow".to_owned())?;
    if budget.files > limits.max_files {
        return Err(format!(
            "Terraform source traversal exceeded the {} file limit",
            limits.max_files
        ));
    }
    budget.bytes = budget
        .bytes
        .checked_add(bytes)
        .ok_or_else(|| "Terraform source byte count overflow".to_owned())?;
    if budget.bytes > limits.max_total_bytes {
        return Err(format!(
            "Terraform source traversal exceeded the {} byte limit",
            limits.max_total_bytes
        ));
    }
    Ok(())
}

fn read_tf_file(canonical_root: &Path, path: &Path, max_bytes: u64) -> Result<String, String> {
    let link_metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
    if is_link_or_reparse_point(&link_metadata) || !link_metadata.is_file() {
        return Err(format!(
            "Terraform source path changed to an unsupported file type: {}",
            path.display()
        ));
    }
    let canonical = fs::canonicalize(path)
        .map_err(|error| format!("failed to canonicalize {}: {error}", path.display()))?;
    ensure_beneath_root(canonical_root, &canonical)?;
    let mut file = File::open(&canonical)
        .map_err(|error| format!("failed to read {}: {error}", canonical.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("failed to inspect {}: {error}", canonical.display()))?;
    if !metadata.is_file() {
        return Err(format!(
            "Terraform source path is not a regular file: {}",
            canonical.display()
        ));
    }
    if metadata.len() > max_bytes {
        return Err(format!(
            "Terraform source file {} exceeds the {max_bytes} byte limit",
            canonical.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed to read {}: {error}", canonical.display()))?;
    if bytes.len() as u64 > max_bytes {
        return Err(format!(
            "Terraform source file {} grew beyond the {max_bytes} byte limit while reading",
            canonical.display()
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        format!(
            "Terraform source file {} is not UTF-8: {error}",
            canonical.display()
        )
    })
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

fn policy_principal(block: &TerraformResourceBlock, statement: &Value) -> Option<String> {
    let attribute = match block.resource_type.as_str() {
        "aws_iam_role_policy" => "role",
        "aws_iam_user_policy" => "user",
        "aws_iam_group_policy" => "group",
        "aws_s3_bucket_policy" => {
            return statement.get("Principal").map(canonical_json_value);
        }
        _ => return None,
    };
    hcl_string_attr(&block.body, attribute)
}

fn canonical_json_value(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "<invalid>".to_owned())
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
    let lines = body.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix(key) else {
            index += 1;
            continue;
        };
        let rest = rest.trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            index += 1;
            continue;
        };
        let rest = rest.trim();
        let Some(rest) = rest.strip_prefix('[') else {
            index += 1;
            continue;
        };
        let mut array_body = rest.to_owned();
        while !array_body.contains(']') {
            index += 1;
            let Some(next_line) = lines.get(index) else {
                break;
            };
            array_body.push('\n');
            array_body.push_str(next_line.trim());
        }
        if let Some(end) = array_body.find(']') {
            return parse_hcl_string_array_items(&array_body[..end]);
        }
        index += 1;
    }
    Vec::new()
}

fn parse_hcl_string_array_items(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|item| item.trim().trim_matches('"').to_owned())
        .filter(|item| !item.is_empty())
        .collect()
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
                        body: principal_body(resource_type, after),
                        line: 0,
                    };
                    facts.extend(policy_grant_facts_with_address(&block, &policy, address));
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
        collect_state_resources(root_module, &mut resources);
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
                        body: principal_body(resource_type, values_obj),
                        line: 0,
                    };
                    facts.extend(policy_grant_facts_with_address(&block, &policy, address));
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

fn collect_state_resources<'a>(module: &'a Value, resources: &mut Vec<&'a Value>) {
    if let Some(items) = module.get("resources").and_then(Value::as_array) {
        resources.extend(items);
    }
    if let Some(children) = module.get("child_modules").and_then(Value::as_array) {
        for child in children {
            collect_state_resources(child, resources);
        }
    }
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
}
