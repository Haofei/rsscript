//! Declaration identity checks: duplicate source symbols and backend-name pins.

use std::collections::HashMap;

use crate::analyzer::{Analyzer, is_valid_rust_identifier};
use crate::diagnostic::{Diagnostic, code};
use crate::syntax::ast::Item;

pub(super) fn check(analyzer: &mut Analyzer<'_>) {
    check_duplicate_declarations(analyzer);
    check_lowered_name_conflicts(analyzer);
}

/// Validate `#lower_name("...")` pins: each must be a valid Rust identifier,
/// and no pin may collide with another function's final backend name (pinned
/// or default), so generated symbols stay unique. Only conflicts that involve
/// at least one pin are reported here (plain default collisions, if any, are a
/// separate concern from this escape hatch).
fn check_lowered_name_conflicts(analyzer: &mut Analyzer<'_>) {
    let mut seen: HashMap<String, (String, bool)> = HashMap::new();
    let mut conflicts: Vec<(crate::diagnostic::Span, String, String)> = Vec::new();
    let mut invalid: Vec<(crate::diagnostic::Span, String)> = Vec::new();
    for item in &analyzer.syntax_program.items {
        let Item::Function(function) = item else {
            continue;
        };
        let pinned = function.lower_name.as_deref();
        if let Some(pin) = pinned
            && !is_valid_rust_identifier(pin)
        {
            invalid.push((function.span.clone(), pin.to_string()));
            continue;
        }
        let lowered = pinned
            .map(str::to_string)
            .unwrap_or_else(|| crate::text_util::default_lowered_symbol_name(&function.name));
        let is_pinned = pinned.is_some();
        if let Some((first_name, first_pinned)) = seen.get(&lowered) {
            if is_pinned || *first_pinned {
                conflicts.push((function.span.clone(), lowered.clone(), first_name.clone()));
            }
        } else {
            seen.insert(lowered, (function.name.clone(), is_pinned));
        }
    }
    for (span, pin) in invalid {
        analyzer.diagnostics.push(
            Diagnostic::error(
                code::LOWER_NAME_CONFLICT,
                format!("`#lower_name(\"{pin}\")` is not a valid Rust identifier."),
                span,
                "invalid lowered name",
            )
            .with_cause(
                "A pinned backend name must be a plain Rust identifier (letters, digits, and underscores, not starting with a digit).",
            ),
        );
    }
    for (span, lowered, first_name) in conflicts {
        analyzer.diagnostics.push(
            Diagnostic::error(
                code::LOWER_NAME_CONFLICT,
                format!("lowered backend name `{lowered}` is already used by `{first_name}`."),
                span,
                "lowered name conflict",
            )
            .with_cause(
                "Two declarations would lower to the same Rust symbol; a `#lower_name(\"...\")` pin must be unique.",
            )
            .with_fix(
                "rename_lowered",
                "Choose a different `#lower_name(\"...\")` value.",
                "manual",
            ),
        );
    }
}

fn check_duplicate_declarations(analyzer: &mut Analyzer<'_>) {
    analyzer
        .diagnostics
        .extend(rsscript_semantics::duplicate_declaration_diagnostics(
            &analyzer.hir,
        ));
}
