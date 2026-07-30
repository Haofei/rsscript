//! Native-JIT analysis, eligibility, fold, scalar-replacement, inlining and
//! closure-sink passes. Pure code-movement out of `reg_vm::mod` (Phase 2);
//! every item retains its original `#[cfg(feature = "native-jit")]` attribute
//! verbatim (now redundant under the cfg-gated `native` module, but harmless).
#![allow(unused_imports)]

use super::super::*;
use super::*;

include!("passes/intrinsics.rs");
include!("passes/facts.rs");
include!("passes/semantics.rs");
include!("passes/region_optimization.rs");
include!("passes/scalar_replacement.rs");
include!("passes/inlining.rs");

#[cfg(all(test, feature = "native-jit"))]
mod architecture_tests {
    #[test]
    fn native_passes_are_partitioned_by_invariant() {
        let root = include_str!("passes.rs");
        let facts = include_str!("passes/facts.rs");
        let semantics = include_str!("passes/semantics.rs");
        let scalar_replacement = include_str!("passes/scalar_replacement.rs");
        let inlining = include_str!("passes/inlining.rs");

        assert!(root.contains("include!(\"passes/facts.rs\")"));
        assert!(root.contains("include!(\"passes/semantics.rs\")"));
        assert!(root.contains("include!(\"passes/scalar_replacement.rs\")"));
        assert!(root.contains("include!(\"passes/inlining.rs\")"));
        assert!(facts.contains("enum NativeFact"));
        assert!(semantics.contains("struct NativeInstrSemantics"));
        assert!(scalar_replacement.contains("native_scalar_replace_two_armed_results_in_region"));
        assert!(inlining.contains("native_inline_leaf_calls"));
    }
}
