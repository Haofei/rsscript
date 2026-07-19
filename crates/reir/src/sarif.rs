use crate::{GateDecision, GateIssueKind};

/// Render the authoritative gate decision as SARIF 2.1.0. Every blocker is an
/// error and every non-blocking issue is a warning.
pub fn format_sarif(decision: &GateDecision) -> String {
    let mut results = Vec::new();
    for (issue, level) in decision
        .blockers
        .iter()
        .map(|issue| (issue, "error"))
        .chain(decision.warnings.iter().map(|issue| (issue, "warning")))
    {
        let mut result = serde_json::json!({
            "ruleId": issue.kind.rule_id(),
            "level": level,
            "message": { "text": issue.message },
        });
        if let Some(evidence) = issue
            .evidence
            .iter()
            .find(|evidence| evidence.file.is_some())
        {
            let mut region = serde_json::Map::new();
            if let Some(line) = evidence.line {
                region.insert("startLine".to_string(), serde_json::json!(line.max(1)));
            }
            if let Some(column) = evidence.column {
                region.insert("startColumn".to_string(), serde_json::json!(column.max(1)));
            }
            if let Some(length) = evidence.length {
                region.insert(
                    "endColumn".to_string(),
                    serde_json::json!(evidence.column.unwrap_or(1).max(1) + length),
                );
            }
            result["locations"] = serde_json::json!([{
                "physicalLocation": {
                    "artifactLocation": { "uri": evidence.file.as_deref().unwrap_or_default() },
                    "region": serde_json::Value::Object(region),
                }
            }]);
        }
        results.push(result);
    }

    let rules = [
        GateIssueKind::InvalidEvidence,
        GateIssueKind::MissingCapability,
        GateIssueKind::UnknownCapability,
        GateIssueKind::ExcessCapability,
        GateIssueKind::UnverifiedCapability,
    ]
    .into_iter()
    .map(|kind| {
        serde_json::json!({
            "id": kind.rule_id(),
            "shortDescription": { "text": kind.rule_id().replace('_', " ") }
        })
    })
    .collect::<Vec<_>>();

    let bundle = serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "rsscript-reir",
                    "informationUri": "https://github.com/Haofei/rsscript",
                    "version": env!("CARGO_PKG_VERSION"),
                    "rules": rules,
                }
            },
            "results": results,
        }]
    });
    serde_json::to_string_pretty(&bundle).unwrap_or_else(|e| format!("{{\"error\": \"{e}\"}}"))
}
