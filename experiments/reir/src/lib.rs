#![forbid(unsafe_code)]

pub mod adapters;
pub mod api;
pub mod bundle;
pub mod capability;
pub mod decision;
pub mod diff;
pub mod edge;
pub mod evidence;
pub mod fact;
pub mod format;
pub mod policy;
pub mod reconciliation;
pub mod sarif;
pub mod slice;
pub mod subject;

pub use bundle::Bundle;
pub use capability::{Capability, CapabilityCategory};
pub use decision::{GatePolicy, decide_gate};
pub use diff::compute_diff;
pub use evidence::{
    AcquisitionMode, Confidence, ConfidenceLevel, Evidence, EvidenceKind, Precision,
};
pub use fact::{Fact, FactKind, FactRole, FactValue};
pub use format::format_pr_review_comment;
pub use reconciliation::{ReconciliationKind, reconcile_capabilities_for_target};
pub use subject::{Subject, SubjectKind};

// Internal modules historically shared these names through the crate root.
// Keep that implementation namespace private while the public root stays small.
pub(crate) use decision::{GateDecision, GateIssue, GateIssueKind, GateStatus};
pub(crate) use diff::{Diff, DiffItemKind};
pub(crate) use edge::{Edge, EdgeKind};
pub(crate) use fact::GateFactDomain;
pub(crate) use policy::{Exception, PolicyResult, PolicyStatus, Profile};
pub(crate) use reconciliation::{Reconciliation, ReconciliationStatus};
pub(crate) use slice::{Slice, SliceKind, slice_by_kind};
pub(crate) use subject::{Producer, SubjectChain};

#[cfg(test)]
pub(crate) use decision::decide_validated_gate;
#[cfg(test)]
pub(crate) use diff::DiffItem;
#[cfg(test)]
pub(crate) use reconciliation::{
    ReconciliationRisk, RiskClass, RiskSeverity, reconcile_capabilities,
};
#[cfg(test)]
pub(crate) use sarif::format_sarif;
#[cfg(test)]
pub(crate) use subject::{ChainEdge, ChainEdgeKind};
