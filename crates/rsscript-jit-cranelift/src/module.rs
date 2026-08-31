/// The native ABI of every compiled function is one pointer to a versioned
/// [`JitCallFrame`]. It returns [`JitStatus::Completed`] and writes through
/// `frame.result` on success, or [`JitStatus::Deopt`] to request fallback.
/// `frame.lens` points at an `i64` array parallel to `frame.args`: for a flat-array
/// parameter (`FlatInt`/`FlatFloat`) the argument word holds
/// the raw data pointer and the `lens` word holds the element count (for in-register
/// bounds-checked direct reads); other params' `lens` words are unused. `frame.bail`
/// points at a `u8` flag the host helpers set when a heap read can't be satisfied;
/// the generated code loads it after every helper call and branches to fallback
/// immediately, so a bad read can't keep executing. `safepoint_ptr` points at a
/// host-owned `i64` cell into which the generated code *stores* the unique
/// [`SafepointId`] of the bail site on the bail edge (and only there - the hot
/// fall-through path never touches it); `0` means no bail was recorded.
/// `payload_ptr` points at a host-owned `i64` array of width
/// `deopt_map.payload_words` into which the generated code *stores* each live
/// register's value on the bail edge only (slot `reg` receives that register's
/// 8-byte word). Native-call bail edges also chain the callee safepoint id and
/// payload after the caller register window.
pub(crate) type CompiledAbi = unsafe extern "C" fn(*mut JitCallFrame) -> JitStatus;

mod call;

/// Stable classification for native-tier failures. Hosts use the kind to decide
/// whether interpreter fallback is expected or the module must be quarantined;
/// human-readable text is diagnostic detail only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitErrorKind {
    InvalidIr,
    UnsupportedInstruction,
    UnsupportedAbi,
    WrongModule,
    InvalidCompiledId,
    AdmissionRejected,
    CodegenFailed,
    FinalizationFailed,
    ReentrantCall,
    UnsafeArgument,
    InternalInvariant,
}

#[derive(Debug)]
pub struct JitError {
    pub kind: JitErrorKind,
    pub message: String,
}

impl JitError {
    pub fn new(kind: JitErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn invalid_ir(message: impl Into<String>) -> Self {
        Self::new(JitErrorKind::InvalidIr, message)
    }
}

/// Logical language-call depth supplied by an embedding VM. Native stack depth
/// is tracked separately by the internal ABI.
#[derive(Clone, Copy, Debug)]
pub struct LogicalCallDepth {
    pub current: usize,
    pub limit: usize,
}

impl std::fmt::Display for JitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "native JIT {:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for JitError {}

fn err(context: &str, e: impl std::fmt::Display) -> JitError {
    let kind = if context == "finalize" {
        JitErrorKind::FinalizationFailed
    } else {
        JitErrorKind::CodegenFailed
    };
    JitError::new(kind, format!("{context}: {e}"))
}

fn native_scalar_leaf_callable(function: &JitFunction, osr: bool, _returns_handle: bool) -> bool {
    !osr && function.reg_types.iter().all(|ty| {
        matches!(
            ty,
            JitValueType::Int
                | JitValueType::Bool
                | JitValueType::Float
                | JitValueType::Handle
                | JitValueType::FlatInt
                | JitValueType::FlatIntMut
                | JitValueType::FlatFloat
                | JitValueType::FlatFloatMut
        )
    }) && function
        .code
        .iter()
        .all(|instr| instr.descriptor().native_leaf)
}

/// Whether a native callee can use the compact scalar child-frame path.
///
/// This deliberately does **not** claim to be a raw/direct scalar ABI. Checked
/// arithmetic can still deopt, so a child safepoint and payload remain necessary
/// for precise nested reconstruction. The compact path only removes state which
/// is provably unused by a scalar, helper-free leaf: flat-buffer lengths and the
/// shared host-helper bail flag load.
fn native_compact_scalar_frame_callable(
    function: &JitFunction,
    osr: bool,
    returns_handle: bool,
) -> bool {
    !osr && !returns_handle
        && function.reg_types.iter().all(|ty| {
            matches!(
                ty,
                JitValueType::Int | JitValueType::Bool | JitValueType::Float
            )
        })
        && function
            .code
            .iter()
            .all(|instr| instr.descriptor().compact_scalar_frame)
}

/// Whether a normal (non-OSR) function can participate in the scalar native-call
/// ABI. Keep this predicate in vm-jit so tiering and compilation cannot drift.
pub fn is_native_callable_leaf(function: &JitFunction) -> bool {
    native_scalar_leaf_callable(function, false, false)
}

/// Declare an imported host helper with one opaque `HostCtx` word plus `n_args`
/// logical `i64` params and an `i64` result.
/// The Cranelift ABI type carrying a logical [`JitValueType`] across the host-helper
/// boundary: a `Float` rides the native `f64` register, everything else (Int/Bool/
/// Handle/flat-array handle) rides an `i64`. The bail signal is always out-of-band
/// (the shared bail flag), so the value channel carries only the value.
fn host_abi_type(ty: JitValueType) -> cranelift_codegen::ir::Type {
    match ty {
        JitValueType::Float => types::F64,
        _ => types::I64,
    }
}

/// Declare an imported host helper from its signature: one opaque `HostCtx` word
/// followed by one param per declared arg type (`Float` → `f64`, else `i64`), an
/// optional private `found_out` pointer, and a result typed from the declared result
/// (`Float` → `f64`, else `i64`). Deriving the ABI from the declared types — rather
/// than assuming all-`i64` — is what lets a helper take a `Float` argument (e.g.
/// `FieldSetFloat`), not just return one.
fn declare_import_for(
    module: &mut JITModule,
    name: &str,
    sig: &HostHelperSig,
) -> Result<FuncId, JitError> {
    let mut cl_sig = module.make_signature();
    cl_sig.params.push(AbiParam::new(types::I64)); // HostCtx
    for arg in sig.args {
        cl_sig.params.push(AbiParam::new(host_abi_type(*arg)));
    }
    if sig.found_out {
        cl_sig
            .params
            .push(AbiParam::new(module.target_config().pointer_type()));
    }
    let ret = match sig.result {
        HostResult::Exact(JitValueType::Float) => types::F64,
        _ => types::I64,
    };
    cl_sig.returns.push(AbiParam::new(ret));
    module
        .declare_function(name, Linkage::Import, &cl_sig)
        .map_err(|e| err("declare import", e))
}

/// A compiled function plus the metadata `call` needs to invoke it safely: the
/// param count, so `call` can reject an argument slice of the wrong length (the
/// generated entry block reads exactly `n_params` words from `args_ptr` and does
/// not bound-check against `n_args`).
struct CompiledFunc {
    f: CompiledAbi,
    id: FuncId,
    /// Optional frame-free entry used only by infallible scalar native callers.
    /// The public/top-level entry always remains `f`/`JitCallFrame`.
    direct_scalar_id: Option<FuncId>,
    /// Machine-code bytes emitted by Cranelift for this function, including any
    /// constant data reported by `CompiledCode::code_info`.
    code_size_bytes: u64,
    /// Direct flat-list bounds checks removed by the codegen-only range and
    /// provenance proof for this compiled function.
    direct_list_bounds_checks_elided: u64,
    /// Longest native-to-native call chain reachable from this function. A function
    /// with no `CallNative` edges has depth 0; a direct native leaf call has depth 1.
    native_call_depth: u32,
    /// Conservative host-stack depth cap derived from this function's frame shape.
    /// A top-level acyclic chain is checked once before entering machine code;
    /// recursive entries retain a generated dynamic guard.
    native_depth_cap: u32,
    n_params: usize,
    /// Register count of the source [`JitFunction`] (the width of each site's
    /// register space).
    n_regs: usize,
    /// Per-safepoint deopt state-map (resume_ip + live registers), built host-side
    /// during `compile`. See [`DeoptMap`].
    deopt_map: DeoptMap,
    /// Whether generated code reads and writes the non-null limits cell passed to
    /// the raw entry ABI. Ordinary safe calls must never enter such a function.
    requires_limits: bool,
    limit_checks: LimitChecks,
    /// OSR-entry (OSR): when `true`, the function was compiled with
    /// [`compile_osr`](NativeModule::compile_osr). Its `args_ptr` is the
    /// interpreter's full `n_regs`-wide register *window* (indexed by register),
    /// not a packed `n_params` arg array, so `call` validates the slice length
    /// against `n_regs` instead of `n_params`.
    osr: bool,
    /// Heap-result return ABI: `true` when this function's return
    /// register is a [`JitValueType::Handle`], so its completed `i64` result is an
    /// **output-table handle** (the host materializes a heap [`VmValue`] from it)
    /// rather than a scalar. Computed once at compile time from the function's
    /// `Return` source register type. A scalar-returning function has this `false`
    /// and takes the unchanged [`NativeOutcome::Completed`] path. An OSR function
    /// never has a top-level `Return` (it exits via `OsrExit`), so it is `false`.
    returns_handle: bool,
    param_types: Vec<JitValueType>,
    reg_types: Vec<JitValueType>,
    return_type: Option<JitValueType>,
    scalar_leaf_callable: bool,
    /// A helper-free scalar leaf can omit the flat-length window and helper-bail
    /// read in its parent call site. It still uses a versioned `JitCallFrame` and
    /// child deopt payload because checked scalar operations may deopt precisely.
    compact_scalar_frame_callable: bool,
    direct_scalar_call_edges: u32,
}

#[derive(Clone)]
pub(crate) struct NativeCallee {
    pub(crate) handle: CompiledId,
    pub(crate) func_id: FuncId,
    pub(crate) n_params: usize,
    pub(crate) param_types: Vec<JitValueType>,
    pub(crate) deopt_payload_words: usize,
    pub(crate) return_type: JitValueType,
    pub(crate) compact_scalar_frame: bool,
    pub(crate) direct_scalar_func_id: Option<FuncId>,
}


#[derive(Clone)]
pub(crate) struct NativeGroupMember;

/// Process-wide source of per-module identities, so a [`CompiledId`] minted by one
/// [`NativeModule`] is rejected by another (it would otherwise index a different
/// module's function table). Monotonic; wraparound is not a practical concern.
static NEXT_MODULE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn flat_proof_matches(
    arg: &mut FlatBufferArg<'_>,
    expected_type: JitValueType,
    expected_ptr: Option<&i64>,
    expected_len: Option<&i64>,
) -> bool {
    let (ptr, len, compatible) = match arg {
        FlatBufferArg::Int(values) => (
            values.as_ptr() as i64,
            values.len() as i64,
            expected_type == JitValueType::FlatInt,
        ),
        FlatBufferArg::IntMut(values) => (
            values.as_mut_ptr() as i64,
            values.len() as i64,
            expected_type == JitValueType::FlatIntMut,
        ),
        FlatBufferArg::Float(values) => (
            values.as_ptr() as i64,
            values.len() as i64,
            expected_type == JitValueType::FlatFloat,
        ),
        FlatBufferArg::FloatMut(values) => (
            values.as_mut_ptr() as i64,
            values.len() as i64,
            expected_type == JitValueType::FlatFloatMut,
        ),
    };
    compatible && expected_ptr == Some(&ptr) && expected_len == Some(&len)
}

fn copy_session_yield_registers(
    function: &CompiledFunc,
    session: &NativeCallSession,
    window: &mut [i64],
) -> bool {
    if window.len() != function.n_regs || session.deopt_payload.len() < function.n_regs {
        return false;
    }
    window.copy_from_slice(&session.deopt_payload[..function.n_regs]);
    true
}

/// Owns the JIT-compiled machine code. Compiled functions live as long as the
/// module, so callers keep this alive and invoke by [`CompiledId`].
pub struct NativeModule {
    module: JITModule,
    ctx: Context,
    fbctx: FunctionBuilderContext,
    funcs: Vec<CompiledFunc>,
    counter: u32,
    /// Identity stamped into every [`CompiledId`] this module mints (see
    /// [`NEXT_MODULE_ID`]).
    id: u64,
    /// Declared host-helper imports (see [`HostHelpers`]).
    imports: HostFuncs,
    limits: JitLimits,
    /// Cumulative successful compilation-phase timings. Validation is recorded by
    /// the sealed validation entries; code generation excludes final publication.
    phase_timings: std::cell::Cell<CompilePhaseTimings>,
    /// Keeps the shared hard-budget reservation alive for the arena's lifetime.
    _memory_reservation: ExecutableMemoryReservation,
}

/// Backend phase timings kept separate from VM-side bytecode translation. These
/// counters are diagnostic only and are updated only at compile boundaries, never
/// on a native execution hot path.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompilePhaseTimings {
    pub validation_nanos: u128,
    pub codegen_nanos: u128,
    pub finalize_nanos: u128,
}

impl CompilePhaseTimings {
    fn add_validation(&mut self, nanos: u128) {
        self.validation_nanos = self.validation_nanos.saturating_add(nanos);
    }

    fn add_codegen(&mut self, nanos: u128) {
        self.codegen_nanos = self.codegen_nanos.saturating_add(nanos);
    }

    fn add_finalize(&mut self, nanos: u128) {
        self.finalize_nanos = self.finalize_nanos.saturating_add(nanos);
    }
}

/// Per-activation scratch owned by the caller rather than the executable-code
/// container. A session may be reused for sequential calls; it never retains VM
/// values, host contexts, or generated pointers after a call returns.
///
/// Keeping deopt/yield payloads here makes published machine code and its metadata
/// independent from mutable execution state. It also lets a normal region yield
/// copy its live-out words before the session is reused, without consulting
/// "the most recent call" state on [`NativeModule`].
#[derive(Default)]
pub struct NativeCallSession {
    deopt_payload: Vec<i64>,
}

impl NativeCallSession {
    pub fn new() -> Self {
        Self::default()
    }

    fn ensure_payload(&mut self, words: usize) -> *mut i64 {
        if self.deopt_payload.len() < words {
            self.deopt_payload.resize(words, 0);
        }
        self.deopt_payload.as_mut_ptr()
    }
}

/// Borrow-checked arguments for one native call.
///
/// The builder owns the ABI word/length arrays while retaining Rust borrows for
/// every flat buffer. Consequently a caller cannot attach the same mutable slice
/// twice, or overlap a mutable and immutable proof, without crossing an unsafe
/// Rust boundary. Final pointer/type validation still happens immediately before
/// dispatch so a mismatched compiled signature declines to the interpreter.
pub struct PreparedCall<'module, 'buffers> {
    module: &'module NativeModule,
    function: CompiledId,
    args: Vec<i64>,
    lens: Vec<i64>,
    host_ctx: HostCtx,
    logical_depth: LogicalCallDepth,
    flat_args: Vec<IndexedFlatBufferArg<'buffers>>,
}

/// Per-activation host context and bounded-execution cells for a compact mixed-
/// mode region call. Grouping these values keeps the safe API auditable as the
/// generated call-frame ABI evolves.
#[derive(Clone, Copy)]
pub struct RegionCallControls<'a> {
    pub host_ctx: HostCtx,
    pub logical_depth: LogicalCallDepth,
    pub initial_steps: i64,
    pub step_budget: Option<i64>,
    pub cancel: Option<&'a std::sync::atomic::AtomicBool>,
}

struct NativeCallInvocation<'a> {
    args: &'a [i64],
    lens: &'a [i64],
    host_ctx: HostCtx,
    logical_depth: LogicalCallDepth,
    limits_ptr: *const i64,
}

/// Generated-code controls selected when a whole function or mixed-mode region
/// is compiled. These booleans are part of the compiled version key: a caller
/// must use a limits-aware entry exactly when any control is enabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RegionCompileControls {
    pub step: bool,
    pub cancel: bool,
    pub deadline: bool,
}

impl From<RegionCompileControls> for LimitChecks {
    fn from(value: RegionCompileControls) -> Self {
        Self {
            step: value.step,
            cancel: value.cancel,
            deadline: value.deadline,
        }
    }
}

impl<'module, 'buffers> PreparedCall<'module, 'buffers> {
    pub fn scalar(mut self, value: i64) -> Self {
        self.args.push(value);
        self.lens.push(0);
        self
    }

    pub fn readonly_int(mut self, values: &'buffers [i64]) -> Self {
        let index = self.args.len();
        self.args.push(values.as_ptr() as i64);
        self.lens.push(values.len() as i64);
        self.flat_args
            .push(IndexedFlatBufferArg::new(index, FlatBufferArg::Int(values)));
        self
    }

    pub fn unique_int_mut(mut self, values: &'buffers mut [i64]) -> Self {
        let index = self.args.len();
        self.args.push(values.as_mut_ptr() as i64);
        self.lens.push(values.len() as i64);
        self.flat_args.push(IndexedFlatBufferArg::new(
            index,
            FlatBufferArg::IntMut(values),
        ));
        self
    }

    pub fn readonly_float(mut self, values: &'buffers [f64]) -> Self {
        let index = self.args.len();
        self.args.push(values.as_ptr() as i64);
        self.lens.push(values.len() as i64);
        self.flat_args.push(IndexedFlatBufferArg::new(
            index,
            FlatBufferArg::Float(values),
        ));
        self
    }

    pub fn unique_float_mut(mut self, values: &'buffers mut [f64]) -> Self {
        let index = self.args.len();
        self.args.push(values.as_mut_ptr() as i64);
        self.lens.push(values.len() as i64);
        self.flat_args.push(IndexedFlatBufferArg::new(
            index,
            FlatBufferArg::FloatMut(values),
        ));
        self
    }

    pub fn host_context(mut self, host_ctx: HostCtx) -> Self {
        self.host_ctx = host_ctx;
        self
    }

    pub fn logical_depth(mut self, current: usize, limit: usize) -> Self {
        self.logical_depth = LogicalCallDepth { current, limit };
        self
    }

    pub fn execute(mut self) -> NativeOutcome {
        self.module.call_with_indexed_flat_args_at_depth(
            self.function,
            &self.args,
            &self.lens,
            self.host_ctx,
            &mut self.flat_args,
            self.logical_depth,
        )
    }
}

/// `FuncId`s of the declared host helpers, resolved into per-function `FuncRef`s
/// at codegen time.
#[derive(Clone)]
pub(crate) struct HostFuncs {
    funcs: Vec<(HostHelper, FuncId)>,
}

impl HostFuncs {
    pub(crate) fn get(&self, helper: HostHelper) -> FuncId {
        self.funcs
            .iter()
            .find_map(|(candidate, id)| (*candidate == helper).then_some(*id))
            .expect("host helper import declared")
    }
}

/// Handle to a function compiled into a [`NativeModule`]. Carries the minting
/// module's identity so it can't be used against a different module (which would
/// index the wrong function table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledId {
    pub(crate) module_id: u64,
    pub(crate) index: usize,
}

pub use crate::deopt::{
    DeoptChildSite, DeoptFrame, DeoptMap, DeoptReg, DeoptSite, DeoptValue, NativeDeclineReason,
    NativeOutcome, SafepointId,
};
use crate::deopt::{abi_mismatch_decline, anonymous_deopt, reentrant_decline};

pub(crate) fn is_flat_type(ty: JitValueType) -> bool {
    matches!(
        ty,
        JitValueType::FlatInt
            | JitValueType::FlatIntMut
            | JitValueType::FlatFloat
            | JitValueType::FlatFloatMut
    )
}

impl NativeOutcome {
    /// The completed **scalar** result bits, or `None` otherwise. `Some` ONLY for
    /// [`Completed`](NativeOutcome::Completed) (a genuine `i64`/`f64`-bits scalar);
    /// [`CompletedHandle`](NativeOutcome::CompletedHandle) and
    /// [`Deopt`](NativeOutcome::Deopt) both yield `None`. A `CompletedHandle` payload
    /// is an OPAQUE output-table handle, not a scalar value, so conflating it here
    /// would let a caller misread a heap-table index as a result — hence this method
    /// is deliberately scalar-only. Use [`completed_handle`](NativeOutcome::completed_handle)
    /// for the handle, or [`completed_any_bits`](NativeOutcome::completed_any_bits)
    /// when you only need the raw bits of either completed variant.
    pub fn completed(self) -> Option<i64> {
        match self {
            NativeOutcome::Completed(value) => Some(value),
            NativeOutcome::CompletedHandle(_)
            | NativeOutcome::Yield { .. }
            | NativeOutcome::Deopt { .. } => None,
        }
    }

    /// The completed **heap-value handle**, or `None` otherwise. `Some` ONLY for
    /// [`CompletedHandle`](NativeOutcome::CompletedHandle) — the opaque output-table
    /// index the host materializes the [`VmValue`] from. [`Completed`](NativeOutcome::Completed)
    /// (a scalar) and [`Deopt`](NativeOutcome::Deopt) yield `None`. The returned value
    /// is NOT a scalar result; it is meaningful only as an output-table handle.
    pub fn completed_handle(self) -> Option<i64> {
        match self {
            NativeOutcome::CompletedHandle(handle) => Some(handle),
            NativeOutcome::Completed(_)
            | NativeOutcome::Yield { .. }
            | NativeOutcome::Deopt { .. } => None,
        }
    }

    /// The raw 64-bit payload of EITHER completed variant, or `None` on a deopt.
    /// `Some` for both [`Completed`](NativeOutcome::Completed) (scalar result bits)
    /// and [`CompletedHandle`](NativeOutcome::CompletedHandle) (an opaque output-table
    /// handle); `None` only for [`Deopt`](NativeOutcome::Deopt). The two cases are
    /// indistinguishable in the returned `i64`, so this is for callers that genuinely
    /// only need "did it complete, give me the raw bits" and disambiguate elsewhere
    /// (e.g. by the return register's [`JitValueType`]). When the scalar/handle
    /// distinction matters, use [`completed`](NativeOutcome::completed) /
    /// [`completed_handle`](NativeOutcome::completed_handle) instead.
    pub fn completed_any_bits(self) -> Option<i64> {
        match self {
            NativeOutcome::Completed(value) | NativeOutcome::CompletedHandle(value) => Some(value),
            NativeOutcome::Yield { .. } | NativeOutcome::Deopt { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ForcedDeopt {
    Site(u32),
    All,
}

impl ForcedDeopt {
    pub(crate) fn forces(self, site_id: i64) -> bool {
        match self {
            ForcedDeopt::Site(site) => i64::from(site) == site_id,
            ForcedDeopt::All => true,
        }
    }
}

impl NativeModule {
    /// Validate normal-entry IR against this module's structural work limits.
    pub fn validate_region<'a>(
        &self,
        function: &'a JitFunction,
    ) -> Result<ValidatedJitFunction<'a>, JitError> {
        let started = std::time::Instant::now();
        let result = ValidatedJitFunction::with_limits(function, &self.limits);
        let mut timings = self.phase_timings.get();
        timings.add_validation(started.elapsed().as_nanos());
        self.phase_timings.set(timings);
        result
    }

    /// Validate window-entry IR against this module's structural work limits.
    pub fn validate_osr_region<'a>(
        &self,
        function: &'a JitFunction,
    ) -> Result<ValidatedJitFunction<'a>, JitError> {
        let started = std::time::Instant::now();
        let result = ValidatedJitFunction::for_osr_with_limits(function, &self.limits);
        let mut timings = self.phase_timings.get();
        timings.add_validation(started.elapsed().as_nanos());
        self.phase_timings.set(timings);
        result
    }

    pub fn compile_phase_timings(&self) -> CompilePhaseTimings {
        self.phase_timings.get()
    }
    #[cfg(test)]
    pub(crate) fn compiled_function_count(&self) -> usize {
        self.funcs.len()
    }

    #[cfg(test)]
    pub(crate) fn test_raw_entry(&self, id: CompiledId) -> Option<CompiledAbi> {
        (id.module_id == self.id)
            .then(|| self.funcs.get(id.index).map(|function| function.f))
            .flatten()
    }

    /// Optimizing native tier (back-compat default): `opt_level="speed"`.
    pub fn new(helpers: HostHelpers) -> Result<Self, JitError> {
        Self::new_with_opt(helpers, false)
    }

    /// Start a safe, phase-typed native call. Prefer this API when constructing
    /// ABI arguments outside the VM's internal reusable marshalling path.
    pub fn prepare_call(&self, function: CompiledId) -> PreparedCall<'_, '_> {
        PreparedCall {
            module: self,
            function,
            args: Vec::new(),
            lens: Vec::new(),
            host_ctx: 0,
            logical_depth: LogicalCallDepth {
                current: 0,
                limit: usize::MAX,
            },
            flat_args: Vec::new(),
        }
    }

    /// Build a native module at a selectable optimization level.
    ///
    /// `baseline == true` selects the **baseline tier**:
    /// `opt_level="none"`. Everything else — IR translation, host helpers, the
    /// bail-flag deopt protocol — is byte-for-byte identical to the optimizing
    /// path; only the Cranelift ISA `opt_level` flag changes. The win is
    /// *compile latency* (less codegen work), at the cost of slightly less
    /// optimized machine code. The interpreter/`run_jit` deopt oracle remains
    /// valid regardless of opt level because the embedding VM rolls back every
    /// journaled heap or mutable-flat write before replay.
    ///
    /// `baseline == false` keeps the optimizing hot-path tier (`opt_level="speed"`).
    pub fn new_with_opt(helpers: HostHelpers, baseline: bool) -> Result<Self, JitError> {
        let budget = ExecutableMemoryBudget::new(DEFAULT_STANDALONE_JIT_ARENA_BYTES);
        Self::new_with_opt_and_memory_budget(
            helpers,
            baseline,
            budget,
            DEFAULT_STANDALONE_JIT_ARENA_BYTES,
        )
    }

    /// Build a native module whose executable mappings are charged to `budget`.
    ///
    /// Multiple modules may share one budget, which is how the embedding VM
    /// enforces one hard allocation boundary across baseline and optimized tiers.
    pub fn new_with_opt_and_memory_budget(
        helpers: HostHelpers,
        baseline: bool,
        budget: ExecutableMemoryBudget,
        arena_bytes: u64,
    ) -> Result<Self, JitError> {
        Self::new_with_opt_and_memory_budget_and_limits(
            helpers,
            baseline,
            budget,
            arena_bytes,
            JitLimits::default(),
        )
    }

    pub fn new_with_opt_and_memory_budget_and_limits(
        helpers: HostHelpers,
        baseline: bool,
        budget: ExecutableMemoryBudget,
        arena_bytes: u64,
        limits: JitLimits,
    ) -> Result<Self, JitError> {
        let reservation = budget.reserve(arena_allocation_charge(arena_bytes)?)?;
        let arena_bytes = usize::try_from(arena_bytes).map_err(|_| {
            JitError::new(
                JitErrorKind::AdmissionRejected,
                "JIT arena size does not fit in usize",
            )
        })?;
        let arena = ArenaMemoryProvider::new_with_size(arena_bytes).map_err(|error| {
            JitError::new(
                JitErrorKind::AdmissionRejected,
                format!("JIT arena allocation: {error}"),
            )
        })?;
        Self::new_with_opt_inner(Some(helpers), baseline, arena, reservation, limits)
    }

    #[cfg(feature = "fuzzing")]
    pub(crate) fn new_for_scalar_fuzzing(limits: JitLimits) -> Result<Self, JitError> {
        const FUZZ_ARENA_BYTES: u64 = 4 * 1024 * 1024;
        let budget = ExecutableMemoryBudget::new(FUZZ_ARENA_BYTES);
        let reservation = budget.reserve(arena_allocation_charge(FUZZ_ARENA_BYTES)?)?;
        let arena =
            ArenaMemoryProvider::new_with_size(FUZZ_ARENA_BYTES as usize).map_err(|error| {
                JitError::new(
                    JitErrorKind::AdmissionRejected,
                    format!("JIT fuzz arena allocation: {error}"),
                )
            })?;
        Self::new_with_opt_inner(None, true, arena, reservation, limits)
    }

    fn new_with_opt_inner(
        helpers: Option<HostHelpers>,
        baseline: bool,
        arena: ArenaMemoryProvider,
        memory_reservation: ExecutableMemoryReservation,
        limits: JitLimits,
    ) -> Result<Self, JitError> {
        let mut flags = settings::builder();
        // Plain JIT: no PIC. Optimize for speed on the hot path, or skip
        // optimization entirely in baseline mode to minimize compile latency.
        flags
            .set("use_colocated_libcalls", "false")
            .map_err(|e| err("settings", e))?;
        flags
            .set("is_pic", "false")
            .map_err(|e| err("settings", e))?;
        flags
            .set("opt_level", if baseline { "none" } else { "speed" })
            .map_err(|e| err("settings", e))?;
        let flags = settings::Flags::new(flags);
        // Build the host ISA with our flags (so `opt_level` actually applies),
        // then a JIT module on top of it.
        let isa = cranelift_native::builder()
            .map_err(|e| err("host isa", e))?
            .finish(flags)
            .map_err(|e| err("isa finish", e))?;
        let mut builder = JITBuilder::with_isa(isa, default_libcall_names());
        builder.memory_provider(Box::new(arena));
        // Register the host helper addresses so imported calls link to them.
        // The typed `extern "C"` pointers become the `*const u8` Cranelift's symbol
        // table wants here, where this crate owns the obligation that the address
        // matches the imported signature declared just below.
        if let Some(helpers) = helpers {
            for &helper in HostHelper::all() {
                builder.symbol(helper.symbol(), helpers.addr(helper));
            }
        }
        let mut module = JITModule::new(builder);
        let imports = HostFuncs {
            funcs: if helpers.is_some() {
                HostHelper::all()
                    .iter()
                    .map(|&helper| {
                        let id =
                            declare_import_for(&mut module, helper.symbol(), &helper.signature())?;
                        Ok((helper, id))
                    })
                    .collect::<Result<Vec<_>, JitError>>()?
            } else {
                Vec::new()
            },
        };
        let ctx = module.make_context();
        Ok(Self {
            module,
            ctx,
            fbctx: FunctionBuilderContext::new(),
            funcs: Vec::new(),
            counter: 0,
            id: NEXT_MODULE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            imports,
            limits,
            phase_timings: std::cell::Cell::new(CompilePhaseTimings::default()),
            _memory_reservation: memory_reservation,
        })
    }

    /// Compile `function` to native code and return a handle to call it.
    pub fn compile(&mut self, function: &JitFunction) -> Result<CompiledId, JitError> {
        let validated = self.validate_region(function)?;
        self.compile_validated(&validated)
    }

    /// Compile IR that has already crossed the sealed validation boundary.
    ///
    /// Callers that prepare or cache JIT work can validate once and pass the
    /// resulting proof here. The proof borrows the source IR, preventing mutation
    /// between validation and code generation.
    pub fn compile_validated(
        &mut self,
        function: &ValidatedJitFunction<'_>,
    ) -> Result<CompiledId, JitError> {
        if function.mode() != validated::ValidationMode::Standard {
            return Err(JitError::invalid_ir(
                "OSR-validated IR cannot use the normal compile entry",
            ));
        }
        self.compile_inner(function, None, None, LimitChecks::default(), false)
    }

    /// Compile a normal-entry region with generated source-step, cancellation,
    /// and/or monotonic-deadline checks.
    pub fn compile_with_controls(
        &mut self,
        function: &JitFunction,
        controls: RegionCompileControls,
    ) -> Result<CompiledId, JitError> {
        let validated = self.validate_region(function)?;
        self.compile_inner(&validated, None, None, controls.into(), false)
    }

    pub fn compile_validated_with_controls(
        &mut self,
        function: &ValidatedJitFunction<'_>,
        controls: RegionCompileControls,
    ) -> Result<CompiledId, JitError> {
        if function.mode() != validated::ValidationMode::Standard {
            return Err(JitError::invalid_ir(
                "OSR-validated IR cannot use the normal compile entry",
            ));
        }
        self.compile_inner(function, None, None, controls.into(), false)
    }

    /// Compile a function that may become the target of a native-to-native call.
    /// Eligible infallible scalar leaves use one frame-free canonical body plus a
    /// small stable-frame adapter, avoiding a second full function lowering.
    pub fn compile_native_callee(
        &mut self,
        function: &JitFunction,
    ) -> Result<CompiledId, JitError> {
        let validated = self.validate_region(function)?;
        self.compile_inner(&validated, None, None, LimitChecks::default(), true)
    }

    /// Compile `function` while forcing the safepoint with id `force_site` (sites are
    /// numbered from 1 in emission order) to bail *unconditionally*: any execution
    /// reaching that site deopts there regardless of its guard condition, capturing
    /// the same live set the natural bail would. All other sites behave normally.
    ///
    /// This is a test/diagnostic hook for exercising the deopt capture + map at every
    /// safepoint, including ones that never fire under normal inputs. The default
    /// [`compile`](Self::compile) path passes `None` and emits identical code, so this
    /// option has zero production cost. An out-of-range `force_site` simply matches no
    /// site and the function compiles as usual.
    pub fn compile_forcing_bail(
        &mut self,
        function: &JitFunction,
        force_site: u32,
    ) -> Result<CompiledId, JitError> {
        let validated = self.validate_region(function)?;
        self.compile_inner(
            &validated,
            Some(ForcedDeopt::Site(force_site)),
            None,
            LimitChecks::default(),
            false,
        )
    }

    /// Compile `function` while forcing every generated safepoint to bail
    /// unconditionally. This is a diagnostic/deopt-stress hook: any executed guard
    /// edge exercises the real deopt capture path regardless of the guard's natural
    /// condition. The default [`compile`](Self::compile) path emits byte-identical
    /// guarded code and never sets this mode.
    pub fn compile_forcing_all_bails(
        &mut self,
        function: &JitFunction,
    ) -> Result<CompiledId, JitError> {
        let validated = self.validate_region(function)?;
        self.compile_inner(
            &validated,
            Some(ForcedDeopt::All),
            None,
            LimitChecks::default(),
            false,
        )
    }

    /// Compile `function` as an **OSR (on-stack replacement) entry** at `header_ip`.
    /// Instead of the normal param-loading entry, the generated function's
    /// entry block treats its `args_ptr` argument as the interpreter's **register
    /// window** (an `i64`/`f64` array of width `n_regs`, indexed by register), loads
    /// the registers definitely-assigned on entry to `header_ip` (the loop's
    /// live-in) out of it, then jumps directly to the block for `header_ip` — so
    /// native execution begins *inside* the loop rather than at the function top.
    ///
    /// The loop exits by reaching a [`JitInstr::OsrExit`] (the post-loop ip), which
    /// deopts with the live-out window and that ip as `resume_ip`; the host then
    /// resumes the interpreter there via the precise-deopt path. Everything outside
    /// the loop (which the host never reaches natively under OSR) is `Bail`/`OsrExit`.
    ///
    /// The window ABI reuses the `CompiledAbi` `args_ptr` slot: it must be a buffer
    /// of `n_regs` 8-byte words (each register's value at offset `reg * 8`; an f64
    /// register's bit pattern in its slot), and the caller passes `n_args = n_regs`
    /// with an `n_regs`-long `lens` slice. An out-of-range `header_ip` (no leader
    /// block) is rejected as a [`JitError`].
    ///
    /// `step_limit`/`cancel_armed` request in-generated-code `VmLimits` enforcement:
    /// when set, the loop ticks `step_budget` per instruction
    /// and tests it (plus polls `cancel`) at every header, bailing to the interpreter
    /// — which then enforces the limit. The caller must use the unsafe raw limits
    /// entry with a valid non-null limits cell; ordinary safe call modes reject this
    /// compiled entry before executing machine code.
    pub fn compile_osr(
        &mut self,
        function: &JitFunction,
        header_ip: u32,
        step_limit: bool,
        cancel_armed: bool,
    ) -> Result<CompiledId, JitError> {
        let validated = self.validate_osr_region(function)?;
        self.compile_inner(
            &validated,
            None,
            Some(header_ip),
            LimitChecks {
                step: step_limit,
                cancel: cancel_armed,
                deadline: false,
            },
            false,
        )
    }

    /// Limits-aware OSR/continuation compile entry. The legacy `compile_osr`
    /// remains as a compatibility wrapper for existing callers.
    pub fn compile_osr_with_controls(
        &mut self,
        function: &JitFunction,
        header_ip: u32,
        controls: RegionCompileControls,
    ) -> Result<CompiledId, JitError> {
        let validated = self.validate_osr_region(function)?;
        self.compile_inner(&validated, None, Some(header_ip), controls.into(), false)
    }

    pub fn compile_validated_osr_with_controls(
        &mut self,
        function: &ValidatedJitFunction<'_>,
        header_ip: u32,
        controls: RegionCompileControls,
    ) -> Result<CompiledId, JitError> {
        if function.mode() != validated::ValidationMode::Osr {
            return Err(JitError::invalid_ir(
                "normal-entry validated IR cannot use the OSR compile entry",
            ));
        }
        self.compile_inner(function, None, Some(header_ip), controls.into(), false)
    }

    fn compile_inner(
        &mut self,
        validated: &ValidatedJitFunction<'_>,
        forced: Option<ForcedDeopt>,
        osr_header: Option<u32>,
        limit_checks: LimitChecks,
        emit_direct_scalar_entry: bool,
    ) -> Result<CompiledId, JitError> {
        let codegen_started = std::time::Instant::now();
        let function = validated.function();
        debug_assert_eq!(
            validated.mode() == validated::ValidationMode::Osr,
            osr_header.is_some()
        );
        // Heap-result return ABI: a non-OSR function whose `Return` source is a
        // `Handle` register returns an output-table handle, not a scalar. Determined
        // purely from the (validated) IR, before codegen. OSR functions never have a
        // top-level `Return` (they exit via `OsrExit`), so this stays `false`.
        let return_type = validated.return_type();
        let returns_handle = return_type == Some(JitValueType::Handle);
        let scalar_leaf_callable =
            native_scalar_leaf_callable(function, osr_header.is_some(), returns_handle);
        let compact_scalar_frame_callable =
            native_compact_scalar_frame_callable(function, osr_header.is_some(), returns_handle);
        let direct_scalar_callable = emit_direct_scalar_entry
            && direct_scalar_callable(function, osr_header.is_some(), return_type);
        let native_callees = self.resolve_native_callees(function)?;
        if native_callees.len() > self.limits.max_native_callees {
            return Err(JitError::new(
                JitErrorKind::AdmissionRejected,
                format!(
                    "JIT function has {} native callees, exceeding the limit {}",
                    native_callees.len(),
                    self.limits.max_native_callees
                ),
            ));
        }
        let mut projected_payload_words = function.n_regs as usize;
        for instr in &function.code {
            if let JitInstr::CallNative { callee, .. } = instr {
                let child = native_callees
                    .iter()
                    .find(|candidate| candidate.handle == *callee)
                    .expect("resolved native callee covers every validated CallNative");
                let child_words = if child.direct_scalar_func_id.is_some() {
                    0
                } else {
                    1 + child.deopt_payload_words
                };
                projected_payload_words = projected_payload_words
                    .checked_add(child_words)
                    .ok_or_else(|| {
                        JitError::new(
                            JitErrorKind::AdmissionRejected,
                            "JIT deoptimization payload size overflow",
                        )
                    })?;
            }
        }
        if projected_payload_words > self.limits.max_deopt_payload_words {
            return Err(JitError::new(
                JitErrorKind::AdmissionRejected,
                format!(
                    "JIT deoptimization payload requires {projected_payload_words} words, exceeding the limit {}",
                    self.limits.max_deopt_payload_words
                ),
            ));
        }
        let native_call_depth = native_callees
            .iter()
            .filter_map(|callee| self.funcs.get(callee.handle.index))
            .map(|callee| callee.native_call_depth.saturating_add(1))
            .max()
            .unwrap_or(0);
        let ptr_ty = self.module.target_config().pointer_type();
        self.module.clear_context(&mut self.ctx);
        push_compiled_abi_signature(&mut self.ctx.func, ptr_ty);

        // Declare before defining: mint this function's `FuncId` before lowering
        // its body so the module holds a stable id for it during codegen.
        let name = format!("rss_jit_{}", self.counter);
        self.counter += 1;
        let id = self
            .module
            .declare_function(&name, Linkage::Local, &self.ctx.func.signature)
            .map_err(|e| err("declare", e))?;

        let (direct_scalar_id, code_size_bytes, deopt_map, bounds_checks_elided) =
            if direct_scalar_callable {
                // The compact scalar body is the canonical implementation. The
                // stable frame ABI gets only a small adapter, rather than a second
                // full lowering of the same function.
                self.module.clear_context(&mut self.ctx);
                let return_type = return_type.expect("direct scalar eligibility requires a return");
                push_direct_scalar_signature(&mut self.ctx.func, function, return_type);
                let direct_name = format!("rss_jit_direct_{}", self.counter);
                self.counter += 1;
                let direct_id = self
                    .module
                    .declare_function(&direct_name, Linkage::Local, &self.ctx.func.signature)
                    .map_err(|e| err("declare direct scalar body", e))?;
                build_direct_scalar_function(&mut self.ctx.func, &mut self.fbctx, function)?;
                self.module
                    .define_function(direct_id, &mut self.ctx)
                    .map_err(|e| err("define direct scalar body", e))?;
                let direct_bytes = self
                    .ctx
                    .compiled_code()
                    .map(|code| u64::from(code.code_info().total_size))
                    .unwrap_or(0);

                self.module.clear_context(&mut self.ctx);
                push_compiled_abi_signature(&mut self.ctx.func, ptr_ty);
                let direct_ref = self
                    .module
                    .declare_func_in_func(direct_id, &mut self.ctx.func);
                build_direct_scalar_frame_wrapper(
                    &mut self.ctx.func,
                    &mut self.fbctx,
                    direct_ref,
                    function,
                    return_type,
                );
                self.module
                    .define_function(id, &mut self.ctx)
                    .map_err(|e| err("define direct scalar frame wrapper", e))?;
                let wrapper_bytes = self
                    .ctx
                    .compiled_code()
                    .map(|code| u64::from(code.code_info().total_size))
                    .unwrap_or(0);
                self.module.clear_context(&mut self.ctx);
                (
                    Some(direct_id),
                    direct_bytes.saturating_add(wrapper_bytes),
                    DeoptMap::default(),
                    0,
                )
            } else {
                let codegen = build_function(
                    &mut self.ctx.func,
                    &mut self.fbctx,
                    &mut self.module,
                    FunctionCodegenInput {
                        imports: self.imports.clone(),
                        program: function,
                        forced,
                        osr_header,
                        native_callees: &native_callees,
                        self_func_id: id,
                        group: &[],
                        limit_checks,
                        native_static_call_depth: native_call_depth,
                        assigned_in: validated.assigned_in(),
                        deopt_in: validated.deopt_in(),
                    },
                )?;

                self.module
                    .define_function(id, &mut self.ctx)
                    .map_err(|e| err("define", e))?;
                let code_size_bytes = self
                    .ctx
                    .compiled_code()
                    .map(|code| u64::from(code.code_info().total_size))
                    .unwrap_or(0);
                self.module.clear_context(&mut self.ctx);
                (
                    None,
                    code_size_bytes,
                    codegen.deopt_map,
                    codegen.direct_list_bounds_checks_elided,
                )
            };
        let mut timings = self.phase_timings.get();
        timings.add_codegen(codegen_started.elapsed().as_nanos());
        self.phase_timings.set(timings);
        let finalize_started = std::time::Instant::now();
        self.module
            .finalize_definitions()
            .map_err(|e| err("finalize", e))?;
        let mut timings = self.phase_timings.get();
        timings.add_finalize(finalize_started.elapsed().as_nanos());
        self.phase_timings.set(timings);
        let code = self.module.get_finalized_function(id);
        // SAFETY: `code` points at the machine code we just emitted with exactly
        // the `CompiledAbi` signature declared above.
        let f: CompiledAbi = unsafe { std::mem::transmute::<*const u8, CompiledAbi>(code) };
        let handle = CompiledId {
            module_id: self.id,
            index: self.funcs.len(),
        };
        self.funcs.push(CompiledFunc {
            f,
            id,
            direct_scalar_id,
            code_size_bytes,
            native_call_depth,
            native_depth_cap: native_recursion_depth_cap(function) as u32,
            n_params: function.n_params as usize,
            n_regs: function.n_regs as usize,
            deopt_map,
            direct_list_bounds_checks_elided: bounds_checks_elided,
            requires_limits: limit_checks.any(),
            limit_checks,
            osr: osr_header.is_some(),
            returns_handle,
            param_types: function.reg_types[..function.n_params as usize].to_vec(),
            reg_types: function.reg_types.clone(),
            return_type,
            scalar_leaf_callable,
            compact_scalar_frame_callable,
            direct_scalar_call_edges: native_callees
                .iter()
                .filter(|callee| callee.direct_scalar_func_id.is_some())
                .count() as u32,
        });
        Ok(handle)
    }


    /// Machine-code bytes emitted for a compiled function, including any constant
    /// data reported by Cranelift. Used only for host-side telemetry.
    pub fn code_size_bytes(&self, id: CompiledId) -> Option<u64> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| func.code_size_bytes)
    }

    /// Number of direct flat-list access checks omitted after a sound range and
    /// provenance proof. This is host-side evidence only; it does not alter the
    /// checked public IR contract.
    pub fn direct_list_bounds_checks_elided(&self, id: CompiledId) -> Option<u64> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs
            .get(id.index)
            .map(|func| func.direct_list_bounds_checks_elided)
    }

    /// Longest native-to-native call chain reachable from a compiled function.
    /// Used only for host-side telemetry.
    pub fn native_call_depth(&self, id: CompiledId) -> Option<u32> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| func.native_call_depth)
    }

    /// Native call sites emitted through the private frame-free scalar ABI.
    pub fn direct_scalar_call_edges(&self, id: CompiledId) -> Option<u32> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs
            .get(id.index)
            .map(|func| func.direct_scalar_call_edges)
    }

    #[cfg(test)]
    pub(crate) fn has_direct_scalar_entry(&self, id: CompiledId) -> bool {
        id.module_id == self.id
            && self
                .funcs
                .get(id.index)
                .is_some_and(|func| func.direct_scalar_id.is_some())
    }

    #[cfg(test)]
    pub(crate) fn compact_scalar_frame_callable(&self, id: CompiledId) -> bool {
        self.funcs
            .get(id.index)
            .filter(|_| id.module_id == self.id)
            .is_some_and(|func| func.compact_scalar_frame_callable)
    }

    fn resolve_native_callees(
        &self,
        function: &JitFunction,
    ) -> Result<Vec<NativeCallee>, JitError> {
        let mut callees = Vec::new();
        for instr in &function.code {
            let JitInstr::CallNative { callee, dst, args } = instr else {
                continue;
            };
            if callee.module_id != self.id {
                return Err(JitError::invalid_ir(
                    "CallNative callee belongs to a different module",
                ));
            }
            let compiled = self
                .funcs
                .get(callee.index)
                .ok_or_else(|| JitError::invalid_ir("CallNative callee index is out of range"))?;
            if compiled.osr {
                return Err(JitError::invalid_ir(
                    "CallNative does not support OSR callees yet",
                ));
            }
            if !compiled.scalar_leaf_callable {
                return Err(JitError::invalid_ir(
                    "CallNative callee is not a scalar-callable function",
                ));
            }
            if compiled.n_params != args.len() {
                return Err(JitError::invalid_ir(format!(
                    "CallNative got {} args, callee expects {}",
                    args.len(),
                    compiled.n_params
                )));
            }
            let Some(return_type) = compiled.return_type else {
                return Err(JitError::invalid_ir(
                    "CallNative callee has no scalar Return instruction",
                ));
            };
            if matches!(
                return_type,
                JitValueType::FlatInt
                    | JitValueType::FlatIntMut
                    | JitValueType::FlatFloat
                    | JitValueType::FlatFloatMut
            ) {
                return Err(JitError::invalid_ir(format!(
                    "CallNative callee return type {return_type:?} is not callable"
                )));
            }
            if function.reg_types[*dst as usize] != return_type {
                return Err(JitError::invalid_ir(format!(
                    "CallNative result register {dst} is {:?}, callee returns {return_type:?}",
                    function.reg_types[*dst as usize]
                )));
            }
            for (i, (&arg, &expected)) in args.iter().zip(compiled.param_types.iter()).enumerate() {
                if function.reg_types[arg as usize] != expected {
                    return Err(JitError::invalid_ir(format!(
                        "CallNative arg {i}: caller register {arg} is {:?}, callee expects {expected:?}",
                        function.reg_types[arg as usize]
                    )));
                }
            }
            callees.push(NativeCallee {
                handle: *callee,
                func_id: compiled.id,
                n_params: compiled.n_params,
                param_types: compiled.param_types.clone(),
                deopt_payload_words: compiled.deopt_map.payload_words,
                return_type,
                compact_scalar_frame: compiled.compact_scalar_frame_callable,
                direct_scalar_func_id: compiled.direct_scalar_id,
            });
        }
        Ok(callees)
    }

    /// Run a compiled function. Returns [`NativeOutcome::Completed`] with the result
    /// on completion, or [`NativeOutcome::Deopt`] when the native code bailed and the
    /// interpreter should re-run the function — either a guard bail (overflow/
    /// divide-by-zero edge) or a host-helper bail (an unsatisfiable heap read; see
    /// [`signal_bail`]). The returned [`SafepointId`] identifies the exact bail site
    /// (codegen numbers sites from 1; [`SafepointId::ANONYMOUS`] / `0` means no site
    /// recorded an id, e.g. an id/length mismatch rejected before the call).
    ///
    /// This is a **fully safe** boundary for scalar/handle args. Functions whose
    /// entry contains a flat buffer or requires a limits cell are rejected;
    /// embedders must use a borrow-checked prepared call or the unsafe raw limits
    /// entry as appropriate.
    /// The bail flag is a
    /// per-thread `u8` owned by this crate; `call` resets it, passes its own address
    /// into the generated code, and reports a set flag as a fallback.
    ///
    /// `args` and `lens` are parallel slices indexed by parameter. Scalar entries
    /// use a zero length. Flat entries cannot pass through this API.
    pub fn call(&self, id: CompiledId, args: &[i64], lens: &[i64]) -> NativeOutcome {
        let Some(func) = self.funcs.get(id.index).filter(|_| id.module_id == self.id) else {
            return anonymous_deopt();
        };
        let entry_types = if func.osr {
            &func.reg_types
        } else {
            &func.param_types
        };
        if entry_types.iter().any(|ty| is_flat_type(*ty)) {
            return anonymous_deopt();
        }
        self.call_with_host_ctx(id, args, lens, 0, &mut [])
    }

    /// Execute a scalar/window entry compiled with step and/or cancellation
    /// polling. The limits cell is owned by this safe call for the full machine
    /// activation; `cancel` is borrowed, so generated code cannot outlive it.
    /// Returns the outcome and the updated interpreter-equivalent step count.
    pub fn call_with_step_cancel(
        &self,
        id: CompiledId,
        args: &[i64],
        lens: &[i64],
        initial_steps: i64,
        step_budget: Option<i64>,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> (NativeOutcome, i64) {
        let Some(func) = self.funcs.get(id.index).filter(|_| id.module_id == self.id) else {
            return (anonymous_deopt(), initial_steps);
        };
        if func.limit_checks.cancel && cancel.is_none() {
            return (anonymous_deopt(), initial_steps);
        }
        let mut session = NativeCallSession::new();
        let mut limits = [
            initial_steps,
            step_budget.unwrap_or(i64::MAX),
            cancel.map_or(0, |flag| flag as *const _ as i64),
        ];
        let outcome = self.call_inner(
            &mut session,
            id,
            NativeCallInvocation {
                args,
                lens,
                host_ctx: 0,
                logical_depth: LogicalCallDepth {
                    current: 0,
                    limit: usize::MAX,
                },
                limits_ptr: limits.as_mut_ptr(),
            },
        );
        (outcome, limits[0])
    }

    /// Execute a compact Handle/scalar region with a VM host context and native
    /// step/cancellation polling. Flat-buffer entries are intentionally rejected;
    /// callers needing them must use the borrow-checked prepared-call API.
    pub fn call_with_host_ctx_step_cancel(
        &self,
        id: CompiledId,
        args: &mut [i64],
        lens: &[i64],
        controls: RegionCallControls<'_>,
    ) -> (NativeOutcome, i64) {
        self.call_with_host_ctx_step_cancel_in_session(
            &mut NativeCallSession::new(),
            id,
            args,
            lens,
            controls,
        )
    }

    pub fn call_with_host_ctx_step_cancel_in_session(
        &self,
        session: &mut NativeCallSession,
        id: CompiledId,
        args: &mut [i64],
        lens: &[i64],
        controls: RegionCallControls<'_>,
    ) -> (NativeOutcome, i64) {
        let Some(func) = self.funcs.get(id.index).filter(|_| id.module_id == self.id) else {
            return (anonymous_deopt(), controls.initial_steps);
        };
        if func.reg_types.iter().any(|ty| is_flat_type(*ty)) {
            return (anonymous_deopt(), controls.initial_steps);
        }
        if func.limit_checks.cancel && controls.cancel.is_none() {
            return (anonymous_deopt(), controls.initial_steps);
        }
        let mut limits = [
            controls.initial_steps,
            controls.step_budget.unwrap_or(i64::MAX),
            controls.cancel.map_or(0, |flag| flag as *const _ as i64),
        ];
        let outcome = self.call_inner(
            session,
            id,
            NativeCallInvocation {
                args,
                lens,
                host_ctx: controls.host_ctx,
                logical_depth: controls.logical_depth,
                limits_ptr: limits.as_mut_ptr(),
            },
        );
        if matches!(outcome, NativeOutcome::Yield { .. })
            && !copy_session_yield_registers(func, session, args)
        {
            return (anonymous_deopt(), limits[0]);
        }
        (outcome, limits[0])
    }

    /// Run with a host context (see [`call_with_host_ctx`](Self::call_with_host_ctx))
    /// and a non-null native limit accounting limits cell. `limits_ptr` must point at a live, immovable
    /// `[i64; 3]` = `[steps, step_budget, cancel_addr]` for the call's duration: an
    /// armed OSR variant reads `step_budget`/`cancel_addr`, accumulates into and writes
    /// back `steps`. Unarmed variants ignore it (so [`call`](Self::call) passes null).
    /// # Safety
    ///
    /// Every flat entry in `args` must be a correctly aligned pointer to a live
    /// buffer of the logical type and length in `lens`. Mutable flat entries must
    /// be exclusively borrowed. `limits_ptr` must be null or point to a live limits
    /// cell required by this compiled OSR entry.
    #[cfg(test)]
    pub(crate) unsafe fn call_with_limits(
        &self,
        id: CompiledId,
        args: &[i64],
        lens: &[i64],
        host_ctx: HostCtx,
        limits_ptr: *const i64,
    ) -> NativeOutcome {
        self.call_inner(
            &mut NativeCallSession::new(),
            id,
            NativeCallInvocation {
                args,
                lens,
                host_ctx,
                logical_depth: LogicalCallDepth {
                    current: 0,
                    limit: usize::MAX,
                },
                limits_ptr,
            },
        )
    }

    /// Run a compiled function while forwarding `host_ctx` to every imported host
    /// helper. The context is opaque to `vm-jit`; the embedding VM validates and
    /// interprets it.
    /// Flat entries are accepted only when `flat_args` contains matching live Rust
    /// borrows whose addresses and lengths equal the ABI words in `args`/`lens`.
    /// Mutable entries require distinct mutable proofs; read-only aliases must use
    /// immutable proofs. Ambiguous aliasing declines to the interpreter.
    pub fn call_with_host_ctx(
        &self,
        id: CompiledId,
        args: &[i64],
        lens: &[i64],
        host_ctx: HostCtx,
        flat_args: &mut [FlatBufferArg<'_>],
    ) -> NativeOutcome {
        self.call_with_host_ctx_at_depth(
            id,
            args,
            lens,
            host_ctx,
            flat_args,
            LogicalCallDepth {
                current: 0,
                limit: usize::MAX,
            },
        )
    }

    /// Equivalent to [`call_with_host_ctx`](Self::call_with_host_ctx), with the
    /// current logical VM call depth and its configured limit supplied by the
    /// embedding interpreter.
    pub fn call_with_host_ctx_at_depth(
        &self,
        id: CompiledId,
        args: &[i64],
        lens: &[i64],
        host_ctx: HostCtx,
        flat_args: &mut [FlatBufferArg<'_>],
        logical_depth: LogicalCallDepth,
    ) -> NativeOutcome {
        let Some(func) = self.funcs.get(id.index).filter(|_| id.module_id == self.id) else {
            return anonymous_deopt();
        };
        if func.requires_limits {
            return anonymous_deopt();
        }
        let entry_types = if func.osr {
            &func.reg_types
        } else {
            &func.param_types
        };
        let mut mutable_proofs_used = vec![false; flat_args.len()];
        for (index, ty) in entry_types.iter().copied().enumerate() {
            if !is_flat_type(ty) {
                continue;
            }
            let expected_ptr = args.get(index).copied();
            let expected_len = lens.get(index).copied();
            let proof = flat_args
                .iter_mut()
                .enumerate()
                .find_map(|(proof_index, arg)| {
                    let (ptr, len, compatible) = match arg {
                        FlatBufferArg::Int(values) => (
                            values.as_ptr() as i64,
                            values.len() as i64,
                            ty == JitValueType::FlatInt,
                        ),
                        FlatBufferArg::IntMut(values) => (
                            values.as_mut_ptr() as i64,
                            values.len() as i64,
                            ty == JitValueType::FlatIntMut && !mutable_proofs_used[proof_index],
                        ),
                        FlatBufferArg::Float(values) => (
                            values.as_ptr() as i64,
                            values.len() as i64,
                            ty == JitValueType::FlatFloat,
                        ),
                        FlatBufferArg::FloatMut(values) => (
                            values.as_mut_ptr() as i64,
                            values.len() as i64,
                            ty == JitValueType::FlatFloatMut && !mutable_proofs_used[proof_index],
                        ),
                    };
                    (compatible && expected_ptr == Some(ptr) && expected_len == Some(len))
                        .then_some(proof_index)
                });
            let Some(proof_index) = proof else {
                return anonymous_deopt();
            };
            if matches!(ty, JitValueType::FlatIntMut | JitValueType::FlatFloatMut) {
                mutable_proofs_used[proof_index] = true;
            }
        }
        self.call_inner(
            &mut NativeCallSession::new(),
            id,
            NativeCallInvocation {
                args,
                lens,
                host_ctx,
                logical_depth,
                limits_ptr: std::ptr::null(),
            },
        )
    }

    /// Linear-time safe flat-buffer entry used by the VM's audited marshaller.
    /// Proofs must be sorted by ABI slot and contain exactly one proof for every
    /// flat entry. Pointer, length, kind, module, and function validation remain
    /// inside this crate; malformed input declines before machine-code entry.
    pub fn call_with_indexed_flat_args_at_depth(
        &self,
        id: CompiledId,
        args: &[i64],
        lens: &[i64],
        host_ctx: HostCtx,
        flat_args: &mut [IndexedFlatBufferArg<'_>],
        logical_depth: LogicalCallDepth,
    ) -> NativeOutcome {
        let Some(func) = self.funcs.get(id.index).filter(|_| id.module_id == self.id) else {
            return anonymous_deopt();
        };
        if func.requires_limits {
            return anonymous_deopt();
        }
        let entry_types = if func.osr {
            &func.reg_types
        } else {
            &func.param_types
        };
        let mut proof_cursor = 0usize;
        for (index, ty) in entry_types.iter().copied().enumerate() {
            if !is_flat_type(ty) {
                continue;
            }
            let Some(proof) = flat_args.get_mut(proof_cursor) else {
                return anonymous_deopt();
            };
            if proof.index != index
                || !flat_proof_matches(&mut proof.value, ty, args.get(index), lens.get(index))
            {
                return anonymous_deopt();
            }
            proof_cursor += 1;
        }
        if proof_cursor != flat_args.len() {
            return anonymous_deopt();
        }
        self.call_inner(
            &mut NativeCallSession::new(),
            id,
            NativeCallInvocation {
                args,
                lens,
                host_ctx,
                logical_depth,
                limits_ptr: std::ptr::null(),
            },
        )
    }

    /// Limits-aware counterpart of [`call_with_indexed_flat_args_at_depth`]. The
    /// indexed borrow proof remains linear-time; source steps and preemption state
    /// are carried by a call-owned limits cell.
    pub fn call_with_indexed_flat_args_and_controls_at_depth(
        &self,
        id: CompiledId,
        args: &[i64],
        lens: &[i64],
        flat_args: &mut [IndexedFlatBufferArg<'_>],
        controls: RegionCallControls<'_>,
    ) -> (NativeOutcome, i64) {
        self.call_with_indexed_flat_args_and_controls_in_session_at_depth(
            &mut NativeCallSession::new(),
            id,
            args,
            lens,
            flat_args,
            controls,
        )
    }

    pub fn call_with_indexed_flat_args_and_controls_in_session_at_depth(
        &self,
        session: &mut NativeCallSession,
        id: CompiledId,
        args: &[i64],
        lens: &[i64],
        flat_args: &mut [IndexedFlatBufferArg<'_>],
        controls: RegionCallControls<'_>,
    ) -> (NativeOutcome, i64) {
        let Some(func) = self.funcs.get(id.index).filter(|_| id.module_id == self.id) else {
            return (anonymous_deopt(), controls.initial_steps);
        };
        if !func.requires_limits {
            return (anonymous_deopt(), controls.initial_steps);
        }
        if func.limit_checks.cancel && controls.cancel.is_none() {
            return (anonymous_deopt(), controls.initial_steps);
        }
        let entry_types = if func.osr {
            &func.reg_types
        } else {
            &func.param_types
        };
        let mut proof_cursor = 0usize;
        for (index, ty) in entry_types.iter().copied().enumerate() {
            if !is_flat_type(ty) {
                continue;
            }
            let Some(proof) = flat_args.get_mut(proof_cursor) else {
                return (anonymous_deopt(), controls.initial_steps);
            };
            if proof.index != index
                || !flat_proof_matches(&mut proof.value, ty, args.get(index), lens.get(index))
            {
                return (anonymous_deopt(), controls.initial_steps);
            }
            proof_cursor += 1;
        }
        if proof_cursor != flat_args.len() {
            return (anonymous_deopt(), controls.initial_steps);
        }
        let mut limits = [
            controls.initial_steps,
            controls.step_budget.unwrap_or(i64::MAX),
            controls.cancel.map_or(0, |flag| flag as *const _ as i64),
        ];
        let outcome = self.call_inner(
            session,
            id,
            NativeCallInvocation {
                args,
                lens,
                host_ctx: controls.host_ctx,
                logical_depth: controls.logical_depth,
                limits_ptr: limits.as_mut_ptr(),
            },
        );
        (outcome, limits[0])
    }

    /// The per-function [`DeoptMap`] computed at compile time, or `None` if `id`
    /// is foreign / out of range (validated exactly like [`call`]). Index it by
    /// safepoint id via `map.sites[id - 1]` (ids start at 1). Host-side metadata
    /// only — reading it has no effect on execution.
    pub fn deopt_map(&self, id: CompiledId) -> Option<&DeoptMap> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| &func.deopt_map)
    }

    /// Register count of the compiled function (the width of its register space),
    /// or `None` for a foreign / out-of-range `id`. Companion to [`deopt_map`]: a
    /// site's `live` regs index into this space.
    pub fn n_regs(&self, id: CompiledId) -> Option<usize> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| func.n_regs)
    }
}

thread_local! {
    /// Re-entry is a property of the current activation, not of the code
    /// container. Keep the guard thread-local so [`NativeModule`] owns no mutable
    /// call state and nested calls through a host helper still decline cleanly.
    static NATIVE_CALL_ACTIVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct TopLevelCallGuard;

impl TopLevelCallGuard {
    fn enter() -> Option<Self> {
        NATIVE_CALL_ACTIVE.with(|active| (!active.replace(true)).then_some(Self))
    }
}

impl Drop for TopLevelCallGuard {
    fn drop(&mut self) {
        NATIVE_CALL_ACTIVE.with(|active| active.set(false));
    }
}

#[repr(C)]
struct HostCallContext {
    user: HostCtx,
    bail: *mut u8,
}

/// Signal from a [`HostHelpers`] callback that the in-flight native call cannot be
/// satisfied (wrong type / out-of-bounds heap read), so the function must fall
/// back to the interpreter. The generated code loads the flag immediately after
/// each helper call and branches to fallback when it is set; [`NativeModule::call`]
/// also reports it. Safe to call any time — it is a no-op outside a `call`, since
/// `call` resets the flag at the start of every invocation.
pub fn signal_bail(context: HostCtx) {
    if context == 0 {
        return;
    }
    // SAFETY: generated code receives only the address of the live
    // `HostCallContext` created by `call_inner`, and helpers cannot retain it.
    let context = unsafe { &mut *(context as *mut HostCallContext) };
    // SAFETY: the bail cell belongs to the same live call frame.
    unsafe { *context.bail = 1 };
}

/// Recover the embedding VM's opaque context from the call-scoped helper context.
pub fn user_host_ctx(context: HostCtx) -> HostCtx {
    if context == 0 {
        return 0;
    }
    // SAFETY: see `signal_bail`; this function is called only by registered host
    // helpers while the generated invocation is active.
    unsafe { (*(context as *const HostCallContext)).user }
}
use super::*;
