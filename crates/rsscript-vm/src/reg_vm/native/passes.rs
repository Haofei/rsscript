//! Native-JIT analysis, eligibility, fold, scalar-replacement, inlining and
//! closure-sink passes.
#![allow(
    unused_imports,
    clippy::doc_lazy_continuation,
    clippy::needless_range_loop,
    clippy::ptr_arg,
    clippy::too_many_arguments,
    clippy::type_complexity
)]

use std::collections::BTreeSet;

use super::super::*;
use super::*;

mod facts;
mod inlining;
mod intrinsics;
mod region_optimization;
mod scalar_replacement;
mod semantics;

pub(in crate::reg_vm) use facts::*;
pub(in crate::reg_vm) use inlining::*;
pub(in crate::reg_vm) use intrinsics::*;
pub(in crate::reg_vm) use region_optimization::*;
pub(in crate::reg_vm) use scalar_replacement::*;
pub(in crate::reg_vm) use semantics::*;

#[cfg(all(test, feature = "native-jit"))]
mod architecture_tests {
    #[test]
    fn native_passes_are_partitioned_by_invariant() {
        let root = include_str!("passes.rs");
        let facts = include_str!("passes/facts.rs");
        let semantics = include_str!("passes/semantics.rs");
        let scalar_replacement = include_str!("passes/scalar_replacement.rs");
        let inlining = include_str!("passes/inlining.rs");

        assert!(root.contains("mod facts;"));
        assert!(root.contains("mod semantics;"));
        assert!(root.contains("mod scalar_replacement;"));
        assert!(root.contains("mod inlining;"));
        let legacy_include = ["include", "!("].concat();
        assert!(!root.contains(&legacy_include));
        assert!(facts.contains("enum NativeFact"));
        assert!(semantics.contains("struct NativeInstrSemantics"));
        assert!(scalar_replacement.contains("native_scalar_replace_two_armed_results_in_region"));
        assert!(inlining.contains("native_inline_leaf_calls"));
    }
}
