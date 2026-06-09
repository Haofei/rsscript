use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Bundle, Evidence, Profile, Reconciliation, Subject};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diff {
    pub schema: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<DiffItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffItem {
    pub kind: DiffItemKind,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<Subject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiffItemKind {
    FactAdded,
    FactRemoved,
    FactChanged,
    EdgeAdded,
    EdgeRemoved,
    EdgeChanged,
    SubjectChainAdded,
    SubjectChainRemoved,
    SubjectChainChanged,
    ReconciliationAdded,
    ReconciliationRemoved,
    ReconciliationChanged,
    SliceAdded,
    SliceRemoved,
    SliceChanged,
    DiffAdded,
    DiffRemoved,
    DiffChanged,
    PolicyResultAdded,
    PolicyResultRemoved,
    PolicyResultChanged,
    ProfileRuleChanged,
    ExceptionAdded,
    ExceptionExpired,
    ExceptionChanged,
    SchemaChanged,
    ProducerChanged,
    OntologyChanged,
    Extension(String),
}

pub fn compute_diff(baseline: &Bundle, current: &Bundle) -> Diff {
    let mut items = Vec::new();

    push_indexed_diffs(
        &mut items,
        &baseline.facts,
        &current.facts,
        IndexedDiffSpec {
            added: DiffItemKind::FactAdded,
            removed: DiffItemKind::FactRemoved,
            changed: DiffItemKind::FactChanged,
            noun: "fact",
        },
        |fact| fact.id.as_str(),
        |fact| Some(fact.subject.clone()),
        |fact| fact.evidence.clone(),
    );

    push_indexed_diffs(
        &mut items,
        &baseline.edges,
        &current.edges,
        IndexedDiffSpec {
            added: DiffItemKind::EdgeAdded,
            removed: DiffItemKind::EdgeRemoved,
            changed: DiffItemKind::EdgeChanged,
            noun: "edge",
        },
        |edge| edge.id.as_str(),
        |_| None,
        |edge| edge.evidence.clone(),
    );

    push_indexed_diffs(
        &mut items,
        &baseline.subject_chains,
        &current.subject_chains,
        IndexedDiffSpec {
            added: DiffItemKind::SubjectChainAdded,
            removed: DiffItemKind::SubjectChainRemoved,
            changed: DiffItemKind::SubjectChainChanged,
            noun: "subject chain",
        },
        |chain| chain.id.as_str(),
        |_| None,
        |chain| chain.evidence.clone(),
    );

    push_reconciliation_diffs(
        &mut items,
        &baseline.reconciliations,
        &current.reconciliations,
    );

    push_indexed_diffs(
        &mut items,
        &baseline.slices,
        &current.slices,
        IndexedDiffSpec {
            added: DiffItemKind::SliceAdded,
            removed: DiffItemKind::SliceRemoved,
            changed: DiffItemKind::SliceChanged,
            noun: "slice",
        },
        |slice| slice.id.as_str(),
        |_| None,
        |slice| slice.evidence.clone(),
    );

    push_indexed_diffs(
        &mut items,
        &baseline.diffs,
        &current.diffs,
        IndexedDiffSpec {
            added: DiffItemKind::DiffAdded,
            removed: DiffItemKind::DiffRemoved,
            changed: DiffItemKind::DiffChanged,
            noun: "diff",
        },
        |diff| diff.id.as_str(),
        |_| None,
        |_| Vec::new(),
    );

    push_indexed_diffs(
        &mut items,
        &baseline.policy_results,
        &current.policy_results,
        IndexedDiffSpec {
            added: DiffItemKind::PolicyResultAdded,
            removed: DiffItemKind::PolicyResultRemoved,
            changed: DiffItemKind::PolicyResultChanged,
            noun: "policy result",
        },
        |result| result.id.as_str(),
        |result| result.subject.clone(),
        |result| result.evidence.clone(),
    );

    push_profile_diffs(&mut items, &baseline.profiles, &current.profiles);

    push_indexed_diffs(
        &mut items,
        &baseline.exceptions,
        &current.exceptions,
        IndexedDiffSpec {
            added: DiffItemKind::ExceptionAdded,
            removed: DiffItemKind::ExceptionExpired,
            changed: DiffItemKind::ExceptionChanged,
            noun: "exception",
        },
        |exception| exception.id.as_str(),
        |_| None,
        |exception| exception.evidence.clone(),
    );

    if baseline.schema != current.schema {
        items.push(DiffItem {
            kind: DiffItemKind::SchemaChanged,
            id: "schema".to_string(),
            subject: None,
            description: Some(format!(
                "schema changed from {} to {}",
                baseline.schema, current.schema
            )),
            evidence: Vec::new(),
        });
    }

    if baseline.producers != current.producers {
        items.push(DiffItem {
            kind: DiffItemKind::ProducerChanged,
            id: "producers".to_string(),
            subject: None,
            description: Some("producer metadata changed".to_string()),
            evidence: Vec::new(),
        });
    }

    if baseline.ontology != current.ontology {
        items.push(DiffItem {
            kind: DiffItemKind::OntologyChanged,
            id: "ontology".to_string(),
            subject: None,
            description: Some(format!(
                "ontology changed from {} to {}",
                baseline.ontology, current.ontology
            )),
            evidence: Vec::new(),
        });
    }

    Diff {
        schema: "reir.diff.v0.1".to_string(),
        id: "diff.bundle".to_string(),
        items,
    }
}

struct IndexedDiffSpec {
    added: DiffItemKind,
    removed: DiffItemKind,
    changed: DiffItemKind,
    noun: &'static str,
}

fn push_indexed_diffs<T>(
    items: &mut Vec<DiffItem>,
    baseline: &[T],
    current: &[T],
    spec: IndexedDiffSpec,
    id: impl Fn(&T) -> &str,
    subject: impl Fn(&T) -> Option<Subject>,
    evidence: impl Fn(&T) -> Vec<Evidence>,
) where
    T: PartialEq,
{
    let baseline_items = baseline
        .iter()
        .map(|item| (id(item), item))
        .collect::<BTreeMap<_, _>>();
    let current_items = current
        .iter()
        .map(|item| (id(item), item))
        .collect::<BTreeMap<_, _>>();

    for (item_id, item) in &current_items {
        match baseline_items.get(item_id) {
            None => items.push(DiffItem {
                kind: spec.added.clone(),
                id: (*item_id).to_string(),
                subject: subject(item),
                description: Some(format!("{} added", spec.noun)),
                evidence: evidence(item),
            }),
            Some(previous) if *previous != *item => items.push(DiffItem {
                kind: spec.changed.clone(),
                id: (*item_id).to_string(),
                subject: subject(item),
                description: Some(format!("{} changed", spec.noun)),
                evidence: evidence(item),
            }),
            Some(_) => {}
        }
    }

    for (item_id, item) in &baseline_items {
        if !current_items.contains_key(item_id) {
            items.push(DiffItem {
                kind: spec.removed.clone(),
                id: (*item_id).to_string(),
                subject: subject(item),
                description: Some(format!("{} removed", spec.noun)),
                evidence: evidence(item),
            });
        }
    }
}

fn push_profile_diffs(items: &mut Vec<DiffItem>, baseline: &[Profile], current: &[Profile]) {
    let baseline_profiles = baseline
        .iter()
        .map(|profile| (profile.kind.as_str(), profile))
        .collect::<BTreeMap<_, _>>();
    let current_profiles = current
        .iter()
        .map(|profile| (profile.kind.as_str(), profile))
        .collect::<BTreeMap<_, _>>();

    for (kind, profile) in &current_profiles {
        match baseline_profiles.get(kind) {
            None => items.push(profile_diff_item(kind, "profile rule added")),
            Some(previous) if *previous != *profile => {
                items.push(profile_diff_item(kind, "profile rule changed"));
            }
            Some(_) => {}
        }
    }

    for kind in baseline_profiles.keys() {
        if !current_profiles.contains_key(kind) {
            items.push(profile_diff_item(kind, "profile rule removed"));
        }
    }
}

fn profile_diff_item(kind: &str, description: &str) -> DiffItem {
    DiffItem {
        kind: DiffItemKind::ProfileRuleChanged,
        id: format!("profile.{kind}"),
        subject: None,
        description: Some(description.to_owned()),
        evidence: Vec::new(),
    }
}

fn push_reconciliation_diffs(
    items: &mut Vec<DiffItem>,
    baseline: &[Reconciliation],
    current: &[Reconciliation],
) {
    let baseline_reconciliations = baseline
        .iter()
        .map(|reconciliation| (reconciliation.id.as_str(), reconciliation))
        .collect::<BTreeMap<_, _>>();
    let current_reconciliations = current
        .iter()
        .map(|reconciliation| (reconciliation.id.as_str(), reconciliation))
        .collect::<BTreeMap<_, _>>();

    for (id, reconciliation) in &current_reconciliations {
        match baseline_reconciliations.get(id) {
            None => items.push(DiffItem {
                kind: DiffItemKind::ReconciliationAdded,
                id: (*id).to_string(),
                subject: None,
                description: Some("reconciliation added".to_string()),
                evidence: reconciliation.evidence.clone(),
            }),
            Some(previous) if *previous != *reconciliation => items.push(DiffItem {
                kind: DiffItemKind::ReconciliationChanged,
                id: (*id).to_string(),
                subject: None,
                description: Some("reconciliation changed".to_string()),
                evidence: reconciliation.evidence.clone(),
            }),
            Some(_) => {}
        }
    }

    for (id, reconciliation) in &baseline_reconciliations {
        if !current_reconciliations.contains_key(id) {
            items.push(DiffItem {
                kind: DiffItemKind::ReconciliationRemoved,
                id: (*id).to_string(),
                subject: None,
                description: Some("reconciliation removed".to_string()),
                evidence: reconciliation.evidence.clone(),
            });
        }
    }
}
