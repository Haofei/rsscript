use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{ArtifactBundle, InterfaceRequirementV1};

pub const SEMANTIC_DIFF_SCHEMA: &str = "rsscript.semantic_diff.v2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIdentityV1 {
    pub bundle_digest: String,
    pub module_digest: String,
    pub snapshot_digest: Option<String>,
    pub source_content_hash: String,
    pub interface_catalog_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangedFactV1<T> {
    pub key: String,
    pub old: T,
    pub new: T,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FactSetDiffV1<T> {
    pub added: Vec<T>,
    pub removed: Vec<T>,
    pub changed: Vec<ChangedFactV1<T>>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportFactV1 {
    pub name: String,
    pub kind: String,
    pub function_kind: Option<String>,
    pub retained_params: Vec<String>,
    pub semantic_facts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalCallFactV1 {
    pub function: String,
    pub symbol: String,
    pub call_chain: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AwaitFactV1 {
    pub function: String,
    pub callee: Option<String>,
    pub live_across_await: Vec<String>,
}

/// Coordinate-free diagnostic identity. Moving source text without changing a
/// diagnostic must not create a semantic diff entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticFactV1 {
    pub code: String,
    pub severity: String,
    pub summary: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountChangeV1 {
    pub old: u64,
    pub new: u64,
}

/// A policy-neutral comparison of facts derived from two build artifacts.
/// It intentionally contains no risk score and makes no allow/deny decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SemanticDiffV1 {
    pub schema: String,
    pub old: ArtifactIdentityV1,
    pub new: ArtifactIdentityV1,
    pub imports: FactSetDiffV1<InterfaceRequirementV1>,
    pub exports: FactSetDiffV1<ExportFactV1>,
    pub external_calls: FactSetDiffV1<ExternalCallFactV1>,
    pub await_sites: FactSetDiffV1<AwaitFactV1>,
    pub diagnostics: FactSetDiffV1<DiagnosticFactV1>,
    pub summary: BTreeMap<String, CountChangeV1>,
}

impl SemanticDiffV1 {
    pub fn between(old: &ArtifactBundle, new: &ArtifactBundle) -> Self {
        Self {
            schema: SEMANTIC_DIFF_SCHEMA.to_string(),
            old: identity(old),
            new: identity(new),
            imports: keyed_diff(
                old.required_interfaces(),
                new.required_interfaces(),
                |item| item.symbol.clone(),
            ),
            exports: keyed_diff(
                &analysis_exports(old.analysis()),
                &analysis_exports(new.analysis()),
                |item| item.name.clone(),
            ),
            external_calls: set_diff(
                &analysis_external_calls(old.analysis()),
                &analysis_external_calls(new.analysis()),
            ),
            await_sites: set_diff(
                &analysis_await_sites(old.analysis()),
                &analysis_await_sites(new.analysis()),
            ),
            diagnostics: set_diff(
                &analysis_diagnostics(old.analysis()),
                &analysis_diagnostics(new.analysis()),
            ),
            summary: summary_diff(old.analysis(), new.analysis()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.imports.added.is_empty()
            && self.imports.removed.is_empty()
            && self.imports.changed.is_empty()
            && self.exports.added.is_empty()
            && self.exports.removed.is_empty()
            && self.exports.changed.is_empty()
            && self.external_calls.added.is_empty()
            && self.external_calls.removed.is_empty()
            && self.external_calls.changed.is_empty()
            && self.await_sites.added.is_empty()
            && self.await_sites.removed.is_empty()
            && self.await_sites.changed.is_empty()
            && self.diagnostics.added.is_empty()
            && self.diagnostics.removed.is_empty()
            && self.diagnostics.changed.is_empty()
            && self.summary.is_empty()
    }

    pub fn to_markdown(&self) -> String {
        let mut output = String::from("## RSScript semantic diff\n\n");
        output.push_str(&format!("- Old module: `{}`\n", self.old.module_digest));
        output.push_str(&format!("- New module: `{}`\n", self.new.module_digest));
        append_counts(&mut output, "Imports", &self.imports);
        append_counts(&mut output, "Exports", &self.exports);
        append_counts(&mut output, "External calls", &self.external_calls);
        append_counts(&mut output, "Await sites", &self.await_sites);
        append_counts(&mut output, "Diagnostics", &self.diagnostics);
        if !self.summary.is_empty() {
            output.push_str("\n### Summary counters\n\n");
            for (name, change) in &self.summary {
                output.push_str(&format!("- `{name}`: {} → {}\n", change.old, change.new));
            }
        }
        output
    }
}

fn identity(bundle: &ArtifactBundle) -> ArtifactIdentityV1 {
    let provenance = bundle.provenance();
    ArtifactIdentityV1 {
        bundle_digest: bundle.digest().to_string(),
        module_digest: provenance.module_digest.clone(),
        snapshot_digest: provenance.snapshot_digest.clone(),
        source_content_hash: provenance.source_content_hash.clone(),
        interface_catalog_digest: provenance.interface_catalog_digest.clone(),
    }
}

fn keyed_diff<T: Clone + Eq>(old: &[T], new: &[T], key: impl Fn(&T) -> String) -> FactSetDiffV1<T> {
    let old = old
        .iter()
        .map(|item| (key(item), item))
        .collect::<BTreeMap<_, _>>();
    let new = new
        .iter()
        .map(|item| (key(item), item))
        .collect::<BTreeMap<_, _>>();
    let mut diff = FactSetDiffV1 {
        added: Vec::new(),
        removed: Vec::new(),
        changed: Vec::new(),
    };
    for (name, item) in &new {
        match old.get(name) {
            None => diff.added.push((*item).clone()),
            Some(previous) if **previous != **item => diff.changed.push(ChangedFactV1 {
                key: name.clone(),
                old: (**previous).clone(),
                new: (*item).clone(),
            }),
            Some(_) => {}
        }
    }
    for (name, item) in old {
        if !new.contains_key(&name) {
            diff.removed.push(item.clone());
        }
    }
    diff
}

fn set_diff<T: Clone + Ord>(old: &[T], new: &[T]) -> FactSetDiffV1<T> {
    let old = old.iter().cloned().collect::<BTreeSet<_>>();
    let new = new.iter().cloned().collect::<BTreeSet<_>>();
    FactSetDiffV1 {
        added: new.difference(&old).cloned().collect(),
        removed: old.difference(&new).cloned().collect(),
        changed: Vec::new(),
    }
}

fn analysis_exports(analysis: &serde_json::Value) -> Vec<ExportFactV1> {
    analysis["exports"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(ExportFactV1 {
                name: item["name"].as_str()?.to_string(),
                kind: item["kind"].as_str()?.to_string(),
                function_kind: item["function_kind"].as_str().map(str::to_string),
                retained_params: strings(&item["retained_params"]),
                semantic_facts: strings(&item["semantic_facts"]),
            })
        })
        .collect()
}

fn analysis_external_calls(analysis: &serde_json::Value) -> Vec<ExternalCallFactV1> {
    analysis["external_imports"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(ExternalCallFactV1 {
                function: item["function"].as_str()?.to_string(),
                symbol: item["symbol"].as_str()?.to_string(),
                call_chain: strings(&item["call_chain"]),
            })
        })
        .collect()
}

fn analysis_await_sites(analysis: &serde_json::Value) -> Vec<AwaitFactV1> {
    analysis["await_sites"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(AwaitFactV1 {
                function: item["function"].as_str()?.to_string(),
                callee: item["callee"].as_str().map(str::to_string),
                live_across_await: strings(&item["live_across_await"]),
            })
        })
        .collect()
}

fn analysis_diagnostics(analysis: &serde_json::Value) -> Vec<DiagnosticFactV1> {
    analysis["diagnostics"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(DiagnosticFactV1 {
                code: item["code"].as_str()?.to_string(),
                severity: item["severity"].as_str()?.to_string(),
                summary: item["summary"].as_str()?.to_string(),
                label: item["label"].as_str()?.to_string(),
            })
        })
        .collect()
}

fn strings(value: &serde_json::Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| value.as_str().map(str::to_string))
        .collect()
}

fn summary_diff(
    old: &serde_json::Value,
    new: &serde_json::Value,
) -> BTreeMap<String, CountChangeV1> {
    let Some(old) = old["summary"].as_object() else {
        return BTreeMap::new();
    };
    let Some(new) = new["summary"].as_object() else {
        return BTreeMap::new();
    };
    old.keys()
        .chain(new.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| {
            let old = old
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let new = new
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            (old != new).then(|| (key.clone(), CountChangeV1 { old, new }))
        })
        .collect()
}

fn append_counts<T: Serialize>(output: &mut String, title: &str, diff: &FactSetDiffV1<T>) {
    output.push_str(&format!(
        "\n### {title}\n\n- Added: {}\n- Removed: {}\n- Changed: {}\n",
        diff.added.len(),
        diff.removed.len(),
        diff.changed.len()
    ));
    for item in &diff.added {
        output.push_str(&format!(
            "  - Added `{}`\n",
            serde_json::to_string(item).expect("semantic fact serializes")
        ));
    }
    for item in &diff.removed {
        output.push_str(&format!(
            "  - Removed `{}`\n",
            serde_json::to_string(item).expect("semantic fact serializes")
        ));
    }
    for item in &diff.changed {
        output.push_str(&format!("  - Changed `{}`\n", item.key));
    }
}

#[cfg(test)]
mod tests {
    use crate::Compiler;

    use super::*;

    #[test]
    fn diff_reports_facts_without_policy_conclusions() {
        let compiler = Compiler;
        let old = compiler
            .compile("old.rss", "pub fn value() -> Int { return 1 }")
            .expect("old artifact");
        let new = compiler
            .compile("new.rss", "pub fn value() -> Int { return 2 }")
            .expect("new artifact");
        let diff = SemanticDiffV1::between(old.bundle(), new.bundle());
        let json = serde_json::to_string(&diff).expect("semantic diff JSON");
        assert_eq!(diff.schema, SEMANTIC_DIFF_SCHEMA);
        assert_ne!(diff.old.module_digest, diff.new.module_digest);
        assert!(!json.contains("risk"));
        assert!(!json.contains("allow"));
        assert!(!json.contains("deny"));
    }

    #[test]
    fn diagnostic_facts_ignore_coordinates_and_remain_policy_neutral() {
        let analysis = serde_json::json!({
            "diagnostics": [{
                "code": "E1001", "severity": "error", "summary": "bad call",
                "label": "argument", "span": { "start": 1, "end": 2 }
            }]
        });
        assert_eq!(
            analysis_diagnostics(&analysis),
            vec![DiagnosticFactV1 {
                code: "E1001".into(),
                severity: "error".into(),
                summary: "bad call".into(),
                label: "argument".into(),
            }]
        );
    }
}
