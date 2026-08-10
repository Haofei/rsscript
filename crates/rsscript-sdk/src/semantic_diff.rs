use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{ArtifactBundle, InterfaceRequirementV1};
use rsscript_abi_model::FunctionSignature;

pub const SEMANTIC_DIFF_SCHEMA: &str = "rsscript.semantic_diff.v2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactIdentityV1 {
    pub bundle_digest: String,
    pub module_digest: String,
    pub snapshot_digest: String,
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
    pub parameters: Vec<FunctionParameterFactV1>,
    pub return_type: Option<String>,
    pub retained_params: Vec<String>,
    pub semantic_facts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FunctionParameterFactV1 {
    pub name: String,
    pub effect: String,
    pub ty: String,
    pub retained: bool,
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
pub struct CallEdgeFactV1 {
    pub caller: String,
    pub callee: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLifetimeFactV1 {
    pub function: String,
    pub binding: String,
    pub acquisition: String,
    pub cleanup: String,
    pub cleanup_on_cancellation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceTransferFactV1 {
    pub function: String,
    pub binding: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskGroupFactV1 {
    pub function: String,
    pub spawned_tasks: u32,
    pub select_arms: u32,
    pub drains_on_exit: bool,
    pub cleanup_on_cancellation: bool,
}

/// A complete Artifact import contract. Unlike the compact bundle manifest
/// requirement, this keeps the canonical parameter effects, retention, types,
/// result and async shape that explain a signature-hash change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalContractFactV1 {
    pub symbol: String,
    pub abi_version: u32,
    pub signature_hash: String,
    pub signature: FunctionSignature,
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
    pub external_contracts: FactSetDiffV1<ExternalContractFactV1>,
    pub exports: FactSetDiffV1<ExportFactV1>,
    pub external_calls: FactSetDiffV1<ExternalCallFactV1>,
    pub call_edges: FactSetDiffV1<CallEdgeFactV1>,
    pub recursive_functions: FactSetDiffV1<String>,
    pub resource_lifetimes: FactSetDiffV1<ResourceLifetimeFactV1>,
    pub resource_transfers: FactSetDiffV1<ResourceTransferFactV1>,
    pub task_groups: FactSetDiffV1<TaskGroupFactV1>,
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
            external_contracts: keyed_diff(
                &external_contracts(old),
                &external_contracts(new),
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
            call_edges: set_diff(
                &analysis_call_edges(old.analysis()),
                &analysis_call_edges(new.analysis()),
            ),
            recursive_functions: set_diff(
                &strings(&old.analysis()["recursive_functions"]),
                &strings(&new.analysis()["recursive_functions"]),
            ),
            resource_lifetimes: set_diff(
                &analysis_resource_lifetimes(old.analysis()),
                &analysis_resource_lifetimes(new.analysis()),
            ),
            resource_transfers: set_diff(
                &analysis_resource_transfers(old.analysis()),
                &analysis_resource_transfers(new.analysis()),
            ),
            task_groups: set_diff(
                &analysis_task_groups(old.analysis()),
                &analysis_task_groups(new.analysis()),
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
            && self.external_contracts.added.is_empty()
            && self.external_contracts.removed.is_empty()
            && self.external_contracts.changed.is_empty()
            && self.exports.added.is_empty()
            && self.exports.removed.is_empty()
            && self.exports.changed.is_empty()
            && self.external_calls.added.is_empty()
            && self.external_calls.removed.is_empty()
            && self.external_calls.changed.is_empty()
            && self.call_edges.added.is_empty()
            && self.call_edges.removed.is_empty()
            && self.call_edges.changed.is_empty()
            && self.recursive_functions.added.is_empty()
            && self.recursive_functions.removed.is_empty()
            && self.recursive_functions.changed.is_empty()
            && self.resource_lifetimes.added.is_empty()
            && self.resource_lifetimes.removed.is_empty()
            && self.resource_lifetimes.changed.is_empty()
            && self.resource_transfers.added.is_empty()
            && self.resource_transfers.removed.is_empty()
            && self.resource_transfers.changed.is_empty()
            && self.task_groups.added.is_empty()
            && self.task_groups.removed.is_empty()
            && self.task_groups.changed.is_empty()
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
        append_counts(&mut output, "External contracts", &self.external_contracts);
        append_counts(&mut output, "Exports", &self.exports);
        append_counts(&mut output, "External calls", &self.external_calls);
        append_counts(&mut output, "Call graph", &self.call_edges);
        append_counts(
            &mut output,
            "Recursive functions",
            &self.recursive_functions,
        );
        append_counts(&mut output, "Resource lifetimes", &self.resource_lifetimes);
        append_counts(&mut output, "Resource transfers", &self.resource_transfers);
        append_counts(&mut output, "Task groups", &self.task_groups);
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
                parameters: analysis_parameters(&item["parameters"]),
                return_type: item["return_type"].as_str().map(str::to_string),
                retained_params: strings(&item["retained_params"]),
                semantic_facts: strings(&item["semantic_facts"]),
            })
        })
        .collect()
}

fn analysis_parameters(value: &serde_json::Value) -> Vec<FunctionParameterFactV1> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(FunctionParameterFactV1 {
                name: item["name"].as_str()?.to_string(),
                effect: item["effect"].as_str()?.to_string(),
                ty: item["ty"].as_str()?.to_string(),
                retained: item["retained"].as_bool()?,
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

fn analysis_call_edges(analysis: &serde_json::Value) -> Vec<CallEdgeFactV1> {
    analysis["call_edges"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(CallEdgeFactV1 {
                caller: item["caller"].as_str()?.to_string(),
                callee: item["callee"].as_str()?.to_string(),
            })
        })
        .collect()
}

fn analysis_resource_lifetimes(analysis: &serde_json::Value) -> Vec<ResourceLifetimeFactV1> {
    analysis["resource_lifetimes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(ResourceLifetimeFactV1 {
                function: item["function"].as_str()?.to_string(),
                binding: item["binding"].as_str()?.to_string(),
                acquisition: item["acquisition"].as_str()?.to_string(),
                cleanup: item["cleanup"].as_str()?.to_string(),
                cleanup_on_cancellation: item["cleanup_on_cancellation"].as_bool()?,
            })
        })
        .collect()
}

fn analysis_resource_transfers(analysis: &serde_json::Value) -> Vec<ResourceTransferFactV1> {
    analysis["resource_transfers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(ResourceTransferFactV1 {
                function: item["function"].as_str()?.to_string(),
                binding: item["binding"].as_str()?.to_string(),
                operation: item["operation"].as_str()?.to_string(),
            })
        })
        .collect()
}

fn analysis_task_groups(analysis: &serde_json::Value) -> Vec<TaskGroupFactV1> {
    analysis["task_groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            Some(TaskGroupFactV1 {
                function: item["function"].as_str()?.to_string(),
                spawned_tasks: u32::try_from(item["spawned_tasks"].as_u64()?).ok()?,
                select_arms: u32::try_from(item["select_arms"].as_u64()?).ok()?,
                drains_on_exit: item["drains_on_exit"].as_bool()?,
                cleanup_on_cancellation: item["cleanup_on_cancellation"].as_bool()?,
            })
        })
        .collect()
}

fn external_contracts(bundle: &ArtifactBundle) -> Vec<ExternalContractFactV1> {
    bundle
        .external_contracts()
        .iter()
        .map(|import| ExternalContractFactV1 {
            symbol: import.symbol.as_str().to_string(),
            abi_version: import.abi_version,
            signature_hash: import.signature_hash.as_str().to_string(),
            signature: import.signature.clone(),
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

    #[test]
    fn external_contract_diff_explains_effect_and_retention_changes() {
        use rsscript_abi_model::{DataEffect, ExternalImport, ExternalSymbol, ParameterSignature};

        let read = FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "value".into(),
                effect: DataEffect::Read,
                ty: "String".into(),
                retained: false,
            }],
            result: "Unit".into(),
            asynchronous: false,
        };
        let mut take_and_retain = read.clone();
        take_and_retain.parameters[0].effect = DataEffect::Take;
        take_and_retain.parameters[0].retained = true;
        let old = ExternalImport {
            symbol: ExternalSymbol::new("host.test.send").unwrap(),
            signature_hash: read.hash(),
            signature: read,
            abi_version: 2,
        };
        let new = ExternalImport {
            symbol: old.symbol.clone(),
            signature_hash: take_and_retain.hash(),
            signature: take_and_retain,
            abi_version: 2,
        };
        let old = ExternalContractFactV1 {
            symbol: old.symbol.to_string(),
            abi_version: old.abi_version,
            signature_hash: old.signature_hash.as_str().to_string(),
            signature: old.signature,
        };
        let new = ExternalContractFactV1 {
            symbol: new.symbol.to_string(),
            abi_version: new.abi_version,
            signature_hash: new.signature_hash.as_str().to_string(),
            signature: new.signature,
        };
        let diff = keyed_diff(&[old], &[new], |item| item.symbol.clone());
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(
            diff.changed[0].old.signature.parameters[0].effect,
            DataEffect::Read
        );
        assert_eq!(
            diff.changed[0].new.signature.parameters[0].effect,
            DataEffect::Take
        );
        assert!(diff.changed[0].new.signature.parameters[0].retained);
    }

    #[test]
    fn export_facts_keep_explicit_local_ownership_contracts() {
        let facts = analysis_exports(&serde_json::json!({
            "exports": [{
                "name": "publish", "kind": "function", "function_kind": "sync",
                "parameters": [{
                    "name": "payload", "effect": "take", "ty": "Payload", "retained": true
                }],
                "return_type": "noescape Unit",
                "retained_params": ["payload"],
                "semantic_facts": ["take parameter `payload`", "retains(payload)"]
            }]
        }));
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].parameters[0].effect, "take");
        assert!(facts[0].parameters[0].retained);
        assert_eq!(facts[0].return_type.as_deref(), Some("noescape Unit"));
    }

    #[test]
    fn call_graph_facts_report_edges_and_recursion_without_review_metadata() {
        let facts = analysis_call_edges(&serde_json::json!({
            "call_edges": [
                { "caller": "main", "callee": "walk" },
                { "caller": "walk", "callee": "walk" }
            ]
        }));
        assert_eq!(facts.len(), 2);
        let recursion = set_diff(&["walk".to_string()], &Vec::new());
        assert_eq!(recursion.removed, ["walk"]);
    }

    #[test]
    fn execution_facts_keep_cleanup_and_structured_task_boundaries() {
        let analysis = serde_json::json!({
            "resource_lifetimes": [{
                "function": "main", "binding": "file", "acquisition": "with",
                "cleanup": "scope_exit", "cleanup_on_cancellation": true
            }],
            "resource_transfers": [{
                "function": "main", "binding": "file", "operation": "take"
            }],
            "task_groups": [{
                "function": "main", "spawned_tasks": 2, "select_arms": 3,
                "drains_on_exit": true, "cleanup_on_cancellation": true
            }]
        });
        assert_eq!(analysis_resource_lifetimes(&analysis)[0].binding, "file");
        assert!(analysis_resource_lifetimes(&analysis)[0].cleanup_on_cancellation);
        assert_eq!(analysis_resource_transfers(&analysis)[0].operation, "take");
        assert_eq!(analysis_task_groups(&analysis)[0].spawned_tasks, 2);
        assert!(analysis_task_groups(&analysis)[0].drains_on_exit);
    }
}
