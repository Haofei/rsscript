use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{Bundle, Evidence, Reconciliation, Subject};

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
    SubjectChainChanged,
    ReconciliationAdded,
    ReconciliationRemoved,
    ReconciliationChanged,
    ProfileRuleChanged,
    ExceptionAdded,
    ExceptionExpired,
    ProducerChanged,
    OntologyChanged,
    Extension(String),
}

pub fn compute_diff(baseline: &Bundle, current: &Bundle) -> Diff {
    let mut items = Vec::new();

    let baseline_facts = baseline
        .facts
        .iter()
        .map(|fact| (fact.id.as_str(), fact))
        .collect::<BTreeMap<_, _>>();
    let current_facts = current
        .facts
        .iter()
        .map(|fact| (fact.id.as_str(), fact))
        .collect::<BTreeMap<_, _>>();

    for (id, fact) in &current_facts {
        match baseline_facts.get(id) {
            None => items.push(DiffItem {
                kind: DiffItemKind::FactAdded,
                id: (*id).to_string(),
                subject: Some(fact.subject.clone()),
                description: Some("fact added".to_string()),
                evidence: fact.evidence.clone(),
            }),
            Some(previous) if *previous != *fact => items.push(DiffItem {
                kind: DiffItemKind::FactChanged,
                id: (*id).to_string(),
                subject: Some(fact.subject.clone()),
                description: Some("fact changed".to_string()),
                evidence: fact.evidence.clone(),
            }),
            Some(_) => {}
        }
    }

    for (id, fact) in &baseline_facts {
        if !current_facts.contains_key(id) {
            items.push(DiffItem {
                kind: DiffItemKind::FactRemoved,
                id: (*id).to_string(),
                subject: Some(fact.subject.clone()),
                description: Some("fact removed".to_string()),
                evidence: fact.evidence.clone(),
            });
        }
    }

    let baseline_edges = baseline
        .edges
        .iter()
        .map(|edge| (edge.id.as_str(), edge))
        .collect::<BTreeMap<_, _>>();
    let current_edges = current
        .edges
        .iter()
        .map(|edge| (edge.id.as_str(), edge))
        .collect::<BTreeMap<_, _>>();

    for (id, edge) in &current_edges {
        match baseline_edges.get(id) {
            None => items.push(DiffItem {
                kind: DiffItemKind::EdgeAdded,
                id: (*id).to_string(),
                subject: None,
                description: Some("edge added".to_string()),
                evidence: edge.evidence.clone(),
            }),
            Some(previous) if *previous != *edge => items.push(DiffItem {
                kind: DiffItemKind::EdgeChanged,
                id: (*id).to_string(),
                subject: None,
                description: Some("edge changed".to_string()),
                evidence: edge.evidence.clone(),
            }),
            Some(_) => {}
        }
    }

    for (id, edge) in &baseline_edges {
        if !current_edges.contains_key(id) {
            items.push(DiffItem {
                kind: DiffItemKind::EdgeRemoved,
                id: (*id).to_string(),
                subject: None,
                description: Some("edge removed".to_string()),
                evidence: edge.evidence.clone(),
            });
        }
    }

    let baseline_chains = baseline
        .subject_chains
        .iter()
        .map(|chain| (chain.id.as_str(), chain))
        .collect::<BTreeMap<_, _>>();
    let current_chains = current
        .subject_chains
        .iter()
        .map(|chain| (chain.id.as_str(), chain))
        .collect::<BTreeMap<_, _>>();

    for (id, chain) in &current_chains {
        match baseline_chains.get(id) {
            None => items.push(DiffItem {
                kind: DiffItemKind::SubjectChainAdded,
                id: (*id).to_string(),
                subject: None,
                description: Some("subject chain added".to_string()),
                evidence: chain.evidence.clone(),
            }),
            Some(previous) if *previous != *chain => items.push(DiffItem {
                kind: DiffItemKind::SubjectChainChanged,
                id: (*id).to_string(),
                subject: None,
                description: Some("subject chain changed".to_string()),
                evidence: chain.evidence.clone(),
            }),
            Some(_) => {}
        }
    }

    push_reconciliation_diffs(
        &mut items,
        &baseline.reconciliations,
        &current.reconciliations,
    );

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
