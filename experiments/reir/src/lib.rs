#![forbid(unsafe_code)]
#![allow(
    clippy::collapsible_if,
    clippy::infallible_try_from,
    clippy::too_many_arguments
)]

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
#[allow(unused_imports)]
pub(crate) use decision::{
    GateDecision, GateIssue, GateIssueKind, GateStatus, decide_validated_gate,
};
#[allow(unused_imports)]
pub(crate) use diff::{Diff, DiffItem, DiffItemKind};
#[allow(unused_imports)]
pub(crate) use edge::{Edge, EdgeKind};
#[allow(unused_imports)]
pub(crate) use fact::{GateFactDomain, GateInputError};
#[allow(unused_imports)]
pub(crate) use format::{
    CiCapabilityFact, CiEvidence, CiGateOutput, CiGatePolicy, CiGateStatus, CiGateSummary,
    CiReconciliation, CiReviewDecision, CiSubject, format_bundle_summary, format_ci_gate_json,
    format_ci_gate_output, format_ci_gate_output_from_decision, format_ci_gate_output_with_policy,
    format_diff_human, format_policy_results_human, format_reconciliation_report,
    format_reconciliations_human, format_slices_human,
};
#[allow(unused_imports)]
pub(crate) use policy::{
    Exception, GatePolicyFile, PolicyResult, PolicyStatus, Profile, ProfileBudget,
    ProfilePermission, TargetGatePolicy, evaluate_policy,
};
#[allow(unused_imports)]
pub(crate) use reconciliation::{
    Reconciliation, ReconciliationLimits, ReconciliationRisk, ReconciliationStatus, RiskClass,
    RiskSeverity, reconcile_capabilities, reconcile_capabilities_for_gate,
    reconcile_capabilities_with_limits,
};
#[allow(unused_imports)]
pub(crate) use sarif::format_sarif;
#[allow(unused_imports)]
pub(crate) use slice::{Slice, SliceKind, slice_by_kind};
#[allow(unused_imports)]
pub(crate) use subject::{ChainEdge, ChainEdgeKind, Producer, SubjectChain};
