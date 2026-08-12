//! Diagnostics for already-resolved local place access facts.

use rsscript_diagnostics::{Diagnostic, Span, code};

/// Diagnose splitting two fields of a managed object within one call.
pub fn managed_field_split_conflict_diagnostic(left: &str, right: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::MANAGED_FIELD_SPLIT_CONFLICT,
        format!("managed object fields `{left}` and `{right}` cannot be split in one call."),
        span,
        "managed field split conflict",
    )
    .with_cause(
        "Field splitting into disjoint inline paths is a local-only external_binding. A managed object is a single runtime value behind one write guard, so two mutable accesses to its inline fields conflict; the conflict root is the managed object base.",
    )
    .with_fix(
        "split_managed_field_accesses",
        "Split the accesses into separate statements, or move the fields behind explicit `handle` fields so they become distinct managed objects.",
        "manual",
    )
}

/// Diagnose mixing a whole local place with one of its fields in a call.
pub fn field_partial_access_conflict_diagnostic(left: &str, right: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::FIELD_PARTIAL_ACCESS_CONFLICT,
        format!("call mixes whole local access `{left}` with field access `{right}`."),
        span,
        "whole-base field conflict",
    )
    .with_cause("A whole local base or prefix conflicts with a mutable or taking subpath in the same call.")
    .with_fix(
        "split_call",
        "Split the whole-base read and field mutation into separate statements or pass disjoint fields explicitly.",
        "manual",
    )
}

/// Diagnose two non-disjoint local field paths.
pub fn field_prefix_conflict_diagnostic(
    left: &str,
    right: &str,
    cause: &str,
    span: Span,
) -> Diagnostic {
    Diagnostic::error(
        code::FIELD_PREFIX_CONFLICT,
        format!("local field paths `{left}` and `{right}` are not disjoint."),
        span,
        "field path conflict",
    )
    .with_cause(cause)
    .with_fix(
        "split_or_refactor_paths",
        "Split the accesses into separate calls or refactor through explicit split APIs.",
        "manual",
    )
}

/// Diagnose indexed local paths whose disjointness cannot be proven.
pub fn indexed_place_conflict_diagnostic(left: &str, right: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::INDEXED_PARTIAL_ACCESS_CONFLICT,
        format!("indexed local paths `{left}` and `{right}` cannot be proven disjoint."),
        span,
        "indexed local access conflict",
    )
    .with_cause(
        "RSScript v0.7 treats indexed access as access to the whole local container for alias checking.",
    )
    .with_fix(
        "use_split_api",
        "Use an explicit container split API that proves or checks disjoint element access.",
        "manual",
    )
}

/// Diagnose moving a local place while another access uses its base or field.
pub fn move_base_field_conflict_diagnostic(moved: &str, accessed: &str, span: Span) -> Diagnostic {
    Diagnostic::error(
        code::MOVE_BASE_FIELD_CONFLICT,
        format!("call moves local path `{moved}` while also accessing `{accessed}`."),
        span,
        "move-base field conflict",
    )
    .with_cause(
        "A local base cannot be `manage`d or `take`n in the same expression where one of its fields is accessed.",
    )
    .with_fix(
        "split_move_from_field_access",
        "Split the field access and `manage`/`take` into separate statements.",
        "manual",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span() -> Span {
        Span {
            file: "place.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn derives_place_conflict_diagnostics_from_resolved_paths() {
        assert_eq!(
            managed_field_split_conflict_diagnostic("item.left", "item.right", span()).code,
            code::MANAGED_FIELD_SPLIT_CONFLICT
        );
        assert_eq!(
            field_partial_access_conflict_diagnostic("item", "item.left", span()).code,
            code::FIELD_PARTIAL_ACCESS_CONFLICT
        );
        assert_eq!(
            field_prefix_conflict_diagnostic("item.left", "item.left.id", "prefix", span()).code,
            code::FIELD_PREFIX_CONFLICT
        );
        assert_eq!(
            indexed_place_conflict_diagnostic("items[...]", "items[...]", span()).code,
            code::INDEXED_PARTIAL_ACCESS_CONFLICT
        );
        assert_eq!(
            move_base_field_conflict_diagnostic("item", "item.left", span()).code,
            code::MOVE_BASE_FIELD_CONFLICT
        );
    }
}
