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
    }) && function.code.iter().all(|instr| {
        (matches!(
            instr,
            JitInstr::Nop
                | JitInstr::TailCallGuard { .. }
                | JitInstr::LoadInt { .. }
                | JitInstr::LoadFloat { .. }
                | JitInstr::LoadBool { .. }
                | JitInstr::Move { .. }
                | JitInstr::Add { .. }
                | JitInstr::Sub { .. }
                | JitInstr::Mul { .. }
                | JitInstr::Div { .. }
                | JitInstr::Mod { .. }
                | JitInstr::IntToFloat { .. }
                | JitInstr::FloatToInt { .. }
                | JitInstr::BitAnd { .. }
                | JitInstr::BitOr { .. }
                | JitInstr::BitXor { .. }
                | JitInstr::Shl { .. }
                | JitInstr::Shr { .. }
                | JitInstr::Compare { .. }
                | JitInstr::Equal { .. }
                | JitInstr::NotEqual { .. }
                | JitInstr::Jump { .. }
                | JitInstr::JumpIfBool { .. }
                | JitInstr::JumpIfIntCompare { .. }
                | JitInstr::ProfiledJumpIfBool { .. }
                | JitInstr::ProfiledJumpIfIntCompare { .. }
                | JitInstr::CallNative { .. }
                | JitInstr::CallSelf { .. }
                | JitInstr::CallGroup { .. }
                | JitInstr::HostCall { .. }
                | JitInstr::MemoizedHostCall { .. }
                | JitInstr::Return { .. }
                | JitInstr::Bail // Flat-list direct ops via the canonical `is_flat_list_direct` set (single
                                 // source of truth shared with the rsscript leaf/cost-model sites).
        ) || instr.is_flat_list_direct())
            && !matches!(
                instr,
                JitInstr::HostCall { helper, .. } | JitInstr::MemoizedHostCall { helper, .. }
                    if helper.heap_effect().extends_input_handles()
            )
    })
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
    /// Machine-code bytes emitted by Cranelift for this function, including any
    /// constant data reported by `CompiledCode::code_info`.
    code_size_bytes: u64,
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
}

#[derive(Clone)]
pub(crate) struct NativeCallee {
    pub(crate) handle: CompiledId,
    pub(crate) func_id: FuncId,
    pub(crate) n_params: usize,
    pub(crate) param_types: Vec<JitValueType>,
    pub(crate) deopt_payload_words: usize,
    pub(crate) return_type: JitValueType,
}

/// Metadata for one member of a co-compiled mutually-recursive group
/// (native-call-ABI slice 4). A `CallGroup { group_index }` lowers to a call of
/// `group[group_index]` by its declared-but-not-yet-defined `FuncId`. Non-chaining
/// (re-run-from-top deopt), so only the callee's shape is needed to marshal the
/// call and size a (discarded-on-bail) payload slot.
#[derive(Clone)]
pub(crate) struct NativeGroupMember {
    pub(crate) func_id: FuncId,
    pub(crate) n_params: usize,
    pub(crate) param_types: Vec<JitValueType>,
    pub(crate) deopt_payload_words: usize,
    pub(crate) return_type: JitValueType,
}

/// Process-wide source of per-module identities, so a [`CompiledId`] minted by one
/// [`NativeModule`] is rejected by another (it would otherwise index a different
/// module's function table). Monotonic; wraparound is not a practical concern.
static NEXT_MODULE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    call_active: std::cell::Cell<bool>,
    deopt_payload: std::cell::RefCell<Vec<i64>>,
    /// Keeps the shared hard-budget reservation alive for the arena's lifetime.
    _memory_reservation: ExecutableMemoryReservation,
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
    flat_args: Vec<FlatBufferArg<'buffers>>,
}

impl<'module, 'buffers> PreparedCall<'module, 'buffers> {
    pub fn scalar(mut self, value: i64) -> Self {
        self.args.push(value);
        self.lens.push(0);
        self
    }

    pub fn readonly_int(mut self, values: &'buffers [i64]) -> Self {
        self.args.push(values.as_ptr() as i64);
        self.lens.push(values.len() as i64);
        self.flat_args.push(FlatBufferArg::Int(values));
        self
    }

    pub fn unique_int_mut(mut self, values: &'buffers mut [i64]) -> Self {
        self.args.push(values.as_mut_ptr() as i64);
        self.lens.push(values.len() as i64);
        self.flat_args.push(FlatBufferArg::IntMut(values));
        self
    }

    pub fn readonly_float(mut self, values: &'buffers [f64]) -> Self {
        self.args.push(values.as_ptr() as i64);
        self.lens.push(values.len() as i64);
        self.flat_args.push(FlatBufferArg::Float(values));
        self
    }

    pub fn unique_float_mut(mut self, values: &'buffers mut [f64]) -> Self {
        self.args.push(values.as_mut_ptr() as i64);
        self.lens.push(values.len() as i64);
        self.flat_args.push(FlatBufferArg::FloatMut(values));
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
        self.module.call_with_host_ctx_at_depth(
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

use crate::deopt::anonymous_deopt;
pub use crate::deopt::{
    DeoptChildSite, DeoptFrame, DeoptMap, DeoptReg, DeoptSite, DeoptValue, NativeOutcome,
    SafepointId,
};

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
            NativeOutcome::CompletedHandle(_) | NativeOutcome::Deopt { .. } => None,
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
            NativeOutcome::Completed(_) | NativeOutcome::Deopt { .. } => None,
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
            NativeOutcome::Deopt { .. } => None,
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
        Self::new_with_opt_inner(helpers, baseline, arena, reservation)
    }

    fn new_with_opt_inner(
        helpers: HostHelpers,
        baseline: bool,
        arena: ArenaMemoryProvider,
        memory_reservation: ExecutableMemoryReservation,
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
        for &helper in HostHelper::all() {
            builder.symbol(helper.symbol(), helpers.addr(helper));
        }
        let mut module = JITModule::new(builder);
        let imports = HostFuncs {
            funcs: HostHelper::all()
                .iter()
                .map(|&helper| {
                    let id = declare_import_for(&mut module, helper.symbol(), &helper.signature())?;
                    Ok((helper, id))
                })
                .collect::<Result<Vec<_>, JitError>>()?,
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
            call_active: std::cell::Cell::new(false),
            deopt_payload: std::cell::RefCell::new(Vec::new()),
            _memory_reservation: memory_reservation,
        })
    }

    /// Compile `function` to native code and return a handle to call it.
    pub fn compile(&mut self, function: &JitFunction) -> Result<CompiledId, JitError> {
        let validated = ValidatedJitFunction::new(function)?;
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
        self.compile_inner(function, None, None, LimitChecks::default())
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
        let validated = ValidatedJitFunction::new(function)?;
        self.compile_inner(
            &validated,
            Some(ForcedDeopt::Site(force_site)),
            None,
            LimitChecks::default(),
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
        let validated = ValidatedJitFunction::new(function)?;
        self.compile_inner(
            &validated,
            Some(ForcedDeopt::All),
            None,
            LimitChecks::default(),
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
        let validated = ValidatedJitFunction::for_osr(function)?;
        self.compile_inner(
            &validated,
            None,
            Some(header_ip),
            LimitChecks {
                step: step_limit,
                cancel: cancel_armed,
            },
        )
    }

    fn compile_inner(
        &mut self,
        validated: &ValidatedJitFunction<'_>,
        forced: Option<ForcedDeopt>,
        osr_header: Option<u32>,
        limit_checks: LimitChecks,
    ) -> Result<CompiledId, JitError> {
        let function = validated.function();
        debug_assert_eq!(
            validated.mode() == validated::ValidationMode::Osr,
            osr_header.is_some()
        );
        // Heap-result return ABI: a non-OSR function whose `Return` source is a
        // `Handle` register returns an output-table handle, not a scalar. Determined
        // purely from the (validated) IR, before codegen. OSR functions never have a
        // top-level `Return` (they exit via `OsrExit`), so this stays `false`.
        let return_type = validated_return_type(function, osr_header.is_some());
        let returns_handle = return_type == Some(JitValueType::Handle);
        let scalar_leaf_callable =
            native_scalar_leaf_callable(function, osr_header.is_some(), returns_handle);
        let native_callees = self.resolve_native_callees(function)?;
        let native_call_depth = native_callees
            .iter()
            .filter_map(|callee| self.funcs.get(callee.handle.index))
            .map(|callee| callee.native_call_depth.saturating_add(1))
            .max()
            .unwrap_or(0);
        let ptr_ty = self.module.target_config().pointer_type();
        self.module.clear_context(&mut self.ctx);
        push_compiled_abi_signature(&mut self.ctx.func, ptr_ty);

        // Declare BEFORE defining (native-call-ABI slice 2): a `CallSelf` must
        // reference this function's own `FuncId`, which only exists once the function
        // is declared. So mint the id now, hand it to `build_function` for self-call
        // lowering, then define the body against it. (Harmless for non-recursive
        // functions, which simply never use the id.)
        let name = format!("rss_jit_{}", self.counter);
        self.counter += 1;
        let id = self
            .module
            .declare_function(&name, Linkage::Local, &self.ctx.func.signature)
            .map_err(|e| err("declare", e))?;

        let deopt_map = build_function(
            &mut self.ctx.func,
            &mut self.fbctx,
            &mut self.module,
            self.imports.clone(),
            function,
            forced,
            osr_header,
            &native_callees,
            id,
            &[],
            limit_checks,
            native_call_depth,
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
        self.module
            .finalize_definitions()
            .map_err(|e| err("finalize", e))?;
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
            code_size_bytes,
            native_call_depth,
            native_depth_cap: native_recursion_depth_cap(function) as u32,
            n_params: function.n_params as usize,
            n_regs: function.n_regs as usize,
            deopt_map,
            requires_limits: limit_checks.any(),
            osr: osr_header.is_some(),
            returns_handle,
            param_types: function.reg_types[..function.n_params as usize].to_vec(),
            reg_types: function.reg_types.clone(),
            return_type,
            scalar_leaf_callable,
        });
        Ok(handle)
    }

    /// Compile a mutually-recursive group together (native-call-ABI slice 4):
    /// **declare every member's `FuncId` first, then build+define each** so a member's
    /// `CallGroup { group_index }` can call a sibling whose body isn't defined yet,
    /// then finalize once. Returns one [`CompiledId`] per member (same order as
    /// `funcs`). Members are scalar, non-OSR, re-run-from-top on deopt, and each
    /// carries the entry depth guard so the cycle cannot overflow the host C stack.
    pub fn compile_recursive_group(
        &mut self,
        funcs: &[JitFunction],
    ) -> Result<Vec<CompiledId>, JitError> {
        if funcs.is_empty() {
            return Ok(Vec::new());
        }
        for function in funcs {
            validate(function, false)?;
        }
        // Complete every contextual check before declaring any Cranelift function.
        // A declaration cannot be rolled back, so rejecting later would poison this
        // NativeModule. Ordinary CallNative edges are temporarily unsupported here:
        // their chained child payload can exceed the n_regs-sized group payload.
        let return_types: Vec<JitValueType> = funcs
            .iter()
            .map(|function| {
                validated_return_type(function, false)
                    .ok_or_else(|| JitError::invalid_ir("recursive group member has no Return"))
            })
            .collect::<Result<_, _>>()?;
        for (member_index, function) in funcs.iter().enumerate() {
            if function
                .code
                .iter()
                .any(|instr| matches!(instr, JitInstr::CallNative { .. }))
            {
                return Err(JitError::invalid_ir(format!(
                    "recursive group member {member_index} contains unsupported CallNative"
                )));
            }
            if function.reg_types[..function.n_params as usize]
                .iter()
                .any(|ty| {
                    !matches!(
                        ty,
                        JitValueType::Int | JitValueType::Bool | JitValueType::Float
                    )
                })
                || !matches!(
                    return_types[member_index],
                    JitValueType::Int | JitValueType::Bool | JitValueType::Float
                )
            {
                return Err(JitError::invalid_ir(format!(
                    "recursive group member {member_index} must use scalar parameters and return"
                )));
            }
            for instr in &function.code {
                let JitInstr::CallGroup {
                    group_index,
                    dst,
                    args,
                } = instr
                else {
                    continue;
                };
                let callee_index = *group_index as usize;
                let Some(callee) = funcs.get(callee_index) else {
                    return Err(JitError::invalid_ir(format!(
                        "CallGroup group_index {callee_index} out of range"
                    )));
                };
                if args.len() != callee.n_params as usize {
                    return Err(JitError::invalid_ir(format!(
                        "CallGroup got {} args, group member {callee_index} expects {}",
                        args.len(),
                        callee.n_params
                    )));
                }
                if function.reg_types[*dst as usize] != return_types[callee_index] {
                    return Err(JitError::invalid_ir(format!(
                        "CallGroup result register {dst} has type {:?}, group member {callee_index} returns {:?}",
                        function.reg_types[*dst as usize], return_types[callee_index]
                    )));
                }
                for (arg_index, (&arg, expected)) in args
                    .iter()
                    .zip(&callee.reg_types[..callee.n_params as usize])
                    .enumerate()
                {
                    let actual = function.reg_types[arg as usize];
                    if actual != *expected {
                        return Err(JitError::invalid_ir(format!(
                            "CallGroup arg {arg_index} has type {actual:?}, group member {callee_index} expects {expected:?}"
                        )));
                    }
                }
            }
        }
        let ptr_ty = self.module.target_config().pointer_type();
        // Phase 1: declare every member + assemble the group metadata.
        let mut func_ids: Vec<FuncId> = Vec::with_capacity(funcs.len());
        let mut group: Vec<NativeGroupMember> = Vec::with_capacity(funcs.len());
        for (function, &return_type) in funcs.iter().zip(&return_types) {
            self.module.clear_context(&mut self.ctx);
            push_compiled_abi_signature(&mut self.ctx.func, ptr_ty);
            let name = format!("rss_jit_{}", self.counter);
            self.counter += 1;
            let id = self
                .module
                .declare_function(&name, Linkage::Local, &self.ctx.func.signature)
                .map_err(|e| err("declare", e))?;
            func_ids.push(id);
            group.push(NativeGroupMember {
                func_id: id,
                n_params: function.n_params as usize,
                param_types: function.reg_types[..function.n_params as usize].to_vec(),
                // Members are scalar (CallGroup/CallSelf are non-chaining), so a
                // member's deopt payload is just its own register window.
                deopt_payload_words: function.n_regs as usize,
                return_type,
            });
        }
        // Phase 2: build + define each member against the declared group ids.
        let mut deopt_maps: Vec<DeoptMap> = Vec::with_capacity(funcs.len());
        let mut code_sizes: Vec<u64> = Vec::with_capacity(funcs.len());
        for (i, function) in funcs.iter().enumerate() {
            let native_callees = self.resolve_native_callees(function)?;
            self.module.clear_context(&mut self.ctx);
            push_compiled_abi_signature(&mut self.ctx.func, ptr_ty);
            let deopt_map = build_function(
                &mut self.ctx.func,
                &mut self.fbctx,
                &mut self.module,
                self.imports.clone(),
                function,
                None,
                None,
                &native_callees,
                func_ids[i],
                &group,
                LimitChecks::default(),
                0,
            )?;
            self.module
                .define_function(func_ids[i], &mut self.ctx)
                .map_err(|e| err("define", e))?;
            let code_size = self
                .ctx
                .compiled_code()
                .map(|code| u64::from(code.code_info().total_size))
                .unwrap_or(0);
            deopt_maps.push(deopt_map);
            code_sizes.push(code_size);
        }
        self.module.clear_context(&mut self.ctx);
        self.module
            .finalize_definitions()
            .map_err(|e| err("finalize", e))?;
        // Phase 3: publish each member and return its handle.
        let mut handles = Vec::with_capacity(funcs.len());
        for (i, function) in funcs.iter().enumerate() {
            let code = self.module.get_finalized_function(func_ids[i]);
            // SAFETY: emitted with the `CompiledAbi` signature declared above.
            let f: CompiledAbi = unsafe { std::mem::transmute::<*const u8, CompiledAbi>(code) };
            let scalar_leaf_callable = native_scalar_leaf_callable(function, false, false);
            let handle = CompiledId {
                module_id: self.id,
                index: self.funcs.len(),
            };
            self.funcs.push(CompiledFunc {
                f,
                id: func_ids[i],
                code_size_bytes: code_sizes[i],
                native_call_depth: 0,
                native_depth_cap: native_recursion_depth_cap(function) as u32,
                n_params: function.n_params as usize,
                n_regs: function.n_regs as usize,
                deopt_map: std::mem::take(&mut deopt_maps[i]),
                requires_limits: false,
                osr: false,
                returns_handle: false,
                param_types: function.reg_types[..function.n_params as usize].to_vec(),
                reg_types: function.reg_types.clone(),
                return_type: Some(return_types[i]),
                scalar_leaf_callable,
            });
            handles.push(handle);
        }
        Ok(handles)
    }

    /// Machine-code bytes emitted for a compiled function, including any constant
    /// data reported by Cranelift. Used only for host-side telemetry.
    pub fn code_size_bytes(&self, id: CompiledId) -> Option<u64> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| func.code_size_bytes)
    }

    /// Longest native-to-native call chain reachable from a compiled function.
    /// Used only for host-side telemetry.
    pub fn native_call_depth(&self, id: CompiledId) -> Option<u32> {
        if id.module_id != self.id {
            return None;
        }
        self.funcs.get(id.index).map(|func| func.native_call_depth)
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
            id,
            args,
            lens,
            host_ctx,
            LogicalCallDepth {
                current: 0,
                limit: usize::MAX,
            },
            limits_ptr,
        )
    }

    fn decode_deopt_live(site: &DeoptSite, payload_base: usize, payload: &[i64]) -> Vec<DeoptReg> {
        site.live
            .iter()
            .filter_map(|&(reg, ty)| {
                let &bits = payload.get(payload_base + reg as usize)?;
                // Heap-aware deopt (heap-aware deopt): only a TRUE scalar (`Int`/`Float`) reg is
                // reconstructible as a deopt value. A non-scalar reg — a `Handle` (heap
                // table index) or a `FlatInt`/`FlatFloat` (raw borrow-pinned buffer
                // pointer) — carries no scalar payload; the interpreter frame already
                // holds its heap `VmValue` (precise resume implies no heap writes), so
                // it MUST NOT be decoded as a raw `Int`/`Float` and written back.
                // Exhaustive on purpose: a new `JitValueType` must choose a side here.
                let value = match ty {
                    JitValueType::Int => DeoptValue::Int(bits),
                    JitValueType::Bool => DeoptValue::Bool(bits != 0),
                    JitValueType::Float => DeoptValue::Float(f64::from_bits(bits as u64)),
                    // A `Handle` carries its heap-table index: the consumer resolves it
                    // against the live JIT heap (heap-aware deopt live-after heap-payload). A flat reg
                    // is a raw borrow-pinned buffer pointer with no such mapping.
                    JitValueType::Handle => DeoptValue::Handle(bits),
                    JitValueType::FlatInt
                    | JitValueType::FlatIntMut
                    | JitValueType::FlatFloat
                    | JitValueType::FlatFloatMut => {
                        return None;
                    }
                };
                Some(DeoptReg { reg, value })
            })
            .collect()
    }

    fn decode_deopt_child(
        &self,
        child: DeoptChildSite,
        payload_base: usize,
        payload: &[i64],
    ) -> Option<Box<DeoptFrame>> {
        let safepoint_bits = *payload.get(payload_base + child.safepoint_slot as usize)?;
        if safepoint_bits <= 0 {
            return None;
        }
        self.decode_deopt_frame(
            child.callee,
            SafepointId(safepoint_bits as u32),
            payload_base + child.payload_slot as usize,
            payload,
        )
        .map(Box::new)
    }

    fn decode_deopt_frame(
        &self,
        function: CompiledId,
        safepoint_id: SafepointId,
        payload_base: usize,
        payload: &[i64],
    ) -> Option<DeoptFrame> {
        if function.module_id != self.id || safepoint_id.0 == 0 {
            return None;
        }
        let func = self.funcs.get(function.index)?;
        let site = func.deopt_map.sites.get(safepoint_id.0 as usize - 1)?;
        let live = Self::decode_deopt_live(site, payload_base, payload);
        let child = site
            .child
            .and_then(|child| self.decode_deopt_child(child, payload_base, payload));
        Some(DeoptFrame {
            function,
            safepoint_id,
            live,
            child,
        })
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
        self.call_inner(id, args, lens, host_ctx, logical_depth, std::ptr::null())
    }

    fn call_inner(
        &self,
        id: CompiledId,
        args: &[i64],
        lens: &[i64],
        host_ctx: HostCtx,
        logical_depth: LogicalCallDepth,
        limits_ptr: *const i64,
    ) -> NativeOutcome {
        // Reject an id from a different module and an out-of-range index: either
        // would invoke the wrong (or no) function. Falling back is always safe.
        if id.module_id != self.id {
            return NativeOutcome::Deopt {
                safepoint_id: SafepointId::ANONYMOUS,
                live: Vec::new(),
                child: None,
                logical_depth: None,
            };
        }
        let func = match self.funcs.get(id.index) {
            Some(func) => func,
            None => {
                return NativeOutcome::Deopt {
                    safepoint_id: SafepointId::ANONYMOUS,
                    live: Vec::new(),
                    child: None,
                    logical_depth: None,
                };
            }
        };
        if func.requires_limits == limits_ptr.is_null() {
            return anonymous_deopt();
        }
        // Ordinary CallNative edges are static. Checking their maximum chain once
        // here removes a depth comparison from every hot child call while still
        // rejecting an acyclic chain before it can exceed the host-stack budget.
        if func.native_call_depth > func.native_depth_cap {
            return anonymous_deopt();
        }
        // The generated entry block reads words from `args_ptr` (and `lens_ptr`)
        // without consulting `n_args`, so a slice shorter than what the entry reads
        // would read out of bounds. A normal compile reads `n_params` packed args; an
        // OSR-entry reads from the full `n_regs`-wide register window. Reject any
        // length mismatch against the required width.
        let required = if func.osr { func.n_regs } else { func.n_params };
        if args.len() != required || lens.len() != required {
            return NativeOutcome::Deopt {
                safepoint_id: SafepointId::ANONYMOUS,
                live: Vec::new(),
                child: None,
                logical_depth: None,
            };
        }
        let Some(_call_guard) = TopLevelCallGuard::enter(&self.call_active) else {
            return NativeOutcome::Deopt {
                safepoint_id: SafepointId::ANONYMOUS,
                live: Vec::new(),
                child: None,
                logical_depth: None,
            };
        };
        let f = func.f;
        let returns_handle = func.returns_handle;
        let deopt_map = &func.deopt_map;
        let mut out: i64 = 0;
        let mut bail = 0_u8;
        let mut safepoint = 0_i64;
        {
            let payload = &self.deopt_payload;
            {
                // Reused per-thread scratch buffer for the deopt payload: a valid
                // pointer for every call, but only written on a bail edge. Grow-only
                // (no per-call zeroing): the success hot path neither allocates nor
                // memsets, and a bail only ever reads slots the generated code just
                // wrote (the live-register set), so stale words in other slots are
                // never observed.
                let mut buf = payload.borrow_mut();
                if buf.len() < deopt_map.payload_words {
                    buf.resize(deopt_map.payload_words, 0);
                }
                let payload_ptr = buf.as_mut_ptr();
                let mut helper_context = HostCallContext {
                    user: host_ctx,
                    bail: &mut bail,
                };
                // SAFETY: `f` was produced by `compile` with the `CompiledAbi`
                // signature; it reads `args.len()` i64s from `args.as_ptr()` and
                // `lens.as_ptr()`, writes one i64 to `&mut out`, and only ever loads
                // (never stores) the `u8` at `bail_ptr` — this thread's `BAIL_FLAG`
                // cell, valid for the call. It only ever *stores* to the `i64` at
                // `safepoint_ptr` (the symmetric write-direction-opposite of
                // `bail_ptr`) — this thread's `SAFEPOINT_ID` cell, also valid for the
                // call, and only on a bail edge (the hot path never touches it).
                // `payload_ptr` addresses this thread's `DEOPT_PAYLOAD` buffer, sized
                // to `deopt_map.payload_words` above and held borrowed (so it stays
                // valid and immovable) for the whole call; the generated code only
                // ever *stores* live register words and copied child-frame payloads
                // into it, and only on a bail edge (the hot path never touches it).
                // Any flat-array data pointer in `args` is read in-bounds (against
                // the matching `lens` entry) per the caller's borrow-protocol
                // obligation documented above. The generated code never retains any
                // of the pointers.
                let mut frame = JitCallFrame {
                    abi_version: JIT_CALL_ABI_VERSION,
                    flags: 0,
                    args: args.as_ptr(),
                    lens: lens.as_ptr(),
                    arg_count: args.len(),
                    host_ctx: (&mut helper_context as *mut HostCallContext) as HostCtx,
                    limits: limits_ptr,
                    result: &mut out,
                    bail: &mut bail,
                    safepoint: &mut safepoint,
                    deopt: payload_ptr,
                    native_depth: 0,
                    logical_depth: logical_depth.current,
                    logical_depth_limit: logical_depth.limit,
                };
                // SAFETY: `f` was finalized from the one-pointer `CompiledAbi`
                // signature and every pointer in `frame` remains live for the call.
                let completed = unsafe { f(&mut frame) };
                if completed == JitStatus::Completed && bail == 0 {
                    // Success: leave the payload buffer untouched, build no Vec.
                    // Heap-result return ABI: a Handle-returning function's
                    // `out` is an output-table handle, signalled distinctly so the
                    // host materializes a heap value from it. The scalar path is
                    // byte-for-byte unchanged. the transactional fallback contract: this branch runs ONLY on a
                    // clean completion (`completed != 0 && bail.get() == 0`); any
                    // bail takes the `else` (`Deopt`) arm, so no heap result is
                    // ever reported on a bailed attempt.
                    if returns_handle {
                        NativeOutcome::CompletedHandle(out)
                    } else {
                        NativeOutcome::Completed(out)
                    }
                } else {
                    let safepoint_id = SafepointId(safepoint as u32);
                    // Decode the captured live registers via the deopt state-map state-map.
                    // A real bail site (id >= 1) names a `sites[id - 1]` entry; an
                    // anonymous bail (id 0, e.g. fell off the end) has no site, so
                    // `live` is empty.
                    let frame = self.decode_deopt_frame(id, safepoint_id, 0, &buf);
                    NativeOutcome::Deopt {
                        safepoint_id,
                        live: frame
                            .as_ref()
                            .map_or_else(Vec::new, |frame| frame.live.clone()),
                        child: frame.and_then(|frame| frame.child),
                        logical_depth: func.osr.then_some(out.max(0) as usize),
                    }
                }
            }
        }
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

struct TopLevelCallGuard<'a>(&'a std::cell::Cell<bool>);

impl<'a> TopLevelCallGuard<'a> {
    fn enter(active: &'a std::cell::Cell<bool>) -> Option<Self> {
        (!active.replace(true)).then_some(Self(active))
    }
}

impl Drop for TopLevelCallGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
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
