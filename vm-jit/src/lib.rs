//! Native (Cranelift) baseline JIT for the RSScript register VM's integer /
//! boolean / control-flow core — Phase 2 of `docs/jit-roadmap.md`.
//!
//! # What it compiles
//!
//! A [`JitFunction`] is a stable, versioned slice of the VM's bytecode: the
//! subset that operates purely on unboxed `i64` registers (integers, and booleans
//! represented as `0`/`1`) with no calls, heap, async, or side effects. The main
//! `rsscript` crate translates an eligible `RegFunction` into this IR; everything
//! outside the subset stays on the interpreter (per-function fallback).
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
//! shift. Because the compiled subset is side-effect-free, the caller can then
//! simply re-run the function on the interpreter, which is the single source of
//! semantic truth. So the native tier can only ever be *faster*, never different.

use cranelift_codegen::Context;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::{AbiParam, Block, InstBuilder, MemFlags, Value, types};
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{Linkage, Module, default_libcall_names};

/// Version of the [`JitInstr`]/[`JitFunction`] IR this crate consumes. The
/// producer (`rsscript`) translates its private bytecode into this stable,
/// versioned surface, so the two crates are decoupled: a breaking IR change bumps
/// this and the producer is updated in lock-step.
pub const IR_VERSION: u32 = 1;

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
}

/// A compilable function: the register count and the instruction stream. Callers
/// only invoke the compiled code when every argument is an `Int`, so all
/// registers are statically `i64`; the result is always an `Int`.
#[derive(Debug, Clone)]
pub struct JitFunction {
    pub n_params: u32,
    pub n_regs: u32,
    pub code: Vec<JitInstr>,
}

/// The native ABI of every compiled function:
/// `(args_ptr, n_args, out_ptr) -> completed`. Returns `1` and writes the result
/// to `*out` on success, or `0` (leaving `*out` untouched) to request fallback.
type CompiledAbi = unsafe extern "C" fn(*const i64, usize, *mut i64) -> u8;

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

/// Owns the JIT-compiled machine code. Compiled functions live as long as the
/// module, so callers keep this alive and invoke by [`CompiledId`].
pub struct NativeModule {
    module: JITModule,
    ctx: Context,
    fbctx: FunctionBuilderContext,
    funcs: Vec<CompiledAbi>,
    counter: u32,
}

/// Handle to a function compiled into a [`NativeModule`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompiledId(usize);

impl NativeModule {
    pub fn new() -> Result<Self, JitError> {
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
        let builder = JITBuilder::with_isa(isa, default_libcall_names());
        let module = JITModule::new(builder);
        let ctx = module.make_context();
        Ok(Self {
            module,
            ctx,
            fbctx: FunctionBuilderContext::new(),
            funcs: Vec::new(),
            counter: 0,
        })
    }

    /// Compile `function` to native code and return a handle to call it.
    pub fn compile(&mut self, function: &JitFunction) -> Result<CompiledId, JitError> {
        let ptr_ty = self.module.target_config().pointer_type();
        self.module.clear_context(&mut self.ctx);
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // args ptr
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // n_args
        self.ctx.func.signature.params.push(AbiParam::new(ptr_ty)); // out ptr
        self.ctx
            .func
            .signature
            .returns
            .push(AbiParam::new(types::I8));

        build_function(&mut self.ctx.func, &mut self.fbctx, function);

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
        let handle = CompiledId(self.funcs.len());
        self.funcs.push(f);
        Ok(handle)
    }

    /// Run a compiled function. Returns `Some(result)` on completion, or `None`
    /// when the native code bailed (an overflow/edge it leaves to the
    /// interpreter). This is the **safe** boundary: the only `unsafe` is the call
    /// through a pointer this module emitted with the matching ABI, and `args`
    /// is passed as a read-only slice.
    pub fn call(&self, id: CompiledId, args: &[i64]) -> Option<i64> {
        let f = self.funcs[id.0];
        let mut out: i64 = 0;
        // SAFETY: `f` was produced by `compile` with the `CompiledAbi` signature;
        // it reads `args.len()` i64s from `args.as_ptr()` and writes one i64 to
        // `&mut out`. The generated code never retains the pointers.
        let completed = unsafe { f(args.as_ptr(), args.len(), &mut out as *mut i64) };
        if completed != 0 { Some(out) } else { None }
    }
}

fn build_function(
    func: &mut cranelift_codegen::ir::Function,
    fbctx: &mut FunctionBuilderContext,
    program: &JitFunction,
) {
    let mut bcx = FunctionBuilder::new(func, fbctx);

    let n = program.code.len();
    let n_regs = program.n_regs as usize;

    // One Cranelift variable per VM register (all i64).
    let vars: Vec<Variable> = (0..n_regs).map(|_| bcx.declare_var(types::I64)).collect();

    // Entry: read params from the args array, zero the rest, then jump to the
    // block for instruction 0.
    let entry = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    let params = bcx.block_params(entry).to_vec();
    let args_ptr = params[0];
    let out_ptr = params[2];
    for i in 0..program.n_params as usize {
        let v = bcx
            .ins()
            .load(types::I64, MemFlags::trusted(), args_ptr, (i as i32) * 8);
        bcx.def_var(vars[i], v);
    }
    let zero = bcx.ins().iconst(types::I64, 0);
    for v in vars.iter().take(n_regs).skip(program.n_params as usize) {
        bcx.def_var(*v, zero);
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
            JitInstr::Return { .. } | JitInstr::Bail => {
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
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
                let (res, of) = bcx.ins().sadd_overflow(a, b);
                let cont = bail_if(&mut bcx, of, fallback);
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Sub { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let (res, of) = bcx.ins().ssub_overflow(a, b);
                let cont = bail_if(&mut bcx, of, fallback);
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Mul { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let (res, of) = bcx.ins().smul_overflow(a, b);
                let cont = bail_if(&mut bcx, of, fallback);
                bcx.switch_to_block(cont);
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Div { dst, lhs, rhs } => {
                let res = emit_checked_divrem(&mut bcx, reg(*lhs), reg(*rhs), fallback, false);
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Mod { dst, lhs, rhs } => {
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
                let c = bcx.ins().icmp(op.cc(), a, b);
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::Equal { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = bcx.ins().icmp(IntCC::Equal, a, b);
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::NotEqual { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = bcx.ins().icmp(IntCC::NotEqual, a, b);
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
                let c = bcx.ins().icmp(op.cc(), a, b);
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

    fn f(n_params: u32, n_regs: u32, code: Vec<JitInstr>) -> JitFunction {
        JitFunction {
            n_params,
            n_regs,
            code,
        }
    }

    #[test]
    fn compiles_and_runs_add() {
        let mut m = NativeModule::new().unwrap();
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
        let mut m = NativeModule::new().unwrap();
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
        let mut m = NativeModule::new().unwrap();
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
}
