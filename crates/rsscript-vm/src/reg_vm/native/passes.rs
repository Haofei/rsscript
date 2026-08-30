//! Native-JIT analysis, eligibility, fold, scalar-replacement, inlining and
//! closure-sink passes.
use std::collections::BTreeSet;

use super::super::*;
use super::*;

mod bytes_fold;
mod facts;
mod inlining;
mod intrinsics;
mod region_optimization;
mod scalar_replacement;
mod semantics;
mod virtual_objects;

/// Marks instruction-position loops that intentionally coordinate multiple
/// parallel IP-indexed fact tables. A single-slice iterator would hide that
/// shared identity; keeping the range explicit makes remapping invariants clear.
pub(in crate::reg_vm) fn parallel_indices<I>(indices: I) -> I
where
    I: Iterator<Item = usize>,
{
    indices
}

pub(in crate::reg_vm) use bytes_fold::*;
pub(in crate::reg_vm) use facts::*;
pub(in crate::reg_vm) use inlining::*;
pub(in crate::reg_vm) use intrinsics::*;
pub(in crate::reg_vm) use region_optimization::*;
pub(in crate::reg_vm) use scalar_replacement::*;
pub(in crate::reg_vm) use semantics::*;
pub(in crate::reg_vm) use virtual_objects::*;

#[cfg(all(test, feature = "native-jit"))]
mod architecture_tests {
    #[test]
    fn native_passes_are_partitioned_by_invariant() {
        let root = include_str!("passes.rs");
        let bytes_fold = include_str!("passes/bytes_fold.rs");
        let facts = include_str!("passes/facts.rs");
        let semantics = include_str!("passes/semantics.rs");
        let scalar_replacement = include_str!("passes/scalar_replacement.rs");
        let inlining = include_str!("passes/inlining.rs");
        let virtual_objects = include_str!("passes/virtual_objects.rs");

        assert!(root.contains("mod bytes_fold;"));
        assert!(root.contains("mod facts;"));
        assert!(root.contains("mod semantics;"));
        assert!(root.contains("mod scalar_replacement;"));
        assert!(root.contains("mod inlining;"));
        assert!(root.contains("mod virtual_objects;"));
        let legacy_include = ["include", "!("].concat();
        assert!(!root.contains(&legacy_include));
        assert!(bytes_fold.contains("native_bytes_length_fold_in_region"));
        assert!(facts.contains("enum NativeFact"));
        assert!(semantics.contains("struct NativeInstrSemantics"));
        assert!(scalar_replacement.contains("native_scalar_replace_two_armed_results_in_region"));
        assert!(inlining.contains("native_inline_leaf_calls"));
        assert!(virtual_objects.contains("struct VirtualObjectAnalysis"));
    }
}
