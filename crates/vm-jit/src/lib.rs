//! Native (Cranelift) baseline JIT for the RSScript register VM's numeric /
//! boolean / control-flow core — the native tier of
//! `docs/spec/RSScript_Execution_Spec_v0.1.md` (§7; status in Appendix B).
//!
//! # What it compiles
//!
//! A [`JitFunction`] is a stable, versioned slice of the VM's bytecode: the subset
//! that operates on unboxed scalar registers — `i64` (integers, and booleans as
//! `0`/`1`) and `f64` (floats) — plus *side-effect-free* heap **reads** of `Int`
//! struct fields and list elements (via [`HostHelpers`]). It has no heap writes,
//! no general calls, no async, and no other side effects. The main `rsscript` crate
//! translates an eligible `RegFunction` into this IR; everything outside the subset
//! stays on the interpreter (per-function fallback). [`NativeModule::compile`]
//! re-validates the IR ([`validate`]) before codegen, so a malformed producer fails
//! as a clean [`JitError`] rather than panicking or miscompiling.
//!
//! # Why a separate crate
//!
//! `rsscript` is `#![forbid(unsafe_code)]`. Executing generated machine code and
//! transmuting a code pointer to a callable function require `unsafe`, so they
//! live here behind a **safe** API ([`NativeModule::call`]): the only `unsafe` is
//! the call through a pointer whose ABI this crate itself emitted, so the safety
//! invariant is locally verifiable.
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
//! bail. Because the compiled subset is side-effect-free (reads only), the caller
//! can then simply re-run the function on the interpreter, which is the single
//! source of semantic truth. So the native tier can only ever be *faster*, never
//! different.

use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{AbiParam, Block, InstBuilder, MemFlags, Value, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module, default_libcall_names};

/// `(struct_handle, slot) -> i64`: the struct's `slot`-th field as an `Int`.
pub type FieldIntFn = extern "C" fn(i64, i64) -> i64;
/// `(list_handle) -> i64`: list length.
pub type ListLenFn = extern "C" fn(i64) -> i64;
/// `(list_handle, index) -> i64`: the list element at `index` as an `Int`.
pub type ListGetIntFn = extern "C" fn(i64, i64) -> i64;
/// `(struct_handle, slot) -> f64`: the struct's `slot`-th field as a `Float`.
/// A wrong-type/out-of-range field signals a bail out-of-band (the f64 return
/// channel needs no tagging — the bail flag is separate), so the returned value
/// is unused on failure.
pub type FieldFloatFn = extern "C" fn(i64, i64) -> f64;
/// `(list_handle, index) -> f64`: the list element at `index` as a `Float`.
/// Like [`FieldFloatFn`], a wrong-type/out-of-bounds element signals a bail.
pub type ListGetFloatFn = extern "C" fn(i64, i64) -> f64;
/// `(closure_handle) -> i64`: the closure's underlying function id (its stable
/// callee identity). Returns `-1` for a non-closure / invalid handle, which never
/// matches a real (`>= 0`) function id, so the identity guard then bails. Total —
/// it never sets the out-of-band bail flag.
pub type ClosureIdFn = extern "C" fn(i64) -> i64;

/// Host helper functions the compiled code calls to read heap values (struct
/// fields, list elements) that don't fit in a scalar register. The `rsscript`
/// crate supplies these `extern "C"` functions; they look the value up in a
/// per-call table the VM populates and return it unboxed as `i64`, signalling any
/// type/bounds mismatch out-of-band (the VM checks and falls back). The native
/// code just calls and uses the result.
///
/// These are **typed** function pointers, not raw `*const u8`: a safe caller can
/// only supply a real `extern "C"` function with the matching signature, so the
/// raw-address-to-symbol conversion (which is the part with an actual safety
/// obligation) stays private to this crate. The conversion to the `*const u8`
/// that Cranelift's symbol table wants happens in [`NativeModule::new`].
#[derive(Clone, Copy)]
pub struct HostHelpers {
    pub field_int: FieldIntFn,
    pub list_len: ListLenFn,
    pub list_get_int: ListGetIntFn,
    pub field_float: FieldFloatFn,
    pub list_get_float: ListGetFloatFn,
    pub closure_id: ClosureIdFn,
}

/// Version of the [`JitInstr`]/[`JitFunction`] IR this crate consumes. The
/// producer (`rsscript`) translates its private bytecode into this stable,
/// versioned surface, so the two crates are decoupled: a breaking IR change bumps
/// this and the producer is updated in lock-step.
pub const IR_VERSION: u32 = 6;

/// Signed integer comparison (the four ordered comparisons; equality is its own
/// instruction so it can also apply to booleans).
#[derive(Debug, Clone, Copy)]
pub enum JitCompare {
    Lt,
    Le,
    Gt,
    Ge,
}

impl JitCompare {
    fn cc(self) -> IntCC {
        match self {
            JitCompare::Lt => IntCC::SignedLessThan,
            JitCompare::Le => IntCC::SignedLessThanOrEqual,
            JitCompare::Gt => IntCC::SignedGreaterThan,
            JitCompare::Ge => IntCC::SignedGreaterThanOrEqual,
        }
    }

    /// Ordered float comparison (NaN → false), matching Rust's `<`/`<=`/`>`/`>=`
    /// on `f64` (the interpreter's float comparison).
    fn fcc(self) -> FloatCC {
        match self {
            JitCompare::Lt => FloatCC::LessThan,
            JitCompare::Le => FloatCC::LessThanOrEqual,
            JitCompare::Gt => FloatCC::GreaterThan,
            JitCompare::Ge => FloatCC::GreaterThanOrEqual,
        }
    }
}

/// One instruction of the JIT IR. Registers are `u32` indices; jump `target`s are
/// indices into the function's instruction vector (matching the VM's bytecode, so
/// translation is 1:1 and target indices need no remapping).
#[derive(Debug, Clone)]
pub enum JitInstr {
    /// Placeholder that preserves 1:1 index alignment with the source bytecode
    /// (e.g. a deep-copy of an `Int`, which is a no-op on an unboxed register).
    Nop,
    LoadInt {
        dst: u32,
        value: i64,
    },
    LoadFloat {
        dst: u32,
        value: f64,
    },
    LoadBool {
        dst: u32,
        value: bool,
    },
    Move {
        dst: u32,
        src: u32,
    },
    Add {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Sub {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Mul {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Div {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Mod {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    BitAnd {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    BitOr {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    BitXor {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Shl {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Shr {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    /// `dst = (lhs <op> rhs) as 0/1`.
    Compare {
        dst: u32,
        op: JitCompare,
        lhs: u32,
        rhs: u32,
    },
    Equal {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    NotEqual {
        dst: u32,
        lhs: u32,
        rhs: u32,
    },
    Jump {
        target: u32,
    },
    JumpIfBool {
        cond: u32,
        expected: bool,
        target: u32,
    },
    JumpIfIntCompare {
        lhs: u32,
        rhs: u32,
        op: JitCompare,
        expected: bool,
        target: u32,
    },
    Return {
        src: u32,
    },
    /// Unconditionally bail to the interpreter (e.g. a `RuntimeError` instruction:
    /// re-running on the interpreter reproduces the exact error).
    Bail,
    /// `dst = field[slot]` of the struct/variant handle in `base`, read as `Int`.
    /// Compiles to a call to [`HostHelpers::field_int`].
    FieldInt {
        dst: u32,
        base: u32,
        slot: u32,
    },
    /// `dst = len` of the list handle in `base`. Calls [`HostHelpers::list_len`].
    ListLen {
        dst: u32,
        base: u32,
    },
    /// `dst = list[index]` (as `Int`) of the list handle in `base`. Calls
    /// [`HostHelpers::list_get_int`]; an out-of-bounds/non-int element makes the
    /// helper flag a fallback (the VM re-runs on the interpreter).
    ListGetInt {
        dst: u32,
        base: u32,
        index: u32,
    },
    /// `dst = field[slot]` of the struct/variant handle in `base`, read as a
    /// `Float`. Compiles to a call to [`HostHelpers::field_float`]; a wrong-type
    /// field makes the helper flag a fallback. `dst` is a Float-class register.
    FieldFloat {
        dst: u32,
        base: u32,
        slot: u32,
    },
    /// `dst = list[index]` (as `Float`) of the list handle in `base`. Calls
    /// [`HostHelpers::list_get_float`]; an out-of-bounds/non-float element makes
    /// the helper flag a fallback. `dst` is a Float-class register.
    ListGetFloat {
        dst: u32,
        base: u32,
        index: u32,
    },
    /// TV2 direct read: `dst = base_ptr[index]` where `base` is a **`FlatInt`**
    /// param register holding the raw `*const i64` of a flat `List<Int>` buffer.
    /// Bounds-checked against the param's length (the `lens` slot for `base`); an
    /// out-of-bounds index branches to fallback exactly like the helper's bail —
    /// no per-element host call. `dst` is an `Int` register.
    ListGetIntDirect {
        dst: u32,
        base: u32,
        index: u32,
    },
    /// TV2 direct read: `dst = base_ptr[index]` where `base` is a **`FlatFloat`**
    /// param register holding the raw `*const f64` of a flat `List<Float>` buffer.
    /// Bounds-checked against the param's length; OOB → fallback. `dst` is a
    /// `Float` register.
    ListGetFloatDirect {
        dst: u32,
        base: u32,
        index: u32,
    },
    /// TV2 direct read: `dst = len` of the flat-array param `base` (a `FlatInt` or
    /// `FlatFloat` register). Reads the length from the param's `lens` slot — no
    /// host call. `dst` is an `Int` register.
    ListLenDirect {
        dst: u32,
        base: u32,
    },
    /// Profile-guided monomorphic inlining guard (J2). `base` is a `Handle`
    /// register holding a closure handle; reads its underlying function id via
    /// [`HostHelpers::closure_id`] and, if it differs from `expected`, **bails** to
    /// the interpreter (the existing re-run-from-top fallback). Emitted just before
    /// the inlined body of the profiled-monomorphic callee, so a different callee
    /// than the one speculated never runs native code. Writes no register; the
    /// matching (hot) path falls through with zero extra work beyond the compare.
    GuardClosureId {
        base: u32,
        expected: i64,
    },
    /// Profile-guided polymorphic inline cache (J2.2). `base` is a `Handle`
    /// register holding a closure handle; reads its underlying function id via
    /// [`HostHelpers::closure_id`] (a single, total helper call — `-1` for a
    /// non-closure handle) into the `Int` register `dst`. The id is then compared
    /// against each speculated callee key with ordinary integer
    /// compare/branch instructions; the no-match arm reuses the standard
    /// re-run-from-top [`Bail`]. Unlike [`GuardClosureId`] (which bails on the
    /// spot), this only materializes the id so the dispatcher can select among
    /// 2–3 inlined callee bodies. Writes `dst`.
    ClosureId {
        dst: u32,
        base: u32,
    },
    /// OSR-exit (J5.2). Marks the post-loop instruction at the loop's exit edge: a
    /// function compiled with an OSR-entry (see [`NativeModule::compile_osr`]) runs
    /// only the loop region natively, so reaching this instruction means the loop
    /// has exited and control must return to the interpreter. It lowers to an
    /// *unconditional* deopt safepoint whose `resume_ip` is this instruction's own
    /// index and whose `live` set is the registers definitely assigned on entry to
    /// it (the loop's live-out): the captured live-out window plus `resume_ip` are
    /// surfaced via [`NativeOutcome::Deopt`], and the host resumes the interpreter
    /// there (the precise-deopt resume path). Only valid in an OSR compilation; the
    /// default [`compile`](NativeModule::compile) path never emits it.
    OsrExit,
}

/// Storage class of a register: an unboxed `i64` (integers and booleans) or an
/// unboxed `f64` (floats). The arithmetic/compare instructions are
/// type-polymorphic — the same `Add`/`Compare`/… opcode lowers to integer or
/// float machine ops depending on the operand registers' types (mirroring the
/// VM, where `AddInt` etc. dispatch on the runtime `VmValue`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitValueType {
    Int,
    Float,
    /// An opaque handle (index into the VM's per-call heap-value table) to a heap
    /// value — a struct/list/etc. — that can't live in a scalar register. Stored
    /// as `i64`; only valid as the `base` of a heap-read instruction.
    Handle,
    /// TV2: a flat `List<Int>` param passed as a raw `*const i64` data pointer (in
    /// the args word) plus its element count (in the parallel `lens` word). Stored
    /// as `i64` (the pointer bits); only valid as the `base` of a `*Direct` read.
    FlatInt,
    /// TV2: a flat `List<Float>` param passed as a raw `*const f64` data pointer
    /// plus its element count. Only valid as the `base` of a `*Direct` read.
    FlatFloat,
}

/// A compilable function: register count, per-register storage class, and the
/// instruction stream. Callers unbox each argument to `i64` bits (an `f64`'s bit
/// pattern for float registers) and read the result back the same way.
#[derive(Debug, Clone)]
pub struct JitFunction {
    pub n_params: u32,
    pub n_regs: u32,
    /// Storage class per register index (length `n_regs`).
    pub reg_types: Vec<JitValueType>,
    pub code: Vec<JitInstr>,
}

impl JitFunction {
    fn is_float(&self, reg: u32) -> bool {
        self.reg_types[reg as usize] == JitValueType::Float
    }
}

/// The native ABI of every compiled function:
/// `(args_ptr, n_args, lens_ptr, out_ptr, bail_ptr, safepoint_ptr) -> completed`.
/// Returns `1` and writes the result to `*out` on success, or `0` (leaving `*out`
/// untouched) to request fallback. `lens_ptr` points at an `i64` array parallel to
/// `args`: for a TV2 flat-array param (`FlatInt`/`FlatFloat`) the args word holds
/// the raw data pointer and the `lens` word holds the element count (for in-register
/// bounds-checked direct reads); other params' `lens` words are unused. `bail_ptr`
/// points at a `u8` flag the host helpers set when a heap read can't be satisfied;
/// the generated code loads it after every helper call and branches to fallback
/// immediately, so a bad read can't keep executing. `safepoint_ptr` points at a
/// host-owned `i64` cell into which the generated code *stores* the unique
/// [`SafepointId`] of the bail site on the bail edge (and only there — the hot
/// fall-through path never touches it); `0` means no bail was recorded.
/// `payload_ptr` points at a host-owned `i64` array of width `n_regs` into which
/// the generated code *stores* each live register's value on the bail edge only
/// (slot `reg` ← that register's 8-byte word; an f64 register's bit pattern lands
/// in its slot). The hot fall-through path never writes it.
type CompiledAbi = unsafe extern "C" fn(
    *const i64,
    usize,
    *const i64,
    *mut i64,
    *const u8,
    *mut i64,
    *mut i64,
) -> u8;

#[derive(Debug)]
pub struct JitError(pub String);

impl std::fmt::Display for JitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "vm-jit error: {}", self.0)
    }
}
impl std::error::Error for JitError {}

fn err(context: &str, e: impl std::fmt::Display) -> JitError {
    JitError(format!("{context}: {e}"))
}

/// Declare an imported host helper with `n_args` `i64` params and an `i64` result.
fn declare_import(module: &mut JITModule, name: &str, n_args: usize) -> Result<FuncId, JitError> {
    let mut sig = module.make_signature();
    for _ in 0..n_args {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::I64));
    module
        .declare_function(name, Linkage::Import, &sig)
        .map_err(|e| err("declare import", e))
}

/// Declare an imported host helper with `n_args` `i64` params and an `f64` result
/// (the Float read helpers). The bail signal stays out-of-band (the shared bail
/// flag), so the f64 return channel carries only the value.
fn declare_import_f64(
    module: &mut JITModule,
    name: &str,
    n_args: usize,
) -> Result<FuncId, JitError> {
    let mut sig = module.make_signature();
    for _ in 0..n_args {
        sig.params.push(AbiParam::new(types::I64));
    }
    sig.returns.push(AbiParam::new(types::F64));
    module
        .declare_function(name, Linkage::Import, &sig)
        .map_err(|e| err("declare import", e))
}

/// A compiled function plus the metadata `call` needs to invoke it safely: the
/// param count, so `call` can reject an argument slice of the wrong length (the
/// generated entry block reads exactly `n_params` words from `args_ptr` and does
/// not bound-check against `n_args`).
struct CompiledFunc {
    f: CompiledAbi,
    n_params: usize,
    /// Register count of the source [`JitFunction`] (the width of each site's
    /// register space; future slices size their payload by it).
    n_regs: usize,
    /// Per-safepoint deopt state-map (resume_ip + live registers), built host-side
    /// during `compile`. See [`DeoptMap`].
    deopt_map: DeoptMap,
    /// OSR-entry (J5.2): when `true`, the function was compiled with
    /// [`compile_osr`](NativeModule::compile_osr). Its `args_ptr` is the
    /// interpreter's full `n_regs`-wide register *window* (indexed by register),
    /// not a packed `n_params` arg array, so `call` validates the slice length
    /// against `n_regs` instead of `n_params`.
    osr: bool,
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
}

/// `FuncId`s of the declared host helpers, resolved into per-function `FuncRef`s
/// at codegen time.
#[derive(Clone, Copy)]
struct HostFuncs {
    field_int: FuncId,
    list_len: FuncId,
    list_get_int: FuncId,
    field_float: FuncId,
    list_get_float: FuncId,
    closure_id: FuncId,
}

/// Handle to a function compiled into a [`NativeModule`]. Carries the minting
/// module's identity so it can't be used against a different module (which would
/// index the wrong function table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledId {
    module_id: u64,
    index: usize,
}

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
/// This slice (J0.1a) only *computes and stores* the map; nothing reads it into the
/// running ABI yet (no payload is captured, no emitted code changes). It is the
/// schema foundation for J0.1b's payload capture and J0.2's reconstruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeoptSite {
    /// The `JitInstr` index to resume interpretation at (the bailing instruction).
    pub resume_ip: u32,
    /// Registers definitely assigned on entry to `resume_ip`, each `(reg, type)`.
    pub live: Vec<(u32, JitValueType)>,
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
}

/// The runtime value of a live register captured at a deopt, typed by its storage
/// class so the caller can reconstruct it faithfully (an `i64` integer/boolean, or
/// an exact `f64`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeoptValue {
    /// An integer (or boolean `0`/`1`) register's value.
    Int(i64),
    /// A float register's value (decoded from its captured 8-byte bit pattern).
    Float(f64),
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

/// Outcome of running a compiled function via [`NativeModule::call`]: either the
/// function ran to completion with a 64-bit result, or it deopted at a named
/// safepoint and the interpreter should re-run it.
#[derive(Debug, Clone, PartialEq)]
pub enum NativeOutcome {
    /// The function completed; the payload is the result bits (an `i64`, or an
    /// `f64` bit pattern for a float-returning function).
    Completed(i64),
    /// The function deopted at `safepoint_id` (a guard bail or a host-helper bail)
    /// and the caller must fall back to the interpreter. `live` carries each
    /// register definitely assigned at the resume point with its captured value
    /// (per the J0.1a state-map); it is empty for a deopt rejected before the call
    /// (id/length mismatch). The caller's behavior depends on its mode (J0.2): by
    /// default it re-runs the function from the top and ignores `live` (sound for the
    /// side-effect-free subset); with precise deopt enabled (`RSS_JIT_PRECISE_DEOPT`)
    /// it consumes `live` to reconstruct the interpreter window and resumes at the
    /// safepoint's `resume_ip` instead.
    Deopt {
        safepoint_id: SafepointId,
        live: Vec<DeoptReg>,
    },
}

impl NativeOutcome {
    /// The completed result, or `None` on a deopt. Convenience for callers that
    /// only care whether the call produced a value.
    pub fn completed(self) -> Option<i64> {
        match self {
            NativeOutcome::Completed(value) => Some(value),
            NativeOutcome::Deopt { .. } => None,
        }
    }
}

impl NativeModule {
    /// Optimizing native tier (back-compat default): `opt_level="speed"`.
    pub fn new(helpers: HostHelpers) -> Result<Self, JitError> {
        Self::new_with_opt(helpers, false)
    }

    /// Build a native module at a selectable optimization level.
    ///
    /// `baseline == true` selects the Phase-2 path-B **baseline tier**:
    /// `opt_level="none"`. Everything else — IR translation, host helpers, the
    /// bail-flag deopt protocol — is byte-for-byte identical to the optimizing
    /// path; only the Cranelift ISA `opt_level` flag changes. The win is
    /// *compile latency* (less codegen work), at the cost of slightly less
    /// optimized machine code. Because the compiled subset is the
    /// side-effect-free scalar + read-only-heap set, the interpreter/`run_jit`
    /// deopt oracle remains valid verbatim regardless of opt level.
    ///
    /// `baseline == false` keeps the optimizing hot-path tier (`opt_level="speed"`).
    pub fn new_with_opt(helpers: HostHelpers, baseline: bool) -> Result<Self, JitError> {
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
        // Register the host helper addresses so imported calls link to them.
        // The typed `extern "C"` pointers become the `*const u8` Cranelift's symbol
        // table wants here, where this crate owns the obligation that the address
        // matches the imported signature declared just below.
        builder.symbol("rss_jit_field_int", helpers.field_int as *const u8);
        builder.symbol("rss_jit_list_len", helpers.list_len as *const u8);
        builder.symbol("rss_jit_list_get_int", helpers.list_get_int as *const u8);
        builder.symbol("rss_jit_field_float", helpers.field_float as *const u8);
        builder.symbol("rss_jit_list_get_float", helpers.list_get_float as *const u8);
        builder.symbol("rss_jit_closure_id", helpers.closure_id as *const u8);
        let mut module = JITModule::new(builder);
        let imports = HostFuncs {
            field_int: declare_import(&mut module, "rss_jit_field_int", 2)?,
            list_len: declare_import(&mut module, "rss_jit_list_len", 1)?,
            list_get_int: declare_import(&mut module, "rss_jit_list_get_int", 2)?,
            field_float: declare_import_f64(&mut module, "rss_jit_field_float", 2)?,
            list_get_float: declare_import_f64(&mut module, "rss_jit_list_get_float", 2)?,
            closure_id: declare_import(&mut module, "rss_jit_closure_id", 1)?,
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
        })
    }

    /// Compile `function` to native code and return a handle to call it.
    pub fn compile(&mut self, function: &JitFunction) -> Result<CompiledId, JitError> {
        self.compile_inner(function, None, None)
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
        self.compile_inner(function, Some(force_site), None)
    }

    /// Compile `function` as an **OSR (on-stack replacement) entry** at `header_ip`
    /// (J5.2). Instead of the normal param-loading entry, the generated function's
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
    pub fn compile_osr(
        &mut self,
        function: &JitFunction,
        header_ip: u32,
    ) -> Result<CompiledId, JitError> {
        self.compile_inner(function, None, Some(header_ip))
    }

    fn compile_inner(
        &mut self,
        function: &JitFunction,
        forced: Option<u32>,
        osr_header: Option<u32>,
    ) -> Result<CompiledId, JitError> {
        // `JitFunction` is a public, versioned surface: a malformed producer must
        // fail cleanly here, not panic inside `build_function` (out-of-range index)
        // or trip Cranelift's verifier (a type mismatch) deep in codegen.
        validate(function)?;
        let ptr_ty = self.module.target_config().pointer_type();
        self.module.clear_context(&mut self.ctx);
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // args ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // n_args
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // lens ptr (TV2)
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // out ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // bail flag ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // safepoint id out ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // deopt payload out ptr
        self.ctx
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I8));

        let deopt_map = build_function(
            &mut self.ctx.func,
            &mut self.fbctx,
            &mut self.module,
            self.imports,
            function,
            forced,
            osr_header,
        )?;

        let name = format!("rss_jit_{}", self.counter);
        self.counter += 1;
        let id = self
            .module
            .declare_function(&name, Linkage::Local, &self.ctx.func.signature)
            .map_err(|e| err("declare", e))?;
        self.module
            .define_function(id, &mut self.ctx)
            .map_err(|e| err("define", e))?;
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
            n_params: function.n_params as usize,
            n_regs: function.n_regs as usize,
            deopt_map,
            osr: osr_header.is_some(),
        });
        Ok(handle)
    }

    /// Run a compiled function. Returns [`NativeOutcome::Completed`] with the result
    /// on completion, or [`NativeOutcome::Deopt`] when the native code bailed and the
    /// interpreter should re-run the function — either a guard bail (overflow/
    /// divide-by-zero edge) or a host-helper bail (an unsatisfiable heap read; see
    /// [`signal_bail`]). The returned [`SafepointId`] identifies the exact bail site
    /// (codegen numbers sites from 1; [`SafepointId::ANONYMOUS`] / `0` means no site
    /// recorded an id, e.g. an id/length mismatch rejected before the call).
    ///
    /// This is a **fully safe** boundary for scalar/handle args. The bail flag is a
    /// per-thread `u8` owned by this crate; `call` resets it, passes its own address
    /// into the generated code, and reports a set flag as a fallback.
    ///
    /// `args` and `lens` are parallel slices indexed by param (both length
    /// `n_params`). For a TV2 flat-array param the caller places the raw data
    /// pointer (`*const i64`/`*const f64` reinterpreted as `i64`) in `args[i]` and
    /// the element count in `lens[i]`. **SAFETY (TV2 borrow protocol — caller
    /// obligation):** any pointer placed in `args` for a flat-array param must point
    /// at a buffer that stays allocated, immovable, and unmutated for the entire
    /// duration of this call. The generated code reads at most `lens[i]` consecutive
    /// elements from it (every index is bounds-checked against `lens[i]` →
    /// `signal_bail` on OOB) and never writes or retains it. The VM caller satisfies
    /// this by pinning a shared `Ref` borrow of the backing `RefCell<TypedVec>` for
    /// the call (so no `borrow_mut`/realloc can occur); see `try_native`.
    pub fn call(&self, id: CompiledId, args: &[i64], lens: &[i64]) -> NativeOutcome {
        // Reject an id from a different module and an out-of-range index: either
        // would invoke the wrong (or no) function. Falling back is always safe.
        if id.module_id != self.id {
            return NativeOutcome::Deopt {
                safepoint_id: SafepointId::ANONYMOUS,
                live: Vec::new(),
            };
        }
        let func = match self.funcs.get(id.index) {
            Some(func) => func,
            None => {
                return NativeOutcome::Deopt {
                    safepoint_id: SafepointId::ANONYMOUS,
                    live: Vec::new(),
                }
            }
        };
        // The generated entry block reads words from `args_ptr` (and `lens_ptr`)
        // without consulting `n_args`, so a slice shorter than what the entry reads
        // would read out of bounds. A normal compile reads `n_params` packed args; an
        // OSR-entry reads from the full `n_regs`-wide register window. Reject any
        // length mismatch against the required width.
        let required = if func.osr {
            func.n_regs
        } else {
            func.n_params
        };
        if args.len() != required || lens.len() != required {
            return NativeOutcome::Deopt {
                safepoint_id: SafepointId::ANONYMOUS,
                live: Vec::new(),
            };
        }
        let f = func.f;
        let n_regs = func.n_regs;
        let deopt_map = &func.deopt_map;
        let mut out: i64 = 0;
        BAIL_FLAG.with(|bail| {
            SAFEPOINT_ID.with(|safepoint| {
                DEOPT_PAYLOAD.with(|payload| {
                    bail.set(0);
                    safepoint.set(0);
                    let bail_ptr = bail.as_ptr() as *const u8;
                    let safepoint_ptr = safepoint.as_ptr();
                    // Reused per-thread scratch buffer for the deopt payload: a valid
                    // pointer for every call, but only written on a bail edge. Grow-only
                    // (no per-call zeroing): the success hot path neither allocates nor
                    // memsets, and a bail only ever reads slots the generated code just
                    // wrote (the live-register set), so stale words in other slots are
                    // never observed.
                    let payload_ptr = {
                        let mut buf = payload.borrow_mut();
                        if buf.len() < n_regs {
                            buf.resize(n_regs, 0);
                        }
                        buf.as_mut_ptr()
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
                    // to `n_regs` words above and held borrowed (so it stays valid and
                    // immovable) for the whole call; the generated code only ever
                    // *stores* a live register's word into its slot, and only on a bail
                    // edge (the hot path never touches it). Any flat-array data pointer
                    // in `args` is read in-bounds (against the matching `lens` entry) per
                    // the caller's borrow-protocol obligation documented above. The
                    // generated code never retains any of the pointers.
                    let completed = unsafe {
                        f(
                            args.as_ptr(),
                            args.len(),
                            lens.as_ptr(),
                            &mut out as *mut i64,
                            bail_ptr,
                            safepoint_ptr,
                            payload_ptr,
                        )
                    };
                    if completed != 0 && bail.get() == 0 {
                        // Success: leave the payload buffer untouched, build no Vec.
                        NativeOutcome::Completed(out)
                    } else {
                        let safepoint_id = SafepointId(safepoint.get() as u32);
                        // Decode the captured live registers via the J0.1a state-map.
                        // A real bail site (id >= 1) names a `sites[id - 1]` entry; an
                        // anonymous bail (id 0, e.g. fell off the end) has no site, so
                        // `live` is empty.
                        let buf = payload.borrow();
                        let live = deopt_map
                            .sites
                            .get((safepoint.get() as usize).wrapping_sub(1))
                            .map(|site| {
                                site.live
                                    .iter()
                                    .filter_map(|&(reg, ty)| {
                                        buf.get(reg as usize).map(|&bits| {
                                            let value = match ty {
                                                JitValueType::Float => {
                                                    DeoptValue::Float(f64::from_bits(bits as u64))
                                                }
                                                _ => DeoptValue::Int(bits),
                                            };
                                            DeoptReg { reg, value }
                                        })
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        NativeOutcome::Deopt { safepoint_id, live }
                    }
                })
            })
        })
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

std::thread_local! {
    /// Per-thread bail flag shared between the in-flight compiled call (which loads
    /// it) and the host helpers (which set it via [`signal_bail`]). `call` resets it
    /// before each invocation, so it is only meaningful during a call.
    static BAIL_FLAG: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };

    /// Per-thread safepoint-id cell the in-flight compiled call *stores* into on a
    /// bail edge (mirrors [`BAIL_FLAG`], opposite write direction). `call` resets it
    /// to `0` before each invocation, so it is only meaningful during a call: `0`
    /// means no bail site fired; a non-zero value is the [`SafepointId`] of the site
    /// that bailed.
    static SAFEPOINT_ID: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };

    /// Per-thread reused scratch buffer for the deopt payload (one `i64` slot per
    /// VM register). `call` resizes it to the function's `n_regs` and passes its
    /// pointer into the compiled function, which *stores* each live register's value
    /// into its slot on a bail edge only. Reused across calls so the success hot
    /// path performs no steady-state allocation.
    static DEOPT_PAYLOAD: std::cell::RefCell<Vec<i64>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Signal from a [`HostHelpers`] callback that the in-flight native call cannot be
/// satisfied (wrong type / out-of-bounds heap read), so the function must fall
/// back to the interpreter. The generated code loads the flag immediately after
/// each helper call and branches to fallback when it is set; [`NativeModule::call`]
/// also reports it. Safe to call any time — it is a no-op outside a `call`, since
/// `call` resets the flag at the start of every invocation.
pub fn signal_bail() {
    BAIL_FLAG.with(|flag| flag.set(1));
}

/// Validate public IR before codegen. `build_function` assumes well-formed input
/// (it indexes `reg_types`/`code` directly and relies on Cranelift register types
/// matching each opcode); this turns every such assumption into a clean
/// [`JitError`] so a buggy producer can never reach codegen with input that would
/// panic or generate invalid assumptions.
///
/// Storage-class rules mirror `build_function`'s lowering: arithmetic preserves the
/// operand class (and forbids `Handle`), the int-only ops (`Mod`, bit/shift) require
/// `Int`, comparisons yield an `Int` boolean, and `Handle` registers are only valid
/// as the `base` of a heap read.
fn validate(program: &JitFunction) -> Result<(), JitError> {
    let n_regs = program.n_regs as usize;
    let n = program.code.len();

    if program.reg_types.len() != n_regs {
        return Err(JitError(format!(
            "reg_types length {} does not match n_regs {n_regs}",
            program.reg_types.len()
        )));
    }
    if program.n_params > program.n_regs {
        return Err(JitError(format!(
            "n_params {} exceeds n_regs {n_regs}",
            program.n_params
        )));
    }

    let check_reg = |r: u32| -> Result<(), JitError> {
        if (r as usize) < n_regs {
            Ok(())
        } else {
            Err(JitError(format!(
                "register {r} out of range (n_regs {n_regs})"
            )))
        }
    };
    let class = |r: u32| program.reg_types[r as usize];
    // Non-scalar register classes (opaque handle or flat-array pointer): valid only
    // as the `base` of a heap/direct read, never in scalar/arith/move/return.
    let is_nonscalar = |r: u32| {
        matches!(
            class(r),
            JitValueType::Handle | JitValueType::FlatInt | JitValueType::FlatFloat
        )
    };
    let check_target = |t: u32| -> Result<(), JitError> {
        if (t as usize) < n {
            Ok(())
        } else {
            Err(JitError(format!(
                "jump target {t} out of range (code length {n})"
            )))
        }
    };

    // Two operands of the same scalar (non-`Handle`) class: the shape every
    // arithmetic/comparison opcode requires.
    let scalar_pair = |lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        check_reg(lhs)?;
        check_reg(rhs)?;
        if is_nonscalar(lhs) || is_nonscalar(rhs) {
            return Err(JitError(format!(
                "{op}: operand is a non-scalar (Handle/flat) register"
            )));
        }
        if class(lhs) != class(rhs) {
            return Err(JitError(format!(
                "{op}: operand classes differ ({:?} vs {:?})",
                class(lhs),
                class(rhs)
            )));
        }
        Ok(())
    };
    // Arithmetic: result register has the operands' class.
    let arith = |dst: u32, lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        scalar_pair(lhs, rhs, op)?;
        check_reg(dst)?;
        if class(dst) != class(lhs) {
            return Err(JitError(format!(
                "{op}: result {:?} does not match operands {:?}",
                class(dst),
                class(lhs)
            )));
        }
        Ok(())
    };
    // Integer-only ternary (Mod, bitwise, shift): every register must be `Int`.
    let int_op = |dst: u32, lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        check_reg(dst)?;
        check_reg(lhs)?;
        check_reg(rhs)?;
        for r in [dst, lhs, rhs] {
            if class(r) != JitValueType::Int {
                return Err(JitError(format!("{op}: register {r} must be Int")));
            }
        }
        Ok(())
    };
    // Comparison: operands share a scalar class, result is an `Int` boolean.
    let compare = |dst: u32, lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        scalar_pair(lhs, rhs, op)?;
        check_reg(dst)?;
        if class(dst) != JitValueType::Int {
            return Err(JitError(format!("{op}: boolean result must be Int")));
        }
        Ok(())
    };
    let require_class = |r: u32, want: JitValueType, op: &str| -> Result<(), JitError> {
        check_reg(r)?;
        if class(r) != want {
            return Err(JitError(format!(
                "{op}: register {r} is {:?}, expected {want:?}",
                class(r)
            )));
        }
        Ok(())
    };
    // A flat-array base must be the expected flat class *and* a parameter (flat
    // pointers only enter via the caller's args/lens, never produced internally).
    let require_flat_param = |r: u32, want: JitValueType, op: &str| -> Result<(), JitError> {
        require_class(r, want, op)?;
        if (r as usize) >= program.n_params as usize {
            return Err(JitError(format!("{op}: register {r} is not a parameter")));
        }
        Ok(())
    };

    for (i, instr) in program.code.iter().enumerate() {
        // Conditional branches fall through to `i + 1` (`build_function` indexes
        // `block_for[i + 1]`), so the instruction must not be the last one.
        let check_fallthrough = || -> Result<(), JitError> {
            if i + 1 < n {
                Ok(())
            } else {
                Err(JitError(format!(
                    "conditional branch at {i} has no fall-through instruction"
                )))
            }
        };
        match instr {
            JitInstr::Nop | JitInstr::Bail => {}
            JitInstr::LoadInt { dst, .. } => require_class(*dst, JitValueType::Int, "LoadInt")?,
            JitInstr::LoadFloat { dst, .. } => {
                require_class(*dst, JitValueType::Float, "LoadFloat")?
            }
            JitInstr::LoadBool { dst, .. } => require_class(*dst, JitValueType::Int, "LoadBool")?,
            JitInstr::Move { dst, src } => {
                check_reg(*dst)?;
                check_reg(*src)?;
                if is_nonscalar(*src) || is_nonscalar(*dst) {
                    return Err(JitError(
                        "Move: non-scalar (Handle/flat) registers cannot be moved".into(),
                    ));
                }
                if class(*dst) != class(*src) {
                    return Err(JitError(format!(
                        "Move: classes differ ({:?} vs {:?})",
                        class(*dst),
                        class(*src)
                    )));
                }
            }
            JitInstr::Add { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Add")?,
            JitInstr::Sub { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Sub")?,
            JitInstr::Mul { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Mul")?,
            JitInstr::Div { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Div")?,
            JitInstr::Mod { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "Mod")?,
            JitInstr::BitAnd { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "BitAnd")?,
            JitInstr::BitOr { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "BitOr")?,
            JitInstr::BitXor { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "BitXor")?,
            JitInstr::Shl { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "Shl")?,
            JitInstr::Shr { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "Shr")?,
            JitInstr::Compare { dst, lhs, rhs, .. } => compare(*dst, *lhs, *rhs, "Compare")?,
            JitInstr::Equal { dst, lhs, rhs } => compare(*dst, *lhs, *rhs, "Equal")?,
            JitInstr::NotEqual { dst, lhs, rhs } => compare(*dst, *lhs, *rhs, "NotEqual")?,
            JitInstr::Jump { target } => check_target(*target)?,
            JitInstr::JumpIfBool { cond, target, .. } => {
                require_class(*cond, JitValueType::Int, "JumpIfBool")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::JumpIfIntCompare {
                lhs, rhs, target, ..
            } => {
                scalar_pair(*lhs, *rhs, "JumpIfIntCompare")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::Return { src } => {
                check_reg(*src)?;
                if is_nonscalar(*src) {
                    return Err(JitError(
                        "Return: cannot return a non-scalar (Handle/flat) register".into(),
                    ));
                }
            }
            JitInstr::FieldInt { dst, base, .. } => {
                require_class(*base, JitValueType::Handle, "FieldInt base")?;
                require_class(*dst, JitValueType::Int, "FieldInt result")?;
            }
            JitInstr::ListLen { dst, base } => {
                require_class(*base, JitValueType::Handle, "ListLen base")?;
                require_class(*dst, JitValueType::Int, "ListLen result")?;
            }
            JitInstr::ListGetInt { dst, base, index } => {
                require_class(*base, JitValueType::Handle, "ListGetInt base")?;
                require_class(*index, JitValueType::Int, "ListGetInt index")?;
                require_class(*dst, JitValueType::Int, "ListGetInt result")?;
            }
            JitInstr::FieldFloat { dst, base, .. } => {
                require_class(*base, JitValueType::Handle, "FieldFloat base")?;
                require_class(*dst, JitValueType::Float, "FieldFloat result")?;
            }
            JitInstr::ListGetFloat { dst, base, index } => {
                require_class(*base, JitValueType::Handle, "ListGetFloat base")?;
                require_class(*index, JitValueType::Int, "ListGetFloat index")?;
                require_class(*dst, JitValueType::Float, "ListGetFloat result")?;
            }
            JitInstr::ListGetIntDirect { dst, base, index } => {
                require_flat_param(*base, JitValueType::FlatInt, "ListGetIntDirect base")?;
                require_class(*index, JitValueType::Int, "ListGetIntDirect index")?;
                require_class(*dst, JitValueType::Int, "ListGetIntDirect result")?;
            }
            JitInstr::ListGetFloatDirect { dst, base, index } => {
                require_flat_param(*base, JitValueType::FlatFloat, "ListGetFloatDirect base")?;
                require_class(*index, JitValueType::Int, "ListGetFloatDirect index")?;
                require_class(*dst, JitValueType::Float, "ListGetFloatDirect result")?;
            }
            JitInstr::ListLenDirect { dst, base } => {
                check_reg(*base)?;
                if !matches!(class(*base), JitValueType::FlatInt | JitValueType::FlatFloat) {
                    return Err(JitError(format!(
                        "ListLenDirect base: register {base} is {:?}, expected a flat-array param",
                        class(*base)
                    )));
                }
                if (*base as usize) >= program.n_params as usize {
                    return Err(JitError(format!(
                        "ListLenDirect base: register {base} is not a parameter"
                    )));
                }
                require_class(*dst, JitValueType::Int, "ListLenDirect result")?;
            }
            JitInstr::GuardClosureId { base, expected } => {
                require_class(*base, JitValueType::Handle, "GuardClosureId base")?;
                if *expected < 0 {
                    return Err(JitError(format!(
                        "GuardClosureId expected: {expected} is not a valid function id"
                    )));
                }
            }
            JitInstr::ClosureId { dst, base } => {
                require_class(*base, JitValueType::Handle, "ClosureId base")?;
                require_class(*dst, JitValueType::Int, "ClosureId result")?;
            }
            // OSR-exit is a parameterless terminator (an unconditional deopt at its
            // own ip); its live set is computed from definite-assignment, so it
            // carries no operands to validate.
            JitInstr::OsrExit => {}
        }
    }
    Ok(())
}

/// The register an instruction definitely writes (its `dst`), if any. Control
/// instructions (`Return`/`Jump`/`JumpIf*`/`Bail`) and `Nop` write nothing.
fn instr_def(instr: &JitInstr) -> Option<u32> {
    match instr {
        JitInstr::LoadInt { dst, .. }
        | JitInstr::LoadFloat { dst, .. }
        | JitInstr::LoadBool { dst, .. }
        | JitInstr::Move { dst, .. }
        | JitInstr::Add { dst, .. }
        | JitInstr::Sub { dst, .. }
        | JitInstr::Mul { dst, .. }
        | JitInstr::Div { dst, .. }
        | JitInstr::Mod { dst, .. }
        | JitInstr::BitAnd { dst, .. }
        | JitInstr::BitOr { dst, .. }
        | JitInstr::BitXor { dst, .. }
        | JitInstr::Shl { dst, .. }
        | JitInstr::Shr { dst, .. }
        | JitInstr::Compare { dst, .. }
        | JitInstr::Equal { dst, .. }
        | JitInstr::NotEqual { dst, .. }
        | JitInstr::FieldInt { dst, .. }
        | JitInstr::ListLen { dst, .. }
        | JitInstr::ListGetInt { dst, .. }
        | JitInstr::FieldFloat { dst, .. }
        | JitInstr::ListGetFloat { dst, .. }
        | JitInstr::ListGetIntDirect { dst, .. }
        | JitInstr::ListGetFloatDirect { dst, .. }
        | JitInstr::ListLenDirect { dst, .. }
        | JitInstr::ClosureId { dst, .. } => Some(*dst),
        JitInstr::Nop
        | JitInstr::Jump { .. }
        | JitInstr::JumpIfBool { .. }
        | JitInstr::JumpIfIntCompare { .. }
        | JitInstr::Return { .. }
        | JitInstr::GuardClosureId { .. }
        | JitInstr::OsrExit
        | JitInstr::Bail => None,
    }
}

/// The control-flow successors of instruction `i` (indices into `program.code`):
/// fallthrough to `i + 1` unless `i` is an unconditional `Jump`; conditional
/// branches add their target; `Jump` goes only to its target; `Return`/`Bail` (and
/// running off the end) go nowhere. Out-of-range targets are dropped — `validate`
/// rejects those before codegen, and the analysis stays total regardless.
fn successors(program: &JitFunction, i: usize) -> Vec<usize> {
    let n = program.code.len();
    let in_range = |t: u32| (t as usize) < n;
    let next = i + 1;
    match &program.code[i] {
        JitInstr::Jump { target } => {
            if in_range(*target) {
                vec![*target as usize]
            } else {
                vec![]
            }
        }
        JitInstr::JumpIfBool { target, .. } | JitInstr::JumpIfIntCompare { target, .. } => {
            let mut succ = Vec::new();
            if next < n {
                succ.push(next);
            }
            if in_range(*target) {
                succ.push(*target as usize);
            }
            succ
        }
        JitInstr::Return { .. } | JitInstr::Bail | JitInstr::OsrExit => vec![],
        _ => {
            if next < n {
                vec![next]
            } else {
                vec![]
            }
        }
    }
}

/// Definite-assignment ("must") analysis over the [`JitInstr`] CFG. Returns, per
/// instruction index `i`, the set (as a `reg -> bool` vector of width `n_regs`) of
/// registers **definitely assigned on entry to `i`** — i.e. assigned on *every*
/// path from the function entry to `i`.
///
/// Lattice: forward must-analysis. The entry-to-instruction-0 set is the parameter
/// registers `0..n_params`. `assigned_out[i] = assigned_in[i] ∪ defs(i)`, and for a
/// non-entry instruction `assigned_in[j] = ⋂ assigned_out[p]` over predecessors `p`
/// (intersection — a register is live on entry only if every incoming path assigns
/// it). Non-entry `assigned_in` starts at the full set and the intersection shrinks
/// it to the fixpoint; instruction 0's entry set is the params and is never
/// intersected down.
fn definite_assignment(program: &JitFunction) -> Vec<Vec<bool>> {
    let n = program.code.len();
    let n_regs = program.n_regs as usize;
    if n == 0 {
        return Vec::new();
    }

    // Predecessor lists, derived from the forward CFG.
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        for s in successors(program, i) {
            preds[s].push(i);
        }
    }

    // Entry set for instruction 0: the parameters are assigned, nothing else.
    let mut entry0 = vec![false; n_regs];
    for r in 0..(program.n_params as usize).min(n_regs) {
        entry0[r] = true;
    }

    // `assigned_in[0]` is pinned to the params; every other block starts at the
    // full (all-true) set so intersection can only shrink it toward the fixpoint.
    let mut assigned_in: Vec<Vec<bool>> = (0..n)
        .map(|i| {
            if i == 0 {
                entry0.clone()
            } else {
                vec![true; n_regs]
            }
        })
        .collect();

    let out_of = |in_set: &[bool], i: usize| -> Vec<bool> {
        let mut out = in_set.to_vec();
        if let Some(d) = instr_def(&program.code[i]) {
            if (d as usize) < n_regs {
                out[d as usize] = true;
            }
        }
        out
    };

    // Iterate to a fixpoint. Intersection is monotone (only clears bits), so the
    // loop terminates in at most `n_regs * n` bit-clears.
    let mut changed = true;
    while changed {
        changed = false;
        for j in 1..n {
            if preds[j].is_empty() {
                // Unreachable block: leave at the full set (it has no resume site of
                // its own that we rely on; its inputs are vacuously satisfied).
                continue;
            }
            let mut new_in = vec![true; n_regs];
            for &p in &preds[j] {
                let out = out_of(&assigned_in[p], p);
                for r in 0..n_regs {
                    new_in[r] = new_in[r] && out[r];
                }
            }
            if new_in != assigned_in[j] {
                assigned_in[j] = new_in;
                changed = true;
            }
        }
    }

    assigned_in
}

/// A sound integer interval `[lo, hi]` (inclusive) over `i128`, an
/// over-approximation of the set of `i64` values an Int register may hold. Held in
/// `i128` so the analysis arithmetic itself never overflows. `TOP` is the full
/// `i64` range — the safe default for any value we cannot track precisely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Interval {
    lo: i128,
    hi: i128,
}

impl Interval {
    const TOP: Interval = Interval {
        lo: i64::MIN as i128,
        hi: i64::MAX as i128,
    };

    fn constant(c: i64) -> Interval {
        Interval {
            lo: c as i128,
            hi: c as i128,
        }
    }

    /// Convex hull (union over-approximation) of two intervals.
    fn hull(self, other: Interval) -> Interval {
        Interval {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// True iff every value in `[lo, hi]` fits in an `i64`. When this holds for the
    /// *result* interval of an Int op, that op provably cannot overflow.
    fn fits_i64(self) -> bool {
        self.lo >= i64::MIN as i128 && self.hi <= i64::MAX as i128
    }
}

/// Forward integer-interval ("range") analysis over the Int registers of a native
/// function. Returns, per instruction index `i`, an `i128` interval per register
/// that **soundly over-approximates** every value the register may hold on entry to
/// `i`. Used solely to prove — conservatively, at compile time — that a particular
/// `Add`/`Sub`/`Mul` cannot overflow, so the checked `*_overflow` + bail can be
/// replaced by a plain `iadd`/`isub`/`imul` with byte-identical results.
///
/// Soundness is the whole point: every transfer function is an over-approximation,
/// and any register/operation we cannot prove a finite bound for is `TOP`
/// (`[i64::MIN, i64::MAX]`), which forces the checked path. Non-Int registers carry
/// `TOP` and are never consulted. The lattice has height 3 per register
/// (constant-derived range ⊑ wider range ⊑ TOP); a widening at every join jumps any
/// register whose merged interval grew straight to `TOP`, which both guarantees
/// termination (no infinite ascending chains across loop back-edges — an
/// unbounded-in-a-loop register widens to `TOP`, the safe answer) and keeps the
/// analysis cheap.
fn interval_analysis(program: &JitFunction) -> Vec<Vec<Interval>> {
    let n = program.code.len();
    let n_regs = program.n_regs as usize;
    if n == 0 {
        return Vec::new();
    }

    let is_int = |r: u32| program.reg_types[r as usize] == JitValueType::Int;
    // Read an operand's interval from an in-set: TOP for any non-Int register (we
    // only reason about Int arithmetic) and for out-of-range indices.
    let read = |set: &[Interval], r: u32| -> Interval {
        if is_int(r) {
            set.get(r as usize).copied().unwrap_or(Interval::TOP)
        } else {
            Interval::TOP
        }
    };

    // Predecessor lists from the forward CFG (same shape as definite_assignment).
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        for s in successors(program, i) {
            preds[s].push(i);
        }
    }

    // Transfer function: given the in-set at `i`, produce the out-set. Only the
    // proven cases narrow below TOP; everything else stays/becomes TOP.
    let out_of = |in_set: &[Interval], i: usize| -> Vec<Interval> {
        let mut out = in_set.to_vec();
        let set = |out: &mut Vec<Interval>, r: u32, v: Interval| {
            if (r as usize) < n_regs {
                // Collapse any range that doesn't fit i64 straight to TOP. Such a
                // range can never prove an op safe anyway, and clamping here keeps the
                // analysis's own i128 arithmetic bounded (a register always holds an
                // i64-fitting range or TOP, so downstream corner products stay well
                // within i128). Also never claim a tighter bound than the register's
                // storage class allows.
                out[r as usize] = if is_int(r) && v.fits_i64() {
                    v
                } else {
                    Interval::TOP
                };
            }
        };
        match &program.code[i] {
            JitInstr::LoadInt { dst, value } => set(&mut out, *dst, Interval::constant(*value)),
            JitInstr::LoadBool { dst, value } => {
                set(&mut out, *dst, Interval::constant(i64::from(*value)));
            }
            JitInstr::Move { dst, src } => {
                let v = read(in_set, *src);
                set(&mut out, *dst, v);
            }
            // A list/array length is a non-negative element count; we soundly bound
            // it to `[0, i64::MAX]` and deliberately assume NO tighter upper bound.
            JitInstr::ListLen { dst, .. } | JitInstr::ListLenDirect { dst, .. } => {
                set(&mut out, *dst, Interval { lo: 0, hi: i64::MAX as i128 });
            }
            JitInstr::Add { dst, lhs, rhs } if is_int(*lhs) => {
                let a = read(in_set, *lhs);
                let b = read(in_set, *rhs);
                set(&mut out, *dst, Interval { lo: a.lo + b.lo, hi: a.hi + b.hi });
            }
            JitInstr::Sub { dst, lhs, rhs } if is_int(*lhs) => {
                let a = read(in_set, *lhs);
                let b = read(in_set, *rhs);
                set(&mut out, *dst, Interval { lo: a.lo - b.hi, hi: a.hi - b.lo });
            }
            JitInstr::Mul { dst, lhs, rhs } if is_int(*lhs) => {
                let a = read(in_set, *lhs);
                let b = read(in_set, *rhs);
                // Product range is the hull of the four corner products (i128, so
                // the proof arithmetic cannot itself overflow for i64 operands).
                let c1 = a.lo * b.lo;
                let c2 = a.lo * b.hi;
                let c3 = a.hi * b.lo;
                let c4 = a.hi * b.hi;
                let lo = c1.min(c2).min(c3).min(c4);
                let hi = c1.max(c2).max(c3).max(c4);
                set(&mut out, *dst, Interval { lo, hi });
            }
            // Every other definer produces an untracked value ⇒ TOP. (Covers Div,
            // Mod, bitops, shifts, compares, heap reads, ClosureId, params, etc.)
            other => {
                if let Some(d) = instr_def(other) {
                    set(&mut out, d, Interval::TOP);
                }
            }
        }
        out
    };

    // Entry to instruction 0: params are untracked (TOP); everything else TOP too.
    // Every per-instruction register starts at TOP and the analysis narrows it only
    // where a definer/merge proves a bound. `initialized[j]` tracks whether block
    // `j`'s in-set has been computed from predecessors at least once (the first such
    // pass *replaces* the all-TOP seed rather than hulling against it).
    let mut interval_in: Vec<Vec<Interval>> =
        (0..n).map(|_| vec![Interval::TOP; n_regs]).collect();
    let mut initialized = vec![false; n];
    // Sticky-widened marker per (instruction, register): once a register at block
    // `j` widens to TOP (e.g. across a loop back-edge), it is pinned to TOP and
    // never narrowed again. This makes each register monotone — narrowed at most
    // once, then only ever widened to the sticky TOP — which guarantees the fixpoint
    // terminates (no narrow/widen oscillation) and keeps any unbounded-in-a-loop
    // register at the safe TOP. Lattice height per register ≤ 3.
    let mut pinned_top: Vec<Vec<bool>> = (0..n).map(|_| vec![false; n_regs]).collect();

    let mut changed = true;
    while changed {
        changed = false;
        for j in 0..n {
            // Instruction 0 and any predecessor-less block keep the all-TOP entry,
            // which is sound (params and unreachable-block inputs are untracked).
            if preds[j].is_empty() {
                continue;
            }
            let mut new_in: Vec<Interval> = vec![
                Interval { lo: i128::MAX, hi: i128::MIN };
                n_regs
            ];
            for &p in &preds[j] {
                let out = out_of(&interval_in[p], p);
                for r in 0..n_regs {
                    new_in[r] = new_in[r].hull(out[r]);
                }
            }
            let first = !initialized[j];
            for r in 0..n_regs {
                if pinned_top[j][r] {
                    new_in[r] = Interval::TOP;
                    continue;
                }
                let merged = new_in[r];
                if first {
                    // First real computation: adopt the predecessors' hull directly,
                    // narrowing off the TOP seed.
                    new_in[r] = merged;
                    continue;
                }
                let old = interval_in[j][r];
                // Subsequent passes: if the merged interval grew strictly wider than
                // the current value, pin to TOP; otherwise keep the (non-growing)
                // merged interval.
                if merged.lo < old.lo || merged.hi > old.hi {
                    new_in[r] = Interval::TOP;
                    pinned_top[j][r] = true;
                } else {
                    new_in[r] = merged;
                }
            }
            if new_in != interval_in[j] || first {
                interval_in[j] = new_in;
                initialized[j] = true;
                changed = true;
            }
        }
    }

    interval_in
}

/// Whether the result of the `Add`/`Sub`/`Mul` at instruction `i` provably fits in
/// `i64` given the operand intervals on entry — i.e. the op cannot overflow, so the
/// checked `*_overflow` + bail may be replaced by a plain unchecked op with
/// identical results. Conservative: any TOP operand (or a non-Int op) makes the
/// result range TOP, which does NOT fit, so the checked path is kept. The result
/// interval is computed in `i128` exactly as the analysis transfer function does.
fn arith_cannot_overflow(intervals: &[Interval], instr: &JitInstr) -> bool {
    let get = |r: u32| intervals.get(r as usize).copied().unwrap_or(Interval::TOP);
    let result = match instr {
        JitInstr::Add { lhs, rhs, .. } => {
            let a = get(*lhs);
            let b = get(*rhs);
            Interval { lo: a.lo + b.lo, hi: a.hi + b.hi }
        }
        JitInstr::Sub { lhs, rhs, .. } => {
            let a = get(*lhs);
            let b = get(*rhs);
            Interval { lo: a.lo - b.hi, hi: a.hi - b.lo }
        }
        JitInstr::Mul { lhs, rhs, .. } => {
            let a = get(*lhs);
            let b = get(*rhs);
            let c1 = a.lo * b.lo;
            let c2 = a.lo * b.hi;
            let c3 = a.hi * b.lo;
            let c4 = a.hi * b.hi;
            Interval {
                lo: c1.min(c2).min(c3).min(c4),
                hi: c1.max(c2).max(c3).max(c4),
            }
        }
        _ => return false,
    };
    result.fits_i64()
}

#[allow(clippy::too_many_arguments)]
fn build_function(
    func: &mut cranelift_codegen::ir::Function,
    fbctx: &mut FunctionBuilderContext,
    module: &mut JITModule,
    imports: HostFuncs,
    program: &JitFunction,
    forced: Option<u32>,
    osr_header: Option<u32>,
) -> Result<DeoptMap, JitError> {
    // Definite-assignment ("must") sets per instruction, computed once up front so
    // each bail site can record its live (entry-assigned) registers. Purely
    // host-side analysis — it shapes no emitted code.
    let assigned_in = definite_assignment(program);
    // Forward integer-interval analysis (J4.3): per-instruction sound ranges for
    // Int registers, used purely to elide provably-non-overflowing checks. Like
    // `definite_assignment`, host-side only — it shapes no code beyond choosing the
    // checked vs unchecked arithmetic form.
    let intervals = interval_analysis(program);
    // Sites accumulate in emission order, aligned 1:1 with the `next_id` counter
    // (`sites[id - 1]` is the site for id `id`).
    let mut sites: Vec<DeoptSite> = Vec::new();

    let mut bcx = FunctionBuilder::new(func, fbctx);

    // Per-function references to the imported host helpers (heap reads call these).
    let field_int_ref = module.declare_func_in_func(imports.field_int, bcx.func);
    let list_len_ref = module.declare_func_in_func(imports.list_len, bcx.func);
    let list_get_int_ref = module.declare_func_in_func(imports.list_get_int, bcx.func);
    let field_float_ref = module.declare_func_in_func(imports.field_float, bcx.func);
    let list_get_float_ref = module.declare_func_in_func(imports.list_get_float, bcx.func);
    let closure_id_ref = module.declare_func_in_func(imports.closure_id, bcx.func);

    let n = program.code.len();
    let n_regs = program.n_regs as usize;

    // One Cranelift variable per VM register, typed by storage class (i64 for
    // integers/booleans, f64 for floats).
    let var_ty = |reg: usize| {
        if program.reg_types[reg] == JitValueType::Float {
            types::F64
        } else {
            types::I64
        }
    };
    let vars: Vec<Variable> = (0..n_regs).map(|i| bcx.declare_var(var_ty(i))).collect();

    // Entry: read params from the args array, zero the rest, then jump to the
    // block for instruction 0. Args are passed as raw 64-bit words; loading a
    // float register's slot as `f64` reinterprets the caller's `f64::to_bits`.
    let entry = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    let params = bcx.block_params(entry).to_vec();
    let args_ptr = params[0];
    let lens_ptr = params[2];
    let out_ptr = params[3];
    let bail_ptr = params[4];
    let safepoint_ptr = params[5];
    let payload_ptr = params[6];
    // Running per-site bail-id counter. Starts at 1 (0 is reserved = no bail);
    // `bail_if` post-increments it so every guard/bail site gets a stable id.
    let mut next_id: i64 = 1;
    // The set of registers to load from the entry window. For a normal compile this
    // is the parameter registers (`0..n_params`, loaded by index from `args_ptr`).
    // For an OSR-entry it is the loop's *live-in* set: the registers definitely
    // assigned on entry to `header_ip`, loaded by register index from the window
    // (`args_ptr` is the interpreter's `n_regs`-wide register window, not a packed
    // arg array). Every other register is zero-initialized so the var is defined on
    // every path (SSA), exactly as on the normal entry.
    let load_set: Vec<usize> = match osr_header {
        Some(header) => {
            let header = header as usize;
            match assigned_in.get(header) {
                Some(set) => set
                    .iter()
                    .enumerate()
                    .filter(|&(_, &assigned)| assigned)
                    .map(|(r, _)| r)
                    .collect(),
                None => Vec::new(),
            }
        }
        None => (0..program.n_params as usize).collect(),
    };
    let mut is_loaded = vec![false; n_regs];
    for &r in &load_set {
        is_loaded[r] = true;
        let v = bcx
            .ins()
            .load(var_ty(r), MemFlags::trusted(), args_ptr, (r as i32) * 8);
        bcx.def_var(vars[r], v);
    }
    let zero_i = bcx.ins().iconst(types::I64, 0);
    let zero_f = bcx.ins().f64const(0.0);
    for (i, &var) in vars.iter().enumerate().take(n_regs) {
        if is_loaded[i] {
            continue;
        }
        bcx.def_var(
            var,
            if var_ty(i) == types::F64 {
                zero_f
            } else {
                zero_i
            },
        );
    }

    // The shared fallback block: "not completed".
    let fallback = bcx.create_block();

    // Block leaders: index 0, every jump target, and the instruction after any
    // control-transfer (so dead/own-block code never lands in a sealed block).
    let mut is_leader = vec![false; n];
    if n > 0 {
        is_leader[0] = true;
    }
    for (i, instr) in program.code.iter().enumerate() {
        match instr {
            JitInstr::Jump { target } => {
                is_leader[*target as usize] = true;
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
            }
            JitInstr::JumpIfBool { target, .. } | JitInstr::JumpIfIntCompare { target, .. } => {
                is_leader[*target as usize] = true;
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
            }
            JitInstr::Return { .. } | JitInstr::Bail | JitInstr::OsrExit if i + 1 < n => {
                is_leader[i + 1] = true;
            }
            _ => {}
        }
    }
    let block_for: Vec<Option<Block>> = (0..n)
        .map(|i| {
            if is_leader[i] {
                Some(bcx.create_block())
            } else {
                None
            }
        })
        .collect();

    let reg = |program_reg: u32| vars[program_reg as usize];

    // Fresh per-call deopt context for the instruction currently being lowered
    // (`i`). Each `bail_if` consumes one and pushes one site, so ids and `sites`
    // indices stay in lock-step. A macro (not a closure) so the `&mut sites` borrow
    // lives only for the single `bail_if` call.
    macro_rules! deopt {
        ($ip:expr) => {
            &mut DeoptCtx {
                ip: $ip as u32,
                assigned_in: &assigned_in,
                reg_types: &program.reg_types,
                sites: &mut sites,
                forced,
                unconditional: false,
            }
        };
        ($ip:expr, unconditional) => {
            &mut DeoptCtx {
                ip: $ip as u32,
                assigned_in: &assigned_in,
                reg_types: &program.reg_types,
                sites: &mut sites,
                forced,
                unconditional: true,
            }
        };
    }

    match osr_header {
        // OSR-entry: begin native execution *inside* the loop at the header block.
        // The header is a backedge target, hence a leader, so its block exists; if a
        // caller passes a non-leader (or out-of-range) header we reject cleanly.
        Some(header) => {
            let header = header as usize;
            let target = block_for.get(header).copied().flatten().ok_or_else(|| {
                JitError(format!(
                    "OSR header ip {header} is not a leader / jump-target block"
                ))
            })?;
            bcx.ins().jump(target, &[]);
        }
        None if n == 0 => {
            bcx.ins().jump(fallback, &[]);
        }
        None => {
            bcx.ins().jump(block_for[0].unwrap(), &[]);
        }
    }

    let mut terminated = true;
    for i in 0..n {
        if let Some(b) = block_for[i] {
            if !terminated {
                bcx.ins().jump(b, &[]);
            }
            bcx.switch_to_block(b);
            terminated = false;
        }
        match &program.code[i] {
            JitInstr::Nop => {}
            JitInstr::LoadInt { dst, value } => {
                let v = bcx.ins().iconst(types::I64, *value);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::LoadFloat { dst, value } => {
                let v = bcx.ins().f64const(*value);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::LoadBool { dst, value } => {
                let v = bcx.ins().iconst(types::I64, i64::from(*value));
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::Move { dst, src } => {
                let v = bcx.use_var(reg(*src));
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::Add { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                if program.is_float(*lhs) {
                    let res = bcx.ins().fadd(a, b);
                    bcx.def_var(reg(*dst), res);
                } else if arith_cannot_overflow(&intervals[i], &program.code[i]) {
                    // Proven non-overflowing by range analysis ⇒ plain unchecked
                    // add, no overflow flag, no bail. Result is byte-identical to the
                    // checked form (which only differs by trapping on overflow).
                    let res = bcx.ins().iadd(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let (res, of) = bcx.ins().sadd_overflow(a, b);
                    let cont =
                        bail_if(
                        &mut bcx,
                        of,
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Sub { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                if program.is_float(*lhs) {
                    let res = bcx.ins().fsub(a, b);
                    bcx.def_var(reg(*dst), res);
                } else if arith_cannot_overflow(&intervals[i], &program.code[i]) {
                    let res = bcx.ins().isub(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let (res, of) = bcx.ins().ssub_overflow(a, b);
                    let cont =
                        bail_if(
                        &mut bcx,
                        of,
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Mul { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                if program.is_float(*lhs) {
                    let res = bcx.ins().fmul(a, b);
                    bcx.def_var(reg(*dst), res);
                } else if arith_cannot_overflow(&intervals[i], &program.code[i]) {
                    let res = bcx.ins().imul(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let (res, of) = bcx.ins().smul_overflow(a, b);
                    let cont =
                        bail_if(
                        &mut bcx,
                        of,
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Div { dst, lhs, rhs } => {
                if program.is_float(*lhs) {
                    // Float division never traps (x/0.0 = ±inf/NaN), matching the
                    // interpreter, so no bail.
                    let a = bcx.use_var(reg(*lhs));
                    let b = bcx.use_var(reg(*rhs));
                    let res = bcx.ins().fdiv(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let res = emit_checked_divrem(
                        &mut bcx,
                        reg(*lhs),
                        reg(*rhs),
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                        &vars,
                        &mut next_id,
                        deopt!(i),
                        false,
                    );
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Mod { dst, lhs, rhs } => {
                // Float modulo is a runtime error in the VM, so only integer
                // registers reach here (eligibility rejects float `%`).
                let res = emit_checked_divrem(
                    &mut bcx,
                    reg(*lhs),
                    reg(*rhs),
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    true,
                );
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::BitAnd { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let v = bcx.ins().band(a, b);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::BitOr { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let v = bcx.ins().bor(a, b);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::BitXor { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let v = bcx.ins().bxor(a, b);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::Shl { dst, lhs, rhs } => {
                let res = emit_checked_shift(
                    &mut bcx,
                    reg(*lhs),
                    reg(*rhs),
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    false,
                );
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Shr { dst, lhs, rhs } => {
                let res = emit_checked_shift(
                    &mut bcx,
                    reg(*lhs),
                    reg(*rhs),
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    true,
                );
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Compare { dst, op, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(op.fcc(), a, b)
                } else {
                    bcx.ins().icmp(op.cc(), a, b)
                };
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::Equal { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(FloatCC::Equal, a, b)
                } else {
                    bcx.ins().icmp(IntCC::Equal, a, b)
                };
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::NotEqual { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(FloatCC::NotEqual, a, b)
                } else {
                    bcx.ins().icmp(IntCC::NotEqual, a, b)
                };
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::Jump { target } => {
                bcx.ins().jump(block_for[*target as usize].unwrap(), &[]);
                terminated = true;
            }
            JitInstr::JumpIfBool {
                cond,
                expected,
                target,
            } => {
                let c = bcx.use_var(reg(*cond));
                let tb = block_for[*target as usize].unwrap();
                let fb = block_for[i + 1].unwrap();
                if *expected {
                    bcx.ins().brif(c, tb, &[], fb, &[]);
                } else {
                    bcx.ins().brif(c, fb, &[], tb, &[]);
                }
                terminated = true;
            }
            JitInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(op.fcc(), a, b)
                } else {
                    bcx.ins().icmp(op.cc(), a, b)
                };
                let tb = block_for[*target as usize].unwrap();
                let fb = block_for[i + 1].unwrap();
                if *expected {
                    bcx.ins().brif(c, tb, &[], fb, &[]);
                } else {
                    bcx.ins().brif(c, fb, &[], tb, &[]);
                }
                terminated = true;
            }
            JitInstr::Return { src } => {
                let v = bcx.use_var(reg(*src));
                bcx.ins().store(MemFlags::trusted(), v, out_ptr, 0);
                let one = bcx.ins().iconst(types::I8, 1);
                bcx.ins().return_(&[one]);
                terminated = true;
            }
            JitInstr::Bail => {
                bcx.ins().jump(fallback, &[]);
                terminated = true;
            }
            JitInstr::OsrExit => {
                // OSR-exit (J5.2): the loop has exited. Deopt *unconditionally* at
                // this ip, capturing the live-out window so the host resumes the
                // interpreter here (precise-deopt). Reuse `bail_if` in its
                // unconditional mode: it mints a stable safepoint id, records the
                // `DeoptSite` (resume_ip = this ip, live = entry-assigned regs), and
                // emits the id-store + live-capture on the (unconditionally taken)
                // bail edge — the exact same machinery a guard bail uses.
                let always = bcx.ins().iconst(types::I8, 1);
                let cont = bail_if(
                    &mut bcx,
                    always,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i, unconditional),
                );
                // `bail_if` switched us to the (now unreachable, since the bail is
                // unconditional) `cont` block; terminate it so it stays well-formed
                // for `seal_all_blocks` (Cranelift DCE drops the dead block).
                bcx.switch_to_block(cont);
                bcx.ins().jump(fallback, &[]);
                terminated = true;
            }
            JitInstr::FieldInt { dst, base, slot } => {
                let handle = bcx.use_var(reg(*base));
                let slot_v = bcx.ins().iconst(types::I64, i64::from(*slot));
                let call = bcx.ins().call(field_int_ref, &[handle, slot_v]);
                let result = bcx.inst_results(call)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListLen { dst, base } => {
                let handle = bcx.use_var(reg(*base));
                let call = bcx.ins().call(list_len_ref, &[handle]);
                let result = bcx.inst_results(call)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListGetInt { dst, base, index } => {
                let handle = bcx.use_var(reg(*base));
                let index_v = bcx.use_var(reg(*index));
                let call = bcx.ins().call(list_get_int_ref, &[handle, index_v]);
                let result = bcx.inst_results(call)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::FieldFloat { dst, base, slot } => {
                let handle = bcx.use_var(reg(*base));
                let slot_v = bcx.ins().iconst(types::I64, i64::from(*slot));
                let call = bcx.ins().call(field_float_ref, &[handle, slot_v]);
                let result = bcx.inst_results(call)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListGetFloat { dst, base, index } => {
                let handle = bcx.use_var(reg(*base));
                let index_v = bcx.use_var(reg(*index));
                let call = bcx.ins().call(list_get_float_ref, &[handle, index_v]);
                let result = bcx.inst_results(call)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListGetIntDirect { dst, base, index } => {
                let result = emit_direct_get(
                    &mut bcx,
                    lens_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    reg(*base),
                    reg(*index),
                    *base,
                    types::I64,
                );
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListGetFloatDirect { dst, base, index } => {
                let result = emit_direct_get(
                    &mut bcx,
                    lens_ptr,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    reg(*base),
                    reg(*index),
                    *base,
                    types::F64,
                );
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListLenDirect { dst, base } => {
                // Length lives in the `lens` slot for the base param (param index ==
                // register index for flat params). No host call, no bail.
                let len = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    lens_ptr,
                    (*base as i32) * 8,
                );
                bcx.def_var(reg(*dst), len);
            }
            JitInstr::GuardClosureId { base, expected } => {
                // Read the closure handle's underlying function id and bail to the
                // interpreter if it isn't the speculated callee `expected`. The
                // helper is total (returns -1 on a non-closure handle), so the
                // compare alone decides; the bail reuses the standard re-run-from-top
                // fallback, sound because the inlined subset is side-effect-free.
                let handle = bcx.use_var(reg(*base));
                let call = bcx.ins().call(closure_id_ref, &[handle]);
                let id = bcx.inst_results(call)[0];
                let want = bcx.ins().iconst(types::I64, *expected);
                let mismatch = bcx.ins().icmp(IntCC::NotEqual, id, want);
                let cont = bail_if(
                    &mut bcx,
                    mismatch,
                    fallback,
                    safepoint_ptr,
                    payload_ptr,
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
            }
            JitInstr::ClosureId { dst, base } => {
                // Polymorphic inline cache (J2.2): read the closure handle's
                // underlying function id once into `dst` (a single, total helper
                // call — `-1` for a non-closure). The dispatcher then selects an
                // inlined callee with ordinary integer compares/branches; a
                // no-match arm bails via the standard re-run-from-top fallback.
                let handle = bcx.use_var(reg(*base));
                let call = bcx.ins().call(closure_id_ref, &[handle]);
                let id = bcx.inst_results(call)[0];
                bcx.def_var(reg(*dst), id);
            }
        }
    }
    if !terminated {
        // Fell off the end without an explicit return: behave like the VM's
        // defensive `Unit` return by bailing (this path is unreachable for
        // well-formed bytecode, which always ends in `Return`).
        bcx.ins().jump(fallback, &[]);
    }

    // Fallback block body: not completed.
    bcx.switch_to_block(fallback);
    let zero8 = bcx.ins().iconst(types::I8, 0);
    bcx.ins().return_(&[zero8]);

    bcx.seal_all_blocks();
    bcx.finalize();

    Ok(DeoptMap { sites })
}

/// TV2 direct list read: `cont: dst = base_ptr[index]`, bounds-checked against the
/// param's `lens` slot. `base_var` holds the raw data pointer (i64); `base_param`
/// is its param/register index (used to index `lens`); `elem_ty` is `I64` for an
/// `Ints` list or `F64` for a `Floats` list. An index `< 0` or `>= len` branches to
/// `fallback` (→ the VM re-runs on the interpreter, matching the helper's OOB bail).
///
/// SAFETY (codegen contract): the generated load reads exactly one `elem_ty` at
/// `base_ptr + index * 8`, only after proving `0 <= index < len`. `base_ptr` and
/// `len` come from the caller's `args`/`lens` for the same param, which the
/// `NativeModule::call` borrow protocol guarantees point at a live, immovable,
/// unmutated buffer of `len` elements for the call's duration. So every in-bounds
/// element address is valid and the read cannot alias a concurrent mutation.
fn emit_direct_get(
    bcx: &mut FunctionBuilder,
    lens_ptr: Value,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
    base_var: Variable,
    index_var: Variable,
    base_param: u32,
    elem_ty: types::Type,
) -> Value {
    let index = bcx.use_var(index_var);
    let len = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), lens_ptr, (base_param as i32) * 8);
    // Single unsigned compare folds "index < 0" (huge unsigned) and "index >= len"
    // into one OOB test (len is a non-negative element count).
    let oob = bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
    let cont = bail_if(bcx, oob, fallback, safepoint_ptr, payload_ptr, vars, next_id, deopt);
    bcx.switch_to_block(cont);
    let base_ptr = bcx.use_var(base_var);
    let offset = bcx.ins().imul_imm(index, 8);
    let addr = bcx.ins().iadd(base_ptr, offset);
    bcx.ins().load(elem_ty, MemFlags::trusted(), addr, 0)
}

/// Host-side deopt bookkeeping threaded through every guard emission. Each
/// [`bail_if`] call records one [`DeoptSite`] into `sites` for the instruction
/// currently being emitted (`ip`), keeping `sites` aligned 1:1 with the minted
/// safepoint ids. Carries no Cranelift state — it shapes no machine code.
struct DeoptCtx<'a> {
    /// Index of the instruction currently being lowered (the resume_ip for any
    /// guard it emits).
    ip: u32,
    /// Definite-assignment sets per instruction (see [`definite_assignment`]).
    assigned_in: &'a [Vec<bool>],
    /// Storage class per register, to type each live register.
    reg_types: &'a [JitValueType],
    /// Accumulated sites, in emission (= id) order.
    sites: &'a mut Vec<DeoptSite>,
    /// Test/diagnostic hook: when `Some(id)`, the site minted with that id bails
    /// unconditionally (see [`NativeModule::compile_forcing_bail`]). `None` on the
    /// default [`compile`](NativeModule::compile) path, where every site is guarded.
    forced: Option<u32>,
    /// OSR-exit (J5.2): when set, the site about to be minted bails unconditionally
    /// (the loop-exit edge always deopts). Independent of `forced` (which is keyed
    /// by a specific id); this applies to whatever id this single `bail_if` mints.
    unconditional: bool,
}

impl DeoptCtx<'_> {
    /// Record the site for the safepoint about to be minted: resume at the current
    /// instruction with its entry-assigned (definitely-live) registers. Returns the
    /// same `live` set so the caller can emit the matching payload-capture stores.
    fn record(&mut self) -> Vec<(u32, JitValueType)> {
        let live: Vec<(u32, JitValueType)> = match self.assigned_in.get(self.ip as usize) {
            Some(set) => set
                .iter()
                .enumerate()
                .filter(|&(_, &assigned)| assigned)
                .map(|(r, _)| (r as u32, self.reg_types[r]))
                .collect(),
            None => Vec::new(),
        };
        self.sites.push(DeoptSite {
            resume_ip: self.ip,
            live: live.clone(),
        });
        live
    }
}

/// Emit a per-site guarded bail and return the `cont` block to continue in.
///
/// `safepoint_ptr` is the host's safepoint-id cell; `next_id` is the running
/// site-id counter (post-incremented to mint this site's stable id, starting from
/// 1; `0` stays reserved). On the bail edge — and *only* there — a dedicated cold
/// `site_block` stores this site's id into `safepoint_ptr` before jumping to the
/// shared `fallback`. The hot fall-through (`cont`) path executes zero extra
/// instructions, so non-bailing iterations are unaffected.
///
/// `deopt` records this site's [`DeoptSite`] (resume_ip + live regs) host-side,
/// pushed in lock-step with `next_id` so `sites[id - 1]` aligns with the id minted
/// here. This recording emits no machine code.
///
/// On the cold edge — and only there — each live register's current value is also
/// *stored* into `payload_ptr[reg]` (`vars[reg]` is its Cranelift variable; an f64
/// var stores its 8-byte bit pattern into the slot). The hot `cont` path emits no
/// capture store, so non-bailing iterations are unaffected.
fn bail_if(
    bcx: &mut FunctionBuilder,
    cond: Value,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Block {
    let site_id = *next_id;
    *next_id += 1;
    let forced = deopt.forced == Some(site_id as u32) || deopt.unconditional;
    let live = deopt.record();
    let site_block = bcx.create_block();
    let cont = bcx.create_block();
    if forced {
        // Forced-bail diagnostic path: deopt at this site unconditionally, ignoring
        // `cond`. `cont` is still created/sealed (and emitted into) below — it just
        // becomes unreachable here, which Cranelift's DCE drops. The site_block body
        // is unchanged, so the forced bail captures the same live set a natural bail
        // would. The default `compile` path never sets `forced`, so it always takes
        // the `brif` branch and emits byte-identical guards.
        bcx.ins().jump(site_block, &[]);
    } else {
        bcx.ins().brif(cond, site_block, &[], cont, &[]);
    }
    // Cold path: record this site's id, capture each live register's value into the
    // payload buffer, then fall through to the shared fallback. None of this is
    // emitted on the hot `cont` edge below.
    bcx.switch_to_block(site_block);
    let id_v = bcx.ins().iconst(types::I64, site_id);
    bcx.ins().store(MemFlags::trusted(), id_v, safepoint_ptr, 0);
    for &(reg, _) in &live {
        let v = bcx.use_var(vars[reg as usize]);
        bcx.ins()
            .store(MemFlags::trusted(), v, payload_ptr, (reg as i32) * 8);
    }
    bcx.ins().jump(fallback, &[]);
    bcx.switch_to_block(cont);
    cont
}

/// Load the host-helper bail flag and branch to `fallback` if a preceding heap
/// read flagged failure — checked immediately after each helper call so a bad
/// read never keeps executing. Returns the continuation block.
fn bail_if_helper_failed(
    bcx: &mut FunctionBuilder,
    bail_ptr: Value,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Block {
    let flag = bcx.ins().load(types::I8, MemFlags::trusted(), bail_ptr, 0);
    bail_if(bcx, flag, fallback, safepoint_ptr, payload_ptr, vars, next_id, deopt)
}

/// Checked division / remainder matching the interpreter: bail on divide-by-zero
/// and on `i64::MIN / -1` (the only signed-division overflow).
fn emit_checked_divrem(
    bcx: &mut FunctionBuilder,
    lhs: Variable,
    rhs: Variable,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
    is_rem: bool,
) -> Value {
    let a = bcx.use_var(lhs);
    let b = bcx.use_var(rhs);
    let zero = bcx.ins().iconst(types::I64, 0);
    let is_zero = bcx.ins().icmp(IntCC::Equal, b, zero);
    let cont1 = bail_if(bcx, is_zero, fallback, safepoint_ptr, payload_ptr, vars, next_id, deopt);
    bcx.switch_to_block(cont1);
    let imin = bcx.ins().iconst(types::I64, i64::MIN);
    let neg1 = bcx.ins().iconst(types::I64, -1);
    let a_is_min = bcx.ins().icmp(IntCC::Equal, a, imin);
    let b_is_neg1 = bcx.ins().icmp(IntCC::Equal, b, neg1);
    let overflow = bcx.ins().band(a_is_min, b_is_neg1);
    let cont2 = bail_if(bcx, overflow, fallback, safepoint_ptr, payload_ptr, vars, next_id, deopt);
    bcx.switch_to_block(cont2);
    if is_rem {
        bcx.ins().srem(a, b)
    } else {
        bcx.ins().sdiv(a, b)
    }
}

/// Checked shift: bail when the shift amount is negative or `>= 64` (so the
/// in-range case matches `wrapping_shl`/`wrapping_shr` exactly).
fn emit_checked_shift(
    bcx: &mut FunctionBuilder,
    lhs: Variable,
    rhs: Variable,
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
    is_right: bool,
) -> Value {
    let a = bcx.use_var(lhs);
    let amt = bcx.use_var(rhs);
    let limit = bcx.ins().iconst(types::I64, 64);
    // Unsigned compare folds "negative" (huge unsigned) and ">= 64" into one test.
    let oob = bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, amt, limit);
    let cont = bail_if(bcx, oob, fallback, safepoint_ptr, payload_ptr, vars, next_id, deopt);
    bcx.switch_to_block(cont);
    if is_right {
        bcx.ins().sshr(a, amt)
    } else {
        bcx.ins().ishl(a, amt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-only convenience over [`NativeModule::call`]: pass a zeroed `lens`
    /// (length-matched to `args`) for tests that use no flat-array params.
    trait CallScalar {
        fn callt(&self, id: CompiledId, args: &[i64]) -> Option<i64>;
    }
    impl CallScalar for NativeModule {
        fn callt(&self, id: CompiledId, args: &[i64]) -> Option<i64> {
            let lens = vec![0i64; args.len()];
            self.call(id, args, &lens).completed()
        }
    }

    extern "C" fn noop_field_int(_handle: i64, _slot: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_len(_handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_get_int(_handle: i64, _index: i64) -> i64 {
        0
    }
    extern "C" fn noop_field_float(_handle: i64, _slot: i64) -> f64 {
        0.0
    }
    extern "C" fn noop_list_get_float(_handle: i64, _index: i64) -> f64 {
        0.0
    }
    extern "C" fn noop_closure_id(_handle: i64) -> i64 {
        0
    }

    /// A module with no-op host helpers (these tests exercise only scalar ops).
    fn module() -> NativeModule {
        NativeModule::new(HostHelpers {
            field_int: noop_field_int,
            list_len: noop_list_len,
            list_get_int: noop_list_get_int,
            field_float: noop_field_float,
            list_get_float: noop_list_get_float,
            closure_id: noop_closure_id,
        })
        .unwrap()
    }

    fn f(n_params: u32, n_regs: u32, code: Vec<JitInstr>) -> JitFunction {
        JitFunction {
            n_params,
            n_regs,
            reg_types: vec![JitValueType::Int; n_regs as usize],
            code,
        }
    }

    /// Like `f` but with explicit per-register storage classes (for float tests).
    fn ft(n_params: u32, reg_types: Vec<JitValueType>, code: Vec<JitInstr>) -> JitFunction {
        JitFunction {
            n_params,
            n_regs: reg_types.len() as u32,
            reg_types,
            code,
        }
    }

    #[test]
    fn compiles_and_runs_float_arith() {
        use JitValueType::{Float, Int};
        let mut m = module();
        // fn(a: f64, b: f64) -> f64 { return a * b - a }  regs 0=a,1=b,2=t
        let id = m
            .compile(&ft(
                2,
                vec![Float, Float, Float],
                vec![
                    JitInstr::Mul {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Sub {
                        dst: 2,
                        lhs: 2,
                        rhs: 0,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let call = |a: f64, b: f64| {
            f64::from_bits(
                m.callt(id, &[a.to_bits() as i64, b.to_bits() as i64])
                    .unwrap() as u64,
            )
        };
        assert_eq!(call(2.5, 4.0), 2.5 * 4.0 - 2.5);
        assert_eq!(call(3.0, 0.0), -3.0);
        let _ = Int; // silence unused in case
    }

    #[test]
    fn float_read_helpers_compile_and_bail() {
        use JitValueType::{Float, Handle, Int};
        // A module whose float helpers return a fixed value or bail by parity of
        // the slot/index, so we can exercise both the success and bail channels.
        extern "C" fn field_float(_handle: i64, slot: i64) -> f64 {
            if slot == 0 {
                2.5
            } else {
                signal_bail();
                0.0
            }
        }
        extern "C" fn list_get_float(_handle: i64, index: i64) -> f64 {
            if index >= 0 {
                index as f64 + 0.5
            } else {
                signal_bail();
                0.0
            }
        }
        let mut m = NativeModule::new(HostHelpers {
            field_int: noop_field_int,
            list_len: noop_list_len,
            list_get_int: noop_list_get_int,
            field_float,
            list_get_float,
            closure_id: noop_closure_id,
        })
        .unwrap();
        // fn(h: Handle, idx: Int) -> Float { return list[idx] }  regs 0=h,1=idx,2=res
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Float],
                vec![
                    JitInstr::ListGetFloat {
                        dst: 2,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        // Handle arg is opaque (helper ignores it); index 3 → 3.5.
        let got = m.callt(id, &[0, 3]).unwrap();
        assert_eq!(f64::from_bits(got as u64), 3.5);
        // Negative index → helper signals bail → None.
        assert_eq!(m.callt(id, &[0, -1]), None);

        // fn(h: Handle) -> Float { return field[1] }  → bails (slot != 0).
        let id2 = m
            .compile(&ft(
                1,
                vec![Handle, Float],
                vec![
                    JitInstr::FieldFloat {
                        dst: 1,
                        base: 0,
                        slot: 1,
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(id2, &[0]), None);
        let _ = Int;
    }

    #[test]
    fn guard_closure_id_passes_or_bails() {
        use JitValueType::{Handle, Int};
        // `closure_id` is the identity on the handle arg, so the handle value IS the
        // observed function id — letting the test drive the guard directly.
        extern "C" fn closure_id(handle: i64) -> i64 {
            handle
        }
        let mut m = NativeModule::new(HostHelpers {
            field_int: noop_field_int,
            list_len: noop_list_len,
            list_get_int: noop_list_get_int,
            field_float: noop_field_float,
            list_get_float: noop_list_get_float,
            closure_id,
        })
        .unwrap();
        // fn(h: Handle, x: Int) -> Int { guard id(h) == 7; return x + 100 }
        // regs 0=h, 1=x, 2=hundred, 3=res
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Int, Int],
                vec![
                    JitInstr::GuardClosureId {
                        base: 0,
                        expected: 7,
                    },
                    JitInstr::LoadInt {
                        dst: 2,
                        value: 100,
                    },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        // Matching callee (handle == expected 7): native completes.
        assert_eq!(m.callt(id, &[7, 5]), Some(105));
        // Mismatched callee (handle != 7): guard bails to the interpreter (None).
        assert_eq!(m.callt(id, &[8, 5]), None);
        assert_eq!(m.callt(id, &[0, 5]), None);
    }

    #[test]
    fn closure_id_dispatch_selects_arm_or_bails() {
        use JitValueType::{Handle, Int};
        // `closure_id` is the identity on the handle arg, so the handle value IS the
        // observed function id — the test drives the polymorphic dispatch directly,
        // mirroring the producer's lowering (read id once via ClosureId, then
        // LoadInt + Equal + JumpIfBool per arm, with a no-match Bail).
        extern "C" fn closure_id(handle: i64) -> i64 {
            handle
        }
        let mut m = NativeModule::new(HostHelpers {
            field_int: noop_field_int,
            list_len: noop_list_len,
            list_get_int: noop_list_get_int,
            field_float: noop_field_float,
            list_get_float: noop_list_get_float,
            closure_id,
        })
        .unwrap();
        // Polymorphic inline cache over two callees {3, 5}:
        //   0: id  = closure_id(h)
        //   1: key = 3
        //   2: eq  = (id == key)
        //   3: if eq -> arm3 (8)
        //   4: key = 5
        //   5: eq  = (id == key)
        //   6: if eq -> arm5 (11)
        //   7: Bail                    ; no match
        //   8: c30 = 30  9: res = x+30  10: return res   (arm3)
        //  11: c50 = 50 12: res = x+50  13: return res   (arm5)
        // regs: 0=h(Handle) 1=x 2=id 3=key 4=eq 5=c30 6=res3 7=c50 8=res5
        let id = m
            .compile(&ft(
                2,
                vec![Handle, Int, Int, Int, Int, Int, Int, Int, Int],
                vec![
                    JitInstr::ClosureId { dst: 2, base: 0 }, // 0
                    JitInstr::LoadInt { dst: 3, value: 3 },  // 1
                    JitInstr::Equal {
                        dst: 4,
                        lhs: 2,
                        rhs: 3,
                    }, // 2
                    JitInstr::JumpIfBool {
                        cond: 4,
                        expected: true,
                        target: 8,
                    }, // 3
                    JitInstr::LoadInt { dst: 3, value: 5 }, // 4
                    JitInstr::Equal {
                        dst: 4,
                        lhs: 2,
                        rhs: 3,
                    }, // 5
                    JitInstr::JumpIfBool {
                        cond: 4,
                        expected: true,
                        target: 11,
                    }, // 6
                    JitInstr::Bail,                          // 7 (no match)
                    JitInstr::LoadInt { dst: 5, value: 30 }, // 8 arm3
                    JitInstr::Add {
                        dst: 6,
                        lhs: 1,
                        rhs: 5,
                    }, // 9
                    JitInstr::Return { src: 6 },             // 10
                    JitInstr::LoadInt { dst: 7, value: 50 }, // 11 arm5
                    JitInstr::Add {
                        dst: 8,
                        lhs: 1,
                        rhs: 7,
                    }, // 12
                    JitInstr::Return { src: 8 }, // 13
                ],
            ))
            .unwrap();
        // Arm 3 selected (h == 3): x + 30.
        assert_eq!(m.callt(id, &[3, 5]), Some(35));
        // Arm 5 selected (h == 5): x + 50.
        assert_eq!(m.callt(id, &[5, 5]), Some(55));
        // No arm matches (h == 9): the cache bails to the interpreter (None).
        assert_eq!(m.callt(id, &[9, 5]), None);
        assert_eq!(m.callt(id, &[0, 5]), None);
    }

    #[test]
    fn direct_flat_reads_index_in_register() {
        use JitValueType::{Float, FlatFloat, FlatInt, Int};
        let mut m = module();

        // fn(a: FlatInt, i: Int) -> Int { return a[i] }  regs 0=a,1=i,2=res
        let id_int = m
            .compile(&ft(
                2,
                vec![FlatInt, Int, Int],
                vec![
                    JitInstr::ListGetIntDirect {
                        dst: 2,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let ints: Vec<i64> = vec![10, 20, 30];
        let ints_ptr = ints.as_ptr() as i64;
        let ilen = ints.len() as i64;
        // In-bounds reads index directly out of the flat buffer.
        assert_eq!(m.call(id_int, &[ints_ptr, 0], &[ilen, 0]).completed(), Some(10));
        assert_eq!(m.call(id_int, &[ints_ptr, 2], &[ilen, 0]).completed(), Some(30));
        // OOB (>= len and < 0) → fallback (None), like the helper's bail.
        assert_eq!(m.call(id_int, &[ints_ptr, 3], &[ilen, 0]).completed(), None);
        assert_eq!(m.call(id_int, &[ints_ptr, -1], &[ilen, 0]).completed(), None);

        // fn(a: FlatFloat, i: Int) -> Float { return a[i] }
        let id_f = m
            .compile(&ft(
                2,
                vec![FlatFloat, Int, Float],
                vec![
                    JitInstr::ListGetFloatDirect {
                        dst: 2,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let floats: Vec<f64> = vec![1.5, 2.5, 3.5];
        let fptr = floats.as_ptr() as i64;
        let flen = floats.len() as i64;
        let read = |i: i64| {
            m.call(id_f, &[fptr, i], &[flen, 0])
                .completed()
                .map(|b| f64::from_bits(b as u64))
        };
        assert_eq!(read(1), Some(2.5));
        assert_eq!(read(0), Some(1.5));
        assert_eq!(read(3), None);

        // fn(a: FlatInt) -> Int { return len(a) }  via ListLenDirect
        let id_len = m
            .compile(&ft(
                1,
                vec![FlatInt, Int],
                vec![
                    JitInstr::ListLenDirect { dst: 1, base: 0 },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        assert_eq!(m.call(id_len, &[ints_ptr], &[ilen]).completed(), Some(3));
    }

    #[test]
    fn distinct_bail_sites_get_stable_safepoint_ids() {
        use JitValueType::{FlatInt, Int};
        let mut m = module();

        // fn(a: FlatInt, x: Int, i: Int) -> Int { t = x + x; return a[i] }
        // Two distinct bail sites: the `Add` overflow guard (site 1) precedes the
        // `ListGetIntDirect` OOB guard (site 2). regs 0=a,1=x,2=i,3=t,4=res.
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();
        let ints: Vec<i64> = vec![10, 20, 30];
        let ints_ptr = ints.as_ptr() as i64;
        let ilen = ints.len() as i64;

        // Bail at the FIRST site: x + x overflows, so the `Add` guard fires (id 1)
        // before the list read is ever reached.
        assert!(matches!(
            m.call(id, &[ints_ptr, i64::MAX, 0], &[ilen, 0, 0]),
            NativeOutcome::Deopt {
                safepoint_id: SafepointId(1),
                ..
            }
        ));
        // Pass the first guard (small x, no overflow) but bail at the SECOND site:
        // index 5 is out of bounds, so the direct-read OOB guard fires (id 2).
        assert!(matches!(
            m.call(id, &[ints_ptr, 1, 5], &[ilen, 0, 0]),
            NativeOutcome::Deopt {
                safepoint_id: SafepointId(2),
                ..
            }
        ));
        // Both guards pass → completes (id stays 0 = no bail recorded).
        assert!(matches!(
            m.call(id, &[ints_ptr, 1, 2], &[ilen, 0, 0]),
            NativeOutcome::Completed(_)
        ));
    }

    // --- J0.1a: deopt state-map (must-analysis) -------------------------------

    #[test]
    fn deopt_map_straightline_single_guard() {
        // fn(a, b) { t = a + b; return t }  regs 0=a,1=b,2=t. The `Add` (ip 0) has
        // one overflow guard (site id 1) → one site, resuming at ip 0 with the two
        // params live (t is not yet assigned on entry to its own instruction).
        let mut m = module();
        let id = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        let map = m.deopt_map(id).expect("map for valid id");
        assert_eq!(map.sites.len(), 1);
        assert_eq!(map.sites[0].resume_ip, 0);
        assert_eq!(
            map.sites[0].live,
            vec![(0, JitValueType::Int), (1, JitValueType::Int)]
        );
    }

    #[test]
    fn deopt_map_two_distinct_sites_track_prior_defs() {
        use JitValueType::{FlatInt, Int};
        // fn(a: FlatInt, x, i) { t = x + x; return a[i] }  regs 0=a,1=x,2=i,3=t,4=res.
        // Site 1: the `Add` overflow guard at ip 0 (t not yet live). Site 2: the
        // `ListGetIntDirect` OOB guard at ip 1 — by then `t` (reg 3) is definitely
        // assigned, so it appears in site 2's live set but not site 1's. (Mirrors
        // `distinct_bail_sites_get_stable_safepoint_ids`.)
        let mut m = module();
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();
        let map = m.deopt_map(id).expect("map for valid id");
        assert_eq!(map.sites.len(), 2);

        // Site 1 (id 1): resume at the Add (ip 0); params live, t (reg 3) is NOT.
        assert_eq!(map.sites[0].resume_ip, 0);
        assert!(!map.sites[0].live.iter().any(|(r, _)| *r == 3));
        // Params 0..3 are live (a is FlatInt, x/i are Int).
        assert_eq!(
            map.sites[0].live,
            vec![
                (0, JitValueType::FlatInt),
                (1, JitValueType::Int),
                (2, JitValueType::Int)
            ]
        );

        // Site 2 (id 2): resume at the direct read (ip 1); t (reg 3) is now live.
        assert_eq!(map.sites[1].resume_ip, 1);
        assert!(map.sites[1].live.contains(&(3, JitValueType::Int)));
    }

    #[test]
    fn deopt_map_must_analysis_excludes_one_armed_def() {
        // A register assigned on only ONE arm before a join with a guard must NOT be
        // in the join's live set (intersection / must-analysis).
        //
        //   0: if cond(reg1) goto 3            (cond is param reg 1)
        //   1:   t(reg3) = a(reg0) + a(reg0)   only the fall-through arm assigns t
        //   2:   goto 4
        //   3:   nop                           the taken arm leaves t unassigned
        //   4:   u(reg4) = a + a               guard here joins both arms
        //   5:   return u
        // regs: 0=a, 1=cond, 2=(unused scratch), 3=t, 4=u.
        use JitValueType::Int;
        let mut m = module();
        let id = m
            .compile(&ft(
                2,
                vec![Int, Int, Int, Int, Int],
                vec![
                    JitInstr::JumpIfBool {
                        cond: 1,
                        expected: true,
                        target: 3,
                    },
                    JitInstr::Add {
                        dst: 3,
                        lhs: 0,
                        rhs: 0,
                    },
                    JitInstr::Jump { target: 4 },
                    JitInstr::Nop,
                    JitInstr::Add {
                        dst: 4,
                        lhs: 0,
                        rhs: 0,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ))
            .unwrap();
        let map = m.deopt_map(id).expect("map for valid id");
        // Two Add guards: site 1 at ip 1, site 2 at the post-join ip 4.
        assert_eq!(map.sites.len(), 2);
        assert_eq!(map.sites[0].resume_ip, 1);
        assert_eq!(map.sites[1].resume_ip, 4);
        // The key assertion: at the post-join guard (ip 4), `t` (reg 3) is assigned
        // on only one arm, so intersection excludes it from the live set.
        assert!(
            !map.sites[1].live.iter().any(|(r, _)| *r == 3),
            "reg 3 assigned on only one arm must not be live at the join: {:?}",
            map.sites[1].live
        );
        // The params (regs 0 and 1) are assigned on every path → still live.
        assert!(map.sites[1].live.contains(&(0, JitValueType::Int)));
        assert!(map.sites[1].live.contains(&(1, JitValueType::Int)));
        // On the fall-through arm's own guard (ip 1) t is also not-yet live.
        assert!(!map.sites[0].live.iter().any(|(r, _)| *r == 3));
    }

    #[test]
    fn deopt_map_rejects_foreign_id() {
        // A foreign / out-of-range id yields no map, mirroring `call`'s validation.
        let mut m1 = module();
        let mut m2 = module();
        let id1 = m1.compile(&two_param_add()).unwrap();
        let _id2 = m2.compile(&two_param_add()).unwrap();
        assert!(m1.deopt_map(id1).is_some());
        assert!(m2.deopt_map(id1).is_none());
    }

    #[test]
    fn compiles_and_runs_add() {
        let mut m = module();
        // fn(a, b) { return a + b }   regs: 0=a,1=b,2=tmp
        let id = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Add {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(id, &[3, 4]), Some(7));
        assert_eq!(m.callt(id, &[-10, 4]), Some(-6));
        // overflow bails:
        assert_eq!(m.callt(id, &[i64::MAX, 1]), None);
    }

    #[test]
    fn loop_sum_to_n() {
        // fn(n) { total=0; i=1; while i<=n { total+=i; i+=1 } return total }
        // regs: 0=n, 1=total, 2=i, 3=one
        let mut m = module();
        let code = vec![
            JitInstr::LoadInt { dst: 1, value: 0 }, // 0 total=0
            JitInstr::LoadInt { dst: 2, value: 1 }, // 1 i=1
            JitInstr::LoadInt { dst: 3, value: 1 }, // 2 one=1
            // 3: loop head: if !(i<=n) goto end(8)
            JitInstr::JumpIfIntCompare {
                lhs: 2,
                rhs: 0,
                op: JitCompare::Le,
                expected: false,
                target: 8,
            },
            JitInstr::Add {
                dst: 1,
                lhs: 1,
                rhs: 2,
            }, // 4 total+=i
            JitInstr::Add {
                dst: 2,
                lhs: 2,
                rhs: 3,
            }, // 5 i+=1
            JitInstr::Jump { target: 3 }, // 6 loop
            JitInstr::Nop,                // 7 (padding leader)
            JitInstr::Return { src: 1 },  // 8 end
        ];
        let id = m.compile(&f(1, 4, code)).unwrap();
        assert_eq!(m.callt(id, &[10]), Some(55));
        assert_eq!(m.callt(id, &[0]), Some(0));
        assert_eq!(m.callt(id, &[100]), Some(5050));
    }

    #[test]
    fn osr_entry_runs_loop_and_exits_with_live_out() {
        // Same loop as `loop_sum_to_n`, but compiled as an OSR-entry at the loop
        // header (ip 3) with the post-loop `Return` replaced by `OsrExit`. The
        // entry loads the live-in window (regs definitely-assigned at ip 3: total,
        // i, one, n) and jumps into the loop; the loop runs natively; on exit it
        // deopts at ip 8 with the live-out window (`total`).
        let mut m = module();
        let code = vec![
            JitInstr::LoadInt { dst: 1, value: 0 }, // 0 total=0 (pre-loop; not run under OSR)
            JitInstr::LoadInt { dst: 2, value: 1 }, // 1 i=1
            JitInstr::LoadInt { dst: 3, value: 1 }, // 2 one=1
            // 3: loop head: if !(i<=n) goto end(8)
            JitInstr::JumpIfIntCompare {
                lhs: 2,
                rhs: 0,
                op: JitCompare::Le,
                expected: false,
                target: 8,
            },
            JitInstr::Add { dst: 1, lhs: 1, rhs: 2 }, // 4 total+=i
            JitInstr::Add { dst: 2, lhs: 2, rhs: 3 }, // 5 i+=1
            JitInstr::Jump { target: 3 },             // 6 loop
            JitInstr::Nop,                            // 7 (padding leader)
            JitInstr::OsrExit,                        // 8 OSR-exit (was Return)
        ];
        let prog = f(1, 4, code);
        let id = m.compile_osr(&prog, 3).unwrap();
        // The window is `n_regs`-wide, indexed by register. Seed the loop live-in:
        // total=0 (reg1), i=1 (reg2), one=1 (reg3), n (reg0). lens parallel & unused.
        let run = |n: i64| -> NativeOutcome {
            let window = [n, 0, 1, 1]; // reg0=n, reg1=total, reg2=i, reg3=one
            let lens = [0i64; 4];
            m.call(id, &window, &lens)
        };
        for &(n, expected) in &[(10i64, 55i64), (0, 0), (100, 5050)] {
            match run(n) {
                NativeOutcome::Deopt { safepoint_id, live } => {
                    let site = m.deopt_map(id).unwrap().sites[safepoint_id.0 as usize - 1].clone();
                    assert_eq!(site.resume_ip, 8, "OSR-exit resumes at the post-loop ip");
                    let total = live
                        .iter()
                        .find(|r| r.reg == 1)
                        .map(|r| match r.value {
                            DeoptValue::Int(v) => v,
                            DeoptValue::Float(_) => panic!("total is Int"),
                        })
                        .expect("total is live-out");
                    assert_eq!(total, expected, "live-out total for n={n}");
                }
                NativeOutcome::Completed(_) => panic!("OSR loop must deopt at exit, not complete"),
            }
        }
    }

    #[test]
    fn osr_rejects_non_leader_header() {
        // An OSR header ip that is not a leader / jump-target block is rejected
        // cleanly (no panic, no miscompile).
        let mut m = module();
        let prog = f(
            1,
            2,
            vec![
                JitInstr::Add { dst: 1, lhs: 0, rhs: 0 },
                JitInstr::Return { src: 1 },
            ],
        );
        // ip 1 (the Return) is a leader only if a jump targets it; here none does,
        // and it is not ip 0, so it has no block.
        assert!(m.compile_osr(&prog, 1).is_err());
    }

    #[test]
    fn div_by_zero_bails() {
        let mut m = module();
        let id = m
            .compile(&f(
                2,
                3,
                vec![
                    JitInstr::Div {
                        dst: 2,
                        lhs: 0,
                        rhs: 1,
                    },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(id, &[20, 5]), Some(4));
        assert_eq!(m.callt(id, &[20, 0]), None);
        assert_eq!(m.callt(id, &[i64::MIN, -1]), None);
    }

    // --- J0.1b: live-register value capture at deopt --------------------------

    /// Find the captured value of register `reg` in a deopt outcome's `live` set.
    fn live_value(outcome: &NativeOutcome, reg: u32) -> Option<DeoptValue> {
        match outcome {
            NativeOutcome::Deopt { live, .. } => {
                live.iter().find(|r| r.reg == reg).map(|r| r.value)
            }
            NativeOutcome::Completed(_) => None,
        }
    }

    #[test]
    fn deopt_capture_records_live_register_values() {
        use JitValueType::{FlatInt, Int};
        let mut m = module();
        // fn(xs: FlatInt, a: Int, b: Int) -> Int { t = a + b; return xs[t] }
        // regs 0=xs, 1=a, 2=b, 3=t. The `ListGetIntDirect` OOB guard (ip 1) resumes
        // with xs(0)/a(1)/b(2)/t(3) all definitely assigned on entry.
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 3,
                        base: 0,
                        index: 3,
                    },
                    JitInstr::Return { src: 3 },
                ],
            ))
            .unwrap();
        let xs: Vec<i64> = vec![10, 20, 30, 40, 50];
        let xs_ptr = xs.as_ptr() as i64;
        let xlen = xs.len() as i64;

        // In range: t = 1 + 2 = 3 → xs[3] = 40.
        assert_eq!(
            m.call(id, &[xs_ptr, 1, 2], &[xlen, 0, 0]),
            NativeOutcome::Completed(40)
        );

        // Out of range: t = 3 + 4 = 7 >= len 5 → the direct-read OOB guard bails.
        let out = m.call(id, &[xs_ptr, 3, 4], &[xlen, 0, 0]);
        assert!(matches!(out, NativeOutcome::Deopt { .. }));
        // t (reg 3) was computed before the guard fired and is captured.
        assert_eq!(live_value(&out, 3), Some(DeoptValue::Int(7)));
        // The params a (reg 1) and b (reg 2) are captured with their passed values.
        assert_eq!(live_value(&out, 1), Some(DeoptValue::Int(3)));
        assert_eq!(live_value(&out, 2), Some(DeoptValue::Int(4)));
    }

    #[test]
    fn deopt_capture_records_float_register_value() {
        use JitValueType::{Float, FlatInt, Int};
        let mut m = module();
        // fn(xs: FlatInt, i: Int, f: Float) -> Int { g = f + f; return xs[i] }
        // regs 0=xs, 1=i, 2=f, 3=g. The float `g` is definitely assigned before the
        // `ListGetIntDirect` OOB guard (ip 2), so it is captured as an exact f64.
        let id = m
            .compile(&ft(
                3,
                vec![FlatInt, Int, Float, Float],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 2,
                        rhs: 2,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 1,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 1 },
                ],
            ))
            .unwrap();
        let xs: Vec<i64> = vec![7];
        let xs_ptr = xs.as_ptr() as i64;
        let xlen = xs.len() as i64;
        let f = 1.5_f64;

        // Out of range index 9 → bail; the float g = f + f = 3.0 round-trips exactly.
        let out = m.call(id, &[xs_ptr, 9, f.to_bits() as i64], &[xlen, 0, 0]);
        assert!(matches!(out, NativeOutcome::Deopt { .. }));
        assert_eq!(live_value(&out, 3), Some(DeoptValue::Float(f + f)));
        // The float param f itself is captured exactly too.
        assert_eq!(live_value(&out, 2), Some(DeoptValue::Float(f)));
    }

    // --- J0.3: deopt-at-every-safepoint stress test (master correctness) ------

    /// Force a bail at EVERY safepoint of a few representative functions and verify
    /// the captured safepoint id + live register values are correct at each — even at
    /// safepoints that never fire under the (deliberately in-range, non-overflowing)
    /// inputs. This exercises the J0 capture/map machinery exhaustively: for every
    /// site `k`, `compile_forcing_bail(f, k)` makes only site `k` bail, and we assert
    /// the outcome is `Deopt { SafepointId(k) }` whose `live` set is exactly the one
    /// `deopt_map().sites[k-1]` advertises, each register carrying the value the
    /// function computes for it. Late sites must capture earlier intermediates.
    #[test]
    fn force_bail_at_every_safepoint_captures_correct_state() {
        use JitValueType::{FlatInt, Float, Int};

        // A representative case: a `JitFunction`, the in-range inputs (args/lens) that
        // make NO natural bail fire, and a closure giving the value the function
        // computes for each register at any safepoint, so we can check every capture.
        struct Case {
            name: &'static str,
            func: JitFunction,
            args: Vec<i64>,
            lens: Vec<i64>,
            // reg -> captured DeoptValue, for any reg appearing in a site's live set.
            expect: Box<dyn Fn(u32) -> DeoptValue>,
        }

        let ints: Vec<i64> = vec![10, 20, 30, 40, 50];
        let ints_ptr = ints.as_ptr() as i64;
        let ilen = ints.len() as i64;

        // Case A: fn(a: FlatInt, x: Int, i: Int) { t = x + x; return a[i] }
        // Sites: 1 = Add overflow guard (ip 0), 2 = ListGetIntDirect OOB (ip 1).
        // Site 2 is LATE and captures the earlier-computed t = x + x.
        let a_ptr = ints_ptr;
        let case_a = Case {
            name: "add-then-direct-get",
            func: ft(
                3,
                vec![FlatInt, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 4,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 4 },
                ],
            ),
            // a = ptr, x = 7, i = 2 (in range). t = x + x = 14.
            args: vec![a_ptr, 7, 2],
            lens: vec![ilen, 0, 0],
            expect: Box::new(|reg| match reg {
                0 => DeoptValue::Int(0), // a: ptr value is asserted separately
                1 => DeoptValue::Int(7),
                2 => DeoptValue::Int(2),
                3 => DeoptValue::Int(14),
                _ => DeoptValue::Int(0),
            }),
        };

        // Case B: fn(xs: FlatInt, i: Int, f: Float) { g = f + f; return xs[i] }
        // The float Add has no guard, so the only site is the ListGetIntDirect OOB
        // (ip 1). Its live set includes the float register g — checked as exact f64.
        let xs_ptr = ints_ptr;
        let fv = 1.25_f64;
        let case_b = Case {
            name: "float-reg-direct-get",
            func: ft(
                3,
                vec![FlatInt, Int, Float, Float],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 2,
                        rhs: 2,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 1,
                        base: 0,
                        index: 1,
                    },
                    JitInstr::Return { src: 1 },
                ],
            ),
            // xs = ptr, i = 1 (in range), f = 1.25. g = f + f = 2.5.
            args: vec![xs_ptr, 1, fv.to_bits() as i64],
            lens: vec![ilen, 0, 0],
            expect: Box::new(move |reg| match reg {
                1 => DeoptValue::Int(1),
                2 => DeoptValue::Float(fv),
                3 => DeoptValue::Float(fv + fv),
                _ => DeoptValue::Int(0),
            }),
        };

        // Case C: fn(a: FlatInt, x: Int, y: Int) { p = x + y; q = p * x; return a[y] }
        // Three sites: 1 = Add (ip 0), 2 = Mul (ip 1), 3 = ListGetIntDirect OOB (ip 2).
        // The LATE site 3 captures both earlier intermediates p and q.
        let case_c = Case {
            name: "add-mul-then-direct-get",
            func: ft(
                3,
                vec![FlatInt, Int, Int, Int, Int, Int],
                vec![
                    JitInstr::Add {
                        dst: 3,
                        lhs: 1,
                        rhs: 2,
                    },
                    JitInstr::Mul {
                        dst: 4,
                        lhs: 3,
                        rhs: 1,
                    },
                    JitInstr::ListGetIntDirect {
                        dst: 5,
                        base: 0,
                        index: 2,
                    },
                    JitInstr::Return { src: 5 },
                ],
            ),
            // a = ptr, x = 3, y = 4 (in range). p = 3 + 4 = 7, q = p * x = 21.
            args: vec![ints_ptr, 3, 4],
            lens: vec![ilen, 0, 0],
            expect: Box::new(|reg| match reg {
                1 => DeoptValue::Int(3),
                2 => DeoptValue::Int(4),
                3 => DeoptValue::Int(7),  // p
                4 => DeoptValue::Int(21), // q
                _ => DeoptValue::Int(0),
            }),
        };

        let cases = [case_a, case_b, case_c];
        let mut combinations = 0usize;
        let mut late_intermediate_checks = 0usize;

        for case in &cases {
            let mut m = module();
            // Site count from the natural (un-forced) compilation.
            let base_id = m.compile(&case.func).unwrap();
            let n = m.deopt_map(base_id).expect("map for valid id").sites.len();
            assert!(n >= 1, "{}: expected at least one safepoint", case.name);

            for k in 1..=n as u32 {
                // Force ONLY site k to bail; inputs are chosen so no natural bail fires.
                let id = m.compile_forcing_bail(&case.func, k).unwrap();
                let site = m.deopt_map(id).expect("map").sites[(k - 1) as usize].clone();
                let out = m.call(id, &case.args, &case.lens);

                // The forced site must bail with exactly its id.
                let live = match &out {
                    NativeOutcome::Deopt { safepoint_id, live } => {
                        assert_eq!(
                            *safepoint_id,
                            SafepointId(k),
                            "{}: forced site {} reported wrong safepoint id",
                            case.name,
                            k
                        );
                        live
                    }
                    NativeOutcome::Completed(_) => {
                        panic!("{}: forced site {} did not bail", case.name, k)
                    }
                };

                // The captured live registers must be exactly the map's live set...
                let mut captured: Vec<u32> = live.iter().map(|r| r.reg).collect();
                captured.sort_unstable();
                let mut expected_regs: Vec<u32> = site.live.iter().map(|(r, _)| *r).collect();
                expected_regs.sort_unstable();
                assert_eq!(
                    captured, expected_regs,
                    "{}: site {} live-reg set mismatch (map vs capture)",
                    case.name, k
                );

                // ...and each captured value must match what the function computes,
                // including FlatInt pointer registers (checked as the raw arg word).
                for &(reg, _ty) in &site.live {
                    let got = live_value(&out, reg).expect("captured reg present");
                    if case.func.reg_types[reg as usize] == FlatInt {
                        assert_eq!(
                            got,
                            DeoptValue::Int(case.args[reg as usize]),
                            "{}: site {} reg {} (flat ptr) value mismatch",
                            case.name,
                            k,
                            reg
                        );
                    } else {
                        assert_eq!(
                            got,
                            (case.expect)(reg),
                            "{}: site {} reg {} value mismatch",
                            case.name,
                            k,
                            reg
                        );
                    }
                }

                combinations += 1;
            }

            // Explicit late-site check: the LAST site of a multi-site function must
            // capture an earlier-computed intermediate with its correct value.
            if n >= 2 {
                let id = m.compile_forcing_bail(&case.func, n as u32).unwrap();
                let out = m.call(id, &case.args, &case.lens);
                // reg 3 is the first arithmetic result in cases A and C; it is computed
                // at an earlier site yet must be captured at the final site.
                assert_eq!(
                    live_value(&out, 3),
                    Some((case.expect)(3)),
                    "{}: late site {} failed to capture earlier intermediate reg 3",
                    case.name,
                    n
                );
                late_intermediate_checks += 1;
            }
        }

        // Sanity: we actually exercised every site of every case plus late checks.
        assert_eq!(combinations, 2 + 1 + 3, "unexpected (case, site) coverage");
        assert!(late_intermediate_checks >= 2, "expected late-site checks");
    }

    // --- IR validation: malformed public IR must fail cleanly, not panic ---

    #[test]
    fn rejects_out_of_range_register() {
        // `Add` reads register 5 in a 3-register function.
        let err = validate(&f(
            1,
            3,
            vec![JitInstr::Add {
                dst: 0,
                lhs: 5,
                rhs: 1,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("out of range"), "{}", err.0);
    }

    #[test]
    fn rejects_out_of_range_jump_target() {
        let err = validate(&f(1, 1, vec![JitInstr::Jump { target: 9 }])).unwrap_err();
        assert!(err.0.contains("target 9"), "{}", err.0);
    }

    #[test]
    fn rejects_conditional_branch_without_fallthrough() {
        // A trailing conditional branch has no `i + 1` to fall through to.
        let err = validate(&f(
            1,
            1,
            vec![JitInstr::JumpIfBool {
                cond: 0,
                expected: true,
                target: 0,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("fall-through"), "{}", err.0);
    }

    #[test]
    fn rejects_reg_types_length_mismatch() {
        let bad = JitFunction {
            n_params: 0,
            n_regs: 3,
            reg_types: vec![JitValueType::Int; 2],
            code: vec![],
        };
        let err = validate(&bad).unwrap_err();
        assert!(err.0.contains("reg_types length"), "{}", err.0);
    }

    #[test]
    fn rejects_params_exceeding_regs() {
        let err = validate(&f(4, 2, vec![])).unwrap_err();
        assert!(err.0.contains("n_params"), "{}", err.0);
    }

    #[test]
    fn rejects_int_op_on_float_register() {
        use JitValueType::{Float, Int};
        // `Mod` (integer-only) applied to float registers.
        let err = validate(&ft(
            2,
            vec![Float, Float, Int],
            vec![JitInstr::Mod {
                dst: 2,
                lhs: 0,
                rhs: 1,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("must be Int"), "{}", err.0);
    }

    #[test]
    fn rejects_mismatched_arith_classes() {
        use JitValueType::{Float, Int};
        // `Add` with one int and one float operand.
        let err = validate(&ft(
            2,
            vec![Int, Float, Int],
            vec![JitInstr::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("classes differ"), "{}", err.0);
    }

    #[test]
    fn rejects_handle_outside_heap_read_base() {
        use JitValueType::{Handle, Int};
        // A `Handle` register used as an arithmetic operand.
        let err = validate(&ft(
            2,
            vec![Handle, Int, Int],
            vec![JitInstr::Add {
                dst: 2,
                lhs: 0,
                rhs: 1,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("Handle"), "{}", err.0);
    }

    #[test]
    fn rejects_non_handle_heap_read_base() {
        // `FieldInt` base must be a `Handle`, not an `Int`.
        let err = validate(&f(
            1,
            2,
            vec![JitInstr::FieldInt {
                dst: 1,
                base: 0,
                slot: 0,
            }],
        ))
        .unwrap_err();
        assert!(err.0.contains("expected Handle"), "{}", err.0);
    }

    #[test]
    fn accepts_well_formed_heap_read() {
        use JitValueType::{Handle, Int};
        validate(&ft(
            1,
            vec![Handle, Int],
            vec![
                JitInstr::ListLen { dst: 1, base: 0 },
                JitInstr::Return { src: 1 },
            ],
        ))
        .expect("well-formed heap read should validate");
    }

    // fn(a, b) { return a + b } — a 2-param function for the call-guard tests.
    fn two_param_add() -> JitFunction {
        f(
            2,
            3,
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        )
    }

    #[test]
    fn call_rejects_wrong_arg_count() {
        // The generated entry block reads exactly `n_params` words from `args_ptr`,
        // so a short slice must be rejected by `call` (otherwise: out-of-bounds
        // read). Both too-few and too-many fall back rather than misread memory.
        let mut m = module();
        let id = m.compile(&two_param_add()).unwrap();
        assert_eq!(m.callt(id, &[2, 3]), Some(5));
        assert_eq!(m.callt(id, &[2]), None); // too few — must not read past the slice
        assert_eq!(m.callt(id, &[]), None);
        assert_eq!(m.callt(id, &[2, 3, 4]), None); // too many
    }

    #[test]
    fn call_rejects_id_from_another_module() {
        // A `CompiledId` minted by one module indexes that module's table; using it
        // against another module must be rejected, not silently mis-dispatched.
        let mut m1 = module();
        let mut m2 = module();
        let id1 = m1.compile(&two_param_add()).unwrap();
        let _id2 = m2.compile(&two_param_add()).unwrap();
        assert_eq!(m1.callt(id1, &[2, 3]), Some(5));
        assert_eq!(m2.callt(id1, &[2, 3]), None); // foreign id → fallback, no panic
    }

    // --- Structured fuzz: validate/compile robustness (execution spec §7) ------
    //
    // The contract is that `compile` is *total* over arbitrary `JitFunction`
    // values: a producer bug (out-of-range register, type-mismatched operand,
    // wild jump target, truncated stream) MUST surface as a clean `JitError` —
    // never a panic, never undefined behaviour, never silently-wrong machine code.
    // These tests drive thousands of random and mutation-derived programs through
    // `compile` (which runs `validate` then Cranelift codegen) and assert it
    // always returns (`Ok` or `Err`). Miscompile detection is the differential
    // suite's job (compile-vs-interpreter on real programs); here we only pin
    // robustness. Randomness is a fixed-seed xorshift so failures are reproducible
    // without an external rng/proptest dependency.

    /// Deterministic xorshift64* PRNG — reproducible, no external dep.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x >> 12;
            x ^= x << 25;
            x ^= x >> 27;
            self.0 = x;
            x.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }
        fn below(&mut self, n: u32) -> u32 {
            if n == 0 {
                0
            } else {
                (self.next() % n as u64) as u32
            }
        }
        /// A register index: usually in `0..n_regs`, occasionally out of range so
        /// `validate`'s bounds checks are exercised.
        fn reg(&mut self, n_regs: u32) -> u32 {
            if self.next() & 7 == 0 {
                self.below(n_regs.saturating_mul(2).saturating_add(3))
            } else {
                self.below(n_regs.max(1))
            }
        }
        fn vty(&mut self) -> JitValueType {
            match self.below(5) {
                0 => JitValueType::Int,
                1 => JitValueType::Float,
                2 => JitValueType::FlatInt,
                3 => JitValueType::FlatFloat,
                _ => JitValueType::Handle,
            }
        }
    }

    /// One random instruction. `n` is the code length (for jump targets), which
    /// may be exceeded so out-of-range targets are tested too.
    fn random_instr(rng: &mut Rng, n_regs: u32, n: u32) -> JitInstr {
        let r = |rng: &mut Rng| rng.reg(n_regs);
        let t = |rng: &mut Rng| rng.below(n.saturating_add(2));
        match rng.below(27) {
            22 => JitInstr::FieldFloat {
                dst: r(rng),
                base: r(rng),
                slot: rng.below(8),
            },
            23 => JitInstr::ListGetFloat {
                dst: r(rng),
                base: r(rng),
                index: r(rng),
            },
            24 => JitInstr::ListGetIntDirect {
                dst: r(rng),
                base: r(rng),
                index: r(rng),
            },
            25 => JitInstr::ListGetFloatDirect {
                dst: r(rng),
                base: r(rng),
                index: r(rng),
            },
            26 => JitInstr::ListLenDirect {
                dst: r(rng),
                base: r(rng),
            },
            0 => JitInstr::Nop,
            1 => JitInstr::Bail,
            2 => JitInstr::LoadInt {
                dst: r(rng),
                value: rng.next() as i64,
            },
            3 => JitInstr::LoadFloat {
                dst: r(rng),
                value: f64::from_bits(rng.next()),
            },
            4 => JitInstr::LoadBool {
                dst: r(rng),
                value: rng.next() & 1 == 0,
            },
            5 => JitInstr::Move {
                dst: r(rng),
                src: r(rng),
            },
            6 => JitInstr::Add {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            7 => JitInstr::Sub {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            8 => JitInstr::Mul {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            9 => JitInstr::Div {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            10 => JitInstr::Mod {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            11 => JitInstr::BitAnd {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            12 => JitInstr::Shl {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            13 => JitInstr::Shr {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            14 => JitInstr::Equal {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
            },
            15 => JitInstr::Compare {
                dst: r(rng),
                lhs: r(rng),
                rhs: r(rng),
                op: match rng.below(4) {
                    0 => JitCompare::Lt,
                    1 => JitCompare::Le,
                    2 => JitCompare::Gt,
                    _ => JitCompare::Ge,
                },
            },
            16 => JitInstr::Jump { target: t(rng) },
            17 => JitInstr::JumpIfBool {
                cond: r(rng),
                expected: rng.next() & 1 == 0,
                target: t(rng),
            },
            18 => JitInstr::Return { src: r(rng) },
            19 => JitInstr::FieldInt {
                dst: r(rng),
                base: r(rng),
                slot: rng.below(8),
            },
            20 => JitInstr::ListLen {
                dst: r(rng),
                base: r(rng),
            },
            _ => JitInstr::ListGetInt {
                dst: r(rng),
                base: r(rng),
                index: r(rng),
            },
        }
    }

    fn random_program(rng: &mut Rng) -> JitFunction {
        let n_regs = rng.below(6); // 0..=5, includes the empty-window edge case
        let n_params = if n_regs == 0 {
            0
        } else {
            rng.below(n_regs + 1)
        };
        let len = rng.below(14);
        let reg_types = (0..n_regs).map(|_| rng.vty()).collect();
        let code = (0..len).map(|_| random_instr(rng, n_regs, len)).collect();
        JitFunction {
            n_params,
            n_regs,
            reg_types,
            code,
        }
    }

    #[test]
    fn fuzz_compile_is_total_over_arbitrary_ir() {
        let mut m = module();
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for _ in 0..6000 {
            let prog = random_program(&mut rng);
            // The whole contract: never panic. Both arms are acceptable.
            match m.compile(&prog) {
                Ok(_) | Err(_) => {}
            }
        }
    }

    #[test]
    fn fuzz_compile_is_total_over_mutated_valid_ir() {
        // Seed: `fn(a, b) { t = a + b; return t }` — a known-valid program. Each
        // round perturbs one field (opcode swap, register bump, target bump,
        // truncation) and re-compiles; a mutation that invalidates the IR must be
        // caught as a clean error, not a panic.
        let mut m = module();
        let mut rng = Rng(0x1234_5678_9ABC_DEF0);
        let base = f(
            2,
            3,
            vec![
                JitInstr::Add {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                JitInstr::Return { src: 2 },
            ],
        );
        for _ in 0..4000 {
            let mut prog = base.clone();
            match rng.below(5) {
                0 => prog.n_regs = rng.below(6),
                1 => prog.n_params = rng.below(6),
                2 => {
                    if !prog.code.is_empty() {
                        let idx = rng.below(prog.code.len() as u32) as usize;
                        prog.code[idx] =
                            random_instr(&mut rng, prog.n_regs.max(1), prog.code.len() as u32);
                    }
                }
                3 => prog
                    .code
                    .truncate(rng.below(prog.code.len() as u32 + 1) as usize),
                _ => {
                    if !prog.reg_types.is_empty() {
                        let idx = rng.below(prog.reg_types.len() as u32) as usize;
                        prog.reg_types[idx] = rng.vty();
                    }
                }
            }
            match m.compile(&prog) {
                Ok(_) | Err(_) => {}
            }
        }
    }

    /// Execution robustness + host-helper handle fuzz: drive *loop-free* (forward-
    /// jump-only, so guaranteed-terminating) validated programs through `call` with
    /// random argument bit patterns — including `Handle` args fed to the no-op
    /// `field_int`/`list_len`/`list_get_int` helpers at random slots/indices. The
    /// compiled code must always return cleanly (`Some`/`None` — a value or a bail),
    /// never UB or a hang. Loop-free generation is what keeps this from spinning on
    /// the native tier, which (by design, §6.2) has no internal step limit.
    #[test]
    fn fuzz_straightline_execution_never_traps_host() {
        let mut m = module();
        let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..3000 {
            let n_regs = rng.below(5).max(1);
            let n_params = rng.below(n_regs + 1);
            let len = rng.below(8);
            let reg_types: Vec<JitValueType> = (0..n_regs).map(|_| rng.vty()).collect();
            let mut code = Vec::new();
            for i in 0..len {
                // Forward-only jumps (target strictly after this index, up to `len`),
                // so control flow always makes progress to the end.
                let forward = i + 1 + rng.below(len.saturating_sub(i).max(1));
                let instr = match rng.below(12) {
                    0 => JitInstr::Jump { target: forward },
                    1 => JitInstr::JumpIfBool {
                        cond: rng.below(n_regs),
                        expected: rng.next() & 1 == 0,
                        target: forward,
                    },
                    other => random_instr(&mut rng, n_regs, len).pipe_nonjump(other),
                };
                code.push(instr);
            }
            // Guarantee a terminating tail so a validated function returns.
            code.push(JitInstr::Return {
                src: rng.below(n_regs),
            });
            let prog = JitFunction {
                n_params,
                n_regs,
                reg_types,
                code,
            };
            if let Ok(id) = m.compile(&prog) {
                let args: Vec<i64> = (0..n_params).map(|_| rng.next() as i64).collect();
                // Must return without UB/hang; value or bail are both fine.
                let _ = m.callt(id, &args);
            }
        }
    }

    // Small helper: keep a non-jump instruction as-is (jumps are generated with
    // forward targets separately above).
    impl JitInstr {
        fn pipe_nonjump(self, _tag: u32) -> JitInstr {
            match self {
                // Re-point any stray jump the generator produced to a Nop so this
                // path stays loop-free; all other instructions pass through.
                JitInstr::Jump { .. } | JitInstr::JumpIfBool { .. } => JitInstr::Nop,
                other => other,
            }
        }
    }

    // --- J4.3: conservative range proof for eliding overflow checks -----------

    /// LoadInt c ⇒ [c, c]; an Add of two known constants whose sum fits i64 is
    /// proven non-overflowing (and the proof line is computed in i128).
    #[test]
    fn interval_load_and_add_constants() {
        let prog = f(
            0,
            3,
            vec![
                JitInstr::LoadInt { dst: 0, value: 5 },
                JitInstr::LoadInt { dst: 1, value: 3 },
                JitInstr::Add { dst: 2, lhs: 0, rhs: 1 },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&prog);
        // On entry to the Add (ip 2), reg0=[5,5], reg1=[3,3].
        assert_eq!(iv[2][0], Interval { lo: 5, hi: 5 });
        assert_eq!(iv[2][1], Interval { lo: 3, hi: 3 });
        // The Add is proven non-overflowing ([8,8] ⊂ i64).
        assert!(arith_cannot_overflow(&iv[2], &prog.code[2]));
        // The result [8,8] flows to the Return's in-set (ip 3).
        assert_eq!(iv[3][2], Interval { lo: 8, hi: 8 });
    }

    /// A proven-unchecked add of two large constants whose sum is STILL within i64
    /// produces the exact (non-wrapping) sum — the unchecked op is byte-identical to
    /// the checked one here because the proof guarantees no overflow.
    #[test]
    fn proven_large_constant_add_is_correct() {
        let mut m = module();
        // c = (i64::MAX - 10) + 10 = i64::MAX, proven safe ⇒ unchecked, exact.
        let id = m
            .compile(&f(
                0,
                3,
                vec![
                    JitInstr::LoadInt { dst: 0, value: i64::MAX - 10 },
                    JitInstr::LoadInt { dst: 1, value: 10 },
                    JitInstr::Add { dst: 2, lhs: 0, rhs: 1 },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(id, &[]), Some(i64::MAX));
    }

    /// Boundary: operand ranges summing to EXACTLY i64::MAX are proven safe; summing
    /// to i64::MAX + 1 are NOT (the analysis draws the line at the i64 boundary).
    #[test]
    fn boundary_exact_max_proven_overflow_unproven() {
        // Proven: (i64::MAX - 1) + 1 = i64::MAX, fits ⇒ unchecked.
        let safe = f(
            0,
            3,
            vec![
                JitInstr::LoadInt { dst: 0, value: i64::MAX - 1 },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add { dst: 2, lhs: 0, rhs: 1 },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&safe);
        assert_eq!(iv[2][0], Interval { lo: i64::MAX as i128 - 1, hi: i64::MAX as i128 - 1 });
        assert!(arith_cannot_overflow(&iv[2], &safe.code[2]));

        // Just over: i64::MAX + 1 overflows ⇒ NOT proven, stays checked.
        let over = f(
            0,
            3,
            vec![
                JitInstr::LoadInt { dst: 0, value: i64::MAX },
                JitInstr::LoadInt { dst: 1, value: 1 },
                JitInstr::Add { dst: 2, lhs: 0, rhs: 1 },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv2 = interval_analysis(&over);
        assert!(!arith_cannot_overflow(&iv2[2], &over.code[2]));
    }

    /// The proven-boundary add runs unchecked and yields exactly i64::MAX (no bail);
    /// the over-boundary constant add — which the proof leaves CHECKED — bails on its
    /// actual overflow. Same analysis, opposite emission, both correct.
    #[test]
    fn boundary_proven_runs_overflow_constant_bails() {
        let mut m = module();
        let safe = m
            .compile(&f(
                0,
                3,
                vec![
                    JitInstr::LoadInt { dst: 0, value: i64::MAX - 1 },
                    JitInstr::LoadInt { dst: 1, value: 1 },
                    JitInstr::Add { dst: 2, lhs: 0, rhs: 1 },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        assert_eq!(m.callt(safe, &[]), Some(i64::MAX));

        let over = m
            .compile(&f(
                0,
                3,
                vec![
                    JitInstr::LoadInt { dst: 0, value: i64::MAX },
                    JitInstr::LoadInt { dst: 1, value: 1 },
                    JitInstr::Add { dst: 2, lhs: 0, rhs: 1 },
                    JitInstr::Return { src: 2 },
                ],
            ))
            .unwrap();
        // Constant operands are tracked, [i64::MAX]+[1] doesn't fit ⇒ stays checked ⇒
        // the sadd_overflow guard fires on the real overflow ⇒ Deopt (None).
        assert_eq!(m.callt(over, &[]), None);
    }

    /// Params are untracked (TOP). `a + b` over params stays CHECKED and bails on a
    /// real overflow (i64::MAX + 1) exactly as before — proving checks are NOT
    /// over-eagerly stripped.
    #[test]
    fn unknown_params_stay_checked_and_bail() {
        let mut m = module();
        // fn(a, b) -> Int { return a + b }, params untracked ⇒ TOP ⇒ checked.
        let prog = f(
            2,
            3,
            vec![
                JitInstr::Add { dst: 2, lhs: 0, rhs: 1 },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&prog);
        // Both operands TOP ⇒ result range is TOP ⇒ not proven.
        assert!(!arith_cannot_overflow(&iv[0], &prog.code[0]));
        let id = m.compile(&prog).unwrap();
        // In-range add returns the exact sum.
        assert_eq!(m.callt(id, &[2, 3]), Some(5));
        // i64::MAX + 1 overflows ⇒ the retained check bails.
        assert_eq!(m.callt(id, &[i64::MAX, 1]), None);
    }

    /// A register fed by `ListLen` is `[0, i64::MAX]` — non-negative but with NO
    /// tighter upper bound. So `len + len` can reach 2*i64::MAX, does NOT fit i64,
    /// and stays CHECKED (we did not assume a smaller length bound).
    #[test]
    fn list_len_is_nonneg_unbounded_above() {
        use JitValueType::{Handle, Int};
        let prog = ft(
            1,
            vec![Handle, Int, Int],
            vec![
                JitInstr::ListLen { dst: 1, base: 0 },
                // len + len
                JitInstr::Add { dst: 2, lhs: 1, rhs: 1 },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv = interval_analysis(&prog);
        // ListLen result is exactly [0, i64::MAX] on entry to the Add (ip 1).
        assert_eq!(iv[1][1], Interval { lo: 0, hi: i64::MAX as i128 });
        // [0,MAX]+[0,MAX] = [0, 2*MAX] does NOT fit i64 ⇒ stays checked.
        assert!(!arith_cannot_overflow(&iv[1], &prog.code[1]));
    }

    /// ListLenDirect is treated identically to ListLen: [0, i64::MAX].
    #[test]
    fn list_len_direct_is_nonneg() {
        use JitValueType::{FlatInt, Int};
        let prog = ft(
            1,
            vec![FlatInt, Int],
            vec![
                JitInstr::ListLenDirect { dst: 1, base: 0 },
                JitInstr::Return { src: 1 },
            ],
        );
        let iv = interval_analysis(&prog);
        assert_eq!(iv[1][1], Interval { lo: 0, hi: i64::MAX as i128 });
    }

    /// Sub and Mul interval transfer functions, plus their proven/unproven lines.
    #[test]
    fn interval_sub_and_mul_transfer() {
        // (10 - 3) = 7, proven; Move copies a range.
        let prog = f(
            0,
            4,
            vec![
                JitInstr::LoadInt { dst: 0, value: 10 },
                JitInstr::LoadInt { dst: 1, value: 3 },
                JitInstr::Sub { dst: 2, lhs: 0, rhs: 1 },
                JitInstr::Mul { dst: 3, lhs: 2, rhs: 1 },
                JitInstr::Return { src: 3 },
            ],
        );
        let iv = interval_analysis(&prog);
        assert!(arith_cannot_overflow(&iv[2], &prog.code[2])); // [10,10]-[3,3]=[7,7]
        assert_eq!(iv[3][2], Interval { lo: 7, hi: 7 });
        assert!(arith_cannot_overflow(&iv[3], &prog.code[3])); // [7,7]*[3,3]=[21,21]

        // A Mul of two large constants whose product overflows is NOT proven.
        let big = f(
            0,
            3,
            vec![
                JitInstr::LoadInt { dst: 0, value: i64::MAX },
                JitInstr::LoadInt { dst: 1, value: 2 },
                JitInstr::Mul { dst: 2, lhs: 0, rhs: 1 },
                JitInstr::Return { src: 2 },
            ],
        );
        let iv2 = interval_analysis(&big);
        assert!(!arith_cannot_overflow(&iv2[2], &big.code[2]));
    }

    /// A register incremented across a loop back-edge widens to TOP (we infer no loop
    /// bound), so an unbounded accumulator's add stays CHECKED — and the fixpoint
    /// terminates.
    #[test]
    fn loop_accumulator_widens_to_top() {
        // i = 0; loop { i = i + 1; } (no exit) — i grows unbounded.
        // regs: 0=i, 1=one
        let prog = f(
            0,
            2,
            vec![
                JitInstr::LoadInt { dst: 0, value: 0 }, // 0
                JitInstr::LoadInt { dst: 1, value: 1 }, // 1
                JitInstr::Add { dst: 0, lhs: 0, rhs: 1 }, // 2: i = i + 1
                JitInstr::Jump { target: 2 },           // 3: back-edge to the Add
            ],
        );
        let iv = interval_analysis(&prog);
        // After widening, `i` on entry to the Add is TOP ⇒ the increment stays checked.
        assert_eq!(iv[2][0], Interval::TOP);
        assert!(!arith_cannot_overflow(&iv[2], &prog.code[0 + 2]));
    }
}
