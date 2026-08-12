//! Backend-neutral structured control-flow exit states.

/// How a checked statement or block leaves its enclosing structured scope.
/// CFG construction and semantic diagnostics share this state without either
/// layer depending on a compiler-private enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Fallthrough,
    Return,
    Break,
    Continue,
}

/// Merge two non-fallthrough exits. Identical exits remain precise; different
/// exits both prevent ordinary fallthrough and are represented by `Return` as
/// the conservative legacy projection used by block checking.
pub fn merge_non_fallthrough(left: Flow, right: Flow) -> Flow {
    if left == right { left } else { Flow::Return }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserving_identical_exits_and_conservatively_merging_different_ones() {
        assert_eq!(merge_non_fallthrough(Flow::Break, Flow::Break), Flow::Break);
        assert_eq!(
            merge_non_fallthrough(Flow::Break, Flow::Continue),
            Flow::Return
        );
    }
}
