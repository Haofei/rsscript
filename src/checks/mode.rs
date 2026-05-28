use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::HirModeUseKind;
use crate::syntax::ast::FileMode as SyntaxFileMode;

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    let Some(mode) = analyzer.syntax_program.mode else {
        return;
    };
    if mode != SyntaxFileMode::Managed {
        return;
    }

    for mode_use in analyzer.hir.mode_uses().to_vec() {
        mode_violation(
            analyzer,
            mode_violation_summary(mode_use.kind),
            mode_use.span,
        );
    }
}

fn mode_violation_summary(kind: HirModeUseKind) -> &'static str {
    match kind {
        HirModeUseKind::LocalLet => "`local` requires `mode: uses-local`.",
        HirModeUseKind::Manage => "`manage` requires `mode: uses-local`.",
        HirModeUseKind::Take => "`take` requires `mode: uses-local`.",
        HirModeUseKind::ResourcePool => "`ResourcePool<T>` requires `mode: uses-local`.",
    }
}

fn mode_violation(analyzer: &mut Analyzer<'_>, summary: &str, span: crate::diagnostic::Span) {
    analyzer.diagnostics.push(
        Diagnostic::error(code::FILE_MODE_VIOLATION, summary, span, "mode violation").with_fix(
            "change_mode",
            "Change the file declaration to `mode: uses-local`.",
            "manual",
        ),
    );
}
