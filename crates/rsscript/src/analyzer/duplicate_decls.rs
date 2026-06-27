use super::*;

impl Analyzer<'_> {
    /// Validate `#lower_name("...")` pins: each must be a valid Rust identifier,
    /// and no pin may collide with another function's final backend name (pinned
    /// or default), so generated symbols stay unique. Only conflicts that involve
    /// at least one pin are reported here (plain default collisions, if any, are a
    /// separate concern from this escape hatch).
    pub(super) fn check_lowered_name_conflicts(&mut self) {
        use crate::rust_lower::lowered_symbol_name;
        let mut seen: HashMap<String, (String, bool)> = HashMap::new();
        let mut conflicts: Vec<(crate::diagnostic::Span, String, String)> = Vec::new();
        let mut invalid: Vec<(crate::diagnostic::Span, String)> = Vec::new();
        for item in &self.syntax_program.items {
            let crate::syntax::ast::Item::Function(function) = item else {
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
                .unwrap_or_else(|| lowered_symbol_name(&function.name));
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
            self.diagnostics.push(
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
            self.diagnostics.push(
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

    pub(super) fn check_duplicate_declarations(&mut self) {
        for duplicate in self.hir.duplicate_symbols() {
            self.diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_DECLARATION,
                    format!(
                        "{} `{}` is declared more than once.",
                        duplicate_symbol_label(duplicate.kind),
                        duplicate.name
                    ),
                    duplicate.duplicate_span.clone(),
                    "duplicate declaration",
                )
                .with_cause(format!(
                    "The first declaration is at {}:{}.",
                    duplicate.first_span.line, duplicate.first_span.column
                ))
                .with_fix(
                    "rename_declaration",
                    "Rename or remove one declaration so the symbol table is unambiguous.",
                    "manual",
                ),
            );
        }
    }
}
