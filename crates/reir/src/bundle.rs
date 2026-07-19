use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::{Diff, Exception, PolicyResult, Profile, Reconciliation, Slice, Subject};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bundle {
    pub schema: String,
    pub ontology: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub producers: Vec<crate::subject::Producer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subjects: Vec<Subject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subject_chains: Vec<crate::SubjectChain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facts: Vec<crate::Fact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<crate::Edge>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reconciliations: Vec<Reconciliation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slices: Vec<Slice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_results: Vec<PolicyResult>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<Profile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diffs: Vec<Diff>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<Exception>,
}

impl Bundle {
    pub fn new() -> Self {
        Self {
            schema: "reir.bundle.v0.2".to_string(),
            ontology: "reir.capability_ontology.v0.2".to_string(),
            producers: Vec::new(),
            subjects: Vec::new(),
            subject_chains: Vec::new(),
            facts: Vec::new(),
            edges: Vec::new(),
            reconciliations: Vec::new(),
            slices: Vec::new(),
            policy_results: Vec::new(),
            profiles: Vec::new(),
            diffs: Vec::new(),
            exceptions: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    pub fn validate_for_gate(&self, label: &str) -> Result<(), String> {
        if self.schema != "reir.bundle.v0.2" {
            return Err(format!(
                "unsupported {label} bundle schema `{}`; expected `reir.bundle.v0.2`",
                self.schema
            ));
        }
        if self.ontology != "reir.capability_ontology.v0.2" {
            return Err(format!(
                "unsupported {label} bundle ontology `{}`; expected `reir.capability_ontology.v0.2`",
                self.ontology
            ));
        }
        let mut ids = HashSet::new();
        for fact in &self.facts {
            if !ids.insert(fact.id.as_str()) {
                return Err(format!(
                    "{label} bundle contains duplicate fact id `{}`",
                    fact.id
                ));
            }
            let gate_relevant = fact.capability.is_some()
                || matches!(
                    fact.role,
                    Some(
                        crate::FactRole::Required
                            | crate::FactRole::Granted
                            | crate::FactRole::Denied
                    )
                );
            if gate_relevant && matches!(fact.kind, crate::FactKind::Extension(_)) {
                return Err(format!(
                    "{label} bundle fact `{}` uses an unsupported extension kind",
                    fact.id
                ));
            }
            if gate_relevant
                && fact.capability.as_ref().is_some_and(|capability| {
                    matches!(capability.category, crate::CapabilityCategory::Extension(_))
                })
            {
                return Err(format!(
                    "{label} bundle fact `{}` uses an unsupported capability category",
                    fact.id
                ));
            }
            if gate_relevant && matches!(fact.subject.kind, crate::SubjectKind::Extension(_)) {
                return Err(format!(
                    "{label} bundle fact `{}` uses an unsupported subject kind",
                    fact.id
                ));
            }
        }
        Ok(())
    }
}

impl Default for Bundle {
    fn default() -> Self {
        Self::new()
    }
}
