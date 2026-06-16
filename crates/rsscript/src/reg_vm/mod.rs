use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;

use base64::Engine;
use chrono::{DateTime, Datelike, NaiveDate, SecondsFormat, TimeZone, Timelike, Utc};
use flate2::read::GzDecoder;
use hmac::{Hmac, Mac};
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use rand::Rng;
use sha2::{Digest, Sha256};
use sha3::{
    Sha3_224, Sha3_256, Shake128,
    digest::{ExtendableOutput, Update, XofReader},
};

use crate::diagnostic::Severity;
use crate::eval_types::{EvalError, EvalOutput, NativeInterpreterFn, NativeValue};
use crate::hir::{
    Hir, HirBlock, HirCallArg, HirCallReceiver, HirExpr, HirMatchArm, HirStmt, HirTypeKind,
    ParamEffect, TypeInfo,
};
use crate::interfaces::builtin_interfaces;
use crate::package::package_lowering_input;
use crate::syntax::ast::{
    BinaryOp, Callee, MatchFieldPattern, MatchLiteral, MatchPattern, merge_programs,
};
use crate::syntax::parse_source;
use crate::text_util::{
    decode_string_token, string_format, string_pad, string_slice_range, type_arg_names,
    type_root_name,
};
use crate::vm_value::{ValueMap, VmClosure, VmMapKey, VmNative, VmStruct, VmValue};

mod runtime_values;
mod value_access;
mod value_convert;
use runtime_values::*;
use value_access::*;
use value_convert::*;

const MS_PER_DAY: i64 = 86_400_000;

const URL_COMPONENT_SET: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub fn reg_vm_eval_source_main_with_args(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args(args)
}

pub fn reg_vm_eval_source_main(file: &str, source: &str) -> Result<EvalOutput, EvalError> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    reg_vm_eval_source_main_with_args(file, source, args)
}

/// Tier-0 JIT entry point.
///
/// Compiles the source, runs the per-function JIT-eligibility analysis (the seam
/// where native code generation will plug in), then executes through the shared
/// `run_frame` interpreter. Because execution reuses the interpreter's runtime,
/// the JIT and the interpreter share a single source of semantic truth — there
/// is no VM<->JIT gap *by construction* at this tier.
///
/// The next tier replaces the shared-execution step with native machine code for
/// `jit_plan().eligible_functions` (it must live in a separate crate because
/// `rsscript` is `#![forbid(unsafe_code)]`). That tier is gated by the N-way
/// differential in `tests/common/differential.rs`: `interp ≡ jit ≡ compiled`.
pub fn reg_vm_eval_source_main_jit(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_jit(args)
}

/// Native (Cranelift) JIT entry point. Like [`reg_vm_eval_source_main_jit`] but
/// the integer/control core executes as machine code (tier-0 covers the rest,
/// the interpreter the remainder). Verified to equal the other backends by the
/// N-way differential.
#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native(args)
}

/// Native-tier entry point in **deopt stress mode** (the native code always bails
/// to the interpreter). Used by the differential to verify the fallback path.
#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_force_deopt(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_force_deopt(args)
}

/// Per-program JIT eligibility: how many functions are fully covered by the
/// tier-0 JIT-supported instruction subset (and so are candidates for native
/// codegen) versus how many must fall back to the interpreter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitPlan {
    pub total_functions: usize,
    pub eligible_functions: usize,
    pub fallback_functions: usize,
}

/// Whether `code` contains a back-edge (a jump/branch whose target is at or
/// before it) — i.e. a loop. The tier-0 JIT only pays off across loop iterations,
/// so straight-line functions are left on the interpreter.
fn jit_function_has_loop(code: &[RegInstr]) -> bool {
    code.iter().enumerate().any(|(index, instr)| match instr {
        RegInstr::Jump { target } => *target <= index,
        RegInstr::JumpIfBool { target, .. } => *target <= index,
        RegInstr::JumpIfIntCompare { target, .. } => *target <= index,
        RegInstr::MatchOption {
            some_ip, none_ip, ..
        } => *some_ip <= index || *none_ip <= index,
        RegInstr::MatchResult { ok_ip, err_ip, .. } => *ok_ip <= index || *err_ip <= index,
        RegInstr::MatchVariant {
            match_ip, else_ip, ..
        } => *match_ip <= index || *else_ip <= index,
        RegInstr::MatchMapGet {
            some_ip, none_ip, ..
        } => *some_ip <= index || *none_ip <= index,
        _ => false,
    })
}

/// Per-function tier-0 JIT eligibility for a whole unit, returned indexed by
/// function id.
///
/// A function is eligible when both hold:
///  1. **Non-suspending.** Every instruction is in the pure subset
///     ([`jit_supported_instruction`]) or is a `CallKnown` to another eligible
///     (non-suspending) function. A least-fixpoint computes this. Such a function
///     — and its entire reachable call graph — contains no `await`/spawn/blocking
///     op, so the tier-0 executor can run callees to completion via `run_frame`
///     without ever needing to suspend.
///  2. **Non-recursive.** The executor runs callee frames on the host stack, so
///     JIT-ing a call cycle could overflow the host stack where the stackless
///     interpreter would not — a behavioural gap. We therefore drop any function
///     that can reach a cycle in the (non-suspending) call graph; recursive
///     functions keep running on the interpreter, which is gap-free by fallback.
fn compute_jit_eligibility(functions: &[RegFunction]) -> Vec<bool> {
    let n = functions.len();

    // (1) Non-suspending least-fixpoint: start optimistic, demote any function
    // with an unsupported instruction or a call to an already-demoted function,
    // until stable (monotone, so it terminates).
    let mut non_suspending = vec![true; n];
    loop {
        let mut changed = false;
        for index in 0..n {
            if !non_suspending[index] {
                continue;
            }
            let ok = functions[index].code.iter().all(|instr| match instr {
                RegInstr::CallKnown { function, .. } => *function < n && non_suspending[*function],
                other => jit_supported_instruction(other),
            });
            if !ok {
                non_suspending[index] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // (2) Call-graph edges among non-suspending functions only.
    let edges: Vec<Vec<usize>> = (0..n)
        .map(|index| {
            if !non_suspending[index] {
                return Vec::new();
            }
            let mut targets = Vec::new();
            for instr in &functions[index].code {
                if let RegInstr::CallKnown { function, .. } = instr
                    && non_suspending[*function]
                    && !targets.contains(function)
                {
                    targets.push(*function);
                }
            }
            targets
        })
        .collect();

    // Nodes reachable from `start` via >= 1 edge.
    let reachable = |start: usize| -> Vec<bool> {
        let mut seen = vec![false; n];
        let mut stack = edges[start].clone();
        while let Some(node) = stack.pop() {
            if seen[node] {
                continue;
            }
            seen[node] = true;
            for &next in &edges[node] {
                if !seen[next] {
                    stack.push(next);
                }
            }
        }
        seen
    };
    let reach: Vec<Vec<bool>> = (0..n).map(reachable).collect();
    // A node is on a cycle iff it can reach itself.
    let cyclic: Vec<bool> = (0..n).map(|node| reach[node][node]).collect();

    (0..n)
        .map(|index| {
            non_suspending[index]
                && !cyclic[index]
                && !(0..n).any(|other| cyclic[other] && reach[index][other])
        })
        .collect()
}

/// Whether an instruction is in the tier-0 JIT-supported subset (the numeric and
/// control-flow core that native codegen targets first). Heap construction,
/// calls, async, resources, and matches fall back to the interpreter.
fn jit_supported_instruction(instr: &RegInstr) -> bool {
    matches!(
        instr,
        RegInstr::LoadUnit { .. }
            | RegInstr::LoadInt { .. }
            | RegInstr::LoadFloat { .. }
            | RegInstr::LoadBool { .. }
            | RegInstr::LoadString { .. }
            | RegInstr::Move { .. }
            | RegInstr::DeepCopy { .. }
            | RegInstr::Manage { .. }
            | RegInstr::GetField { .. }
            | RegInstr::SetField { .. }
            | RegInstr::GetFieldSlot { .. }
            | RegInstr::SetFieldSlot { .. }
            | RegInstr::MakeStruct { .. }
            | RegInstr::MakeVariant { .. }
            | RegInstr::MakeList { .. }
            | RegInstr::MakeObject { .. }
            | RegInstr::MakeMap { .. }
            | RegInstr::MakeSome { .. }
            | RegInstr::LoadNone { .. }
            | RegInstr::MakeClosure { .. }
            | RegInstr::MatchOption { .. }
            | RegInstr::MatchResult { .. }
            | RegInstr::MatchVariant { .. }
            | RegInstr::MatchMapGet { .. }
            | RegInstr::UnwrapSome { .. }
            | RegInstr::UnwrapVariantValue { .. }
            | RegInstr::RuntimeError { .. }
            // Collection get/set/index ops (closure-free; closure-driven
            // map/filter/fold/sort still fall back to the interpreter).
            | RegInstr::ListGet { .. }
            | RegInstr::ListLen { .. }
            | RegInstr::ListPush { .. }
            | RegInstr::ListAppend { .. }
            | RegInstr::ListClear { .. }
            | RegInstr::ListPop { .. }
            | RegInstr::ListRemoveAt { .. }
            | RegInstr::ListSet { .. }
            | RegInstr::MapGet { .. }
            | RegInstr::MapClear { .. }
            | RegInstr::MapInsert { .. }
            | RegInstr::MapInsertOld { .. }
            | RegInstr::MapRemove { .. }
            | RegInstr::AddInt { .. }
            | RegInstr::SubInt { .. }
            | RegInstr::MulInt { .. }
            | RegInstr::DivInt { .. }
            | RegInstr::ModInt { .. }
            | RegInstr::BitAndInt { .. }
            | RegInstr::BitOrInt { .. }
            | RegInstr::BitXorInt { .. }
            | RegInstr::ShiftLeftInt { .. }
            | RegInstr::ShiftRightInt { .. }
            | RegInstr::LessInt { .. }
            | RegInstr::LessEqualInt { .. }
            | RegInstr::GreaterInt { .. }
            | RegInstr::GreaterEqualInt { .. }
            | RegInstr::Equal { .. }
            | RegInstr::NotEqual { .. }
            | RegInstr::Jump { .. }
            | RegInstr::JumpIfBool { .. }
            | RegInstr::JumpIfIntCompare { .. }
            | RegInstr::Return { .. }
    )
}

/// Static type of a register in the native-JIT subset: every register is an
/// unboxed `i64` holding either an `Int` or a `Bool` (`0`/`1`).
#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NativeTy {
    Int,
    Bool,
    Float,
    /// An opaque handle to a heap value (struct/list) passed as a parameter, used
    /// only as the base of a heap-read instruction. Stored as `i64` (a table index).
    Handle,
}

#[cfg(feature = "native-jit")]
impl NativeTy {
    fn jit_value_type(self) -> vm_jit::JitValueType {
        match self {
            // Booleans are stored as `i64` 0/1, like integers.
            NativeTy::Int | NativeTy::Bool => vm_jit::JitValueType::Int,
            NativeTy::Float => vm_jit::JitValueType::Float,
            NativeTy::Handle => vm_jit::JitValueType::Handle,
        }
    }
}

/// Whether an instruction is in the *native* JIT subset (integer/boolean/control
/// core, no heap/calls/async/floats). Tighter than [`jit_supported_instruction`].
#[cfg(feature = "native-jit")]
fn native_subset_instruction(instr: &RegInstr) -> bool {
    matches!(
        instr,
        RegInstr::LoadInt { .. }
            | RegInstr::LoadFloat { .. }
            | RegInstr::LoadBool { .. }
            | RegInstr::Move { .. }
            | RegInstr::DeepCopy { .. }
            | RegInstr::AddInt { .. }
            | RegInstr::SubInt { .. }
            | RegInstr::MulInt { .. }
            | RegInstr::DivInt { .. }
            | RegInstr::ModInt { .. }
            | RegInstr::BitAndInt { .. }
            | RegInstr::BitOrInt { .. }
            | RegInstr::BitXorInt { .. }
            | RegInstr::ShiftLeftInt { .. }
            | RegInstr::ShiftRightInt { .. }
            | RegInstr::LessInt { .. }
            | RegInstr::LessEqualInt { .. }
            | RegInstr::GreaterInt { .. }
            | RegInstr::GreaterEqualInt { .. }
            | RegInstr::Equal { .. }
            | RegInstr::NotEqual { .. }
            | RegInstr::Jump { .. }
            | RegInstr::JumpIfBool { .. }
            | RegInstr::JumpIfIntCompare { .. }
            | RegInstr::Return { .. }
            | RegInstr::RuntimeError { .. }
            // Heap reads via host-helper calls (base must be a handle parameter).
            | RegInstr::GetFieldSlot { .. }
            | RegInstr::ListLen { .. }
            | RegInstr::ListGet { .. }
    )
}

/// Assign type `t` to register `reg`; return `false` on a conflicting reassignment.
#[cfg(feature = "native-jit")]
fn native_set_ty(ty: &mut [Option<NativeTy>], reg: usize, t: NativeTy, changed: &mut bool) -> bool {
    match ty[reg] {
        Some(existing) => existing == t,
        None => {
            ty[reg] = Some(t);
            *changed = true;
            true
        }
    }
}

/// Unify two registers' types (they must end up equal). Propagates a known type
/// to an unknown one — this is how *parameter* types are inferred from the typed
/// operands they're combined with. `false` on a conflict.
#[cfg(feature = "native-jit")]
fn native_unify(ty: &mut [Option<NativeTy>], a: usize, b: usize, changed: &mut bool) -> bool {
    match (ty[a], ty[b]) {
        (Some(x), Some(y)) => x == y,
        (Some(x), None) => native_set_ty(ty, b, x, changed),
        (None, Some(y)) => native_set_ty(ty, a, y, changed),
        (None, None) => true,
    }
}

/// Mark which instructions are reachable from `ip == 0` along the control-flow
/// graph (sequential fallthrough, jumps, conditional branches). Used to ignore
/// the lowerer's unreachable defensive tail when judging native eligibility.
#[cfg(feature = "native-jit")]
fn native_reachable_instructions(code: &[RegInstr]) -> Vec<bool> {
    let n = code.len();
    let mut reachable = vec![false; n];
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        if i >= n || reachable[i] {
            continue;
        }
        reachable[i] = true;
        match &code[i] {
            RegInstr::Jump { target } => stack.push(*target),
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                stack.push(*target);
                stack.push(i + 1);
            }
            // Terminators with no fallthrough.
            RegInstr::Return { .. } | RegInstr::RuntimeError { .. } => {}
            // Everything else falls through to the next instruction. (Functions
            // containing non-subset control flow are rejected anyway.)
            _ => stack.push(i + 1),
        }
    }
    reachable
}

/// Clone a *pure* (branch-free, call-free) native-subset instruction with every
/// register shifted by `base` — used to splice a callee body into the caller's
/// register window during inlining. `None` for anything outside that pure subset.
#[cfg(feature = "native-jit")]
fn native_offset_regs(instr: &RegInstr, b: usize) -> Option<RegInstr> {
    Some(match instr {
        RegInstr::LoadInt { dst, value } => RegInstr::LoadInt {
            dst: dst + b,
            value: *value,
        },
        RegInstr::LoadFloat { dst, value } => RegInstr::LoadFloat {
            dst: dst + b,
            value: *value,
        },
        RegInstr::LoadBool { dst, value } => RegInstr::LoadBool {
            dst: dst + b,
            value: *value,
        },
        RegInstr::Move { dst, src } => RegInstr::Move {
            dst: dst + b,
            src: src + b,
        },
        RegInstr::DeepCopy { reg } => RegInstr::DeepCopy { reg: reg + b },
        RegInstr::AddInt { dst, lhs, rhs } => RegInstr::AddInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::SubInt { dst, lhs, rhs } => RegInstr::SubInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::MulInt { dst, lhs, rhs } => RegInstr::MulInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::DivInt { dst, lhs, rhs } => RegInstr::DivInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::ModInt { dst, lhs, rhs } => RegInstr::ModInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::BitAndInt { dst, lhs, rhs } => RegInstr::BitAndInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::BitOrInt { dst, lhs, rhs } => RegInstr::BitOrInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::BitXorInt { dst, lhs, rhs } => RegInstr::BitXorInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::ShiftLeftInt { dst, lhs, rhs } => RegInstr::ShiftLeftInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::ShiftRightInt { dst, lhs, rhs } => RegInstr::ShiftRightInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::LessInt { dst, lhs, rhs } => RegInstr::LessInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::LessEqualInt { dst, lhs, rhs } => RegInstr::LessEqualInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::GreaterInt { dst, lhs, rhs } => RegInstr::GreaterInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::GreaterEqualInt { dst, lhs, rhs } => RegInstr::GreaterEqualInt {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::Equal { dst, lhs, rhs } => RegInstr::Equal {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        RegInstr::NotEqual { dst, lhs, rhs } => RegInstr::NotEqual {
            dst: dst + b,
            lhs: lhs + b,
            rhs: rhs + b,
        },
        _ => return None,
    })
}

/// Whether `callee` can be inlined into a native function: captureless, arity
/// matches, and every reachable instruction is a pure native-subset op, native
/// control flow (jump/branch), or a `Return`. Unlike the original straight-line
/// restriction this permits internal branches and loops; calls, suspends,
/// matches, heap ops and runtime errors still make the caller fall back.
#[cfg(feature = "native-jit")]
fn native_callee_inlinable(callee: &RegFunction, n_args: usize) -> bool {
    if callee.captures != 0 || callee.params != n_args {
        return false;
    }
    let reachable = native_reachable_instructions(&callee.code);
    callee.code.iter().enumerate().all(|(i, instr)| {
        !reachable[i]
            || matches!(
                instr,
                RegInstr::Jump { .. }
                    | RegInstr::JumpIfBool { .. }
                    | RegInstr::JumpIfIntCompare { .. }
                    | RegInstr::Return { .. }
            )
            || native_offset_regs(instr, 0).is_some()
    })
}

/// Inline `CallKnown`s to [`native_callee_inlinable`] callees into `func`,
/// returning the rewritten code and new register count — this is what makes a
/// function that calls small helpers native-eligible (the calls vanish). Callees
/// may now contain internal branches/loops: each is spliced into a fresh register
/// window, its internal jump targets are remapped, and every `Return` becomes a
/// `Move` of the result into the call's destination plus a jump to the join point
/// just past the spliced block. `None` if any call target is not inlinable (the
/// function then falls back).
#[cfg(feature = "native-jit")]
fn native_inline_leaf_calls(unit: &RegUnit, func: &RegFunction) -> Option<(Vec<RegInstr>, usize)> {
    if !func
        .code
        .iter()
        .any(|instr| matches!(instr, RegInstr::CallKnown { .. }))
    {
        return Some((func.code.clone(), func.regs));
    }

    /// A jump target to be resolved once all positions are known.
    enum Fix {
        /// Target is a caller instruction index (use `index_map`).
        Caller(usize),
        /// Target is a callee instruction index within splice `id` (use its `cmap`).
        Callee { id: usize, callee_target: usize },
        /// A `Return` jump to the position just past splice `id`'s block.
        Join(usize),
    }
    struct Splice {
        cmap: Vec<usize>,
        join: usize,
    }

    let mut new_code: Vec<RegInstr> = Vec::new();
    let mut index_map = vec![0usize; func.code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    let mut splices: Vec<Splice> = Vec::new();
    let mut next_reg = func.regs;

    for (i, instr) in func.code.iter().enumerate() {
        index_map[i] = new_code.len();
        match instr {
            RegInstr::CallKnown {
                dst,
                function,
                args,
                mut_args,
            } => {
                let callee = unit.functions.get(*function)?;
                // Calls with `mut` args need a write-back at return; don't inline
                // them (native-inlinable callees are side-effect-free anyway).
                if !mut_args.is_empty() || !native_callee_inlinable(callee, args.len()) {
                    return None;
                }
                let base = next_reg;
                next_reg += callee.regs;
                for (param, arg) in args.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + param,
                        src: *arg,
                    });
                }
                let id = splices.len();
                let reachable = native_reachable_instructions(&callee.code);
                let mut cmap = vec![0usize; callee.code.len()];
                for (ci, cinstr) in callee.code.iter().enumerate() {
                    if !reachable[ci] {
                        continue;
                    }
                    cmap[ci] = new_code.len();
                    match cinstr {
                        RegInstr::Return { src } => {
                            new_code.push(RegInstr::Move {
                                dst: *dst,
                                src: base + src,
                            });
                            fixups.push((new_code.len(), Fix::Join(id)));
                            new_code.push(RegInstr::Jump { target: 0 });
                        }
                        RegInstr::Jump { target } => {
                            fixups.push((
                                new_code.len(),
                                Fix::Callee {
                                    id,
                                    callee_target: *target,
                                },
                            ));
                            new_code.push(RegInstr::Jump { target: 0 });
                        }
                        RegInstr::JumpIfBool {
                            cond,
                            expected,
                            target,
                        } => {
                            fixups.push((
                                new_code.len(),
                                Fix::Callee {
                                    id,
                                    callee_target: *target,
                                },
                            ));
                            new_code.push(RegInstr::JumpIfBool {
                                cond: cond + base,
                                expected: *expected,
                                target: 0,
                            });
                        }
                        RegInstr::JumpIfIntCompare {
                            lhs,
                            rhs,
                            op,
                            expected,
                            target,
                        } => {
                            fixups.push((
                                new_code.len(),
                                Fix::Callee {
                                    id,
                                    callee_target: *target,
                                },
                            ));
                            new_code.push(RegInstr::JumpIfIntCompare {
                                lhs: lhs + base,
                                rhs: rhs + base,
                                op: *op,
                                expected: *expected,
                                target: 0,
                            });
                        }
                        pure => new_code.push(native_offset_regs(pure, base)?),
                    }
                }
                let join = new_code.len();
                splices.push(Splice { cmap, join });
            }
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Caller(*target)));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }

    for (pos, fix) in fixups {
        let target = match fix {
            Fix::Caller(t) => index_map[t],
            Fix::Callee { id, callee_target } => splices[id].cmap[callee_target],
            Fix::Join(id) => splices[id].join,
        };
        match &mut new_code[pos] {
            RegInstr::Jump { target: t }
            | RegInstr::JumpIfBool { target: t, .. }
            | RegInstr::JumpIfIntCompare { target: t, .. } => *t = target,
            _ => {}
        }
    }
    Some((new_code, next_reg))
}

/// Translate a `RegFunction` into the native-JIT IR, or `None` if it is not in the
/// native subset (anything that isn't integer/boolean/control core, has captures,
/// or does not return an `Int`).
///
/// Callers invoke the compiled code only when **every argument is an `Int`**, so
/// all parameters are statically `i64`; type inference (a small fixpoint, to
/// handle loop back-edges) then proves every register is consistently `Int` or
/// `Bool`, every operand is well-typed, and the result is an `Int`.
#[cfg(feature = "native-jit")]
fn translate_to_native_jit(
    unit: &RegUnit,
    func: &RegFunction,
) -> Option<(vm_jit::JitFunction, NativeTy, Vec<NativeTy>)> {
    use vm_jit::{JitCompare, JitInstr};

    if func.captures != 0 {
        return None;
    }
    // Inline straight-line leaf calls first, so a function that only leaves the
    // native subset via small helper calls still qualifies (the calls vanish).
    let (code, n_regs) = native_inline_leaf_calls(unit, func)?;
    if func.params > n_regs {
        return None;
    }
    // Reachability from `ip == 0` over the control-flow graph. The lowerer appends
    // a defensive `LoadUnit; Return(unit)` to every function body even when the
    // body always returns earlier; that tail is unreachable. Restricting analysis
    // (and codegen) to reachable instructions lets such functions still qualify —
    // dead instructions become `Nop`.
    let reachable = native_reachable_instructions(&code);

    // Every *reachable* instruction must be in the native subset.
    for (i, instr) in code.iter().enumerate() {
        if reachable[i] && !native_subset_instruction(instr) {
            return None;
        }
    }

    // Type inference by unification (fixpoint, to handle loop back-edges).
    // Parameters start untyped and acquire their type from the operands they are
    // combined with — so a float-parameter function is inferred correctly rather
    // than forced to `Int`.
    let mut ty: Vec<Option<NativeTy>> = vec![None; n_regs];
    let mut changed = true;
    while changed {
        changed = false;
        for (i, instr) in code.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            let ty = &mut ty;
            let c = &mut changed;
            let ok = match instr {
                RegInstr::LoadInt { dst, .. } => native_set_ty(ty, *dst, NativeTy::Int, c),
                RegInstr::LoadFloat { dst, .. } => native_set_ty(ty, *dst, NativeTy::Float, c),
                RegInstr::LoadBool { dst, .. } => native_set_ty(ty, *dst, NativeTy::Bool, c),
                // Integer-only ops (`ModInt`/bitwise/shift; VM rejects them on
                // floats): all three operands are `Int`.
                RegInstr::ModInt { dst, lhs, rhs }
                | RegInstr::BitAndInt { dst, lhs, rhs }
                | RegInstr::BitOrInt { dst, lhs, rhs }
                | RegInstr::BitXorInt { dst, lhs, rhs }
                | RegInstr::ShiftLeftInt { dst, lhs, rhs }
                | RegInstr::ShiftRightInt { dst, lhs, rhs } => {
                    native_set_ty(ty, *dst, NativeTy::Int, c)
                        && native_set_ty(ty, *lhs, NativeTy::Int, c)
                        && native_set_ty(ty, *rhs, NativeTy::Int, c)
                }
                // Type-polymorphic arithmetic: `dst`, `lhs`, `rhs` share one
                // (numeric) type — unification flows it among them and to params.
                RegInstr::AddInt { dst, lhs, rhs }
                | RegInstr::SubInt { dst, lhs, rhs }
                | RegInstr::MulInt { dst, lhs, rhs }
                | RegInstr::DivInt { dst, lhs, rhs } => {
                    native_unify(ty, *lhs, *rhs, c) && native_unify(ty, *dst, *lhs, c)
                }
                RegInstr::LessInt { dst, lhs, rhs }
                | RegInstr::LessEqualInt { dst, lhs, rhs }
                | RegInstr::GreaterInt { dst, lhs, rhs }
                | RegInstr::GreaterEqualInt { dst, lhs, rhs }
                | RegInstr::Equal { dst, lhs, rhs }
                | RegInstr::NotEqual { dst, lhs, rhs } => {
                    native_unify(ty, *lhs, *rhs, c) && native_set_ty(ty, *dst, NativeTy::Bool, c)
                }
                RegInstr::Move { dst, src } => native_unify(ty, *dst, *src, c),
                RegInstr::JumpIfBool { cond, .. } => native_set_ty(ty, *cond, NativeTy::Bool, c),
                RegInstr::JumpIfIntCompare { lhs, rhs, .. } => native_unify(ty, *lhs, *rhs, c),
                // Heap reads: the base is a handle, the result an Int (the list
                // index is also an Int).
                RegInstr::GetFieldSlot { dst, base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListLen { dst, list } => {
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListGet { dst, list, index } => {
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *index, NativeTy::Int, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                _ => true,
            };
            if !ok {
                return None; // conflicting register types
            }
        }
    }

    let int = |reg: usize| ty[reg] == Some(NativeTy::Int);
    let bool_ty = |reg: usize| ty[reg] == Some(NativeTy::Bool);
    // Numeric = Int or Float; `same` = both operands typed and identical (so a
    // polymorphic op lowers consistently and native equality matches `VmValue`).
    let numeric = |reg: usize| matches!(ty[reg], Some(NativeTy::Int | NativeTy::Float));
    let same = |a: usize, b: usize| ty[a].is_some() && ty[a] == ty[b];
    // A handle register must be a *parameter*: handles only enter via the caller's
    // heap args (`try_native`), never produced by a native instruction.
    let handle_param = |reg: usize| ty[reg] == Some(NativeTy::Handle) && reg < func.params;
    let r = |reg: usize| reg as u32;
    let cmp = |op: &RegIntCompare| match op {
        RegIntCompare::Less => JitCompare::Lt,
        RegIntCompare::LessEqual => JitCompare::Le,
        RegIntCompare::Greater => JitCompare::Gt,
        RegIntCompare::GreaterEqual => JitCompare::Ge,
    };

    let mut jit_code = Vec::with_capacity(code.len());
    for (i, instr) in code.iter().enumerate() {
        if !reachable[i] {
            // Dead code (e.g. the lowerer's defensive trailing `Unit` return):
            // keep an index-aligned `Nop`, never executed.
            jit_code.push(JitInstr::Nop);
            continue;
        }
        let jit = match instr {
            RegInstr::LoadInt { dst, value } => JitInstr::LoadInt {
                dst: r(*dst),
                value: *value,
            },
            RegInstr::LoadFloat { dst, value } => JitInstr::LoadFloat {
                dst: r(*dst),
                value: *value,
            },
            RegInstr::LoadBool { dst, value } => JitInstr::LoadBool {
                dst: r(*dst),
                value: *value,
            },
            RegInstr::Move { dst, src } => {
                ty[*src]?; // src must be typed
                JitInstr::Move {
                    dst: r(*dst),
                    src: r(*src),
                }
            }
            RegInstr::DeepCopy { reg } => {
                ty[*reg]?; // copy of an int/bool register is a no-op
                JitInstr::Nop
            }
            RegInstr::AddInt { dst, lhs, rhs } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::Add {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::SubInt { dst, lhs, rhs } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::Sub {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::MulInt { dst, lhs, rhs } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::Mul {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::DivInt { dst, lhs, rhs } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::Div {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::ModInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::Mod {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::BitAndInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::BitAnd {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::BitOrInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::BitOr {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::BitXorInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::BitXor {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::ShiftLeftInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::Shl {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::ShiftRightInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::Shr {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::LessInt { dst, lhs, rhs }
            | RegInstr::LessEqualInt { dst, lhs, rhs }
            | RegInstr::GreaterInt { dst, lhs, rhs }
            | RegInstr::GreaterEqualInt { dst, lhs, rhs } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                let op = match instr {
                    RegInstr::LessInt { .. } => JitCompare::Lt,
                    RegInstr::LessEqualInt { .. } => JitCompare::Le,
                    RegInstr::GreaterInt { .. } => JitCompare::Gt,
                    _ => JitCompare::Ge,
                };
                JitInstr::Compare {
                    dst: r(*dst),
                    op,
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::Equal { dst, lhs, rhs } => {
                // Same statically-known type so native equality matches the
                // interpreter's `VmValue` equality (Int/Bool via icmp, Float via fcmp).
                require(same(*lhs, *rhs))?;
                JitInstr::Equal {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::NotEqual { dst, lhs, rhs } => {
                require(same(*lhs, *rhs))?;
                JitInstr::NotEqual {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::Jump { target } => JitInstr::Jump { target: r(*target) },
            RegInstr::JumpIfBool {
                cond,
                expected,
                target,
            } => {
                require(bool_ty(*cond))?;
                JitInstr::JumpIfBool {
                    cond: r(*cond),
                    expected: *expected,
                    target: r(*target),
                }
            }
            RegInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::JumpIfIntCompare {
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                    op: cmp(op),
                    expected: *expected,
                    target: r(*target),
                }
            }
            RegInstr::Return { src } => {
                // The native ABI returns 64 bits boxed by the caller as the
                // function's return type, which must be `Int` or `Float`.
                require(numeric(*src))?;
                JitInstr::Return { src: r(*src) }
            }
            RegInstr::RuntimeError { .. } => JitInstr::Bail,
            RegInstr::GetFieldSlot { dst, base, slot } => {
                require(handle_param(*base) && int(*dst))?;
                JitInstr::FieldInt {
                    dst: r(*dst),
                    base: r(*base),
                    slot: *slot as u32,
                }
            }
            RegInstr::ListLen { dst, list } => {
                require(handle_param(*list) && int(*dst))?;
                JitInstr::ListLen {
                    dst: r(*dst),
                    base: r(*list),
                }
            }
            RegInstr::ListGet { dst, list, index } => {
                require(handle_param(*list) && int(*index) && int(*dst))?;
                JitInstr::ListGetInt {
                    dst: r(*dst),
                    base: r(*list),
                    index: r(*index),
                }
            }
            // `native_subset_instruction` already rejected everything else.
            _ => return None,
        };
        jit_code.push(jit);
    }

    // Return type = the type of any reachable `Return`'s source (all consistent,
    // validated numeric above); defaults to `Int` for an empty body.
    let ret_type = code
        .iter()
        .enumerate()
        .find_map(|(i, instr)| match instr {
            RegInstr::Return { src } if reachable[i] => ty[*src],
            _ => None,
        })
        .unwrap_or(NativeTy::Int);

    let reg_types = (0..n_regs)
        .map(|reg| ty[reg].unwrap_or(NativeTy::Int).jit_value_type())
        .collect();

    // Parameter types (for the caller's argument unboxing); an unconstrained
    // parameter defaults to `Int` (and a mismatching argument then just falls back).
    let param_types: Vec<NativeTy> = (0..func.params)
        .map(|reg| ty[reg].unwrap_or(NativeTy::Int))
        .collect();

    let jit_fn = vm_jit::JitFunction {
        n_params: func.params as u32,
        n_regs: n_regs as u32,
        reg_types,
        code: jit_code,
    };
    Some((jit_fn, ret_type, param_types))
}

/// `Some(())` if the condition holds, else `None` — lets the translator use `?`
/// to bail out of a non-eligible function.
#[cfg(feature = "native-jit")]
fn require(condition: bool) -> Option<()> {
    condition.then_some(())
}

/// Outcome of executing a single "pure" instruction via the shared
/// [`RegVm::try_exec_pure`] dispatcher. Pure instructions push no frames, never
/// suspend, and never call other functions, so both the interpreter (`drive`)
/// and the tier-0 JIT executor (`run_jit`) share one copy of their semantics —
/// gap-freeness is then structural, not just differential-checked.
enum PureStep {
    /// Executed; advance to the next instruction (`ip` already updated for jumps).
    Next,
    /// A `Return` instruction; the caller decides how to unwind (the JIT returns
    /// the value directly, the interpreter pops the frame).
    Return(VmValue),
    /// Not in the pure subset; the caller must handle it (frames, calls, async…).
    NotPure,
}

pub fn reg_vm_eval_source_main_with_args_and_native_bindings(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?
        .eval_main_with_args_and_native_bindings(args, native_bindings)
}

/// Streaming-stdout source entry point for `rss dev --run`: evaluates `main` and
/// writes `Log.write` output live (line-flushed) to the real process stdout as it
/// runs. The captured stdout in the returned `EvalOutput` is unchanged, so it must
/// not be re-printed by the caller. Other callers and the tests keep using the
/// non-streaming `reg_vm_eval_source_main_with_args`, whose behavior is untouched.
pub fn reg_vm_eval_source_main_with_args_streaming_stdout(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?
        .eval_main_with_args_and_native_bindings_streaming_stdout(
            args,
            std::iter::empty::<(String, NativeInterpreterFn)>(),
        )
}

/// Streaming-stdout package entry point for `rss dev --run`. See
/// [`reg_vm_eval_source_main_with_args_streaming_stdout`].
pub fn reg_vm_eval_package_main_with_args_and_native_bindings_streaming_stdout(
    package_dir: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
    native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_package(package_dir)?
        .eval_main_with_args_and_native_bindings_streaming_stdout(args, native_bindings)
}

pub fn reg_vm_eval_package_main_with_args(
    package_dir: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_eval_package_main_with_args_and_native_bindings(
        package_dir,
        args,
        std::iter::empty::<(String, NativeInterpreterFn)>(),
    )
}

pub fn reg_vm_eval_package_main_with_args_and_native_bindings(
    package_dir: &Path,
    args: impl IntoIterator<Item = impl Into<String>>,
    native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_package(package_dir)?
        .eval_main_with_args_and_native_bindings(args, native_bindings)
}

/// Compile a multi-file package (its merged sources plus dependency and builtin
/// interfaces) into a reusable VM executable. Native functions are resolved at
/// run time via the `native_bindings` passed to the eval call, so this can be
/// compiled once and executed repeatedly (e.g. for benchmarking).
pub fn reg_vm_compile_package(package_dir: &Path) -> Result<RegVmExecutable, EvalError> {
    let input = package_lowering_input(package_dir).map_err(EvalError::Runtime)?;
    let mut interface_refs = builtin_interfaces()
        .map(|(path, contents)| (path.to_string(), contents.to_string()))
        .collect::<Vec<_>>();
    interface_refs.extend(input.interfaces.iter().cloned());
    let source_refs = input
        .sources
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let interface_refs_borrowed = interface_refs
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect::<Vec<_>>();
    let diagnostics =
        crate::analyze_sources_with_interfaces_without_core(&source_refs, &interface_refs_borrowed);
    let errors = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity.is_error())
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(EvalError::Diagnostics(errors));
    }

    let mut program = merge_programs(
        input
            .sources
            .iter()
            .map(|(path, source)| parse_source(path, source)),
    );
    crate::syntax::isolate_module_namespaces(&mut program);
    let interface_programs = interface_refs
        .iter()
        .map(|(path, source)| parse_source(path, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&program, &interface_programs);
    Ok(RegVmExecutable {
        unit: Rc::new(RegUnit::lower(&hir)?),
    })
}

#[derive(Debug, Clone)]
pub struct RegVmExecutable {
    unit: Rc<RegUnit>,
}

pub fn reg_vm_compile_source(file: &str, source: &str) -> Result<RegVmExecutable, EvalError> {
    let diagnostics = crate::analyze_source_with_core(file, source);
    let errors = diagnostics
        .into_iter()
        .filter(|diagnostic| diagnostic.severity == Severity::Error)
        .collect::<Vec<_>>();
    if !errors.is_empty() {
        return Err(EvalError::Diagnostics(errors));
    }

    let mut program = parse_source(file, source);
    crate::syntax::isolate_module_namespaces(&mut program);
    let hir = Hir::from_syntax_with_standard_package_interfaces(&program);
    Ok(RegVmExecutable {
        unit: Rc::new(RegUnit::lower(&hir)?),
    })
}

impl RegVmExecutable {
    /// Per-function JIT eligibility analysis (the tier-0 "compile" step). A
    /// function is eligible when it is non-suspending and non-recursive — every
    /// instruction in the JIT-supported subset or a `CallKnown` to another
    /// eligible function (see [`compute_jit_eligibility`]); otherwise it falls
    /// back to the interpreter.
    pub fn jit_plan(&self) -> JitPlan {
        let mut plan = JitPlan::default();
        for function in &self.unit.functions {
            plan.total_functions += 1;
            let eligible = function
                .jit_analysis
                .get()
                .map(|(eligible, _)| eligible)
                .unwrap_or_else(|| function.code.iter().all(jit_supported_instruction));
            if eligible {
                plan.eligible_functions += 1;
            } else {
                plan.fallback_functions += 1;
            }
        }
        plan
    }

    pub fn eval_main_with_args(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_native_bindings(
            args,
            std::iter::empty::<(String, NativeInterpreterFn)>(),
        )
    }

    /// Run `main` with the tier-0 JIT enabled: JIT-eligible functions execute via
    /// the specializing executor, the rest via the interpreter. Output is
    /// identical to `eval_main_with_args` (verified by the N-way differential).
    pub fn eval_main_with_args_jit(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        // The differential/parity callers want every supported function JIT'd so
        // the whole covered subset is verified, not just loop functions.
        self.eval_main_with_args_and_native_bindings_jit_inner(
            args,
            std::iter::empty::<(String, NativeInterpreterFn)>(),
            true,
        )
    }

    /// Run `main` with the native (Cranelift) JIT enabled: the integer/control
    /// core executes as machine code, tier-0 covers the rest of the supported
    /// subset, and the interpreter the remainder. Output is identical to
    /// `eval_main_with_args` (verified by the N-way differential). Compiles
    /// eligible functions on first call (threshold 0) so the differential
    /// exercises them.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            0,
            false,
            std::env::var_os("RSS_JIT_STATS").is_some(),
        )
        .map(|(output, _stats)| output)
    }

    /// Like [`Self::eval_main_with_args_native`] but also returns the native-tier
    /// [`NativeStats`] from the run (for benchmark/telemetry reporting).
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_with_stats(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner(args, 0, false, true)
    }

    /// Run `main` with the native tier in **deopt stress mode**: the native code
    /// always bails at its first guard, so every native-eligible function falls
    /// back to the interpreter. Its output must equal every other backend — this
    /// is how the deopt/fallback path is verified end-to-end.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_force_deopt(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            0,
            true,
            std::env::var_os("RSS_JIT_STATS").is_some(),
        )
        .map(|(output, _stats)| output)
    }

    #[cfg(feature = "native-jit")]
    fn eval_main_with_args_native_inner(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        let mut vm = RegVm::new(
            Rc::clone(&self.unit),
            args.into_iter().map(Into::into).collect(),
            std::iter::empty::<(String, NativeInterpreterFn)>().collect(),
        );
        // Native first, then tier-0, then interpreter.
        vm.native = Some(NativeState::new(
            tier_up_threshold,
            force_bail,
            collect_stats,
        )?);
        vm.jit_enabled = true;
        vm.jit_force_all = true;
        let value = vm.run_program("main")?;
        // Telemetry: `RSS_JIT_STATS=1` prints where native-tier attempts went, so
        // the next coverage win is measurable.
        if std::env::var_os("RSS_JIT_STATS").is_some()
            && let Some(native) = &vm.native
        {
            eprintln!("{}", native.stats.summary());
        }
        let stats = vm
            .native
            .as_ref()
            .map(|native| native.stats.clone())
            .unwrap_or_default();
        let display_value = value.display();
        let native_value = value.native_value();
        Ok((
            EvalOutput {
                value: display_value.clone(),
                display_value,
                native_value,
                stdout: vm.stdout,
                stderr: vm.stderr,
            },
            stats,
        ))
    }

    /// Like [`eval_main_with_args_jit`] but with native host bindings, using the
    /// production has-loop heuristic (only loop functions are JIT'd).
    pub fn eval_main_with_args_and_native_bindings_jit(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_and_native_bindings_jit_inner(args, native_bindings, false)
    }

    fn eval_main_with_args_and_native_bindings_jit_inner(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
        force_all: bool,
    ) -> Result<EvalOutput, EvalError> {
        let mut vm = RegVm::new(
            Rc::clone(&self.unit),
            args.into_iter().map(Into::into).collect(),
            native_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
        );
        vm.jit_enabled = true;
        vm.jit_force_all = force_all;
        let value = vm.run_program("main")?;
        let display_value = value.display();
        let native_value = value.native_value();
        Ok(EvalOutput {
            value: display_value.clone(),
            display_value,
            native_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
        })
    }

    /// Like [`Self::eval_main_with_args_and_native_bindings`] but streams program
    /// stdout (`Log.write` output) live to the real process stdout, line-flushed,
    /// as the program runs. Used ONLY by `rss dev --run` so a slow/looping program
    /// shows output immediately instead of buffering until exit. The returned
    /// `EvalOutput.stdout` is still the full captured buffer (identical to the
    /// non-streaming call), so the program output has already been written to the
    /// terminal — the caller must NOT print it a second time.
    pub fn eval_main_with_args_and_native_bindings_streaming_stdout(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
    ) -> Result<EvalOutput, EvalError> {
        let mut vm = RegVm::new(
            Rc::clone(&self.unit),
            args.into_iter().map(Into::into).collect(),
            native_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
        );
        vm.stream_stdout = true;
        let result = vm.run_program("main");
        // Flush any final line that lacks a trailing newline so no output is lost.
        if vm.stream_flushed < vm.stdout.len() {
            let mut out = std::io::stdout();
            let _ = out.write_all(&vm.stdout.as_bytes()[vm.stream_flushed..]);
            let _ = out.flush();
            vm.stream_flushed = vm.stdout.len();
        }
        let value = result?;
        let display_value = value.display();
        let native_value = value.native_value();
        Ok(EvalOutput {
            value: display_value.clone(),
            display_value,
            native_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
        })
    }

    pub fn eval_main_with_args_and_native_bindings(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        native_bindings: impl IntoIterator<Item = (impl Into<String>, NativeInterpreterFn)>,
    ) -> Result<EvalOutput, EvalError> {
        let mut vm = RegVm::new(
            Rc::clone(&self.unit),
            args.into_iter().map(Into::into).collect(),
            native_bindings
                .into_iter()
                .map(|(key, function)| (key.into(), function))
                .collect(),
        );
        let value = vm.run_program("main")?;
        let display_value = value.display();
        let native_value = value.native_value();
        Ok(EvalOutput {
            value: display_value.clone(),
            display_value,
            native_value,
            stdout: vm.stdout,
            stderr: vm.stderr,
        })
    }
}

#[derive(Debug, Clone)]
struct RegUnit {
    functions: Vec<Rc<RegFunction>>,
    function_ids: HashMap<String, usize>,
    resource_drop_functions: HashMap<String, usize>,
    types: HashMap<String, TypeInfo>,
}

/// `RegFunction::native_status` value: the function is known not native-eligible.
#[cfg(feature = "native-jit")]
const NATIVE_STATUS_NOT_ELIGIBLE: u8 = 1;

#[derive(Debug, Clone)]
struct RegFunction {
    // `params`/`captures` are metadata read only by the native JIT (translation);
    // `name` is retained as diagnostic/debug metadata.
    #[allow(dead_code)]
    name: String,
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    params: usize,
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    captures: usize,
    regs: usize,
    local_regs: HashMap<String, Reg>,
    code: Vec<RegInstr>,
    /// Cached tier-0 JIT analysis `(all_instructions_supported, has_loop)`,
    /// computed once after `code` is emitted.
    jit_analysis: std::cell::Cell<Option<(bool, bool)>>,
    /// Cached native-tier verdict, an invariant property of the function:
    /// `0` unknown, `1` known not native-eligible. Lets `try_native` skip all
    /// per-call tiering/cache/name-hash work once a function is known to never
    /// compile (so `jit-native` isn't slower than the VM on uncompilable code).
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    native_status: std::cell::Cell<u8>,
}

impl RegFunction {
    fn placeholder(name: String) -> Self {
        Self {
            name,
            params: 0,
            captures: 0,
            regs: 0,
            local_regs: HashMap::new(),
            code: Vec::new(),
            jit_analysis: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
        }
    }
}

#[derive(Debug, Clone)]
enum RegInstr {
    LoadUnit {
        dst: Reg,
    },
    LoadInt {
        dst: Reg,
        value: i64,
    },
    LoadFloat {
        dst: Reg,
        value: f64,
    },
    LoadBool {
        dst: Reg,
        value: bool,
    },
    LoadString {
        dst: Reg,
        value: Rc<String>,
    },
    Move {
        dst: Reg,
        src: Reg,
    },
    /// Replace `reg` with a deep copy of its value (fresh `Rc` for every mutable
    /// container in the tree, recursing through structs/variants/options; shared
    /// reference values like `Managed` keep their handle). Emitted at the function
    /// prologue for every non-`mut` parameter so the callee owns an isolated copy,
    /// mirroring the Rust backend, which passes `mut` as `&mut` (mutations
    /// propagate) and everything else by value/`&` + an inserted `.clone()`.
    DeepCopy {
        reg: Reg,
    },
    Manage {
        dst: Reg,
        src: Reg,
    },
    GetField {
        dst: Reg,
        base: Reg,
        name: String,
    },
    /// Read a struct/variant field by precomputed slot (the lowerer resolved the
    /// declaration-order index from the static type) — no name lookup at runtime.
    GetFieldSlot {
        dst: Reg,
        base: Reg,
        slot: usize,
    },
    /// Slot-indexed counterpart of `SetField` (copy-on-write by slot).
    SetFieldSlot {
        dst: Reg,
        base: Reg,
        slot: usize,
        value: Reg,
    },
    /// Produce a copy of the struct in `base` with field `name` set to `value`.
    /// Structs are value types, so this rebuilds the struct rather than mutating
    /// in place; nested assignment targets compose these writes back up the path.
    SetField {
        dst: Reg,
        base: Reg,
        name: String,
        value: Reg,
    },
    MakeStruct {
        dst: Reg,
        name: String,
        fields: Vec<(String, Reg)>,
    },
    ResourceDrop {
        resource: Reg,
    },
    MakeVariant {
        dst: Reg,
        name: String,
        fields: Vec<(String, Reg)>,
    },
    MakeList {
        dst: Reg,
        items: Vec<Reg>,
    },
    MakeObject {
        dst: Reg,
        fields: Vec<(String, Reg)>,
    },
    MakeMap {
        dst: Reg,
        entries: Vec<(Reg, Reg)>,
    },
    AddInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    SubInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    MulInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    DivInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    ModInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BitAndInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BitOrInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BitXorInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    ShiftLeftInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    ShiftRightInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    LessInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    LessEqualInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    GreaterInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    GreaterEqualInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Equal {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    NotEqual {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Jump {
        target: usize,
    },
    JumpIfBool {
        cond: Reg,
        expected: bool,
        target: usize,
    },
    JumpIfIntCompare {
        lhs: Reg,
        rhs: Reg,
        op: RegIntCompare,
        expected: bool,
        target: usize,
    },
    MatchOption {
        src: Reg,
        some_ip: usize,
        none_ip: usize,
    },
    MatchResult {
        src: Reg,
        ok_ip: usize,
        err_ip: usize,
    },
    MatchVariant {
        src: Reg,
        expected: String,
        match_ip: usize,
        else_ip: usize,
    },
    RuntimeError {
        message: String,
    },
    MatchMapGet {
        map: Reg,
        key: Reg,
        value_dst: Reg,
        some_ip: usize,
        none_ip: usize,
    },
    UnwrapSome {
        dst: Reg,
        src: Reg,
    },
    UnwrapVariantValue {
        dst: Reg,
        src: Reg,
        expected: String,
    },
    MakeClosure {
        dst: Reg,
        function: usize,
        captures: Vec<Reg>,
    },
    MakeSome {
        dst: Reg,
        value: Reg,
    },
    LoadNone {
        dst: Reg,
    },
    CallKnown {
        dst: Reg,
        function: usize,
        args: Vec<Reg>,
        /// Argument positions passed with `mut` (the callee's `mut` params). After
        /// the call returns, each such argument's (possibly mutated) value is
        /// written back to the caller's argument register, so a `mut` parameter's
        /// field/element mutations propagate to the caller — matching AOT's
        /// `&mut` semantics.
        mut_args: Vec<usize>,
    },
    /// Dynamic protocol dispatch: a `Protocol.method(self: x, ...)` call whose
    /// concrete impl is chosen at runtime by `args[0]`'s struct type name. This is
    /// how capability objects and generic protocol bounds dispatch in the VM,
    /// mirroring the compiled backend's closed-world enum dispatch. `dispatch`
    /// maps each implementing struct name to the impl's target function id.
    CallDynamic {
        dst: Reg,
        dispatch: Vec<(String, usize)>,
        args: Vec<Reg>,
        mut_args: Vec<usize>,
    },
    /// `spawn f(args)` / `async let`: start `function` as a new concurrent task
    /// and put a Task handle in `dst` (the spawning task keeps running).
    SpawnTask {
        dst: Reg,
        function: usize,
        args: Vec<Reg>,
    },
    /// `await x`: if `src` is a Task handle, join it (park until it finishes and
    /// receive its value); otherwise it is an already-evaluated async result and
    /// this is the identity move.
    AwaitJoin {
        dst: Reg,
        src: Reg,
    },
    /// `select { ... }`: each `handles` reg holds a spawned arm task. Park until
    /// the first arm finishes, then write its index to `winner` and its value to
    /// `value`; a branch ladder afterwards dispatches to the winning arm's body.
    SelectWait {
        handles: Vec<Reg>,
        winner: Reg,
        value: Reg,
    },
    CallNative {
        dst: Reg,
        key: String,
        args: Vec<Reg>,
        /// Positions within `args` whose corresponding parameter is `mut`. After
        /// the call the host writes the mutated value back to those arg
        /// registers, so native in-place mutation propagates to the caller.
        mut_args: Vec<usize>,
    },
    #[allow(dead_code)]
    CallClosure {
        dst: Reg,
        closure: Reg,
        args: Vec<Reg>,
    },
    ListFilter {
        dst: Reg,
        list: Reg,
        predicate: Reg,
    },
    ListFold {
        dst: Reg,
        list: Reg,
        state: Reg,
        folder: Reg,
    },
    ListGet {
        dst: Reg,
        list: Reg,
        index: Reg,
    },
    ListLen {
        dst: Reg,
        list: Reg,
    },
    ListMap {
        dst: Reg,
        list: Reg,
        mapper: Reg,
    },
    ListAppend {
        dst: Reg,
        list: Reg,
        values: Reg,
    },
    ListClear {
        dst: Reg,
        list: Reg,
    },
    ListPop {
        dst: Reg,
        list: Reg,
    },
    ListPush {
        dst: Reg,
        list: Reg,
        value: Reg,
    },
    ListRemoveAt {
        dst: Reg,
        list: Reg,
        index: Reg,
    },
    ListSet {
        dst: Reg,
        list: Reg,
        index: Reg,
        value: Reg,
    },
    ListSort {
        dst: Reg,
        list: Reg,
    },
    ListSortBy {
        dst: Reg,
        list: Reg,
        key: Reg,
        compare: Reg,
    },
    ListSortWith {
        dst: Reg,
        list: Reg,
        compare: Reg,
    },
    DequeClear {
        dst: Reg,
        deque: Reg,
    },
    DequePopBack {
        dst: Reg,
        deque: Reg,
    },
    DequePopFront {
        dst: Reg,
        deque: Reg,
    },
    DequePushBack {
        dst: Reg,
        deque: Reg,
        value: Reg,
    },
    DequePushFront {
        dst: Reg,
        deque: Reg,
        value: Reg,
    },
    SetClear {
        dst: Reg,
        set: Reg,
    },
    SetForEach {
        dst: Reg,
        set: Reg,
        callback: Reg,
    },
    SetInsert {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SetRemove {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SortedSetClear {
        dst: Reg,
        set: Reg,
    },
    SortedSetInsert {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SortedSetRemove {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SortedMapClear {
        dst: Reg,
        map: Reg,
    },
    SortedMapInsert {
        dst: Reg,
        map: Reg,
        key: Reg,
        value: Reg,
    },
    SortedMapRemove {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    MapGet {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    MapClear {
        dst: Reg,
        map: Reg,
    },
    MapInsertOld {
        dst: Reg,
        map: Reg,
        key: Reg,
        value: Reg,
    },
    MapRemove {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    BufferClear {
        dst: Reg,
        buffer: Reg,
    },
    CounterAdd {
        dst: Reg,
        counter: Reg,
        amount: Reg,
    },
    ConfigStoreReplace {
        dst: Reg,
        store: Reg,
        value: Reg,
    },
    GlobalConfigReplace {
        dst: Reg,
        global: Reg,
        value: Reg,
    },
    MapInsert {
        dst: Reg,
        map: Reg,
        key: Reg,
        value: Reg,
    },
    StringBuilderPush {
        dst: Reg,
        builder: Reg,
        value: Reg,
    },
    StringConcat {
        dst: Reg,
        left: Reg,
        right: Reg,
    },
    CallIntrinsic {
        dst: Reg,
        intrinsic: RegIntrinsic,
        args: Vec<Reg>,
    },
    CallTypedIntrinsic {
        dst: Reg,
        intrinsic: RegIntrinsic,
        type_arg: String,
        args: Vec<Reg>,
    },
    TryResult {
        dst: Reg,
        src: Reg,
        cleanup: Vec<Reg>,
    },
    Return {
        src: Reg,
    },
}

#[derive(Debug, Clone, Copy)]
enum RegIntrinsic {
    ArgsAll,
    ArgsCount,
    ArgsGet,
    ArgsGetOrDefault,
    AssertEqual,
    AssertEqualBool,
    AssertEqualInt,
    Base64Decode,
    Base64DecodeString,
    Base64Encode,
    Base64EncodeBytes,
    BytesConcat,
    BytesConsume,
    BytesFromString,
    BytesFromUints,
    BytesIsEmpty,
    BytesLen,
    BytesSlice,
    BytesToString,
    BytesToUints,
    BytesViewStartsWith,
    BytesViewToBytes,
    BufferNew,
    CacheGet,
    CacheLookup,
    CancellationSourceCancel,
    CancellationSourceNew,
    CancellationSourceToken,
    CancellationTokenIsCancelled,
    ChannelBounded,
    ChannelReceiver,
    ChannelSender,
    ChannelErrorMessage,
    TensorFromF32Slice,
    TensorToF32Slice,
    TensorShape,
    TensorRank,
    TensorMatmul,
    TensorAdd,
    TensorSub,
    TensorMul,
    TensorDiv,
    TensorNeg,
    TensorExp,
    TensorLog,
    TensorSqrt,
    TensorRelu,
    TensorSumAll,
    TensorSumAxis,
    TensorMaxAxis,
    TensorMeanAxis,
    TensorArgmaxAxis,
    TensorReshape,
    TensorTranspose,
    TensorPermute,
    TensorBroadcastTo,
    TensorCmplt,
    TensorCmpne,
    TensorCmpeq,
    TensorSelect,
    TensorMaximum,
    TensorMinimum,
    TensorCastF32,
    TensorCastI32,
    TensorCastBool,
    TensorDtypeCode,
    // movement+gather (ops B)
    TensorPad,
    TensorShrink,
    TensorFlip,
    TensorGather,
    // reductions+math (ops C)
    TensorProdAxis,
    TensorMinAxis,
    TensorSumAxes,
    TensorProdAxes,
    TensorMaxAxes,
    TensorMinAxes,
    TensorMeanAxes,
    TensorReciprocal,
    TensorExp2,
    TensorLog2,
    TensorRsqrt,
    TensorPow,
    // bmm+int/bit (ops D)
    TensorBmm,
    TensorIdiv,
    TensorMod,
    TensorShl,
    TensorShr,
    TensorAnd,
    TensorOr,
    TensorXor,
    TensorBitcastF32ToI32,
    TensorBitcastI32ToF32,
    // nn (slice F)
    TensorIota,
    TensorOneHot,
    TensorSoftmax,
    TensorLogSoftmax,
    TensorCrossEntropy,
    TensorErrorMessage,
    CharCompare,
    CharFromCode,
    CharIsAlphanumeric,
    CharIsAlpha,
    CharIsDigit,
    CharIsLower,
    CharIsUpper,
    CharIsWhitespace,
    CharToCode,
    CharToLower,
    CharToString,
    CharToUpper,
    CloneClone,
    ClockNow,
    ClockSystemUnixMs,
    ConfigLoad,
    CapabilityFrom,
    ConfigName,
    ConfigNew,
    ConfigRuleCount,
    ConfigStoreName,
    ConfigStoreNew,
    CounterNew,
    CounterValue,
    CsvOpenRead,
    CsvParseRow,
    CsvReadInto,
    CsvRows,
    DateAddDays,
    DateAddMs,
    DateDay,
    DateDaysBetween,
    DateDaysInMonth,
    DateFormatIso,
    DateFormatYmd,
    DateHour,
    DateIsLeapYear,
    DateMinute,
    DateMonth,
    DateParseIso,
    DateParseYmd,
    DateSecond,
    DateStartOfDay,
    DateWeekday,
    DateYear,
    DecodeErrorMessage,
    DeadlineAfter,
    DeadlineAfterMs,
    DeadlineIsExpired,
    DeadlineRemainingMs,
    DequeIsEmpty,
    DequeLen,
    DequeNew,
    DequeToList,
    DiffUnified,
    DirectoryCopyFile,
    DirectoryCreate,
    DirectoryCreateAll,
    DirectoryCreateDirAll,
    DirectoryExists,
    DirectoryIsDir,
    DirectoryIsFile,
    DirectoryListFiles,
    DirectoryListPaths,
    DirectoryMetadata,
    DirectoryReadString,
    DirectoryRemoveDirAll,
    DirectoryRemoveFile,
    DirectoryRename,
    DirectoryWriteString,
    DbClose,
    DbConnectionOpen,
    DbConnectionQuery,
    DbConnectionTryOpen,
    DurationAdd,
    DurationAsMs,
    DurationAsSeconds,
    DurationMs,
    DurationSeconds,
    EnvironmentBindFunction,
    EnvironmentChild,
    EnvironmentHasFunction,
    EnvironmentHasParent,
    EnvironmentRoot,
    EnvCurrentDir,
    EnvGet,
    EnvGetOrDefault,
    EnvHomeDir,
    EnvRunWorkspaceRoot,
    EnvSet,
    EnvSetCurrentDir,
    EnvTempDir,
    FileAppendBytes,
    FileAppendString,
    FileBytesStream,
    FileExists,
    FileErrorMessage,
    FileOpen,
    FileOpenRead,
    FileOpenWrite,
    FileReadAll,
    FileReadAllAsync,
    FileReadAllString,
    FileReadAllStringAsync,
    FileReadBytes,
    FileReadInto,
    FileReadString,
    FileRemove,
    FileWrite,
    FileWriteAsync,
    FileWriteAtomic,
    FileWriteBytes,
    FileWriteBytesView,
    FileWriteBuffer,
    FileWriteBufferView,
    FileWriteString,
    FileWriteStringAsync,
    FileWriteStringToPath,
    FalliblePipelineCollect,
    FalliblePipelineEach,
    FalliblePipelineFilter,
    FalliblePipelineMap,
    FalliblePipelineTryMap,
    FunctionObjectHasClosure,
    FunctionObjectNew,
    HashSha256Bytes,
    HashSha256File,
    HashSha256String,
    HashSha3_224Bytes,
    HashSha3_256Bytes,
    HashShake128Bytes,
    HmacSha256Bytes,
    HmacSha256String,
    GlobalConfigNew,
    GlobalConfigRuleCount,
    GzipDecompressBytes,
    HexDecode,
    HexEncode,
    HexEncodeString,
    HttpErrorMessage,
    HttpGet,
    HttpGetAsync,
    HttpGetRetryAsync,
    HttpGetTimeoutAsync,
    HttpPostForm,
    HttpPostFormAsync,
    HttpPostJson,
    HttpPostJsonAsync,
    HttpPostJsonBearerRetryAsync,
    HttpPostJsonRetryAsync,
    HttpPostJsonTimeoutAsync,
    HttpSendAsync,
    HttpRequestJson,
    HttpRequestWithHeader,
    HttpRequestWithRetry,
    HttpRequestWithTimeout,
    HttpResponseBytes,
    HttpResponseIsSuccess,
    HttpResponseLines,
    HttpResponseStatus,
    HttpResponseText,
    ImageInspect,
    ImageLoad,
    ImageNormalize,
    ImageResize,
    ImageSave,
    ImageSharpen,
    InstantElapsed,
    FloatToString,
    FloatIsFinite,
    FloatIsInfinite,
    FloatIsNan,
    IntToString,
    IntToFloat,
    IntBitAnd,
    IntBitNot,
    IntBitOr,
    IntBitXor,
    IntShiftLeft,
    IntShiftRight,
    MathAbs,
    MathAbsFloat,
    MathCeil,
    MathClamp,
    MathClampFloat,
    MathCos,
    MathExp,
    MathExp2,
    MathFloor,
    MathLog,
    MathLog2,
    MathMax,
    MathMaxFloat,
    MathMin,
    MathMinFloat,
    MathPow,
    MathPowFloat,
    MathRound,
    MathSin,
    MathSqrt,
    MathTanh,
    MathTruncFloat,
    JsonArray,
    JsonArrayBools,
    JsonArrayContainsPrefix,
    JsonArrayContainsString,
    JsonArrayContainsSubstring,
    JsonArrayCountWhere,
    JsonArrayFold,
    JsonArrayGet,
    JsonArrayInts,
    JsonArrayLen,
    JsonArrayStrings,
    JsonAt,
    JsonAtBool,
    JsonAtBoolOr,
    JsonAtInt,
    JsonAtIntOr,
    JsonAtOptional,
    JsonAtOptionalBool,
    JsonAtOptionalInt,
    JsonAtOptionalString,
    JsonAtOr,
    JsonAtString,
    JsonAtStringOr,
    JsonAtToString,
    JsonAtToStringOr,
    JsonAsBool,
    JsonAsInt,
    JsonAsString,
    JsonBoolAt,
    JsonBoolAtOr,
    JsonBoolField,
    JsonClone,
    JsonDecode,
    JsonDecodeText,
    JsonEncode,
    JsonErrorMessage,
    JsonField,
    JsonFieldBool,
    JsonFieldInt,
    JsonFieldOptional,
    JsonFieldOptionalBool,
    JsonFieldOptionalInt,
    JsonFieldOptionalString,
    JsonFieldString,
    JsonIntAt,
    JsonIntAtOr,
    JsonIntField,
    JsonIsArray,
    JsonIsNull,
    JsonIsObject,
    JsonKind,
    JsonObject,
    JsonObjectKeys,
    JsonObjectLen,
    JsonParse,
    JsonParseFile,
    JsonQuoteString,
    JsonRawField,
    JsonStringAt,
    JsonStringAtOr,
    JsonStringArray,
    JsonStringField,
    JsonStrings,
    JsonToStringAt,
    JsonToStringAtOr,
    JsonToString,
    JsonValue,
    JsonValues,
    ListAll,
    ListAny,
    ListConsume,
    ListContains,
    ListContainsValue,
    ListCountWhere,
    ListEnumerate,
    ListFind,
    ListFlatMap,
    ListFlatten,
    ListFirst,
    ListGroupBy,
    ListIsEmpty,
    ListJoin,
    ListLast,
    ListDedup,
    ListMax,
    ListMin,
    ListNew,
    ListPartition,
    ListPipeline,
    ListReverse,
    ListSkip,
    ListSlice,
    ListSum,
    ListTake,
    ListZip,
    ListToJsonStrings,
    ListToJsonValues,
    ListTryFold,
    LogError,
    LogErrorJson,
    LogTrace,
    LogWrite,
    LogWriteJson,
    MapContainsKey,
    MapFilter,
    MapFold,
    MapForEach,
    MapGetOrDefault,
    MapIsEmpty,
    MapKeys,
    MapLen,
    MapMapValues,
    MapMerge,
    MapNew,
    MapTryFold,
    MapValues,
    OptionIsNone,
    OptionIsSome,
    OptionAndThen,
    OptionFilter,
    OptionMap,
    OptionOkOr,
    OptionOr,
    OptionUnwrapOr,
    OptionUnwrapOrElse,
    OrdCompare,
    OsClose,
    PatchApplyText,
    PathExists,
    PathExtension,
    PathFileName,
    PathFromString,
    PathIsAbsolute,
    PathIsDir,
    PathIsFile,
    PathJoin,
    PathListFiles,
    PathListPaths,
    PathNormalize,
    PathParent,
    PathReadString,
    PathResolveRelative,
    PathSafeRelative,
    PathStartsWith,
    PathToString,
    PathWithExtension,
    PathWriteString,
    PersistentMapClear,
    PersistentMapContainsKey,
    PersistentMapGet,
    PersistentMapInsert,
    PersistentMapIsEmpty,
    PersistentMapLen,
    PersistentMapNew,
    PersistentMapRemove,
    PipelineCollect,
    PipelineEach,
    PoolErrorMessage,
    PoolStatsAvailable,
    PoolStatsCapacity,
    PoolStatsCreated,
    PoolStatsInUse,
    PipelineTryMap,
    ProcessRun,
    ProcessRunAsync,
    ProcessRunManyStdout,
    ProcessRunManyStdoutAsync,
    ProcessRunManyStdoutTimeout,
    ProcessRunManyStdoutTimeoutAsync,
    ProcessRunRequest,
    ProcessRunRequestAsync,
    ProcessRunRequestCancellableAsync,
    ProcessRunStdout,
    ProcessRunStdoutAsync,
    ProcessRunStdoutTimeout,
    ProcessRunStdoutTimeoutAsync,
    ProcessRunTimeout,
    ProcessRunTimeoutAsync,
    ProcessStream,
    RandomBool,
    RandomBytes,
    RandomFloat,
    RandomInt,
    RandomString,
    RegexCaptures,
    RegexCompile,
    RegexErrorMessage,
    RegexFind,
    RegexIsMatch,
    RegexReplaceAll,
    RegexSplit,
    ResultErr,
    ResultErrMessage,
    ResultAndThen,
    ResultIsErr,
    ResultIsOk,
    ResultMap,
    ResultMapError,
    ResultOk,
    ResultUnwrapOr,
    ResultUnwrapOrElse,
    RequestNew,
    RequestPath,
    ReceiverClose,
    ReceiverIntoStream,
    ReceiverRecv,
    ReceiverRecvCancellable,
    ResponseBody,
    ResponseOk,
    ResponseStatus,
    RowBufferNew,
    RowFieldString,
    RuleLoaderLoadRules,
    ResourcePoolBorrow,
    ResourcePoolDiscard,
    ResourcePoolLazy,
    ResourcePoolNew,
    ResourcePoolStats,
    ResourcePoolTryBorrow,
    ResourcePoolTryLazy,
    ResourcePoolTryNew,
    SetContains,
    SetDifference,
    SetIntersection,
    SetIsEmpty,
    SetIsSubset,
    SetLen,
    SetNew,
    SetToList,
    SetUnion,
    SortedSetContains,
    SortedSetIsEmpty,
    SortedSetLen,
    SortedSetNew,
    SortedSetToList,
    SortedMapContainsKey,
    SortedMapGet,
    SortedMapIsEmpty,
    SortedMapKeys,
    SortedMapLen,
    SortedMapNew,
    SortedMapValues,
    StringAfter,
    StringBefore,
    StringBuilderNew,
    StringCharAt,
    StringChars,
    StringContains,
    StringCount,
    StringCopy,
    StringEndsWith,
    StringFromBool,
    StringFromFloat,
    StringFormat,
    StringIndexOf,
    StringFromInt,
    StringIsEmpty,
    StringJoin,
    StringLines,
    StringLen,
    StringPadLeft,
    StringPadRight,
    StringParseFloat,
    StringParseInt,
    StringRepeat,
    StringReplace,
    StringReplaceFirst,
    StringReverse,
    StringSlice,
    StringSplit,
    StringStartsWith,
    StringStripPrefix,
    StringToLowercase,
    StringToUppercase,
    StringTrim,
    StringTrimEnd,
    StringTrimStart,
    StreamCollectList,
    StreamFromList,
    StreamNext,
    SenderClose,
    SenderSend,
    SenderSendCancellable,
    TcpConnect,
    TcpErrorMessage,
    TcpStreamRead,
    TcpStreamShutdown,
    TcpStreamWrite,
    TcpStreamWriteAll,
    TempDirKeep,
    TempDirNew,
    TempDirNewIn,
    TempDirPath,
    TomlParseFile,
    UuidNewV4,
    UrlDecodeComponent,
    UrlEncodeComponent,
    UrlFromString,
    UrlToString,
    TimerSleep,
    TimerSleepCancellable,
    TimerSleepUntil,
    WebSocketClose,
    WebSocketConnect,
    WebSocketErrorMessage,
    WebSocketRecvBytes,
    WebSocketRecvText,
    WebSocketSendBytes,
    WebSocketSendText,
    YamlParse,
    YamlParseFile,
    WeakDowngrade,
    WeakFrom,
    WeakUpgrade,
}

#[derive(Debug, Clone, Copy)]
enum RegIntCompare {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

type Reg = usize;

#[derive(Debug, Default)]
struct LoopPatch {
    breaks: Vec<usize>,
    continues: Vec<usize>,
    cleanup_base: usize,
}

#[derive(Debug, Clone, Copy)]
enum MatchFailurePatch {
    Jump(usize),
    OptionSome(usize),
    OptionNone(usize),
    ResultOk(usize),
    ResultErr(usize),
    VariantOther(usize),
}

impl RegUnit {
    fn lower(hir: &Hir) -> Result<Self, EvalError> {
        let names = hir
            .function_bodies()
            .filter_map(|(name, body)| body.block.as_ref().map(|_| name.to_string()))
            .collect::<Vec<_>>();
        let function_ids = names
            .iter()
            .enumerate()
            .map(|(id, name)| (name.clone(), id))
            .collect::<HashMap<_, _>>();
        let mut functions = names
            .iter()
            .cloned()
            .map(RegFunction::placeholder)
            .collect::<Vec<_>>();
        for (function_id, name) in names.into_iter().enumerate() {
            let body = hir
                .function_body(&name)
                .and_then(|body| body.block.as_ref())
                .ok_or_else(|| {
                    EvalError::Runtime(format!("reg VM cannot find function `{name}`."))
                })?;
            let signature = hir.resolve_function(None, &name).ok_or_else(|| {
                EvalError::Runtime(format!("reg VM cannot resolve function `{name}`."))
            })?;
            let mut lowerer = RegLowerer {
                hir,
                function_ids: &function_ids,
                functions: &mut functions,
                function: RegFunction {
                    name,
                    params: signature.params.len(),
                    captures: 0,
                    regs: 0,
                    local_regs: HashMap::new(),
                    code: Vec::new(),
                    jit_analysis: std::cell::Cell::new(None),
                    native_status: std::cell::Cell::new(0),
                },
                loop_stack: Vec::new(),
                cleanup_stack: Vec::new(),
            };
            for param in &signature.params {
                let reg = lowerer.local(&param.name);
                // `mut` params alias the caller's value (the backend lowers them to
                // `&mut`), so mutations must propagate; every other effect is
                // by-value/`&`, so the callee gets an isolated deep copy.
                if param.effect != Some(ParamEffect::Mut) {
                    lowerer.emit(RegInstr::DeepCopy { reg });
                }
            }
            lowerer.block(body)?;
            let unit = lowerer.temp();
            lowerer.emit(RegInstr::LoadUnit { dst: unit });
            lowerer.emit(RegInstr::Return { src: unit });
            functions[function_id] = lowerer.function;
        }
        let mut resource_drop_functions = HashMap::new();
        for (type_name, body) in hir.resource_drop_bodies() {
            let function_id = functions.len();
            functions.push(RegFunction::placeholder(format!("<drop:{type_name}>")));
            let mut lowerer = RegLowerer {
                hir,
                function_ids: &function_ids,
                functions: &mut functions,
                function: RegFunction {
                    name: format!("<drop:{type_name}>"),
                    params: 0,
                    captures: 0,
                    regs: 0,
                    local_regs: HashMap::new(),
                    code: Vec::new(),
                    jit_analysis: std::cell::Cell::new(None),
                    native_status: std::cell::Cell::new(0),
                },
                loop_stack: Vec::new(),
                cleanup_stack: Vec::new(),
            };
            if let Some(info) = hir.type_info(type_name) {
                for field in &info.fields_ordered {
                    lowerer.local(&field.name);
                }
            }
            lowerer.block(body)?;
            let unit = lowerer.temp();
            lowerer.emit(RegInstr::LoadUnit { dst: unit });
            lowerer.emit(RegInstr::Return { src: unit });
            functions[function_id] = lowerer.function;
            resource_drop_functions.insert(type_name.to_string(), function_id);
        }
        // Cache tier-0 JIT analysis `(eligible, has_loop)` per function. Eligible
        // is the unit-wide non-suspending + non-recursive fixpoint, so it accounts
        // for cross-function calls; `has_loop` gates whether the production
        // heuristic bothers JIT-ing a given eligible function.
        let eligibility = compute_jit_eligibility(&functions);
        for (function, &eligible) in functions.iter().zip(&eligibility) {
            let has_loop = jit_function_has_loop(&function.code);
            function.jit_analysis.set(Some((eligible, has_loop)));
        }
        Ok(Self {
            functions: functions.into_iter().map(Rc::new).collect(),
            function_ids,
            resource_drop_functions,
            types: hir
                .types()
                .map(|type_info| (type_info.name.clone(), type_info.clone()))
                .collect(),
        })
    }
}

struct RegLowerer<'a> {
    hir: &'a Hir,
    function_ids: &'a HashMap<String, usize>,
    functions: &'a mut Vec<RegFunction>,
    function: RegFunction,
    loop_stack: Vec<LoopPatch>,
    cleanup_stack: Vec<Reg>,
}

impl RegLowerer<'_> {
    fn local(&mut self, name: &str) -> Reg {
        if let Some(reg) = self.function.local_regs.get(name) {
            return *reg;
        }
        let reg = self.temp();
        self.function.local_regs.insert(name.to_string(), reg);
        reg
    }

    fn lookup_local(&self, name: &str) -> Result<Reg, EvalError> {
        self.function
            .local_regs
            .get(name)
            .copied()
            .ok_or_else(|| EvalError::Runtime(format!("reg VM cannot resolve local `{name}`.")))
    }

    /// The declaration-order slot of `field` on a statically-known struct/variant
    /// type, used to emit `GetFieldSlot`/`SetFieldSlot`. `None` (→ name-based
    /// access) when the base type is unknown or not a registered type. Struct
    /// construction is canonicalized to this same order (see `MakeStruct`), so the
    /// runtime layout matches the slot.
    fn field_slot(&self, base_type: Option<&str>, field: &str) -> Option<usize> {
        let info = self.hir.type_info(base_type?)?;
        info.fields_ordered.iter().position(|f| f.name == field)
    }

    /// Reorder named constructor fields into the type's declaration order so every
    /// instance of a type shares one field layout (and matches `field_slot`).
    fn canonicalize_field_order(&self, type_name: &str, fields: &mut [(String, Reg)]) {
        if let Some(info) = self.hir.type_info(type_name) {
            fields.sort_by_key(|(name, _)| {
                info.fields_ordered
                    .iter()
                    .position(|f| &f.name == name)
                    .unwrap_or(usize::MAX)
            });
        }
    }

    fn temp(&mut self) -> Reg {
        let reg = self.function.regs;
        self.function.regs += 1;
        reg
    }

    fn emit(&mut self, instr: RegInstr) -> usize {
        let ip = self.function.code.len();
        self.function.code.push(instr);
        ip
    }

    fn cleanup_regs_since(&self, base: usize) -> Vec<Reg> {
        self.cleanup_stack[base..].iter().rev().copied().collect()
    }

    fn all_cleanup_regs(&self) -> Vec<Reg> {
        self.cleanup_regs_since(0)
    }

    fn emit_cleanup_since(&mut self, base: usize) {
        for resource in self.cleanup_regs_since(base) {
            self.emit(RegInstr::ResourceDrop { resource });
        }
    }

    fn emit_all_cleanup(&mut self) {
        self.emit_cleanup_since(0);
    }

    fn patch_jump(&mut self, jump_ip: usize, target: usize) {
        match &mut self.function.code[jump_ip] {
            RegInstr::Jump {
                target: jump_target,
            }
            | RegInstr::JumpIfBool {
                target: jump_target,
                ..
            }
            | RegInstr::JumpIfIntCompare {
                target: jump_target,
                ..
            }
            | RegInstr::MatchOption {
                some_ip: jump_target,
                ..
            }
            | RegInstr::MatchResult {
                ok_ip: jump_target, ..
            }
            | RegInstr::MatchVariant {
                match_ip: jump_target,
                ..
            } => *jump_target = target,
            _ => {}
        }
    }

    fn patch_match_none(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchOption { none_ip, .. } = &mut self.function.code[match_ip] {
            *none_ip = target;
        }
    }

    fn patch_result_match_err(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchResult { err_ip, .. } = &mut self.function.code[match_ip] {
            *err_ip = target;
        }
    }

    fn patch_variant_match_else(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchVariant { else_ip, .. } = &mut self.function.code[match_ip] {
            *else_ip = target;
        }
    }

    fn patch_map_match_some(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchMapGet { some_ip, .. } = &mut self.function.code[match_ip] {
            *some_ip = target;
        }
    }

    fn patch_map_match_none(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchMapGet { none_ip, .. } = &mut self.function.code[match_ip] {
            *none_ip = target;
        }
    }

    fn patch_many(&mut self, jump_ips: Vec<usize>, target: usize) {
        for jump_ip in jump_ips {
            self.patch_jump(jump_ip, target);
        }
    }

    fn block(&mut self, block: &HirBlock) -> Result<(), EvalError> {
        for statement in &block.statements {
            self.statement(statement)?;
        }
        Ok(())
    }

    /// Assign `value` to an lvalue `target`. Locals are written directly; field
    /// and index targets read the current container, produce an updated copy
    /// (value semantics), and recurse to store that copy back into the enclosing
    /// place, so arbitrarily nested targets like `a.b.items[i]` compose.
    /// Lower `spawn f(args)` (and the call behind an `async let`): evaluate the
    /// arguments in the spawning task, then emit a `SpawnTask` that starts `f`
    /// as a new task and yields its handle. Only a direct call to a known
    /// function is supported (matching how the backend desugars `spawn`).
    fn lower_spawn(&mut self, value: &HirExpr) -> Result<Reg, EvalError> {
        let HirExpr::Call { callee, args, .. } = value else {
            return Err(EvalError::Runtime(
                "reg VM spawn/async-let expects a direct function call.".to_string(),
            ));
        };
        let Callee::Name(name) = callee else {
            return Err(EvalError::Runtime(
                "reg VM spawn/async-let supports only named function calls.".to_string(),
            ));
        };
        let function = self
            .function_ids
            .get(type_root_name(name))
            .copied()
            .ok_or_else(|| {
                EvalError::Runtime(format!(
                    "reg VM cannot resolve spawned function `{}`.",
                    type_root_name(name)
                ))
            })?;
        let arg_regs = args
            .iter()
            .map(|arg| self.expr(&arg.value))
            .collect::<Result<Vec<_>, _>>()?;
        let dst = self.temp();
        self.emit(RegInstr::SpawnTask {
            dst,
            function,
            args: arg_regs,
        });
        Ok(dst)
    }

    fn lower_assign(&mut self, target: &HirExpr, value: Reg) -> Result<(), EvalError> {
        match target {
            HirExpr::Ident { name, .. } => {
                let dst = self.lookup_local(name)?;
                self.emit(RegInstr::Move { dst, src: value });
                Ok(())
            }
            HirExpr::Field {
                base, name, access, ..
            } => {
                // Read the current container, write an updated copy back into
                // `base_value` in place, then store that copy into the enclosing
                // place (value semantics, composes for nested paths).
                let base_value = self.expr(base)?;
                let dst = self.temp();
                if let Some(slot) = self.field_slot(access.base_type.as_deref(), name) {
                    self.emit(RegInstr::SetFieldSlot {
                        dst,
                        base: base_value,
                        slot,
                        value,
                    });
                } else {
                    self.emit(RegInstr::SetField {
                        dst,
                        base: base_value,
                        name: name.clone(),
                        value,
                    });
                }
                self.lower_assign(base, base_value)
            }
            HirExpr::Index { base, index, .. } => {
                let base_value = self.expr(base)?;
                let index = self.expr(index)?;
                let dst = self.temp();
                self.emit(RegInstr::ListSet {
                    dst,
                    list: base_value,
                    index,
                    value,
                });
                self.lower_assign(base, base_value)
            }
            _ => Err(EvalError::Runtime(
                "reg VM assignment target must be a local, field, or index path.".to_string(),
            )),
        }
    }

    fn expr_block_value(&mut self, block: &HirBlock) -> Result<Reg, EvalError> {
        let Some((last, prefix)) = block.statements.split_last() else {
            return Err(EvalError::Runtime(
                "reg VM match expression arm cannot be empty.".to_string(),
            ));
        };
        for statement in prefix {
            self.statement(statement)?;
        }
        match last {
            HirStmt::Expr(value) => self.expr(value),
            HirStmt::Return { value, .. } => {
                let src = if let Some(value) = value {
                    self.expr(value)?
                } else {
                    let src = self.temp();
                    self.emit(RegInstr::LoadUnit { dst: src });
                    src
                };
                self.emit_all_cleanup();
                self.emit(RegInstr::Return { src });
                Ok(src)
            }
            other => Err(EvalError::Runtime(format!(
                "reg VM match expression arm must end with an expression, got `{other:?}`."
            ))),
        }
    }

    fn condition_jump(
        &mut self,
        condition: &HirExpr,
        expected: bool,
        target: usize,
    ) -> Result<usize, EvalError> {
        if let HirExpr::Binary {
            op, left, right, ..
        } = condition
        {
            if let Some(op) = int_compare_op(*op) {
                let lhs = self.expr(left)?;
                let rhs = self.expr(right)?;
                return Ok(self.emit(RegInstr::JumpIfIntCompare {
                    lhs,
                    rhs,
                    op,
                    expected,
                    target,
                }));
            }
        }
        let cond = self.expr(condition)?;
        Ok(self.emit(RegInstr::JumpIfBool {
            cond,
            expected,
            target,
        }))
    }

    fn statement(&mut self, statement: &HirStmt) -> Result<(), EvalError> {
        match statement {
            HirStmt::Let {
                name,
                value,
                is_async,
                ..
            } => {
                let dst = self.local(name);
                if let Some(value) = value {
                    // `async let x = f()` spawns `f` as a task and binds `x` to its
                    // handle; a plain `let` evaluates eagerly in the current task.
                    let src = if *is_async {
                        self.lower_spawn(value)?
                    } else {
                        self.expr(value)?
                    };
                    self.emit(RegInstr::Move { dst, src });
                } else {
                    self.emit(RegInstr::LoadUnit { dst });
                }
            }
            HirStmt::Assign { target, value, .. } => {
                let src = self.expr(value)?;
                self.lower_assign(target, src)?;
            }
            HirStmt::Return { value, .. } => {
                let src = if let Some(value) = value {
                    self.expr(value)?
                } else {
                    let src = self.temp();
                    self.emit(RegInstr::LoadUnit { dst: src });
                    src
                };
                self.emit_all_cleanup();
                self.emit(RegInstr::Return { src });
            }
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                let src = self.expr(resource)?;
                let dst = self.local(binding);
                self.emit(RegInstr::Move { dst, src });
                self.cleanup_stack.push(dst);
                self.block(body)?;
                self.cleanup_stack
                    .pop()
                    .expect("with cleanup stack should contain binding");
                self.emit(RegInstr::ResourceDrop { resource: dst });
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                let else_jump = self.condition_jump(condition, false, usize::MAX)?;
                self.block(then_body)?;
                let end_jump = self.emit(RegInstr::Jump { target: usize::MAX });
                let else_ip = self.function.code.len();
                self.patch_jump(else_jump, else_ip);
                if let Some(else_body) = else_body {
                    self.block(else_body)?;
                }
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                let start_ip = self.function.code.len();
                let exit_jump = if let Some(condition) = condition {
                    Some(self.condition_jump(condition, false, usize::MAX)?)
                } else {
                    None
                };
                self.loop_stack.push(LoopPatch {
                    cleanup_base: self.cleanup_stack.len(),
                    ..LoopPatch::default()
                });
                self.block(body)?;
                let loop_patch = self.loop_stack.pop().expect("loop patch should exist");
                self.emit(RegInstr::Jump { target: start_ip });
                let exit_ip = self.function.code.len();
                if let Some(exit_jump) = exit_jump {
                    self.patch_jump(exit_jump, exit_ip);
                }
                self.patch_many(loop_patch.breaks, exit_ip);
                self.patch_many(loop_patch.continues, start_ip);
            }
            HirStmt::For {
                binding,
                iterable,
                is_async,
                body,
                ..
            } => {
                if *is_async {
                    let stream = self.expr(iterable)?;
                    let start_ip = self.function.code.len();
                    let next_result = self.temp();
                    self.emit(RegInstr::CallIntrinsic {
                        dst: next_result,
                        intrinsic: RegIntrinsic::StreamNext,
                        args: vec![stream],
                    });
                    let next_option = self.temp();
                    self.emit(RegInstr::TryResult {
                        dst: next_option,
                        src: next_result,
                        cleanup: self.all_cleanup_regs(),
                    });
                    let match_ip = self.emit(RegInstr::MatchOption {
                        src: next_option,
                        some_ip: usize::MAX,
                        none_ip: usize::MAX,
                    });
                    let some_ip = self.function.code.len();
                    self.patch_jump(match_ip, some_ip);
                    let item = self.local(binding);
                    self.emit(RegInstr::UnwrapSome {
                        dst: item,
                        src: next_option,
                    });

                    self.loop_stack.push(LoopPatch {
                        cleanup_base: self.cleanup_stack.len(),
                        ..LoopPatch::default()
                    });
                    self.block(body)?;
                    let loop_patch = self.loop_stack.pop().expect("loop patch should exist");
                    self.emit(RegInstr::Jump { target: start_ip });
                    let exit_ip = self.function.code.len();
                    self.patch_match_none(match_ip, exit_ip);
                    self.patch_many(loop_patch.breaks, exit_ip);
                    self.patch_many(loop_patch.continues, start_ip);
                    return Ok(());
                }
                let list = self.expr(iterable)?;
                let index = self.temp();
                self.emit(RegInstr::LoadInt {
                    dst: index,
                    value: 0,
                });
                let len = self.temp();
                self.emit(RegInstr::ListLen { dst: len, list });
                let one = self.temp();
                self.emit(RegInstr::LoadInt { dst: one, value: 1 });

                let start_ip = self.function.code.len();
                let exit_jump = self.emit(RegInstr::JumpIfIntCompare {
                    lhs: index,
                    rhs: len,
                    op: RegIntCompare::Less,
                    expected: false,
                    target: usize::MAX,
                });
                let item = self.local(binding);
                self.emit(RegInstr::ListGet {
                    dst: item,
                    list,
                    index,
                });

                self.loop_stack.push(LoopPatch {
                    cleanup_base: self.cleanup_stack.len(),
                    ..LoopPatch::default()
                });
                self.block(body)?;
                let loop_patch = self.loop_stack.pop().expect("loop patch should exist");
                let continue_ip = self.function.code.len();
                self.emit(RegInstr::AddInt {
                    dst: index,
                    lhs: index,
                    rhs: one,
                });
                self.emit(RegInstr::Jump { target: start_ip });
                let exit_ip = self.function.code.len();
                self.patch_jump(exit_jump, exit_ip);
                self.patch_many(loop_patch.breaks, exit_ip);
                self.patch_many(loop_patch.continues, continue_ip);
            }
            HirStmt::Match { value, arms, .. } => {
                if !self.map_get_match(value, arms)?
                    && !self.variant_match(value, arms)?
                    && !self.struct_match(value, arms)?
                {
                    return Err(EvalError::Runtime(
                        "reg VM v0 does not support this match pattern.".to_string(),
                    ));
                }
            }
            HirStmt::Select { arms, .. } => {
                // First-ready select: spawn each arm's operation as a concurrent
                // task, park on whichever finishes first, then dispatch to that
                // arm's body. The scheduler's clock makes timing (e.g. differing
                // sleeps) decide the winner, matching the backend's executor.
                if arms.is_empty() {
                    return Ok(());
                }
                let mut handles = Vec::with_capacity(arms.len());
                let mut arm_has_try = Vec::with_capacity(arms.len());
                for arm in arms {
                    let (call, has_try) = peel_select_operation(&arm.operation);
                    handles.push(self.lower_spawn(call)?);
                    arm_has_try.push(has_try);
                }
                let winner = self.temp();
                let value = self.temp();
                self.emit(RegInstr::SelectWait {
                    handles,
                    winner,
                    value,
                });
                let mut end_jumps = Vec::with_capacity(arms.len());
                for (index, arm) in arms.iter().enumerate() {
                    let index_const = self.temp();
                    self.emit(RegInstr::LoadInt {
                        dst: index_const,
                        value: index as i64,
                    });
                    let is_winner = self.temp();
                    self.emit(RegInstr::Equal {
                        dst: is_winner,
                        lhs: winner,
                        rhs: index_const,
                    });
                    let skip = self.emit(RegInstr::JumpIfBool {
                        cond: is_winner,
                        expected: false,
                        target: usize::MAX,
                    });
                    // The winning arm's value is the spawned task's result; apply
                    // the arm operation's `?` (if any) before binding.
                    let bound = if arm_has_try[index] {
                        let dst = self.temp();
                        self.emit(RegInstr::TryResult {
                            dst,
                            src: value,
                            cleanup: self.all_cleanup_regs(),
                        });
                        dst
                    } else {
                        value
                    };
                    if arm.binding != "_" {
                        let binding = self.local(&arm.binding);
                        self.emit(RegInstr::Move {
                            dst: binding,
                            src: bound,
                        });
                    }
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
                    let next_arm = self.function.code.len();
                    self.patch_jump(skip, next_arm);
                }
                let end = self.function.code.len();
                for jump in end_jumps {
                    self.patch_jump(jump, end);
                }
            }
            HirStmt::Break(_) => {
                if self.loop_stack.is_empty() {
                    return Err(EvalError::Runtime(
                        "reg VM break used outside of a loop.".to_string(),
                    ));
                }
                let cleanup_base = self
                    .loop_stack
                    .last()
                    .expect("loop patch should exist")
                    .cleanup_base;
                self.emit_cleanup_since(cleanup_base);
                let jump = self.emit(RegInstr::Jump { target: usize::MAX });
                self.loop_stack
                    .last_mut()
                    .expect("loop patch should exist")
                    .breaks
                    .push(jump);
            }
            HirStmt::Continue(_) => {
                if self.loop_stack.is_empty() {
                    return Err(EvalError::Runtime(
                        "reg VM continue used outside of a loop.".to_string(),
                    ));
                }
                let cleanup_base = self
                    .loop_stack
                    .last()
                    .expect("loop patch should exist")
                    .cleanup_base;
                self.emit_cleanup_since(cleanup_base);
                let jump = self.emit(RegInstr::Jump { target: usize::MAX });
                self.loop_stack
                    .last_mut()
                    .expect("loop patch should exist")
                    .continues
                    .push(jump);
            }
            HirStmt::Expr(expr) => {
                self.expr(expr)?;
            }
            unsupported => Err(EvalError::Runtime(format!(
                "reg VM v0 does not support statement `{unsupported:?}`."
            )))?,
        }
        Ok(())
    }

    fn logical_binary(
        &mut self,
        op: BinaryOp,
        left: &HirExpr,
        right: &HirExpr,
    ) -> Result<Reg, EvalError> {
        let lhs = self.expr(left)?;
        let dst = self.temp();
        match op {
            BinaryOp::LogicalAnd => {
                let short_circuit = self.emit(RegInstr::JumpIfBool {
                    cond: lhs,
                    expected: false,
                    target: usize::MAX,
                });
                let rhs = self.expr(right)?;
                self.emit(RegInstr::Move { dst, src: rhs });
                let end_jump = self.emit(RegInstr::Jump { target: usize::MAX });
                let false_ip = self.function.code.len();
                self.patch_jump(short_circuit, false_ip);
                self.emit(RegInstr::LoadBool { dst, value: false });
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            BinaryOp::LogicalOr => {
                let short_circuit = self.emit(RegInstr::JumpIfBool {
                    cond: lhs,
                    expected: true,
                    target: usize::MAX,
                });
                let rhs = self.expr(right)?;
                self.emit(RegInstr::Move { dst, src: rhs });
                let end_jump = self.emit(RegInstr::Jump { target: usize::MAX });
                let true_ip = self.function.code.len();
                self.patch_jump(short_circuit, true_ip);
                self.emit(RegInstr::LoadBool { dst, value: true });
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            _ => unreachable!(),
        }
        Ok(dst)
    }

    fn expr(&mut self, expr: &HirExpr) -> Result<Reg, EvalError> {
        match expr {
            HirExpr::Ident { name, .. } if name == "Unit" => {
                let dst = self.temp();
                self.emit(RegInstr::LoadUnit { dst });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } if name == "None" => {
                let dst = self.temp();
                self.emit(RegInstr::LoadNone { dst });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } if name == "true" || name == "false" => {
                let dst = self.temp();
                self.emit(RegInstr::LoadBool {
                    dst,
                    value: name == "true",
                });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } if self.hir.sum_type_for_variant(name).is_some() => {
                let fields = self.hir.sum_variant_fields(name).unwrap_or(&[]);
                if !fields.is_empty() {
                    return Err(EvalError::Runtime(format!(
                        "reg VM variant `{name}` requires {} field(s).",
                        fields.len()
                    )));
                }
                let dst = self.temp();
                self.emit(RegInstr::MakeVariant {
                    dst,
                    name: name.clone(),
                    fields: Vec::new(),
                });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } => self.lookup_local(name),
            HirExpr::Number { value, .. } => {
                let dst = self.temp();
                if value.contains('.') {
                    let value = value.parse::<f64>().map_err(|error| {
                        EvalError::Runtime(format!("invalid reg VM float `{value}`: {error}"))
                    })?;
                    self.emit(RegInstr::LoadFloat { dst, value });
                } else {
                    let value = value.parse::<i64>().map_err(|error| {
                        EvalError::Runtime(format!("invalid reg VM integer `{value}`: {error}"))
                    })?;
                    self.emit(RegInstr::LoadInt { dst, value });
                }
                Ok(dst)
            }
            HirExpr::String { value, .. } => {
                let dst = self.temp();
                self.emit(RegInstr::LoadString {
                    dst,
                    value: Rc::new(decode_string_token(value)),
                });
                Ok(dst)
            }
            HirExpr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.expr(item))
                    .collect::<Result<Vec<_>, _>>()?;
                let dst = self.temp();
                self.emit(RegInstr::MakeList { dst, items });
                Ok(dst)
            }
            HirExpr::ObjectLiteral { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|field| Ok((field.name.clone(), self.expr(&field.value)?)))
                    .collect::<Result<Vec<_>, EvalError>>()?;
                let dst = self.temp();
                self.emit(RegInstr::MakeObject { dst, fields });
                Ok(dst)
            }
            HirExpr::MapLiteral { entries, .. } => {
                let entries = entries
                    .iter()
                    .map(|entry| Ok((self.expr(&entry.key)?, self.expr(&entry.value)?)))
                    .collect::<Result<Vec<_>, EvalError>>()?;
                let dst = self.temp();
                self.emit(RegInstr::MakeMap { dst, entries });
                Ok(dst)
            }
            HirExpr::Binary {
                op, left, right, ..
            } => {
                if matches!(op, BinaryOp::LogicalAnd | BinaryOp::LogicalOr) {
                    return self.logical_binary(*op, left, right);
                }
                let lhs = self.expr(left)?;
                let rhs = self.expr(right)?;
                let dst = self.temp();
                let instr = match op {
                    BinaryOp::Add => RegInstr::AddInt { dst, lhs, rhs },
                    BinaryOp::Subtract => RegInstr::SubInt { dst, lhs, rhs },
                    BinaryOp::Multiply => RegInstr::MulInt { dst, lhs, rhs },
                    BinaryOp::Divide => RegInstr::DivInt { dst, lhs, rhs },
                    BinaryOp::Modulo => RegInstr::ModInt { dst, lhs, rhs },
                    BinaryOp::BitAnd => RegInstr::BitAndInt { dst, lhs, rhs },
                    BinaryOp::BitOr => RegInstr::BitOrInt { dst, lhs, rhs },
                    BinaryOp::BitXor => RegInstr::BitXorInt { dst, lhs, rhs },
                    BinaryOp::ShiftLeft => RegInstr::ShiftLeftInt { dst, lhs, rhs },
                    BinaryOp::ShiftRight => RegInstr::ShiftRightInt { dst, lhs, rhs },
                    BinaryOp::Less => RegInstr::LessInt { dst, lhs, rhs },
                    BinaryOp::LessEqual => RegInstr::LessEqualInt { dst, lhs, rhs },
                    BinaryOp::Greater => RegInstr::GreaterInt { dst, lhs, rhs },
                    BinaryOp::GreaterEqual => RegInstr::GreaterEqualInt { dst, lhs, rhs },
                    BinaryOp::Equal => RegInstr::Equal { dst, lhs, rhs },
                    BinaryOp::NotEqual => RegInstr::NotEqual { dst, lhs, rhs },
                    BinaryOp::LogicalAnd | BinaryOp::LogicalOr => unreachable!(),
                };
                self.emit(instr);
                Ok(dst)
            }
            HirExpr::Field {
                base, name, access, ..
            } => {
                let slot = self.field_slot(access.base_type.as_deref(), name);
                let base = self.expr(base)?;
                let dst = self.temp();
                if let Some(slot) = slot {
                    self.emit(RegInstr::GetFieldSlot { dst, base, slot });
                } else {
                    self.emit(RegInstr::GetField {
                        dst,
                        base,
                        name: name.clone(),
                    });
                }
                Ok(dst)
            }
            HirExpr::Index { base, index, .. } => {
                let list = self.expr(base)?;
                let index = self.expr(index)?;
                let dst = self.temp();
                self.emit(RegInstr::ListGet { dst, list, index });
                Ok(dst)
            }
            HirExpr::Effect { value, .. } => self.expr(value),
            HirExpr::Await { value, .. } => {
                let src = self.expr(value)?;
                let dst = self.temp();
                self.emit(RegInstr::AwaitJoin { dst, src });
                Ok(dst)
            }
            HirExpr::Spawn { value, .. } => self.lower_spawn(value),
            HirExpr::Manage { value, .. } => {
                let src = self.expr(value)?;
                let dst = self.temp();
                self.emit(RegInstr::Manage { dst, src });
                Ok(dst)
            }
            HirExpr::Try { value, .. } => {
                let src = self.expr(value)?;
                let dst = self.temp();
                self.emit(RegInstr::TryResult {
                    dst,
                    src,
                    cleanup: self.all_cleanup_regs(),
                });
                Ok(dst)
            }
            HirExpr::Call {
                callee,
                args,
                receiver,
                ..
            } => self.call(callee, receiver.as_ref(), args),
            HirExpr::Closure {
                params,
                captures,
                body,
                ..
            } => {
                let capture_names =
                    closure_capture_names(body, params, captures, &self.function.local_regs);
                let capture_regs = capture_names
                    .iter()
                    .map(|capture| self.lookup_local(capture))
                    .collect::<Result<Vec<_>, _>>()?;
                let function_id = self.functions.len();
                self.functions
                    .push(RegFunction::placeholder(format!("<closure:{function_id}>")));
                let closure_function = {
                    let mut lowerer = RegLowerer {
                        hir: self.hir,
                        function_ids: self.function_ids,
                        functions: &mut *self.functions,
                        function: RegFunction {
                            name: format!("<closure:{function_id}>"),
                            params: params.len(),
                            captures: capture_names.len(),
                            regs: 0,
                            local_regs: HashMap::new(),
                            code: Vec::new(),
                            jit_analysis: std::cell::Cell::new(None),
                            native_status: std::cell::Cell::new(0),
                        },
                        loop_stack: Vec::new(),
                        cleanup_stack: Vec::new(),
                    };
                    for capture in &capture_names {
                        lowerer.local(capture);
                    }
                    for param in params {
                        lowerer.local(param);
                    }
                    // A closure whose body ends in a bare expression yields that
                    // expression's value (e.g. `|x| x > 10`), matching the Rust
                    // backend's tail-expression closure rule. Bodies ending in any
                    // other statement (including an explicit `return`) fall through
                    // to an implicit `Unit` return.
                    if let Some((HirStmt::Expr(value), prefix)) = body.statements.split_last() {
                        for statement in prefix {
                            lowerer.statement(statement)?;
                        }
                        let src = lowerer.expr(value)?;
                        lowerer.emit(RegInstr::Return { src });
                    } else {
                        lowerer.block(body)?;
                        let unit = lowerer.temp();
                        lowerer.emit(RegInstr::LoadUnit { dst: unit });
                        lowerer.emit(RegInstr::Return { src: unit });
                    }
                    lowerer.function
                };
                self.functions[function_id] = closure_function;
                let dst = self.temp();
                self.emit(RegInstr::MakeClosure {
                    dst,
                    function: function_id,
                    captures: capture_regs,
                });
                Ok(dst)
            }
            HirExpr::Match { value, arms, .. } => self.match_expr(value, arms),
            unsupported => Err(EvalError::Runtime(format!(
                "reg VM v0 does not support expression `{unsupported:?}`."
            )))?,
        }
    }

    fn call(
        &mut self,
        callee: &Callee,
        receiver: Option<&HirCallReceiver>,
        args: &[HirCallArg],
    ) -> Result<Reg, EvalError> {
        if let Callee::ReceiverCall { method, .. } = callee {
            // A receiver call `x.method(args)` is sugar for `Type.method(self, args)`.
            // Rather than maintain a second (perpetually-incomplete) intrinsic table
            // here, reuse the full qualified-call lowering — stdlib intrinsics, native
            // functions, user-defined methods, and protocol dispatch — by recursing
            // with the receiver as the first argument. (The reg VM previously bailed
            // on any receiver call outside a small hand-written subset, which blocked
            // running real packages like tinygrad-rss.)
            let Some(receiver) = receiver else {
                return Err(EvalError::Runtime(format!(
                    "reg VM receiver call `{method}` is missing HIR receiver metadata."
                )));
            };
            let Some(namespace) = receiver
                .resolved_namespace
                .as_deref()
                .or(receiver.type_name.as_deref())
            else {
                return Err(EvalError::Runtime(format!(
                    "reg VM receiver call `{method}` is missing receiver type metadata."
                )));
            };
            let synthetic_callee = Callee::Qualified {
                namespace: namespace.to_string(),
                name: method.clone(),
            };
            let mut synthetic_args = Vec::with_capacity(args.len() + 1);
            synthetic_args.push(HirCallArg {
                name: None,
                value: (*receiver.value).clone(),
                span: crate::diagnostic::Span::default(),
            });
            synthetic_args.extend(args.iter().cloned());
            return self.call(&synthetic_callee, None, &synthetic_args);
        }

        let arg_regs = args
            .iter()
            .map(|arg| self.expr(&arg.value))
            .collect::<Result<Vec<_>, _>>()?;
        let dst = self.temp();
        match callee {
            Callee::Name(name) => {
                // A generic call carries its type args in `name` (e.g.
                // `get_v<Int>`); functions are keyed by their bare name, so strip
                // the generics before the lookup — otherwise a generic *function*
                // call falls through and is mis-lowered as a struct construction.
                if let Some(function) = self.function_ids.get(type_root_name(name)).copied() {
                    let mut_args = self.user_mut_arg_positions(name);
                    self.emit(RegInstr::CallKnown {
                        dst,
                        function,
                        args: arg_regs,
                        mut_args,
                    });
                } else if self.is_native_function(None, name) {
                    let mut_args = self.native_mut_arg_positions(None, name);
                    self.emit(RegInstr::CallNative {
                        dst,
                        key: type_root_name(name).to_string(),
                        args: arg_regs,
                        mut_args,
                    });
                } else if type_root_name(name) == "Some" {
                    if arg_regs.len() != 1 {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Option variant `Some` expected 1 payload, got {}.",
                            arg_regs.len()
                        )));
                    }
                    self.emit(RegInstr::MakeSome {
                        dst,
                        value: arg_regs[0],
                    });
                } else if matches!(type_root_name(name), "Ok" | "Err") {
                    if arg_regs.len() != 1 {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Result variant `{}` expected 1 payload, got {}.",
                            type_root_name(name),
                            arg_regs.len()
                        )));
                    }
                    self.emit(RegInstr::MakeVariant {
                        dst,
                        name: type_root_name(name).to_string(),
                        fields: vec![("value".to_string(), arg_regs[0])],
                    });
                } else if self
                    .hir
                    .sum_type_for_variant(type_root_name(name))
                    .is_some()
                {
                    let variant_name = type_root_name(name);
                    let fields = self.hir.sum_variant_fields(variant_name).unwrap_or(&[]);
                    match fields.len() {
                        0 if arg_regs.is_empty() => {
                            self.emit(RegInstr::MakeVariant {
                                dst,
                                name: variant_name.to_string(),
                                fields: Vec::new(),
                            });
                        }
                        1 if arg_regs.len() == 1 => {
                            self.emit(RegInstr::MakeVariant {
                                dst,
                                name: variant_name.to_string(),
                                fields: vec![(fields[0].name.clone(), arg_regs[0])],
                            });
                        }
                        field_count if field_count == arg_regs.len() => {
                            let fields = args
                                .iter()
                                .zip(arg_regs)
                                .enumerate()
                                .map(|(index, (arg, reg))| {
                                    let name = arg
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| fields[index].name.clone());
                                    (name, reg)
                                })
                                .collect::<Vec<_>>();
                            self.emit(RegInstr::MakeVariant {
                                dst,
                                name: variant_name.to_string(),
                                fields,
                            });
                        }
                        field_count => {
                            return Err(EvalError::Runtime(format!(
                                "reg VM variant `{variant_name}` expected {field_count} field(s), got {}.",
                                arg_regs.len()
                            )));
                        }
                    }
                } else {
                    let mut fields = args
                        .iter()
                        .zip(arg_regs)
                        .map(|(arg, reg)| {
                            arg.name.clone().map(|name| (name, reg)).ok_or_else(|| {
                                EvalError::Runtime(
                                    "reg VM v0 struct constructors require named fields."
                                        .to_string(),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let type_name = type_root_name(name).to_string();
                    self.canonicalize_field_order(&type_name, &mut fields);
                    self.emit(RegInstr::MakeStruct {
                        dst,
                        name: type_name,
                        fields,
                    });
                }
            }
            Callee::Qualified { namespace, name } => {
                let namespace_root = type_root_name(namespace);
                let name_root = type_root_name(name);
                let intrinsic = if let Some(intrinsic) =
                    qualified_intrinsic(namespace_root, name_root)
                {
                    intrinsic
                } else {
                    match (namespace_root, name_root) {
                        ("Buffer", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Buffer.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::BufferClear {
                                dst,
                                buffer: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Cache", "insert") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Cache.insert expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapInsert {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("CancellationToken", "is_cancelled") => {
                            RegIntrinsic::CancellationTokenIsCancelled
                        }
                        ("ConfigStore", "replace") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM ConfigStore.replace expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ConfigStoreReplace {
                                dst,
                                store: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Counter", "add") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Counter.add expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::CounterAdd {
                                dst,
                                counter: arg_regs[0],
                                amount: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequeClear {
                                dst,
                                deque: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "pop_back") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.pop_back expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequePopBack {
                                dst,
                                deque: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "pop_front") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.pop_front expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequePopFront {
                                dst,
                                deque: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "push_back") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.push_back expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequePushBack {
                                dst,
                                deque: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "push_front") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.push_front expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequePushFront {
                                dst,
                                deque: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("GlobalConfig", "replace") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM GlobalConfig.replace expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::GlobalConfigReplace {
                                dst,
                                global: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Http", "post_json_bearer_retry_async") => {
                            RegIntrinsic::HttpPostJsonBearerRetryAsync
                        }
                        ("Json", "array_contains_substring") => {
                            RegIntrinsic::JsonArrayContainsSubstring
                        }
                        ("Json", "bool_at_or") | ("Json", "json_bool_at_or") => {
                            RegIntrinsic::JsonBoolAtOr
                        }
                        ("Json", "string_at_or") | ("Json", "json_string_at_or") => {
                            RegIntrinsic::JsonStringAtOr
                        }
                        ("List", "append") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.append expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListAppend {
                                dst,
                                list: arg_regs[0],
                                values: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListClear {
                                dst,
                                list: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("List", "filter") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.filter expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListFilter {
                                dst,
                                list: arg_regs[0],
                                predicate: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "fold") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.fold expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListFold {
                                dst,
                                list: arg_regs[0],
                                state: arg_regs[1],
                                folder: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("List", "get") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.get expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListGet {
                                dst,
                                list: arg_regs[0],
                                index: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "len") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.len expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListLen {
                                dst,
                                list: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("List", "map") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.map expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListMap {
                                dst,
                                list: arg_regs[0],
                                mapper: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "pop") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.pop expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListPop {
                                dst,
                                list: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("List", "remove_at") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.remove_at expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListRemoveAt {
                                dst,
                                list: arg_regs[0],
                                index: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "set") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.set expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListSet {
                                dst,
                                list: arg_regs[0],
                                index: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("List", "sort") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.sort expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListSort {
                                dst,
                                list: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("List", "sort_by") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.sort_by expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListSortBy {
                                dst,
                                list: arg_regs[0],
                                key: arg_regs[1],
                                compare: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("List", "sort_with") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.sort_with expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListSortWith {
                                dst,
                                list: arg_regs[0],
                                compare: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "push") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.push expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListPush {
                                dst,
                                list: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Map", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapClear {
                                dst,
                                map: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Map", "get") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.get expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapGet {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Map", "insert") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.insert expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapInsert {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("Map", "insert_old") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.insert_old expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapInsertOld {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("Map", "remove") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.remove expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapRemove {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Process", "run_many_stdout_timeout") => {
                            RegIntrinsic::ProcessRunManyStdoutTimeout
                        }
                        ("Process", "run_many_stdout_timeout_async") => {
                            RegIntrinsic::ProcessRunManyStdoutTimeoutAsync
                        }
                        ("Process", "run_request_cancellable_async") => {
                            RegIntrinsic::ProcessRunRequestCancellableAsync
                        }
                        ("Process", "run_stdout_timeout_async") => {
                            RegIntrinsic::ProcessRunStdoutTimeoutAsync
                        }
                        ("Pipeline", "filter") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Pipeline.filter expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListFilter {
                                dst,
                                list: arg_regs[0],
                                predicate: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Pipeline", "map") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Pipeline.map expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListMap {
                                dst,
                                list: arg_regs[0],
                                mapper: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("String", "concat") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM String.concat expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::StringConcat {
                                dst,
                                left: arg_regs[0],
                                right: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Set", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Set.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SetClear {
                                dst,
                                set: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Set", "for_each") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Set.for_each expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SetForEach {
                                dst,
                                set: arg_regs[0],
                                callback: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Set", "insert") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Set.insert expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SetInsert {
                                dst,
                                set: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Set", "remove") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Set.remove expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SetRemove {
                                dst,
                                set: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("SortedSet", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedSet.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedSetClear {
                                dst,
                                set: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("SortedSet", "insert") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedSet.insert expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedSetInsert {
                                dst,
                                set: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("SortedSet", "remove") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedSet.remove expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedSetRemove {
                                dst,
                                set: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("SortedMap", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedMap.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedMapClear {
                                dst,
                                map: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("SortedMap", "insert") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedMap.insert expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedMapInsert {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("SortedMap", "remove") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedMap.remove expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedMapRemove {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("StringBuilder", "push") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM StringBuilder.push expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::StringBuilderPush {
                                dst,
                                builder: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        _ => {
                            let qualified_key = format!("{namespace_root}.{name_root}");
                            // Native declarations also appear in `function_ids` (with
                            // empty bodies), so dispatch them as native boundaries
                            // first. A user-defined qualified function (e.g.
                            // `pub fn Sqlx.execute`) is never native, so it falls
                            // through to the `function_ids` lookup below.
                            if self.is_native_function(Some(namespace_root), name_root) {
                                let mut_args =
                                    self.native_mut_arg_positions(Some(namespace_root), name_root);
                                self.emit(RegInstr::CallNative {
                                    dst,
                                    key: qualified_key,
                                    args: arg_regs,
                                    mut_args,
                                });
                                return Ok(dst);
                            }
                            // Dynamic protocol dispatch: `Protocol.method(self: x, ...)`
                            // where `Protocol` is a protocol with impls. The concrete
                            // function is selected at runtime by `args[0]`'s struct type
                            // (capability objects + generic bounds) — the VM equivalent
                            // of the compiled backend's closed-world enum dispatch.
                            // Checked before the `function_ids` lookup because a protocol
                            // method also appears there as a bodyless stub (which would
                            // wrongly return `Unit`).
                            let dispatch: Vec<(String, usize)> = self
                                .hir
                                .protocol_method_targets(namespace_root, name_root)
                                .into_iter()
                                .filter_map(|(type_name, target)| {
                                    self.function_ids
                                        .get(type_root_name(&target))
                                        .copied()
                                        .map(|function| (type_name, function))
                                })
                                .collect();
                            if !dispatch.is_empty() {
                                let mut_args =
                                    self.native_mut_arg_positions(Some(namespace_root), name_root);
                                self.emit(RegInstr::CallDynamic {
                                    dst,
                                    dispatch,
                                    args: arg_regs,
                                    mut_args,
                                });
                                return Ok(dst);
                            }
                            if let Some(function) = self.function_ids.get(&qualified_key).copied() {
                                let mut_args =
                                    self.native_mut_arg_positions(Some(namespace_root), name_root);
                                self.emit(RegInstr::CallKnown {
                                    dst,
                                    function,
                                    args: arg_regs,
                                    mut_args,
                                });
                                return Ok(dst);
                            }
                            // `.clone()` (a derived `Clone`) deep-copies any value. A
                            // receiver call resolves its namespace to the concrete type
                            // (e.g. `Ops.clone`), not `Clone`, so map an otherwise
                            // unresolved `clone` to the deep-clone intrinsic.
                            if name_root == "clone" && arg_regs.len() == 1 {
                                self.emit(RegInstr::CallIntrinsic {
                                    dst,
                                    intrinsic: RegIntrinsic::CloneClone,
                                    args: arg_regs,
                                });
                                return Ok(dst);
                            }
                            return Err(EvalError::Runtime(format!(
                                "reg VM v0 does not support intrinsic `{namespace}.{name}`."
                            )));
                        }
                    }
                };
                match intrinsic {
                    RegIntrinsic::JsonDecode | RegIntrinsic::JsonDecodeText => {
                        let type_arg = type_arg_names(name)
                            .and_then(|args| args.first().copied())
                            .ok_or_else(|| {
                                EvalError::Runtime(format!(
                                    "reg VM {namespace}.{name} requires a concrete type argument."
                                ))
                            })?;
                        self.emit(RegInstr::CallTypedIntrinsic {
                            dst,
                            intrinsic,
                            type_arg: type_root_name(type_arg).to_string(),
                            args: arg_regs,
                        });
                    }
                    _ => {
                        self.emit(RegInstr::CallIntrinsic {
                            dst,
                            intrinsic,
                            args: arg_regs,
                        });
                    }
                }
            }
            Callee::ReceiverCall { .. } => {
                unreachable!("receiver calls return before arg lowering")
            }
        }
        Ok(dst)
    }

    fn is_native_function(&self, namespace: Option<&str>, name: &str) -> bool {
        self.hir
            .resolve_function(namespace, type_root_name(name))
            .is_some_and(|signature| signature.is_native && !signature.is_builtin)
    }

    /// Parameter positions of a native function that are `mut`. These map to
    /// `CallNative` arg positions (the arg list is positional, with the receiver
    /// at index 0 for receiver calls), so the host can write mutated values back.
    fn native_mut_arg_positions(&self, namespace: Option<&str>, name: &str) -> Vec<usize> {
        self.hir
            .resolve_function(namespace, type_root_name(name))
            .map(|signature| {
                signature
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(_, param)| param.effect == Some(ParamEffect::Mut))
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `mut` parameter positions of a user function, so a `CallKnown` can write
    /// the mutated arguments back to the caller (matching AOT's `&mut` params).
    fn user_mut_arg_positions(&self, name: &str) -> Vec<usize> {
        self.native_mut_arg_positions(None, name)
    }

    fn variant_match(&mut self, value: &HirExpr, arms: &[HirMatchArm]) -> Result<bool, EvalError> {
        if arms.is_empty()
            || !arms
                .iter()
                .all(|arm| self.is_supported_match_pattern(&arm.pattern))
        {
            return Ok(false);
        }

        let src = self.expr(value)?;
        let mut failure_patches = Vec::new();
        let mut end_jumps = Vec::new();
        for arm in arms {
            let arm_ip = self.function.code.len();
            self.patch_match_failures(failure_patches, arm_ip);
            failure_patches = self.lower_match_pattern(&arm.pattern, src)?;
            if let Some(guard) = &arm.guard {
                let guard_failure = self.condition_jump(guard, false, usize::MAX)?;
                failure_patches.push(MatchFailurePatch::Jump(guard_failure));
            }
            self.block(&arm.body)?;
            end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
        }

        let no_match_ip = self.function.code.len();
        self.patch_match_failures(failure_patches, no_match_ip);
        self.emit(RegInstr::RuntimeError {
            message: "reg VM match did not match any arm.".to_string(),
        });
        let end_ip = self.function.code.len();
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        Ok(true)
    }

    fn match_expr(&mut self, value: &HirExpr, arms: &[HirMatchArm]) -> Result<Reg, EvalError> {
        if arms.is_empty()
            || !arms
                .iter()
                .all(|arm| self.is_supported_match_pattern(&arm.pattern))
        {
            return Err(EvalError::Runtime(
                "reg VM v0 does not support this match expression pattern.".to_string(),
            ));
        }

        let src = self.expr(value)?;
        let dst = self.temp();
        let mut failure_patches = Vec::new();
        let mut end_jumps = Vec::new();
        for arm in arms {
            let arm_ip = self.function.code.len();
            self.patch_match_failures(failure_patches, arm_ip);
            failure_patches = self.lower_match_pattern(&arm.pattern, src)?;
            if let Some(guard) = &arm.guard {
                let guard_failure = self.condition_jump(guard, false, usize::MAX)?;
                failure_patches.push(MatchFailurePatch::Jump(guard_failure));
            }
            let value = self.expr_block_value(&arm.body)?;
            self.emit(RegInstr::Move { dst, src: value });
            end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
        }

        let no_match_ip = self.function.code.len();
        self.patch_match_failures(failure_patches, no_match_ip);
        self.emit(RegInstr::RuntimeError {
            message: "reg VM match expression did not match any arm.".to_string(),
        });
        let end_ip = self.function.code.len();
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        Ok(dst)
    }

    fn lower_match_pattern(
        &mut self,
        pattern: &MatchPattern,
        src: Reg,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        match pattern {
            MatchPattern::Binding { name, .. } => {
                let dst = self.local(name);
                self.emit(RegInstr::Move { dst, src });
                Ok(Vec::new())
            }
            MatchPattern::Wildcard(_) => Ok(Vec::new()),
            MatchPattern::Variant { name, binding, .. } if name == "Some" => {
                self.lower_option_some_pattern(src, binding.as_deref())
            }
            MatchPattern::Variant { name, .. } if name == "None" => {
                let match_ip = self.emit(RegInstr::MatchOption {
                    src,
                    some_ip: usize::MAX,
                    none_ip: usize::MAX,
                });
                let pass_ip = self.function.code.len();
                self.patch_match_none(match_ip, pass_ip);
                Ok(vec![MatchFailurePatch::OptionSome(match_ip)])
            }
            MatchPattern::Variant { name, binding, .. } if name == "Ok" || name == "Err" => {
                self.lower_result_variant_pattern(src, name, binding.as_deref())
            }
            MatchPattern::Variant { name, binding, .. }
                if self.hir.sum_type_for_variant(name).is_some() =>
            {
                self.lower_user_variant_pattern(src, name, binding.as_deref())
            }
            MatchPattern::Struct { name, fields, .. }
                if self.hir.sum_type_for_variant(name).is_some() =>
            {
                self.lower_user_struct_variant_pattern(src, name, fields)
            }
            MatchPattern::Struct { fields, .. } => self.lower_struct_field_patterns(src, fields),
            MatchPattern::List {
                prefix,
                rest,
                suffix,
                ..
            } => self.lower_list_pattern(src, prefix, rest, suffix),
            MatchPattern::Literal { value, .. } => self.lower_literal_pattern(src, value),
            _ => Err(EvalError::Runtime(
                "reg VM v0 does not support this match pattern.".to_string(),
            )),
        }
    }

    fn is_supported_match_pattern(&self, pattern: &MatchPattern) -> bool {
        match pattern {
            MatchPattern::Binding { .. }
            | MatchPattern::Literal { .. }
            | MatchPattern::Wildcard(_) => true,
            MatchPattern::Variant { name, binding, .. }
                if matches!(name.as_str(), "Some" | "None" | "Ok" | "Err") =>
            {
                binding
                    .as_deref()
                    .is_none_or(|binding| self.is_supported_match_pattern(binding))
            }
            MatchPattern::Variant { name, binding, .. } => {
                self.hir.sum_type_for_variant(name).is_some()
                    && binding
                        .as_deref()
                        .is_none_or(|binding| self.is_supported_match_pattern(binding))
            }
            MatchPattern::Struct { name, fields, .. } => {
                (self.hir.sum_type_for_variant(name).is_some()
                    || matches!(
                        self.hir.type_kind(name),
                        Some(HirTypeKind::Struct | HirTypeKind::Class)
                    ))
                    && fields.iter().all(|field| {
                        field.ignored
                            || field
                                .pattern
                                .as_deref()
                                .is_none_or(|pattern| self.is_supported_match_pattern(pattern))
                    })
            }
            MatchPattern::List { prefix, suffix, .. } => prefix
                .iter()
                .chain(suffix)
                .all(|pattern| self.is_supported_match_pattern(pattern)),
        }
    }

    /// Lower a plain (non-variant) struct pattern: there is no tag to test, so
    /// refutability comes only from nested field sub-patterns (e.g. literals).
    /// Each field is projected and either bound or recursively matched.
    fn lower_struct_field_patterns(
        &mut self,
        src: Reg,
        fields: &[MatchFieldPattern],
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let mut failures = Vec::new();
        for field in fields {
            if field.ignored {
                continue;
            }
            let field_reg = self.temp();
            self.emit(RegInstr::GetField {
                dst: field_reg,
                base: src,
                name: field.name.clone(),
            });
            if let Some(pattern) = field.pattern.as_deref() {
                failures.extend(self.lower_match_pattern(pattern, field_reg)?);
            } else if let Some(binding) = field.binding.as_ref() {
                let dst = self.local(binding);
                self.emit(RegInstr::Move {
                    dst,
                    src: field_reg,
                });
            } else {
                return Err(EvalError::Runtime(format!(
                    "reg VM struct pattern field `{}` has no binding or nested pattern.",
                    field.name
                )));
            }
        }
        Ok(failures)
    }

    /// Lower a `List<T>` slice pattern. Refutability is a length test (`==` for a
    /// fixed pattern, `>=` when a rest segment is present); elements are projected
    /// with `ListGet` and the rest segment (if bound) with `List.slice`.
    fn lower_list_pattern(
        &mut self,
        src: Reg,
        prefix: &[MatchPattern],
        rest: &Option<Option<String>>,
        suffix: &[MatchPattern],
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let mut failures = Vec::new();
        let required = (prefix.len() + suffix.len()) as i64;
        let len = self.temp();
        self.emit(RegInstr::ListLen {
            dst: len,
            list: src,
        });
        let bound = self.temp();
        self.emit(RegInstr::LoadInt {
            dst: bound,
            value: required,
        });
        // Fail (jump to the next arm) when the length constraint does not hold.
        // `RegIntCompare` has no `Equal`, so a fixed length is bracketed by `>=`
        // and `<=`; a rest pattern only needs the lower bound.
        let lower = self.emit(RegInstr::JumpIfIntCompare {
            lhs: len,
            rhs: bound,
            op: RegIntCompare::GreaterEqual,
            expected: false,
            target: usize::MAX,
        });
        failures.push(MatchFailurePatch::Jump(lower));
        if rest.is_none() {
            let upper = self.emit(RegInstr::JumpIfIntCompare {
                lhs: len,
                rhs: bound,
                op: RegIntCompare::LessEqual,
                expected: false,
                target: usize::MAX,
            });
            failures.push(MatchFailurePatch::Jump(upper));
        }
        for (index, pattern) in prefix.iter().enumerate() {
            let idx = self.temp();
            self.emit(RegInstr::LoadInt {
                dst: idx,
                value: index as i64,
            });
            let elem = self.temp();
            self.emit(RegInstr::ListGet {
                dst: elem,
                list: src,
                index: idx,
            });
            failures.extend(self.lower_match_pattern(pattern, elem)?);
        }
        if let Some(Some(rest_name)) = rest {
            let start = self.temp();
            self.emit(RegInstr::LoadInt {
                dst: start,
                value: prefix.len() as i64,
            });
            let slice_len = self.temp();
            self.emit(RegInstr::SubInt {
                dst: slice_len,
                lhs: len,
                rhs: bound,
            });
            let dst = self.local(rest_name);
            self.emit(RegInstr::CallIntrinsic {
                dst,
                intrinsic: RegIntrinsic::ListSlice,
                args: vec![src, start, slice_len],
            });
        }
        if !suffix.is_empty() {
            let suffix_count = self.temp();
            self.emit(RegInstr::LoadInt {
                dst: suffix_count,
                value: suffix.len() as i64,
            });
            let base = self.temp();
            self.emit(RegInstr::SubInt {
                dst: base,
                lhs: len,
                rhs: suffix_count,
            });
            for (offset, pattern) in suffix.iter().enumerate() {
                let offset_reg = self.temp();
                self.emit(RegInstr::LoadInt {
                    dst: offset_reg,
                    value: offset as i64,
                });
                let idx = self.temp();
                self.emit(RegInstr::AddInt {
                    dst: idx,
                    lhs: base,
                    rhs: offset_reg,
                });
                let elem = self.temp();
                self.emit(RegInstr::ListGet {
                    dst: elem,
                    list: src,
                    index: idx,
                });
                failures.extend(self.lower_match_pattern(pattern, elem)?);
            }
        }
        Ok(failures)
    }

    fn lower_literal_pattern(
        &mut self,
        src: Reg,
        literal: &MatchLiteral,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let expected = self.temp();
        match literal {
            MatchLiteral::Int(value) => {
                let value = value.parse::<i64>().map_err(|error| {
                    EvalError::Runtime(format!(
                        "reg VM could not parse match int literal `{value}`: {error}"
                    ))
                })?;
                self.emit(RegInstr::LoadInt {
                    dst: expected,
                    value,
                });
            }
            MatchLiteral::String(value) => {
                self.emit(RegInstr::LoadString {
                    dst: expected,
                    value: Rc::new(decode_string_token(value)),
                });
            }
            MatchLiteral::Bool(value) => {
                self.emit(RegInstr::LoadBool {
                    dst: expected,
                    value: *value,
                });
            }
        }
        let matches = self.temp();
        self.emit(RegInstr::Equal {
            dst: matches,
            lhs: src,
            rhs: expected,
        });
        let failure = self.emit(RegInstr::JumpIfBool {
            cond: matches,
            expected: false,
            target: usize::MAX,
        });
        Ok(vec![MatchFailurePatch::Jump(failure)])
    }

    fn lower_option_some_pattern(
        &mut self,
        src: Reg,
        binding: Option<&MatchPattern>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let match_ip = self.emit(RegInstr::MatchOption {
            src,
            some_ip: usize::MAX,
            none_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::OptionNone(match_ip)];
        if let Some(binding) = binding {
            match binding {
                MatchPattern::Binding { name, .. } => {
                    let dst = self.local(name);
                    self.emit(RegInstr::UnwrapSome { dst, src });
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapSome { dst: payload, src });
                    failures.extend(self.lower_match_pattern(binding, payload)?);
                }
            }
        }
        Ok(failures)
    }

    fn lower_result_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        binding: Option<&MatchPattern>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let match_ip = self.emit(RegInstr::MatchResult {
            src,
            ok_ip: usize::MAX,
            err_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        let mut failures = match variant {
            "Ok" => {
                self.patch_jump(match_ip, pass_ip);
                vec![MatchFailurePatch::ResultErr(match_ip)]
            }
            "Err" => {
                self.patch_result_match_err(match_ip, pass_ip);
                vec![MatchFailurePatch::ResultOk(match_ip)]
            }
            _ => unreachable!("result variant was validated before lowering"),
        };
        if let Some(binding) = binding {
            match binding {
                MatchPattern::Binding { name, .. } => {
                    let dst = self.local(name);
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst,
                        src,
                        expected: variant.to_string(),
                    });
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst: payload,
                        src,
                        expected: variant.to_string(),
                    });
                    failures.extend(self.lower_match_pattern(binding, payload)?);
                }
            }
        }
        Ok(failures)
    }

    fn lower_user_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        binding: Option<&MatchPattern>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let fields = self.hir.sum_variant_fields(variant).unwrap_or(&[]);
        let match_ip = self.emit(RegInstr::MatchVariant {
            src,
            expected: variant.to_string(),
            match_ip: usize::MAX,
            else_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::VariantOther(match_ip)];
        if let Some(binding) = binding {
            if fields.len() != 1 {
                return Err(EvalError::Runtime(format!(
                    "reg VM variant `{variant}` binding requires exactly one field, got {}.",
                    fields.len()
                )));
            }
            match binding {
                MatchPattern::Binding { name, .. } => {
                    let dst = self.local(name);
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst,
                        src,
                        expected: variant.to_string(),
                    });
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst: payload,
                        src,
                        expected: variant.to_string(),
                    });
                    failures.extend(self.lower_match_pattern(binding, payload)?);
                }
            }
        } else if !fields.is_empty() {
            return Err(EvalError::Runtime(format!(
                "reg VM variant `{variant}` pattern requires a payload binding."
            )));
        }
        Ok(failures)
    }

    fn lower_user_struct_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        fields: &[MatchFieldPattern],
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let match_ip = self.emit(RegInstr::MatchVariant {
            src,
            expected: variant.to_string(),
            match_ip: usize::MAX,
            else_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::VariantOther(match_ip)];
        for field in fields {
            if field.ignored {
                continue;
            }
            let field_reg = self.temp();
            self.emit(RegInstr::GetField {
                dst: field_reg,
                base: src,
                name: field.name.clone(),
            });
            if let Some(pattern) = field.pattern.as_deref() {
                failures.extend(self.lower_match_pattern(pattern, field_reg)?);
            } else if let Some(binding) = field.binding.as_ref() {
                let dst = self.local(binding);
                self.emit(RegInstr::Move {
                    dst,
                    src: field_reg,
                });
            } else {
                return Err(EvalError::Runtime(format!(
                    "reg VM struct variant pattern field `{}` has no binding or nested pattern.",
                    field.name
                )));
            }
        }
        Ok(failures)
    }

    fn patch_match_failures(&mut self, patches: Vec<MatchFailurePatch>, target: usize) {
        for patch in patches {
            match patch {
                MatchFailurePatch::Jump(ip) => self.patch_jump(ip, target),
                MatchFailurePatch::OptionSome(ip) | MatchFailurePatch::ResultOk(ip) => {
                    self.patch_jump(ip, target)
                }
                MatchFailurePatch::OptionNone(ip) => self.patch_match_none(ip, target),
                MatchFailurePatch::ResultErr(ip) => self.patch_result_match_err(ip, target),
                MatchFailurePatch::VariantOther(ip) => self.patch_variant_match_else(ip, target),
            }
        }
    }

    fn map_get_match(
        &mut self,
        value: &HirExpr,
        arms: &[crate::hir::HirMatchArm],
    ) -> Result<bool, EvalError> {
        let HirExpr::Call {
            callee: Callee::Qualified { namespace, name },
            args,
            receiver: None,
            ..
        } = value
        else {
            return Ok(false);
        };
        if type_root_name(namespace) != "Map" || type_root_name(name) != "get" || args.len() != 2 {
            return Ok(false);
        }
        if arms.len() != 2 {
            return Ok(false);
        }
        if arms.iter().any(|arm| arm.guard.is_some()) {
            return Ok(false);
        }

        let mut some_binding = None;
        let mut has_none = false;
        for arm in arms {
            match &arm.pattern {
                MatchPattern::Variant {
                    name,
                    binding: Some(binding),
                    ..
                } if name == "Some" => {
                    some_binding = binding.binding_names().into_iter().next();
                }
                MatchPattern::Variant { name, .. } if name == "None" => {
                    has_none = true;
                }
                _ => return Ok(false),
            }
        }
        let Some(some_binding) = some_binding else {
            return Ok(false);
        };
        if !has_none {
            return Ok(false);
        }

        let map = self.expr(&args[0].value)?;
        let key = self.expr(&args[1].value)?;
        let value_dst = self.local(&some_binding);
        let match_ip = self.emit(RegInstr::MatchMapGet {
            map,
            key,
            value_dst,
            some_ip: usize::MAX,
            none_ip: usize::MAX,
        });
        let mut some_ip = None;
        let mut none_ip = None;
        let mut end_jumps = Vec::new();
        for arm in arms {
            match &arm.pattern {
                MatchPattern::Variant { name, .. } if name == "Some" => {
                    let ip = self.function.code.len();
                    some_ip = Some(ip);
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
                }
                MatchPattern::Variant { name, .. } if name == "None" => {
                    let ip = self.function.code.len();
                    none_ip = Some(ip);
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
                }
                _ => unreachable!("Map.get match arms were validated before lowering"),
            }
        }
        let end_ip = self.function.code.len();
        self.patch_map_match_some(
            match_ip,
            some_ip.ok_or_else(|| {
                EvalError::Runtime("reg VM Map.get match is missing Some arm.".to_string())
            })?,
        );
        self.patch_map_match_none(
            match_ip,
            none_ip.ok_or_else(|| {
                EvalError::Runtime("reg VM Map.get match is missing None arm.".to_string())
            })?,
        );
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        Ok(true)
    }

    fn struct_match(
        &mut self,
        value: &HirExpr,
        arms: &[crate::hir::HirMatchArm],
    ) -> Result<bool, EvalError> {
        let [arm] = arms else {
            return Ok(false);
        };
        if arm.guard.is_some() {
            return Ok(false);
        }
        let MatchPattern::Struct { fields, .. } = &arm.pattern else {
            return Ok(false);
        };

        let src = self.expr(value)?;
        for field in fields {
            if field.ignored {
                continue;
            }
            let Some(binding) = field.binding.as_ref() else {
                return Ok(false);
            };
            if field.pattern.is_some() {
                return Ok(false);
            }
            let dst = self.local(binding);
            self.emit(RegInstr::GetField {
                dst,
                base: src,
                name: field.name.clone(),
            });
        }
        self.block(&arm.body)?;
        Ok(true)
    }
}

/// Pure name->intrinsic mapping for qualified/receiver calls. Returns the
/// stdlib `RegIntrinsic` for the simple `Ns.method` mappings, or `None` for
/// names that need inline lowering logic or fall through to native/dynamic
/// dispatch (handled by the caller's remaining match arms).
fn qualified_intrinsic(namespace: &str, name: &str) -> Option<RegIntrinsic> {
    match (namespace, name) {
        ("Args", "all") => Some(RegIntrinsic::ArgsAll),
        ("Args", "count") => Some(RegIntrinsic::ArgsCount),
        ("Args", "get") => Some(RegIntrinsic::ArgsGet),
        ("Args", "get_or_default") => Some(RegIntrinsic::ArgsGetOrDefault),
        ("Assert", "equal") => Some(RegIntrinsic::AssertEqual),
        ("Assert", "equal_bool") => Some(RegIntrinsic::AssertEqualBool),
        ("Assert", "equal_int") => Some(RegIntrinsic::AssertEqualInt),
        ("Base64", "decode") => Some(RegIntrinsic::Base64Decode),
        ("Base64", "decode_string") => Some(RegIntrinsic::Base64DecodeString),
        ("Base64", "encode") => Some(RegIntrinsic::Base64Encode),
        ("Base64", "encode_bytes") => Some(RegIntrinsic::Base64EncodeBytes),
        ("Bytes", "concat") => Some(RegIntrinsic::BytesConcat),
        ("Bytes", "consume") => Some(RegIntrinsic::BytesConsume),
        ("Bytes", "from_buffer") => Some(RegIntrinsic::BytesViewToBytes),
        ("Bytes", "from_string") => Some(RegIntrinsic::BytesFromString),
        ("Bytes", "from_uints") => Some(RegIntrinsic::BytesFromUints),
        ("Bytes", "is_empty") => Some(RegIntrinsic::BytesIsEmpty),
        ("Bytes", "len") => Some(RegIntrinsic::BytesLen),
        ("Bytes", "slice") | ("Bytes", "view") => Some(RegIntrinsic::BytesSlice),
        ("Bytes", "to_string") => Some(RegIntrinsic::BytesToString),
        ("Bytes", "to_uints") => Some(RegIntrinsic::BytesToUints),
        ("Buffer", "consume") => Some(RegIntrinsic::BytesConsume),
        ("Buffer", "is_empty") => Some(RegIntrinsic::BytesIsEmpty),
        ("Buffer", "len") => Some(RegIntrinsic::BytesLen),
        ("Buffer", "new") => Some(RegIntrinsic::BufferNew),
        ("Buffer", "view") => Some(RegIntrinsic::BytesSlice),
        ("BufferView", "is_empty") => Some(RegIntrinsic::BytesIsEmpty),
        ("BufferView", "len") => Some(RegIntrinsic::BytesLen),
        ("BufferView", "slice") => Some(RegIntrinsic::BytesSlice),
        ("BufferView", "to_bytes") => Some(RegIntrinsic::BytesViewToBytes),
        ("BytesView", "is_empty") => Some(RegIntrinsic::BytesIsEmpty),
        ("BytesView", "len") => Some(RegIntrinsic::BytesLen),
        ("BytesView", "slice") => Some(RegIntrinsic::BytesSlice),
        ("BytesView", "starts_with") => Some(RegIntrinsic::BytesViewStartsWith),
        ("BytesView", "to_bytes") => Some(RegIntrinsic::BytesViewToBytes),
        ("Cache", "get") => Some(RegIntrinsic::CacheGet),
        ("Cache", "lookup") => Some(RegIntrinsic::CacheLookup),
        ("Cache", "new") => Some(RegIntrinsic::MapNew),
        ("CancellationSource", "cancel") => Some(RegIntrinsic::CancellationSourceCancel),
        ("CancellationSource", "new") => Some(RegIntrinsic::CancellationSourceNew),
        ("CancellationSource", "token") => Some(RegIntrinsic::CancellationSourceToken),
        ("Channel", "bounded") => Some(RegIntrinsic::ChannelBounded),
        // A message channel reuses the bounded-channel runtime; the
        // cross-isolate payload contract is enforced at check time.
        ("Channel", "message") => Some(RegIntrinsic::ChannelBounded),
        ("Channel", "receiver") => Some(RegIntrinsic::ChannelReceiver),
        ("Channel", "sender") => Some(RegIntrinsic::ChannelSender),
        ("ChannelError", "message") => Some(RegIntrinsic::ChannelErrorMessage),
        ("Tensor", "from_f32_slice") => Some(RegIntrinsic::TensorFromF32Slice),
        ("Tensor", "to_f32_slice") => Some(RegIntrinsic::TensorToF32Slice),
        ("Tensor", "shape") => Some(RegIntrinsic::TensorShape),
        ("Tensor", "rank") => Some(RegIntrinsic::TensorRank),
        ("Tensor", "matmul") => Some(RegIntrinsic::TensorMatmul),
        ("Tensor", "add") => Some(RegIntrinsic::TensorAdd),
        ("Tensor", "sub") => Some(RegIntrinsic::TensorSub),
        ("Tensor", "mul") => Some(RegIntrinsic::TensorMul),
        ("Tensor", "div") => Some(RegIntrinsic::TensorDiv),
        ("Tensor", "neg") => Some(RegIntrinsic::TensorNeg),
        ("Tensor", "exp") => Some(RegIntrinsic::TensorExp),
        ("Tensor", "log") => Some(RegIntrinsic::TensorLog),
        ("Tensor", "sqrt") => Some(RegIntrinsic::TensorSqrt),
        ("Tensor", "relu") => Some(RegIntrinsic::TensorRelu),
        ("Tensor", "sum_all") => Some(RegIntrinsic::TensorSumAll),
        ("Tensor", "sum_axis") => Some(RegIntrinsic::TensorSumAxis),
        ("Tensor", "max_axis") => Some(RegIntrinsic::TensorMaxAxis),
        ("Tensor", "mean_axis") => Some(RegIntrinsic::TensorMeanAxis),
        ("Tensor", "argmax_axis") => Some(RegIntrinsic::TensorArgmaxAxis),
        ("Tensor", "reshape") => Some(RegIntrinsic::TensorReshape),
        ("Tensor", "transpose") => Some(RegIntrinsic::TensorTranspose),
        ("Tensor", "permute") => Some(RegIntrinsic::TensorPermute),
        ("Tensor", "broadcast_to") => Some(RegIntrinsic::TensorBroadcastTo),
        ("Tensor", "cmplt") => Some(RegIntrinsic::TensorCmplt),
        ("Tensor", "cmpne") => Some(RegIntrinsic::TensorCmpne),
        ("Tensor", "cmpeq") => Some(RegIntrinsic::TensorCmpeq),
        ("Tensor", "select") => Some(RegIntrinsic::TensorSelect),
        ("Tensor", "maximum") => Some(RegIntrinsic::TensorMaximum),
        ("Tensor", "minimum") => Some(RegIntrinsic::TensorMinimum),
        ("Tensor", "cast_f32") => Some(RegIntrinsic::TensorCastF32),
        ("Tensor", "cast_i32") => Some(RegIntrinsic::TensorCastI32),
        ("Tensor", "cast_bool") => Some(RegIntrinsic::TensorCastBool),
        ("Tensor", "dtype_code") => Some(RegIntrinsic::TensorDtypeCode),
        // movement+gather (ops B)
        ("Tensor", "pad") => Some(RegIntrinsic::TensorPad),
        ("Tensor", "shrink") => Some(RegIntrinsic::TensorShrink),
        ("Tensor", "flip") => Some(RegIntrinsic::TensorFlip),
        ("Tensor", "gather") => Some(RegIntrinsic::TensorGather),
        // reductions+math (ops C)
        ("Tensor", "prod_axis") => Some(RegIntrinsic::TensorProdAxis),
        ("Tensor", "min_axis") => Some(RegIntrinsic::TensorMinAxis),
        ("Tensor", "sum_axes") => Some(RegIntrinsic::TensorSumAxes),
        ("Tensor", "prod_axes") => Some(RegIntrinsic::TensorProdAxes),
        ("Tensor", "max_axes") => Some(RegIntrinsic::TensorMaxAxes),
        ("Tensor", "min_axes") => Some(RegIntrinsic::TensorMinAxes),
        ("Tensor", "mean_axes") => Some(RegIntrinsic::TensorMeanAxes),
        ("Tensor", "reciprocal") => Some(RegIntrinsic::TensorReciprocal),
        ("Tensor", "exp2") => Some(RegIntrinsic::TensorExp2),
        ("Tensor", "log2") => Some(RegIntrinsic::TensorLog2),
        ("Tensor", "rsqrt") => Some(RegIntrinsic::TensorRsqrt),
        ("Tensor", "pow") => Some(RegIntrinsic::TensorPow),
        // bmm+int/bit (ops D)
        ("Tensor", "bmm") => Some(RegIntrinsic::TensorBmm),
        ("Tensor", "idiv") => Some(RegIntrinsic::TensorIdiv),
        ("Tensor", "modulo") => Some(RegIntrinsic::TensorMod),
        ("Tensor", "shl") => Some(RegIntrinsic::TensorShl),
        ("Tensor", "shr") => Some(RegIntrinsic::TensorShr),
        ("Tensor", "bit_and") => Some(RegIntrinsic::TensorAnd),
        ("Tensor", "bit_or") => Some(RegIntrinsic::TensorOr),
        ("Tensor", "bit_xor") => Some(RegIntrinsic::TensorXor),
        ("Tensor", "bitcast_f32_to_i32") => Some(RegIntrinsic::TensorBitcastF32ToI32),
        ("Tensor", "bitcast_i32_to_f32") => Some(RegIntrinsic::TensorBitcastI32ToF32),
        // nn (slice F)
        ("Tensor", "iota") => Some(RegIntrinsic::TensorIota),
        ("Tensor", "one_hot") => Some(RegIntrinsic::TensorOneHot),
        ("Tensor", "softmax") => Some(RegIntrinsic::TensorSoftmax),
        ("Tensor", "log_softmax") => Some(RegIntrinsic::TensorLogSoftmax),
        ("Tensor", "cross_entropy") => Some(RegIntrinsic::TensorCrossEntropy),
        ("TensorError", "message") => Some(RegIntrinsic::TensorErrorMessage),
        ("Char", "compare") => Some(RegIntrinsic::CharCompare),
        ("Char", "from_code") => Some(RegIntrinsic::CharFromCode),
        ("Char", "is_alphanumeric") => Some(RegIntrinsic::CharIsAlphanumeric),
        ("Char", "is_alpha") => Some(RegIntrinsic::CharIsAlpha),
        ("Char", "is_digit") => Some(RegIntrinsic::CharIsDigit),
        ("Char", "is_lower") => Some(RegIntrinsic::CharIsLower),
        ("Char", "is_upper") => Some(RegIntrinsic::CharIsUpper),
        ("Char", "is_whitespace") => Some(RegIntrinsic::CharIsWhitespace),
        ("Char", "to_code") => Some(RegIntrinsic::CharToCode),
        ("Char", "to_lower") => Some(RegIntrinsic::CharToLower),
        ("Char", "to_string") => Some(RegIntrinsic::CharToString),
        ("Char", "to_upper") => Some(RegIntrinsic::CharToUpper),
        ("Clock", "now") => Some(RegIntrinsic::ClockNow),
        ("Clock", "system_unix_ms") => Some(RegIntrinsic::ClockSystemUnixMs),
        ("Config", "load") => Some(RegIntrinsic::ConfigLoad),
        ("Capability", "from") => Some(RegIntrinsic::CapabilityFrom),
        ("Config", "name") => Some(RegIntrinsic::ConfigName),
        ("Config", "new") => Some(RegIntrinsic::ConfigNew),
        ("Config", "rule_count") => Some(RegIntrinsic::ConfigRuleCount),
        ("ConfigStore", "name") => Some(RegIntrinsic::ConfigStoreName),
        ("ConfigStore", "new") => Some(RegIntrinsic::ConfigStoreNew),
        ("Counter", "new") => Some(RegIntrinsic::CounterNew),
        ("Counter", "value") => Some(RegIntrinsic::CounterValue),
        ("Csv", "open_read") => Some(RegIntrinsic::CsvOpenRead),
        ("Csv", "parse_row") => Some(RegIntrinsic::CsvParseRow),
        ("Csv", "read_into") => Some(RegIntrinsic::CsvReadInto),
        ("Csv", "rows") => Some(RegIntrinsic::CsvRows),
        ("Deadline", "after") => Some(RegIntrinsic::DeadlineAfter),
        ("Deadline", "after_ms") => Some(RegIntrinsic::DeadlineAfterMs),
        ("Deadline", "is_expired") => Some(RegIntrinsic::DeadlineIsExpired),
        ("Deadline", "remaining_ms") => Some(RegIntrinsic::DeadlineRemainingMs),
        ("DecodeError", "message") => Some(RegIntrinsic::DecodeErrorMessage),
        ("Deque", "is_empty") => Some(RegIntrinsic::DequeIsEmpty),
        ("Deque", "len") => Some(RegIntrinsic::DequeLen),
        ("Deque", "new") => Some(RegIntrinsic::DequeNew),
        ("Deque", "to_list") => Some(RegIntrinsic::DequeToList),
        ("Diff", "unified") => Some(RegIntrinsic::DiffUnified),
        ("Directory", "copy_file") => Some(RegIntrinsic::DirectoryCopyFile),
        ("Directory", "create") => Some(RegIntrinsic::DirectoryCreate),
        ("Directory", "create_all") => Some(RegIntrinsic::DirectoryCreateAll),
        ("Directory", "create_dir_all") => Some(RegIntrinsic::DirectoryCreateDirAll),
        ("Directory", "exists") => Some(RegIntrinsic::DirectoryExists),
        ("Directory", "is_dir") => Some(RegIntrinsic::DirectoryIsDir),
        ("Directory", "is_file") => Some(RegIntrinsic::DirectoryIsFile),
        ("Directory", "list_files") => Some(RegIntrinsic::DirectoryListFiles),
        ("Directory", "list_paths") => Some(RegIntrinsic::DirectoryListPaths),
        ("Directory", "metadata") => Some(RegIntrinsic::DirectoryMetadata),
        ("Directory", "read_string") => Some(RegIntrinsic::DirectoryReadString),
        ("Directory", "remove_dir_all") => Some(RegIntrinsic::DirectoryRemoveDirAll),
        ("Directory", "remove_file") => Some(RegIntrinsic::DirectoryRemoveFile),
        ("Directory", "rename") => Some(RegIntrinsic::DirectoryRename),
        ("Directory", "write_string") => Some(RegIntrinsic::DirectoryWriteString),
        ("Db", "close") => Some(RegIntrinsic::DbClose),
        ("DbConnection", "open") => Some(RegIntrinsic::DbConnectionOpen),
        ("DbConnection", "query") => Some(RegIntrinsic::DbConnectionQuery),
        ("DbConnection", "try_open") => Some(RegIntrinsic::DbConnectionTryOpen),
        ("Date", "add_days") => Some(RegIntrinsic::DateAddDays),
        ("Date", "add_ms") => Some(RegIntrinsic::DateAddMs),
        ("Date", "day") => Some(RegIntrinsic::DateDay),
        ("Date", "days_between") => Some(RegIntrinsic::DateDaysBetween),
        ("Date", "days_in_month") => Some(RegIntrinsic::DateDaysInMonth),
        ("Date", "format_iso") => Some(RegIntrinsic::DateFormatIso),
        ("Date", "format_ymd") => Some(RegIntrinsic::DateFormatYmd),
        ("Date", "hour") => Some(RegIntrinsic::DateHour),
        ("Date", "is_leap_year") => Some(RegIntrinsic::DateIsLeapYear),
        ("Date", "minute") => Some(RegIntrinsic::DateMinute),
        ("Date", "month") => Some(RegIntrinsic::DateMonth),
        ("Date", "parse_iso") => Some(RegIntrinsic::DateParseIso),
        ("Date", "parse_ymd") => Some(RegIntrinsic::DateParseYmd),
        ("Date", "second") => Some(RegIntrinsic::DateSecond),
        ("Date", "start_of_day") => Some(RegIntrinsic::DateStartOfDay),
        ("Date", "weekday") => Some(RegIntrinsic::DateWeekday),
        ("Date", "year") => Some(RegIntrinsic::DateYear),
        ("Duration", "add") => Some(RegIntrinsic::DurationAdd),
        ("Duration", "as_ms") => Some(RegIntrinsic::DurationAsMs),
        ("Duration", "as_seconds") => Some(RegIntrinsic::DurationAsSeconds),
        ("Duration", "ms") => Some(RegIntrinsic::DurationMs),
        ("Duration", "seconds") => Some(RegIntrinsic::DurationSeconds),
        ("Environment", "bind_function") => Some(RegIntrinsic::EnvironmentBindFunction),
        ("Environment", "child") => Some(RegIntrinsic::EnvironmentChild),
        ("Environment", "has_function") => Some(RegIntrinsic::EnvironmentHasFunction),
        ("Environment", "has_parent") => Some(RegIntrinsic::EnvironmentHasParent),
        ("Environment", "root") => Some(RegIntrinsic::EnvironmentRoot),
        ("Env", "current_dir") => Some(RegIntrinsic::EnvCurrentDir),
        ("Env", "get") => Some(RegIntrinsic::EnvGet),
        ("Env", "get_or_default") => Some(RegIntrinsic::EnvGetOrDefault),
        ("Env", "home_dir") => Some(RegIntrinsic::EnvHomeDir),
        ("Env", "run_workspace_root") => Some(RegIntrinsic::EnvRunWorkspaceRoot),
        ("Env", "set") => Some(RegIntrinsic::EnvSet),
        ("Env", "set_current_dir") => Some(RegIntrinsic::EnvSetCurrentDir),
        ("Env", "temp_dir") => Some(RegIntrinsic::EnvTempDir),
        ("File", "append_bytes") => Some(RegIntrinsic::FileAppendBytes),
        ("File", "append_string") => Some(RegIntrinsic::FileAppendString),
        ("File", "bytes_stream") => Some(RegIntrinsic::FileBytesStream),
        ("File", "exists") => Some(RegIntrinsic::FileExists),
        ("File", "open") => Some(RegIntrinsic::FileOpen),
        ("File", "open_read") => Some(RegIntrinsic::FileOpenRead),
        ("File", "open_write") => Some(RegIntrinsic::FileOpenWrite),
        ("File", "read_all") => Some(RegIntrinsic::FileReadAll),
        ("File", "read_all_async") => Some(RegIntrinsic::FileReadAllAsync),
        ("File", "read_all_string") => Some(RegIntrinsic::FileReadAllString),
        ("File", "read_all_string_async") => Some(RegIntrinsic::FileReadAllStringAsync),
        ("File", "read_bytes") => Some(RegIntrinsic::FileReadBytes),
        ("File", "read_into") => Some(RegIntrinsic::FileReadInto),
        ("File", "read_string") => Some(RegIntrinsic::FileReadString),
        ("File", "remove") => Some(RegIntrinsic::FileRemove),
        ("File", "write") => Some(RegIntrinsic::FileWrite),
        ("File", "write_async") => Some(RegIntrinsic::FileWriteAsync),
        ("File", "write_atomic") => Some(RegIntrinsic::FileWriteAtomic),
        ("File", "write_bytes") => Some(RegIntrinsic::FileWriteBytes),
        ("File", "write_bytes_view") => Some(RegIntrinsic::FileWriteBytesView),
        ("File", "write_buffer") => Some(RegIntrinsic::FileWriteBuffer),
        ("File", "write_buffer_view") => Some(RegIntrinsic::FileWriteBufferView),
        ("File", "write_string") => Some(RegIntrinsic::FileWriteString),
        ("File", "write_string_async") => Some(RegIntrinsic::FileWriteStringAsync),
        ("File", "write_string_to_path") => Some(RegIntrinsic::FileWriteStringToPath),
        ("FalliblePipeline", "collect") => Some(RegIntrinsic::FalliblePipelineCollect),
        ("FalliblePipeline", "each") => Some(RegIntrinsic::FalliblePipelineEach),
        ("FalliblePipeline", "filter") => Some(RegIntrinsic::FalliblePipelineFilter),
        ("FalliblePipeline", "map") => Some(RegIntrinsic::FalliblePipelineMap),
        ("FalliblePipeline", "try_map") => Some(RegIntrinsic::FalliblePipelineTryMap),
        ("FileError", "message") => Some(RegIntrinsic::FileErrorMessage),
        ("FunctionObject", "has_closure") => Some(RegIntrinsic::FunctionObjectHasClosure),
        ("FunctionObject", "new") => Some(RegIntrinsic::FunctionObjectNew),
        ("Hash", "sha256_bytes") => Some(RegIntrinsic::HashSha256Bytes),
        ("Hash", "sha256_file") => Some(RegIntrinsic::HashSha256File),
        ("Hash", "sha256_string") => Some(RegIntrinsic::HashSha256String),
        ("Hash", "sha3_224_bytes") => Some(RegIntrinsic::HashSha3_224Bytes),
        ("Hash", "sha3_256_bytes") => Some(RegIntrinsic::HashSha3_256Bytes),
        ("Hash", "shake128_bytes") => Some(RegIntrinsic::HashShake128Bytes),
        ("Hmac", "sha256_bytes") => Some(RegIntrinsic::HmacSha256Bytes),
        ("Hmac", "sha256_string") => Some(RegIntrinsic::HmacSha256String),
        ("GlobalConfig", "new") => Some(RegIntrinsic::GlobalConfigNew),
        ("GlobalConfig", "rule_count") => Some(RegIntrinsic::GlobalConfigRuleCount),
        ("Gzip", "decompress_bytes") => Some(RegIntrinsic::GzipDecompressBytes),
        ("Hex", "decode") => Some(RegIntrinsic::HexDecode),
        ("Hex", "encode") => Some(RegIntrinsic::HexEncode),
        ("Hex", "encode_string") => Some(RegIntrinsic::HexEncodeString),
        ("HttpError", "message") => Some(RegIntrinsic::HttpErrorMessage),
        ("Http", "get") => Some(RegIntrinsic::HttpGet),
        ("Http", "get_async") => Some(RegIntrinsic::HttpGetAsync),
        ("Http", "get_retry_async") => Some(RegIntrinsic::HttpGetRetryAsync),
        ("Http", "get_timeout_async") => Some(RegIntrinsic::HttpGetTimeoutAsync),
        ("Http", "post_form") => Some(RegIntrinsic::HttpPostForm),
        ("Http", "post_form_async") => Some(RegIntrinsic::HttpPostFormAsync),
        ("Http", "post_json") => Some(RegIntrinsic::HttpPostJson),
        ("Http", "post_json_async") => Some(RegIntrinsic::HttpPostJsonAsync),
        ("Http", "post_json_retry_async") => Some(RegIntrinsic::HttpPostJsonRetryAsync),
        ("Http", "post_json_timeout_async") => Some(RegIntrinsic::HttpPostJsonTimeoutAsync),
        ("Http", "send_async") => Some(RegIntrinsic::HttpSendAsync),
        ("HttpRequest", "json") => Some(RegIntrinsic::HttpRequestJson),
        ("HttpRequest", "with_header") => Some(RegIntrinsic::HttpRequestWithHeader),
        ("HttpRequest", "with_retry") => Some(RegIntrinsic::HttpRequestWithRetry),
        ("HttpRequest", "with_timeout") => Some(RegIntrinsic::HttpRequestWithTimeout),
        ("HttpResponse", "bytes") => Some(RegIntrinsic::HttpResponseBytes),
        ("HttpResponse", "is_success") => Some(RegIntrinsic::HttpResponseIsSuccess),
        ("HttpResponse", "lines") => Some(RegIntrinsic::HttpResponseLines),
        ("HttpResponse", "status") => Some(RegIntrinsic::HttpResponseStatus),
        ("HttpResponse", "text") => Some(RegIntrinsic::HttpResponseText),
        ("Image", "inspect") => Some(RegIntrinsic::ImageInspect),
        ("Image", "load") => Some(RegIntrinsic::ImageLoad),
        ("Image", "normalize") => Some(RegIntrinsic::ImageNormalize),
        ("Image", "resize") => Some(RegIntrinsic::ImageResize),
        ("Image", "save") => Some(RegIntrinsic::ImageSave),
        ("Image", "sharpen") => Some(RegIntrinsic::ImageSharpen),
        ("Instant", "elapsed") => Some(RegIntrinsic::InstantElapsed),
        ("Float", "is_finite") => Some(RegIntrinsic::FloatIsFinite),
        ("Float", "is_infinite") => Some(RegIntrinsic::FloatIsInfinite),
        ("Float", "is_nan") => Some(RegIntrinsic::FloatIsNan),
        ("Float", "to_string") => Some(RegIntrinsic::FloatToString),
        ("Int", "bit_and") => Some(RegIntrinsic::IntBitAnd),
        ("Int", "bit_not") => Some(RegIntrinsic::IntBitNot),
        ("Int", "bit_or") => Some(RegIntrinsic::IntBitOr),
        ("Int", "bit_xor") => Some(RegIntrinsic::IntBitXor),
        ("Int", "shift_left") => Some(RegIntrinsic::IntShiftLeft),
        ("Int", "shift_right") => Some(RegIntrinsic::IntShiftRight),
        ("Int", "to_string") => Some(RegIntrinsic::IntToString),
        ("Int", "to_float") => Some(RegIntrinsic::IntToFloat),
        ("Math", "abs") => Some(RegIntrinsic::MathAbs),
        ("Math", "abs_float") => Some(RegIntrinsic::MathAbsFloat),
        ("Math", "ceil") => Some(RegIntrinsic::MathCeil),
        ("Math", "clamp") => Some(RegIntrinsic::MathClamp),
        ("Math", "clamp_float") => Some(RegIntrinsic::MathClampFloat),
        ("Math", "cos") => Some(RegIntrinsic::MathCos),
        ("Math", "exp") => Some(RegIntrinsic::MathExp),
        ("Math", "exp2") => Some(RegIntrinsic::MathExp2),
        ("Math", "floor") => Some(RegIntrinsic::MathFloor),
        ("Math", "log") => Some(RegIntrinsic::MathLog),
        ("Math", "log2") => Some(RegIntrinsic::MathLog2),
        ("Math", "max") => Some(RegIntrinsic::MathMax),
        ("Math", "max_float") => Some(RegIntrinsic::MathMaxFloat),
        ("Math", "min") => Some(RegIntrinsic::MathMin),
        ("Math", "min_float") => Some(RegIntrinsic::MathMinFloat),
        ("Math", "pow") => Some(RegIntrinsic::MathPow),
        ("Math", "pow_float") => Some(RegIntrinsic::MathPowFloat),
        ("Math", "round") => Some(RegIntrinsic::MathRound),
        ("Math", "sin") => Some(RegIntrinsic::MathSin),
        ("Math", "sqrt") => Some(RegIntrinsic::MathSqrt),
        ("Math", "tanh") => Some(RegIntrinsic::MathTanh),
        ("Math", "trunc_float") => Some(RegIntrinsic::MathTruncFloat),
        ("Json", "array") => Some(RegIntrinsic::JsonArray),
        ("Json", "array_bools") => Some(RegIntrinsic::JsonArrayBools),
        ("Json", "array_contains_prefix") => Some(RegIntrinsic::JsonArrayContainsPrefix),
        ("Json", "array_contains_string") => Some(RegIntrinsic::JsonArrayContainsString),
        ("Json", "array_count_where") => Some(RegIntrinsic::JsonArrayCountWhere),
        ("Json", "array_fold") => Some(RegIntrinsic::JsonArrayFold),
        ("Json", "array_get") => Some(RegIntrinsic::JsonArrayGet),
        ("Json", "array_ints") => Some(RegIntrinsic::JsonArrayInts),
        ("Json", "array_len") => Some(RegIntrinsic::JsonArrayLen),
        ("Json", "array_strings") => Some(RegIntrinsic::JsonArrayStrings),
        ("Json", "at") | ("Json", "value_at") => Some(RegIntrinsic::JsonAt),
        ("Json", "at_bool") => Some(RegIntrinsic::JsonAtBool),
        ("Json", "at_bool_or") => Some(RegIntrinsic::JsonAtBoolOr),
        ("Json", "at_int") => Some(RegIntrinsic::JsonAtInt),
        ("Json", "at_int_or") => Some(RegIntrinsic::JsonAtIntOr),
        ("Json", "at_optional") => Some(RegIntrinsic::JsonAtOptional),
        ("Json", "at_optional_bool") => Some(RegIntrinsic::JsonAtOptionalBool),
        ("Json", "at_optional_int") => Some(RegIntrinsic::JsonAtOptionalInt),
        ("Json", "at_optional_string") => Some(RegIntrinsic::JsonAtOptionalString),
        ("Json", "at_or") => Some(RegIntrinsic::JsonAtOr),
        ("Json", "at_string") => Some(RegIntrinsic::JsonAtString),
        ("Json", "at_string_or") => Some(RegIntrinsic::JsonAtStringOr),
        ("Json", "at_to_string") => Some(RegIntrinsic::JsonAtToString),
        ("Json", "at_to_string_or") => Some(RegIntrinsic::JsonAtToStringOr),
        ("Json", "as_bool") => Some(RegIntrinsic::JsonAsBool),
        ("Json", "as_int") => Some(RegIntrinsic::JsonAsInt),
        ("Json", "as_string") => Some(RegIntrinsic::JsonAsString),
        ("Json", "bool_at") => Some(RegIntrinsic::JsonBoolAt),
        ("Json", "bool_field") => Some(RegIntrinsic::JsonBoolField),
        ("Json", "clone") => Some(RegIntrinsic::JsonClone),
        ("Json", "decode") => Some(RegIntrinsic::JsonDecode),
        ("Json", "decode_text") => Some(RegIntrinsic::JsonDecodeText),
        ("Json", "encode") => Some(RegIntrinsic::JsonEncode),
        ("Json", "field") => Some(RegIntrinsic::JsonField),
        ("Json", "field_bool") => Some(RegIntrinsic::JsonFieldBool),
        ("Json", "field_int") => Some(RegIntrinsic::JsonFieldInt),
        ("Json", "field_optional") => Some(RegIntrinsic::JsonFieldOptional),
        ("Json", "field_optional_bool") => Some(RegIntrinsic::JsonFieldOptionalBool),
        ("Json", "field_optional_int") => Some(RegIntrinsic::JsonFieldOptionalInt),
        ("Json", "field_optional_string") => Some(RegIntrinsic::JsonFieldOptionalString),
        ("Json", "field_string") => Some(RegIntrinsic::JsonFieldString),
        ("Json", "int_at") => Some(RegIntrinsic::JsonIntAt),
        ("Json", "int_at_or") | ("Json", "json_int_at_or") => Some(RegIntrinsic::JsonIntAtOr),
        ("Json", "is_array") => Some(RegIntrinsic::JsonIsArray),
        ("Json", "is_null") => Some(RegIntrinsic::JsonIsNull),
        ("Json", "is_object") => Some(RegIntrinsic::JsonIsObject),
        ("Json", "int_field") => Some(RegIntrinsic::JsonIntField),
        ("Json", "kind") => Some(RegIntrinsic::JsonKind),
        ("Json", "object") => Some(RegIntrinsic::JsonObject),
        ("Json", "json_parse") | ("Json", "parse") => Some(RegIntrinsic::JsonParse),
        ("Json", "parse_file") => Some(RegIntrinsic::JsonParseFile),
        ("Json", "object_keys") => Some(RegIntrinsic::JsonObjectKeys),
        ("Json", "object_len") => Some(RegIntrinsic::JsonObjectLen),
        ("Json", "quote_string") => Some(RegIntrinsic::JsonQuoteString),
        ("Json", "raw_field") => Some(RegIntrinsic::JsonRawField),
        ("Json", "string_at") => Some(RegIntrinsic::JsonStringAt),
        ("Json", "string_array") => Some(RegIntrinsic::JsonStringArray),
        ("Json", "string_field") => Some(RegIntrinsic::JsonStringField),
        ("Json", "strings") => Some(RegIntrinsic::JsonStrings),
        ("Json", "to_string_at") => Some(RegIntrinsic::JsonToStringAt),
        ("Json", "to_string_at_or") => Some(RegIntrinsic::JsonToStringAtOr),
        ("Json", "to_string") => Some(RegIntrinsic::JsonToString),
        ("Json", "value") => Some(RegIntrinsic::JsonValue),
        ("Json", "values") => Some(RegIntrinsic::JsonValues),
        ("JsonError", "message") => Some(RegIntrinsic::JsonErrorMessage),
        ("List", "all") => Some(RegIntrinsic::ListAll),
        ("List", "any") => Some(RegIntrinsic::ListAny),
        ("List", "contains") => Some(RegIntrinsic::ListContains),
        ("List", "contains_value") => Some(RegIntrinsic::ListContainsValue),
        ("List", "count_where") => Some(RegIntrinsic::ListCountWhere),
        ("List", "consume") => Some(RegIntrinsic::ListConsume),
        ("List", "find") => Some(RegIntrinsic::ListFind),
        ("List", "flat_map") => Some(RegIntrinsic::ListFlatMap),
        ("List", "flatten") => Some(RegIntrinsic::ListFlatten),
        ("List", "first") => Some(RegIntrinsic::ListFirst),
        ("List", "is_empty") => Some(RegIntrinsic::ListIsEmpty),
        ("List", "join") => Some(RegIntrinsic::ListJoin),
        ("List", "group_by") => Some(RegIntrinsic::ListGroupBy),
        ("List", "last") => Some(RegIntrinsic::ListLast),
        ("List", "dedup") => Some(RegIntrinsic::ListDedup),
        ("List", "enumerate") => Some(RegIntrinsic::ListEnumerate),
        ("List", "max") => Some(RegIntrinsic::ListMax),
        ("List", "min") => Some(RegIntrinsic::ListMin),
        ("List", "new") => Some(RegIntrinsic::ListNew),
        ("List", "partition") => Some(RegIntrinsic::ListPartition),
        ("List", "pipeline") => Some(RegIntrinsic::ListPipeline),
        ("List", "reverse") => Some(RegIntrinsic::ListReverse),
        ("List", "skip") => Some(RegIntrinsic::ListSkip),
        ("List", "slice") => Some(RegIntrinsic::ListSlice),
        ("List", "sum") => Some(RegIntrinsic::ListSum),
        ("List", "zip") => Some(RegIntrinsic::ListZip),
        ("List", "take") => Some(RegIntrinsic::ListTake),
        ("List", "to_json_strings") => Some(RegIntrinsic::ListToJsonStrings),
        ("List", "to_json_values") => Some(RegIntrinsic::ListToJsonValues),
        ("List", "try_fold") => Some(RegIntrinsic::ListTryFold),
        ("Log", "error") => Some(RegIntrinsic::LogError),
        ("Log", "error_json") => Some(RegIntrinsic::LogErrorJson),
        ("Log", "trace") => Some(RegIntrinsic::LogTrace),
        ("Log", "write") => Some(RegIntrinsic::LogWrite),
        ("Log", "write_json") => Some(RegIntrinsic::LogWriteJson),
        ("Map", "contains_key") => Some(RegIntrinsic::MapContainsKey),
        ("Map", "filter") => Some(RegIntrinsic::MapFilter),
        ("Map", "fold") => Some(RegIntrinsic::MapFold),
        ("Map", "for_each") => Some(RegIntrinsic::MapForEach),
        ("Map", "get_or_default") => Some(RegIntrinsic::MapGetOrDefault),
        ("Map", "is_empty") => Some(RegIntrinsic::MapIsEmpty),
        ("Map", "keys") => Some(RegIntrinsic::MapKeys),
        ("Map", "len") => Some(RegIntrinsic::MapLen),
        ("Map", "map_values") => Some(RegIntrinsic::MapMapValues),
        ("Map", "merge") => Some(RegIntrinsic::MapMerge),
        ("Map", "new") => Some(RegIntrinsic::MapNew),
        ("Map", "try_fold") => Some(RegIntrinsic::MapTryFold),
        ("Map", "values") => Some(RegIntrinsic::MapValues),
        ("Option", "and_then") => Some(RegIntrinsic::OptionAndThen),
        ("Option", "filter") => Some(RegIntrinsic::OptionFilter),
        ("Option", "is_none") => Some(RegIntrinsic::OptionIsNone),
        ("Option", "is_some") => Some(RegIntrinsic::OptionIsSome),
        ("Option", "map") => Some(RegIntrinsic::OptionMap),
        ("Option", "ok_or") => Some(RegIntrinsic::OptionOkOr),
        ("Option", "or") => Some(RegIntrinsic::OptionOr),
        ("Option", "unwrap_or") => Some(RegIntrinsic::OptionUnwrapOr),
        ("Option", "unwrap_or_else") => Some(RegIntrinsic::OptionUnwrapOrElse),
        ("Clone", "clone") => Some(RegIntrinsic::CloneClone),
        ("Ord", "compare") => Some(RegIntrinsic::OrdCompare),
        ("OS", "close") => Some(RegIntrinsic::OsClose),
        ("Patch", "apply_text") => Some(RegIntrinsic::PatchApplyText),
        ("Path", "exists") => Some(RegIntrinsic::PathExists),
        ("Path", "extension") => Some(RegIntrinsic::PathExtension),
        ("Path", "file_name") => Some(RegIntrinsic::PathFileName),
        ("Path", "from_string") => Some(RegIntrinsic::PathFromString),
        ("Path", "is_absolute") => Some(RegIntrinsic::PathIsAbsolute),
        ("Path", "is_dir") => Some(RegIntrinsic::PathIsDir),
        ("Path", "is_file") => Some(RegIntrinsic::PathIsFile),
        ("Path", "join") => Some(RegIntrinsic::PathJoin),
        ("Path", "list_files") => Some(RegIntrinsic::PathListFiles),
        ("Path", "list_paths") => Some(RegIntrinsic::PathListPaths),
        ("Path", "normalize") => Some(RegIntrinsic::PathNormalize),
        ("Path", "parent") => Some(RegIntrinsic::PathParent),
        ("Path", "read_string") => Some(RegIntrinsic::PathReadString),
        ("Path", "resolve_relative") => Some(RegIntrinsic::PathResolveRelative),
        ("Path", "safe_relative") => Some(RegIntrinsic::PathSafeRelative),
        ("Path", "starts_with") => Some(RegIntrinsic::PathStartsWith),
        ("Path", "to_string") => Some(RegIntrinsic::PathToString),
        ("Path", "with_extension") => Some(RegIntrinsic::PathWithExtension),
        ("Path", "write_string") => Some(RegIntrinsic::PathWriteString),
        ("PersistentMap", "clear") => Some(RegIntrinsic::PersistentMapClear),
        ("PersistentMap", "contains_key") => Some(RegIntrinsic::PersistentMapContainsKey),
        ("PersistentMap", "get") => Some(RegIntrinsic::PersistentMapGet),
        ("PersistentMap", "insert") => Some(RegIntrinsic::PersistentMapInsert),
        ("PersistentMap", "is_empty") => Some(RegIntrinsic::PersistentMapIsEmpty),
        ("PersistentMap", "len") => Some(RegIntrinsic::PersistentMapLen),
        ("PersistentMap", "new") => Some(RegIntrinsic::PersistentMapNew),
        ("PersistentMap", "remove") => Some(RegIntrinsic::PersistentMapRemove),
        ("Pipeline", "collect") => Some(RegIntrinsic::PipelineCollect),
        ("Pipeline", "each") => Some(RegIntrinsic::PipelineEach),
        ("Pipeline", "try_map") => Some(RegIntrinsic::PipelineTryMap),
        ("PoolError", "message") => Some(RegIntrinsic::PoolErrorMessage),
        ("PoolStats", "available") => Some(RegIntrinsic::PoolStatsAvailable),
        ("PoolStats", "capacity") => Some(RegIntrinsic::PoolStatsCapacity),
        ("PoolStats", "created") => Some(RegIntrinsic::PoolStatsCreated),
        ("PoolStats", "in_use") => Some(RegIntrinsic::PoolStatsInUse),
        ("Process", "run") => Some(RegIntrinsic::ProcessRun),
        ("Process", "run_async") => Some(RegIntrinsic::ProcessRunAsync),
        ("Process", "run_many_stdout") => Some(RegIntrinsic::ProcessRunManyStdout),
        ("Process", "run_many_stdout_async") => Some(RegIntrinsic::ProcessRunManyStdoutAsync),
        ("Process", "run_request") => Some(RegIntrinsic::ProcessRunRequest),
        ("Process", "run_request_async") => Some(RegIntrinsic::ProcessRunRequestAsync),
        ("Process", "run_stdout") => Some(RegIntrinsic::ProcessRunStdout),
        ("Process", "run_stdout_async") => Some(RegIntrinsic::ProcessRunStdoutAsync),
        ("Process", "run_stdout_timeout") => Some(RegIntrinsic::ProcessRunStdoutTimeout),
        ("Process", "run_timeout") => Some(RegIntrinsic::ProcessRunTimeout),
        ("Process", "run_timeout_async") => Some(RegIntrinsic::ProcessRunTimeoutAsync),
        ("Process", "stream") => Some(RegIntrinsic::ProcessStream),
        ("Random", "bool") => Some(RegIntrinsic::RandomBool),
        ("Random", "bytes") => Some(RegIntrinsic::RandomBytes),
        ("Random", "float") => Some(RegIntrinsic::RandomFloat),
        ("Random", "int") => Some(RegIntrinsic::RandomInt),
        ("Random", "string") => Some(RegIntrinsic::RandomString),
        ("Regex", "captures") => Some(RegIntrinsic::RegexCaptures),
        ("Regex", "compile") => Some(RegIntrinsic::RegexCompile),
        ("Regex", "find") => Some(RegIntrinsic::RegexFind),
        ("Regex", "is_match") => Some(RegIntrinsic::RegexIsMatch),
        ("Regex", "replace_all") => Some(RegIntrinsic::RegexReplaceAll),
        ("Regex", "split") => Some(RegIntrinsic::RegexSplit),
        ("RegexError", "message") => Some(RegIntrinsic::RegexErrorMessage),
        ("Result", "and_then") => Some(RegIntrinsic::ResultAndThen),
        ("Result", "err") => Some(RegIntrinsic::ResultErr),
        ("Result", "err_message") => Some(RegIntrinsic::ResultErrMessage),
        ("Result", "is_err") => Some(RegIntrinsic::ResultIsErr),
        ("Result", "is_ok") => Some(RegIntrinsic::ResultIsOk),
        ("Result", "map") => Some(RegIntrinsic::ResultMap),
        ("Result", "map_error") => Some(RegIntrinsic::ResultMapError),
        ("Result", "ok") => Some(RegIntrinsic::ResultOk),
        ("Result", "unwrap_or") => Some(RegIntrinsic::ResultUnwrapOr),
        ("Result", "unwrap_or_else") => Some(RegIntrinsic::ResultUnwrapOrElse),
        ("Request", "new") => Some(RegIntrinsic::RequestNew),
        ("Request", "path") => Some(RegIntrinsic::RequestPath),
        ("Receiver", "close") => Some(RegIntrinsic::ReceiverClose),
        ("Receiver", "into_stream") => Some(RegIntrinsic::ReceiverIntoStream),
        ("Receiver", "recv") => Some(RegIntrinsic::ReceiverRecv),
        ("Receiver", "recv_cancellable") => Some(RegIntrinsic::ReceiverRecvCancellable),
        ("Response", "body") => Some(RegIntrinsic::ResponseBody),
        ("Response", "ok") => Some(RegIntrinsic::ResponseOk),
        ("Response", "status") => Some(RegIntrinsic::ResponseStatus),
        ("Row", "field_string") => Some(RegIntrinsic::RowFieldString),
        ("RowBuffer", "new") => Some(RegIntrinsic::RowBufferNew),
        ("RuleLoader", "load_rules") => Some(RegIntrinsic::RuleLoaderLoadRules),
        ("ResourcePool", "borrow") => Some(RegIntrinsic::ResourcePoolBorrow),
        ("ResourcePool", "discard") => Some(RegIntrinsic::ResourcePoolDiscard),
        ("ResourcePool", "lazy") => Some(RegIntrinsic::ResourcePoolLazy),
        ("ResourcePool", "new") => Some(RegIntrinsic::ResourcePoolNew),
        ("ResourcePool", "stats") => Some(RegIntrinsic::ResourcePoolStats),
        ("ResourcePool", "try_borrow") => Some(RegIntrinsic::ResourcePoolTryBorrow),
        ("ResourcePool", "try_lazy") => Some(RegIntrinsic::ResourcePoolTryLazy),
        ("ResourcePool", "try_new") => Some(RegIntrinsic::ResourcePoolTryNew),
        ("Set", "contains") => Some(RegIntrinsic::SetContains),
        ("Set", "difference") => Some(RegIntrinsic::SetDifference),
        ("Set", "intersection") => Some(RegIntrinsic::SetIntersection),
        ("Set", "is_empty") => Some(RegIntrinsic::SetIsEmpty),
        ("Set", "is_subset") => Some(RegIntrinsic::SetIsSubset),
        ("Set", "len") => Some(RegIntrinsic::SetLen),
        ("Set", "new") => Some(RegIntrinsic::SetNew),
        ("Set", "to_list") => Some(RegIntrinsic::SetToList),
        ("Set", "union") => Some(RegIntrinsic::SetUnion),
        ("SortedSet", "contains") => Some(RegIntrinsic::SortedSetContains),
        ("SortedSet", "is_empty") => Some(RegIntrinsic::SortedSetIsEmpty),
        ("SortedSet", "len") => Some(RegIntrinsic::SortedSetLen),
        ("SortedSet", "new") => Some(RegIntrinsic::SortedSetNew),
        ("SortedSet", "to_list") => Some(RegIntrinsic::SortedSetToList),
        ("SortedMap", "contains_key") => Some(RegIntrinsic::SortedMapContainsKey),
        ("SortedMap", "get") => Some(RegIntrinsic::SortedMapGet),
        ("SortedMap", "is_empty") => Some(RegIntrinsic::SortedMapIsEmpty),
        ("SortedMap", "keys") => Some(RegIntrinsic::SortedMapKeys),
        ("SortedMap", "len") => Some(RegIntrinsic::SortedMapLen),
        ("SortedMap", "new") => Some(RegIntrinsic::SortedMapNew),
        ("SortedMap", "values") => Some(RegIntrinsic::SortedMapValues),
        ("String", "after") => Some(RegIntrinsic::StringAfter),
        ("String", "before") => Some(RegIntrinsic::StringBefore),
        ("String", "char_at") => Some(RegIntrinsic::StringCharAt),
        ("String", "contains") => Some(RegIntrinsic::StringContains),
        ("String", "count") => Some(RegIntrinsic::StringCount),
        ("String", "copy") | ("String", "clone") => Some(RegIntrinsic::StringCopy),
        ("String", "ends_with") => Some(RegIntrinsic::StringEndsWith),
        ("String", "env") => Some(RegIntrinsic::EnvGet),
        ("String", "env_or") => Some(RegIntrinsic::EnvGetOrDefault),
        ("String", "format") => Some(RegIntrinsic::StringFormat),
        ("String", "from_bool") => Some(RegIntrinsic::StringFromBool),
        ("String", "from_float") => Some(RegIntrinsic::StringFromFloat),
        ("String", "from_int") => Some(RegIntrinsic::StringFromInt),
        ("String", "index_of") => Some(RegIntrinsic::StringIndexOf),
        ("String", "is_empty") => Some(RegIntrinsic::StringIsEmpty),
        ("String", "join") => Some(RegIntrinsic::StringJoin),
        ("String", "lines") => Some(RegIntrinsic::StringLines),
        ("String", "chars") => Some(RegIntrinsic::StringChars),
        ("String", "len") => Some(RegIntrinsic::StringLen),
        ("String", "pad_left") => Some(RegIntrinsic::StringPadLeft),
        ("String", "pad_right") => Some(RegIntrinsic::StringPadRight),
        ("String", "parse_float") => Some(RegIntrinsic::StringParseFloat),
        ("String", "parse_int") => Some(RegIntrinsic::StringParseInt),
        ("String", "repeat") => Some(RegIntrinsic::StringRepeat),
        ("String", "replace") => Some(RegIntrinsic::StringReplace),
        ("String", "replace_first") => Some(RegIntrinsic::StringReplaceFirst),
        ("String", "reverse") => Some(RegIntrinsic::StringReverse),
        ("String", "slice") | ("String", "view") => Some(RegIntrinsic::StringSlice),
        ("String", "split") => Some(RegIntrinsic::StringSplit),
        ("String", "starts_with") => Some(RegIntrinsic::StringStartsWith),
        ("String", "strip_prefix") => Some(RegIntrinsic::StringStripPrefix),
        ("String", "safe_relative") => Some(RegIntrinsic::PathSafeRelative),
        ("String", "to_path") => Some(RegIntrinsic::PathFromString),
        ("String", "to_url") => Some(RegIntrinsic::UrlFromString),
        ("String", "to_bytes") => Some(RegIntrinsic::BytesFromString),
        ("String", "to_lowercase") => Some(RegIntrinsic::StringToLowercase),
        ("String", "to_uppercase") => Some(RegIntrinsic::StringToUppercase),
        ("String", "trim") => Some(RegIntrinsic::StringTrim),
        ("String", "trim_end") => Some(RegIntrinsic::StringTrimEnd),
        ("String", "trim_start") => Some(RegIntrinsic::StringTrimStart),
        ("TcpError", "message") => Some(RegIntrinsic::TcpErrorMessage),
        ("Toml", "parse_file") => Some(RegIntrinsic::TomlParseFile),
        ("StringBuilder", "finish") => Some(RegIntrinsic::StringCopy),
        ("StringBuilder", "new") => Some(RegIntrinsic::StringBuilderNew),
        ("Stream", "collect_list") => Some(RegIntrinsic::StreamCollectList),
        ("Stream", "from_list") => Some(RegIntrinsic::StreamFromList),
        ("Stream", "next") => Some(RegIntrinsic::StreamNext),
        ("Sender", "close") => Some(RegIntrinsic::SenderClose),
        ("Sender", "send") => Some(RegIntrinsic::SenderSend),
        ("Sender", "send_cancellable") => Some(RegIntrinsic::SenderSendCancellable),
        ("StringView", "after") => Some(RegIntrinsic::StringAfter),
        ("StringView", "before") => Some(RegIntrinsic::StringBefore),
        ("StringView", "contains") => Some(RegIntrinsic::StringContains),
        ("StringView", "is_empty") => Some(RegIntrinsic::StringIsEmpty),
        ("StringView", "len") => Some(RegIntrinsic::StringLen),
        ("StringView", "slice") => Some(RegIntrinsic::StringSlice),
        ("StringView", "starts_with") => Some(RegIntrinsic::StringStartsWith),
        ("StringView", "to_string") => Some(RegIntrinsic::StringCopy),
        ("Tcp", "connect") => Some(RegIntrinsic::TcpConnect),
        ("TempDir", "keep") => Some(RegIntrinsic::TempDirKeep),
        ("TempDir", "new") => Some(RegIntrinsic::TempDirNew),
        ("TempDir", "new_in") => Some(RegIntrinsic::TempDirNewIn),
        ("TempDir", "path") => Some(RegIntrinsic::TempDirPath),
        ("TcpStream", "read") => Some(RegIntrinsic::TcpStreamRead),
        ("TcpStream", "shutdown") => Some(RegIntrinsic::TcpStreamShutdown),
        ("TcpStream", "write") => Some(RegIntrinsic::TcpStreamWrite),
        ("TcpStream", "write_all") => Some(RegIntrinsic::TcpStreamWriteAll),
        ("Timer", "sleep") => Some(RegIntrinsic::TimerSleep),
        ("Timer", "sleep_cancellable") => Some(RegIntrinsic::TimerSleepCancellable),
        ("Timer", "sleep_until") => Some(RegIntrinsic::TimerSleepUntil),
        ("Url", "decode_component") => Some(RegIntrinsic::UrlDecodeComponent),
        ("Url", "encode_component") => Some(RegIntrinsic::UrlEncodeComponent),
        ("Url", "from_string") => Some(RegIntrinsic::UrlFromString),
        ("Url", "to_string") => Some(RegIntrinsic::UrlToString),
        ("Uuid", "new_v4") => Some(RegIntrinsic::UuidNewV4),
        ("Workspace", "resolve") => Some(RegIntrinsic::PathResolveRelative),
        ("WebSocket", "close") => Some(RegIntrinsic::WebSocketClose),
        ("WebSocket", "connect") => Some(RegIntrinsic::WebSocketConnect),
        ("WebSocket", "recv_bytes") => Some(RegIntrinsic::WebSocketRecvBytes),
        ("WebSocket", "recv_text") => Some(RegIntrinsic::WebSocketRecvText),
        ("WebSocket", "send_bytes") => Some(RegIntrinsic::WebSocketSendBytes),
        ("WebSocket", "send_text") => Some(RegIntrinsic::WebSocketSendText),
        ("WebSocketError", "message") => Some(RegIntrinsic::WebSocketErrorMessage),
        ("Yaml", "parse") => Some(RegIntrinsic::YamlParse),
        ("Yaml", "parse_file") => Some(RegIntrinsic::YamlParseFile),
        ("Weak", "downgrade") => Some(RegIntrinsic::WeakDowngrade),
        ("Weak", "from") => Some(RegIntrinsic::WeakFrom),
        ("Weak", "upgrade") => Some(RegIntrinsic::WeakUpgrade),
        _ => None,
    }
}
fn closure_capture_names(
    body: &HirBlock,
    params: &[String],
    explicit_captures: &[crate::hir::HirClosureCapture],
    outer_locals: &HashMap<String, Reg>,
) -> Vec<String> {
    let mut names = explicit_captures
        .iter()
        .map(|capture| capture.name.clone())
        .collect::<Vec<_>>();
    let mut seen = names.iter().cloned().collect::<HashSet<_>>();
    let mut bound = params.iter().cloned().collect::<HashSet<_>>();
    let mut free = BTreeSet::new();
    collect_free_locals_block(body, &mut bound, &mut free);
    for name in free {
        if outer_locals.contains_key(&name) && seen.insert(name.clone()) {
            names.push(name);
        }
    }
    names
}

fn collect_free_locals_block(
    block: &HirBlock,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    for statement in &block.statements {
        collect_free_locals_stmt(statement, bound, free);
    }
}

fn collect_free_locals_stmt(
    statement: &HirStmt,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    match statement {
        HirStmt::Let { name, value, .. } => {
            if let Some(value) = value {
                collect_free_locals_expr(value, bound, free);
            }
            bound.insert(name.clone());
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_free_locals_expr(value, bound, free);
            }
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            collect_free_locals_expr(resource, bound, free);
            let mut body_bound = bound.clone();
            body_bound.insert(binding.clone());
            collect_free_locals_block(body, &mut body_bound, free);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_free_locals_expr(condition, bound, free);
            collect_free_locals_block(&then_body.clone(), &mut bound.clone(), free);
            if let Some(else_body) = else_body {
                collect_free_locals_block(&else_body.clone(), &mut bound.clone(), free);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_free_locals_expr(condition, bound, free);
            }
            collect_free_locals_block(&body.clone(), &mut bound.clone(), free);
        }
        HirStmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            collect_free_locals_expr(iterable, bound, free);
            let mut body_bound = bound.clone();
            body_bound.insert(binding.clone());
            collect_free_locals_block(body, &mut body_bound, free);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_free_locals_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                for binding in arm.pattern.binding_names() {
                    arm_bound.insert(binding.to_string());
                }
                if let Some(guard) = &arm.guard {
                    collect_free_locals_expr(guard, &mut arm_bound, free);
                }
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_free_locals_expr(&arm.operation, bound, free);
                let mut arm_bound = bound.clone();
                arm_bound.insert(arm.binding.clone());
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirStmt::Assign { target, value, .. } => {
            collect_free_locals_expr(target, bound, free);
            collect_free_locals_expr(value, bound, free);
        }
        HirStmt::Expr(value) => collect_free_locals_expr(value, bound, free),
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn collect_free_locals_expr(
    expr: &HirExpr,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    match expr {
        HirExpr::Ident { name, .. } => {
            if !bound.contains(name) {
                free.insert(name.clone());
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_free_locals_expr(&field.value, bound, free);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_free_locals_expr(&entry.key, bound, free);
                collect_free_locals_expr(&entry.value, bound, free);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_free_locals_expr(item, bound, free);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            collect_free_locals_expr(left, bound, free);
            collect_free_locals_expr(right, bound, free);
        }
        HirExpr::Field { base, .. } => collect_free_locals_expr(base, bound, free),
        HirExpr::Index { base, index, .. } => {
            collect_free_locals_expr(base, bound, free);
            collect_free_locals_expr(index, bound, free);
        }
        HirExpr::Call { receiver, args, .. } => {
            if let Some(receiver) = receiver {
                collect_free_locals_expr(&receiver.value, bound, free);
            }
            for arg in args {
                collect_free_locals_expr(&arg.value, bound, free);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_free_locals_expr(value, bound, free),
        HirExpr::Closure {
            params,
            captures,
            body,
            ..
        } => {
            for capture in captures {
                if !bound.contains(&capture.name) {
                    free.insert(capture.name.clone());
                }
            }
            let mut nested_bound = bound.clone();
            for param in params {
                nested_bound.insert(param.clone());
            }
            collect_free_locals_block(body, &mut nested_bound, free);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_free_locals_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                for binding in arm.pattern.binding_names() {
                    arm_bound.insert(binding.to_string());
                }
                if let Some(guard) = &arm.guard {
                    collect_free_locals_expr(guard, &mut arm_bound, free);
                }
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirExpr::Number { .. } | HirExpr::String { .. } | HirExpr::Unknown(_) => {}
    }
}

/// One activation record on the explicit call stack. The interpreter is
/// stackless: instead of `run_frame` recursing into itself for each RSScript
/// call, it pushes a `Frame` and keeps looping, so a task's whole call chain
/// lives in `RegVm::frames` and can be suspended/resumed (the foundation for
/// the cooperative async scheduler). Synchronous closure callbacks
/// (`List.map`/`sort_with`/…) still nest via `run_frame`, which is fine because
/// they can never `await`.
struct Frame {
    func: Rc<RegFunction>,
    ip: usize,
    base: usize,
    /// Absolute register in the caller that receives this frame's return value.
    /// `usize::MAX` marks a driver root (its value is returned out of `run_frame`).
    ret_dst: usize,
    /// `mut`-argument write-backs to perform when this frame completes:
    /// `(caller_abs_reg, this_frame_abs_reg)` pairs. The caller register receives
    /// the parameter's final (possibly mutated) value, so `mut` params propagate.
    /// Empty for the overwhelmingly common no-`mut`-arg call (then a no-op).
    mut_writeback: Vec<(usize, usize)>,
}

/// Result of driving a task's call stack one slice at a time.
enum Outcome {
    /// The frame at `floor` returned this value (the task or sync call finished).
    Completed(VmValue),
    /// A blocking op parked the task; details are in `RegVm::suspension`.
    Suspended,
}

type TaskId = usize;

/// What a parked task is waiting for. When the condition is met the scheduler
/// produces the operation's result, writes it into `Suspension::resume_dst`, and
/// re-queues the task (the "completion" model — the parked instruction is not
/// re-executed; the saved `ip` already points past it).
enum Wait {
    /// `Receiver.recv` on an empty-but-open channel: ready when a value is queued
    /// or the channel closes.
    Recv { channel: i64 },
    /// `Sender.send` on a full bounded channel: ready when capacity frees up. The
    /// sender + value are carried so the send can be retried on wake.
    Send { sender: VmSender, value: VmValue },
    /// `await`-ing a spawned task / `async let`: ready when that task finishes.
    Join { task: TaskId },
    /// `Timer.sleep*`: ready once the wall clock reaches `deadline`.
    Sleep { deadline: std::time::Instant },
    /// `select { ... }`: ready as soon as any arm task in `handles` finishes. The
    /// winning arm index and its value are written to `winner_dst`/`value_dst`.
    Select {
        handles: Vec<TaskId>,
        winner_dst: usize,
        value_dst: usize,
    },
}

struct Suspension {
    wait: Wait,
    /// Absolute register that receives the operation's result on wake.
    resume_dst: usize,
}

/// A parked task's full execution state, swapped out of `RegVm` while another
/// task runs and swapped back in on resume.
struct SavedTask {
    frames: Vec<Frame>,
    stack: Vec<VmValue>,
    written: Vec<bool>,
}

struct TaskSlot {
    /// Parked execution state; `None` while the task is the one swapped into
    /// `RegVm` and running.
    saved: Option<SavedTask>,
    /// `Some(value)` once the task has returned (value available to joiners).
    done: Option<VmValue>,
    /// `Some(wait)` while the task is parked on a blocking op.
    wait: Option<Wait>,
    /// Register (in the task's own stack) that receives the op result on wake.
    resume_dst: usize,
}

struct RegVm {
    unit: Rc<RegUnit>,
    args: Vec<String>,
    native_bindings: HashMap<String, NativeInterpreterFn>,
    stdout: String,
    /// When set, complete lines appended to `stdout` are also written live to the
    /// real process stdout (line-flushed). Used ONLY by `rss dev --run` so a slow
    /// or looping program shows output as it runs instead of buffering until exit.
    /// `stream_flushed` tracks how many bytes of `stdout` have been streamed so a
    /// partial trailing line is not emitted twice. The captured `stdout` String is
    /// built identically whether or not streaming is on, so every other caller
    /// (and the parity/differential tests) is unaffected.
    stream_stdout: bool,
    stream_flushed: usize,
    stderr: String,
    stack: Vec<VmValue>,
    written: Vec<bool>,
    frames: Vec<Frame>,
    /// Set by a blocking op during `drive`; consumed by the scheduler.
    suspension: Option<Suspension>,
    /// Cooperative single-threaded task table + ready queue.
    tasks: HashMap<TaskId, TaskSlot>,
    ready_queue: VecDeque<TaskId>,
    next_task_id: TaskId,
    current_task: TaskId,
    next_cancellation_id: i64,
    cancellation_flags: HashMap<i64, bool>,
    next_channel_id: i64,
    channels: HashMap<i64, VmChannel>,
    next_tcp_stream_id: i64,
    tcp_streams: HashMap<i64, TcpStream>,
    next_websocket_id: i64,
    websockets: HashMap<i64, TcpStream>,
    next_pool_id: i64,
    pools: HashMap<i64, VmResourcePool>,
    // Native tensor handles. The VM stores the real `RssTensor` (the same type
    // the AOT backend lowers to) keyed by id and carries an opaque
    // `VmValue::Native { type_name: "Tensor", id }` handle through the program.
    // The intrinsic handlers call the exact `rsscript_runtime::tensor_*` kernels
    // the lowered code calls, so VM<->compiled results are bit-identical.
    next_tensor_id: i64,
    tensors: HashMap<i64, rsscript_runtime::RssTensor>,
    /// Tier-0 JIT: when set, JIT-eligible functions run via the specializing
    /// executor `run_jit` (which reuses the interpreter's value/register
    /// semantics, so it is gap-free by construction).
    jit_enabled: bool,
    /// JIT every supported function, ignoring the has-loop heuristic (used by the
    /// differential tests so the whole covered instruction subset is verified).
    jit_force_all: bool,
    /// Native (Cranelift) JIT state, `Some` when the native tier is enabled. The
    /// native tier compiles the integer/control core to machine code and is tried
    /// before the tier-0 executor; anything it can't compile (or bails on) falls
    /// back to tier-0 / the interpreter.
    #[cfg(feature = "native-jit")]
    native: Option<NativeState>,
}

/// State for the native JIT tier: the Cranelift module owning the compiled code,
/// a per-function cache (`None` = known not native-eligible), and the tiering /
/// deopt knobs.
#[cfg(feature = "native-jit")]
struct NativeState {
    module: vm_jit::NativeModule,
    // `None` = known not native-eligible; `Some((id, ret, params))` = compiled
    // handle, return type (to box the 64-bit result), and parameter types (to
    // unbox each argument: `Int`/`Bool` from their VM value, `Float` as bits).
    #[allow(clippy::type_complexity)]
    cache: HashMap<usize, Option<(vm_jit::CompiledId, NativeTy, Vec<NativeTy>)>>,
    /// Per-function call counts, for tiering: a function is compiled and run
    /// natively only once it has been entered more than `tier_up_threshold` times
    /// (a hot-function heuristic). `0` means "compile on first call" (force-all).
    counts: HashMap<usize, u32>,
    tier_up_threshold: u32,
    /// Deopt stress mode: when set, the native tier always bails, so every
    /// native-eligible function exercises the fallback path. Used to verify
    /// `{interp, tier0, native, force-deopt, compiled}` all agree.
    force_bail: bool,
    /// Telemetry: where native-tier attempts go (so the next coverage win is
    /// measurable rather than guessed).
    stats: NativeStats,
    /// Whether to collect telemetry. Keep timing and counter updates out of the
    /// native-call hot path unless a caller explicitly asks for them.
    collect_stats: bool,
}

/// Native-JIT telemetry. The VM is single-threaded, so plain counters suffice.
#[cfg(feature = "native-jit")]
#[derive(Debug, Default, Clone)]
pub struct NativeStats {
    /// Hot functions that reached the native tier (passed tiering, not force-bail).
    pub considered: u64,
    /// Calls deferred below the tier-up threshold (still on the interpreter).
    pub tier_deferred: u64,
    /// Functions that translated into the native IR.
    pub translated: u64,
    /// Functions rejected by translation (outside the native subset).
    pub not_eligible: u64,
    /// Functions Cranelift compiled to machine code.
    pub compiled: u64,
    /// Functions that translated but failed to compile.
    pub compile_failed: u64,
    /// Native calls whose runtime args didn't match the inferred parameter types.
    pub arg_mismatch: u64,
    /// Native calls that ran to completion.
    pub native_calls: u64,
    /// Native calls that bailed at a guard (overflow/div-by-zero/…) → interpreter.
    pub native_bails: u64,
    /// Total nanoseconds spent in Cranelift compilation.
    pub compile_nanos: u128,
    /// Total nanoseconds spent executing native code.
    pub run_nanos: u128,
}

#[cfg(feature = "native-jit")]
impl NativeStats {
    fn summary(&self) -> String {
        format!(
            "native-jit: considered={} translated={} compiled={} not_eligible={} \
compile_failed={} calls={} bails={} arg_mismatch={} tier_deferred={} \
compile_ms={:.3} run_ms={:.3}",
            self.considered,
            self.translated,
            self.compiled,
            self.not_eligible,
            self.compile_failed,
            self.native_calls,
            self.native_bails,
            self.arg_mismatch,
            self.tier_deferred,
            self.compile_nanos as f64 / 1.0e6,
            self.run_nanos as f64 / 1.0e6,
        )
    }

    /// Telemetry as JSON, for the `jit` field of `rss bench --json` output.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "considered": self.considered,
            "translated": self.translated,
            "compiled": self.compiled,
            "not_eligible": self.not_eligible,
            "compile_failed": self.compile_failed,
            "native_calls": self.native_calls,
            "bails": self.native_bails,
            "arg_mismatch": self.arg_mismatch,
            "tier_deferred": self.tier_deferred,
            "compile_ms": self.compile_nanos as f64 / 1.0e6,
            "run_ms": self.run_nanos as f64 / 1.0e6,
        })
    }
}

// --- Native-JIT host helpers ------------------------------------------------
//
// Heap values (structs/lists) can't live in the native tier's scalar registers,
// so the compiled code reads them by calling back into these helpers, passing an
// opaque handle (an index into a per-call table the VM fills in `try_native`).
// A read that can't be satisfied (wrong type / out of bounds) sets a bail flag;
// `try_native` checks it and re-runs the function on the interpreter, preserving
// the gap-free model. `rsscript` stays `#![forbid(unsafe_code)]`: defining these
// `extern "C"` functions and taking their addresses needs no `unsafe` — the only
// `unsafe` (the indirect call) lives in `vm-jit`.

#[cfg(feature = "native-jit")]
thread_local! {
    /// Heap values for the in-flight native call, indexed by handle.
    static JIT_HEAP_ARGS: RefCell<Vec<VmValue>> = const { RefCell::new(Vec::new()) };
}

/// Clears the per-call heap-arg table on drop, so a native attempt never retains
/// its cloned struct/list arguments past the call (on success, bail, or error).
#[cfg(feature = "native-jit")]
struct JitHeapArgsGuard;

#[cfg(feature = "native-jit")]
impl Drop for JitHeapArgsGuard {
    fn drop(&mut self) {
        JIT_HEAP_ARGS.with(|table| table.borrow_mut().clear());
    }
}

#[cfg(feature = "native-jit")]
fn jit_host_helpers() -> vm_jit::HostHelpers {
    // Typed `extern "C"` functions: `vm-jit` owns the raw-pointer conversion, so
    // `rsscript` never hands it an untyped address. Keeps this crate's
    // `#![forbid(unsafe_code)]` honest without an unsound safe API on the boundary.
    vm_jit::HostHelpers {
        field_int: rss_jit_field_int,
        list_len: rss_jit_list_len,
        list_get_int: rss_jit_list_get_int,
    }
}

/// Look up the heap value for `handle` and apply `read`; `None` (→ bail) if the
/// handle is invalid or the read fails.
#[cfg(feature = "native-jit")]
fn jit_heap_read<R>(handle: i64, read: impl FnOnce(&VmValue) -> Option<R>) -> Option<R> {
    let index = usize::try_from(handle).ok()?;
    JIT_HEAP_ARGS.with(|args| args.borrow().get(index).and_then(|value| read(value)))
}

#[cfg(feature = "native-jit")]
fn jit_struct_field_int(value: &VmValue, slot: usize) -> Option<i64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => match data.fields.get(slot)? {
            VmValue::Int(v) => Some(*v),
            _ => None,
        },
        VmValue::Managed(inner) => jit_struct_field_int(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_list_len(value: &VmValue) -> Option<i64> {
    match value {
        VmValue::List(list) => i64::try_from(list.borrow().len()).ok(),
        VmValue::Managed(inner) => jit_list_len(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_list_get_int(value: &VmValue, index: i64) -> Option<i64> {
    match value {
        VmValue::List(list) => {
            let index = usize::try_from(index).ok()?;
            match list.borrow().get(index)? {
                VmValue::Int(v) => Some(*v),
                _ => None,
            }
        }
        VmValue::Managed(inner) => jit_list_get_int(&inner.borrow(), index),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_int(handle: i64, slot: i64) -> i64 {
    match usize::try_from(slot)
        .ok()
        .and_then(|slot| jit_heap_read(handle, |value| jit_struct_field_int(value, slot)))
    {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_len(handle: i64) -> i64 {
    match jit_heap_read(handle, jit_list_len) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_get_int(handle: i64, index: i64) -> i64 {
    match jit_heap_read(handle, |value| jit_list_get_int(value, index)) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
impl NativeState {
    fn new(
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
    ) -> Result<Self, EvalError> {
        Ok(Self {
            module: vm_jit::NativeModule::new(jit_host_helpers())
                .map_err(|e| EvalError::Runtime(e.to_string()))?,
            cache: HashMap::new(),
            counts: HashMap::new(),
            tier_up_threshold,
            force_bail,
            stats: NativeStats::default(),
            collect_stats,
        })
    }
}

#[derive(Debug, Clone)]
struct VmChannel {
    capacity: usize,
    queue: VecDeque<VmValue>,
    senders: i64,
    receiver_taken: bool,
    receiver_closed: bool,
}

impl VmChannel {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            queue: VecDeque::new(),
            senders: 0,
            receiver_taken: false,
            receiver_closed: false,
        }
    }
}

#[derive(Debug, Clone)]
struct VmResourcePool {
    capacity: i64,
    created: i64,
    in_use: i64,
    idle: Vec<VmValue>,
    factory: Option<Rc<VmClosure>>,
    factory_returns_result: bool,
}

struct VmResourcePoolLease {
    pool_id: i64,
    discarded: bool,
    value: VmValue,
}

impl RegVm {
    fn new(
        unit: Rc<RegUnit>,
        args: Vec<String>,
        native_bindings: HashMap<String, NativeInterpreterFn>,
    ) -> Self {
        Self {
            unit,
            args,
            native_bindings,
            stdout: String::new(),
            stream_stdout: false,
            stream_flushed: 0,
            stderr: String::new(),
            stack: Vec::new(),
            written: Vec::new(),
            frames: Vec::new(),
            suspension: None,
            tasks: HashMap::new(),
            ready_queue: VecDeque::new(),
            next_task_id: 0,
            current_task: 0,
            next_cancellation_id: 1,
            cancellation_flags: HashMap::new(),
            next_channel_id: 1,
            channels: HashMap::new(),
            next_tcp_stream_id: 1,
            tcp_streams: HashMap::new(),
            next_websocket_id: 1,
            websockets: HashMap::new(),
            next_pool_id: 1,
            pools: HashMap::new(),
            next_tensor_id: 1,
            tensors: HashMap::new(),
            jit_enabled: false,
            jit_force_all: false,
            #[cfg(feature = "native-jit")]
            native: None,
        }
    }

    /// Append program output to the captured `stdout` buffer, and — when live
    /// streaming is enabled (`rss dev --run`) — flush newly completed lines to the
    /// real process stdout immediately. The captured buffer is appended to exactly
    /// the same way regardless, so callers that read `EvalOutput.stdout` see no
    /// difference.
    fn push_stdout(&mut self, text: &str) {
        self.stdout.push_str(text);
        if self.stream_stdout {
            self.flush_stdout_stream();
        }
    }

    /// Write every complete (newline-terminated) line appended since the last
    /// flush to the real process stdout, then advance the streamed cursor. A
    /// partial trailing line is left buffered until its newline arrives.
    fn flush_stdout_stream(&mut self) {
        if let Some(offset) = self.stdout[self.stream_flushed..].rfind('\n') {
            let end = self.stream_flushed + offset + 1;
            let chunk = &self.stdout[self.stream_flushed..end];
            let mut out = std::io::stdout();
            let _ = out.write_all(chunk.as_bytes());
            let _ = out.flush();
            self.stream_flushed = end;
        }
    }

    /// Whether `func` should run on the tier-0 JIT. Reads the analysis cached on
    /// the function (`(eligible, has_loop)`, computed once for the whole unit by
    /// [`compute_jit_eligibility`], which already accounts for cross-function
    /// calls). A function is JIT'd only if (a) it is eligible — non-suspending and
    /// non-recursive — and (b) it contains a back-edge (a loop): straight-line
    /// functions gain nothing from the specializing executor, so JIT-ing them in a
    /// hot call would only add overhead. This keeps the JIT at-least-parity with
    /// the interpreter.
    fn is_jit_eligible(&self, func: &RegFunction) -> bool {
        let (eligible, has_loop) = func.jit_analysis.get().unwrap_or_else(|| {
            // Defensive: every unit function has its analysis pre-set in `lower`.
            // A function with no cross-call context can only be the pure subset.
            (
                func.code.iter().all(jit_supported_instruction),
                jit_function_has_loop(&func.code),
            )
        });
        // Production: only JIT functions with a loop (where the specializing
        // executor pays off). `jit_force_all` (tests) JITs every eligible function
        // so the differential verifies the whole covered subset.
        eligible && (self.jit_force_all || has_loop)
    }

    /// Try to run `func` on the native (Cranelift) tier. Returns `Some(result)` if
    /// the compiled code ran to completion, or `None` when the function isn't
    /// native-eligible, an argument isn't an `Int`, or the native code bailed on an
    /// edge (overflow / divide-by-zero / out-of-range shift) — in all of which
    /// cases the caller falls back to the interpreter, which produces the exact
    /// value or error. Safe because native-eligible functions are leaf and
    /// side-effect-free, so re-running them is observationally identical.
    #[cfg(feature = "native-jit")]
    fn try_native(&mut self, func: &RegFunction, base: usize) -> Option<VmValue> {
        // Cheap negative path: a function known not native-eligible never compiles,
        // so skip all per-call tiering/cache/name-hash work and fall straight back
        // to the interpreter (keeps `jit-native` from being slower than the VM on
        // code the native tier can't take).
        if func.native_status.get() == NATIVE_STATUS_NOT_ELIGIBLE {
            return None;
        }
        // The unit is needed to resolve inlinable callees; clone the `Rc` so the
        // mutable `self.native` borrow below doesn't conflict.
        let unit = Rc::clone(&self.unit);
        let native_key = func as *const RegFunction as usize;
        // Phase 1: tiering + resolve (and lazily compile) the native function.
        // `None` in the cache means "known not native-eligible".
        let (id, ret_type, param_types) = {
            let native = self.native.as_mut()?;
            if native.force_bail {
                // Deopt stress mode: pretend the native code bailed at its first
                // guard, so the interpreter handles the function.
                return None;
            }
            // Tiering: stay on the interpreter until the function is hot.
            let count = native.counts.entry(native_key).or_insert(0);
            *count += 1;
            if *count <= native.tier_up_threshold {
                if native.collect_stats {
                    native.stats.tier_deferred += 1;
                }
                return None;
            }
            if native.collect_stats {
                native.stats.considered += 1;
            }
            let entry = match native.cache.get(&native_key) {
                Some(entry) => entry.clone(),
                None => {
                    let entry = match translate_to_native_jit(&unit, func) {
                        Some((jit_fn, ret, params)) => {
                            if native.collect_stats {
                                native.stats.translated += 1;
                            }
                            let started = native.collect_stats.then(std::time::Instant::now);
                            let compiled = native.module.compile(&jit_fn);
                            if let Some(started) = started {
                                native.stats.compile_nanos += started.elapsed().as_nanos();
                            }
                            match compiled {
                                Ok(id) => {
                                    if native.collect_stats {
                                        native.stats.compiled += 1;
                                    }
                                    Some((id, ret, params))
                                }
                                Err(_) => {
                                    if native.collect_stats {
                                        native.stats.compile_failed += 1;
                                    }
                                    None
                                }
                            }
                        }
                        None => {
                            if native.collect_stats {
                                native.stats.not_eligible += 1;
                            }
                            // Invariant verdict — cache it on the function so future
                            // calls take the cheap negative path above.
                            func.native_status.set(NATIVE_STATUS_NOT_ELIGIBLE);
                            None
                        }
                    };
                    native.cache.insert(native_key, entry.clone());
                    entry
                }
            };
            entry?
        };
        // Phase 2: marshal each argument to 64 bits per its inferred parameter
        // type. Scalars unbox directly; a `Handle` (struct/list) is registered in
        // the per-call heap table and passed as its index, for the host helpers to
        // read. (`NativeModule::call` resets its own bail flag.) A drop guard clears
        // the (possibly large) heap table on every exit path so cloned args aren't
        // retained after the call.
        let _heap_guard = JitHeapArgsGuard;
        let mut inline_args = [0i64; 8];
        let mut heap_args = Vec::new();
        let use_inline_args = param_types.len() <= inline_args.len();
        if !use_inline_args {
            heap_args.reserve(param_types.len());
        }
        for (index, param_type) in param_types.iter().enumerate() {
            let value = self.reg(base + index);
            let bits = match param_type {
                NativeTy::Int => match value {
                    VmValue::Int(value) => Some(*value),
                    _ => None,
                },
                NativeTy::Float => match value {
                    VmValue::Float(value) => Some(value.to_bits() as i64),
                    _ => None,
                },
                NativeTy::Bool => match value {
                    VmValue::Bool(value) => Some(i64::from(*value)),
                    _ => None,
                },
                NativeTy::Handle => Some(JIT_HEAP_ARGS.with(|table| {
                    let mut table = table.borrow_mut();
                    table.push(value.clone());
                    (table.len() - 1) as i64
                })),
            };
            match bits {
                Some(bits) => {
                    if use_inline_args {
                        inline_args[index] = bits;
                    } else {
                        heap_args.push(bits);
                    }
                }
                None => {
                    if let Some(native) = self.native.as_mut() {
                        if native.collect_stats {
                            native.stats.arg_mismatch += 1;
                        }
                    }
                    return None;
                }
            }
        }
        let args: &[i64] = if use_inline_args {
            &inline_args[..param_types.len()]
        } else {
            &heap_args
        };
        // Phase 3: call. `call` returns `None` if the native code bailed at a guard
        // *or* a host helper flagged an unsatisfiable heap read; either way the
        // interpreter re-runs the function. A clean result is boxed per the
        // function's return type (a float register stored its `f64` bit pattern).
        let collect_stats = self.native.as_ref()?.collect_stats;
        let started = collect_stats.then(std::time::Instant::now);
        let result = self.native.as_ref()?.module.call(id, args);
        let elapsed = started.map(|started| started.elapsed().as_nanos());
        let native = self.native.as_mut()?;
        if let Some(elapsed) = elapsed {
            native.stats.run_nanos += elapsed;
        }
        match result {
            Some(bits) => {
                if native.collect_stats {
                    native.stats.native_calls += 1;
                }
                Some(match ret_type {
                    NativeTy::Float => VmValue::Float(f64::from_bits(bits as u64)),
                    _ => VmValue::Int(bits),
                })
            }
            None => {
                if native.collect_stats {
                    native.stats.native_bails += 1;
                }
                None
            }
        }
    }

    /// Tier-0 JIT executor for a JIT-eligible function. Runs the body via the
    /// same shared helpers (`eval_numeric_binary`, `eval_numeric_compare`, …) and
    /// register methods (`reg`/`set_reg`/`take_reg`) the interpreter uses, so its
    /// result is identical to `drive` by construction.
    ///
    /// Eligibility guarantees the function (and its whole reachable call graph) is
    /// non-suspending and non-recursive (see [`compute_jit_eligibility`]), so a
    /// `CallKnown` can be run to completion synchronously via `run_frame` without
    /// ever suspending or unbounded host-stack growth. All other instructions are
    /// pure and go through [`Self::try_exec_pure`].
    fn run_jit(
        &mut self,
        unit: &RegUnit,
        func: &RegFunction,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let mut ip = 0usize;
        while let Some(instr) = func.code.get(ip) {
            ip += 1;
            // Cross-function call: eligibility proved the callee cannot suspend and
            // the call graph is acyclic, so drive it to completion on a fresh frame
            // window above ours, exactly like `drive`'s `CallKnown` but synchronous.
            if let RegInstr::CallKnown {
                dst,
                function: callee_id,
                args,
                mut_args,
            } = instr
            {
                let callee = Rc::clone(&unit.functions[*callee_id]);
                let next_base = base + func.regs;
                self.prepare_frame(next_base, callee.regs);
                for (index, reg) in args.iter().enumerate() {
                    let value = self.reg(base + *reg).clone();
                    self.set_reg(next_base + index, value);
                }
                let result = self.run_frame(unit, callee, next_base)?;
                // Propagate `mut` parameters back to the caller's argument regs.
                for &pos in mut_args {
                    let value = self.reg(next_base + pos).clone();
                    self.set_reg(base + args[pos], value);
                }
                self.set_reg(base + *dst, result);
                continue;
            }
            match self.try_exec_pure(instr, base, &mut ip)? {
                PureStep::Next => {}
                PureStep::Return(value) => return Ok(value),
                // Eligibility guarantees only pure instructions (and the
                // `CallKnown` handled above) reach here; `NotPure` is an internal
                // bug.
                PureStep::NotPure => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM JIT reached non-eligible instruction `{instr:?}`."
                    )));
                }
            }
        }
        Ok(VmValue::Unit)
    }

    /// Execute one *pure* instruction (no frame push, no suspend, no call). This
    /// is the single source of truth for the tier-0 subset's semantics, shared by
    /// the interpreter (`drive`) and the JIT executor (`run_jit`), so the two can
    /// never silently diverge. Jumps update `*ip`; `Return` is handed back to the
    /// caller (which owns frame unwinding); everything else is [`PureStep::NotPure`].
    // `VmMapKey` is interior-mutable (List/struct keys hold `Rc<RefCell<…>>`),
    // but `Map.insert`'s `retains(key)` effect makes mutating a live key
    // unreachable in well-typed RSScript, so the lint's hazard cannot occur.
    #[allow(clippy::mutable_key_type)]
    fn try_exec_pure(
        &mut self,
        instr: &RegInstr,
        base: usize,
        ip: &mut usize,
    ) -> Result<PureStep, EvalError> {
        match instr {
            RegInstr::LoadUnit { dst } => self.set_reg(base + *dst, VmValue::Unit),
            RegInstr::LoadInt { dst, value } => self.set_reg(base + *dst, VmValue::Int(*value)),
            RegInstr::LoadFloat { dst, value } => self.set_reg(base + *dst, VmValue::Float(*value)),
            RegInstr::LoadBool { dst, value } => self.set_reg(base + *dst, VmValue::Bool(*value)),
            RegInstr::Move { dst, src } => {
                let value = self.reg(base + *src).clone();
                self.set_reg(base + *dst, value);
            }
            RegInstr::DeepCopy { reg } => {
                let copied = deep_copy_value(self.reg(base + *reg));
                self.set_reg(base + *reg, copied);
            }
            RegInstr::LoadString { dst, value } => {
                self.set_reg(base + *dst, VmValue::String(Rc::clone(value)));
            }
            RegInstr::Manage { dst, src } => {
                let value = self.reg(base + *src).clone();
                // `manage` wraps a value in a shared mutable cell so it can be
                // retained (stored in a collection/field) and mutated in place.
                // Immutable scalars cannot be mutated in place and have value (not
                // reference) semantics, so wrapping them is a no-op that only leaks
                // an opaque `Managed` into reads — borrow-returning accessors
                // (`String`/`Bytes`/`Json`) can't peel it. Store them directly.
                let managed = if value.is_immutable_scalar() {
                    value
                } else {
                    VmValue::Managed(Rc::new(RefCell::new(value)))
                };
                self.set_reg(base + *dst, managed);
            }
            RegInstr::GetField {
                dst,
                base: obj,
                name,
            } => {
                let value = read_field_ref(self.reg(base + *obj), name)?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::SetField {
                dst,
                base: obj,
                name,
                value,
            } => {
                let obj_reg = base + *obj;
                let new_value = self.reg(base + *value).clone();
                // Take the struct out so its `Rc` count reflects only other live
                // holders; `write_field_value_owned` then mutates in place when
                // uniquely owned, or copy-on-writes when shared.
                let current = self.take_reg(obj_reg);
                let updated = write_field_value_owned(current, name, new_value)?;
                self.set_reg(obj_reg, updated);
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::GetFieldSlot {
                dst,
                base: obj,
                slot,
            } => {
                let value = read_field_slot(self.reg(base + *obj), *slot)?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::SetFieldSlot {
                dst,
                base: obj,
                slot,
                value,
            } => {
                let obj_reg = base + *obj;
                let new_value = self.reg(base + *value).clone();
                let current = self.take_reg(obj_reg);
                let updated = write_field_slot_owned(current, *slot, new_value)?;
                self.set_reg(obj_reg, updated);
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::MakeStruct { dst, name, fields } => {
                let mut field_values: Vec<(String, VmValue)> = Vec::with_capacity(fields.len());
                for (field, reg) in fields {
                    field_values.push((field.clone(), self.reg(base + *reg).clone()));
                }
                self.set_reg(
                    base + *dst,
                    VmValue::Struct(Rc::new(VmStruct::from_named(
                        Rc::from(name.as_str()),
                        field_values,
                    ))),
                );
            }
            RegInstr::MakeVariant { dst, name, fields } => {
                let mut field_values: Vec<(String, VmValue)> = Vec::with_capacity(fields.len());
                for (field, reg) in fields {
                    field_values.push((field.clone(), self.reg(base + *reg).clone()));
                }
                self.set_reg(
                    base + *dst,
                    VmValue::Variant(Rc::new(VmStruct::from_named(
                        Rc::from(name.as_str()),
                        field_values,
                    ))),
                );
            }
            RegInstr::MakeList { dst, items } => {
                let mut list = Vec::with_capacity(items.len());
                for reg in items {
                    list.push(self.reg(base + *reg).clone());
                }
                self.set_reg(base + *dst, VmValue::List(Rc::new(RefCell::new(list))));
            }
            RegInstr::MakeObject { dst, fields } => {
                let mut object = serde_json::Map::new();
                for (field, reg) in fields {
                    let value = vm_value_to_json_literal(self.reg(base + *reg))?;
                    object.insert(field.clone(), value);
                }
                self.set_reg(
                    base + *dst,
                    VmValue::Json(Rc::new(serde_json::Value::Object(object))),
                );
            }
            RegInstr::MakeMap { dst, entries } => {
                let mut map = ValueMap::with_capacity_and_hasher(entries.len(), Default::default());
                for (key, value) in entries {
                    let key = map_key_from_value(self.reg(base + *key))?;
                    map.insert(key, self.reg(base + *value).clone());
                }
                self.set_reg(base + *dst, VmValue::Map(Rc::new(RefCell::new(map))));
            }
            RegInstr::AddInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Add,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::SubInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Subtract,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::MulInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Multiply,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::DivInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Divide,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::ModInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Modulo,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::BitAndInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                self.set_reg(base + *dst, VmValue::Int(l & r));
            }
            RegInstr::BitOrInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                self.set_reg(base + *dst, VmValue::Int(l | r));
            }
            RegInstr::BitXorInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                self.set_reg(base + *dst, VmValue::Int(l ^ r));
            }
            RegInstr::ShiftLeftInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                self.set_reg(base + *dst, VmValue::Int(l.wrapping_shl(r.max(0) as u32)));
            }
            RegInstr::ShiftRightInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                self.set_reg(base + *dst, VmValue::Int(l.wrapping_shr(r.max(0) as u32)));
            }
            RegInstr::LessInt { dst, lhs, rhs } => {
                let value = eval_numeric_compare(
                    RegIntCompare::Less,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, VmValue::Bool(value));
            }
            RegInstr::LessEqualInt { dst, lhs, rhs } => {
                let value = eval_numeric_compare(
                    RegIntCompare::LessEqual,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, VmValue::Bool(value));
            }
            RegInstr::GreaterInt { dst, lhs, rhs } => {
                let value = eval_numeric_compare(
                    RegIntCompare::Greater,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, VmValue::Bool(value));
            }
            RegInstr::GreaterEqualInt { dst, lhs, rhs } => {
                let value = eval_numeric_compare(
                    RegIntCompare::GreaterEqual,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, VmValue::Bool(value));
            }
            RegInstr::Equal { dst, lhs, rhs } => {
                let eq = self.reg(base + *lhs) == self.reg(base + *rhs);
                self.set_reg(base + *dst, VmValue::Bool(eq));
            }
            RegInstr::NotEqual { dst, lhs, rhs } => {
                let ne = self.reg(base + *lhs) != self.reg(base + *rhs);
                self.set_reg(base + *dst, VmValue::Bool(ne));
            }
            RegInstr::Jump { target } => *ip = *target,
            RegInstr::JumpIfBool {
                cond,
                expected,
                target,
            } => {
                if expect_bool_ref(self.reg(base + *cond))? == *expected {
                    *ip = *target;
                }
            }
            RegInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            } => {
                let l = self.reg(base + *lhs);
                let r = self.reg(base + *rhs);
                if eval_numeric_compare(*op, l, r)? == *expected {
                    *ip = *target;
                }
            }
            RegInstr::MakeSome { dst, value } => {
                let value = self.reg(base + *value).clone();
                self.set_reg(base + *dst, VmValue::OptionSome(Box::new(value)));
            }
            RegInstr::LoadNone { dst } => {
                self.set_reg(base + *dst, VmValue::OptionNone);
            }
            RegInstr::MakeClosure {
                dst,
                function: callee,
                captures,
            } => {
                let mut captured = Vec::with_capacity(captures.len());
                for reg in captures {
                    captured.push(self.reg(base + *reg).clone());
                }
                self.set_reg(
                    base + *dst,
                    VmValue::Closure(Rc::new(VmClosure {
                        function: *callee,
                        captures: captured,
                    })),
                );
            }
            RegInstr::MatchOption {
                src,
                some_ip,
                none_ip,
            } => match self.reg(base + *src) {
                VmValue::OptionSome(_) => *ip = *some_ip,
                VmValue::OptionNone => *ip = *none_ip,
                other => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM Option match expected Option, got `{}`.",
                        other.display()
                    )));
                }
            },
            RegInstr::MatchResult { src, ok_ip, err_ip } => match self.reg(base + *src) {
                VmValue::Variant(data) if data.name.as_ref() == "Ok" => *ip = *ok_ip,
                VmValue::Variant(data) if data.name.as_ref() == "Err" => *ip = *err_ip,
                other => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM Result match expected Result, got `{}`.",
                        other.display()
                    )));
                }
            },
            RegInstr::MatchVariant {
                src,
                expected,
                match_ip,
                else_ip,
            } => match self.reg(base + *src) {
                VmValue::Variant(data) if data.name.as_ref() == expected.as_str() => {
                    *ip = *match_ip
                }
                VmValue::Variant(_) => *ip = *else_ip,
                other => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM variant match expected `{expected}`, got `{}`.",
                        other.display()
                    )));
                }
            },
            RegInstr::MatchMapGet {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let key = map_key_from_value(self.reg(base + *key))?;
                if let Some(value) = map.borrow().get(&key).cloned() {
                    self.set_reg(base + *value_dst, value);
                    *ip = *some_ip;
                } else {
                    *ip = *none_ip;
                }
            }
            RegInstr::UnwrapSome { dst, src } => {
                let value = match self.reg(base + *src) {
                    VmValue::OptionSome(value) => (**value).clone(),
                    other => {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Some binding expected Some, got `{}`.",
                            other.display()
                        )));
                    }
                };
                self.set_reg(base + *dst, value);
            }
            RegInstr::UnwrapVariantValue { dst, src, expected } => {
                let value = match self.reg(base + *src) {
                    VmValue::Variant(data) if data.name.as_ref() == expected.as_str() => data
                        .get("value")
                        .cloned()
                        .or_else(|| {
                            (data.fields.len() == 1)
                                .then(|| data.fields.first().cloned())
                                .flatten()
                        })
                        .ok_or_else(|| {
                            EvalError::Runtime(format!(
                                "reg VM `{expected}` variant is missing value."
                            ))
                        })?,
                    other => {
                        return Err(EvalError::Runtime(format!(
                            "reg VM expected `{expected}` variant, got `{}`.",
                            other.display()
                        )));
                    }
                };
                self.set_reg(base + *dst, value);
            }
            RegInstr::RuntimeError { message } => {
                return Err(EvalError::Runtime(message.clone()));
            }
            // Collection get/set/index ops: pure (no frame push, no closure
            // call), so they belong to the tier-0 subset. Closure-driven
            // collection ops (map/filter/fold/sort-by) stay on the interpreter.
            RegInstr::ListGet { dst, list, index } => {
                let list = expect_list_ref(self.reg(base + *list))?;
                let index = expect_usize_ref(self.reg(base + *index))?;
                let value = list.borrow().get(index).cloned().ok_or_else(|| {
                    EvalError::Runtime(format!("reg VM List.get index {index} out of bounds."))
                })?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::ListLen { dst, list } => {
                let len = expect_list_ref(self.reg(base + *list))?.borrow().len();
                self.set_reg(base + *dst, VmValue::Int(len as i64));
            }
            RegInstr::ListPush { dst, list, value } => {
                let list = expect_list_ref(self.reg(base + *list))?;
                let value = self.reg(base + *value).clone();
                list.borrow_mut().push(value);
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::ListAppend { dst, list, values } => {
                // In-place to mirror `List.append(mut list, ...)`: clone the
                // source first (handles append-to-self), then extend the
                // receiver's existing buffer so a `mut` param propagates.
                let append_values = expect_list_ref(self.reg(base + *values))?.borrow().clone();
                expect_list_ref(self.reg(base + *list))?
                    .borrow_mut()
                    .extend(append_values);
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::ListClear { dst, list } => {
                expect_list_ref(self.reg(base + *list))?
                    .borrow_mut()
                    .clear();
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::ListPop { dst, list } => {
                let value = expect_list_ref(self.reg(base + *list))?
                    .borrow_mut()
                    .pop()
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone);
                self.set_reg(base + *dst, value);
            }
            RegInstr::ListRemoveAt { dst, list, index } => {
                let index = expect_int_ref(self.reg(base + *index))?;
                let list = expect_list_ref(self.reg(base + *list))?.clone();
                let mut borrowed = list.borrow_mut();
                let value = if index < 0 || index as usize >= borrowed.len() {
                    VmValue::OptionNone
                } else {
                    VmValue::OptionSome(Box::new(borrowed.remove(index as usize)))
                };
                drop(borrowed);
                self.set_reg(base + *dst, value);
            }
            RegInstr::ListSet {
                dst,
                list,
                index,
                value,
            } => {
                let index = expect_int_ref(self.reg(base + *index))?;
                let new_value = self.reg(base + *value).clone();
                let list = expect_list_ref(self.reg(base + *list))?.clone();
                let mut borrowed = list.borrow_mut();
                if index < 0 || index as usize >= borrowed.len() {
                    return Err(EvalError::Runtime(format!(
                        "reg VM List.set index {index} out of bounds for length {}.",
                        borrowed.len()
                    )));
                }
                borrowed[index as usize] = new_value;
                drop(borrowed);
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::MapGet { dst, map, key } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let key = map_key_from_value(self.reg(base + *key))?;
                let value = map
                    .borrow()
                    .get(&key)
                    .cloned()
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone);
                self.set_reg(base + *dst, value);
            }
            RegInstr::MapClear { dst, map } => {
                expect_map_ref(self.reg(base + *map))?.borrow_mut().clear();
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::MapInsert {
                dst,
                map,
                key,
                value,
            } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let key = map_key_from_value(self.reg(base + *key))?;
                let value = self.reg(base + *value).clone();
                map.borrow_mut().insert(key, value);
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::MapInsertOld {
                dst,
                map,
                key,
                value,
            } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let key = map_key_from_value(self.reg(base + *key))?;
                let value = self.reg(base + *value).clone();
                let old = map.borrow_mut().insert(key, value);
                self.set_reg(
                    base + *dst,
                    old.map(|value| VmValue::OptionSome(Box::new(value)))
                        .unwrap_or(VmValue::OptionNone),
                );
            }
            RegInstr::MapRemove { dst, map, key } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let key = map_key_from_value(self.reg(base + *key))?;
                let old = map.borrow_mut().remove(&key);
                self.set_reg(
                    base + *dst,
                    old.map(|value| VmValue::OptionSome(Box::new(value)))
                        .unwrap_or(VmValue::OptionNone),
                );
            }
            RegInstr::Return { src } => {
                return Ok(PureStep::Return(self.take_reg(base + *src)));
            }
            // Anything else is outside the pure subset; the caller handles it.
            _ => return Ok(PureStep::NotPure),
        }
        Ok(PureStep::Next)
    }

    /// Grow the shared register stack so that `stack[..upto]` is addressable.
    /// The stack only ever grows; frames are reused in place.
    fn ensure_regs(&mut self, upto: usize) {
        if self.stack.len() < upto {
            self.stack.resize(upto, VmValue::Unit);
            self.written.resize(upto, false);
        }
    }

    fn prepare_frame(&mut self, base: usize, regs: usize) {
        self.ensure_regs(base + regs);
        for written in &mut self.written[base..base + regs] {
            *written = false;
        }
    }

    #[inline(always)]
    fn reg(&self, index: usize) -> &VmValue {
        // Reading an unwritten register is a lowering/codegen invariant violation,
        // never a user-level runtime error. Assert in release too so we fail loudly
        // instead of silently observing a stale value left in the reused frame
        // window (the stack only grows and frames are reused in place).
        assert!(
            self.written.get(index).copied().unwrap_or(false),
            "reg VM internal error: read uninitialized register {index}"
        );
        &self.stack[index]
    }

    #[inline(always)]
    fn set_reg(&mut self, index: usize, value: VmValue) {
        self.stack[index] = value;
        self.written[index] = true;
    }

    /// Propagate a completing frame's `mut` parameters back to the caller: each
    /// `(caller_reg, callee_reg)` copies the parameter's final value out. A no-op
    /// for the common call with no `mut` args (empty `mut_writeback`).
    fn apply_mut_writeback(&mut self, frame: &Frame) {
        for &(caller_reg, callee_reg) in &frame.mut_writeback {
            let value = self.reg(callee_reg).clone();
            self.set_reg(caller_reg, value);
        }
    }

    #[inline(always)]
    fn take_reg(&mut self, index: usize) -> VmValue {
        assert!(
            self.written.get(index).copied().unwrap_or(false),
            "reg VM internal error: take uninitialized register {index}"
        );
        self.written[index] = false;
        std::mem::replace(&mut self.stack[index], VmValue::Unit)
    }

    // Shared register stack with frame windows. Each frame owns
    // `stack[base .. base + function.regs]`; a callee is placed immediately
    // above the caller at `base + function.regs`. The stack only grows
    // (`ensure_regs`) so recursion is bounded only by memory. Debug builds keep
    // a written-register bitmap so stale slots cannot mask lowering bugs.
    /// Synchronous call entry used by contexts that cannot suspend (closure
    /// callbacks, resource drops, the program root before the scheduler owns it).
    /// Pushes `function`'s frame and drives to completion; a suspension here is a
    /// lowering/runtime invariant violation (only `async` code awaits, and that
    /// always runs under the task scheduler).
    fn run_frame(
        &mut self,
        unit: &RegUnit,
        function: Rc<RegFunction>,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        self.ensure_regs(base + function.regs);
        let floor = self.frames.len();
        self.frames.push(Frame {
            func: function,
            ip: 0,
            base,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        });
        match self.drive(unit, floor)? {
            Outcome::Completed(value) => Ok(value),
            Outcome::Suspended => Err(EvalError::Runtime(
                "reg VM cannot suspend (await/blocking op) inside a synchronous context."
                    .to_string(),
            )),
        }
    }

    /// Run `name` (the program entry, usually `main`) as the root task under the
    /// cooperative scheduler and return its value. Other tasks created via
    /// `spawn`/`async let` are interleaved at suspension points.
    fn run_program(&mut self, name: &str) -> Result<VmValue, EvalError> {
        let function_id = self.unit.function_ids.get(name).copied().ok_or_else(|| {
            EvalError::Runtime(format!("reg VM cannot resolve function `{name}`."))
        })?;
        let unit = Rc::clone(&self.unit);
        let func = Rc::clone(&unit.functions[function_id]);
        let root = self.create_task(func, Vec::new());
        self.run_scheduler(&unit, root)
    }

    /// Register a new ready task running `func` with `args` placed in its first
    /// registers (its own private register stack, base 0).
    fn create_task(&mut self, func: Rc<RegFunction>, args: Vec<VmValue>) -> TaskId {
        let tid = self.next_task_id;
        self.next_task_id += 1;
        let regs = func.regs.max(args.len());
        let mut stack = vec![VmValue::Unit; regs];
        let mut written = vec![false; regs];
        for (index, arg) in args.into_iter().enumerate() {
            stack[index] = arg;
            written[index] = true;
        }
        let frames = vec![Frame {
            func,
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        }];
        self.tasks.insert(
            tid,
            TaskSlot {
                saved: Some(SavedTask {
                    frames,
                    stack,
                    written,
                }),
                done: None,
                wait: None,
                resume_dst: usize::MAX,
            },
        );
        self.ready_queue.push_back(tid);
        tid
    }

    /// Make `tid` the running task: move its parked register state into `self`.
    fn swap_in(&mut self, tid: TaskId) {
        let saved = self
            .tasks
            .get_mut(&tid)
            .expect("task slot")
            .saved
            .take()
            .expect("parked task state");
        self.frames = saved.frames;
        self.stack = saved.stack;
        self.written = saved.written;
        self.current_task = tid;
    }

    /// Park the running task: move its register state back into its slot.
    fn swap_out(&mut self, tid: TaskId) {
        let saved = SavedTask {
            frames: std::mem::take(&mut self.frames),
            stack: std::mem::take(&mut self.stack),
            written: std::mem::take(&mut self.written),
        };
        self.tasks.get_mut(&tid).expect("task slot").saved = Some(saved);
    }

    fn run_scheduler(&mut self, unit: &RegUnit, root: TaskId) -> Result<VmValue, EvalError> {
        loop {
            let Some(tid) = self.ready_queue.pop_front() else {
                // Nothing runnable: advance the clock to the earliest sleep
                // deadline and wake those timers. If there are no sleepers either,
                // every remaining task is blocked forever — a deadlock.
                if self.wake_due_sleepers()? {
                    continue;
                }
                return Err(EvalError::Runtime(
                    "reg VM async scheduler stalled: every task is blocked (deadlock).".to_string(),
                ));
            };
            // Skip stale queue entries (finished, still parked, or running).
            match self.tasks.get(&tid) {
                Some(slot)
                    if slot.done.is_none() && slot.wait.is_none() && slot.saved.is_some() => {}
                _ => continue,
            }
            self.swap_in(tid);
            match self.drive(unit, 0) {
                Ok(Outcome::Completed(value)) => {
                    // Drop the finished task's register state.
                    self.frames = Vec::new();
                    self.stack = Vec::new();
                    self.written = Vec::new();
                    if tid == root {
                        return Ok(value);
                    }
                    self.tasks.get_mut(&tid).expect("task slot").done = Some(value);
                }
                Ok(Outcome::Suspended) => {
                    let suspension = self.suspension.take().expect("suspension recorded");
                    self.swap_out(tid);
                    let slot = self.tasks.get_mut(&tid).expect("task slot");
                    slot.resume_dst = suspension.resume_dst;
                    slot.wait = Some(suspension.wait);
                }
                // A hard VM error in any task aborts the whole program.
                Err(error) => return Err(error),
            }
            // A send/recv/finish may have unblocked parked tasks.
            self.satisfy_waiters()?;
        }
    }

    /// Idle step: with no ready task, find the earliest `Sleep` deadline, sleep
    /// the host thread until then, and wake every timer that is now due. Returns
    /// `false` when there are no sleeping tasks (so the caller can report a stall).
    fn wake_due_sleepers(&mut self) -> Result<bool, EvalError> {
        let earliest = self
            .tasks
            .values()
            .filter_map(|slot| match &slot.wait {
                Some(Wait::Sleep { deadline, .. }) => Some(*deadline),
                _ => None,
            })
            .min();
        let Some(deadline) = earliest else {
            return Ok(false);
        };
        let now = std::time::Instant::now();
        if deadline > now {
            std::thread::sleep(deadline - now);
        }
        let now = std::time::Instant::now();
        let due: Vec<TaskId> = self
            .tasks
            .iter()
            .filter(|(_, slot)| {
                matches!(&slot.wait, Some(Wait::Sleep { deadline, .. }) if *deadline <= now)
            })
            .map(|(id, _)| *id)
            .collect();
        for tid in due {
            self.resolve_wait(tid)?;
        }
        self.satisfy_waiters()?;
        Ok(true)
    }

    /// Repeatedly wake any parked task whose wait is now satisfiable, until no
    /// further progress (a fixpoint), so a single send can cascade-wake a chain.
    fn satisfy_waiters(&mut self) -> Result<(), EvalError> {
        loop {
            let ready: Vec<TaskId> = self
                .tasks
                .iter()
                .filter(|(_, slot)| slot.done.is_none())
                .filter(|(_, slot)| match &slot.wait {
                    Some(Wait::Recv { channel }) => self.channel_ready(*channel),
                    Some(Wait::Send { sender, .. }) => self.channel_has_space(sender.channel_id),
                    Some(Wait::Join { task }) => {
                        self.tasks.get(task).is_some_and(|s| s.done.is_some())
                    }
                    // Sleeps are woken by the scheduler's clock step, not here.
                    Some(Wait::Sleep { .. }) => false,
                    Some(Wait::Select { handles, .. }) => handles
                        .iter()
                        .any(|h| self.tasks.get(h).is_some_and(|s| s.done.is_some())),
                    None => false,
                })
                .map(|(id, _)| *id)
                .collect();
            if ready.is_empty() {
                return Ok(());
            }
            for tid in ready {
                self.resolve_wait(tid)?;
            }
        }
    }

    /// Cancel every losing `select` arm task once a winner is chosen. A resolved
    /// `select` keeps only the winner; the backend drops the losing arms' futures
    /// so they stop immediately, and the VM must do the same — otherwise a loser
    /// would keep being scheduled at later suspension points, run its remaining
    /// side effects, and could even abort the whole program with a late error.
    /// Removing the task slot makes any stale ready-queue entry a no-op and stops
    /// the scheduler (and sleeper wakeups) from ever resuming it.
    fn cancel_select_losers(&mut self, handles: &[TaskId], winner: TaskId) {
        for handle in handles {
            if *handle != winner {
                self.tasks.remove(handle);
            }
        }
    }

    /// Produce the result of `tid`'s satisfied wait and re-queue it.
    fn resolve_wait(&mut self, tid: TaskId) -> Result<(), EvalError> {
        let wait = self
            .tasks
            .get_mut(&tid)
            .expect("task slot")
            .wait
            .take()
            .expect("parked wait");
        match wait {
            Wait::Recv { channel } => {
                let result = json_result(self.channel_recv(channel));
                self.complete_wait(tid, result);
            }
            Wait::Send { sender, value } => {
                let result = json_result(self.channel_send(sender, value));
                self.complete_wait(tid, result);
            }
            Wait::Join { task } => {
                let result = self
                    .tasks
                    .get(&task)
                    .and_then(|slot| slot.done.clone())
                    .expect("joined task finished");
                self.complete_wait(tid, result);
            }
            Wait::Sleep { .. } => {
                self.complete_wait(tid, value_ok(VmValue::Unit));
            }
            Wait::Select {
                handles,
                winner_dst,
                value_dst,
            } => {
                // First finished arm wins; its value goes to `value_dst`, its arm
                // index to `winner_dst`. The losing arms are cancelled (see
                // `cancel_select_losers`) so they cannot keep running.
                let (index, task) = handles
                    .iter()
                    .enumerate()
                    .find(|(_, h)| self.tasks.get(h).is_some_and(|s| s.done.is_some()))
                    .map(|(i, h)| (i, *h))
                    .expect("a select arm finished");
                let value = self
                    .tasks
                    .get(&task)
                    .and_then(|slot| slot.done.clone())
                    .expect("winning arm value");
                self.cancel_select_losers(&handles, task);
                self.write_saved_reg(tid, winner_dst, VmValue::Int(index as i64));
                self.complete_wait_at(tid, value_dst, value);
            }
        }
        Ok(())
    }

    /// Write `value` into a parked task's saved register window (no re-queue).
    fn write_saved_reg(&mut self, tid: TaskId, dst: usize, value: VmValue) {
        let saved = self
            .tasks
            .get_mut(&tid)
            .expect("task slot")
            .saved
            .as_mut()
            .expect("parked task state");
        if dst >= saved.stack.len() {
            saved.stack.resize(dst + 1, VmValue::Unit);
            saved.written.resize(dst + 1, false);
        }
        saved.stack[dst] = value;
        saved.written[dst] = true;
    }

    /// Write a woken task's result into its recorded `resume_dst` and re-queue it.
    fn complete_wait(&mut self, tid: TaskId, result: VmValue) {
        let dst = self.tasks.get(&tid).expect("task slot").resume_dst;
        self.complete_wait_at(tid, dst, result);
    }

    /// Write `result` into a parked task's register `dst` and re-queue it.
    fn complete_wait_at(&mut self, tid: TaskId, dst: usize, result: VmValue) {
        self.write_saved_reg(tid, dst, result);
        self.ready_queue.push_back(tid);
    }

    fn channel_ready(&self, channel: i64) -> bool {
        self.channels.get(&channel).is_none_or(|state| {
            !state.queue.is_empty() || state.senders == 0 || state.receiver_closed
        })
    }

    fn channel_has_space(&self, channel: i64) -> bool {
        self.channels
            .get(&channel)
            .is_none_or(|state| state.receiver_closed || state.queue.len() < state.capacity)
    }

    /// Park the running task for `ms` milliseconds (clamped at 0). The scheduler's
    /// clock step wakes it when the deadline passes; the `await` result is `Ok`.
    fn park_sleep_ms(&mut self, ms: i64) {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(ms.max(0) as u64);
        self.suspension = Some(Suspension {
            wait: Wait::Sleep { deadline },
            resume_dst: usize::MAX,
        });
    }

    /// True when `Sender.send` on an open channel would block (buffer full).
    fn channel_send_would_block(&self, sender: &VmSender) -> bool {
        if sender.closed {
            return false;
        }
        self.channels
            .get(&sender.channel_id)
            .is_some_and(|state| !state.receiver_closed && state.queue.len() >= state.capacity)
    }

    /// Drive the explicit call stack until the frame at depth `floor` returns
    /// (`Completed`) or a blocking operation parks the current task
    /// (`Suspended`, with the wait recorded in `self.suspension`). `floor` is
    /// the stack depth below the frame we are running.
    fn drive(&mut self, unit: &RegUnit, floor: usize) -> Result<Outcome, EvalError> {
        'frames: loop {
            // Hoist the current (top) frame into fast locals. The instruction
            // body below only references `base`/`next_base`/`ip`/`unit`, so it is
            // byte-for-byte the recursive interpreter; only `CallKnown`/`Return`
            // (and falling off the end) manipulate the frame stack.
            let func = {
                let frame = self.frames.last().expect("active frame");
                Rc::clone(&frame.func)
            };
            let base = self.frames.last().expect("active frame").base;
            let next_base = base + func.regs;
            let mut ip = self.frames.last().expect("active frame").ip;

            // Native JIT tier: a fresh frame whose function compiles to machine
            // code runs there first (the integer/control core). Completes exactly
            // like the `Return` arm. Falls through if not native-eligible or the
            // native code bailed on an edge.
            #[cfg(feature = "native-jit")]
            if ip == 0
                && self.native.is_some()
                // Inline negative check: skip the `try_native` call entirely for
                // functions already known not native-eligible (just a `Cell` read).
                && func.native_status.get() != NATIVE_STATUS_NOT_ELIGIBLE
                && let Some(value) = self.try_native(&func, base)
            {
                let frame = self.frames.pop().expect("active frame");
                self.apply_mut_writeback(&frame);
                if self.frames.len() == floor {
                    return Ok(Outcome::Completed(value));
                }
                self.set_reg(frame.ret_dst, value);
                continue 'frames;
            }

            // Tier-0 JIT: a fresh JIT-eligible frame runs via the specializing
            // executor (which reuses the interpreter's semantics), then completes
            // exactly like the `Return` arm. Eligible functions never suspend, so
            // they are always entered at `ip == 0`.
            if self.jit_enabled && ip == 0 && self.is_jit_eligible(&func) {
                let value = self.run_jit(unit, &func, base)?;
                let frame = self.frames.pop().expect("active frame");
                self.apply_mut_writeback(&frame);
                if self.frames.len() == floor {
                    return Ok(Outcome::Completed(value));
                }
                self.set_reg(frame.ret_dst, value);
                continue 'frames;
            }

            while let Some(instr) = func.code.get(ip) {
                ip += 1;
                // Pure instructions (loads, arithmetic, jumps, matches, heap
                // construction, …) run through the shared `try_exec_pure`, the one
                // copy of their semantics that the JIT executor also uses — so the
                // two can never diverge. Only frame/suspension/call-shaped
                // instructions need the interpreter-specific handling below.
                match self.try_exec_pure(instr, base, &mut ip)? {
                    PureStep::Next => {}
                    PureStep::Return(value) => {
                        let frame = self.frames.pop().expect("active frame");
                        self.apply_mut_writeback(&frame);
                        if self.frames.len() == floor {
                            return Ok(Outcome::Completed(value));
                        }
                        self.set_reg(frame.ret_dst, value);
                        continue 'frames;
                    }
                    PureStep::NotPure => match instr {
                        RegInstr::ResourceDrop { resource } => {
                            let value = self.reg(base + *resource).clone();
                            self.run_resource_drop(unit, value, next_base)?;
                        }
                        RegInstr::CallKnown {
                            dst,
                            function: callee_id,
                            args,
                            mut_args,
                        } => {
                            let callee = Rc::clone(&unit.functions[*callee_id]);
                            self.prepare_frame(next_base, callee.regs);
                            for (index, reg) in args.iter().enumerate() {
                                let value = self.reg(base + *reg).clone();
                                self.set_reg(next_base + index, value);
                            }
                            // `mut` args: when this frame completes, write each
                            // parameter's final value back to the caller's register
                            // so mutations propagate (caller_abs_reg, callee_abs_reg).
                            let mut_writeback = mut_args
                                .iter()
                                .map(|&pos| (base + args[pos], next_base + pos))
                                .collect();
                            // Stackless call: save our resume point, push the callee, and
                            // re-enter the driver loop instead of recursing on the host
                            // stack — so an `await` deep in this chain can later suspend it.
                            self.frames.last_mut().expect("active frame").ip = ip;
                            self.frames.push(Frame {
                                func: callee,
                                ip: 0,
                                base: next_base,
                                ret_dst: base + *dst,
                                mut_writeback,
                            });
                            continue 'frames;
                        }
                        RegInstr::CallDynamic {
                            dst,
                            dispatch,
                            args,
                            mut_args,
                        } => {
                            // Select the concrete impl by the runtime struct type of
                            // the receiver (args[0]), then call it like `CallKnown`.
                            let receiver = self.reg(base + args[0]).clone();
                            let type_name = match &receiver {
                                VmValue::Struct(data) => Some(data.name.clone()),
                                _ => None,
                            };
                            let callee_id = type_name.as_ref().and_then(|name| {
                                dispatch
                                    .iter()
                                    .find(|(struct_name, _)| struct_name.as_str() == &**name)
                                    .map(|(_, id)| *id)
                            });
                            let Some(callee_id) = callee_id else {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM dynamic protocol dispatch found no impl for receiver `{}`.",
                                    type_name.as_deref().unwrap_or("<non-struct value>")
                                )));
                            };
                            let callee = Rc::clone(&unit.functions[callee_id]);
                            self.prepare_frame(next_base, callee.regs);
                            for (index, reg) in args.iter().enumerate() {
                                let value = self.reg(base + *reg).clone();
                                self.set_reg(next_base + index, value);
                            }
                            let mut_writeback = mut_args
                                .iter()
                                .map(|&pos| (base + args[pos], next_base + pos))
                                .collect();
                            self.frames.last_mut().expect("active frame").ip = ip;
                            self.frames.push(Frame {
                                func: callee,
                                ip: 0,
                                base: next_base,
                                ret_dst: base + *dst,
                                mut_writeback,
                            });
                            continue 'frames;
                        }
                        RegInstr::SpawnTask {
                            dst,
                            function: callee_id,
                            args,
                        } => {
                            let callee = Rc::clone(&unit.functions[*callee_id]);
                            let arg_values = args
                                .iter()
                                .map(|reg| self.reg(base + *reg).clone())
                                .collect::<Vec<_>>();
                            let tid = self.create_task(callee, arg_values);
                            self.set_reg(base + *dst, task_handle_value(tid));
                        }
                        RegInstr::AwaitJoin { dst, src } => {
                            let value = self.reg(base + *src).clone();
                            match as_task_handle(&value) {
                                Some(task) => {
                                    match self.tasks.get(&task).and_then(|s| s.done.clone()) {
                                        // Already finished: take its value, no park.
                                        Some(result) => self.set_reg(base + *dst, result),
                                        // Park until the joined task completes.
                                        None => {
                                            self.suspension = Some(Suspension {
                                                wait: Wait::Join { task },
                                                resume_dst: base + *dst,
                                            });
                                        }
                                    }
                                }
                                // Not a handle: `await` of an already-evaluated value.
                                None => self.set_reg(base + *dst, value),
                            }
                        }
                        RegInstr::SelectWait {
                            handles,
                            winner,
                            value,
                        } => {
                            let tids = handles
                                .iter()
                                .map(|reg| {
                                    as_task_handle(self.reg(base + *reg)).ok_or_else(|| {
                                        EvalError::Runtime(
                                            "reg VM select arm did not produce a task.".to_string(),
                                        )
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                            // If an arm already finished, resolve immediately; else park.
                            let ready = tids
                                .iter()
                                .enumerate()
                                .find(|(_, tid)| {
                                    self.tasks.get(tid).is_some_and(|s| s.done.is_some())
                                })
                                .map(|(index, tid)| (index, *tid));
                            match ready {
                                Some((index, won_tid)) => {
                                    let won = self
                                        .tasks
                                        .get(&won_tid)
                                        .and_then(|s| s.done.clone())
                                        .expect("done");
                                    self.cancel_select_losers(&tids, won_tid);
                                    self.set_reg(base + *winner, VmValue::Int(index as i64));
                                    self.set_reg(base + *value, won);
                                }
                                None => {
                                    self.suspension = Some(Suspension {
                                        wait: Wait::Select {
                                            handles: tids,
                                            winner_dst: base + *winner,
                                            value_dst: base + *value,
                                        },
                                        resume_dst: usize::MAX,
                                    });
                                }
                            }
                        }
                        RegInstr::CallNative {
                            dst,
                            key,
                            args,
                            mut_args,
                        } => {
                            let result = self.call_native_key(key, args, mut_args, base)?;
                            self.set_reg(base + *dst, result);
                        }
                        RegInstr::CallClosure { dst, closure, args } => {
                            let closure = match self.reg(base + *closure) {
                                VmValue::Closure(closure) => Rc::clone(closure),
                                other => {
                                    return Err(EvalError::Runtime(format!(
                                        "reg VM expected Closure, got `{}`.",
                                        other.display()
                                    )));
                                }
                            };
                            let result =
                                self.call_closure_from_regs(unit, &closure, args, base, next_base)?;
                            self.set_reg(base + *dst, result);
                        }
                        RegInstr::ListFilter {
                            dst,
                            list,
                            predicate,
                        } => {
                            let list = expect_list_ref(self.reg(base + *list))?;
                            let predicate = expect_closure_rc(self.reg(base + *predicate))?;
                            let result = self.filter_list(unit, list, &predicate, next_base)?;
                            self.set_reg(base + *dst, result);
                        }
                        RegInstr::ListFold {
                            dst,
                            list,
                            state,
                            folder,
                        } => {
                            let list = expect_list_ref(self.reg(base + *list))?;
                            let state = self.reg(base + *state).clone();
                            let folder = expect_closure_rc(self.reg(base + *folder))?;
                            let result = self.fold_list(unit, list, state, &folder, next_base)?;
                            self.set_reg(base + *dst, result);
                        }
                        RegInstr::ListMap { dst, list, mapper } => {
                            let list = expect_list_ref(self.reg(base + *list))?;
                            let mapper = expect_closure_rc(self.reg(base + *mapper))?;
                            let result = self.map_list(unit, list, &mapper, next_base)?;
                            self.set_reg(base + *dst, result);
                        }
                        RegInstr::ListSort { dst, list } => {
                            let list = expect_list_ref(self.reg(base + *list))?.clone();
                            let mut borrowed = list.borrow_mut();
                            sort_vm_values(&mut borrowed)?;
                            drop(borrowed);
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::ListSortBy {
                            dst,
                            list,
                            key,
                            compare,
                        } => {
                            let values = expect_list_ref(self.reg(base + *list))?.borrow().clone();
                            let key = expect_closure_rc(self.reg(base + *key))?;
                            let compare = expect_closure_rc(self.reg(base + *compare))?;
                            let sorted =
                                self.sort_list_by_closure(unit, values, &key, &compare, next_base)?;
                            self.set_reg(base + *dst, VmValue::List(Rc::new(RefCell::new(sorted))));
                        }
                        RegInstr::ListSortWith { dst, list, compare } => {
                            // Sort a detached copy first so the comparator closure can read
                            // the list without a RefCell double-borrow, then overwrite the
                            // receiver's buffer in place so `mut list` propagates.
                            let mut values =
                                expect_list_ref(self.reg(base + *list))?.borrow().clone();
                            let compare = expect_closure_rc(self.reg(base + *compare))?;
                            self.sort_list_with_closure(unit, &mut values, &compare, next_base)?;
                            *expect_list_ref(self.reg(base + *list))?.borrow_mut() = values;
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::DequeClear { dst, deque } => {
                            expect_deque_ref(self.reg(base + *deque))?
                                .borrow_mut()
                                .clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::DequePopBack { dst, deque } => {
                            let value = expect_deque_ref(self.reg(base + *deque))?
                                .borrow_mut()
                                .pop_back()
                                .map(|value| VmValue::OptionSome(Box::new(value)))
                                .unwrap_or(VmValue::OptionNone);
                            self.set_reg(base + *dst, value);
                        }
                        RegInstr::DequePopFront { dst, deque } => {
                            let value = expect_deque_ref(self.reg(base + *deque))?
                                .borrow_mut()
                                .pop_front() // O(1), unlike the old `Vec::remove(0)`
                                .map(|value| VmValue::OptionSome(Box::new(value)))
                                .unwrap_or(VmValue::OptionNone);
                            self.set_reg(base + *dst, value);
                        }
                        RegInstr::DequePushBack { dst, deque, value } => {
                            let value = self.reg(base + *value).clone();
                            expect_deque_ref(self.reg(base + *deque))?
                                .borrow_mut()
                                .push_back(value);
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::DequePushFront { dst, deque, value } => {
                            let value = self.reg(base + *value).clone();
                            expect_deque_ref(self.reg(base + *deque))?
                                .borrow_mut()
                                .push_front(value); // O(1), unlike the old `Vec::insert(0, _)`
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SetClear { dst, set } => {
                            expect_list_ref(self.reg(base + *set))?.borrow_mut().clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SetForEach { dst, set, callback } => {
                            let set = expect_list_ref(self.reg(base + *set))?;
                            let callback = expect_closure_rc(self.reg(base + *callback))?;
                            let len = set.borrow().len();
                            for index in 0..len {
                                let value = set.borrow()[index].clone();
                                let _ = self.call_closure_one(unit, &callback, value, next_base)?;
                            }
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SetInsert { dst, set, value } => {
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *set))?;
                            let inserted = set_insert_vm(&mut list.borrow_mut(), value);
                            self.set_reg(base + *dst, VmValue::Bool(inserted));
                        }
                        RegInstr::SetRemove { dst, set, value } => {
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *set))?;
                            let removed = set_remove_vm(&mut list.borrow_mut(), &value);
                            self.set_reg(base + *dst, VmValue::Bool(removed));
                        }
                        RegInstr::SortedSetClear { dst, set } => {
                            expect_list_ref(self.reg(base + *set))?.borrow_mut().clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SortedSetInsert { dst, set, value } => {
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *set))?;
                            let inserted = sorted_insert_vm(&mut list.borrow_mut(), value)?;
                            self.set_reg(base + *dst, VmValue::Bool(inserted));
                        }
                        RegInstr::SortedSetRemove { dst, set, value } => {
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *set))?;
                            let removed = sorted_remove_vm(&mut list.borrow_mut(), &value)?;
                            self.set_reg(base + *dst, VmValue::Bool(removed));
                        }
                        RegInstr::SortedMapClear { dst, map } => {
                            expect_list_ref(self.reg(base + *map))?.borrow_mut().clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SortedMapInsert {
                            dst,
                            map,
                            key,
                            value,
                        } => {
                            let key = self.reg(base + *key).clone();
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *map))?;
                            sorted_map_insert_in_place(&mut list.borrow_mut(), key, value)?;
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SortedMapRemove { dst, map, key } => {
                            let key = self.reg(base + *key).clone();
                            let list = expect_list_ref(self.reg(base + *map))?;
                            let removed = sorted_map_remove_in_place(&mut list.borrow_mut(), &key)?;
                            self.set_reg(
                                base + *dst,
                                removed
                                    .map(|value| VmValue::OptionSome(Box::new(value)))
                                    .unwrap_or(VmValue::OptionNone),
                            );
                        }
                        RegInstr::BufferClear { dst, buffer } => {
                            expect_bytes_ref(self.reg(base + *buffer))?;
                            self.set_reg(base + *buffer, VmValue::Bytes(Rc::new(Vec::new())));
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::CounterAdd {
                            dst,
                            counter,
                            amount,
                        } => {
                            let counter_reg = base + *counter;
                            let value = expect_counter_value(self.reg(counter_reg))?
                                + expect_int_ref(self.reg(base + *amount))?;
                            self.set_reg(counter_reg, counter_value(value));
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::ConfigStoreReplace { dst, store, value } => {
                            let store_reg = base + *store;
                            let name = expect_config_value_name(self.reg(base + *value))?;
                            self.set_reg(store_reg, config_store_value(name));
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::GlobalConfigReplace { dst, global, value } => {
                            let global_reg = base + *global;
                            let rule_count = expect_config_rule_count(self.reg(base + *value))?;
                            self.set_reg(global_reg, global_config_value(rule_count));
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::StringBuilderPush {
                            dst,
                            builder,
                            value,
                        } => {
                            let mut builder_value =
                                expect_string_ref(self.reg(base + *builder))?.to_string();
                            let value = expect_string_ref(self.reg(base + *value))?;
                            builder_value.push_str(value);
                            self.set_reg(base + *builder, VmValue::string(builder_value));
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::StringConcat { dst, left, right } => {
                            let value = {
                                let left = expect_string_ref(self.reg(base + *left))?;
                                let right = expect_string_ref(self.reg(base + *right))?;
                                let mut value = String::with_capacity(left.len() + right.len());
                                value.push_str(left);
                                value.push_str(right);
                                value
                            };
                            self.set_reg(base + *dst, VmValue::string(value));
                        }
                        RegInstr::CallIntrinsic {
                            dst,
                            intrinsic,
                            args,
                        } => {
                            let value =
                                self.call_intrinsic(unit, *intrinsic, args, base, next_base)?;
                            // A blocking intrinsic (channel/sleep) parked the task and left
                            // `resume_dst` unfilled; record where its result must land. The
                            // end-of-loop check then yields to the scheduler.
                            if let Some(suspension) = self.suspension.as_mut() {
                                suspension.resume_dst = base + *dst;
                            } else {
                                self.set_reg(base + *dst, value);
                            }
                        }
                        RegInstr::CallTypedIntrinsic {
                            dst,
                            intrinsic,
                            type_arg,
                            args,
                        } => {
                            let value =
                                self.call_typed_intrinsic(unit, *intrinsic, type_arg, args, base)?;
                            self.set_reg(base + *dst, value);
                        }
                        RegInstr::TryResult { dst, src, cleanup } => {
                            let value = self.reg(base + *src).clone();
                            // `?` keeps the success payload (`Ok(x)`/`Some(x)`) and
                            // short-circuits on failure (`Err(e)`/`None`), returning
                            // that failure from the current frame. Option support
                            // mirrors Result so `?` works in `Option`-returning fns.
                            let short_circuit = match &value {
                                VmValue::OptionSome(inner) => {
                                    self.set_reg(base + *dst, (**inner).clone());
                                    None
                                }
                                VmValue::OptionNone => Some(VmValue::OptionNone),
                                _ => match result_variant_payload(&value)? {
                                    Ok(payload) => {
                                        self.set_reg(base + *dst, payload);
                                        None
                                    }
                                    Err(error) => Some(value_err(error)),
                                },
                            };
                            if let Some(return_value) = short_circuit {
                                for resource in cleanup {
                                    let resource_value = self.reg(base + *resource).clone();
                                    self.run_resource_drop(unit, resource_value, next_base)?;
                                }
                                // Short-circuit: return the failure from the *current*
                                // frame only (pop one frame like `Return`), not out of
                                // the whole stackless driver.
                                let frame = self.frames.pop().expect("active frame");
                                self.apply_mut_writeback(&frame);
                                if self.frames.len() == floor {
                                    return Ok(Outcome::Completed(return_value));
                                }
                                self.set_reg(frame.ret_dst, return_value);
                                continue 'frames;
                            }
                        }
                        // `Return` and the rest of the pure subset are handled above by
                        // `try_exec_pure`; reaching this arm means an instruction is in
                        // neither the pure subset nor the impure arms — a lowering bug.
                        _ => unreachable!(
                            "reg VM instruction handled by neither try_exec_pure nor the interpreter: {instr:?}"
                        ),
                    },
                }
                // A blocking op (channel/sleep/join) parked the task: save the
                // resume point (`ip` already points past the instruction, so on
                // wake the scheduler writes the result into `resume_dst` and we
                // continue here) and hand control back to the scheduler.
                if self.suspension.is_some() {
                    self.frames.last_mut().expect("active frame").ip = ip;
                    return Ok(Outcome::Suspended);
                }
            }
            // Fell off the end of the function body without an explicit `Return`.
            // Lowering always appends one, so this is a defensive `Unit` return.
            let frame = self.frames.pop().expect("active frame");
            self.apply_mut_writeback(&frame);
            if self.frames.len() == floor {
                return Ok(Outcome::Completed(VmValue::Unit));
            }
            self.set_reg(frame.ret_dst, VmValue::Unit);
        }
    }

    fn run_resource_drop(
        &mut self,
        unit: &RegUnit,
        value: VmValue,
        base: usize,
    ) -> Result<(), EvalError> {
        if self.finish_resource_pool_lease(value.clone())? {
            return Ok(());
        }
        let VmValue::Struct(data) = value else {
            return Ok(());
        };
        let Some(function_id) = unit
            .resource_drop_functions
            .get(data.name.as_ref())
            .copied()
        else {
            return Ok(());
        };
        let callee = Rc::clone(&unit.functions[function_id]);
        self.prepare_frame(base, callee.regs);
        for (field, value) in data.iter() {
            if let Some(reg) = callee.local_regs.get(field.as_ref()) {
                self.set_reg(base + *reg, value.clone());
            }
        }
        let result = self.run_frame(unit, callee, base)?;
        if matches!(result, VmValue::Unit) {
            Ok(())
        } else {
            Err(EvalError::Runtime(format!(
                "resource drop for `{}` returned unsupported value `{}`.",
                data.name,
                result.display()
            )))
        }
    }

    fn tcp_connect(&mut self, host: &str, port: i64) -> Result<VmValue, VmValue> {
        if port <= 0 || port > u16::MAX as i64 {
            return Err(tcp_error_value("TCP port must be between 1 and 65535"));
        }
        let stream = TcpStream::connect(format!("{host}:{port}")).map_err(|error| {
            tcp_error_value(format!("TCP connect to `{host}:{port}` failed: {error}"))
        })?;
        let timeout = Some(std::time::Duration::from_secs(5));
        let _ = stream.set_read_timeout(timeout);
        let _ = stream.set_write_timeout(timeout);
        let id = self.next_tcp_stream_id;
        self.next_tcp_stream_id = self.next_tcp_stream_id.saturating_add(1);
        self.tcp_streams.insert(id, stream);
        Ok(tcp_stream_value(id))
    }

    fn tcp_stream_mut(&mut self, id: i64) -> Result<&mut TcpStream, VmValue> {
        self.tcp_streams
            .get_mut(&id)
            .ok_or_else(|| tcp_error_value(format!("unknown TcpStream id `{id}`")))
    }

    fn tcp_stream_read(&mut self, id: i64, max_bytes: i64) -> Result<Vec<u8>, VmValue> {
        if max_bytes <= 0 {
            return Err(tcp_error_value("TCP read max_bytes must be positive"));
        }
        let mut buffer = vec![0; max_bytes as usize];
        let read = self
            .tcp_stream_mut(id)?
            .read(&mut buffer)
            .map_err(|error| tcp_error_value(format!("TCP read failed: {error}")))?;
        buffer.truncate(read);
        Ok(buffer)
    }

    fn tcp_stream_write(&mut self, id: i64, data: &[u8]) -> Result<i64, VmValue> {
        self.tcp_stream_mut(id)?
            .write(data)
            .map(|written| written as i64)
            .map_err(|error| tcp_error_value(format!("TCP write failed: {error}")))
    }

    fn tcp_stream_write_all(&mut self, id: i64, data: &[u8]) -> Result<(), VmValue> {
        self.tcp_stream_mut(id)?
            .write_all(data)
            .map_err(|error| tcp_error_value(format!("TCP write_all failed: {error}")))
    }

    fn tcp_stream_shutdown(&mut self, id: i64) -> Result<(), VmValue> {
        self.tcp_stream_mut(id)?
            .shutdown(Shutdown::Both)
            .map_err(|error| tcp_error_value(format!("TCP shutdown failed: {error}")))
    }

    fn websocket_connect(&mut self, url: &str) -> Result<VmValue, VmValue> {
        let (host_port, path) = parse_ws_url(url)?;
        let mut stream = TcpStream::connect(&host_port).map_err(|error| {
            websocket_error_value(format!("WebSocket connect to `{url}` failed: {error}"))
        })?;
        let timeout = Some(std::time::Duration::from_secs(5));
        let _ = stream.set_read_timeout(timeout);
        let _ = stream.set_write_timeout(timeout);
        let key = "cnNzY3JpcHQtcmVnLXZt";
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).map_err(|error| {
            websocket_error_value(format!("WebSocket handshake write failed: {error}"))
        })?;
        let mut response = Vec::new();
        let mut byte = [0; 1];
        while !response.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).map_err(|error| {
                websocket_error_value(format!("WebSocket handshake read failed: {error}"))
            })?;
            response.push(byte[0]);
            if response.len() > 8192 {
                return Err(websocket_error_value(
                    "WebSocket handshake response is too large",
                ));
            }
        }
        let response_text = String::from_utf8_lossy(&response);
        if !response_text.starts_with("HTTP/1.1 101 ")
            && !response_text.starts_with("HTTP/1.0 101 ")
        {
            return Err(websocket_error_value(format!(
                "WebSocket handshake failed: {}",
                response_text.lines().next().unwrap_or("")
            )));
        }
        let id = self.next_websocket_id;
        self.next_websocket_id = self.next_websocket_id.saturating_add(1);
        self.websockets.insert(id, stream);
        Ok(websocket_value(id))
    }

    fn websocket_stream_mut(&mut self, id: i64) -> Result<&mut TcpStream, VmValue> {
        self.websockets
            .get_mut(&id)
            .ok_or_else(|| websocket_error_value(format!("unknown WebSocket id `{id}`")))
    }

    fn websocket_send(&mut self, id: i64, opcode: u8, payload: &[u8]) -> Result<(), VmValue> {
        websocket_write_frame(self.websocket_stream_mut(id)?, opcode, payload)
    }

    fn websocket_recv(
        &mut self,
        id: i64,
        expected: WebSocketExpectedFrame,
    ) -> Result<Option<Vec<u8>>, VmValue> {
        loop {
            let frame = websocket_read_frame(self.websocket_stream_mut(id)?)?;
            match frame.opcode {
                0x1 if matches!(expected, WebSocketExpectedFrame::Text) => {
                    return Ok(Some(frame.payload));
                }
                0x2 if matches!(expected, WebSocketExpectedFrame::Binary) => {
                    return Ok(Some(frame.payload));
                }
                0x8 => return Ok(None),
                0x9 => {
                    websocket_write_frame(self.websocket_stream_mut(id)?, 0xA, &frame.payload)?;
                }
                0xA => {}
                0x1 => {
                    return Err(websocket_error_value(
                        "WebSocket received text frame while waiting for bytes",
                    ));
                }
                0x2 => {
                    return Err(websocket_error_value(
                        "WebSocket received binary frame while waiting for text",
                    ));
                }
                opcode => {
                    return Err(websocket_error_value(format!(
                        "WebSocket received unsupported opcode {opcode}"
                    )));
                }
            }
        }
    }

    fn websocket_close(&mut self, id: i64) -> Result<(), VmValue> {
        websocket_write_frame(self.websocket_stream_mut(id)?, 0x8, &[])
    }

    fn resource_pool_new(
        &mut self,
        unit: &RegUnit,
        args: &[Reg],
        base: usize,
        next_base: usize,
        lazy: bool,
        factory_returns_result: bool,
    ) -> Result<VmValue, EvalError> {
        let factory = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 0)?)?;
        let max_size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
        let capacity = max_size.max(0);
        let mut idle = Vec::new();
        if !lazy {
            idle.reserve(capacity as usize);
            for _ in 0..capacity {
                let value = self.call_closure_zero(unit, &factory, next_base)?;
                if factory_returns_result {
                    match result_variant_payload(&value)? {
                        Ok(value) => idle.push(value),
                        Err(error) => return Ok(value_err(error)),
                    }
                } else {
                    idle.push(value);
                }
            }
        }
        let id = self.next_pool_id;
        self.next_pool_id = self.next_pool_id.saturating_add(1);
        self.pools.insert(
            id,
            VmResourcePool {
                capacity,
                created: idle.len() as i64,
                in_use: 0,
                idle,
                factory: lazy.then_some(factory),
                factory_returns_result,
            },
        );
        let pool = resource_pool_value(id);
        if factory_returns_result && !lazy {
            Ok(value_ok(pool))
        } else {
            Ok(pool)
        }
    }

    fn resource_pool_borrow(
        &mut self,
        unit: &RegUnit,
        args: &[Reg],
        base: usize,
        next_base: usize,
        fallible: bool,
    ) -> Result<VmValue, EvalError> {
        let pool = expect_resource_pool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
        let borrowed = self.resource_pool_borrow_value(unit, pool.id, next_base);
        if fallible {
            return Ok(match borrowed {
                Ok(value) => value_ok(value),
                Err(error) => value_err(error),
            });
        }
        borrowed.map_err(|error| {
            EvalError::Runtime(format!(
                "ResourcePool.borrow failed: {}",
                pool_error_message(&error).unwrap_or_else(|| error.display())
            ))
        })
    }

    fn resource_pool_borrow_value(
        &mut self,
        unit: &RegUnit,
        pool_id: i64,
        next_base: usize,
    ) -> Result<VmValue, VmValue> {
        let idle = self
            .pools
            .get_mut(&pool_id)
            .ok_or_else(|| pool_error_value(format!("unknown ResourcePool id `{pool_id}`")))?
            .idle
            .pop();
        let value = if let Some(value) = idle {
            value
        } else {
            let factory = {
                let state = self.pools.get(&pool_id).ok_or_else(|| {
                    pool_error_value(format!("unknown ResourcePool id `{pool_id}`"))
                })?;
                if state.created >= state.capacity {
                    return Err(pool_error_value("resource pool exhausted"));
                }
                state
                    .factory
                    .clone()
                    .ok_or_else(|| pool_error_value("resource pool exhausted"))?
            };
            let value = self
                .call_closure_zero(unit, &factory, next_base)
                .map_err(|error| {
                    pool_error_value(format!("resource pool factory failed: {error:?}"))
                })?;
            let factory_returns_result = self
                .pools
                .get(&pool_id)
                .map(|state| state.factory_returns_result)
                .unwrap_or(false);
            let value = if factory_returns_result {
                match result_variant_payload(&value) {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => return Err(error),
                    Err(error) => {
                        return Err(pool_error_value(format!(
                            "resource pool factory returned non-Result value: {error:?}"
                        )));
                    }
                }
            } else {
                value
            };
            let state = self
                .pools
                .get_mut(&pool_id)
                .ok_or_else(|| pool_error_value(format!("unknown ResourcePool id `{pool_id}`")))?;
            state.created = state.created.saturating_add(1);
            value
        };
        let state = self
            .pools
            .get_mut(&pool_id)
            .ok_or_else(|| pool_error_value(format!("unknown ResourcePool id `{pool_id}`")))?;
        state.in_use = state.in_use.saturating_add(1);
        mark_pool_lease(value, pool_id).map_err(pool_error_value)
    }

    fn finish_resource_pool_lease(&mut self, value: VmValue) -> Result<bool, EvalError> {
        let Some(lease) = split_pool_lease(value)? else {
            return Ok(false);
        };
        let state = self.pools.get_mut(&lease.pool_id).ok_or_else(|| {
            EvalError::Runtime(format!("unknown ResourcePool id `{}`.", lease.pool_id))
        })?;
        state.in_use = state.in_use.saturating_sub(1);
        if lease.discarded {
            state.created = state.created.saturating_sub(1);
        } else {
            state.idle.push(lease.value);
        }
        Ok(true)
    }

    fn call_native_key(
        &mut self,
        key: &str,
        args: &[Reg],
        mut_args: &[usize],
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let Some(function) = self.native_bindings.get(key).copied() else {
            return Err(EvalError::Runtime(format!(
                "reg VM native function `{key}` has no host binding."
            )));
        };
        let arg_values = args
            .iter()
            .map(|reg| native_value_from_vm_value(self.reg(base + *reg).clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let raw = function(arg_values)
            .map_err(|error| EvalError::Runtime(format!("native host binding failed: {error}")))?;

        // No `mut` params: the binding returns its result directly.
        if mut_args.is_empty() {
            return Ok(vm_value_from_native_value(raw));
        }

        // With `mut` params the shim returns an envelope `List[result, mutated...]`
        // where the mutated values are in `mut_args` order. Write each mutated
        // value back to its arg register so the caller observes the mutation.
        let NativeValue::List(mut envelope) = raw else {
            return Err(EvalError::Runtime(format!(
                "native binding `{key}` was expected to return a mutation envelope."
            )));
        };
        if envelope.len() != mut_args.len() + 1 {
            return Err(EvalError::Runtime(format!(
                "native binding `{key}` returned {} envelope entries, expected {}.",
                envelope.len(),
                mut_args.len() + 1
            )));
        }
        let mutated: Vec<NativeValue> = envelope.split_off(1);
        let result = vm_value_from_native_value(envelope.pop().unwrap_or(NativeValue::Unit));
        for (position, value) in mut_args.iter().zip(mutated) {
            let reg = base + args[*position];
            self.set_reg(reg, vm_value_from_native_value(value));
        }
        Ok(result)
    }

    fn call_closure_from_regs(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        arg_regs: &[Reg],
        caller_base: usize,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        let offset = closure.captures.len();
        for (index, reg) in arg_regs.iter().enumerate() {
            let value = self.reg(caller_base + *reg).clone();
            self.set_reg(base + offset + index, value);
        }
        self.run_frame(unit, callee, base)
    }

    fn call_closure_one(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        arg: VmValue,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        self.set_reg(base + closure.captures.len(), arg);
        self.run_frame(unit, callee, base)
    }

    fn call_closure_two(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        first: VmValue,
        second: VmValue,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        let offset = closure.captures.len();
        self.set_reg(base + offset, first);
        self.set_reg(base + offset + 1, second);
        self.run_frame(unit, callee, base)
    }

    fn call_closure_three(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        first: VmValue,
        second: VmValue,
        third: VmValue,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        let offset = closure.captures.len();
        self.set_reg(base + offset, first);
        self.set_reg(base + offset + 1, second);
        self.set_reg(base + offset + 2, third);
        self.run_frame(unit, callee, base)
    }

    fn call_closure_zero(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs);
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        self.run_frame(unit, callee, base)
    }

    fn channel_state_mut(&mut self, id: i64) -> Result<&mut VmChannel, EvalError> {
        self.channels
            .get_mut(&id)
            .ok_or_else(|| EvalError::Runtime(format!("unknown channel id `{id}`.")))
    }

    /// Store a native tensor handle and return the opaque `VmValue::Native`
    /// carried through the program (mirrors `task_handle_value`).
    fn store_tensor(&mut self, tensor: rsscript_runtime::RssTensor) -> VmValue {
        let id = self.next_tensor_id;
        self.next_tensor_id = self.next_tensor_id.saturating_add(1);
        self.tensors.insert(id, tensor);
        VmValue::Native(Rc::new(VmNative {
            type_name: Rc::from("Tensor"),
            id,
        }))
    }

    /// Resolve a `Tensor` handle to the stored `RssTensor` (cloned — the buffer is
    /// `Rc`-shared, so this is a cheap pointer bump, not a data copy).
    fn expect_tensor_ref(
        &self,
        value: &VmValue,
    ) -> Result<rsscript_runtime::RssTensor, EvalError> {
        let id = match value {
            VmValue::Native(native) if native.type_name.as_ref() == "Tensor" => native.id,
            VmValue::Managed(inner) => return self.expect_tensor_ref(&inner.borrow()),
            other => {
                return Err(EvalError::Runtime(format!(
                    "reg VM expected Tensor, got `{}`.",
                    other.display()
                )));
            }
        };
        self.tensors
            .get(&id)
            .cloned()
            .ok_or_else(|| EvalError::Runtime(format!("unknown tensor id `{id}`.")))
    }

    fn channel_send(&mut self, sender: VmSender, value: VmValue) -> Result<VmValue, VmValue> {
        if sender.closed {
            return Err(channel_error_value("channel sender closed"));
        }
        let state = self.channels.get_mut(&sender.channel_id).ok_or_else(|| {
            channel_error_value(format!("unknown channel id `{}`", sender.channel_id))
        })?;
        if state.receiver_closed {
            return Err(channel_error_value("channel closed"));
        }
        if state.queue.len() >= state.capacity {
            return Err(channel_error_value(
                "channel send would block on a full channel in the VM",
            ));
        }
        state.queue.push_back(value);
        Ok(VmValue::Unit)
    }

    fn channel_recv(&mut self, channel_id: i64) -> Result<VmValue, VmValue> {
        let state = self
            .channels
            .get_mut(&channel_id)
            .ok_or_else(|| channel_error_value(format!("unknown channel id `{channel_id}`")))?;
        if let Some(value) = state.queue.pop_front() {
            return Ok(VmValue::OptionSome(Box::new(value)));
        }
        if state.senders == 0 {
            return Ok(VmValue::OptionNone);
        }
        Err(channel_error_value(
            "channel recv would block on an open empty channel in the VM",
        ))
    }

    fn filter_list(
        &mut self,
        unit: &RegUnit,
        list: Rc<RefCell<Vec<VmValue>>>,
        predicate: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let len = list.borrow().len();
        let mut filtered = Vec::with_capacity(len);
        for index in 0..len {
            let item = list_item_at(&list, index, "List.filter")?;
            let keep = self.call_closure_one(unit, predicate, item.clone(), base)?;
            if expect_bool_ref(&keep)? {
                filtered.push(item);
            }
        }
        Ok(VmValue::List(Rc::new(RefCell::new(filtered))))
    }

    fn fold_list(
        &mut self,
        unit: &RegUnit,
        list: Rc<RefCell<Vec<VmValue>>>,
        mut state: VmValue,
        folder: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        // Fast path: a fold whose folder is a recognized simple numeric binary
        // closure (`|acc, x| acc <op> x`) over a list of scalar `Int`/`Float`
        // values is the hot shape for sum/product-style reductions. Running it as
        // a tight loop over the element values — calling the *same*
        // `eval_numeric_binary` the interpreter uses, in the *same* operand
        // order, on the *same* values — avoids a full frame setup + bytecode
        // dispatch per element while producing bit-identical results (identical
        // f64 ops, order, NaN/inf, and error behavior). Any case that does not
        // exactly match (wrong shape, non-scalar element, captures present)
        // falls through to the generic interpreter path below.
        if let Some(form) = recognize_numeric_binary_closure(unit, folder) {
            if matches!(state, VmValue::Int(_) | VmValue::Float(_)) {
                let list = list.borrow();
                if list
                    .iter()
                    .all(|item| matches!(item, VmValue::Int(_) | VmValue::Float(_)))
                {
                    for item in list.iter() {
                        // Preserve the closure's operand order exactly: `state`
                        // and `item` are placed at the two param registers, so
                        // whichever param the lhs/rhs reads determines the order.
                        let (lhs, rhs) = if form.lhs_is_state {
                            (&state, item)
                        } else {
                            (item, &state)
                        };
                        state = eval_numeric_binary(form.op, lhs, rhs)?;
                    }
                    return Ok(state);
                }
            }
        }
        let len = list.borrow().len();
        for index in 0..len {
            let item = list_item_at(&list, index, "List.fold")?;
            state = self.call_closure_two(unit, folder, state, item, base)?;
        }
        Ok(state)
    }

    fn map_list(
        &mut self,
        unit: &RegUnit,
        list: Rc<RefCell<Vec<VmValue>>>,
        mapper: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let len = list.borrow().len();
        let mut mapped = Vec::with_capacity(len);
        for index in 0..len {
            let item = list_item_at(&list, index, "List.map")?;
            mapped.push(self.call_closure_one(unit, mapper, item, base)?);
        }
        Ok(VmValue::List(Rc::new(RefCell::new(mapped))))
    }

    fn sort_list_with_closure(
        &mut self,
        unit: &RegUnit,
        list: &mut [VmValue],
        compare: &VmClosure,
        base: usize,
    ) -> Result<(), EvalError> {
        for right_index in 1..list.len() {
            let mut index = right_index;
            while index > 0 {
                let ordering = self.call_closure_two(
                    unit,
                    compare,
                    list[index - 1].clone(),
                    list[index].clone(),
                    base,
                )?;
                if expect_int_ref(&ordering)? <= 0 {
                    break;
                }
                list.swap(index - 1, index);
                index -= 1;
            }
        }
        Ok(())
    }

    fn sort_list_by_closure(
        &mut self,
        unit: &RegUnit,
        mut list: Vec<VmValue>,
        key: &VmClosure,
        compare: &VmClosure,
        base: usize,
    ) -> Result<Vec<VmValue>, EvalError> {
        for right_index in 1..list.len() {
            let mut index = right_index;
            while index > 0 {
                let left_key = self.call_closure_one(unit, key, list[index - 1].clone(), base)?;
                let right_key = self.call_closure_one(unit, key, list[index].clone(), base)?;
                let ordering = self.call_closure_two(unit, compare, left_key, right_key, base)?;
                if expect_int_ref(&ordering)? <= 0 {
                    break;
                }
                list.swap(index - 1, index);
                index -= 1;
            }
        }
        Ok(list)
    }

    fn call_typed_intrinsic(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        type_arg: &str,
        args: &[Reg],
        base: usize,
    ) -> Result<VmValue, EvalError> {
        match intrinsic {
            RegIntrinsic::JsonDecode => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_decode_struct_value(unit, type_arg, value)))
            }
            RegIntrinsic::JsonDecodeText => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(parse_json_text(text).and_then(|value| {
                    json_decode_struct_value(unit, type_arg, &value)
                })))
            }
            other => Err(EvalError::Runtime(format!(
                "reg VM typed intrinsic `{other:?}` is not implemented."
            ))),
        }
    }

    // See `try_exec_pure`: interior-mutable `VmMapKey` is safe because
    // `retains(key)` forbids mutating a key while it is in a map.
    #[allow(clippy::mutable_key_type)]
    fn call_intrinsic(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        match intrinsic {
            RegIntrinsic::ArgsAll => Ok(VmValue::List(Rc::new(RefCell::new(
                self.args.iter().cloned().map(VmValue::string).collect(),
            )))),
            RegIntrinsic::ArgsCount => Ok(VmValue::Int(self.args.len() as i64)),
            RegIntrinsic::ArgsGet => {
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(usize::try_from(index)
                    .ok()
                    .and_then(|index| self.args.get(index).cloned())
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::string(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ArgsGetOrDefault => {
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let default =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(VmValue::string(
                    usize::try_from(index)
                        .ok()
                        .and_then(|index| self.args.get(index).cloned())
                        .unwrap_or(default),
                ))
            }
            RegIntrinsic::AssertEqual => {
                let left = intrinsic_arg(&self.stack, base, args, 0)?;
                let right = intrinsic_arg(&self.stack, base, args, 1)?;
                if left == right {
                    Ok(VmValue::Unit)
                } else {
                    Err(EvalError::Runtime(format!(
                        "assertion failed: left `{}` does not equal right `{}`.",
                        left.display(),
                        right.display()
                    )))
                }
            }
            RegIntrinsic::AssertEqualBool => {
                let left = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                if left == right {
                    Ok(VmValue::Unit)
                } else {
                    Err(EvalError::Runtime(format!(
                        "assertion failed: left `{left}` does not equal right `{right}`."
                    )))
                }
            }
            RegIntrinsic::AssertEqualInt => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                if left == right {
                    Ok(VmValue::Unit)
                } else {
                    Err(EvalError::Runtime(format!(
                        "assertion failed: left `{left}` does not equal right `{right}`."
                    )))
                }
            }
            RegIntrinsic::Base64Decode => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    base64::engine::general_purpose::STANDARD
                        .decode(text)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| decode_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::Base64DecodeString => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = base64::engine::general_purpose::STANDARD
                    .decode(text)
                    .map_err(|error| decode_error_value(error.to_string()))
                    .and_then(|bytes| {
                        String::from_utf8(bytes)
                            .map(VmValue::string)
                            .map_err(|error| decode_error_value(error.to_string()))
                    });
                Ok(json_result(result))
            }
            RegIntrinsic::Base64Encode => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    base64::engine::general_purpose::STANDARD.encode(value.as_bytes()),
                ))
            }
            RegIntrinsic::Base64EncodeBytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    base64::engine::general_purpose::STANDARD.encode(value),
                ))
            }
            RegIntrinsic::BytesConcat | RegIntrinsic::BytesConsume | RegIntrinsic::BytesFromString | RegIntrinsic::BytesFromUints | RegIntrinsic::BytesIsEmpty | RegIntrinsic::BytesLen | RegIntrinsic::BytesSlice | RegIntrinsic::BytesToString | RegIntrinsic::BytesToUints | RegIntrinsic::BytesViewStartsWith | RegIntrinsic::BytesViewToBytes => self.exec_bytes_intrinsics(unit, intrinsic, args, base, next_base),
            RegIntrinsic::BufferNew => {
                let size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(Vec::with_capacity(
                    size.max(0) as usize
                ))))
            }
            RegIntrinsic::CacheGet => {
                let cache = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bytes = cache
                    .borrow()
                    .values()
                    .find_map(|value| expect_string_ref(value).ok().map(str::to_owned))
                    .map(String::into_bytes)
                    .unwrap_or_default();
                Ok(image_value(
                    bytes,
                    None,
                    None,
                    vec!["cache-get".to_string()],
                ))
            }
            RegIntrinsic::CacheLookup => {
                let cache = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(cache
                    .borrow()
                    .get(&key)
                    .cloned()
                    .unwrap_or_else(|| VmValue::string("")))
            }
            RegIntrinsic::CapabilityFrom => Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone()),
            RegIntrinsic::CancellationSourceCancel => {
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 0)?,
                    "CancellationSource",
                )?;
                self.cancellation_flags.insert(id, true);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::CancellationSourceNew => {
                let id = self.next_cancellation_id;
                self.next_cancellation_id = self.next_cancellation_id.saturating_add(1);
                self.cancellation_flags.insert(id, false);
                Ok(cancellation_source_value(id))
            }
            RegIntrinsic::CancellationSourceToken => {
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 0)?,
                    "CancellationSource",
                )?;
                Ok(cancellation_token_value(id))
            }
            RegIntrinsic::CancellationTokenIsCancelled => {
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 0)?,
                    "CancellationToken",
                )?;
                Ok(VmValue::Bool(
                    self.cancellation_flags.get(&id).copied().unwrap_or(false),
                ))
            }
            RegIntrinsic::ChannelBounded => {
                let capacity = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(if capacity <= 0 {
                    Err(channel_error_value("channel capacity must be positive"))
                } else {
                    let id = self.next_channel_id;
                    self.next_channel_id = self.next_channel_id.saturating_add(1);
                    self.channels.insert(id, VmChannel::new(capacity as usize));
                    Ok(channel_value(id, capacity, false))
                }))
            }
            RegIntrinsic::ChannelSender => {
                let channel = expect_channel_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let state = self.channel_state_mut(channel.id)?;
                state.senders = state.senders.saturating_add(1);
                Ok(sender_value(channel.id, false))
            }
            RegIntrinsic::ChannelReceiver => {
                let channel_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Channel.receiver missing channel.".to_string())
                })?;
                let mut channel = expect_channel_ref(self.reg(base + channel_reg))?;
                let already_taken = self
                    .channels
                    .get(&channel.id)
                    .map(|state| state.receiver_taken)
                    .unwrap_or(channel.receiver_taken);
                Ok(json_result(if already_taken {
                    Err(channel_error_value("channel receiver already taken"))
                } else {
                    channel.receiver_taken = true;
                    self.channel_state_mut(channel.id)?.receiver_taken = true;
                    self.set_reg(base + channel_reg, channel.to_value());
                    Ok(receiver_value(channel.id, false))
                }))
            }
            RegIntrinsic::ChannelErrorMessage
            | RegIntrinsic::DecodeErrorMessage
            | RegIntrinsic::FileErrorMessage
            | RegIntrinsic::HttpErrorMessage
            | RegIntrinsic::PoolErrorMessage
            | RegIntrinsic::TcpErrorMessage
            | RegIntrinsic::TensorErrorMessage
            | RegIntrinsic::WebSocketErrorMessage => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "message")
            }
            RegIntrinsic::TensorFromF32Slice => {
                let data = expect_float_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let shape = expect_int_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    match rsscript_runtime::tensor_from_f32_slice(&data, &shape) {
                        Ok(tensor) => Ok(self.store_tensor(tensor)),
                        Err(error) => Err(tensor_error_value(
                            rsscript_runtime::tensor_error_message(&error),
                        )),
                    },
                ))
            }
            RegIntrinsic::TensorToF32Slice => {
                let tensor = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    match rsscript_runtime::tensor_to_f32_slice(&tensor) {
                        Ok(values) => Ok(VmValue::List(Rc::new(RefCell::new(
                            values.into_iter().map(VmValue::Float).collect(),
                        )))),
                        Err(error) => Err(tensor_error_value(
                            rsscript_runtime::tensor_error_message(&error),
                        )),
                    },
                ))
            }
            RegIntrinsic::TensorShape => {
                let tensor = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(match rsscript_runtime::tensor_shape(&tensor) {
                    Ok(dims) => Ok(VmValue::List(Rc::new(RefCell::new(
                        dims.into_iter().map(VmValue::Int).collect(),
                    )))),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorRank => {
                let tensor = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(rsscript_runtime::tensor_rank(&tensor)))
            }
            RegIntrinsic::TensorMatmul => {
                let a = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let b = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(match rsscript_runtime::tensor_matmul(&a, &b) {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorAdd
            | RegIntrinsic::TensorSub
            | RegIntrinsic::TensorMul
            | RegIntrinsic::TensorDiv => {
                let a = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let b = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorAdd => rsscript_runtime::tensor_add(&a, &b),
                    RegIntrinsic::TensorSub => rsscript_runtime::tensor_sub(&a, &b),
                    RegIntrinsic::TensorMul => rsscript_runtime::tensor_mul(&a, &b),
                    _ => rsscript_runtime::tensor_div(&a, &b),
                };
                Ok(json_result(match result {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorNeg
            | RegIntrinsic::TensorExp
            | RegIntrinsic::TensorLog
            | RegIntrinsic::TensorSqrt
            | RegIntrinsic::TensorRelu => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorNeg => rsscript_runtime::tensor_neg(&t),
                    RegIntrinsic::TensorExp => rsscript_runtime::tensor_exp(&t),
                    RegIntrinsic::TensorLog => rsscript_runtime::tensor_log(&t),
                    RegIntrinsic::TensorSqrt => rsscript_runtime::tensor_sqrt(&t),
                    _ => rsscript_runtime::tensor_relu(&t),
                };
                Ok(self.store_tensor(result))
            }
            RegIntrinsic::TensorSumAll => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Float(rsscript_runtime::tensor_sum_all(&t)))
            }
            RegIntrinsic::TensorSumAxis
            | RegIntrinsic::TensorMaxAxis
            | RegIntrinsic::TensorMeanAxis
            | RegIntrinsic::TensorArgmaxAxis => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let axis = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorSumAxis => rsscript_runtime::tensor_sum_axis(&t, axis),
                    RegIntrinsic::TensorMaxAxis => rsscript_runtime::tensor_max_axis(&t, axis),
                    RegIntrinsic::TensorMeanAxis => rsscript_runtime::tensor_mean_axis(&t, axis),
                    _ => rsscript_runtime::tensor_argmax_axis(&t, axis),
                };
                Ok(json_result(match result {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorReshape => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let shape = expect_int_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    match rsscript_runtime::tensor_reshape(&t, &shape) {
                        Ok(tensor) => Ok(self.store_tensor(tensor)),
                        Err(error) => Err(tensor_error_value(
                            rsscript_runtime::tensor_error_message(&error),
                        )),
                    },
                ))
            }
            RegIntrinsic::TensorTranspose => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(match rsscript_runtime::tensor_transpose(&t) {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorPermute => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let axes = expect_int_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    match rsscript_runtime::tensor_permute(&t, &axes) {
                        Ok(tensor) => Ok(self.store_tensor(tensor)),
                        Err(error) => Err(tensor_error_value(
                            rsscript_runtime::tensor_error_message(&error),
                        )),
                    },
                ))
            }
            RegIntrinsic::TensorBroadcastTo => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let shape = expect_int_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    match rsscript_runtime::tensor_broadcast_to(&t, &shape) {
                        Ok(tensor) => Ok(self.store_tensor(tensor)),
                        Err(error) => Err(tensor_error_value(
                            rsscript_runtime::tensor_error_message(&error),
                        )),
                    },
                ))
            }
            RegIntrinsic::TensorCmplt
            | RegIntrinsic::TensorCmpne
            | RegIntrinsic::TensorCmpeq
            | RegIntrinsic::TensorMaximum
            | RegIntrinsic::TensorMinimum => {
                let a = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let b = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorCmplt => rsscript_runtime::tensor_cmplt(&a, &b),
                    RegIntrinsic::TensorCmpne => rsscript_runtime::tensor_cmpne(&a, &b),
                    RegIntrinsic::TensorCmpeq => rsscript_runtime::tensor_cmpeq(&a, &b),
                    RegIntrinsic::TensorMaximum => rsscript_runtime::tensor_maximum(&a, &b),
                    _ => rsscript_runtime::tensor_minimum(&a, &b),
                };
                Ok(json_result(match result {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorSelect => {
                let cond = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let a = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let b = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_result(
                    match rsscript_runtime::tensor_select(&cond, &a, &b) {
                        Ok(tensor) => Ok(self.store_tensor(tensor)),
                        Err(error) => Err(tensor_error_value(
                            rsscript_runtime::tensor_error_message(&error),
                        )),
                    },
                ))
            }
            RegIntrinsic::TensorCastF32
            | RegIntrinsic::TensorCastI32
            | RegIntrinsic::TensorCastBool => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorCastF32 => rsscript_runtime::tensor_cast_f32(&t),
                    RegIntrinsic::TensorCastI32 => rsscript_runtime::tensor_cast_i32(&t),
                    _ => rsscript_runtime::tensor_cast_bool(&t),
                };
                Ok(self.store_tensor(result))
            }
            RegIntrinsic::TensorDtypeCode => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(rsscript_runtime::tensor_dtype_code(&t)))
            }
            // movement+gather (ops B)
            RegIntrinsic::TensorPad
            | RegIntrinsic::TensorShrink
            | RegIntrinsic::TensorFlip => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let amounts = expect_int_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorPad => rsscript_runtime::tensor_pad(&t, &amounts),
                    RegIntrinsic::TensorShrink => rsscript_runtime::tensor_shrink(&t, &amounts),
                    _ => rsscript_runtime::tensor_flip(&t, &amounts),
                };
                Ok(json_result(match result {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorGather => {
                let data = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let axis = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let indices = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_result(
                    match rsscript_runtime::tensor_gather(&data, axis, &indices) {
                        Ok(tensor) => Ok(self.store_tensor(tensor)),
                        Err(error) => Err(tensor_error_value(
                            rsscript_runtime::tensor_error_message(&error),
                        )),
                    },
                ))
            }
            // reductions+math (ops C)
            RegIntrinsic::TensorProdAxis | RegIntrinsic::TensorMinAxis => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let axis = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorProdAxis => rsscript_runtime::tensor_prod_axis(&t, axis),
                    _ => rsscript_runtime::tensor_min_axis(&t, axis),
                };
                Ok(json_result(match result {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorSumAxes
            | RegIntrinsic::TensorProdAxes
            | RegIntrinsic::TensorMaxAxes
            | RegIntrinsic::TensorMinAxes
            | RegIntrinsic::TensorMeanAxes => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let axes = expect_int_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorSumAxes => rsscript_runtime::tensor_sum_axes(&t, &axes),
                    RegIntrinsic::TensorProdAxes => rsscript_runtime::tensor_prod_axes(&t, &axes),
                    RegIntrinsic::TensorMaxAxes => rsscript_runtime::tensor_max_axes(&t, &axes),
                    RegIntrinsic::TensorMinAxes => rsscript_runtime::tensor_min_axes(&t, &axes),
                    _ => rsscript_runtime::tensor_mean_axes(&t, &axes),
                };
                Ok(json_result(match result {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorReciprocal
            | RegIntrinsic::TensorExp2
            | RegIntrinsic::TensorLog2
            | RegIntrinsic::TensorRsqrt => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorReciprocal => rsscript_runtime::tensor_reciprocal(&t),
                    RegIntrinsic::TensorExp2 => rsscript_runtime::tensor_exp2(&t),
                    RegIntrinsic::TensorLog2 => rsscript_runtime::tensor_log2(&t),
                    _ => rsscript_runtime::tensor_rsqrt(&t),
                };
                Ok(self.store_tensor(result))
            }
            RegIntrinsic::TensorPow => {
                let a = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let b = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(match rsscript_runtime::tensor_pow(&a, &b) {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            // bmm+int/bit (ops D)
            RegIntrinsic::TensorBmm
            | RegIntrinsic::TensorIdiv
            | RegIntrinsic::TensorMod
            | RegIntrinsic::TensorShl
            | RegIntrinsic::TensorShr
            | RegIntrinsic::TensorAnd
            | RegIntrinsic::TensorOr
            | RegIntrinsic::TensorXor => {
                let a = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let b = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorBmm => rsscript_runtime::tensor_bmm(&a, &b),
                    RegIntrinsic::TensorIdiv => rsscript_runtime::tensor_idiv(&a, &b),
                    RegIntrinsic::TensorMod => rsscript_runtime::tensor_mod(&a, &b),
                    RegIntrinsic::TensorShl => rsscript_runtime::tensor_shl(&a, &b),
                    RegIntrinsic::TensorShr => rsscript_runtime::tensor_shr(&a, &b),
                    RegIntrinsic::TensorAnd => rsscript_runtime::tensor_and(&a, &b),
                    RegIntrinsic::TensorOr => rsscript_runtime::tensor_or(&a, &b),
                    _ => rsscript_runtime::tensor_xor(&a, &b),
                };
                Ok(json_result(match result {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorBitcastF32ToI32 | RegIntrinsic::TensorBitcastI32ToF32 => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorBitcastF32ToI32 => {
                        rsscript_runtime::tensor_bitcast_f32_to_i32(&t)
                    }
                    _ => rsscript_runtime::tensor_bitcast_i32_to_f32(&t),
                };
                Ok(self.store_tensor(result))
            }
            // nn (slice F)
            RegIntrinsic::TensorIota => {
                let n = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(match rsscript_runtime::tensor_iota(n) {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorOneHot => {
                let indices = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let num_classes = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    match rsscript_runtime::tensor_one_hot(&indices, num_classes) {
                        Ok(tensor) => Ok(self.store_tensor(tensor)),
                        Err(error) => Err(tensor_error_value(
                            rsscript_runtime::tensor_error_message(&error),
                        )),
                    },
                ))
            }
            RegIntrinsic::TensorSoftmax | RegIntrinsic::TensorLogSoftmax => {
                let t = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let axis = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = match intrinsic {
                    RegIntrinsic::TensorSoftmax => rsscript_runtime::tensor_softmax(&t, axis),
                    _ => rsscript_runtime::tensor_log_softmax(&t, axis),
                };
                Ok(json_result(match result {
                    Ok(tensor) => Ok(self.store_tensor(tensor)),
                    Err(error) => Err(tensor_error_value(
                        rsscript_runtime::tensor_error_message(&error),
                    )),
                }))
            }
            RegIntrinsic::TensorCrossEntropy => {
                let logits = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let targets = self.expect_tensor_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let axis = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_result(
                    match rsscript_runtime::tensor_cross_entropy(&logits, &targets, axis) {
                        Ok(tensor) => Ok(self.store_tensor(tensor)),
                        Err(error) => Err(tensor_error_value(
                            rsscript_runtime::tensor_error_message(&error),
                        )),
                    },
                ))
            }
            RegIntrinsic::CharCompare | RegIntrinsic::CharFromCode | RegIntrinsic::CharIsAlphanumeric | RegIntrinsic::CharIsAlpha | RegIntrinsic::CharIsDigit | RegIntrinsic::CharIsLower | RegIntrinsic::CharIsUpper | RegIntrinsic::CharIsWhitespace | RegIntrinsic::CharToCode | RegIntrinsic::CharToLower | RegIntrinsic::CharToString | RegIntrinsic::CharToUpper => self.exec_char_intrinsics(unit, intrinsic, args, base, next_base),
            RegIntrinsic::ClockNow => Ok(instant_value(clock_system_unix_ms())),
            RegIntrinsic::ClockSystemUnixMs => Ok(VmValue::Int(clock_system_unix_ms())),
            RegIntrinsic::ConfigLoad => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(|text| config_value(config_name_from_text(&text)))
                        .map_err(|error| config_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::ConfigName => {
                let value = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::string(expect_config_value_name(value)?))
            }
            RegIntrinsic::ConfigNew => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let rules = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(config_rules_value(name, rules.borrow().len() as i64))
            }
            RegIntrinsic::ConfigRuleCount => {
                let config = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Int(expect_config_rule_count(config)?))
            }
            RegIntrinsic::ConfigStoreName => {
                let store = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::string(expect_config_store_name(store)?))
            }
            RegIntrinsic::ConfigStoreNew => {
                let value = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(config_store_value(expect_config_value_name(value)?))
            }
            RegIntrinsic::DateAddDays | RegIntrinsic::DateAddMs | RegIntrinsic::DateDay | RegIntrinsic::DateDaysBetween | RegIntrinsic::DateDaysInMonth | RegIntrinsic::DateFormatIso | RegIntrinsic::DateFormatYmd | RegIntrinsic::DateHour | RegIntrinsic::DateIsLeapYear | RegIntrinsic::DateMinute | RegIntrinsic::DateMonth | RegIntrinsic::DateParseIso | RegIntrinsic::DateParseYmd | RegIntrinsic::DateSecond | RegIntrinsic::DateStartOfDay | RegIntrinsic::DateWeekday | RegIntrinsic::DateYear => self.exec_date_intrinsics(unit, intrinsic, args, base, next_base),
            RegIntrinsic::CounterNew => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(counter_value(value))
            }
            RegIntrinsic::CounterValue => {
                let counter = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Int(expect_counter_value(counter)?))
            }
            RegIntrinsic::CsvOpenRead => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::File::open(path)
                        .map(|_| file_value(path, "read", 0))
                        .map_err(|error| csv_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::CsvParseRow => {
                let buffer = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(json_result(csv_parse_row_value(
                    &expect_row_buffer_bytes_ref(buffer)?,
                )))
            }
            RegIntrinsic::CsvReadInto => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Csv.read_into missing file.".to_string())
                })?;
                let buffer_reg = *args.get(1).ok_or_else(|| {
                    EvalError::Runtime("reg VM Csv.read_into missing buffer.".to_string())
                })?;
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_read_remaining(&mut file)
                    .map_err(|error| csv_error_value(error.to_string()));
                self.set_reg(base + file_reg, file.to_value());
                Ok(match result {
                    Ok(bytes) => {
                        self.set_reg(base + buffer_reg, row_buffer_value(bytes));
                        value_ok(VmValue::Unit)
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::CsvRows => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _buffer_size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    csv_rows_stream_value(path).map_err(csv_error_value),
                ))
            }
            RegIntrinsic::DeadlineAfter | RegIntrinsic::DeadlineAfterMs => {
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(deadline_value(deadline_after_ms(ms)))
            }
            RegIntrinsic::DeadlineIsExpired => {
                let deadline = expect_deadline_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(clock_system_unix_ms() >= deadline))
            }
            RegIntrinsic::DeadlineRemainingMs => {
                let deadline = expect_deadline_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(
                    deadline.saturating_sub(clock_system_unix_ms()).max(0),
                ))
            }
            RegIntrinsic::DequeIsEmpty
            | RegIntrinsic::DequeLen
            | RegIntrinsic::DequeNew
            | RegIntrinsic::DequeToList => {
                self.exec_deque_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::DiffUnified => {
                let old = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let new = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(diff_unified_string(old, new)))
            }
            RegIntrinsic::DirectoryCopyFile => {
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::copy(from, to).map(|_| ())))
            }
            RegIntrinsic::DirectoryCreate => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::create_dir(path)))
            }
            RegIntrinsic::DirectoryCreateAll | RegIntrinsic::DirectoryCreateDirAll => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::create_dir_all(path)))
            }
            RegIntrinsic::DirectoryExists => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).exists()))
            }
            RegIntrinsic::DirectoryIsDir => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_dir()))
            }
            RegIntrinsic::DirectoryIsFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_file()))
            }
            RegIntrinsic::DirectoryListFiles => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    directory_list_files(Path::new(path))
                        .map(|files| {
                            VmValue::List(Rc::new(RefCell::new(
                                files.into_iter().map(VmValue::string).collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::DirectoryListPaths => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    directory_list_paths(Path::new(path))
                        .map(|paths| {
                            VmValue::List(Rc::new(RefCell::new(
                                paths
                                    .into_iter()
                                    .map(|path| VmValue::string(path.to_string_lossy()))
                                    .collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::DirectoryMetadata => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::metadata(path)
                        .map(file_metadata_value)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::DirectoryReadString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(VmValue::string)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::DirectoryRemoveDirAll => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::remove_dir_all(path)))
            }
            RegIntrinsic::DirectoryRemoveFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::remove_file(path)))
            }
            RegIntrinsic::DirectoryRename => {
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::rename(from, to)))
            }
            RegIntrinsic::DirectoryWriteString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let content = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::write(path, content)))
            }
            RegIntrinsic::DbClose => {
                let _ = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::DbConnectionOpen => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(db_connection_value(url, Vec::new()))
            }
            RegIntrinsic::DbConnectionQuery => {
                let conn_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM DbConnection.query missing conn.".to_string())
                })?;
                let sql =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                let mut conn = expect_db_connection_ref(self.reg(base + conn_reg))?;
                Ok(json_result(if sql.trim().is_empty() {
                    Err(db_error_value("SQL query is empty"))
                } else {
                    self.push_stdout(&format!("db query on {}: {sql}\n", conn.url));
                    conn.queries.push(sql);
                    self.set_reg(base + conn_reg, conn.to_value());
                    Ok(VmValue::Unit)
                }))
            }
            RegIntrinsic::DbConnectionTryOpen => {
                let url =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                Ok(json_result(if url.trim().is_empty() {
                    Err(db_error_value("database URL is empty"))
                } else {
                    Ok(db_connection_value(url, Vec::new()))
                }))
            }
            RegIntrinsic::DurationAdd => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left + right))
            }
            RegIntrinsic::DurationAsMs | RegIntrinsic::DurationMs => Ok(VmValue::Int(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?,
            )),
            RegIntrinsic::DurationAsSeconds => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value / 1000))
            }
            RegIntrinsic::DurationSeconds => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value * 1000))
            }
            RegIntrinsic::EnvironmentBindFunction => {
                let env_reg = args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Environment.bind_function missing env.".to_string())
                })?;
                let function = intrinsic_arg(&self.stack, base, args, 1)?;
                let _ = expect_function_has_closure(function)?;
                let env = intrinsic_arg(&self.stack, base, args, 0)?;
                let (has_parent, _) = expect_environment_state(env)?;
                self.set_reg(base + *env_reg, environment_value(has_parent, true));
                Ok(VmValue::Unit)
            }
            RegIntrinsic::EnvironmentChild => {
                let parent = intrinsic_arg(&self.stack, base, args, 0)?;
                let _ = expect_environment_state(parent)?;
                Ok(environment_value(true, false))
            }
            RegIntrinsic::EnvironmentHasFunction => {
                let env = intrinsic_arg(&self.stack, base, args, 0)?;
                let (_, has_function) = expect_environment_state(env)?;
                Ok(VmValue::Bool(has_function))
            }
            RegIntrinsic::EnvironmentHasParent => {
                let env = intrinsic_arg(&self.stack, base, args, 0)?;
                let (has_parent, _) = expect_environment_state(env)?;
                Ok(VmValue::Bool(has_parent))
            }
            RegIntrinsic::EnvironmentRoot => Ok(environment_value(false, false)),
            RegIntrinsic::EnvCurrentDir => Ok(json_result(
                std::env::current_dir()
                    .map(|path| VmValue::string(path.to_string_lossy()))
                    .map_err(|error| file_error_value(error.to_string())),
            )),
            RegIntrinsic::EnvGet => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(std::env::var(name)
                    .ok()
                    .map(VmValue::string)
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::EnvGetOrDefault => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let default = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(
                    std::env::var(name).unwrap_or_else(|_| default.to_string()),
                ))
            }
            RegIntrinsic::EnvHomeDir => Ok(std::env::var("HOME")
                .ok()
                .filter(|value| !value.is_empty())
                .map(VmValue::string)
                .map(|value| VmValue::OptionSome(Box::new(value)))
                .unwrap_or(VmValue::OptionNone)),
            RegIntrinsic::EnvRunWorkspaceRoot => Ok(VmValue::string(
                std::env::var("RSS_RUN_WORKSPACE_ROOT")
                    .ok()
                    .or_else(|| {
                        std::env::current_dir()
                            .ok()
                            .map(|path| path.display().to_string())
                    })
                    .unwrap_or_else(|| ".".to_string()),
            )),
            RegIntrinsic::EnvSet => {
                let _ = intrinsic_arg(&self.stack, base, args, 0)?;
                let _ = intrinsic_arg(&self.stack, base, args, 1)?;
                self.stderr
                    .push_str("[rsscript] warning: Env.set is a no-op in the safe runtime\n");
                Ok(VmValue::Unit)
            }
            RegIntrinsic::EnvSetCurrentDir => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::env::set_current_dir(path)))
            }
            RegIntrinsic::EnvTempDir => Ok(VmValue::string(std::env::temp_dir().to_string_lossy())),
            RegIntrinsic::FileAppendBytes => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(file_result_unit(file_append(path, &data)))
            }
            RegIntrinsic::FileAppendString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?
                    .as_bytes()
                    .to_vec();
                Ok(file_result_unit(file_append(path, &text)))
            }
            RegIntrinsic::FileBytesStream => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let chunk_size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    file_bytes_stream_value(path, chunk_size).map_err(channel_error_value),
                ))
            }
            RegIntrinsic::FileExists => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).exists()))
            }
            RegIntrinsic::FileOpen | RegIntrinsic::FileOpenRead => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::File::open(path)
                        .map(|_| file_value(path, "read", 0))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileOpenWrite => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::File::create(path)
                        .map(|_| file_value(path, "write", 0))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileReadAllAsync => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read(path)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileReadAll => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.read_all missing file.".to_string())
                })?;
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_read_remaining(&mut file)
                    .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                    .map_err(|error| file_error_value(error.to_string()));
                self.set_reg(base + file_reg, file.to_value());
                Ok(json_result(result))
            }
            RegIntrinsic::FileReadAllString => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.read_all_string missing file.".to_string())
                })?;
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_read_remaining(&mut file)
                    .and_then(|bytes| {
                        String::from_utf8(bytes).map_err(|error| {
                            std::io::Error::new(std::io::ErrorKind::InvalidData, error)
                        })
                    })
                    .map(VmValue::string)
                    .map_err(|error| file_error_value(error.to_string()));
                self.set_reg(base + file_reg, file.to_value());
                Ok(json_result(result))
            }
            RegIntrinsic::FileReadAllStringAsync => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(VmValue::string)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileReadBytes => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read(path)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileReadInto => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.read_into missing file.".to_string())
                })?;
                let buffer_reg = *args.get(1).ok_or_else(|| {
                    EvalError::Runtime("reg VM File.read_into missing buffer.".to_string())
                })?;
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_read_remaining(&mut file)
                    .map(|bytes| {
                        let did_read = !bytes.is_empty();
                        (VmValue::Bytes(Rc::new(bytes)), VmValue::Bool(did_read))
                    })
                    .map_err(|error| file_error_value(error.to_string()));
                self.set_reg(base + file_reg, file.to_value());
                Ok(match result {
                    Ok((buffer, did_read)) => {
                        self.set_reg(base + buffer_reg, buffer);
                        value_ok(did_read)
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FileReadString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(VmValue::string)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::FileRemove => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(file_result_unit(std::fs::remove_file(path)))
            }
            RegIntrinsic::FileWrite => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.write missing file.".to_string())
                })?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_write_at_cursor(&mut file, &data);
                self.set_reg(base + file_reg, file.to_value());
                Ok(file_result_unit(result))
            }
            RegIntrinsic::FileWriteAsync | RegIntrinsic::FileWriteBytes => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(file_result_unit(std::fs::write(path, data)))
            }
            RegIntrinsic::FileWriteAtomic => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_atomic_write_result(PathBuf::from(path), text))
            }
            RegIntrinsic::FileWriteBytesView
            | RegIntrinsic::FileWriteBuffer
            | RegIntrinsic::FileWriteBufferView => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM file write missing file.".to_string())
                })?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_write_at_cursor(&mut file, &data);
                self.set_reg(base + file_reg, file.to_value());
                Ok(file_result_unit(result))
            }
            RegIntrinsic::FileWriteString => {
                let file_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM File.write_string missing file.".to_string())
                })?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?
                    .as_bytes()
                    .to_vec();
                let mut file = expect_file_ref(self.reg(base + file_reg))?;
                let result = file_write_at_cursor(&mut file, &text);
                self.set_reg(base + file_reg, file.to_value());
                Ok(file_result_unit(result))
            }
            RegIntrinsic::FileWriteStringAsync => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::write(path, text)))
            }
            RegIntrinsic::FileWriteStringToPath => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::write(path, text)))
            }
            RegIntrinsic::FalliblePipelineCollect => {
                Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone())
            }
            RegIntrinsic::FalliblePipelineMap => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let len = items.borrow().len();
                        let mut mapped = Vec::with_capacity(len);
                        for index in 0..len {
                            let value = items.borrow()[index].clone();
                            mapped.push(self.call_closure_one(unit, &mapper, value, next_base)?);
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(mapped))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FalliblePipelineFilter => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let len = items.borrow().len();
                        let mut filtered = Vec::new();
                        for index in 0..len {
                            let value = items.borrow()[index].clone();
                            let keep =
                                self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                            if expect_bool_ref(&keep)? {
                                filtered.push(value);
                            }
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(filtered))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FalliblePipelineEach => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let action = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let values = items.borrow().clone();
                        for value in values.iter().cloned() {
                            let _ = self.call_closure_one(unit, &action, value, next_base)?;
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(values))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FalliblePipelineTryMap => {
                let pipeline = intrinsic_arg(&self.stack, base, args, 0)?.clone();
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(match result_variant_payload(&pipeline)? {
                    Ok(items) => {
                        let items = expect_list_ref(&items)?;
                        let len = items.borrow().len();
                        let mut mapped = Vec::with_capacity(len);
                        for index in 0..len {
                            let value = items.borrow()[index].clone();
                            match result_variant_payload(
                                &self.call_closure_one(unit, &mapper, value, next_base)?,
                            )? {
                                Ok(value) => mapped.push(value),
                                Err(error) => return Ok(value_err(error)),
                            }
                        }
                        value_ok(VmValue::List(Rc::new(RefCell::new(mapped))))
                    }
                    Err(error) => value_err(error),
                })
            }
            RegIntrinsic::FunctionObjectHasClosure => {
                let function = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Bool(expect_function_has_closure(function)?))
            }
            RegIntrinsic::FunctionObjectNew => {
                let closure = intrinsic_arg(&self.stack, base, args, 0)?;
                let _ = expect_environment_state(closure)?;
                Ok(function_object_value(true))
            }
            RegIntrinsic::HashSha256Bytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(sha256_digest(value)))
            }
            RegIntrinsic::HashSha256File => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read(path)
                        .map(|bytes| VmValue::string(sha256_digest(&bytes)))
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::HashSha256String => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(sha256_digest(value.as_bytes())))
            }
            RegIntrinsic::HashSha3_224Bytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(sha3_224_digest(value))))
            }
            RegIntrinsic::HashSha3_256Bytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(sha3_256_digest(value))))
            }
            RegIntrinsic::HashShake128Bytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let out_len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bytes(Rc::new(shake128_digest(value, out_len))))
            }
            RegIntrinsic::HmacSha256Bytes => {
                let key = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(hmac_sha256_digest(key, value)))
            }
            RegIntrinsic::HmacSha256String => {
                let key = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(hmac_sha256_digest(
                    key.as_bytes(),
                    value.as_bytes(),
                )))
            }
            RegIntrinsic::GlobalConfigNew => {
                let value = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(global_config_value(expect_config_rule_count(value)?))
            }
            RegIntrinsic::GlobalConfigRuleCount => {
                let global = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Int(expect_global_config_rule_count(global)?))
            }
            RegIntrinsic::GzipDecompressBytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut decoder = GzDecoder::new(value);
                let mut out = Vec::new();
                Ok(json_result(
                    decoder
                        .read_to_end(&mut out)
                        .map(|_| VmValue::Bytes(Rc::new(out)))
                        .map_err(|error| decode_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::HexDecode
            | RegIntrinsic::HexEncode
            | RegIntrinsic::HexEncodeString => {
                self.exec_hex_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::HttpGet => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(http_get_local(url)))
            }
            RegIntrinsic::HttpGetAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(http_get_local(url)))
            }
            RegIntrinsic::HttpGetRetryAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let attempts = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let _backoff = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                let mut last = Err(http_error_value("HTTP retry attempts must be positive"));
                for _ in 0..attempts.max(1) {
                    last = http_get_local(url);
                    if last.is_ok() {
                        break;
                    }
                }
                Ok(json_result(last))
            }
            RegIntrinsic::HttpGetTimeoutAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(http_get_local(url)))
            }
            RegIntrinsic::HttpPostForm | RegIntrinsic::HttpPostFormAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _ = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP client runtime is not configured for POST form {url}"
                ))))
            }
            RegIntrinsic::HttpPostJson | RegIntrinsic::HttpPostJsonAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _ = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP client runtime is not configured for POST JSON {url}"
                ))))
            }
            RegIntrinsic::HttpPostJsonBearerRetryAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _body = intrinsic_arg(&self.stack, base, args, 1)?;
                let _token = intrinsic_arg(&self.stack, base, args, 2)?;
                let timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                let attempts = expect_int_ref(intrinsic_arg(&self.stack, base, args, 4)?)?;
                let backoff = expect_int_ref(intrinsic_arg(&self.stack, base, args, 5)?)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP async provider is not configured for POST JSON {url} with timeout {timeout}ms attempts {attempts} backoff {backoff}ms"
                ))))
            }
            RegIntrinsic::HttpPostJsonRetryAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _body = intrinsic_arg(&self.stack, base, args, 1)?;
                let timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let attempts = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                let backoff = expect_int_ref(intrinsic_arg(&self.stack, base, args, 4)?)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP async provider is not configured for POST JSON {url} with timeout {timeout}ms attempts {attempts} backoff {backoff}ms"
                ))))
            }
            RegIntrinsic::HttpPostJsonTimeoutAsync => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _body = intrinsic_arg(&self.stack, base, args, 1)?;
                let timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(value_err(http_error_value(format!(
                    "HTTP async provider is not configured for POST JSON {url} with timeout {timeout}ms"
                ))))
            }
            RegIntrinsic::HttpSendAsync => {
                let request = expect_http_request_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if request.method == "GET" {
                    Ok(json_result(http_get_local(&request.url)))
                } else {
                    Ok(value_err(http_error_value(format!(
                        "HTTP async provider is not configured for {} {}",
                        request.method, request.url
                    ))))
                }
            }
            RegIntrinsic::HttpRequestJson => {
                let url = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let body = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(http_request_value("POST", url, body, 0, 1, 0, 0))
            }
            RegIntrinsic::HttpRequestWithHeader => {
                let request = intrinsic_arg(&self.stack, base, args, 0)?;
                let _name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let _value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut request = expect_http_request_ref(request)?;
                request.header_count = request.header_count.saturating_add(1);
                Ok(request.to_value())
            }
            RegIntrinsic::HttpRequestWithRetry => {
                let request = intrinsic_arg(&self.stack, base, args, 0)?;
                let attempts = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let backoff_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut request = expect_http_request_ref(request)?;
                request.attempts = attempts;
                request.backoff_ms = backoff_ms;
                Ok(request.to_value())
            }
            RegIntrinsic::HttpRequestWithTimeout => {
                let request = intrinsic_arg(&self.stack, base, args, 0)?;
                let timeout_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut request = expect_http_request_ref(request)?;
                request.timeout_ms = timeout_ms;
                Ok(request.to_value())
            }
            RegIntrinsic::HttpResponseBytes => {
                let response = intrinsic_arg(&self.stack, base, args, 0)?;
                let text = read_field_ref(response, "body")?;
                let text = expect_string_ref(&text)?;
                Ok(VmValue::Bytes(Rc::new(text.as_bytes().to_vec())))
            }
            RegIntrinsic::HttpResponseIsSuccess => {
                let response = intrinsic_arg(&self.stack, base, args, 0)?;
                let status = expect_int_ref(&read_field_ref(response, "status")?)?;
                Ok(VmValue::Bool((200..300).contains(&status)))
            }
            RegIntrinsic::HttpResponseLines => {
                let response = intrinsic_arg(&self.stack, base, args, 0)?;
                let text = read_field_ref(response, "body")?;
                let text = expect_string_ref(&text)?;
                Ok(VmValue::List(Rc::new(RefCell::new(
                    text.lines().map(VmValue::string).collect(),
                ))))
            }
            RegIntrinsic::HttpResponseStatus => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "status")
            }
            RegIntrinsic::HttpResponseText => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "body")
            }
            RegIntrinsic::ImageInspect => {
                let image = expect_image_state(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let line = image.inspect_line();
                self.push_stdout(&line);
                self.push_stdout("\n");
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ImageLoad => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read(path)
                        .map(|bytes| image_value(bytes, None, None, vec!["load".to_string()]))
                        .map_err(|error| image_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::ImageNormalize => {
                let image_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Image.normalize missing image.".to_string())
                })?;
                let mut image = expect_image_state(self.reg(base + image_reg))?;
                image.operations.push("normalize".to_string());
                self.set_reg(base + image_reg, image.to_value());
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ImageResize => {
                let image_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Image.resize missing image.".to_string())
                })?;
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let height = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut image = expect_image_state(self.reg(base + image_reg))?;
                image.width = Some(width);
                image.height = Some(height);
                image.operations.push("resize".to_string());
                self.set_reg(base + image_reg, image.to_value());
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ImageSave => {
                let image = expect_image_state(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    std::fs::write(path, image.saved_bytes())
                        .map(|_| VmValue::Unit)
                        .map_err(|error| image_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::ImageSharpen => {
                let image_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Image.sharpen missing image.".to_string())
                })?;
                let mut image = expect_image_state(self.reg(base + image_reg))?;
                image.operations.push("sharpen".to_string());
                self.set_reg(base + image_reg, image.to_value());
                Ok(VmValue::Unit)
            }
            RegIntrinsic::InstantElapsed => {
                let start = expect_instant_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(
                    clock_system_unix_ms().saturating_sub(start).max(0),
                ))
            }
            RegIntrinsic::IntBitAnd
            | RegIntrinsic::IntBitNot
            | RegIntrinsic::IntBitOr
            | RegIntrinsic::IntBitXor
            | RegIntrinsic::IntShiftLeft
            | RegIntrinsic::IntShiftRight
            | RegIntrinsic::IntToString
            | RegIntrinsic::IntToFloat
            | RegIntrinsic::FloatToString
            | RegIntrinsic::FloatIsFinite
            | RegIntrinsic::FloatIsInfinite
            | RegIntrinsic::FloatIsNan => {
                self.exec_scalar_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::MathAbs | RegIntrinsic::MathAbsFloat | RegIntrinsic::MathCeil | RegIntrinsic::MathClamp | RegIntrinsic::MathClampFloat | RegIntrinsic::MathCos | RegIntrinsic::MathExp | RegIntrinsic::MathExp2 | RegIntrinsic::MathFloor | RegIntrinsic::MathLog | RegIntrinsic::MathLog2 | RegIntrinsic::MathMax | RegIntrinsic::MathMaxFloat | RegIntrinsic::MathMin | RegIntrinsic::MathMinFloat | RegIntrinsic::MathPow | RegIntrinsic::MathPowFloat | RegIntrinsic::MathRound | RegIntrinsic::MathSin | RegIntrinsic::MathSqrt | RegIntrinsic::MathTanh | RegIntrinsic::MathTruncFloat => self.exec_math_intrinsics(unit, intrinsic, args, base, next_base),
            RegIntrinsic::JsonArray | RegIntrinsic::JsonArrayBools | RegIntrinsic::JsonArrayContainsPrefix | RegIntrinsic::JsonArrayContainsString | RegIntrinsic::JsonArrayContainsSubstring | RegIntrinsic::JsonArrayCountWhere | RegIntrinsic::JsonArrayFold | RegIntrinsic::JsonArrayGet | RegIntrinsic::JsonArrayInts | RegIntrinsic::JsonArrayLen | RegIntrinsic::JsonArrayStrings | RegIntrinsic::JsonAt | RegIntrinsic::JsonAtBool | RegIntrinsic::JsonAtBoolOr | RegIntrinsic::JsonAtInt | RegIntrinsic::JsonAtIntOr | RegIntrinsic::JsonAtOptional | RegIntrinsic::JsonAtOptionalBool | RegIntrinsic::JsonAtOptionalInt | RegIntrinsic::JsonAtOptionalString | RegIntrinsic::JsonAtOr | RegIntrinsic::JsonAtString | RegIntrinsic::JsonAtStringOr | RegIntrinsic::JsonAtToString | RegIntrinsic::JsonAtToStringOr | RegIntrinsic::JsonAsBool | RegIntrinsic::JsonAsInt | RegIntrinsic::JsonAsString | RegIntrinsic::JsonBoolAt | RegIntrinsic::JsonBoolAtOr | RegIntrinsic::JsonBoolField | RegIntrinsic::JsonClone | RegIntrinsic::JsonDecode | RegIntrinsic::JsonDecodeText | RegIntrinsic::JsonEncode | RegIntrinsic::JsonErrorMessage | RegIntrinsic::JsonField | RegIntrinsic::JsonFieldBool | RegIntrinsic::JsonFieldInt | RegIntrinsic::JsonFieldOptional | RegIntrinsic::JsonFieldOptionalBool | RegIntrinsic::JsonFieldOptionalInt | RegIntrinsic::JsonFieldOptionalString | RegIntrinsic::JsonFieldString | RegIntrinsic::JsonIntAt | RegIntrinsic::JsonIntAtOr | RegIntrinsic::JsonIsArray | RegIntrinsic::JsonIsNull | RegIntrinsic::JsonIsObject | RegIntrinsic::JsonIntField | RegIntrinsic::JsonKind | RegIntrinsic::JsonObject | RegIntrinsic::JsonObjectKeys | RegIntrinsic::JsonObjectLen | RegIntrinsic::JsonParse | RegIntrinsic::JsonParseFile | RegIntrinsic::JsonQuoteString | RegIntrinsic::JsonRawField | RegIntrinsic::JsonStringAt | RegIntrinsic::JsonStringAtOr | RegIntrinsic::JsonStringArray | RegIntrinsic::JsonStringField | RegIntrinsic::JsonStrings | RegIntrinsic::JsonToStringAt | RegIntrinsic::JsonToStringAtOr | RegIntrinsic::JsonToString | RegIntrinsic::JsonValue | RegIntrinsic::JsonValues => self.exec_json_intrinsics(unit, intrinsic, args, base, next_base),
            RegIntrinsic::ListAll | RegIntrinsic::ListAny | RegIntrinsic::ListContains | RegIntrinsic::ListContainsValue | RegIntrinsic::ListCountWhere | RegIntrinsic::ListConsume | RegIntrinsic::ListFind | RegIntrinsic::ListFirst | RegIntrinsic::ListFlatMap | RegIntrinsic::ListFlatten | RegIntrinsic::ListGroupBy | RegIntrinsic::ListIsEmpty | RegIntrinsic::ListJoin | RegIntrinsic::ListLast | RegIntrinsic::ListDedup | RegIntrinsic::ListEnumerate | RegIntrinsic::ListMax | RegIntrinsic::ListMin | RegIntrinsic::ListNew | RegIntrinsic::ListPartition | RegIntrinsic::ListReverse | RegIntrinsic::ListSkip | RegIntrinsic::ListSlice | RegIntrinsic::ListSum | RegIntrinsic::ListZip | RegIntrinsic::ListTryFold | RegIntrinsic::ListTake | RegIntrinsic::ListToJsonStrings | RegIntrinsic::ListToJsonValues => self.exec_list_intrinsics(unit, intrinsic, args, base, next_base),
            RegIntrinsic::ListPipeline | RegIntrinsic::PipelineCollect => {
                Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone())
            }
            RegIntrinsic::PipelineEach => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let action = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                for value in values.iter().cloned() {
                    let _ = self.call_closure_one(unit, &action, value, next_base)?;
                }
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::PipelineTryMap => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                let mut mapped = Vec::with_capacity(len);
                for index in 0..len {
                    let value = list.borrow()[index].clone();
                    match result_variant_payload(
                        &self.call_closure_one(unit, &mapper, value, next_base)?,
                    )? {
                        Ok(value) => mapped.push(value),
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(VmValue::List(Rc::new(RefCell::new(mapped)))))
            }
            RegIntrinsic::LogError => {
                let line =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                self.stderr.push_str(&line);
                self.stderr.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogErrorJson => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.stderr.push_str(&value.to_string());
                self.stderr.push('\n');
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogTrace => {
                let event =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                let message =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                self.push_stdout(&format!("trace {event}: {message}\n"));
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogWrite => {
                let line =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                self.push_stdout(&line);
                self.push_stdout("\n");
                Ok(VmValue::Unit)
            }
            RegIntrinsic::LogWriteJson => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.push_stdout(&value.to_string());
                self.push_stdout("\n");
                Ok(VmValue::Unit)
            }
            RegIntrinsic::MapContainsKey | RegIntrinsic::MapFilter | RegIntrinsic::MapFold | RegIntrinsic::MapForEach | RegIntrinsic::MapGetOrDefault | RegIntrinsic::MapIsEmpty | RegIntrinsic::MapKeys | RegIntrinsic::MapLen | RegIntrinsic::MapMapValues | RegIntrinsic::MapMerge | RegIntrinsic::MapNew | RegIntrinsic::MapTryFold | RegIntrinsic::MapValues => self.exec_map_intrinsics(unit, intrinsic, args, base, next_base),
            RegIntrinsic::OptionAndThen
            | RegIntrinsic::OptionFilter
            | RegIntrinsic::OptionIsNone
            | RegIntrinsic::OptionIsSome
            | RegIntrinsic::OptionMap
            | RegIntrinsic::OptionOkOr
            | RegIntrinsic::OptionOr
            | RegIntrinsic::OptionUnwrapOr
            | RegIntrinsic::OptionUnwrapOrElse => {
                self.exec_option_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::CloneClone => {
                let value = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(deep_copy_value(value))
            }
            RegIntrinsic::OrdCompare => {
                let left = intrinsic_arg(&self.stack, base, args, 0)?;
                let right = intrinsic_arg(&self.stack, base, args, 1)?;
                let value = match vm_value_cmp(left, right)? {
                    Ordering::Less => -1,
                    Ordering::Equal => 0,
                    Ordering::Greater => 1,
                };
                Ok(VmValue::Int(value))
            }
            RegIntrinsic::OsClose => {
                let _ = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::PatchApplyText => {
                let original = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let patch = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    patch_apply_text_string(original, patch)
                        .map(VmValue::string)
                        .map_err(VmValue::string),
                ))
            }
            RegIntrinsic::ProcessRun | RegIntrinsic::ProcessRunAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    process_run_output(command, &argv).map(process_output_value),
                ))
            }
            RegIntrinsic::ProcessRunStdout | RegIntrinsic::ProcessRunStdoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(process_run_output(command, &argv).and_then(
                    |output| process_stdout_result(command, output).map(VmValue::string),
                )))
            }
            RegIntrinsic::ProcessRunTimeout | RegIntrinsic::ProcessRunTimeoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_result(
                    process_run_output(command, &argv).map(process_output_value),
                ))
            }
            RegIntrinsic::ProcessRunStdoutTimeout | RegIntrinsic::ProcessRunStdoutTimeoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_result(process_run_output(command, &argv).and_then(
                    |output| process_stdout_result(command, output).map(VmValue::string),
                )))
            }
            RegIntrinsic::ProcessRunManyStdout | RegIntrinsic::ProcessRunManyStdoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let appended = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let _jobs = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                Ok(json_result(
                    process_run_many_stdout(command, &argv, &appended).map(|items| {
                        VmValue::List(Rc::new(RefCell::new(
                            items.into_iter().map(VmValue::string).collect(),
                        )))
                    }),
                ))
            }
            RegIntrinsic::ProcessRunManyStdoutTimeout
            | RegIntrinsic::ProcessRunManyStdoutTimeoutAsync => {
                let command = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let argv = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let appended = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let _jobs = expect_int_ref(intrinsic_arg(&self.stack, base, args, 3)?)?;
                let _timeout = expect_int_ref(intrinsic_arg(&self.stack, base, args, 4)?)?;
                Ok(json_result(
                    process_run_many_stdout(command, &argv, &appended).map(|items| {
                        VmValue::List(Rc::new(RefCell::new(
                            items.into_iter().map(VmValue::string).collect(),
                        )))
                    }),
                ))
            }
            RegIntrinsic::ProcessRunRequest
            | RegIntrinsic::ProcessRunRequestAsync
            | RegIntrinsic::ProcessRunRequestCancellableAsync => {
                let request =
                    expect_process_request_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if matches!(intrinsic, RegIntrinsic::ProcessRunRequestCancellableAsync) {
                    let _ = expect_cancellation_id_ref(
                        intrinsic_arg(&self.stack, base, args, 1)?,
                        "CancellationToken",
                    )?;
                }
                Ok(json_result(
                    process_run_request(&request).map(process_output_value),
                ))
            }
            RegIntrinsic::ProcessStream => {
                let request =
                    expect_process_request_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(process_run_request(&request).map(|output| {
                    let mut events = Vec::new();
                    if !output.stdout.is_empty() {
                        events.push(process_event_value("stdout", &output.stdout, output.status));
                    }
                    if !output.stderr.is_empty() {
                        events.push(process_event_value("stderr", &output.stderr, output.status));
                    }
                    events.push(process_event_value("exit", "", output.status));
                    stream_value(events)
                })))
            }
            RegIntrinsic::SetContains
            | RegIntrinsic::SetDifference
            | RegIntrinsic::SetIntersection
            | RegIntrinsic::SetIsEmpty
            | RegIntrinsic::SetIsSubset
            | RegIntrinsic::SetLen
            | RegIntrinsic::SetNew
            | RegIntrinsic::SetToList
            | RegIntrinsic::SetUnion
            | RegIntrinsic::SortedSetContains
            | RegIntrinsic::SortedSetIsEmpty
            | RegIntrinsic::SortedSetLen
            | RegIntrinsic::SortedSetNew
            | RegIntrinsic::SortedSetToList
            | RegIntrinsic::SortedMapContainsKey
            | RegIntrinsic::SortedMapGet
            | RegIntrinsic::SortedMapIsEmpty
            | RegIntrinsic::SortedMapKeys
            | RegIntrinsic::SortedMapLen
            | RegIntrinsic::SortedMapNew
            | RegIntrinsic::SortedMapValues => {
                self.exec_set_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::PathExists | RegIntrinsic::PathExtension | RegIntrinsic::PathFileName | RegIntrinsic::PathFromString | RegIntrinsic::PathToString | RegIntrinsic::PathIsAbsolute | RegIntrinsic::PathIsDir | RegIntrinsic::PathIsFile | RegIntrinsic::PathJoin | RegIntrinsic::PathListFiles | RegIntrinsic::PathListPaths | RegIntrinsic::PathNormalize | RegIntrinsic::PathParent | RegIntrinsic::PathReadString | RegIntrinsic::PathResolveRelative | RegIntrinsic::PathSafeRelative | RegIntrinsic::PathStartsWith | RegIntrinsic::PathWithExtension | RegIntrinsic::PathWriteString => self.exec_path_intrinsics(unit, intrinsic, args, base, next_base),
            RegIntrinsic::PersistentMapClear => Ok(sorted_map_value(Vec::new())),
            RegIntrinsic::PersistentMapContainsKey => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(sorted_map_get(&entries, key).is_some()))
            }
            RegIntrinsic::PersistentMapGet => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(sorted_map_get(&entries, key)
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::PersistentMapInsert => {
                let mut entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let value = intrinsic_arg(&self.stack, base, args, 2)?.clone();
                sorted_map_insert(&mut entries, key, value);
                Ok(sorted_map_value(entries))
            }
            RegIntrinsic::PersistentMapIsEmpty => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(entries.is_empty()))
            }
            RegIntrinsic::PersistentMapLen => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(entries.len() as i64))
            }
            RegIntrinsic::PersistentMapNew => Ok(sorted_map_value(Vec::new())),
            RegIntrinsic::PersistentMapRemove => {
                let mut entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                sorted_map_remove(&mut entries, key);
                Ok(sorted_map_value(entries))
            }
            RegIntrinsic::PoolStatsAvailable => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "available")
            }
            RegIntrinsic::PoolStatsCapacity => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "capacity")
            }
            RegIntrinsic::PoolStatsCreated => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "created")
            }
            RegIntrinsic::PoolStatsInUse => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "in_use")
            }
            RegIntrinsic::ResourcePoolBorrow => {
                self.resource_pool_borrow(unit, args, base, next_base, false)
            }
            RegIntrinsic::ResourcePoolDiscard => {
                let lease_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM ResourcePool.discard missing lease.".to_string())
                })?;
                let lease = self.reg(base + lease_reg).clone();
                self.set_reg(base + lease_reg, mark_pool_lease_discarded(lease)?);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ResourcePoolLazy => {
                self.resource_pool_new(unit, args, base, next_base, true, false)
            }
            RegIntrinsic::ResourcePoolNew => {
                self.resource_pool_new(unit, args, base, next_base, false, false)
            }
            RegIntrinsic::ResourcePoolStats => {
                let pool = expect_resource_pool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let stats = self.pools.get(&pool.id).cloned().unwrap_or(VmResourcePool {
                    capacity: 0,
                    created: 0,
                    in_use: 0,
                    idle: Vec::new(),
                    factory: None,
                    factory_returns_result: false,
                });
                Ok(pool_stats_value(
                    stats.capacity,
                    stats.created,
                    stats.idle.len() as i64,
                    stats.in_use,
                ))
            }
            RegIntrinsic::ResourcePoolTryBorrow => {
                self.resource_pool_borrow(unit, args, base, next_base, true)
            }
            RegIntrinsic::ResourcePoolTryLazy => {
                self.resource_pool_new(unit, args, base, next_base, true, true)
            }
            RegIntrinsic::ResourcePoolTryNew => {
                self.resource_pool_new(unit, args, base, next_base, false, true)
            }
            RegIntrinsic::RandomBool => {
                let mut rng = rand::thread_rng();
                Ok(VmValue::Bool(rng.r#gen()))
            }
            RegIntrinsic::RandomBytes => {
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut rng = rand::thread_rng();
                let mut bytes = vec![0u8; len.max(0) as usize];
                rng.fill(bytes.as_mut_slice());
                Ok(VmValue::Bytes(Rc::new(bytes)))
            }
            RegIntrinsic::RandomFloat => {
                let mut rng = rand::thread_rng();
                Ok(VmValue::Float(rng.r#gen()))
            }
            RegIntrinsic::RandomInt => {
                let min = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let max = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut rng = rand::thread_rng();
                Ok(VmValue::Int(rng.gen_range(min..=max)))
            }
            RegIntrinsic::RandomString => {
                const CHARSET: &[u8] =
                    b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut rng = rand::thread_rng();
                let value = (0..len.max(0))
                    .map(|_| {
                        let idx = rng.gen_range(0..CHARSET.len());
                        CHARSET[idx] as char
                    })
                    .collect::<String>();
                Ok(VmValue::string(value))
            }
            RegIntrinsic::RegexCaptures
            | RegIntrinsic::RegexCompile
            | RegIntrinsic::RegexErrorMessage
            | RegIntrinsic::RegexFind
            | RegIntrinsic::RegexIsMatch
            | RegIntrinsic::RegexReplaceAll
            | RegIntrinsic::RegexSplit => {
                self.exec_regex_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::RequestNew => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(request_value(path))
            }
            RegIntrinsic::RequestPath => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "path")
            }
            RegIntrinsic::ReceiverClose => {
                let receiver_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Receiver.close missing receiver.".to_string())
                })?;
                let receiver = expect_receiver_ref(self.reg(base + receiver_reg))?;
                self.channel_state_mut(receiver.channel_id)?.receiver_closed = true;
                self.set_reg(
                    base + receiver_reg,
                    receiver_value(receiver.channel_id, true),
                );
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ReceiverIntoStream => {
                let receiver = expect_receiver_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if receiver.closed {
                    return Ok(stream_collect_error_value("channel receiver closed"));
                }
                Ok(stream_channel_value(receiver.channel_id))
            }
            RegIntrinsic::ReceiverRecv => {
                let receiver = expect_receiver_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if receiver.closed {
                    return Ok(value_err(channel_error_value("channel receiver closed")));
                }
                if !self.channel_ready(receiver.channel_id) {
                    // Empty open channel: park until a sender enqueues or closes.
                    self.suspension = Some(Suspension {
                        wait: Wait::Recv {
                            channel: receiver.channel_id,
                        },
                        resume_dst: usize::MAX,
                    });
                    return Ok(VmValue::Unit);
                }
                Ok(json_result(self.channel_recv(receiver.channel_id)))
            }
            RegIntrinsic::ReceiverRecvCancellable => {
                let receiver = expect_receiver_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if receiver.closed {
                    return Ok(value_err(channel_error_value("channel receiver closed")));
                }
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 1)?,
                    "CancellationToken",
                )?;
                if self.cancellation_flags.get(&id).copied().unwrap_or(false) {
                    return Ok(value_err(channel_error_value("channel recv cancelled")));
                }
                if !self.channel_ready(receiver.channel_id) {
                    self.suspension = Some(Suspension {
                        wait: Wait::Recv {
                            channel: receiver.channel_id,
                        },
                        resume_dst: usize::MAX,
                    });
                    return Ok(VmValue::Unit);
                }
                Ok(json_result(self.channel_recv(receiver.channel_id)))
            }
            RegIntrinsic::ResponseBody => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "body")
            }
            RegIntrinsic::ResponseOk => {
                let body = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(value_ok(response_value(200, body)))
            }
            RegIntrinsic::ResponseStatus => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "status")
            }
            RegIntrinsic::RowBufferNew => {
                let size = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(row_buffer_value(Vec::with_capacity(size.max(0) as usize)))
            }
            RegIntrinsic::RowFieldString => {
                let row = intrinsic_arg(&self.stack, base, args, 0)?;
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(row_field_string_value(
                    expect_row_fields_ref(row)?,
                    index,
                )))
            }
            RegIntrinsic::ResultErr
            | RegIntrinsic::ResultErrMessage
            | RegIntrinsic::ResultIsErr
            | RegIntrinsic::ResultIsOk
            | RegIntrinsic::ResultOk
            | RegIntrinsic::ResultAndThen
            | RegIntrinsic::ResultMap
            | RegIntrinsic::ResultMapError
            | RegIntrinsic::ResultUnwrapOr
            | RegIntrinsic::ResultUnwrapOrElse => {
                self.exec_result_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::RuleLoaderLoadRules => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(|text| VmValue::List(Rc::new(RefCell::new(rules_from_text(&text)))))
                        .map_err(|error| config_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::StringAfter | RegIntrinsic::StringBefore | RegIntrinsic::StringBuilderNew | RegIntrinsic::StringCharAt | RegIntrinsic::StringChars | RegIntrinsic::StringContains | RegIntrinsic::StringCount | RegIntrinsic::StringCopy | RegIntrinsic::StringEndsWith | RegIntrinsic::StringFormat | RegIntrinsic::StringFromBool | RegIntrinsic::StringFromFloat | RegIntrinsic::StringFromInt | RegIntrinsic::StringIndexOf | RegIntrinsic::StringIsEmpty | RegIntrinsic::StringJoin | RegIntrinsic::StringLines | RegIntrinsic::StringLen | RegIntrinsic::StringPadLeft | RegIntrinsic::StringPadRight | RegIntrinsic::StringParseFloat | RegIntrinsic::StringParseInt | RegIntrinsic::StringRepeat | RegIntrinsic::StringReplace | RegIntrinsic::StringReplaceFirst | RegIntrinsic::StringReverse | RegIntrinsic::StringSlice | RegIntrinsic::StringSplit | RegIntrinsic::StringStartsWith | RegIntrinsic::StringStripPrefix | RegIntrinsic::StringToLowercase | RegIntrinsic::StringToUppercase | RegIntrinsic::StringTrim | RegIntrinsic::StringTrimEnd | RegIntrinsic::StringTrimStart => self.exec_string_intrinsics(unit, intrinsic, args, base, next_base),
            RegIntrinsic::StreamCollectList => {
                let stream = expect_stream_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if let Some(message) = stream.collect_error {
                    return Ok(value_err(channel_error_value(message)));
                }
                if let Some(channel_id) = stream.channel_id {
                    let state = self.channel_state_mut(channel_id)?;
                    let values = state.queue.drain(..).collect::<Vec<_>>();
                    if state.senders == 0 {
                        return Ok(value_ok(VmValue::List(Rc::new(RefCell::new(values)))));
                    }
                    return Ok(value_err(channel_error_value(
                        "stream collect_list would block on an open channel stream",
                    )));
                }
                let values = stream.items.borrow_mut().drain(..).collect::<Vec<_>>();
                Ok(value_ok(VmValue::List(Rc::new(RefCell::new(values)))))
            }
            RegIntrinsic::StreamFromList => {
                let items = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?
                    .borrow()
                    .clone();
                Ok(stream_value(items))
            }
            RegIntrinsic::StreamNext => {
                let stream = expect_stream_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                if let Some(message) = stream.collect_error {
                    return Ok(value_err(channel_error_value(message)));
                }
                if let Some(channel_id) = stream.channel_id {
                    return Ok(json_result(self.channel_recv(channel_id)));
                }
                let value = if stream.items.borrow().is_empty() {
                    VmValue::OptionNone
                } else {
                    VmValue::OptionSome(Box::new(stream.items.borrow_mut().remove(0)))
                };
                Ok(value_ok(value))
            }
            RegIntrinsic::SenderClose => {
                let sender_reg = *args.first().ok_or_else(|| {
                    EvalError::Runtime("reg VM Sender.close missing sender.".to_string())
                })?;
                let sender = expect_sender_ref(self.reg(base + sender_reg))?;
                if !sender.closed {
                    let state = self.channel_state_mut(sender.channel_id)?;
                    state.senders = state.senders.saturating_sub(1);
                }
                self.set_reg(base + sender_reg, sender_value(sender.channel_id, true));
                Ok(VmValue::Unit)
            }
            RegIntrinsic::SenderSend => {
                let sender = expect_sender_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                if self.channel_send_would_block(&sender) {
                    // Full bounded channel: park until a receiver frees space.
                    self.suspension = Some(Suspension {
                        wait: Wait::Send { sender, value },
                        resume_dst: usize::MAX,
                    });
                    return Ok(VmValue::Unit);
                }
                Ok(json_result(self.channel_send(sender, value)))
            }
            RegIntrinsic::SenderSendCancellable => {
                let sender = expect_sender_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let id = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 2)?,
                    "CancellationToken",
                )?;
                if self.cancellation_flags.get(&id).copied().unwrap_or(false) {
                    return Ok(value_err(channel_error_value("channel send cancelled")));
                }
                if self.channel_send_would_block(&sender) {
                    self.suspension = Some(Suspension {
                        wait: Wait::Send { sender, value },
                        resume_dst: usize::MAX,
                    });
                    return Ok(VmValue::Unit);
                }
                Ok(json_result(self.channel_send(sender, value)))
            }
            RegIntrinsic::TcpConnect => {
                let host =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                let port = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(self.tcp_connect(&host, port)))
            }
            RegIntrinsic::TcpStreamRead => {
                let id = expect_tcp_stream_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let max_bytes = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    self.tcp_stream_read(id, max_bytes)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes))),
                ))
            }
            RegIntrinsic::TcpStreamShutdown => {
                let id = expect_tcp_stream_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    self.tcp_stream_shutdown(id).map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::TcpStreamWrite => {
                let id = expect_tcp_stream_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(json_result(
                    self.tcp_stream_write(id, &data).map(VmValue::Int),
                ))
            }
            RegIntrinsic::TcpStreamWriteAll => {
                let id = expect_tcp_stream_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(json_result(
                    self.tcp_stream_write_all(id, &data).map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::TempDirKeep | RegIntrinsic::TempDirPath => {
                let dir = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::string(expect_tempdir_path_ref(dir)?))
            }
            RegIntrinsic::TempDirNew => Ok(json_result(tempdir_new_value(std::env::temp_dir()))),
            RegIntrinsic::TempDirNewIn => {
                let parent = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(tempdir_new_value(PathBuf::from(parent))))
            }
            RegIntrinsic::TimerSleep => {
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.park_sleep_ms(ms);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::TimerSleepCancellable => {
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let _ = expect_cancellation_id_ref(
                    intrinsic_arg(&self.stack, base, args, 1)?,
                    "CancellationToken",
                )?;
                self.park_sleep_ms(ms);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::TimerSleepUntil => {
                let target_unix_ms =
                    expect_deadline_unix_ms(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let now_unix_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0);
                self.park_sleep_ms(target_unix_ms - now_unix_ms);
                Ok(VmValue::Unit)
            }
            RegIntrinsic::TomlParseFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(toml_parse_file_value(path)))
            }
            RegIntrinsic::UrlDecodeComponent
            | RegIntrinsic::UrlEncodeComponent
            | RegIntrinsic::UrlFromString
            | RegIntrinsic::UrlToString => {
                self.exec_url_intrinsics(unit, intrinsic, args, base, next_base)
            }
            RegIntrinsic::UuidNewV4 => Ok(VmValue::string(uuid::Uuid::new_v4().to_string())),
            RegIntrinsic::WebSocketConnect => {
                let url =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string();
                Ok(json_result(self.websocket_connect(&url)))
            }
            RegIntrinsic::WebSocketClose => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    self.websocket_close(id).map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::WebSocketRecvBytes => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    self.websocket_recv(id, WebSocketExpectedFrame::Binary).map(
                        |value| match value {
                            Some(bytes) => value_some(VmValue::Bytes(Rc::new(bytes))),
                            None => value_none(),
                        },
                    ),
                ))
            }
            RegIntrinsic::WebSocketRecvText => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    self.websocket_recv(id, WebSocketExpectedFrame::Text).map(
                        |value| match value {
                            Some(bytes) => {
                                value_some(VmValue::string(String::from_utf8_lossy(&bytes)))
                            }
                            None => value_none(),
                        },
                    ),
                ))
            }
            RegIntrinsic::WebSocketSendBytes => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let data = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_vec();
                Ok(json_result(
                    self.websocket_send(id, 0x2, &data).map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::WebSocketSendText => {
                let id = expect_websocket_id_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(json_result(
                    self.websocket_send(id, 0x1, text.as_bytes())
                        .map(|()| VmValue::Unit),
                ))
            }
            RegIntrinsic::YamlParse => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(yaml_parse_json_value(text)))
            }
            RegIntrinsic::YamlParseFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map_err(|error| json_error_value(error.to_string()))
                        .and_then(|text| yaml_parse_json_value(&text)),
                ))
            }
            RegIntrinsic::WeakDowngrade | RegIntrinsic::WeakFrom => {
                Ok(intrinsic_arg(&self.stack, base, args, 0)?.clone())
            }
            RegIntrinsic::WeakUpgrade => Ok(VmValue::OptionSome(Box::new(
                intrinsic_arg(&self.stack, base, args, 0)?.clone(),
            ))),
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn exec_json_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::JsonArray => {
                let items = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(format!("[{}]", items.join(","))))
            }
            RegIntrinsic::JsonArrayBools => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_array_bools_value(value)))
            }
            RegIntrinsic::JsonArrayContainsPrefix => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_contains_string_value(
                    value,
                    prefix,
                    JsonArrayStringMatch::Prefix,
                )))
            }
            RegIntrinsic::JsonArrayContainsString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let item = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_contains_string_value(
                    value,
                    item,
                    JsonArrayStringMatch::Exact,
                )))
            }
            RegIntrinsic::JsonArrayContainsSubstring => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_contains_string_value(
                    value,
                    text,
                    JsonArrayStringMatch::Substring,
                )))
            }
            RegIntrinsic::JsonArrayCountWhere => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let items = match json_array_items(value) {
                    Ok(items) => items.clone(),
                    Err(error) => return Ok(value_err(error)),
                };
                let mut count = 0_i64;
                for item in items {
                    let result = self.call_closure_one(
                        unit,
                        &predicate,
                        VmValue::Json(Rc::new(item)),
                        next_base,
                    )?;
                    match result_variant_payload(&result)? {
                        Ok(value) => {
                            if expect_bool_ref(&value)? {
                                count += 1;
                            }
                        }
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(VmValue::Int(count)))
            }
            RegIntrinsic::JsonArrayFold => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let items = match json_array_items(value) {
                    Ok(items) => items.clone(),
                    Err(error) => return Ok(value_err(error)),
                };
                for item in items {
                    let result = self.call_closure_two(
                        unit,
                        &folder,
                        state,
                        VmValue::Json(Rc::new(item)),
                        next_base,
                    )?;
                    match result_variant_payload(&result)? {
                        Ok(value) => state = value,
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(state))
            }
            RegIntrinsic::JsonArrayGet => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_array_get_value(value, index)))
            }
            RegIntrinsic::JsonArrayInts => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_array_ints_value(value)))
            }
            RegIntrinsic::JsonArrayLen => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = value
                    .as_array()
                    .map(|items| VmValue::Int(items.len() as i64))
                    .ok_or_else(|| json_error_value("JSON value is not an array"));
                Ok(json_result(result))
            }
            RegIntrinsic::JsonArrayStrings => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_array_strings_value(value)))
            }
            RegIntrinsic::JsonAt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).map(|value| VmValue::Json(Rc::new(value))),
                ))
            }
            RegIntrinsic::JsonAtBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).and_then(json_as_bool_value),
                ))
            }
            RegIntrinsic::JsonAtBoolOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_value_at(value, path)
                    .and_then(json_as_bool_value)
                    .unwrap_or(VmValue::Bool(fallback)))
            }
            RegIntrinsic::JsonAtInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).and_then(json_as_int_value),
                ))
            }
            RegIntrinsic::JsonAtIntOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(json_value_at(value, path)
                    .and_then(json_as_int_value)
                    .unwrap_or(VmValue::Int(fallback)))
            }
            RegIntrinsic::JsonAtOptional => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value_ok(json_optional_path_value(value, path)))
            }
            RegIntrinsic::JsonAtOptionalBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_path_value(
                    value,
                    path,
                    json_as_bool_value,
                )))
            }
            RegIntrinsic::JsonAtOptionalInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_path_value(
                    value,
                    path,
                    json_as_int_value,
                )))
            }
            RegIntrinsic::JsonAtOptionalString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_path_value(
                    value,
                    path,
                    json_as_string_value,
                )))
            }
            RegIntrinsic::JsonAtOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_json_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::Json(Rc::new(
                    json_value_at(value, path).unwrap_or_else(|_| fallback.clone()),
                )))
            }
            RegIntrinsic::JsonAtString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).and_then(json_as_string_value),
                ))
            }
            RegIntrinsic::JsonAtStringOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(json_value_at(value, path)
                    .and_then(json_as_string_value)
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonAtToString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    json_value_at(value, path).map(|value| VmValue::string(value.to_string())),
                ))
            }
            RegIntrinsic::JsonAtToStringOr => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(json_value_at(value, path)
                    .map(|value| VmValue::string(value.to_string()))
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonAsBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(value.as_bool().map(VmValue::Bool).ok_or_else(
                    || json_error_value("JSON value is not a boolean"),
                )))
            }
            RegIntrinsic::JsonAsInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(value.as_i64().map(VmValue::Int).ok_or_else(
                    || json_error_value("JSON value is not an integer"),
                )))
            }
            RegIntrinsic::JsonAsString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(value.as_str().map(VmValue::string).ok_or_else(
                    || json_error_value("JSON value is not a string"),
                )))
            }
            RegIntrinsic::JsonBoolAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .and_then(json_as_bool_value),
                ))
            }
            RegIntrinsic::JsonBoolAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .and_then(json_as_bool_value)
                    .unwrap_or(VmValue::Bool(fallback)))
            }
            RegIntrinsic::JsonBoolField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    value
                )))
            }
            RegIntrinsic::JsonClone => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Json(Rc::new(value.clone())))
            }
            RegIntrinsic::JsonDecode | RegIntrinsic::JsonDecodeText => Err(EvalError::Runtime(
                "reg VM Json.decode requires typed intrinsic metadata.".to_string(),
            )),
            RegIntrinsic::JsonEncode => {
                let value = vm_value_to_json_literal(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::JsonErrorMessage => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "message")
            }
            RegIntrinsic::JsonField => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_field_value(value, name)))
            }
            RegIntrinsic::JsonFieldBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_typed_field_value(
                    value,
                    name,
                    "boolean",
                    |field| field.as_bool().map(VmValue::Bool),
                )))
            }
            RegIntrinsic::JsonFieldInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_typed_field_value(
                    value,
                    name,
                    "integer",
                    |field| field.as_i64().map(VmValue::Int),
                )))
            }
            RegIntrinsic::JsonFieldOptional => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value_ok(json_optional_field_value(value, name)))
            }
            RegIntrinsic::JsonFieldOptionalBool => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_field_value(
                    value,
                    name,
                    "boolean",
                    |field| field.as_bool().map(VmValue::Bool),
                )))
            }
            RegIntrinsic::JsonFieldOptionalInt => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_field_value(
                    value,
                    name,
                    "integer",
                    |field| field.as_i64().map(VmValue::Int),
                )))
            }
            RegIntrinsic::JsonFieldOptionalString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_optional_typed_field_value(
                    value,
                    name,
                    "string",
                    |field| field.as_str().map(VmValue::string),
                )))
            }
            RegIntrinsic::JsonFieldString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(json_typed_field_value(
                    value,
                    name,
                    "string",
                    |field| field.as_str().map(VmValue::string),
                )))
            }
            RegIntrinsic::JsonIntAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .and_then(json_as_int_value),
                ))
            }
            RegIntrinsic::JsonIntAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .and_then(json_as_int_value)
                    .unwrap_or(VmValue::Int(fallback)))
            }
            RegIntrinsic::JsonIsArray => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_array()))
            }
            RegIntrinsic::JsonIsNull => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_null()))
            }
            RegIntrinsic::JsonIsObject => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_object()))
            }
            RegIntrinsic::JsonIntField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    value
                )))
            }
            RegIntrinsic::JsonKind => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(json_kind(value)))
            }
            RegIntrinsic::JsonObject => {
                let fields = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(format!("{{{}}}", fields.join(","))))
            }
            RegIntrinsic::JsonObjectKeys => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = value
                    .as_object()
                    .map(|fields| {
                        let mut keys = fields.keys().map(VmValue::string).collect::<Vec<_>>();
                        keys.sort_by_key(VmValue::display);
                        VmValue::List(Rc::new(RefCell::new(keys)))
                    })
                    .ok_or_else(|| json_error_value("JSON value is not an object"));
                Ok(json_result(result))
            }
            RegIntrinsic::JsonObjectLen => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = value
                    .as_object()
                    .map(|fields| VmValue::Int(fields.len() as i64))
                    .ok_or_else(|| json_error_value("JSON value is not an object"));
                Ok(json_result(result))
            }
            RegIntrinsic::JsonParse => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    serde_json::from_str::<serde_json::Value>(text)
                        .map(|value| VmValue::Json(Rc::new(value)))
                        .map_err(|error| json_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::JsonParseFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map_err(|error| json_error_value(error.to_string()))
                        .and_then(|text| {
                            serde_json::from_str::<serde_json::Value>(&text)
                                .map(|value| VmValue::Json(Rc::new(value)))
                                .map_err(|error| json_error_value(error.to_string()))
                        }),
                ))
            }
            RegIntrinsic::JsonQuoteString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(json_quote_string(value)?))
            }
            RegIntrinsic::JsonRawField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    value
                )))
            }
            RegIntrinsic::JsonStringAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .and_then(json_as_string_value),
                ))
            }
            RegIntrinsic::JsonStringAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .and_then(json_as_string_value)
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonStringArray => {
                let items = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let quoted = items
                    .iter()
                    .map(|item| json_quote_string(item))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::string(format!("[{}]", quoted.join(","))))
            }
            RegIntrinsic::JsonStringField => {
                let name = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(format!(
                    "{}:{}",
                    json_quote_string(name)?,
                    json_quote_string(value)?
                )))
            }
            RegIntrinsic::JsonStrings => {
                let items = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(
                    items.into_iter().map(serde_json::Value::String).collect(),
                ))))
            }
            RegIntrinsic::JsonToStringAt => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    parse_json_text(text)
                        .and_then(|value| json_value_at(&value, path))
                        .map(|value| VmValue::string(value.to_string())),
                ))
            }
            RegIntrinsic::JsonToStringAtOr => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fallback =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_string();
                Ok(parse_json_text(text)
                    .and_then(|value| json_value_at(&value, path))
                    .map(|value| VmValue::string(value.to_string()))
                    .unwrap_or_else(|_| VmValue::string(fallback)))
            }
            RegIntrinsic::JsonToString => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::JsonValue => {
                let value = vm_value_to_json_literal(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Json(Rc::new(value)))
            }
            RegIntrinsic::JsonValues => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = list
                    .borrow()
                    .iter()
                    .map(|value| expect_json_ref(value).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(values))))
            }
            other => unreachable!(
                "exec_json_intrinsics called with non-json intrinsic: {other:?}"
            ),
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn exec_string_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        let _ = unit;
        match intrinsic {
            RegIntrinsic::StringAfter => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .split_once(delimiter)
                    .map(|(_, right)| VmValue::OptionSome(Box::new(VmValue::string(right))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringBefore => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .find(delimiter)
                    .map(|index| VmValue::OptionSome(Box::new(VmValue::string(&value[..index]))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringBuilderNew => Ok(VmValue::string("")),
            RegIntrinsic::StringCharAt => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(usize::try_from(index)
                    .ok()
                    .and_then(|index| value.chars().nth(index))
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Char(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringChars => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::List(Rc::new(RefCell::new(
                    value.chars().map(VmValue::Char).collect(),
                ))))
            }
            RegIntrinsic::StringContains => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let needle = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.contains(needle)))
            }
            RegIntrinsic::StringCount => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let needle = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(value.matches(needle).count() as i64))
            }
            RegIntrinsic::StringCopy => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value))
            }
            RegIntrinsic::StringEndsWith => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let suffix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.ends_with(suffix)))
            }
            RegIntrinsic::StringFormat => {
                let template = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let args = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(string_format(template, &args)))
            }
            RegIntrinsic::StringFromBool => {
                let value = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::StringFromFloat => Ok(VmValue::string(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
            RegIntrinsic::StringFromInt => Ok(VmValue::string(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
            RegIntrinsic::StringIndexOf => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let needle = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .find(needle)
                    .map(|index| VmValue::OptionSome(Box::new(VmValue::Int(index as i64))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringIsEmpty => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_empty()))
            }
            RegIntrinsic::StringJoin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let separator =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(VmValue::string(join_string_values(
                    &list.borrow(),
                    &separator,
                )?))
            }
            RegIntrinsic::StringLines => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let lines = value.lines().map(VmValue::string).collect::<Vec<VmValue>>();
                Ok(VmValue::List(Rc::new(RefCell::new(lines))))
            }
            RegIntrinsic::StringLen => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value.len() as i64))
            }
            RegIntrinsic::StringPadLeft => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fill = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(string_pad(value, width, fill, true)))
            }
            RegIntrinsic::StringPadRight => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fill = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(string_pad(value, width, fill, false)))
            }
            RegIntrinsic::StringParseFloat => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match value.parse::<f64>() {
                    Ok(value) => VmValue::OptionSome(Box::new(VmValue::Float(value))),
                    Err(_) => VmValue::OptionNone,
                })
            }
            RegIntrinsic::StringParseInt => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match value.parse::<i64>() {
                    Ok(value) => VmValue::OptionSome(Box::new(VmValue::Int(value))),
                    Err(_) => VmValue::OptionNone,
                })
            }
            RegIntrinsic::StringRepeat => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(value.repeat(count)))
            }
            RegIntrinsic::StringReplace => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(value.replace(from, to)))
            }
            RegIntrinsic::StringReplaceFirst => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(value.replacen(from, to, 1)))
            }
            RegIntrinsic::StringReverse => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.chars().rev().collect::<String>()))
            }
            RegIntrinsic::StringSlice => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(string_slice_range(value, start, len)))
            }
            RegIntrinsic::StringSplit => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let parts = value
                    .split(delimiter)
                    .map(VmValue::string)
                    .collect::<Vec<VmValue>>();
                Ok(VmValue::List(Rc::new(RefCell::new(parts))))
            }
            RegIntrinsic::StringStartsWith => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.starts_with(prefix)))
            }
            RegIntrinsic::StringStripPrefix => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .strip_prefix(prefix)
                    .map(|rest| VmValue::OptionSome(Box::new(VmValue::string(rest))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringToLowercase => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_lowercase()))
            }
            RegIntrinsic::StringToUppercase => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_uppercase()))
            }
            RegIntrinsic::StringTrim => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.trim()))
            }
            RegIntrinsic::StringTrimEnd => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.trim_end()))
            }
            RegIntrinsic::StringTrimStart => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.trim_start()))
            }
            other => unreachable!(
                "exec_string_intrinsics called with non-string intrinsic: {other:?}"
            ),
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn exec_list_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::ListAll => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                for value in values {
                    let keep = self.call_closure_one(unit, &predicate, value, next_base)?;
                    if !expect_bool_ref(&keep)? {
                        return Ok(VmValue::Bool(false));
                    }
                }
                Ok(VmValue::Bool(true))
            }
            RegIntrinsic::ListAny | RegIntrinsic::ListContains => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                for value in values {
                    let matched = self.call_closure_one(unit, &predicate, value, next_base)?;
                    if expect_bool_ref(&matched)? {
                        return Ok(VmValue::Bool(true));
                    }
                }
                Ok(VmValue::Bool(false))
            }
            RegIntrinsic::ListContainsValue => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(
                    list.borrow().iter().any(|item| item == value),
                ))
            }
            RegIntrinsic::ListCountWhere => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                let mut count = 0;
                for value in values {
                    let matched = self.call_closure_one(unit, &predicate, value, next_base)?;
                    if expect_bool_ref(&matched)? {
                        count += 1;
                    }
                }
                Ok(VmValue::Int(count))
            }
            RegIntrinsic::ListConsume => {
                let _ = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ListFind => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                for value in values {
                    let matched =
                        self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                    if expect_bool_ref(&matched)? {
                        return Ok(VmValue::OptionSome(Box::new(value)));
                    }
                }
                Ok(VmValue::OptionNone)
            }
            RegIntrinsic::ListFirst => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(list
                    .borrow()
                    .first()
                    .cloned()
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListFlatMap => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                let mut flattened = Vec::new();
                for value in values {
                    let mapped = self.call_closure_one(unit, &mapper, value, next_base)?;
                    let mapped = expect_list_ref(&mapped)?;
                    flattened.extend(mapped.borrow().iter().cloned());
                }
                Ok(VmValue::List(Rc::new(RefCell::new(flattened))))
            }
            RegIntrinsic::ListFlatten => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut flattened = Vec::new();
                for value in list.borrow().iter() {
                    let nested = expect_list_ref(value)?;
                    flattened.extend(nested.borrow().iter().cloned());
                }
                Ok(VmValue::List(Rc::new(RefCell::new(flattened))))
            }
            RegIntrinsic::ListGroupBy => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key_fn = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                let mut groups: ValueMap = ValueMap::default();
                for value in values {
                    let key_value =
                        self.call_closure_one(unit, &key_fn, value.clone(), next_base)?;
                    let key = map_key_from_value(&key_value)?;
                    match groups.get(&key) {
                        Some(VmValue::List(items)) => {
                            items.borrow_mut().push(value);
                        }
                        Some(other) => {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.group_by expected List group, got `{}`.",
                                other.display()
                            )));
                        }
                        None => {
                            groups.insert(key, VmValue::List(Rc::new(RefCell::new(vec![value]))));
                        }
                    }
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(groups))))
            }
            RegIntrinsic::ListIsEmpty => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(list.borrow().is_empty()))
            }
            RegIntrinsic::ListJoin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let separator =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(VmValue::string(join_string_values(
                    &list.borrow(),
                    &separator,
                )?))
            }
            RegIntrinsic::ListLast => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(list
                    .borrow()
                    .last()
                    .cloned()
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListDedup => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut values = Vec::new();
                for value in list.borrow().iter() {
                    if !values.contains(value) {
                        values.push(value.clone());
                    }
                }
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListEnumerate => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut values = Vec::new();
                for (index, value) in list.borrow().iter().enumerate() {
                    values.push(VmValue::List(Rc::new(RefCell::new(vec![
                        VmValue::Int(index as i64),
                        VmValue::Int(expect_int_ref(value)?),
                    ]))));
                }
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListMax => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let max = list
                    .borrow()
                    .iter()
                    .map(expect_int_ref)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .max();
                Ok(max
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Int(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListMin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let min = list
                    .borrow()
                    .iter()
                    .map(expect_int_ref)
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .min();
                Ok(min
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Int(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListNew => Ok(VmValue::List(Rc::new(RefCell::new(Vec::new())))),
            RegIntrinsic::ListPartition => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().clone();
                let mut matched = Vec::new();
                let mut unmatched = Vec::new();
                for value in values {
                    let keep = self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                    if expect_bool_ref(&keep)? {
                        matched.push(value);
                    } else {
                        unmatched.push(value);
                    }
                }
                Ok(VmValue::List(Rc::new(RefCell::new(vec![
                    VmValue::List(Rc::new(RefCell::new(matched))),
                    VmValue::List(Rc::new(RefCell::new(unmatched))),
                ]))))
            }
            RegIntrinsic::ListReverse => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut values = list.borrow().clone();
                values.reverse();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListSkip => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().iter().skip(count).cloned().collect();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListSlice => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = nonnegative_count(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let borrowed = list.borrow();
                if start >= borrowed.len() {
                    return Ok(VmValue::List(Rc::new(RefCell::new(Vec::new()))));
                }
                let end = start.saturating_add(len).min(borrowed.len());
                Ok(VmValue::List(Rc::new(RefCell::new(
                    borrowed[start..end].to_vec(),
                ))))
            }
            RegIntrinsic::ListSum => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let total = list
                    .borrow()
                    .iter()
                    .map(expect_int_ref)
                    .try_fold(0_i64, |total, value| value.map(|value| total + value))?;
                Ok(VmValue::Int(total))
            }
            RegIntrinsic::ListZip => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let left = left.borrow();
                let right = right.borrow();
                let values = left
                    .iter()
                    .zip(right.iter())
                    .map(|(left, right)| {
                        VmValue::List(Rc::new(RefCell::new(vec![left.clone(), right.clone()])))
                    })
                    .collect();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListTryFold => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let values = list.borrow().clone();
                for value in values {
                    let folded = self.call_closure_two(unit, &folder, state, value, next_base)?;
                    match result_variant_payload(&folded)? {
                        Ok(value) => state = value,
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(state))
            }
            RegIntrinsic::ListTake => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().iter().take(count).cloned().collect();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListToJsonStrings => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = list
                    .borrow()
                    .iter()
                    .map(|value| expect_string_ref(value).map(|value| value.to_string()))
                    .map(|value| value.map(serde_json::Value::String))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(values))))
            }
            RegIntrinsic::ListToJsonValues => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = list
                    .borrow()
                    .iter()
                    .map(|value| expect_json_ref(value).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(values))))
            }
            other => unreachable!(
                "exec_list_intrinsics called with non-list intrinsic: {other:?}"
            ),
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn exec_map_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::MapContainsKey => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(map.borrow().contains_key(&key)))
            }
            RegIntrinsic::MapFilter => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                let mut filtered = ValueMap::default();
                for (key, value) in entries {
                    let keep = self.call_closure_two(
                        unit,
                        &predicate,
                        vm_value_from_map_key(&key),
                        value.clone(),
                        next_base,
                    )?;
                    if expect_bool_ref(&keep)? {
                        filtered.insert(key, value);
                    }
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(filtered))))
            }
            RegIntrinsic::MapFold => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, value) in entries {
                    state = self.call_closure_three(
                        unit,
                        &folder,
                        state,
                        vm_value_from_map_key(&key),
                        value,
                        next_base,
                    )?;
                }
                Ok(state)
            }
            RegIntrinsic::MapForEach => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let callback = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, value) in entries {
                    let _ = self.call_closure_two(
                        unit,
                        &callback,
                        vm_value_from_map_key(&key),
                        value,
                        next_base,
                    )?;
                }
                Ok(VmValue::Unit)
            }
            RegIntrinsic::MapGetOrDefault => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let default = intrinsic_arg(&self.stack, base, args, 2)?.clone();
                Ok(map.borrow().get(&key).cloned().unwrap_or(default))
            }
            RegIntrinsic::MapIsEmpty => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(map.borrow().is_empty()))
            }
            RegIntrinsic::MapKeys => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let keys = map
                    .borrow()
                    .keys()
                    .map(vm_value_from_map_key)
                    .collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(keys))))
            }
            RegIntrinsic::MapLen => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(map.borrow().len() as i64))
            }
            RegIntrinsic::MapMapValues => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                let mut mapped = ValueMap::default();
                for (key, value) in entries {
                    mapped.insert(key, self.call_closure_one(unit, &mapper, value, next_base)?);
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(mapped))))
            }
            RegIntrinsic::MapMerge => {
                let left = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_map_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let resolver = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut merged = left.borrow().clone();
                let right_entries = right
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, right_value) in right_entries {
                    if let Some(left_value) = merged.get(&key).cloned() {
                        let resolved = self.call_closure_two(
                            unit,
                            &resolver,
                            left_value,
                            right_value,
                            next_base,
                        )?;
                        merged.insert(key, resolved);
                    } else {
                        merged.insert(key, right_value);
                    }
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(merged))))
            }
            RegIntrinsic::MapNew => Ok(VmValue::Map(Rc::new(RefCell::new(ValueMap::default())))),
            RegIntrinsic::MapTryFold => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, value) in entries {
                    let folded = self.call_closure_three(
                        unit,
                        &folder,
                        state,
                        vm_value_from_map_key(&key),
                        value,
                        next_base,
                    )?;
                    match result_variant_payload(&folded)? {
                        Ok(value) => state = value,
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(state))
            }
            RegIntrinsic::MapValues => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = map.borrow().values().cloned().collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            other => unreachable!(
                "exec_map_intrinsics called with non-map intrinsic: {other:?}"
            ),
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn exec_bytes_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        let _ = unit;
        match intrinsic {
            RegIntrinsic::BytesConcat => {
                let left = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut bytes = Vec::with_capacity(left.len() + right.len());
                bytes.extend_from_slice(left);
                bytes.extend_from_slice(right);
                Ok(VmValue::Bytes(Rc::new(bytes)))
            }
            RegIntrinsic::BytesConsume => {
                expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::BytesFromString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(value.as_bytes().to_vec())))
            }
            RegIntrinsic::BytesFromUints => {
                let values = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bytes = values
                    .borrow()
                    .iter()
                    .map(|value| expect_int_ref(value).map(|v| v as u8))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Bytes(Rc::new(bytes)))
            }
            RegIntrinsic::BytesIsEmpty => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_empty()))
            }
            RegIntrinsic::BytesLen => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value.len() as i64))
            }
            RegIntrinsic::BytesSlice => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::Bytes(Rc::new(bytes_slice(value, start, len))))
            }
            RegIntrinsic::BytesToString => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(String::from_utf8_lossy(value)))
            }
            RegIntrinsic::BytesToUints => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::List(Rc::new(RefCell::new(
                    value
                        .iter()
                        .map(|byte| VmValue::Int(i64::from(*byte)))
                        .collect(),
                ))))
            }
            RegIntrinsic::BytesViewStartsWith => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.starts_with(prefix)))
            }
            RegIntrinsic::BytesViewToBytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(value.to_vec())))
            }
            other => unreachable!(
                "exec_bytes_intrinsics called with non-bytes intrinsic: {other:?}"
            ),
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn exec_date_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        let _ = unit;
        match intrinsic {
            RegIntrinsic::DateAddDays => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let days = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(
                    unix_ms.saturating_add(days.saturating_mul(MS_PER_DAY)),
                ))
            }
            RegIntrinsic::DateAddMs => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(unix_ms.saturating_add(ms)))
            }
            RegIntrinsic::DateDay => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).day() as i64))
            }
            RegIntrinsic::DateDaysBetween => {
                let start_unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let end_unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(
                    end_unix_ms.saturating_sub(start_unix_ms) / MS_PER_DAY,
                ))
            }
            RegIntrinsic::DateDaysInMonth => {
                let year = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let month = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(date_days_in_month(year, month)))
            }
            RegIntrinsic::DateFormatIso => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    utc_datetime(unix_ms).to_rfc3339_opts(SecondsFormat::Millis, true),
                ))
            }
            RegIntrinsic::DateFormatYmd => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    utc_datetime(unix_ms).format("%Y-%m-%d").to_string(),
                ))
            }
            RegIntrinsic::DateHour => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).hour() as i64))
            }
            RegIntrinsic::DateIsLeapYear => {
                let year = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(date_is_leap_year(year)))
            }
            RegIntrinsic::DateMinute => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).minute() as i64))
            }
            RegIntrinsic::DateMonth => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).month() as i64))
            }
            RegIntrinsic::DateParseIso => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(date_parse_iso(value)
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Int(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::DateParseYmd => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(date_parse_ymd(value)
                    .map(|value| VmValue::OptionSome(Box::new(VmValue::Int(value))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::DateSecond => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).second() as i64))
            }
            RegIntrinsic::DateStartOfDay => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = utc_datetime(unix_ms)
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is valid");
                Ok(VmValue::Int(
                    Utc.from_utc_datetime(&start).timestamp_millis(),
                ))
            }
            RegIntrinsic::DateWeekday => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(
                    utc_datetime(unix_ms).weekday().number_from_monday() as i64,
                ))
            }
            RegIntrinsic::DateYear => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).year() as i64))
            }
            other => unreachable!(
                "exec_date_intrinsics called with non-date intrinsic: {other:?}"
            ),
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn exec_math_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        let _ = unit;
        match intrinsic {
            RegIntrinsic::MathAbs => Ok(VmValue::Int(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.abs(),
            )),
            RegIntrinsic::MathAbsFloat => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.abs(),
            )),
            RegIntrinsic::MathCeil => Ok(VmValue::Int(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.ceil() as i64,
            )),
            RegIntrinsic::MathClamp => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let min = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let max = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                if min > max {
                    return Err(EvalError::Runtime(format!(
                        "Math.clamp requires min <= max, got min {min} and max {max}"
                    )));
                }
                Ok(VmValue::Int(value.clamp(min, max)))
            }
            RegIntrinsic::MathClampFloat => {
                let value = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let min = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let max = expect_float_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                if min > max {
                    return Err(EvalError::Runtime(format!(
                        "Math.clamp_float requires min <= max, got min {min} and max {max}"
                    )));
                }
                Ok(VmValue::Float(value.clamp(min, max)))
            }
            RegIntrinsic::MathCos => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.cos(),
            )),
            RegIntrinsic::MathExp => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.exp(),
            )),
            RegIntrinsic::MathExp2 => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.exp2(),
            )),
            RegIntrinsic::MathFloor => Ok(VmValue::Int(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.floor() as i64,
            )),
            RegIntrinsic::MathLog => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.ln(),
            )),
            RegIntrinsic::MathLog2 => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.log2(),
            )),
            RegIntrinsic::MathMax => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.max(right)))
            }
            RegIntrinsic::MathMaxFloat => {
                let left = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Float(left.max(right)))
            }
            RegIntrinsic::MathMin => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.min(right)))
            }
            RegIntrinsic::MathMinFloat => {
                let left = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Float(left.min(right)))
            }
            RegIntrinsic::MathPow => {
                let base_value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let exponent = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                base_value
                    .checked_pow(exponent.max(0) as u32)
                    .map(VmValue::Int)
                    .ok_or_else(|| {
                        EvalError::Runtime(format!(
                            "Math.pow overflow: {base_value} raised to {exponent} exceeds the Int range"
                        ))
                    })
            }
            RegIntrinsic::MathPowFloat => {
                let base_value = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let exponent = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Float(base_value.powf(exponent)))
            }
            RegIntrinsic::MathRound => Ok(VmValue::Int(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.round() as i64,
            )),
            RegIntrinsic::MathSin => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.sin(),
            )),
            RegIntrinsic::MathSqrt => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.sqrt(),
            )),
            RegIntrinsic::MathTanh => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.tanh(),
            )),
            RegIntrinsic::MathTruncFloat => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.trunc(),
            )),
            other => unreachable!(
                "exec_math_intrinsics called with non-math intrinsic: {other:?}"
            ),
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn exec_char_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        let _ = unit;
        match intrinsic {
            RegIntrinsic::CharCompare => {
                let left = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_char_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let value = match left.cmp(&right) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                };
                Ok(VmValue::Int(value))
            }
            RegIntrinsic::CharFromCode => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(u32::try_from(value)
                    .ok()
                    .and_then(char::from_u32)
                    .map(VmValue::Char)
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::CharIsAlphanumeric => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_ascii_alphanumeric()))
            }
            RegIntrinsic::CharIsAlpha => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_ascii_alphabetic()))
            }
            RegIntrinsic::CharIsDigit => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_ascii_digit()))
            }
            RegIntrinsic::CharIsLower => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_lowercase()))
            }
            RegIntrinsic::CharIsUpper => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_uppercase()))
            }
            RegIntrinsic::CharIsWhitespace => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_whitespace()))
            }
            RegIntrinsic::CharToCode => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value as u32 as i64))
            }
            RegIntrinsic::CharToLower => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Char(value.to_lowercase().next().unwrap_or(value)))
            }
            RegIntrinsic::CharToString => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::CharToUpper => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Char(value.to_uppercase().next().unwrap_or(value)))
            }
            other => unreachable!(
                "exec_char_intrinsics called with non-char intrinsic: {other:?}"
            ),
        }
    }

    #[allow(clippy::mutable_key_type)]
    fn exec_path_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        let _ = unit;
        match intrinsic {
            RegIntrinsic::PathExists => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).exists()))
            }
            RegIntrinsic::PathExtension => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(Path::new(path)
                    .extension()
                    .map(|extension| {
                        VmValue::OptionSome(Box::new(VmValue::string(extension.to_string_lossy())))
                    })
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::PathFileName => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(Path::new(path)
                    .file_name()
                    .map(|name| {
                        VmValue::OptionSome(Box::new(VmValue::string(name.to_string_lossy())))
                    })
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::PathFromString | RegIntrinsic::PathToString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value))
            }
            RegIntrinsic::PathIsAbsolute => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_absolute()))
            }
            RegIntrinsic::PathIsDir => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_dir()))
            }
            RegIntrinsic::PathIsFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_file()))
            }
            RegIntrinsic::PathJoin => {
                let base_path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let child = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(path_join_string(base_path, child)))
            }
            RegIntrinsic::PathListFiles => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    directory_list_files(Path::new(path))
                        .map(|files| {
                            VmValue::List(Rc::new(RefCell::new(
                                files.into_iter().map(VmValue::string).collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::PathListPaths => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    directory_list_paths(Path::new(path))
                        .map(|paths| {
                            VmValue::List(Rc::new(RefCell::new(
                                paths
                                    .into_iter()
                                    .map(|path| VmValue::string(path.to_string_lossy()))
                                    .collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::PathNormalize => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(path_normalize_string(path)))
            }
            RegIntrinsic::PathParent => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(Path::new(path)
                    .parent()
                    .map(|parent| {
                        VmValue::OptionSome(Box::new(VmValue::string(parent.to_string_lossy())))
                    })
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::PathReadString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    std::fs::read_to_string(path)
                        .map(VmValue::string)
                        .map_err(|error| file_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::PathResolveRelative => {
                let root = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let relative = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(json_result(
                    path_resolve_relative_string(root, relative)
                        .map(VmValue::string)
                        .map_err(VmValue::string),
                ))
            }
            RegIntrinsic::PathSafeRelative => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    path_safe_relative_string(value)
                        .map(VmValue::string)
                        .map_err(VmValue::string),
                ))
            }
            RegIntrinsic::PathStartsWith => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let base_path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(
                    Path::new(path).starts_with(Path::new(base_path)),
                ))
            }
            RegIntrinsic::PathWithExtension => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let extension = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut path = PathBuf::from(path);
                path.set_extension(extension);
                Ok(VmValue::string(path.to_string_lossy()))
            }
            RegIntrinsic::PathWriteString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::write(path, text)))
            }
            other => unreachable!(
                "exec_path_intrinsics called with non-path intrinsic: {other:?}"
            ),
        }
    }

    fn exec_option_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        match intrinsic {
            RegIntrinsic::OptionAndThen => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSome(value) => ensure_option_value(self.call_closure_one(
                        unit,
                        &mapper,
                        (**value).clone(),
                        next_base,
                    )?),
                    VmValue::OptionNone => Ok(VmValue::OptionNone),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.and_then expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionFilter => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSome(value) => {
                        let value = (**value).clone();
                        let keep =
                            self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                        if expect_bool_ref(&keep)? {
                            Ok(VmValue::OptionSome(Box::new(value)))
                        } else {
                            Ok(VmValue::OptionNone)
                        }
                    }
                    VmValue::OptionNone => Ok(VmValue::OptionNone),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.filter expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionIsNone => Ok(VmValue::Bool(matches!(
                intrinsic_arg(&self.stack, base, args, 0)?,
                VmValue::OptionNone
            ))),
            RegIntrinsic::OptionIsSome => Ok(VmValue::Bool(matches!(
                intrinsic_arg(&self.stack, base, args, 0)?,
                VmValue::OptionSome(_)
            ))),
            RegIntrinsic::OptionMap => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSome(value) => Ok(VmValue::OptionSome(Box::new(
                        self.call_closure_one(unit, &mapper, (**value).clone(), next_base)?,
                    ))),
                    VmValue::OptionNone => Ok(VmValue::OptionNone),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.map expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionOkOr => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let error = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                match option {
                    VmValue::OptionSome(value) => Ok(value_ok((**value).clone())),
                    VmValue::OptionNone => Ok(value_err(error)),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.ok_or expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionOr => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let fallback = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                match option {
                    VmValue::OptionSome(_) => Ok(option.clone()),
                    VmValue::OptionNone => Ok(fallback),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.or expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionUnwrapOr => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let default = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                match option {
                    VmValue::OptionSome(value) => Ok((**value).clone()),
                    VmValue::OptionNone => Ok(default),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.unwrap_or expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionUnwrapOrElse => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let fallback = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSome(value) => Ok((**value).clone()),
                    VmValue::OptionNone => self.call_closure_zero(unit, &fallback, next_base),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.unwrap_or_else expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            other => unreachable!(
                "exec_option_intrinsics called with non-option intrinsic: {other:?}"
            ),
        }
    }

    fn exec_result_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        match intrinsic {
            RegIntrinsic::ResultErr => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match result {
                    Ok(_) => VmValue::OptionNone,
                    Err(error) => VmValue::OptionSome(Box::new(error)),
                })
            }
            RegIntrinsic::ResultErrMessage => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match result {
                    Ok(_) => VmValue::OptionNone,
                    Err(error) => VmValue::OptionSome(Box::new(VmValue::string(error.display()))),
                })
            }
            RegIntrinsic::ResultIsErr => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(result.is_err()))
            }
            RegIntrinsic::ResultIsOk => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(result.is_ok()))
            }
            RegIntrinsic::ResultOk => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match result {
                    Ok(value) => VmValue::OptionSome(Box::new(value)),
                    Err(_) => VmValue::OptionNone,
                })
            }
            RegIntrinsic::ResultAndThen => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => {
                        let mapped = self.call_closure_one(unit, &mapper, value, next_base)?;
                        let _ = result_variant_payload(&mapped)?;
                        Ok(mapped)
                    }
                    Err(error) => Ok(value_err(error)),
                }
            }
            RegIntrinsic::ResultMap => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => Ok(value_ok(
                        self.call_closure_one(unit, &mapper, value, next_base)?,
                    )),
                    Err(error) => Ok(value_err(error)),
                }
            }
            RegIntrinsic::ResultMapError => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => Ok(value_ok(value)),
                    Err(error) => Ok(value_err(
                        self.call_closure_one(unit, &mapper, error, next_base)?,
                    )),
                }
            }
            RegIntrinsic::ResultUnwrapOr => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let default = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                Ok(match result {
                    Ok(value) => value,
                    Err(_) => default,
                })
            }
            RegIntrinsic::ResultUnwrapOrElse => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let fallback = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => Ok(value),
                    Err(error) => self.call_closure_one(unit, &fallback, error, next_base),
                }
            }
            other => unreachable!(
                "exec_result_intrinsics called with non-result intrinsic: {other:?}"
            ),
        }
    }

    fn exec_set_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = unit;
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::SetContains => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(set.borrow().iter().any(|item| item == value)))
            }
            RegIntrinsic::SetDifference => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let right = right.borrow().clone();
                let values = left
                    .borrow()
                    .iter()
                    .filter(|value| !right.iter().any(|item| item == *value))
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::SetIntersection => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let right = right.borrow().clone();
                let values = left
                    .borrow()
                    .iter()
                    .filter(|value| right.iter().any(|item| item == *value))
                    .cloned()
                    .collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::SetIsEmpty => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(set.borrow().is_empty()))
            }
            RegIntrinsic::SetIsSubset => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let right = right.borrow().clone();
                Ok(VmValue::Bool(
                    left.borrow()
                        .iter()
                        .all(|value| right.iter().any(|item| item == value)),
                ))
            }
            RegIntrinsic::SetLen => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(set.borrow().len() as i64))
            }
            RegIntrinsic::SetNew => Ok(VmValue::List(Rc::new(RefCell::new(Vec::new())))),
            RegIntrinsic::SetToList => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::List(Rc::new(RefCell::new(set.borrow().clone()))))
            }
            RegIntrinsic::SetUnion => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut values = left.borrow().clone();
                for value in right.borrow().iter().cloned() {
                    set_insert_vm(&mut values, value);
                }
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::SortedSetContains => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(sorted_contains_vm(&set.borrow(), value)?))
            }
            RegIntrinsic::SortedSetIsEmpty => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(set.borrow().is_empty()))
            }
            RegIntrinsic::SortedSetLen => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(set.borrow().len() as i64))
            }
            RegIntrinsic::SortedSetNew => Ok(VmValue::List(Rc::new(RefCell::new(Vec::new())))),
            RegIntrinsic::SortedSetToList => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::List(Rc::new(RefCell::new(set.borrow().clone()))))
            }
            RegIntrinsic::SortedMapContainsKey => {
                let map = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(
                    sorted_map_get_in_place(&map.borrow(), key)?.is_some(),
                ))
            }
            RegIntrinsic::SortedMapGet => {
                let map = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(sorted_map_get_in_place(&map.borrow(), key)?
                    .map(|value| VmValue::OptionSome(Box::new(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::SortedMapIsEmpty => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(entries.is_empty()))
            }
            RegIntrinsic::SortedMapKeys => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let keys = entries.into_iter().map(|(key, _)| key).collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(keys))))
            }
            RegIntrinsic::SortedMapLen => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(entries.len() as i64))
            }
            RegIntrinsic::SortedMapNew => Ok(sorted_map_value(Vec::new())),
            RegIntrinsic::SortedMapValues => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = entries
                    .into_iter()
                    .map(|(_, value)| value)
                    .collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            other => unreachable!(
                "exec_set_intrinsics called with non-set intrinsic: {other:?}"
            ),
        }
    }

    fn exec_deque_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = unit;
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::DequeIsEmpty => {
                let deque = expect_deque_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(deque.borrow().is_empty()))
            }
            RegIntrinsic::DequeLen => {
                let deque = expect_deque_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(deque.borrow().len() as i64))
            }
            RegIntrinsic::DequeNew => Ok(VmValue::Deque(Rc::new(RefCell::new(
                std::collections::VecDeque::new(),
            )))),
            RegIntrinsic::DequeToList => {
                let deque = expect_deque_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let list = deque.borrow().iter().cloned().collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(list))))
            }
            other => unreachable!(
                "exec_deque_intrinsics called with non-deque intrinsic: {other:?}"
            ),
        }
    }

    fn exec_regex_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = unit;
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::RegexCaptures => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let captures = regex
                    .captures(value)
                    .map(|captures| {
                        captures
                            .iter()
                            .filter_map(|matched| {
                                matched.map(|matched| VmValue::string(matched.as_str()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Ok(VmValue::List(Rc::new(RefCell::new(captures))))
            }
            RegIntrinsic::RegexCompile => {
                let pattern = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match regex::Regex::new(pattern) {
                    Ok(_) => value_ok(regex_value(pattern)),
                    Err(error) => value_err(regex_error_value(error.to_string())),
                })
            }
            RegIntrinsic::RegexErrorMessage => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "message")
            }
            RegIntrinsic::RegexFind => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(regex
                    .find(value)
                    .map(|matched| VmValue::OptionSome(Box::new(VmValue::string(matched.as_str()))))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::RegexIsMatch => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(regex.is_match(value)))
            }
            RegIntrinsic::RegexReplaceAll => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let replacement = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(
                    regex.replace_all(value, replacement).to_string(),
                ))
            }
            RegIntrinsic::RegexSplit => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let parts = regex.split(value).map(VmValue::string).collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(parts))))
            }
            other => unreachable!(
                "exec_regex_intrinsics called with non-regex intrinsic: {other:?}"
            ),
        }
    }

    fn exec_hex_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = unit;
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::HexDecode => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    hex::decode(text)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| decode_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::HexEncode => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(hex::encode(value)))
            }
            RegIntrinsic::HexEncodeString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(hex::encode(value.as_bytes())))
            }
            other => unreachable!(
                "exec_hex_intrinsics called with non-hex intrinsic: {other:?}"
            ),
        }
    }

    fn exec_url_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = unit;
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::UrlDecodeComponent => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(
                    percent_decode_str(value)
                        .decode_utf8()
                        .map(|value| VmValue::string(value.to_string()))
                        .map_err(|error| decode_error_value(error.to_string())),
                ))
            }
            RegIntrinsic::UrlEncodeComponent => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(
                    utf8_percent_encode(value, URL_COMPONENT_SET).to_string(),
                ))
            }
            RegIntrinsic::UrlFromString | RegIntrinsic::UrlToString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value))
            }
            other => unreachable!(
                "exec_url_intrinsics called with non-url intrinsic: {other:?}"
            ),
        }
    }

    fn exec_scalar_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = unit;
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::IntBitAnd => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left & right))
            }
            RegIntrinsic::IntBitNot => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(!value))
            }
            RegIntrinsic::IntBitOr => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left | right))
            }
            RegIntrinsic::IntBitXor => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left ^ right))
            }
            RegIntrinsic::IntShiftLeft => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bits = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(value.wrapping_shl(bits.max(0) as u32)))
            }
            RegIntrinsic::IntShiftRight => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bits = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(value.wrapping_shr(bits.max(0) as u32)))
            }
            RegIntrinsic::IntToString => Ok(VmValue::string(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
            RegIntrinsic::IntToFloat => {
                Ok(VmValue::Float(
                    expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)? as f64,
                ))
            }
            RegIntrinsic::FloatToString => Ok(VmValue::string(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
            RegIntrinsic::FloatIsFinite => Ok(VmValue::Bool(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.is_finite(),
            )),
            RegIntrinsic::FloatIsInfinite => Ok(VmValue::Bool(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.is_infinite(),
            )),
            RegIntrinsic::FloatIsNan => Ok(VmValue::Bool(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.is_nan(),
            )),
            other => unreachable!(
                "exec_scalar_intrinsics called with non-scalar intrinsic: {other:?}"
            ),
        }
    }
}

fn intrinsic_arg<'a>(
    stack: &'a [VmValue],
    base: usize,
    args: &[Reg],
    index: usize,
) -> Result<&'a VmValue, EvalError> {
    args.get(index)
        .and_then(|reg| stack.get(base + *reg))
        .ok_or_else(|| EvalError::Runtime(format!("reg VM missing argument {index}.")))
}

fn int_compare_op(op: BinaryOp) -> Option<RegIntCompare> {
    match op {
        BinaryOp::Less => Some(RegIntCompare::Less),
        BinaryOp::LessEqual => Some(RegIntCompare::LessEqual),
        BinaryOp::Greater => Some(RegIntCompare::Greater),
        BinaryOp::GreaterEqual => Some(RegIntCompare::GreaterEqual),
        _ => None,
    }
}

fn eval_int_compare(op: RegIntCompare, lhs: i64, rhs: i64) -> bool {
    match op {
        RegIntCompare::Less => lhs < rhs,
        RegIntCompare::LessEqual => lhs <= rhs,
        RegIntCompare::Greater => lhs > rhs,
        RegIntCompare::GreaterEqual => lhs >= rhs,
    }
}

fn int_overflow_error(operation: &str, lhs: i64, rhs: i64) -> EvalError {
    EvalError::Runtime(format!(
        "integer {operation} overflow: {lhs} and {rhs} exceed the Int range"
    ))
}

/// A folder closure recognized as a single numeric binary op over its two
/// parameters: `op` is the arithmetic operator and `lhs_is_state` says whether
/// the op's left operand is the accumulator (param 0) — i.e. `acc <op> x` — or
/// the element (`x <op> acc`). Used to fast-path `List.fold` without losing the
/// closure's exact operand order.
struct NumericBinaryClosure {
    op: BinaryOp,
    lhs_is_state: bool,
}

/// Recognize `|state, x| state <op> x` (or `x <op> state`) closures with no
/// captures whose body is exactly one arithmetic instruction returning its
/// result. Returns `None` for anything else so the caller falls back to the
/// generic interpreter (the recognizer is intentionally conservative — a missed
/// match only forgoes a speedup, never changes results).
fn recognize_numeric_binary_closure(
    unit: &RegUnit,
    closure: &VmClosure,
) -> Option<NumericBinaryClosure> {
    // The fast path supplies the two operands by value at the param registers;
    // captured values would not be supplied, so require a capture-free closure.
    if !closure.captures.is_empty() {
        return None;
    }
    let function = &unit.functions[closure.function];
    if function.params != 2 {
        return None;
    }
    // Param registers: captures occupy `0..captures.len()` (none here), then the
    // two params at registers 0 and 1.
    let state_reg = 0usize;
    let item_reg = 1usize;
    let [instr, RegInstr::Return { src }] = function.code.as_slice() else {
        return None;
    };
    let (op, dst, lhs, rhs) = arithmetic_binop_parts(instr)?;
    if dst != *src {
        return None;
    }
    // Both operands must be exactly the two distinct param registers (so every
    // input is a supplied param and nothing else is read).
    let lhs_is_state = if lhs == state_reg && rhs == item_reg {
        true
    } else if lhs == item_reg && rhs == state_reg {
        false
    } else {
        return None;
    };
    Some(NumericBinaryClosure { op, lhs_is_state })
}

/// If `instr` is one of the numeric arithmetic instructions handled by
/// [`eval_numeric_binary`], return `(op, dst, lhs, rhs)`. Bitwise/shift ops are
/// excluded: they are `Int`-only and not routed through `eval_numeric_binary`.
fn arithmetic_binop_parts(instr: &RegInstr) -> Option<(BinaryOp, Reg, Reg, Reg)> {
    match instr {
        RegInstr::AddInt { dst, lhs, rhs } => Some((BinaryOp::Add, *dst, *lhs, *rhs)),
        RegInstr::SubInt { dst, lhs, rhs } => Some((BinaryOp::Subtract, *dst, *lhs, *rhs)),
        RegInstr::MulInt { dst, lhs, rhs } => Some((BinaryOp::Multiply, *dst, *lhs, *rhs)),
        RegInstr::DivInt { dst, lhs, rhs } => Some((BinaryOp::Divide, *dst, *lhs, *rhs)),
        RegInstr::ModInt { dst, lhs, rhs } => Some((BinaryOp::Modulo, *dst, *lhs, *rhs)),
        _ => None,
    }
}

fn eval_numeric_binary(op: BinaryOp, lhs: &VmValue, rhs: &VmValue) -> Result<VmValue, EvalError> {
    match (lhs, rhs) {
        (VmValue::Int(lhs), VmValue::Int(rhs)) => match op {
            // Integer arithmetic traps (overflow, divide/modulo by zero) are
            // language-level runtime errors, never host panics, so the VM exits
            // through `EvalError` exactly like the Rust backend's checked ops.
            BinaryOp::Add => lhs
                .checked_add(*rhs)
                .map(VmValue::Int)
                .ok_or_else(|| int_overflow_error("addition", *lhs, *rhs)),
            BinaryOp::Subtract => lhs
                .checked_sub(*rhs)
                .map(VmValue::Int)
                .ok_or_else(|| int_overflow_error("subtraction", *lhs, *rhs)),
            BinaryOp::Multiply => lhs
                .checked_mul(*rhs)
                .map(VmValue::Int)
                .ok_or_else(|| int_overflow_error("multiplication", *lhs, *rhs)),
            BinaryOp::Divide => {
                if *rhs == 0 {
                    return Err(EvalError::Runtime("integer division by zero".to_string()));
                }
                lhs.checked_div(*rhs)
                    .map(VmValue::Int)
                    .ok_or_else(|| int_overflow_error("division", *lhs, *rhs))
            }
            BinaryOp::Modulo => {
                if *rhs == 0 {
                    return Err(EvalError::Runtime("integer modulo by zero".to_string()));
                }
                lhs.checked_rem(*rhs)
                    .map(VmValue::Int)
                    .ok_or_else(|| int_overflow_error("modulo", *lhs, *rhs))
            }
            _ => unreachable!("numeric binary helper called with non-arithmetic op"),
        },
        (VmValue::Float(lhs), VmValue::Float(rhs)) => match op {
            BinaryOp::Add => Ok(VmValue::Float(lhs + rhs)),
            BinaryOp::Subtract => Ok(VmValue::Float(lhs - rhs)),
            BinaryOp::Multiply => Ok(VmValue::Float(lhs * rhs)),
            BinaryOp::Divide => Ok(VmValue::Float(lhs / rhs)),
            BinaryOp::Modulo => Err(EvalError::Runtime(
                "reg VM modulo expects Int operands.".to_string(),
            )),
            _ => unreachable!("numeric binary helper called with non-arithmetic op"),
        },
        _ => Err(EvalError::Runtime(format!(
            "reg VM numeric operator expected matching Int or Float operands, got `{}` and `{}`.",
            lhs.display(),
            rhs.display()
        ))),
    }
}

fn eval_numeric_compare(
    op: RegIntCompare,
    lhs: &VmValue,
    rhs: &VmValue,
) -> Result<bool, EvalError> {
    match (lhs, rhs) {
        (VmValue::Int(lhs), VmValue::Int(rhs)) => Ok(eval_int_compare(op, *lhs, *rhs)),
        (VmValue::Float(lhs), VmValue::Float(rhs)) => Ok(match op {
            RegIntCompare::Less => lhs < rhs,
            RegIntCompare::LessEqual => lhs <= rhs,
            RegIntCompare::Greater => lhs > rhs,
            RegIntCompare::GreaterEqual => lhs >= rhs,
        }),
        _ => Err(EvalError::Runtime(format!(
            "reg VM numeric comparison expected matching Int or Float operands, got `{}` and `{}`.",
            lhs.display(),
            rhs.display()
        ))),
    }
}

fn expect_int_ref(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Int(value) => Ok(*value),
        // `Managed` is transparent (see vm_value: display/native_value/equality
        // all unwrap it). A value retained into storage via `manage` and read
        // back arrives wrapped; see through it like the rest of the value model.
        VmValue::Managed(inner) => expect_int_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Int, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_float_ref(value: &VmValue) -> Result<f64, EvalError> {
    match value {
        VmValue::Float(value) => Ok(*value),
        VmValue::Managed(inner) => expect_float_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Float, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_char_ref(value: &VmValue) -> Result<char, EvalError> {
    match value {
        VmValue::Char(value) => Ok(*value),
        VmValue::Managed(inner) => expect_char_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Char, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_bytes_ref(value: &VmValue) -> Result<&[u8], EvalError> {
    match value {
        VmValue::Bytes(value) => Ok(value.as_slice()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Bytes, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_usize_ref(value: &VmValue) -> Result<usize, EvalError> {
    let value = expect_int_ref(value)?;
    usize::try_from(value).map_err(|_| {
        EvalError::Runtime(format!(
            "reg VM expected non-negative index, got `{value}`."
        ))
    })
}

fn nonnegative_count(value: &VmValue) -> Result<usize, EvalError> {
    Ok(expect_int_ref(value)?.max(0) as usize)
}

fn bytes_slice(value: &[u8], start: i64, len: i64) -> Vec<u8> {
    let start = start.max(0) as usize;
    if start >= value.len() {
        return Vec::new();
    }
    let len = len.max(0) as usize;
    let end = start.saturating_add(len).min(value.len());
    value[start..end].to_vec()
}

fn sha256_digest(value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    Digest::update(&mut hasher, value);
    format!("{:x}", hasher.finalize())
}

fn sha3_224_digest(value: &[u8]) -> Vec<u8> {
    let mut hasher = Sha3_224::new();
    Update::update(&mut hasher, value);
    hasher.finalize().to_vec()
}

fn sha3_256_digest(value: &[u8]) -> Vec<u8> {
    let mut hasher = Sha3_256::new();
    Update::update(&mut hasher, value);
    hasher.finalize().to_vec()
}

fn shake128_digest(value: &[u8], out_len: i64) -> Vec<u8> {
    let mut hasher = Shake128::default();
    Update::update(&mut hasher, value);
    let mut reader = hasher.finalize_xof();
    let mut out = vec![0u8; out_len.max(0) as usize];
    XofReader::read(&mut reader, &mut out);
    out
}

fn hmac_sha256_digest(key: &[u8], value: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    Mac::update(&mut mac, value);
    format!("{:x}", mac.finalize().into_bytes())
}

fn utc_datetime(unix_ms: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(unix_ms)
        .single()
        .unwrap_or_else(|| {
            Utc.timestamp_millis_opt(0)
                .single()
                .expect("epoch is valid")
        })
}

fn date_parse_ymd(value: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let datetime = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&datetime).timestamp_millis())
}

fn date_parse_iso(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc).timestamp_millis())
}

fn date_is_leap_year(year: i64) -> bool {
    let Ok(year) = i32::try_from(year) else {
        return false;
    };
    NaiveDate::from_ymd_opt(year, 2, 29).is_some()
}

fn date_days_in_month(year: i64, month: i64) -> i64 {
    let Ok(year) = i32::try_from(year) else {
        return 0;
    };
    let Ok(month) = u32::try_from(month) else {
        return 0;
    };
    let Some(first) = NaiveDate::from_ymd_opt(year, month, 1) else {
        return 0;
    };
    let Some(next_month) = (if month == 12 {
        year.checked_add(1)
            .and_then(|year| NaiveDate::from_ymd_opt(year, 1, 1))
    } else {
        month
            .checked_add(1)
            .and_then(|month| NaiveDate::from_ymd_opt(year, month, 1))
    }) else {
        return 0;
    };
    (next_month - first).num_days()
}

fn set_insert_vm(items: &mut Vec<VmValue>, value: VmValue) -> bool {
    if items.iter().any(|item| item == &value) {
        return false;
    }
    items.push(value);
    true
}

fn set_remove_vm(items: &mut Vec<VmValue>, value: &VmValue) -> bool {
    let Some(index) = items.iter().position(|item| item == value) else {
        return false;
    };
    items.remove(index);
    true
}

/// Insert `value` into an already-sorted `Vec` via binary search, keeping it
/// sorted (`Ok(false)` if an equal element is present). O(log n) search + O(n)
/// shift — no clone and no full re-sort, unlike rebuilding the whole backing.
fn sorted_insert_vm(items: &mut Vec<VmValue>, value: VmValue) -> Result<bool, EvalError> {
    vm_value_cmp(&value, &value)?; // reject non-orderable values (parity with re-sort)
    let mut lo = 0;
    let mut hi = items.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match vm_value_cmp(&items[mid], &value)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(false),
        }
    }
    items.insert(lo, value);
    Ok(true)
}

/// Remove `value` from an already-sorted `Vec` via binary search.
fn sorted_remove_vm(items: &mut Vec<VmValue>, value: &VmValue) -> Result<bool, EvalError> {
    let mut lo = 0;
    let mut hi = items.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match vm_value_cmp(&items[mid], value)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => {
                items.remove(mid);
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Binary-search a sorted `Vec` for `value` (O(log n)) — used by `SortedSet`'s
/// membership test in place of a linear scan.
fn sorted_contains_vm(items: &[VmValue], value: &VmValue) -> Result<bool, EvalError> {
    let mut lo = 0;
    let mut hi = items.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        match vm_value_cmp(&items[mid], value)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(true),
        }
    }
    Ok(false)
}

/// Binary-search a sorted-map backing by key and return the value if present —
/// O(log n), cloning only the matched value (not the whole backing).
fn sorted_map_get_in_place(
    backing: &[VmValue],
    key: &VmValue,
) -> Result<Option<VmValue>, EvalError> {
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let pair = expect_list_ref(&backing[mid])?;
        let pair = pair.borrow();
        let entry_key = pair
            .first()
            .ok_or_else(|| EvalError::Runtime("reg VM SortedMap entry missing key.".to_string()))?;
        match vm_value_cmp(entry_key, key)? {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => return Ok(pair.get(1).cloned()),
        }
    }
    Ok(None)
}

/// Binary-search a sorted-map backing (a `List` of `[key, value]` pair lists) by
/// key and insert/update in place — no clone, no full re-sort, no rebuild.
fn sorted_map_insert_in_place(
    backing: &mut Vec<VmValue>,
    key: VmValue,
    value: VmValue,
) -> Result<(), EvalError> {
    vm_value_cmp(&key, &key)?;
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let ordering = {
            let pair = expect_list_ref(&backing[mid])?;
            let pair = pair.borrow();
            let entry_key = pair.first().ok_or_else(|| {
                EvalError::Runtime("reg VM SortedMap entry missing key.".to_string())
            })?;
            vm_value_cmp(entry_key, &key)?
        };
        match ordering {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => {
                let pair = expect_list_ref(&backing[mid])?;
                let mut pair = pair.borrow_mut();
                if let Some(slot) = pair.get_mut(1) {
                    *slot = value;
                } else {
                    pair.push(value);
                }
                return Ok(());
            }
        }
    }
    backing.insert(lo, VmValue::List(Rc::new(RefCell::new(vec![key, value]))));
    Ok(())
}

/// Binary-search a sorted-map backing by key and remove the entry, returning its
/// value if present.
fn sorted_map_remove_in_place(
    backing: &mut Vec<VmValue>,
    key: &VmValue,
) -> Result<Option<VmValue>, EvalError> {
    let mut lo = 0;
    let mut hi = backing.len();
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let ordering = {
            let pair = expect_list_ref(&backing[mid])?;
            let pair = pair.borrow();
            let entry_key = pair.first().ok_or_else(|| {
                EvalError::Runtime("reg VM SortedMap entry missing key.".to_string())
            })?;
            vm_value_cmp(entry_key, key)?
        };
        match ordering {
            Ordering::Less => lo = mid + 1,
            Ordering::Greater => hi = mid,
            Ordering::Equal => {
                let removed = backing.remove(mid);
                let pair = expect_list_ref(&removed)?;
                let value = pair.borrow().get(1).cloned();
                return Ok(value);
            }
        }
    }
    Ok(None)
}

fn sort_vm_values(items: &mut [VmValue]) -> Result<(), EvalError> {
    for item in items.iter() {
        vm_value_cmp(item, item)?;
    }
    let mut sorted = items.to_vec();
    sorted.sort_by(|left, right| vm_value_cmp(left, right).unwrap_or(Ordering::Equal));
    for pair in sorted.windows(2) {
        vm_value_cmp(&pair[0], &pair[1])?;
    }
    items.clone_from_slice(&sorted);
    Ok(())
}

fn vm_value_cmp(left: &VmValue, right: &VmValue) -> Result<Ordering, EvalError> {
    match (left, right) {
        (VmValue::Unit, VmValue::Unit) => Ok(Ordering::Equal),
        (VmValue::Int(left), VmValue::Int(right)) => Ok(left.cmp(right)),
        (VmValue::Bool(left), VmValue::Bool(right)) => Ok(left.cmp(right)),
        (VmValue::String(left), VmValue::String(right)) => Ok(left.cmp(right)),
        (VmValue::Char(left), VmValue::Char(right)) => Ok(left.cmp(right)),
        _ => Err(EvalError::Runtime(format!(
            "SortedSet value is not orderable: `{}`.",
            left.display()
        ))),
    }
}

fn expect_sorted_map_entries(value: &VmValue) -> Result<Vec<(VmValue, VmValue)>, EvalError> {
    let entries = expect_list_ref(value)?;
    entries
        .borrow()
        .iter()
        .map(|entry| {
            let pair = expect_list_ref(entry)?;
            let pair = pair.borrow();
            let [key, value] = pair.as_slice() else {
                return Err(EvalError::Runtime(format!(
                    "reg VM expected SortedMap entry, got `{}`.",
                    entry.display()
                )));
            };
            Ok((key.clone(), value.clone()))
        })
        .collect()
}

fn sorted_map_value(entries: Vec<(VmValue, VmValue)>) -> VmValue {
    VmValue::List(Rc::new(RefCell::new(sorted_map_entry_values(entries))))
}

fn sorted_map_entry_values(entries: Vec<(VmValue, VmValue)>) -> Vec<VmValue> {
    entries
        .into_iter()
        .map(|(key, value)| VmValue::List(Rc::new(RefCell::new(vec![key, value]))))
        .collect()
}

fn sorted_map_get(entries: &[(VmValue, VmValue)], key: &VmValue) -> Option<VmValue> {
    entries
        .iter()
        .find_map(|(entry_key, value)| (entry_key == key).then(|| value.clone()))
}

fn sorted_map_insert(entries: &mut Vec<(VmValue, VmValue)>, key: VmValue, value: VmValue) {
    if let Some((_, existing)) = entries.iter_mut().find(|(entry_key, _)| entry_key == &key) {
        *existing = value;
        return;
    }
    entries.push((key, value));
}

fn sorted_map_remove(entries: &mut Vec<(VmValue, VmValue)>, key: &VmValue) -> Option<VmValue> {
    let index = entries.iter().position(|(entry_key, _)| entry_key == key)?;
    Some(entries.remove(index).1)
}

fn path_join_string(base: &str, child: &str) -> String {
    Path::new(base).join(child).to_string_lossy().to_string()
}

fn path_normalize_string(path: &str) -> String {
    let mut normalized = PathBuf::new();
    for component in Path::new(path).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized.to_string_lossy().to_string()
}

fn path_safe_relative_string(value: &str) -> Result<String, String> {
    let path = Path::new(value);
    if value.is_empty() {
        return Err("path must be non-empty".to_string());
    }
    if path.is_absolute() {
        return Err("absolute paths are not allowed".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                return Err("parent-directory traversal is not allowed".to_string());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err("absolute paths are not allowed".to_string());
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err("path must name a relative file or directory".to_string());
    }
    Ok(normalized.to_string_lossy().to_string())
}

fn path_resolve_relative_string(root: &str, relative: &str) -> Result<String, String> {
    let relative = path_safe_relative_string(relative)?;
    let resolved = path_normalize_string(&path_join_string(root, &relative));
    if !Path::new(&resolved).starts_with(Path::new(root)) {
        return Err("resolved path escapes the workspace root".to_string());
    }
    Ok(resolved)
}

fn join_string_values(values: &[VmValue], separator: &str) -> Result<String, EvalError> {
    Ok(values
        .iter()
        .map(|value| expect_string_ref(value).map(str::to_string))
        .collect::<Result<Vec<_>, _>>()?
        .join(separator))
}

fn expect_string_list_ref(value: &VmValue) -> Result<Vec<String>, EvalError> {
    let list = expect_list_ref(value)?;
    list.borrow()
        .iter()
        .map(|value| expect_string_ref(value).map(str::to_string))
        .collect()
}

fn expect_float_list_ref(value: &VmValue) -> Result<Vec<f64>, EvalError> {
    let list = expect_list_ref(value)?;
    list.borrow().iter().map(expect_float_ref).collect()
}

fn expect_int_list_ref(value: &VmValue) -> Result<Vec<i64>, EvalError> {
    let list = expect_list_ref(value)?;
    list.borrow().iter().map(expect_int_ref).collect()
}

fn expect_bool_ref(value: &VmValue) -> Result<bool, EvalError> {
    match value {
        VmValue::Bool(value) => Ok(*value),
        VmValue::Managed(inner) => expect_bool_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Bool, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_list_ref(value: &VmValue) -> Result<Rc<RefCell<Vec<VmValue>>>, EvalError> {
    match value {
        VmValue::List(value) => Ok(Rc::clone(value)),
        VmValue::Managed(inner) => expect_list_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected List, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_deque_ref(
    value: &VmValue,
) -> Result<Rc<RefCell<std::collections::VecDeque<VmValue>>>, EvalError> {
    match value {
        VmValue::Deque(value) => Ok(Rc::clone(value)),
        VmValue::Managed(inner) => expect_deque_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Deque, got `{}`.",
            other.display()
        ))),
    }
}

fn list_item_at(
    list: &Rc<RefCell<Vec<VmValue>>>,
    index: usize,
    operation: &str,
) -> Result<VmValue, EvalError> {
    let values = list.borrow();
    values.get(index).cloned().ok_or_else(|| {
        EvalError::Runtime(format!(
            "reg VM {operation} observed list length change at index {index}."
        ))
    })
}

fn expect_map_ref(value: &VmValue) -> Result<Rc<RefCell<ValueMap>>, EvalError> {
    match value {
        VmValue::Map(value) => Ok(Rc::clone(value)),
        VmValue::Managed(inner) => expect_map_ref(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Map, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_json_ref(value: &VmValue) -> Result<&serde_json::Value, EvalError> {
    match value {
        VmValue::Json(value) => Ok(value.as_ref()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected JsonValue, got `{}`.",
            other.display()
        ))),
    }
}

fn expect_closure_rc(value: &VmValue) -> Result<Rc<VmClosure>, EvalError> {
    match value {
        VmValue::Closure(value) => Ok(Rc::clone(value)),
        VmValue::Managed(inner) => expect_closure_rc(&inner.borrow()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Closure, got `{}`.",
            other.display()
        ))),
    }
}

fn ensure_option_value(value: VmValue) -> Result<VmValue, EvalError> {
    match value {
        VmValue::OptionSome(_) | VmValue::OptionNone => Ok(value),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Option, got `{}`.",
            other.display()
        ))),
    }
}

fn map_key_from_value(value: &VmValue) -> Result<VmMapKey, EvalError> {
    // The checker guarantees a key's static type is `Hashable`, so this is a
    // defensive gate: it accepts every shape RSScript admits as a key — scalars,
    // strings/bytes, the structural `List`/`Deque`/`Option` containers, and
    // `derives(Eq, Hash)` structs/sums (recursively) — and rejects only the
    // genuinely unhashable values (`Float`, `Map`, raw `Json`, closures) with a
    // clean runtime error rather than a host panic.
    fn is_hashable(value: &VmValue) -> bool {
        match value {
            VmValue::Unit
            | VmValue::Bool(_)
            | VmValue::Int(_)
            | VmValue::Char(_)
            | VmValue::String(_)
            | VmValue::Bytes(_)
            | VmValue::Native(_)
            | VmValue::OptionNone => true,
            VmValue::OptionSome(inner) => is_hashable(inner),
            VmValue::List(items) => items.borrow().iter().all(is_hashable),
            VmValue::Deque(items) => items.borrow().iter().all(is_hashable),
            VmValue::Struct(data) | VmValue::Variant(data) => data.fields.iter().all(is_hashable),
            VmValue::Managed(inner) => is_hashable(&inner.borrow()),
            VmValue::Float(_) | VmValue::Map(_) | VmValue::Json(_) | VmValue::Closure(_) => false,
        }
    }

    if is_hashable(value) {
        Ok(VmMapKey::new(value.clone()))
    } else {
        Err(EvalError::Runtime(format!(
            "reg VM Map key does not support `{}`.",
            value.display()
        )))
    }
}

fn vm_value_from_map_key(key: &VmMapKey) -> VmValue {
    key.value().clone()
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn json_quote_string(value: &str) -> Result<String, EvalError> {
    serde_json::to_string(value).map_err(|error| EvalError::Runtime(error.to_string()))
}

fn json_array_get_value(value: &serde_json::Value, index: i64) -> Result<VmValue, VmValue> {
    if index < 0 {
        return Err(json_error_value(format!(
            "JSON array index `{index}` is negative"
        )));
    }
    let serde_json::Value::Array(items) = value else {
        return Err(json_error_value("JSON value is not an array"));
    };
    items
        .get(index as usize)
        .cloned()
        .map(|value| VmValue::Json(Rc::new(value)))
        .ok_or_else(|| json_error_value(format!("JSON array index `{index}` is out of bounds")))
}

fn json_array_items(value: &serde_json::Value) -> Result<&Vec<serde_json::Value>, VmValue> {
    let serde_json::Value::Array(items) = value else {
        return Err(json_error_value("JSON value is not an array"));
    };
    Ok(items)
}

fn json_array_bools_value(value: &serde_json::Value) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    let mut flags = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(flag) = item.as_bool() else {
            return Err(json_error_value(format!(
                "JSON array item `{index}` is not a boolean"
            )));
        };
        flags.push(VmValue::Bool(flag));
    }
    Ok(VmValue::List(Rc::new(RefCell::new(flags))))
}

fn json_array_ints_value(value: &serde_json::Value) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    let mut numbers = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(number) = item.as_i64() else {
            return Err(json_error_value(format!(
                "JSON array item `{index}` is not an integer"
            )));
        };
        numbers.push(VmValue::Int(number));
    }
    Ok(VmValue::List(Rc::new(RefCell::new(numbers))))
}

fn json_array_strings_value(value: &serde_json::Value) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    let mut strings = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(text) = item.as_str() else {
            return Err(json_error_value(format!(
                "JSON array item `{index}` is not a string"
            )));
        };
        strings.push(VmValue::string(text));
    }
    Ok(VmValue::List(Rc::new(RefCell::new(strings))))
}

#[derive(Debug, Clone, Copy)]
enum JsonArrayStringMatch {
    Exact,
    Substring,
    Prefix,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonPathPart {
    Field(String),
    Index(usize),
}

#[derive(Debug, Clone)]
struct VmChannelState {
    id: i64,
    capacity: i64,
    receiver_taken: bool,
}

impl VmChannelState {
    fn to_value(&self) -> VmValue {
        channel_value(self.id, self.capacity, self.receiver_taken)
    }
}

fn channel_value(id: i64, capacity: i64, receiver_taken: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("id".to_string(), VmValue::Int(id)),
        ("capacity".to_string(), VmValue::Int(capacity)),
        ("receiver_taken".to_string(), VmValue::Bool(receiver_taken)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Channel"), fields)))
}

#[derive(Debug, Clone)]
struct VmSender {
    channel_id: i64,
    closed: bool,
}

/// A spawned-task handle (`Task<T>`), carried as a `Native` so it flows through
/// bindings/structs like any value; `await` recognises it via [`as_task_handle`].
fn task_handle_value(task: TaskId) -> VmValue {
    VmValue::Native(Rc::new(VmNative {
        type_name: Rc::from("Task"),
        id: task as i64,
    }))
}

fn as_task_handle(value: &VmValue) -> Option<TaskId> {
    match value {
        VmValue::Native(native) if native.type_name.as_ref() == "Task" => Some(native.id as TaskId),
        _ => None,
    }
}

/// Strip the `await`/`?`/effect wrappers off a `select` arm operation down to the
/// underlying call to spawn, reporting whether a `?` was present (so the arm body
/// can re-apply it to the winning result).
fn peel_select_operation(operation: &HirExpr) -> (&HirExpr, bool) {
    let mut current = operation;
    let mut has_try = false;
    loop {
        match current {
            HirExpr::Try { value, .. } => {
                has_try = true;
                current = value;
            }
            HirExpr::Await { value, .. } | HirExpr::Effect { value, .. } => {
                current = value;
            }
            other => return (other, has_try),
        }
    }
}

fn sender_value(channel_id: i64, closed: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("channel_id".to_string(), VmValue::Int(channel_id)),
        ("closed".to_string(), VmValue::Bool(closed)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Sender"), fields)))
}

#[derive(Debug, Clone)]
struct VmReceiver {
    channel_id: i64,
    closed: bool,
}

fn receiver_value(channel_id: i64, closed: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("channel_id".to_string(), VmValue::Int(channel_id)),
        ("closed".to_string(), VmValue::Bool(closed)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Receiver"), fields)))
}

#[derive(Debug, Clone)]
struct VmResourcePoolState {
    id: i64,
}

const POOL_LEASE_ID_FIELD: &str = "__rsscript_vm_pool_id";
const POOL_LEASE_DISCARDED_FIELD: &str = "__rsscript_vm_pool_discarded";

fn resource_pool_value(id: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("id".to_string(), VmValue::Int(id))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ResourcePool"),
        fields,
    )))
}

fn pool_stats_value(capacity: i64, created: i64, available: i64, in_use: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("capacity".to_string(), VmValue::Int(capacity)),
        ("created".to_string(), VmValue::Int(created)),
        ("available".to_string(), VmValue::Int(available)),
        ("in_use".to_string(), VmValue::Int(in_use)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("PoolStats"), fields)))
}

fn pool_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("PoolError"), fields)))
}

fn pool_error_message(value: &VmValue) -> Option<String> {
    match value {
        VmValue::Struct(data) if data.name.as_ref() == "PoolError" => data
            .get("message")
            .and_then(|value| expect_string_ref(value).ok())
            .map(str::to_string),
        _ => None,
    }
}

fn mark_pool_lease(value: VmValue, pool_id: i64) -> Result<VmValue, String> {
    let VmValue::Struct(data) = value else {
        return Err(format!(
            "ResourcePool can only lease resource structs in the VM, got `{}`",
            value.display()
        ));
    };
    let mut fields: Vec<(Rc<str>, VmValue)> = data
        .iter()
        .map(|(name, v)| (name.clone(), v.clone()))
        .collect();
    fields.push((Rc::from(POOL_LEASE_ID_FIELD), VmValue::Int(pool_id)));
    fields.push((Rc::from(POOL_LEASE_DISCARDED_FIELD), VmValue::Bool(false)));
    Ok(VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::clone(&data.name),
        fields,
    ))))
}

fn mark_pool_lease_discarded(value: VmValue) -> Result<VmValue, EvalError> {
    let VmValue::Struct(data) = value else {
        return Err(EvalError::Runtime(format!(
            "ResourcePool.discard expected an active pool lease, got `{}`.",
            value.display()
        )));
    };
    if !data.contains(POOL_LEASE_ID_FIELD) {
        return Err(EvalError::Runtime(format!(
            "ResourcePool.discard expected an active pool lease, got `{}`.",
            VmValue::Struct(Rc::clone(&data)).display()
        )));
    }
    let fields: Vec<(Rc<str>, VmValue)> = data
        .iter()
        .map(|(name, v)| {
            if name.as_ref() == POOL_LEASE_DISCARDED_FIELD {
                (name.clone(), VmValue::Bool(true))
            } else {
                (name.clone(), v.clone())
            }
        })
        .collect();
    Ok(VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::clone(&data.name),
        fields,
    ))))
}

fn split_pool_lease(value: VmValue) -> Result<Option<VmResourcePoolLease>, EvalError> {
    let VmValue::Struct(data) = value else {
        return Ok(None);
    };
    let Some(pool_id) = data.get(POOL_LEASE_ID_FIELD).cloned() else {
        return Ok(None);
    };
    let discarded = data
        .get(POOL_LEASE_DISCARDED_FIELD)
        .map(expect_bool_ref)
        .transpose()?
        .unwrap_or(false);
    // Rebuild the underlying resource struct without the lease bookkeeping fields.
    let fields: Vec<(Rc<str>, VmValue)> = data
        .iter()
        .filter(|(name, _)| {
            name.as_ref() != POOL_LEASE_ID_FIELD && name.as_ref() != POOL_LEASE_DISCARDED_FIELD
        })
        .map(|(name, v)| (name.clone(), v.clone()))
        .collect();
    Ok(Some(VmResourcePoolLease {
        pool_id: expect_int_ref(&pool_id)?,
        discarded,
        value: VmValue::Struct(Rc::new(VmStruct::from_named(Rc::clone(&data.name), fields))),
    }))
}

fn clock_system_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_millis() as i64
}

fn instant_value(unix_ms: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("unix_ms".to_string(), VmValue::Int(unix_ms))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Instant"), fields)))
}

fn deadline_after_ms(ms: i64) -> i64 {
    clock_system_unix_ms().saturating_add(ms.max(0))
}

fn deadline_value(unix_ms: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("unix_ms".to_string(), VmValue::Int(unix_ms))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Deadline"), fields)))
}

fn counter_value(value: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("value".to_string(), VmValue::Int(value))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Counter"), fields)))
}

fn config_value(name: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("name".to_string(), VmValue::string(name.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ConfigValue"),
        fields,
    )))
}

fn config_name_from_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("default")
        .to_string()
}

fn config_rules_value(name: impl Into<String>, rule_count: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("name".to_string(), VmValue::string(name.into())),
        ("rule_count".to_string(), VmValue::Int(rule_count)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Config"), fields)))
}

fn rule_value(name: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("name".to_string(), VmValue::string(name.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Rule"), fields)))
}

fn rules_from_text(text: &str) -> Vec<VmValue> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(rule_value)
        .collect()
}

fn environment_value(has_parent: bool, has_function: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("has_parent".to_string(), VmValue::Bool(has_parent)),
        ("has_function".to_string(), VmValue::Bool(has_function)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("Environment"),
        fields,
    )))
}

fn function_object_value(has_closure: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("has_closure".to_string(), VmValue::Bool(has_closure))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("FunctionObject"),
        fields,
    )))
}

fn config_store_value(name: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("name".to_string(), VmValue::string(name.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ConfigStore"),
        fields,
    )))
}

fn global_config_value(rule_count: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("rule_count".to_string(), VmValue::Int(rule_count))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("GlobalConfig"),
        fields,
    )))
}

fn request_value(path: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("path".to_string(), VmValue::string(path.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Request"), fields)))
}

fn response_value(status: i64, body: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("status".to_string(), VmValue::Int(status)),
        ("body".to_string(), VmValue::string(body.into())),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Response"), fields)))
}

#[derive(Debug, Clone)]
struct VmHttpRequest {
    method: String,
    url: String,
    body: String,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
    header_count: i64,
}

impl VmHttpRequest {
    fn to_value(&self) -> VmValue {
        http_request_value(
            &self.method,
            &self.url,
            &self.body,
            self.timeout_ms,
            self.attempts,
            self.backoff_ms,
            self.header_count,
        )
    }
}

fn http_request_value(
    method: impl Into<String>,
    url: impl Into<String>,
    body: impl Into<String>,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
    header_count: i64,
) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("method".to_string(), VmValue::string(method.into())),
        ("url".to_string(), VmValue::string(url.into())),
        ("body".to_string(), VmValue::string(body.into())),
        ("timeout_ms".to_string(), VmValue::Int(timeout_ms)),
        ("attempts".to_string(), VmValue::Int(attempts)),
        ("backoff_ms".to_string(), VmValue::Int(backoff_ms)),
        ("header_count".to_string(), VmValue::Int(header_count)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("HttpRequest"),
        fields,
    )))
}

enum WebSocketExpectedFrame {
    Text,
    Binary,
}

struct WebSocketFrame {
    opcode: u8,
    payload: Vec<u8>,
}

fn parse_ws_url(url: &str) -> Result<(String, String), VmValue> {
    let Some(rest) = url.strip_prefix("ws://") else {
        return Err(websocket_error_value(format!(
            "WebSocket VM only supports ws URLs, got `{url}`"
        )));
    };
    let (host_port, path) = match rest.split_once('/') {
        Some((host_port, path)) => (host_port, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if host_port.is_empty() {
        return Err(websocket_error_value(format!(
            "WebSocket URL is missing a host: `{url}`"
        )));
    }
    Ok((host_port.to_string(), path))
}

fn websocket_write_frame(
    stream: &mut TcpStream,
    opcode: u8,
    payload: &[u8],
) -> Result<(), VmValue> {
    let mut frame = Vec::new();
    frame.push(0x80 | (opcode & 0x0F));
    let mask_bit = 0x80;
    if payload.len() < 126 {
        frame.push(mask_bit | payload.len() as u8);
    } else if u16::try_from(payload.len()).is_ok() {
        frame.push(mask_bit | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(mask_bit | 127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    let mask = [0_u8; 4];
    frame.extend_from_slice(&mask);
    frame.extend_from_slice(payload);
    stream
        .write_all(&frame)
        .map_err(|error| websocket_error_value(format!("WebSocket send failed: {error}")))
}

fn websocket_read_frame(stream: &mut TcpStream) -> Result<WebSocketFrame, VmValue> {
    let mut header = [0; 2];
    stream
        .read_exact(&mut header)
        .map_err(|error| websocket_error_value(format!("WebSocket receive failed: {error}")))?;
    let opcode = header[0] & 0x0F;
    let masked = header[1] & 0x80 != 0;
    let mut len = u64::from(header[1] & 0x7F);
    if len == 126 {
        let mut bytes = [0; 2];
        stream.read_exact(&mut bytes).map_err(|error| {
            websocket_error_value(format!("WebSocket frame length read failed: {error}"))
        })?;
        len = u64::from(u16::from_be_bytes(bytes));
    } else if len == 127 {
        let mut bytes = [0; 8];
        stream.read_exact(&mut bytes).map_err(|error| {
            websocket_error_value(format!("WebSocket frame length read failed: {error}"))
        })?;
        len = u64::from_be_bytes(bytes);
    }
    let mut mask = [0; 4];
    if masked {
        stream.read_exact(&mut mask).map_err(|error| {
            websocket_error_value(format!("WebSocket frame mask read failed: {error}"))
        })?;
    }
    let len = usize::try_from(len)
        .map_err(|_| websocket_error_value("WebSocket frame payload is too large"))?;
    let mut payload = vec![0; len];
    stream.read_exact(&mut payload).map_err(|error| {
        websocket_error_value(format!("WebSocket frame payload read failed: {error}"))
    })?;
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok(WebSocketFrame { opcode, payload })
}

#[derive(Debug, Clone)]
struct VmDbConnection {
    url: String,
    queries: Vec<String>,
}

impl VmDbConnection {
    fn to_value(&self) -> VmValue {
        db_connection_value(self.url.clone(), self.queries.clone())
    }
}

fn db_connection_value(url: impl Into<String>, queries: Vec<String>) -> VmValue {
    let mut fields: Vec<(String, VmValue)> = vec![("url".to_string(), VmValue::string(url.into()))];
    fields.push((
        "queries".to_string(),
        VmValue::List(Rc::new(RefCell::new(
            queries.into_iter().map(VmValue::string).collect(),
        ))),
    ));
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("DbConnection"),
        fields,
    )))
}

fn db_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("DbError"), fields)))
}

#[derive(Debug, Clone)]
struct VmProcessOutput {
    status: i64,
    stdout: String,
    stderr: String,
    merged: String,
    truncated: bool,
}

#[derive(Debug, Clone)]
struct VmProcessRequest {
    command: String,
    args: Vec<String>,
    cwd: Option<PathBuf>,
    stdin: Option<String>,
    env: Vec<(String, String)>,
    /// Process deadline in ms; enforced by `process_run_request` (kills the child
    /// past the deadline), matching the compiled runtime.
    timeout_ms: i64,
    merge_stderr: bool,
    output_cap_bytes: i64,
}

fn process_run_output(command: &str, args: &[String]) -> Result<VmProcessOutput, VmValue> {
    std::process::Command::new(command)
        .args(args)
        .output()
        .map(process_output_state)
        .map_err(|error| VmValue::string(error.to_string()))
}

fn process_run_request(request: &VmProcessRequest) -> Result<VmProcessOutput, VmValue> {
    if request.command.trim().is_empty() {
        return Err(VmValue::string("process command must not be empty"));
    }
    let mut command = std::process::Command::new(&request.command);
    command
        .args(&request.args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    if request.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    }
    if let Some(cwd) = &request.cwd {
        command.current_dir(cwd);
    }
    for (name, value) in &request.env {
        command.env(name, value);
    }
    let mut child = command.spawn().map_err(|error| {
        VmValue::string(format!("failed to run `{}`: {error}", request.command))
    })?;
    if let Some(stdin) = &request.stdin
        && let Some(mut child_stdin) = child.stdin.take()
    {
        child_stdin.write_all(stdin.as_bytes()).map_err(|error| {
            VmValue::string(format!(
                "failed to write stdin for `{}`: {error}",
                request.command
            ))
        })?;
    }

    // With a positive deadline, mirror the runtime: read the pipes on background
    // threads (so they can't deadlock the child) while polling for exit, and kill
    // the child once the deadline passes — returning a `timed out` error.
    if request.timeout_ms > 0 {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(request.timeout_ms as u64);
        let read_pipe = |pipe: Option<std::process::ChildStdout>| {
            pipe.map(|mut reader| {
                std::thread::spawn(move || {
                    let mut buf = Vec::new();
                    let _ = reader.read_to_end(&mut buf);
                    buf
                })
            })
        };
        let stdout_thread = read_pipe(child.stdout.take());
        let stderr_thread = child.stderr.take().map(|mut reader| {
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = reader.read_to_end(&mut buf);
                buf
            })
        });
        let mut timed_out = false;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        timed_out = true;
                        let _ = child.kill();
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(error) => {
                    return Err(VmValue::string(format!(
                        "failed to poll `{}`: {error}",
                        request.command
                    )));
                }
            }
        }
        let status = child.wait().map_err(|error| {
            VmValue::string(format!("failed to wait for `{}`: {error}", request.command))
        })?;
        let stdout = stdout_thread
            .and_then(|t| t.join().ok())
            .unwrap_or_default();
        let stderr = stderr_thread
            .and_then(|t| t.join().ok())
            .unwrap_or_default();
        let state = process_output_state_from_parts(
            status.code().unwrap_or(-1) as i64,
            &stdout,
            &stderr,
            request.output_cap_bytes,
            request.merge_stderr,
        );
        if timed_out {
            return Err(VmValue::string(format!(
                "`{}` timed out after {}ms: {}",
                request.command,
                request.timeout_ms,
                process_output_details(&state.stdout, &state.stderr)
            )));
        }
        return Ok(state);
    }

    let output = child.wait_with_output().map_err(|error| {
        VmValue::string(format!("failed to wait for `{}`: {error}", request.command))
    })?;
    Ok(process_output_state_with_capture(
        output,
        request.output_cap_bytes,
        request.merge_stderr,
    ))
}

fn process_output_state(output: std::process::Output) -> VmProcessOutput {
    process_output_state_with_capture(output, 0, false)
}

fn process_output_state_with_capture(
    output: std::process::Output,
    output_cap_bytes: i64,
    merge_stderr: bool,
) -> VmProcessOutput {
    process_output_state_from_parts(
        output.status.code().unwrap_or(-1) as i64,
        &output.stdout,
        &output.stderr,
        output_cap_bytes,
        merge_stderr,
    )
}

/// Build a process output from already-captured stdout/stderr bytes and a status
/// code (used by the timeout path, which reads the pipes on background threads).
fn process_output_state_from_parts(
    status: i64,
    stdout_bytes: &[u8],
    stderr_bytes: &[u8],
    output_cap_bytes: i64,
    merge_stderr: bool,
) -> VmProcessOutput {
    let cap = usize::try_from(output_cap_bytes)
        .ok()
        .filter(|value| *value > 0);
    let mut capture = VmProcessCapture::new(cap, merge_stderr);
    capture.push(false, stdout_bytes);
    capture.push(true, stderr_bytes);
    let stdout = String::from_utf8_lossy(&capture.stdout).to_string();
    let stderr = String::from_utf8_lossy(&capture.stderr).to_string();
    let merged = String::from_utf8_lossy(&capture.merged).to_string();
    VmProcessOutput {
        status,
        stdout,
        stderr,
        merged,
        truncated: capture.truncated,
    }
}

struct VmProcessCapture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    merged: Vec<u8>,
    cap: Option<usize>,
    used: usize,
    merge_stderr: bool,
    truncated: bool,
}

impl VmProcessCapture {
    fn new(cap: Option<usize>, merge_stderr: bool) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            merged: Vec::new(),
            cap,
            used: 0,
            merge_stderr,
            truncated: false,
        }
    }

    fn push(&mut self, stderr: bool, bytes: &[u8]) {
        let bytes = self.capped_bytes(bytes).to_vec();
        if bytes.is_empty() {
            return;
        }
        if stderr {
            self.stderr.extend_from_slice(&bytes);
        } else {
            self.stdout.extend_from_slice(&bytes);
        }
        if self.merge_stderr || !stderr {
            self.merged.extend_from_slice(&bytes);
        }
    }

    fn capped_bytes<'a>(&mut self, bytes: &'a [u8]) -> &'a [u8] {
        let Some(cap) = self.cap else {
            return bytes;
        };
        if self.used >= cap {
            self.truncated = true;
            return &bytes[..0];
        }
        let remaining = cap - self.used;
        if bytes.len() > remaining {
            self.truncated = true;
            self.used = cap;
            &bytes[..remaining]
        } else {
            self.used += bytes.len();
            bytes
        }
    }
}

fn process_stdout_result(command: &str, output: VmProcessOutput) -> Result<String, VmValue> {
    if output.status == 0 {
        Ok(output.stdout)
    } else {
        Err(VmValue::string(format!(
            "process `{command}` exited with status {}: {}",
            output.status,
            process_output_details(&output.stdout, &output.stderr)
        )))
    }
}

fn process_run_many_stdout(
    command: &str,
    args: &[String],
    appended: &[String],
) -> Result<Vec<String>, VmValue> {
    let mut values = Vec::with_capacity(appended.len());
    for item in appended {
        let mut command_args = args.to_vec();
        command_args.push(item.clone());
        let output = process_run_output(command, &command_args)?;
        values.push(process_stdout_result(command, output)?);
    }
    Ok(values)
}

fn process_output_details(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.trim().to_string(),
        (true, false) => stderr.trim().to_string(),
        (false, false) => format!("{} {}", stdout.trim(), stderr.trim()),
    }
}

fn process_output_value(output: VmProcessOutput) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("status".to_string(), VmValue::Int(output.status)),
        ("stdout".to_string(), VmValue::string(output.stdout)),
        ("stderr".to_string(), VmValue::string(output.stderr)),
        ("merged".to_string(), VmValue::string(output.merged)),
        ("truncated".to_string(), VmValue::Bool(output.truncated)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ProcessOutput"),
        fields,
    )))
}

fn process_event_value(kind: &str, data: &str, status: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("kind".to_string(), VmValue::string(kind)),
        ("data".to_string(), VmValue::string(data)),
        ("status".to_string(), VmValue::Int(status)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ProcessEvent"),
        fields,
    )))
}

#[derive(Debug, Clone)]
struct VmFileState {
    path: String,
    mode: String,
    cursor: u64,
}

impl VmFileState {
    fn to_value(&self) -> VmValue {
        file_value(&self.path, &self.mode, self.cursor)
    }
}

fn file_value(path: impl Into<String>, mode: impl Into<String>, cursor: u64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("path".to_string(), VmValue::string(path.into())),
        ("mode".to_string(), VmValue::string(mode.into())),
        ("cursor".to_string(), VmValue::Int(cursor as i64)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("File"), fields)))
}

fn file_read_remaining(file: &mut VmFileState) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    if file.mode != "read" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "file is not open for reading",
        ));
    }
    let mut handle = std::fs::File::open(&file.path)?;
    handle.seek(SeekFrom::Start(file.cursor))?;
    let mut bytes = Vec::new();
    handle.read_to_end(&mut bytes)?;
    file.cursor = file.cursor.saturating_add(bytes.len() as u64);
    Ok(bytes)
}

fn file_write_at_cursor(file: &mut VmFileState, data: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};

    if file.mode != "write" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "file is not open for writing",
        ));
    }
    let mut handle = std::fs::OpenOptions::new().write(true).open(&file.path)?;
    handle.seek(SeekFrom::Start(file.cursor))?;
    handle.write_all(data)?;
    file.cursor = file.cursor.saturating_add(data.len() as u64);
    Ok(())
}

fn file_result_unit(value: std::io::Result<()>) -> VmValue {
    json_result(
        value
            .map(|_| VmValue::Unit)
            .map_err(|error| file_error_value(error.to_string())),
    )
}

fn file_atomic_write_result(path: PathBuf, text: &str) -> VmValue {
    use std::io::Write;

    let result = (|| -> std::io::Result<()> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "rsscript-atomic-write".to_string());
        let temp_path = parent.join(format!(
            ".{file_name}.tmp-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));
        {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temp_path)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
        }
        match std::fs::rename(&temp_path, &path) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = std::fs::remove_file(&temp_path);
                Err(error)
            }
        }
    })();
    file_result_unit(result)
}

fn file_append(path: &str, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    handle.write_all(data)
}

fn file_bytes_stream_value(path: &str, chunk_size: i64) -> Result<VmValue, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("file byte stream open failed: {error}"))?;
    let chunk_size = chunk_size.max(1) as usize;
    Ok(stream_value(
        bytes
            .chunks(chunk_size)
            .map(|chunk| VmValue::Bytes(Rc::new(chunk.to_vec())))
            .collect(),
    ))
}

fn file_metadata_value(metadata: std::fs::Metadata) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("is_file".to_string(), VmValue::Bool(metadata.is_file())),
        ("is_dir".to_string(), VmValue::Bool(metadata.is_dir())),
        ("len".to_string(), VmValue::Int(metadata.len() as i64)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("FileMetadata"),
        fields,
    )))
}

fn directory_list_files(root: &Path) -> std::io::Result<Vec<String>> {
    let mut files = Vec::new();
    collect_directory_files(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_directory_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<String>,
) -> std::io::Result<()> {
    if current.is_file() {
        files.push(relative_runtime_path(root, current));
        return Ok(());
    }
    for entry in std::fs::read_dir(current)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_directory_files(root, &path, files)?;
        } else if path.is_file() {
            files.push(relative_runtime_path(root, &path));
        }
    }
    Ok(())
}

fn directory_list_paths(root: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in std::fs::read_dir(root)? {
        paths.push(entry?.path());
    }
    paths.sort();
    Ok(paths)
}

fn relative_runtime_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn tempdir_value(path: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("path".to_string(), VmValue::string(path.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("TempDir"), fields)))
}

fn tempdir_new_value(parent: PathBuf) -> Result<VmValue, VmValue> {
    let seed = clock_system_unix_ms();
    for attempt in 0..100 {
        let path = parent.join(format!("rsscript-{}-{seed}-{attempt}", std::process::id()));
        match std::fs::create_dir_all(&path) {
            Ok(()) => return Ok(tempdir_value(path.to_string_lossy())),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(file_error_value(error.to_string())),
        }
    }
    Err(file_error_value(
        "could not allocate unique TempDir path".to_string(),
    ))
}

struct ImageState {
    bytes: Vec<u8>,
    width: Option<i64>,
    height: Option<i64>,
    operations: Vec<String>,
}

impl ImageState {
    fn to_value(&self) -> VmValue {
        image_value(
            self.bytes.clone(),
            self.width,
            self.height,
            self.operations.clone(),
        )
    }

    fn saved_bytes(&self) -> Vec<u8> {
        let mut bytes = self.bytes.clone();
        bytes.extend_from_slice(b"\n# rsscript-image-ops:");
        bytes.extend_from_slice(self.operations.join(",").as_bytes());
        if let (Some(width), Some(height)) = (self.width, self.height) {
            bytes.extend_from_slice(format!(";size={width}x{height}").as_bytes());
        }
        bytes
    }

    fn inspect_line(&self) -> String {
        let size = self
            .width
            .zip(self.height)
            .map(|(width, height)| format!("{width}x{height}"))
            .unwrap_or_else(|| "unknown".to_string());
        format!(
            "image bytes={} size={} ops={}",
            self.bytes.len(),
            size,
            self.operations.join(",")
        )
    }
}

fn image_value(
    bytes: Vec<u8>,
    width: Option<i64>,
    height: Option<i64>,
    operations: Vec<String>,
) -> VmValue {
    let mut fields: Vec<(String, VmValue)> =
        vec![("bytes".to_string(), VmValue::Bytes(Rc::new(bytes)))];
    fields.push((
        "width".to_string(),
        width
            .map(|value| value_some(VmValue::Int(value)))
            .unwrap_or_else(value_none),
    ));
    fields.push((
        "height".to_string(),
        height
            .map(|value| value_some(VmValue::Int(value)))
            .unwrap_or_else(value_none),
    ));
    fields.push((
        "operations".to_string(),
        VmValue::List(Rc::new(RefCell::new(
            operations.into_iter().map(VmValue::string).collect(),
        ))),
    ));
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Image"), fields)))
}

fn image_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ImageError"),
        fields,
    )))
}

fn cancellation_source_value(id: i64) -> VmValue {
    cancellation_handle_value("CancellationSource", id)
}

fn cancellation_token_value(id: i64) -> VmValue {
    cancellation_handle_value("CancellationToken", id)
}

fn cancellation_handle_value(name: &'static str, id: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("id".to_string(), VmValue::Int(id))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from(name), fields)))
}

fn stream_value(items: Vec<VmValue>) -> VmValue {
    let mut fields: Vec<(String, VmValue)> = vec![(
        "items".to_string(),
        VmValue::List(Rc::new(RefCell::new(items))),
    )];
    fields.push(("collect_error".to_string(), VmValue::OptionNone));
    fields.push(("channel_id".to_string(), VmValue::OptionNone));
    fields.push(("stream_id".to_string(), VmValue::OptionNone));
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Stream"), fields)))
}

fn stream_channel_value(channel_id: i64) -> VmValue {
    let mut fields: Vec<(String, VmValue)> = vec![(
        "items".to_string(),
        VmValue::List(Rc::new(RefCell::new(Vec::new()))),
    )];
    fields.push(("collect_error".to_string(), VmValue::OptionNone));
    fields.push((
        "channel_id".to_string(),
        VmValue::OptionSome(Box::new(VmValue::Int(channel_id))),
    ));
    fields.push(("stream_id".to_string(), VmValue::OptionNone));
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Stream"), fields)))
}

fn stream_collect_error_value(message: impl Into<String>) -> VmValue {
    let mut fields: Vec<(String, VmValue)> = vec![(
        "items".to_string(),
        VmValue::List(Rc::new(RefCell::new(Vec::new()))),
    )];
    fields.push((
        "collect_error".to_string(),
        VmValue::OptionSome(Box::new(VmValue::string(message.into()))),
    ));
    fields.push(("channel_id".to_string(), VmValue::OptionNone));
    fields.push(("stream_id".to_string(), VmValue::OptionNone));
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Stream"), fields)))
}

#[derive(Debug, Clone)]
struct VmStreamState {
    items: Rc<RefCell<Vec<VmValue>>>,
    collect_error: Option<String>,
    channel_id: Option<i64>,
}
