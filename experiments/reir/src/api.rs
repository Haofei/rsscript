//! Versioned, structured public API facades.

/// Curated version 1 API.
///
/// Adapter entrypoints are intentionally excluded because they are
/// integration-specific and remain available under [`crate::adapters`].
/// REIR remains `0.1.x`; this namespace is versioned without promising a
/// stable SemVer surface.
pub mod v1 {
    /// REIR wire-model types.
    pub mod model {
        pub use crate::bundle::Bundle;
        pub use crate::capability::{Capability, CapabilityCategory};
        pub use crate::edge::{Edge, EdgeKind};
        pub use crate::evidence::{
            AcquisitionMode, Confidence, ConfidenceLevel, Evidence, EvidenceKind, Precision,
        };
        pub use crate::fact::{
            Fact, FactKind, FactRole, FactValue, GateFactDomain, GateInputError,
        };
        pub use crate::subject::{
            ChainEdge, ChainEdgeKind, Producer, Subject, SubjectChain, SubjectKind,
        };
    }

    /// Policy configuration, evaluation, and gate decisions.
    pub mod decision {
        pub use crate::decision::{
            GateDecision, GateIssue, GateIssueKind, GatePolicy, GateStatus, decide_gate,
            decide_validated_gate,
        };
        pub use crate::policy::{
            Exception, GatePolicyFile, PolicyResult, PolicyStatus, Profile, ProfileBudget,
            ProfilePermission, TargetGatePolicy, evaluate_policy,
        };
    }

    /// Reconciliation, comparison, and review slicing.
    pub mod reconciliation {
        pub use crate::diff::{Diff, DiffItem, DiffItemKind, compute_diff};
        pub use crate::reconciliation::{
            Reconciliation, ReconciliationKind, ReconciliationLimits, ReconciliationRisk,
            ReconciliationStatus, RiskClass, RiskSeverity, reconcile_capabilities,
            reconcile_capabilities_for_gate, reconcile_capabilities_for_target,
            reconcile_capabilities_with_limits,
        };
        pub use crate::slice::{Slice, SliceKind, slice_by_kind};
    }

    /// Human-readable and CI-oriented rendering.
    pub mod rendering {
        pub use crate::format::{
            CiCapabilityFact, CiEvidence, CiGateOutput, CiGatePolicy, CiGateStatus, CiGateSummary,
            CiReconciliation, CiReviewDecision, CiSubject, format_bundle_summary,
            format_ci_gate_json, format_ci_gate_output, format_ci_gate_output_from_decision,
            format_ci_gate_output_with_policy, format_diff_human, format_policy_results_human,
            format_pr_review_comment, format_reconciliation_report, format_reconciliations_human,
            format_slices_human,
        };
        pub use crate::sarif::format_sarif;
    }
}
