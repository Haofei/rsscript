use reir::api::v1::{
    decision::{GateDecision, GatePolicy, GatePolicyFile, GateStatus, TargetGatePolicy},
    model::{Bundle, Capability, Fact, Subject},
    reconciliation::{Reconciliation, compute_diff, reconcile_capabilities_for_target},
    rendering::{CiGateOutput, format_ci_gate_json, format_pr_review_comment, format_sarif},
};

#[test]
fn crate_root_preserves_rsscript_compatibility_names() {
    use reir::{
        AcquisitionMode, Bundle, Capability, CapabilityCategory, Confidence, ConfidenceLevel,
        Evidence, EvidenceKind, Fact, FactKind, FactRole, FactValue, GatePolicy, Precision,
        ReconciliationKind, Subject, SubjectKind, compute_diff, decide_gate,
        format_pr_review_comment, reconcile_capabilities_for_target,
    };

    let exported_types = [
        std::any::type_name::<AcquisitionMode>(),
        std::any::type_name::<Bundle>(),
        std::any::type_name::<Capability>(),
        std::any::type_name::<CapabilityCategory>(),
        std::any::type_name::<Confidence>(),
        std::any::type_name::<ConfidenceLevel>(),
        std::any::type_name::<Evidence>(),
        std::any::type_name::<EvidenceKind>(),
        std::any::type_name::<Fact>(),
        std::any::type_name::<FactKind>(),
        std::any::type_name::<FactRole>(),
        std::any::type_name::<FactValue>(),
        std::any::type_name::<GatePolicy>(),
        std::any::type_name::<Precision>(),
        std::any::type_name::<ReconciliationKind>(),
        std::any::type_name::<Subject>(),
        std::any::type_name::<SubjectKind>(),
    ];
    assert!(exported_types.iter().all(|name| name.starts_with("reir::")));
    let _ = compute_diff;
    let _ = decide_gate;
    let _ = format_pr_review_comment;
    let _ = reconcile_capabilities_for_target;
}

#[test]
fn v1_facade_exposes_supported_api_groups() {
    let required = Bundle::new();
    let granted = Bundle::new();
    let reconciliations =
        reconcile_capabilities_for_target(&required.facts, &granted.facts, Some("test"));
    let diff = compute_diff(&required, &granted);

    let _: &[Fact] = &required.facts;
    let _: &[Subject] = &required.subjects;
    let _: Option<&Capability> = required
        .facts
        .first()
        .and_then(|fact| fact.capability.as_ref());
    let _: &[Reconciliation] = &reconciliations;
    assert!(diff.items.is_empty());

    let policy = GatePolicy::development();
    let decision = reir::api::v1::decision::decide_gate(
        &required.facts,
        &granted.facts,
        &reconciliations,
        policy,
    );
    assert_eq!(decision.status, GateStatus::Pass);

    let _: fn(&str) -> Result<GatePolicyFile, String> = GatePolicyFile::parse;
    let _ = TargetGatePolicy::default();
    let _: fn(&GateDecision, &[Fact], &[Fact], &[Reconciliation]) -> String =
        format_pr_review_comment;
    let _: fn(&GateDecision) -> String = format_sarif;
    let _: fn(&CiGateOutput) -> String = format_ci_gate_json;
}

#[test]
fn crate_root_has_no_blanket_public_reexports() {
    let root = include_str!("../src/lib.rs");
    let blanket_exports: Vec<_> = root
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("pub use ") && line.contains("::*"))
        .collect();

    assert!(
        blanket_exports.is_empty(),
        "crate root contains blanket public re-exports: {blanket_exports:?}"
    );
}
