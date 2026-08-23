//! Native-JIT (Cranelift) translation and optimization passes, split out of
//! `reg_vm::mod` as a cfg-gated submodule (pure code-movement, Phase 2).
use super::*;

mod facts;
mod passes;
mod profitability;
mod translate;
mod typed_region;

pub(in crate::reg_vm) use facts::*;
pub(super) use passes::*;
pub(super) use profitability::*;
pub(super) use translate::*;
pub(in crate::reg_vm) use typed_region::*;
