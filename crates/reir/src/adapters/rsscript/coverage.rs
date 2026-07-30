// Fail-closed coverage emitted when bounded bundle construction cannot complete.

fn rsscript_budget_exceeded_bundle(error: AdapterBuildError, source: &'static str) -> Bundle {
    let reason = format!("RSScript adapter output is incomplete: {error}");
    let mut builder = BoundedEvidenceBuilder::new(AdapterLimits::new(4, 1, 64 * 1024));
    let coverage = UnknownCoverage {
        id: "fact.rsscript.adapter_budget.unknown".to_owned(),
        subject_kind: SubjectKind::Package,
        subject_id: "package::rsscript".to_owned(),
        subject_name: "rsscript".to_owned(),
        package: "rsscript",
        reason,
        source,
        acquisition_mode: AcquisitionMode::CompilerContract,
        evidence_kind: EvidenceKind::UnknownReason,
        evidence_file: source,
        evidence_pointer: None,
        evidence_value: "adapter_budget_exceeded",
    };
    if builder.push_unknown_coverage(coverage).is_err() {
        return Bundle::new();
    }
    builder
        .finish(rsscript_provenance("rsscript-language", source))
        .unwrap_or_default()
}
