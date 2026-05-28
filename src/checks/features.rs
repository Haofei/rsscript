use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::HirFeatureUseKind;

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    if analyzer.syntax_program.local_capability_enabled() {
        return;
    }

    for feature_use in analyzer.hir.feature_uses().to_vec() {
        feature_violation(
            analyzer,
            feature_violation_summary(feature_use.kind),
            feature_use.span,
        );
    }
}

fn feature_violation_summary(kind: HirFeatureUseKind) -> &'static str {
    match kind {
        HirFeatureUseKind::LocalLet => "`local` requires `features: local`.",
        HirFeatureUseKind::LocalClosure => "local closures require `features: local`.",
        HirFeatureUseKind::Manage => "`manage` requires `features: local`.",
        HirFeatureUseKind::Take => "`take` requires `features: local`.",
        HirFeatureUseKind::ResourcePool => "`ResourcePool<T>` requires `features: local`.",
    }
}

fn feature_violation(analyzer: &mut Analyzer<'_>, summary: &str, span: crate::diagnostic::Span) {
    analyzer.diagnostics.push(
        Diagnostic::error(code::FEATURE_VIOLATION, summary, span, "feature violation").with_fix(
            "enable_local_feature",
            "Add `features: local` to the file header.",
            "manual",
        ),
    );
}
