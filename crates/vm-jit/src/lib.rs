//! Native (Cranelift) baseline JIT for the RSScript register VM's numeric /
//! boolean / control-flow core — Phase 2 of `docs/jit-roadmap.md`.
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
}

/// Version of the [`JitInstr`]/[`JitFunction`] IR this crate consumes. The
/// producer (`rsscript`) translates its private bytecode into this stable,
/// versioned surface, so the two crates are decoupled: a breaking IR change bumps
/// this and the producer is updated in lock-step.
pub const IR_VERSION: u32 = 2;

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
/// `(args_ptr, n_args, out_ptr, bail_ptr) -> completed`. Returns `1` and writes
/// the result to `*out` on success, or `0` (leaving `*out` untouched) to request
/// fallback. `bail_ptr` points at a `u8` flag the host helpers set when a heap
/// read can't be satisfied; the generated code loads it after every helper call
/// and branches to fallback immediately, so a bad read can't keep executing.
type CompiledAbi = unsafe extern "C" fn(*const i64, usize, *mut i64, *const u8) -> u8;

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

/// A compiled function plus the metadata `call` needs to invoke it safely: the
/// param count, so `call` can reject an argument slice of the wrong length (the
/// generated entry block reads exactly `n_params` words from `args_ptr` and does
/// not bound-check against `n_args`).
struct CompiledFunc {
    f: CompiledAbi,
    n_params: usize,
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
}

/// Handle to a function compiled into a [`NativeModule`]. Carries the minting
/// module's identity so it can't be used against a different module (which would
/// index the wrong function table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledId {
    module_id: u64,
    index: usize,
}

impl NativeModule {
    pub fn new(helpers: HostHelpers) -> Result<Self, JitError> {
        let mut flags = settings::builder();
        // Plain JIT: no PIC, and optimize for speed (this is the hot path).
        flags
            .set("use_colocated_libcalls", "false")
            .map_err(|e| err("settings", e))?;
        flags
            .set("is_pic", "false")
            .map_err(|e| err("settings", e))?;
        flags
            .set("opt_level", "speed")
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
        let mut module = JITModule::new(builder);
        let imports = HostFuncs {
            field_int: declare_import(&mut module, "rss_jit_field_int", 2)?,
            list_len: declare_import(&mut module, "rss_jit_list_len", 1)?,
            list_get_int: declare_import(&mut module, "rss_jit_list_get_int", 2)?,
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
        // `JitFunction` is a public, versioned surface: a malformed producer must
        // fail cleanly here, not panic inside `build_function` (out-of-range index)
        // or trip Cranelift's verifier (a type mismatch) deep in codegen.
        validate(function)?;
        let ptr_ty = self.module.target_config().pointer_type();
        self.module.clear_context(&mut self.ctx);
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // args ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // n_args
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // out ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // bail flag ptr
        self.ctx
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I8));

        build_function(
            &mut self.ctx.func,
            &mut self.fbctx,
            &mut self.module,
            self.imports,
            function,
        );

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
        });
        Ok(handle)
    }

    /// Run a compiled function. Returns `Some(result)` on completion, or `None`
    /// when the native code bailed and the interpreter should re-run the function —
    /// either a guard bail (overflow/divide-by-zero edge) or a host-helper bail
    /// (an unsatisfiable heap read; see [`signal_bail`]).
    ///
    /// This is a **fully safe** boundary: it takes no raw pointers. The bail flag
    /// is a per-thread `u8` owned by this crate; `call` resets it, passes its own
    /// address into the generated code, and reports a set flag as a fallback. The
    /// only `unsafe` is the indirect call through a pointer this module emitted with
    /// the matching ABI, with every pointer it passes derived from owned locals.
    pub fn call(&self, id: CompiledId, args: &[i64]) -> Option<i64> {
        // Reject an id from a different module and an out-of-range index: either
        // would invoke the wrong (or no) function. Falling back is always safe.
        if id.module_id != self.id {
            return None;
        }
        let func = self.funcs.get(id.index)?;
        // The generated entry block reads exactly `n_params` words from `args_ptr`
        // without consulting `n_args`, so an args slice shorter than `n_params`
        // would read out of bounds. Reject any length mismatch and fall back.
        if args.len() != func.n_params {
            return None;
        }
        let f = func.f;
        let mut out: i64 = 0;
        BAIL_FLAG.with(|bail| {
            bail.set(0);
            let bail_ptr = bail.as_ptr() as *const u8;
            // SAFETY: `f` was produced by `compile` with the `CompiledAbi`
            // signature; it reads `args.len()` i64s from `args.as_ptr()`, writes
            // one i64 to `&mut out`, and only ever loads (never stores) the `u8` at
            // `bail_ptr` — this thread's `BAIL_FLAG` cell, valid for the call. The
            // generated code never retains any of the pointers.
            let completed = unsafe { f(args.as_ptr(), args.len(), &mut out as *mut i64, bail_ptr) };
            if completed != 0 && bail.get() == 0 {
                Some(out)
            } else {
                None
            }
        })
    }
}

std::thread_local! {
    /// Per-thread bail flag shared between the in-flight compiled call (which loads
    /// it) and the host helpers (which set it via [`signal_bail`]). `call` resets it
    /// before each invocation, so it is only meaningful during a call.
    static BAIL_FLAG: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
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
        if class(lhs) == JitValueType::Handle || class(rhs) == JitValueType::Handle {
            return Err(JitError(format!("{op}: operand is a Handle register")));
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
                if class(*src) == JitValueType::Handle || class(*dst) == JitValueType::Handle {
                    return Err(JitError("Move: Handle registers cannot be moved".into()));
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
                if class(*src) == JitValueType::Handle {
                    return Err(JitError("Return: cannot return a Handle register".into()));
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
        }
    }
    Ok(())
}

fn build_function(
    func: &mut cranelift_codegen::ir::Function,
    fbctx: &mut FunctionBuilderContext,
    module: &mut JITModule,
    imports: HostFuncs,
    program: &JitFunction,
) {
    let mut bcx = FunctionBuilder::new(func, fbctx);

    // Per-function references to the imported host helpers (heap reads call these).
    let field_int_ref = module.declare_func_in_func(imports.field_int, bcx.func);
    let list_len_ref = module.declare_func_in_func(imports.list_len, bcx.func);
    let list_get_int_ref = module.declare_func_in_func(imports.list_get_int, bcx.func);

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
    let out_ptr = params[2];
    let bail_ptr = params[3];
    for (i, &var) in vars.iter().take(program.n_params as usize).enumerate() {
        let v = bcx
            .ins()
            .load(var_ty(i), MemFlags::trusted(), args_ptr, (i as i32) * 8);
        bcx.def_var(var, v);
    }
    let zero_i = bcx.ins().iconst(types::I64, 0);
    let zero_f = bcx.ins().f64const(0.0);
    for (i, &var) in vars
        .iter()
        .enumerate()
        .take(n_regs)
        .skip(program.n_params as usize)
    {
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
            JitInstr::Return { .. } | JitInstr::Bail if i + 1 < n => {
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

    if n == 0 {
        bcx.ins().jump(fallback, &[]);
    } else {
        bcx.ins().jump(block_for[0].unwrap(), &[]);
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
                } else {
                    let (res, of) = bcx.ins().sadd_overflow(a, b);
                    let cont = bail_if(&mut bcx, of, fallback);
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
                } else {
                    let (res, of) = bcx.ins().ssub_overflow(a, b);
                    let cont = bail_if(&mut bcx, of, fallback);
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
                } else {
                    let (res, of) = bcx.ins().smul_overflow(a, b);
                    let cont = bail_if(&mut bcx, of, fallback);
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
                    let res = emit_checked_divrem(&mut bcx, reg(*lhs), reg(*rhs), fallback, false);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Mod { dst, lhs, rhs } => {
                // Float modulo is a runtime error in the VM, so only integer
                // registers reach here (eligibility rejects float `%`).
                let res = emit_checked_divrem(&mut bcx, reg(*lhs), reg(*rhs), fallback, true);
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
                let res = emit_checked_shift(&mut bcx, reg(*lhs), reg(*rhs), fallback, false);
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Shr { dst, lhs, rhs } => {
                let res = emit_checked_shift(&mut bcx, reg(*lhs), reg(*rhs), fallback, true);
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
            JitInstr::FieldInt { dst, base, slot } => {
                let handle = bcx.use_var(reg(*base));
                let slot_v = bcx.ins().iconst(types::I64, i64::from(*slot));
                let call = bcx.ins().call(field_int_ref, &[handle, slot_v]);
                let result = bcx.inst_results(call)[0];
                let cont = bail_if_helper_failed(&mut bcx, bail_ptr, fallback);
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListLen { dst, base } => {
                let handle = bcx.use_var(reg(*base));
                let call = bcx.ins().call(list_len_ref, &[handle]);
                let result = bcx.inst_results(call)[0];
                let cont = bail_if_helper_failed(&mut bcx, bail_ptr, fallback);
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListGetInt { dst, base, index } => {
                let handle = bcx.use_var(reg(*base));
                let index_v = bcx.use_var(reg(*index));
                let call = bcx.ins().call(list_get_int_ref, &[handle, index_v]);
                let result = bcx.inst_results(call)[0];
                let cont = bail_if_helper_failed(&mut bcx, bail_ptr, fallback);
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), result);
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
}

/// Emit `brif(cond, fallback, cont)` and return the `cont` block to continue in.
fn bail_if(bcx: &mut FunctionBuilder, cond: Value, fallback: Block) -> Block {
    let cont = bcx.create_block();
    bcx.ins().brif(cond, fallback, &[], cont, &[]);
    cont
}

/// Load the host-helper bail flag and branch to `fallback` if a preceding heap
/// read flagged failure — checked immediately after each helper call so a bad
/// read never keeps executing. Returns the continuation block.
fn bail_if_helper_failed(bcx: &mut FunctionBuilder, bail_ptr: Value, fallback: Block) -> Block {
    let flag = bcx.ins().load(types::I8, MemFlags::trusted(), bail_ptr, 0);
    bail_if(bcx, flag, fallback)
}

/// Checked division / remainder matching the interpreter: bail on divide-by-zero
/// and on `i64::MIN / -1` (the only signed-division overflow).
fn emit_checked_divrem(
    bcx: &mut FunctionBuilder,
    lhs: Variable,
    rhs: Variable,
    fallback: Block,
    is_rem: bool,
) -> Value {
    let a = bcx.use_var(lhs);
    let b = bcx.use_var(rhs);
    let zero = bcx.ins().iconst(types::I64, 0);
    let is_zero = bcx.ins().icmp(IntCC::Equal, b, zero);
    let cont1 = bail_if(bcx, is_zero, fallback);
    bcx.switch_to_block(cont1);
    let imin = bcx.ins().iconst(types::I64, i64::MIN);
    let neg1 = bcx.ins().iconst(types::I64, -1);
    let a_is_min = bcx.ins().icmp(IntCC::Equal, a, imin);
    let b_is_neg1 = bcx.ins().icmp(IntCC::Equal, b, neg1);
    let overflow = bcx.ins().band(a_is_min, b_is_neg1);
    let cont2 = bail_if(bcx, overflow, fallback);
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
    is_right: bool,
) -> Value {
    let a = bcx.use_var(lhs);
    let amt = bcx.use_var(rhs);
    let limit = bcx.ins().iconst(types::I64, 64);
    // Unsigned compare folds "negative" (huge unsigned) and ">= 64" into one test.
    let oob = bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, amt, limit);
    let cont = bail_if(bcx, oob, fallback);
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

    extern "C" fn noop_field_int(_handle: i64, _slot: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_len(_handle: i64) -> i64 {
        0
    }
    extern "C" fn noop_list_get_int(_handle: i64, _index: i64) -> i64 {
        0
    }

    /// A module with no-op host helpers (these tests exercise only scalar ops).
    fn module() -> NativeModule {
        NativeModule::new(HostHelpers {
            field_int: noop_field_int,
            list_len: noop_list_len,
            list_get_int: noop_list_get_int,
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
                m.call(id, &[a.to_bits() as i64, b.to_bits() as i64])
                    .unwrap() as u64,
            )
        };
        assert_eq!(call(2.5, 4.0), 2.5 * 4.0 - 2.5);
        assert_eq!(call(3.0, 0.0), -3.0);
        let _ = Int; // silence unused in case
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
        assert_eq!(m.call(id, &[3, 4]), Some(7));
        assert_eq!(m.call(id, &[-10, 4]), Some(-6));
        // overflow bails:
        assert_eq!(m.call(id, &[i64::MAX, 1]), None);
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
        assert_eq!(m.call(id, &[10]), Some(55));
        assert_eq!(m.call(id, &[0]), Some(0));
        assert_eq!(m.call(id, &[100]), Some(5050));
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
        assert_eq!(m.call(id, &[20, 5]), Some(4));
        assert_eq!(m.call(id, &[20, 0]), None);
        assert_eq!(m.call(id, &[i64::MIN, -1]), None);
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
        assert_eq!(m.call(id, &[2, 3]), Some(5));
        assert_eq!(m.call(id, &[2]), None); // too few — must not read past the slice
        assert_eq!(m.call(id, &[]), None);
        assert_eq!(m.call(id, &[2, 3, 4]), None); // too many
    }

    #[test]
    fn call_rejects_id_from_another_module() {
        // A `CompiledId` minted by one module indexes that module's table; using it
        // against another module must be rejected, not silently mis-dispatched.
        let mut m1 = module();
        let mut m2 = module();
        let id1 = m1.compile(&two_param_add()).unwrap();
        let _id2 = m2.compile(&two_param_add()).unwrap();
        assert_eq!(m1.call(id1, &[2, 3]), Some(5));
        assert_eq!(m2.call(id1, &[2, 3]), None); // foreign id → fallback, no panic
    }
}
