// HCL/JSON value parsing and canonical normalization helpers.

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
