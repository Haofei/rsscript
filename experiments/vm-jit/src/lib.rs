//! Native (Cranelift) baseline JIT for the RSScript register VM's numeric /
//! boolean / control-flow core — the native tier of
//! `docs/spec/RSScript_Execution_Spec_v0.1.md` (§7; status in Appendix B).
//!
//! # What it compiles
//!
//! A [`JitFunction`] is a stable, versioned slice of the VM's bytecode: the subset
//! that operates on unboxed scalar registers — logical `Int`/`Bool` values stored
//! in `i64` machine words and `Float` values stored in `f64` — plus heap reads,
//! native-to-native calls, and a declared set of VM-owned transactional heap
//! helpers. It has no arbitrary host mutation or async execution. The main `rsscript` crate
//! translates an eligible `RegFunction` into this IR; everything outside the subset
//! stays on the interpreter (per-function fallback). Public IR crosses the sealed
//! [`ValidatedJitFunction`] boundary before codegen, so a malformed producer fails
//! as a clean [`JitError`] rather than panicking or miscompiling.
//!
//! # Why a separate crate
//!
//! `rsscript` is `#![forbid(unsafe_code)]`. Executing generated machine code and
//! transmuting a code pointer to a callable function require `unsafe`, so they
//! live here behind safe scalar and borrow-checked flat-buffer APIs. Raw ABI calls
//! remain crate-controlled `unsafe` boundaries; generated pointers can only come
//! from live Rust slices/references held for the duration of the call.
//!
//! # Gap-freeness
//!
//! Integer arithmetic in RSScript is *checked* (overflow, divide/modulo by zero
//! are language-level runtime errors). Rather than reproduce those error paths in
//! native code, the generated function **bails** (returns "not completed") on any
//! such edge — overflow, division by zero, `i64::MIN / -1`, or an out-of-range
//! shift — and likewise on a heap read the helper can't satisfy (wrong type or out
//! of bounds, signalled via the bail flag). Float arithmetic never traps (it
//! mirrors the interpreter's `f64` semantics, NaN/±inf included), so it needs no
//! bail. Native writes are journaled by the embedding VM: success commits, while
//! bailout/error restores heap and mutable-flat state before interpreter replay.
//! Precise resume is admitted only when that transaction state is compatible with
//! the safepoint. The interpreter remains the semantic source of truth.

mod analysis;
mod executable_memory;
mod validated;

#[cfg(test)]
use analysis::Interval;
use analysis::{arith_cannot_overflow, definite_assignment, interval_analysis, list_bounds_plan};
use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, Block, InstBuilder, MemFlags, StackSlotData, StackSlotKind, Value, types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
pub use executable_memory::ExecutableMemoryBudget;
use executable_memory::{ExecutableMemoryReservation, arena_allocation_charge};
pub use validated::{ValidatedJitFunction, validate_function};

include!("host_abi.rs");
include!("ir.rs");
include!("module.rs");
include!("ir_validation.rs");
include!("codegen.rs");
include!("tests.rs");
