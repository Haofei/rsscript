//! Native (Cranelift) JIT for the RSScript register VM's trusted in-process
//! acceleration path. Its durable embedding and fallback rules are specified in
//! `docs/spec/native-jit-contract.md`.
//!
//! # What it compiles
//!
//! A [`JitFunction`] is a process-local, lockstep representation of the VM subset
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

#![deny(unsafe_op_in_unsafe_fn)]
#![deny(clippy::undocumented_unsafe_blocks)]
#![deny(clippy::missing_safety_doc)]

mod analysis;
mod codegen;
mod deopt;
mod direct_codegen;
mod executable_memory;
#[cfg(feature = "fuzzing")]
pub mod fuzzing;
mod host_abi;
mod ir;
mod ir_validation;
mod limits;
mod module;
mod validated;

#[cfg(test)]
use analysis::Interval;
use analysis::{
    arith_cannot_overflow, definite_assignment, interval_analysis, list_bounds_plan,
    register_liveness,
};
use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    AbiParam, Block, InstBuilder, MemFlags, StackSlotData, StackSlotKind, Value, types,
};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{ArenaMemoryProvider, JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};
use deopt::{DeoptChildSite, DeoptMap, DeoptSite};
pub use deopt::{DeoptFrame, DeoptReg, DeoptValue, NativeOutcome, SafepointId};
pub use executable_memory::ExecutableMemoryBudget;
use executable_memory::{ExecutableMemoryReservation, arena_allocation_charge};
use host_abi::{
    CALL_FRAME_SIZE, FRAME_ABI_VERSION, FRAME_ARG_COUNT, FRAME_ARGS, FRAME_BAIL, FRAME_DEOPT,
    FRAME_FLAGS, FRAME_HOST_CTX, FRAME_LENS, FRAME_LIMITS, FRAME_LOGICAL_DEPTH,
    FRAME_LOGICAL_DEPTH_LIMIT, FRAME_NATIVE_DEPTH, FRAME_RESULT, FRAME_SAFEPOINT, FRAME_SIZE,
    HostFailureMode, JIT_CALL_ABI_VERSION, JitCallFrame, JitStatus,
};
pub use host_abi::{
    FlatBufferArg, HostCtx, HostHeapEffect, HostHeapProjection, HostHelper, HostHelpers,
    IndexedFlatBufferArg,
};
pub use ir::{
    FloatRounding, HostArg, JitCompare, JitControlFlow, JitFunction, JitInstr,
    JitInstructionOrigin, JitValueType, MemoScope,
};
pub use limits::JitLimits;
pub use module::{
    CompiledId, JitError, JitErrorKind, LogicalCallDepth, NativeDeclineReason, NativeModule,
    PreparedCall, RegionCallControls, is_native_callable_leaf, signal_bail, user_host_ctx,
};
pub use validated::{ValidatedJitFunction, validate_function};

// Unit tests exercise the crate-private ABI layout and code-generation contract.
// These imports deliberately stay non-public so production consumers cannot grow
// accidental dependencies on raw frame offsets or helper function aliases.
#[cfg(test)]
use host_abi::*;
#[cfg(test)]
use ir::*;

pub(crate) use codegen::{
    LimitChecks, build_function, native_recursion_depth_cap, push_compiled_abi_signature,
};
#[cfg(feature = "recursion")]
pub(crate) use codegen::{
    NATIVE_RECURSION_STACK_BUDGET_BYTES, native_recursion_frame_bytes_estimate,
};
pub(crate) use direct_codegen::{
    build_direct_scalar_frame_wrapper, build_direct_scalar_function, direct_scalar_callable,
    push_direct_scalar_signature,
};
pub(crate) use host_abi::{DEFAULT_STANDALONE_JIT_ARENA_BYTES, HostHelperSig, HostResult};
#[cfg(test)]
pub(crate) use ir_validation::validate;
pub(crate) use ir_validation::{instr_def, reachable_jit_instrs, successors, validate_with_limits};
pub(crate) use module::{ForcedDeopt, HostFuncs, NativeCallee, NativeGroupMember, is_flat_type};

#[cfg(test)]
mod tests;
