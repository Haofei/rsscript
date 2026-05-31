use serde::{Deserialize, Serialize};

use crate::{Diff, Exception, PolicyResult, Reconciliation, Slice, Subject};

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
    pub diffs: Vec<Diff>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exceptions: Vec<Exception>,
}

impl Bundle {
    pub fn new() -> Self {
        Self {
            schema: "reir.bundle.v0.1".to_string(),
            ontology: "reir.capability_ontology.v0.1".to_string(),
            producers: Vec::new(),
            subjects: Vec::new(),
            subject_chains: Vec::new(),
            facts: Vec::new(),
            edges: Vec::new(),
            reconciliations: Vec::new(),
            slices: Vec::new(),
            policy_results: Vec::new(),
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
}

impl Default for Bundle {
    fn default() -> Self {
        Self::new()
    }
}
