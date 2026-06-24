//! Native-JIT (Cranelift) translation and optimization passes, split out of
//! `reg_vm::mod` as a cfg-gated submodule (pure code-movement, Phase 2).
use super::*;

mod passes;
mod translate;

pub(super) use passes::*;
pub(super) use translate::*;
