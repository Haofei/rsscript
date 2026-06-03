#[derive(Debug, Clone, PartialEq, Eq)]
struct HunkHeader {
    old_start: usize,
}

pub fn diff_unified(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let old_lines = split_lines(old);
    let new_lines = split_lines(new);
    let mut out = Vec::new();
    out.push("--- old".to_string());
    out.push("+++ new".to_string());
    out.push(format!(
        "@@ -1,{} +1,{} @@",
        old_lines.len(),
        new_lines.len()
    ));
    for line in &old_lines {
        out.push(format!("-{line}"));
    }
    for line in &new_lines {
        out.push(format!("+{line}"));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

pub fn patch_apply_text(original: &str, patch: &str) -> Result<String, String> {
    let original_had_trailing_newline = original.ends_with('\n');
    let original_lines = split_lines(original);
    let patch_lines = split_lines(patch);
    if patch_lines.is_empty() {
        return Ok(original.to_string());
    }

    let mut output = Vec::new();
    let mut original_index = 0usize;
    let mut patch_index = 0usize;

    while patch_index < patch_lines.len() {
        let line = &patch_lines[patch_index];
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            patch_index += 1;
            continue;
        }
        if !line.starts_with("@@ ") {
            return Err(format!("expected unified diff hunk header, got `{line}`"));
        }
        let header = parse_hunk_header(line)?;
        let target_index = header.old_start.saturating_sub(1);
        if target_index < original_index || target_index > original_lines.len() {
            return Err("patch hunk applies outside the original text".to_string());
        }
        while original_index < target_index {
            output.push(original_lines[original_index].clone());
            original_index += 1;
        }
        patch_index += 1;
        while patch_index < patch_lines.len() {
            let hunk_line = &patch_lines[patch_index];
            if hunk_line.starts_with("@@ ") {
                break;
            }
            if hunk_line.starts_with("--- ") || hunk_line.starts_with("+++ ") {
                break;
            }
            let Some(prefix) = hunk_line.chars().next() else {
                return Err("empty patch hunk line".to_string());
            };
            let value = hunk_line[1..].to_string();
            match prefix {
                ' ' => {
                    let Some(original_line) = original_lines.get(original_index) else {
                        return Err("patch context extends past original text".to_string());
                    };
                    if original_line != &value {
                        return Err(format!(
                            "patch context mismatch: expected `{}`, got `{value}`",
                            original_line
                        ));
                    }
                    output.push(original_line.clone());
                    original_index += 1;
                }
                '-' => {
                    let Some(original_line) = original_lines.get(original_index) else {
                        return Err("patch removal extends past original text".to_string());
                    };
                    if original_line != &value {
                        return Err(format!(
                            "patch removal mismatch: expected `{}`, got `{value}`",
                            original_line
                        ));
                    }
                    original_index += 1;
                }
                '+' => output.push(value),
                '\\' => {}
                _ => return Err(format!("unsupported patch hunk prefix `{prefix}`")),
            }
            patch_index += 1;
        }
    }

    while original_index < original_lines.len() {
        output.push(original_lines[original_index].clone());
        original_index += 1;
    }

    let mut text = output.join("\n");
    if original_had_trailing_newline || patch.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

fn split_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let trimmed = value.strip_suffix('\n').unwrap_or(value);
    trimmed.split('\n').map(ToString::to_string).collect()
}

fn parse_hunk_header(line: &str) -> Result<HunkHeader, String> {
    let mut parts = line.split_whitespace();
    let Some("@@") = parts.next() else {
        return Err("malformed hunk header".to_string());
    };
    let Some(old_part) = parts.next() else {
        return Err("hunk header missing old range".to_string());
    };
    if !old_part.starts_with('-') {
        return Err("hunk header old range must start with `-`".to_string());
    }
    let old_start = old_part[1..]
        .split(',')
        .next()
        .unwrap_or("1")
        .parse::<usize>()
        .map_err(|_| "hunk header old range start must be an integer".to_string())?;
    Ok(HunkHeader { old_start })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_diff_applies_to_original_text() {
        let old = "one\ntwo\nthree\n";
        let new = "one\n2\nthree\n";
        let patch = diff_unified(old, new);

        let applied = patch_apply_text(old, &patch).expect("patch applies");

        assert_eq!(applied, new);
    }

    #[test]
    fn patch_rejects_mismatched_removal() {
        let patch = "--- old\n+++ new\n@@ -1,1 +1,1 @@\n-bad\n+new\n";

        let error = patch_apply_text("old\n", patch).expect_err("mismatch should fail");

        assert!(error.contains("patch removal mismatch"));
    }
}
