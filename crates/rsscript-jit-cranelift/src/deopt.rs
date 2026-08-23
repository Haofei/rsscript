use super::*;

/// Identity of a deopt (bail) point in a compiled function. Codegen assigns every
/// distinct guard/bail site a unique id numbered from 1 (see `build_function`);
/// the generated code stores it into the host's safepoint cell on the bail edge,
/// and [`NativeModule::call`] surfaces it as [`NativeOutcome::Deopt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SafepointId(pub u32);

impl SafepointId {
    /// Reserved id `0`: no bail was recorded (the call either fell through to
    /// completion or bailed before any site stored its id). Real bail sites are
    /// numbered from `1`.
    pub const ANONYMOUS: SafepointId = SafepointId(0);
}

/// The deopt state for one safepoint: where the interpreter must resume and which
/// registers carry live state into that resume point.
///
/// `resume_ip` is the [`JitInstr`] index the interpreter re-executes when this
/// guard fires. It is the very instruction whose guard bailed: native code bails
/// *before* completing that instruction (e.g. before storing an `Add`'s checked
/// result), so the interpreter must run it again. Its inputs are therefore exactly
/// the registers definitely assigned on entry to that instruction.
///
/// `live` lists those entry-assigned registers (definite-assignment / "must"
/// analysis — see [`definite_assignment`]), each paired with its storage class. A
/// register absent from `live` is not guaranteed assigned on every path to the
/// resume point, so it carries no meaningful value to reconstruct.
///
/// Generated code captures these registers into the call frame's deoptimization
/// payload. The embedding VM consumes the map to reconstruct a precise interpreter
/// resume window after transactional rollback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeoptSite {
    /// The `JitInstr` index to resume interpretation at (the bailing instruction).
    pub resume_ip: u32,
    /// Registers definitely assigned on entry to `resume_ip`, each `(reg, type)`.
    pub live: Vec<(u32, JitValueType)>,
    /// If this safepoint is a native-call edge, the callee's deopt payload is
    /// chained into this function's payload buffer so the host can inspect the
    /// complete native call stack.
    pub child: Option<DeoptChildSite>,
}

/// Location of a child native frame's deopt payload inside its caller's payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeoptChildSite {
    /// The compiled callee that produced the nested payload.
    pub callee: CompiledId,
    /// Slot containing the child [`SafepointId`] as an `i64`.
    pub safepoint_slot: u32,
    /// First slot of the child's payload buffer, copied verbatim from the child
    /// call. The child deopt map interprets this region.
    pub payload_slot: u32,
    /// Number of payload words copied from the child call.
    pub payload_words: u32,
}

/// Per-function deopt state-map, indexed by safepoint id.
///
/// Codegen mints safepoint ids from `1` (id `0` is [`SafepointId::ANONYMOUS`]), one
/// per [`bail_if`] call in emission order. This map mirrors that numbering with a
/// **0-based** vector: `sites[id - 1]` is the [`DeoptSite`] for `safepoint_id == id`
/// (so `sites[0]` is id `1`). The alignment is structural — codegen pushes exactly
/// one [`DeoptSite`] per `bail_if` call, in the same traversal that increments the
/// id counter — so indices never drift from ids.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeoptMap {
    /// `sites[id - 1]` is the site for `safepoint_id == id` (ids start at 1).
    pub sites: Vec<DeoptSite>,
    /// Total width of the deopt payload buffer required by this function: its own
    /// register window plus any chained child native-call payload regions.
    pub payload_words: usize,
}

/// The runtime value of a live register captured at a deopt, typed by its storage
/// class so the caller can reconstruct it faithfully (an `i64` integer/boolean, or
/// an exact `f64`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeoptValue {
    /// An integer register's value.
    Int(i64),
    /// A logical boolean register's value.
    Bool(bool),
    /// A float register's value (decoded from its captured 8-byte bit pattern).
    Float(f64),
    /// A `Handle` register's captured heap-table index. Carries no VM value by itself;
    /// the consumer resolves the index against the still-live JIT heap during
    /// heap-payload reconstruction. It is never written back as a raw scalar.
    Handle(i64),
}

/// One live register's captured value at a deopt: its register index plus the
/// decoded [`DeoptValue`]. See [`NativeOutcome::Deopt`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeoptReg {
    /// The VM register index.
    pub reg: u32,
    /// The register's value at the bail edge.
    pub value: DeoptValue,
}

/// A nested native frame captured when a `CallNative` callee deopts. The top-level
/// [`NativeOutcome::Deopt`] still names the caller safepoint; this chain preserves
/// the callee safepoint and live payload so embedders can build a full native-frame
/// deopt later instead of losing the child frame at the call edge.
#[derive(Debug, Clone, PartialEq)]
pub struct DeoptFrame {
    /// Compiled function whose frame deopted.
    pub function: CompiledId,
    /// Safepoint in `function`.
    pub safepoint_id: SafepointId,
    /// Live registers decoded with `function`'s [`DeoptMap`].
    pub live: Vec<DeoptReg>,
    /// Further nested child, when native calls are chained.
    pub child: Option<Box<DeoptFrame>>,
}

/// Outcome of running a compiled function via [`NativeModule::call`]: either the
/// function ran to completion with a 64-bit result, or it deopted at a named
/// safepoint and the interpreter should re-run it.
#[derive(Debug, Clone, PartialEq)]
pub enum NativeOutcome {
    /// The function completed; the payload is the result bits (an `i64`, or an
    /// `f64` bit pattern for a float-returning function).
    Completed(i64),
    /// The function completed and its result is a **heap value** (a struct/list),
    /// not a scalar. The payload is an **opaque output-table handle**: the host
    /// materializes the actual [`VmValue`] (host-side type) from its VM-owned output
    /// table at this index. Emitted only on a clean completion (bail flag clear) of
    /// a function whose return register is a [`JitValueType::Handle`]; the scalar
    /// [`Completed`](NativeOutcome::Completed) path is byte-for-byte unchanged.
    ///
    /// The host materializes the result **only** on this clean
    /// completion; **any** bail returns [`Deopt`](NativeOutcome::Deopt) and the
    /// output table is cleared by the VM-side guard, so a bailed attempt has no
    /// observable heap result, preserving transactional fallback equivalence.
    CompletedHandle(i64),
    /// The function deopted at `safepoint_id` (a guard bail or a host-helper bail)
    /// and the caller must fall back to the interpreter. `live` carries each
    /// register definitely assigned at the resume point with its captured value
    /// according to the function's state map; it is empty for a deopt rejected
    /// before the call (id/length mismatch). The embedding VM rolls back
    /// transactional writes, consumes `live`, reconstructs the interpreter window,
    /// and resumes at the safepoint's `resume_ip`. `child` is populated when this deopt came
    /// from a nested [`JitInstr::CallNative`] callee; embedders that support full
    /// native frame-chain deopt can inspect it, while conservative embedders may
    /// still resume/re-run at this caller safepoint.
    Deopt {
        safepoint_id: SafepointId,
        live: Vec<DeoptReg>,
        child: Option<Box<DeoptFrame>>,
        /// Final logical call depth at an OSR exit. Embedders must commit this only
        /// after validating the designated `OsrExit`; ordinary guard bails replay
        /// the interpreter and therefore discard it.
        logical_depth: Option<usize>,
        /// A typed pre-execution decline. `None` means generated code actually
        /// entered and deoptimized (or an ordinary shape check requested fallback).
        /// Reentrant entry is reported explicitly so hosts do not mistake it for a
        /// resumable safepoint.
        decline: Option<NativeDeclineReason>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeDeclineReason {
    ReentrantCall,
    AbiMismatch,
}

pub(super) fn anonymous_deopt() -> NativeOutcome {
    NativeOutcome::Deopt {
        safepoint_id: SafepointId::ANONYMOUS,
        live: Vec::new(),
        child: None,
        logical_depth: None,
        decline: None,
    }
}

pub(super) fn abi_mismatch_decline() -> NativeOutcome {
    NativeOutcome::Deopt {
        safepoint_id: SafepointId::ANONYMOUS,
        live: Vec::new(),
        child: None,
        logical_depth: None,
        decline: Some(NativeDeclineReason::AbiMismatch),
    }
}

pub(super) fn reentrant_decline() -> NativeOutcome {
    NativeOutcome::Deopt {
        safepoint_id: SafepointId::ANONYMOUS,
        live: Vec::new(),
        child: None,
        logical_depth: None,
        decline: Some(NativeDeclineReason::ReentrantCall),
    }
}
