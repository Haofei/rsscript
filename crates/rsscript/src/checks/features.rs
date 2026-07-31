use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::HirFeatureUseKind;
use crate::syntax::ast::FileFeature;

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    for feature_use in analyzer.hir.feature_uses().to_vec() {
        if !analyzer
            .syntax_program
            .file_has_feature(&feature_use.span.file, required_feature(feature_use.kind))
        {
            feature_violation(
                analyzer,
                feature_violation_summary(feature_use.kind),
                feature_use.span,
                required_feature(feature_use.kind),
            );
        }
    }
}

fn required_feature(kind: HirFeatureUseKind) -> FileFeature {
    match kind {
        HirFeatureUseKind::LocalLet
        | HirFeatureUseKind::LocalClosure
        | HirFeatureUseKind::Manage
        | HirFeatureUseKind::Take => FileFeature::Local,
        HirFeatureUseKind::Native => FileFeature::Native,
        HirFeatureUseKind::Unsafe => FileFeature::Unsafe,
        HirFeatureUseKind::Async => FileFeature::Async,
    }
}

fn feature_violation_summary(kind: HirFeatureUseKind) -> &'static str {
    match kind {
        HirFeatureUseKind::LocalLet => "`local` requires `features: local`.",
        HirFeatureUseKind::LocalClosure => "local closures require `features: local`.",
        HirFeatureUseKind::Manage => "`manage` requires `features: local`.",
        HirFeatureUseKind::Take => "`take` requires `features: local`.",
        HirFeatureUseKind::Native => "`native` boundaries require `features: native`.",
        HirFeatureUseKind::Unsafe => "`unsafe` effects require `features: unsafe`.",
        HirFeatureUseKind::Async => "`async fn`, `await`, and `spawn` require `features: async`.",
    }
}

fn feature_violation(
    analyzer: &mut Analyzer<'_>,
    summary: &str,
    span: crate::diagnostic::Span,
    feature: FileFeature,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(code::FEATURE_VIOLATION, summary, span, "feature violation").with_fix(
            "enable_file_feature",
            format!(
                "Add `features: {}` to the file header.",
                file_feature_name(feature)
            ),
            "manual",
        ),
    );
}

fn file_feature_name(feature: FileFeature) -> &'static str {
    match feature {
        FileFeature::Local => "local",
        FileFeature::Native => "native",
        FileFeature::Unsafe => "unsafe",
        FileFeature::Async => "async",
        FileFeature::Device => "device",
        FileFeature::Ffi => "ffi",
        FileFeature::Reflection => "reflection",
    }
}
