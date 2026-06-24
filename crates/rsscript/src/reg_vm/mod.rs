use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

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
use crate::vm_value::{
    intern_layout, TypeLayout, TypedVec, ValueMap, VmClosure, VmMapKey, VmNative, VmStruct, VmValue,
};

/// Intern the layout for a struct/variant whose canonical field order is given by
/// `fields` (slot order). Used at lowering time so `MakeStruct`/`MakeVariant` carry
/// a precomputed `Rc<TypeLayout>` and never re-hash per construction (V2.0).
fn intern_struct_layout(name: &str, fields: &[(String, Reg)]) -> Rc<TypeLayout> {
    let field_names: Vec<Rc<str>> = fields.iter().map(|(name, _)| Rc::from(name.as_str())).collect();
    intern_layout(Rc::from(name), field_names)
}

mod calls;
mod intrinsics;
mod resources;
mod scheduler;
mod runtime_resources;
mod runtime_values;
mod value_access;
mod value_convert;
mod value_ops;
#[cfg(feature = "native-jit")]
mod native;
#[cfg(feature = "native-jit")]
use native::*;
use resources::*;
use runtime_resources::*;
use runtime_values::*;
use value_access::*;
use value_convert::*;
use value_ops::*;

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

/// Compile `source` and run `main` under explicit sandbox resource limits.
/// Convenience wrapper around [`RegVmExecutable::eval_main_with_limits`] for the
/// untrusted/agent-facing path (and for the hostile-input tests).
pub fn reg_vm_eval_source_main_with_limits(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
    limits: VmLimits,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_limits(args, limits)
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

/// Native-tier entry point with J0.2 **precise resume** forced on: a real native
/// guard bail reconstructs the interpreter register window and resumes at the
/// safepoint instead of re-running from the function top. Must equal every other
/// backend. Validation/test entry point (sets the flag deterministically).
#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_precise(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_precise(args)
}

/// Native-tier entry point with J5.2 **OSR** (on-stack replacement) forced on: a
/// function with a qualifying native-subset hot loop runs that loop natively
/// mid-function (OSR-entry at the loop header reading the live-in window;
/// OSR-exit/precise-resume at the post-loop ip with the live-out window). Must
/// equal every other backend byte-for-byte. Validation/test/bench entry point.
#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_osr(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<EvalOutput, EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_osr(args)
}

/// Lever 2 test entry point: run `main` with the native tier + OSR forced on AND the
/// `RSS_JIT_REPORT` missed-optimization report armed deterministically, returning the
/// report's per-region lines (one `\n`-joined block per function) alongside the output
/// and stats. The report is observational, so the output equals every other backend.
#[cfg(feature = "native-jit")]
pub fn reg_vm_eval_source_main_native_osr_report(
    file: &str,
    source: &str,
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<(EvalOutput, NativeStats, Vec<String>), EvalError> {
    reg_vm_compile_source(file, source)?.eval_main_with_args_native_osr_report(args)
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

/// Instructions reachable from `ip == 0` along the control-flow graph
/// (sequential fallthrough, jumps, conditional branches, branch-shaped match
/// arms). Mirrors [`native_reachable_instructions`] but is always compiled (the
/// TCO pass runs regardless of the `native-jit` feature). Used to ignore the
/// lowerer's unreachable defensive `LoadUnit; Return` tail when deciding whether
/// a self-tail-recursive function has a genuine base case.
fn tco_reachable_instructions(code: &[RegInstr]) -> Vec<bool> {
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
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            } => {
                stack.push(*some_ip);
                stack.push(*none_ip);
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                stack.push(*ok_ip);
                stack.push(*err_ip);
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                stack.push(*match_ip);
                stack.push(*else_ip);
            }
            // Terminators with no fallthrough.
            RegInstr::Return { .. } | RegInstr::RuntimeError { .. } => {}
            // Everything else falls through to the next instruction.
            _ => stack.push(i + 1),
        }
    }
    reachable
}

/// Self-tail-call optimization (TCO).
///
/// Rewrites every **self-tail-call** in `function` — a `CallKnown` to this same
/// `function_id`, immediately followed by a `Return` of the call's result, with
/// no `mut`-args and no intervening observable use of the result — into an
/// argument rebind plus a backward `Jump` to the function body's entry (just
/// after the one-time parameter prologue). This turns self-tail-recursion into a
/// loop: the function loses its call-graph self-edge, so
/// [`compute_jit_eligibility`] (run *after* this pass) sees it as non-recursive
/// and it becomes native-eligible.
///
/// ## Rewrite shape (index-stable)
/// To avoid renumbering every jump target, the rewrite never deletes or inserts
/// instructions in the middle. For each self-tail-call at `(call_ip, return_ip)`
/// (the `Return` is at `call_ip + 1`):
///   * a rebind block is **appended** at the end of `code`:
///       1. `Move staging_k := args[k]`  (copy every new arg into a fresh temp
///          first, so the rebind is *simultaneous* even when an arg reads a
///          parameter register — e.g. `f(n: n - 1, acc: acc + n)` reads the old
///          `acc`/`n` while computing the new ones);
///       2. `Move param_k := staging_k`  (rebind the parameter registers);
///       3. `Jump { target: entry }`     (the backward loop edge to the body).
///   * the original `CallKnown` is replaced **in place** by `Jump { target: rebind_block }`.
///   * the original `Return` becomes unreachable (left as dead code; the
///     reachability/native passes ignore it).
///
/// Appended instructions only add *forward* edges from the existing body and a
/// single backward edge to `entry`; every pre-existing index is untouched.
///
/// `entry` is the first instruction after the prologue — the leading run of
/// `DeepCopy` instructions emitted once per non-`mut` parameter (see `lower`).
/// The back-edge must NOT re-run that prologue: a rebind value is either a fresh
/// arithmetic result or a value this frame already owns (deep-copied on the
/// original entry), so re-isolating it is unnecessary and skipping it is sound.
///
/// ## Soundness gates (any failure ⇒ the whole function is left untouched, so it
/// keeps running interpreted exactly as today — a strict improvement, never a
/// regression):
///   * **Tail position.** The instruction after the `CallKnown` must be exactly
///     `Return { src: dst }` for the call's `dst`, and that `dst` must be read by
///     no other instruction in the body (nothing observes the result between the
///     call and the return). Mutual/non-tail self-calls (`return f(..) + g(..)`)
///     therefore never qualify — their result is consumed by an arithmetic op.
///   * **Self only.** `function == function_id`.
///   * **No `mut`-args.** `mut_args` must be empty (a `mut` write-back is an
///     observable effect that must run on return; a loop would skip it).
///   * **Arity.** `args.len() == params`.
///   * **Recursion-depth-limit (`VmLimits::max_depth`) observability — the key
///     soundness condition.** An unbounded self-tail-recursion with *no* base
///     case (e.g. `fn f(n){ return f(n: n + 1) }`) today hits the depth cap and
///     returns a clean `"recursion depth limit exceeded"` error (this is an
///     asserted safety property — `hostile::deep_recursion_returns_clean_error_not_crash`).
///     Converting such a function to a loop would replace that error with an
///     infinite loop (a hang / observability change). We therefore **refuse to
///     TCO any function whose every `Return` is a self-tail-call** — i.e. one
///     with no non-tail-call return reachable, hence no base case at all. Such a
///     function stays interpreted and keeps tripping the depth cap, preserving
///     the limit's observability byte-for-byte. A function *with* a base-case
///     return (`sum_to`'s `return acc`) is TCO'd; for it the depth limit was only
///     ever reachable on a non-terminating input, which is not exercised by the
///     differential corpus (curated terminating programs, all backends share this
///     transformed bytecode) nor asserted anywhere — and native already relies on
///     `step_budget`, not `max_depth`, to bound every ordinary loop, so this does
///     not change native's safety posture.
fn optimize_self_tail_calls(function: &mut RegFunction, function_id: usize) {
    // Locate every candidate self-tail-call site: (call_ip, dst, args, ...).
    let mut sites: Vec<(usize, Reg, Vec<Reg>)> = Vec::new();
    let code = &function.code;
    for call_ip in 0..code.len() {
        let RegInstr::CallKnown {
            dst,
            function: callee,
            args,
            mut_args,
        } = &code[call_ip]
        else {
            continue;
        };
        // Self only, no mut write-backs, matching arity.
        if *callee != function_id || !mut_args.is_empty() || args.len() != function.params {
            continue;
        }
        // Tail position: the very next instruction returns this call's result.
        let return_ip = call_ip + 1;
        let Some(RegInstr::Return { src }) = code.get(return_ip) else {
            continue;
        };
        if *src != *dst {
            continue;
        }
        // No other instruction may read the call's result (nothing observes it
        // between the call and the return). The `Return` at `return_ip` is the
        // only legitimate reader.
        let used_elsewhere = code.iter().enumerate().any(|(other_ip, instr)| {
            other_ip != return_ip && instr_reads_register(instr, *dst)
        });
        if used_elsewhere {
            continue;
        }
        sites.push((call_ip, *dst, args.clone()));
    }
    if sites.is_empty() {
        return;
    }
    // Recursion-depth-limit gate: bail entirely if the function has NO
    // **reachable** non-tail `Return` (no base case). A function whose every
    // reachable exit is a self-tail-call (e.g. `fn f(n){ return f(n: n + 1) }`)
    // only ever terminated by hitting the depth cap; converting it to a loop
    // would turn that clean `"recursion depth limit exceeded"` error into a hang,
    // so it must stay interpreted. Reachability matters: `lower` always appends a
    // defensive, usually-unreachable `LoadUnit; Return` tail, which must not be
    // mistaken for a real base case.
    let reachable = tco_reachable_instructions(&function.code);
    let tail_return_ips: Vec<usize> = sites.iter().map(|(call_ip, _, _)| call_ip + 1).collect();
    let has_base_case = function.code.iter().enumerate().any(|(ip, instr)| {
        reachable[ip]
            && matches!(instr, RegInstr::Return { .. })
            && !tail_return_ips.contains(&ip)
    });
    if !has_base_case {
        return;
    }
    // Entry = first instruction past the prologue (leading `DeepCopy` run).
    let entry = function
        .code
        .iter()
        .position(|instr| !matches!(instr, RegInstr::DeepCopy { .. }))
        .unwrap_or(0);

    // Apply every site. Appends rebind blocks at the tail; existing indices are
    // never disturbed, so jump targets stay valid.
    for (call_ip, _dst, args) in sites {
        // Stage each new arg into a fresh temp (simultaneous rebind), then move
        // the temps into the parameter registers, then jump to the body entry.
        let staging: Vec<Reg> = (0..args.len())
            .map(|_| {
                let reg = function.regs;
                function.regs += 1;
                reg
            })
            .collect();
        let block_start = function.code.len();
        for (slot, &arg) in args.iter().enumerate() {
            function.code.push(RegInstr::Move {
                dst: staging[slot],
                src: arg,
            });
        }
        for (param, &src) in staging.iter().enumerate() {
            function.code.push(RegInstr::Move { dst: param, src });
        }
        function.code.push(RegInstr::Jump { target: entry });
        // Redirect the original call site into the rebind block. The following
        // `Return` becomes unreachable dead code.
        function.code[call_ip] = RegInstr::Jump {
            target: block_start,
        };
    }
}

/// Whether `instr` reads register `reg` as a value operand. Used by the TCO pass
/// to confirm a self-tail-call's result is observed by nothing but its trailing
/// `Return`. Conservative by construction: any instruction variant whose operands
/// are not explicitly enumerated returns `true` (treated as a use), so the TCO
/// gate bails rather than risk missing a reader.
fn instr_reads_register(instr: &RegInstr, reg: Reg) -> bool {
    match instr {
        RegInstr::LoadUnit { .. }
        | RegInstr::LoadInt { .. }
        | RegInstr::LoadFloat { .. }
        | RegInstr::LoadBool { .. }
        | RegInstr::LoadString { .. }
        | RegInstr::LoadNone { .. }
        | RegInstr::Jump { .. }
        | RegInstr::RuntimeError { .. } => false,
        RegInstr::Move { src, .. }
        | RegInstr::Manage { src, .. }
        | RegInstr::MakeSome { value: src, .. }
        | RegInstr::UnwrapSome { src, .. }
        | RegInstr::UnwrapVariantValue { src, .. }
        | RegInstr::AwaitJoin { src, .. } => *src == reg,
        RegInstr::DeepCopy { reg: r } => *r == reg,
        RegInstr::GetField { base, .. } | RegInstr::GetFieldSlot { base, .. } => *base == reg,
        RegInstr::SetField { base, value, .. } | RegInstr::SetFieldSlot { base, value, .. } => {
            *base == reg || *value == reg
        }
        RegInstr::AddInt { lhs, rhs, .. }
        | RegInstr::SubInt { lhs, rhs, .. }
        | RegInstr::MulInt { lhs, rhs, .. }
        | RegInstr::DivInt { lhs, rhs, .. }
        | RegInstr::ModInt { lhs, rhs, .. }
        | RegInstr::BitAndInt { lhs, rhs, .. }
        | RegInstr::BitOrInt { lhs, rhs, .. }
        | RegInstr::BitXorInt { lhs, rhs, .. }
        | RegInstr::ShiftLeftInt { lhs, rhs, .. }
        | RegInstr::ShiftRightInt { lhs, rhs, .. }
        | RegInstr::LessInt { lhs, rhs, .. }
        | RegInstr::LessEqualInt { lhs, rhs, .. }
        | RegInstr::GreaterInt { lhs, rhs, .. }
        | RegInstr::GreaterEqualInt { lhs, rhs, .. }
        | RegInstr::Equal { lhs, rhs, .. }
        | RegInstr::NotEqual { lhs, rhs, .. }
        | RegInstr::JumpIfIntCompare { lhs, rhs, .. } => *lhs == reg || *rhs == reg,
        RegInstr::JumpIfBool { cond, .. } => *cond == reg,
        RegInstr::Return { src } => *src == reg,
        RegInstr::MatchOption { src, .. }
        | RegInstr::MatchResult { src, .. }
        | RegInstr::MatchVariant { src, .. } => *src == reg,
        RegInstr::MatchMapGet { map, key, .. } => *map == reg || *key == reg,
        RegInstr::MakeStruct { fields, .. } | RegInstr::MakeVariant { fields, .. } => {
            fields.iter().any(|(_, r)| *r == reg)
        }
        RegInstr::MakeObject { fields, .. } => fields.iter().any(|(_, r)| *r == reg),
        RegInstr::MakeMap { entries, .. } => {
            entries.iter().any(|(k, v)| *k == reg || *v == reg)
        }
        RegInstr::MakeList { items, .. } => items.contains(&reg),
        RegInstr::MakeClosure { captures, .. } => captures.contains(&reg),
        RegInstr::ResourceDrop { resource } => *resource == reg,
        RegInstr::CallKnown { args, .. }
        | RegInstr::CallDynamic { args, .. }
        | RegInstr::CallNative { args, .. }
        | RegInstr::SpawnTask { args, .. } => args.contains(&reg),
        RegInstr::CallClosure { closure, args, .. } => {
            *closure == reg || args.contains(&reg)
        }
        RegInstr::SelectWait { handles, .. } => handles.contains(&reg),
        // Synthetic native-only ops never appear in lowered code seen by TCO, but
        // enumerate them for completeness.
        RegInstr::NativeGuardClosureId { closure, .. }
        | RegInstr::NativeClosureId { closure, .. }
        | RegInstr::NativeClosureCapture { closure, .. } => *closure == reg,
        // Any operand-bearing variant not enumerated above is treated as a use,
        // so the TCO gate conservatively bails.
        _ => true,
    }
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



// ============================================================================
// Central intrinsic/effect registry (JIT descriptor table)
// ============================================================================
//
// One `IntrinsicDescriptor` per `RegIntrinsic`, re-encoding the per-intrinsic
// facts the JIT's three hand-coded classification sites need. The table is the
// single source of truth for *which* intrinsics each site admits/expands/folds;
// the sites keep their exact lowering/fold/expansion *mechanism*.
//
// Conservative DEFAULT: the vast majority of the ~637 `RegIntrinsic` variants
// are opaque to the JIT — they allocate / write / suspend / are not foldable and
// not native-lowerable, so they BAIL out of the native subset. The `Default` impl
// encodes exactly that (`effect: Allocate`, every capability `false`). Only the
// intrinsics the three sites historically special-cased carry an explicit
// descriptor; populating richer facts for the rest is incremental future work and
// changes no behavior until a site is taught to read the new field.

/// The observable effect class of an intrinsic, as the JIT cares about it. Today's
/// sites only need to distinguish "pure/read" (safe to fold / re-run after a native
/// bail) from "allocate/write/suspend" (opaque to the native path). The richer
/// split is recorded for the future missed-optimization report (lever 2).
// The registry's consumers (the three JIT classification sites) are all
// `native-jit`-gated, so in a plain library build the table and its fields look
// dead. They are exercised under `--features native-jit` and by the table unit
// test; keep them compiled unconditionally as the lever-2 substrate.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IntrinsicEffect {
    /// No observable effect; result depends only on the (read-only) operands.
    Pure,
    /// Reads heap/host state but mutates nothing (e.g. a length query).
    Read,
    /// Allocates a fresh heap value from its operands; observes/mutates nothing
    /// else. This is the conservative DEFAULT.
    Allocate,
    /// Mutates heap/host/collection state.
    Write,
    /// May suspend (async/stream/await).
    Suspend,
}

/// The role a foldable-string intrinsic plays in the string-length-fold pass: it is
/// either a *producer* of a string whose byte length the pass can compute from its
/// operands, or the length *query* itself. `None` ⇒ not part of that pass.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StringFoldRole {
    /// `String.from_int` — produces an (always-ASCII) decimal string from an Int.
    ProducerFromInt,
    /// `String.slice` — produces a (length-law, ASCII-gated) substring.
    ProducerSlice,
    /// `String.len` — the byte-length query the pass dissolves into arithmetic.
    LengthQuery,
}

/// The role a foldable-Bytes intrinsic plays in the (Bytes sibling of the) query-fold
/// pass. Bytes are RAW bytes — there is no char/grapheme boundary, so the Bytes slice
/// length law is exact integer arithmetic with NO ASCII gate (unlike `String.slice`).
/// A producer's byte length is computed from its operands; the query is the length read.
/// `None` ⇒ not part of the Bytes fold.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BytesFoldRole {
    /// `Bytes.from_string` — produces raw bytes from a String; its byte length equals
    /// the source String's byte length (`value.as_bytes().len()`).
    ProducerFromString,
    /// `Bytes.slice` — produces a byte-index substring; the length law is the exact
    /// clamp arithmetic of `bytes_slice` (no char-boundary subtlety, no ASCII gate).
    ProducerSlice,
    /// `Bytes.len` — the byte-length query the pass dissolves into arithmetic.
    LengthQuery,
}

/// Per-intrinsic JIT facts, keyed by `RegIntrinsic` via [`intrinsic_descriptor`].
/// Every field defaults to the most conservative value so an unlisted intrinsic is
/// automatically opaque to all three sites (see the module note above).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
struct IntrinsicDescriptor {
    /// Observable effect class (pure/read vs allocate/write/suspend).
    effect: IntrinsicEffect,
    /// Whether a value this intrinsic produces can be folded away when used only by
    /// a read-only query (e.g. `String.from_int`/`String.slice` feeding `String.len`).
    /// Also marks the pure heap value-builders a deopt cold arm may re-run.
    can_fold: bool,
    /// Whether the intrinsic can be emitted directly in the native subset (today:
    /// only `IntToFloat`, in its single-Int-arg form — the shape check stays at the
    /// call site).
    native_lowerable: bool,
    /// RESERVED for the future view work (zero-copy slice/borrow lowering). Set
    /// `false` for every intrinsic today; it exists only so the table shape is right
    /// for lever-2 / view consumers. No site reads it yet.
    view_capable: bool,
    /// If `Some`, this intrinsic is one of the six expandable Option/Result
    /// combinators, with its concrete lowering kind. The combinator-expansion pass
    /// uses this for *recognition*; it keeps the per-kind match/construct emission.
    combinator_kind: Option<CombinatorKind>,
    /// If `Some`, this intrinsic participates in the string-length-fold pass in the
    /// given role. The pass uses this for *classification*; it keeps the exact length
    /// laws and the ASCII-only-slice bail.
    string_fold_role: Option<StringFoldRole>,
    /// If `Some`, this intrinsic participates in the Bytes-length-fold pass in the
    /// given role (the Bytes sibling of `string_fold_role`). The pass uses this for
    /// *classification*; the exact byte-length laws stay in the pass. Bytes carry no
    /// char-boundary subtlety, so the slice law needs no ASCII gate.
    bytes_fold_role: Option<BytesFoldRole>,
    /// Whether this intrinsic is a pure, re-runnable heap String *builder* that the
    /// deopt-before-heap cold-arm classifier permits inside a bailable cold arm (it
    /// allocates a fresh String from read-only operands and observes/mutates nothing
    /// else). A tight whitelist; impure intrinsics (I/O, env, collections, time, RNG)
    /// are excluded. Distinct from `can_fold` (which also covers queries/combinators).
    cold_arm_pure_builder: bool,
    /// Short human-readable reason for the conservative classification, for the
    /// future missed-optimization report (e.g. "allocates", "suspends",
    /// "non-ASCII-dependent slice"). Empty for the trivial/expected cases.
    notes: &'static str,
}

impl Default for IntrinsicDescriptor {
    /// The conservative default for the ~637 intrinsics that no site special-cases:
    /// treat as an opaque allocator that the JIT cannot fold or lower.
    fn default() -> Self {
        IntrinsicDescriptor {
            effect: IntrinsicEffect::Allocate,
            can_fold: false,
            native_lowerable: false,
            view_capable: false,
            combinator_kind: None,
            string_fold_role: None,
            bytes_fold_role: None,
            cold_arm_pure_builder: false,
            notes: "default: opaque to JIT (allocate/not-foldable/not-native-lowerable)",
        }
    }
}

/// The central JIT descriptor for `intrinsic`. Returns the conservative
/// [`IntrinsicDescriptor::default`] for every intrinsic not explicitly listed (the
/// vast majority) and an explicit descriptor for the ones the three classification
/// sites historically special-cased.
#[allow(dead_code)]
fn intrinsic_descriptor(intrinsic: RegIntrinsic) -> IntrinsicDescriptor {
    use IntrinsicEffect::*;
    let d = IntrinsicDescriptor::default;
    match intrinsic {
        // --- native_subset_instruction: the single native-lowerable intrinsic ---
        // `Int.to_float` lowers to a native signed-int→f64 conversion (the single-Int
        // -arg shape check stays at the call site).
        RegIntrinsic::IntToFloat => IntrinsicDescriptor {
            effect: Pure,
            native_lowerable: true,
            notes: "native i64→f64 conversion (single Int arg)",
            ..d()
        },

        // --- Option/Result combinator expansion: the six expandable combinators ---
        RegIntrinsic::OptionMap => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::OptionMap),
            notes: "expandable pure Option combinator",
            ..d()
        },
        RegIntrinsic::OptionAndThen => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::OptionAndThen),
            notes: "expandable pure Option combinator",
            ..d()
        },
        RegIntrinsic::OptionUnwrapOr => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::OptionUnwrapOr),
            notes: "expandable pure Option combinator",
            ..d()
        },
        RegIntrinsic::ResultMap => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::ResultMap),
            notes: "expandable pure Result combinator",
            ..d()
        },
        RegIntrinsic::ResultAndThen => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::ResultAndThen),
            notes: "expandable pure Result combinator",
            ..d()
        },
        RegIntrinsic::ResultUnwrapOr => IntrinsicDescriptor {
            effect: Pure,
            can_fold: true,
            combinator_kind: Some(CombinatorKind::ResultUnwrapOr),
            notes: "expandable pure Result combinator",
            ..d()
        },

        // --- string-length fold: the foldable string producers + the length query ---
        // `String.len` is a pure byte-length READ; the pass dissolves it to arithmetic.
        RegIntrinsic::StringLen => IntrinsicDescriptor {
            effect: Read,
            can_fold: true,
            string_fold_role: Some(StringFoldRole::LengthQuery),
            notes: "byte-length query (foldable to arithmetic)",
            ..d()
        },
        // `String.from_int` allocates a fresh (always-ASCII) decimal string, but its
        // byte length is computable, so the length-fold pass can dissolve it; it is
        // also a whitelisted pure heap builder for deopt cold arms.
        RegIntrinsic::StringFromInt => IntrinsicDescriptor {
            effect: Allocate,
            can_fold: true,
            string_fold_role: Some(StringFoldRole::ProducerFromInt),
            cold_arm_pure_builder: true,
            notes: "allocates ASCII decimal string; byte length foldable; pure cold-arm builder",
            ..d()
        },
        // `String.slice` allocates a substring; foldable only when the source is
        // provably ASCII (the ASCII-gate stays in the pass).
        RegIntrinsic::StringSlice => IntrinsicDescriptor {
            effect: Allocate,
            can_fold: true,
            string_fold_role: Some(StringFoldRole::ProducerSlice),
            notes: "allocates substring; byte length foldable only when source is ASCII",
            ..d()
        },

        // --- Bytes-length fold: the foldable Bytes producers + the length query ---
        // `Bytes.len` is a pure raw-byte-length READ (`value.len()`); the Bytes fold
        // dissolves it to arithmetic. No char/grapheme subtlety — raw bytes.
        RegIntrinsic::BytesLen => IntrinsicDescriptor {
            effect: Read,
            can_fold: true,
            bytes_fold_role: Some(BytesFoldRole::LengthQuery),
            notes: "raw byte-length query (foldable to arithmetic)",
            ..d()
        },
        // `Bytes.from_string` allocates raw bytes from a String; its byte length is
        // exactly the source String's byte length (`as_bytes().len()`), so the Bytes
        // fold can dissolve it when the source length is known.
        RegIntrinsic::BytesFromString => IntrinsicDescriptor {
            effect: Allocate,
            can_fold: true,
            bytes_fold_role: Some(BytesFoldRole::ProducerFromString),
            notes: "allocates raw bytes from String; byte length = source String byte length",
            ..d()
        },
        // `Bytes.slice` allocates a byte-index substring; its length is the exact clamp
        // arithmetic of `bytes_slice` — NO ASCII gate (raw bytes have no char boundary).
        RegIntrinsic::BytesSlice => IntrinsicDescriptor {
            effect: Allocate,
            can_fold: true,
            bytes_fold_role: Some(BytesFoldRole::ProducerSlice),
            notes: "allocates byte-index substring; byte length foldable (exact clamp, no ASCII gate)",
            ..d()
        },

        // --- deopt cold-arm pure heap builders (cold_arm_pure_intrinsic) ---
        // These allocate a fresh String from read-only operands and observe/mutate
        // nothing else, so a native Bail can discard the arm and the interpreter
        // re-runs it faithfully. (`StringFromInt` above already carries can_fold.)
        RegIntrinsic::StringCopy
        | RegIntrinsic::StringFromBool
        | RegIntrinsic::StringFromFloat => IntrinsicDescriptor {
            effect: Allocate,
            cold_arm_pure_builder: true,
            notes: "pure String builder (re-runnable after a native cold-arm bail)",
            ..d()
        },

        // Everything else: conservative default (opaque allocator). Intentionally the
        // common case for the ~637 intrinsics; see the module note.
        _ => d(),
    }
}



























// Not `native-jit`-gated: the intrinsic descriptor table (always compiled, for the
// table unit test and lever-2) embeds `Option<CombinatorKind>`. Read by the
// `native-jit` combinator-expansion pass and the table unit test.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CombinatorKind {
    OptionMap,
    OptionAndThen,
    OptionUnwrapOr,
    ResultMap,
    ResultAndThen,
    ResultUnwrapOr,
}
























/// Argument positions of a closure call site that carry a `mut` effect marker
/// (`f(read u, mut ctx)`), so a `CallClosure` can write the mutated values back
/// to the caller after the closure body runs. The call-site effect is the
/// `HirExpr::Effect { ParamEffect::Mut, .. }` wrapper the checker already
/// type-checked against the stored `Fn`'s declared `mut` parameter — the same
/// information `CallKnown`/`CallNative` recover from the callee signature, but
/// for a first-class closure value the effect lives on the call-site argument.
fn call_arg_mut_positions(args: &[HirCallArg]) -> Vec<usize> {
    args.iter()
        .enumerate()
        .filter(|(_, arg)| {
            matches!(
                &arg.value,
                HirExpr::Effect {
                    effect: ParamEffect::Mut,
                    ..
                }
            )
        })
        .map(|(index, _)| index)
        .collect()
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
    reg_vm_compile_source(file, source)?.eval_main_with_args_and_native_bindings_streaming_stdout(
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
    ///
    /// The default tier-up threshold is 0 (compile on first call), which keeps
    /// the differential's full coverage. The `RSS_JIT_TIER_THRESHOLD` env var
    /// overrides it with any valid `u32`: a function then only compiles to
    /// native after being called more than that many times. This is the runtime
    /// knob for tuning/measuring tier-up (see plan §3.4) and for production
    /// deployments that want to defer native compilation for cold functions.
    /// The differential never sets it, so its behavior is unchanged.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        let tier_up_threshold = std::env::var("RSS_JIT_TIER_THRESHOLD")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0);
        self.eval_main_with_args_native_inner(
            args,
            tier_up_threshold,
            false,
            std::env::var_os("RSS_JIT_STATS").is_some(),
            false,
            false,
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
        self.eval_main_with_args_native_inner(args, 0, false, true, false, false)
    }

    /// Like [`Self::eval_main_with_args_native_osr`] but also returns the
    /// native-tier [`NativeStats`] (notably `osr_entries`) for bench telemetry.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_osr_with_stats(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner(args, 0, false, true, true, true)
    }

    /// Run `main` with the native tier AND J5.2 OSR forced on (deterministically,
    /// independent of `RSS_JIT_OSR`): a function with a qualifying native-subset
    /// hot loop runs that loop natively mid-function (OSR-entry at the header,
    /// OSR-exit/precise-resume at the post-loop ip). Must equal every other backend
    /// byte-for-byte. Test/validation + bench entry point.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_osr(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            0,
            false,
            std::env::var_os("RSS_JIT_STATS").is_some(),
            // OSR-exit resumes via the precise-deopt path, so OSR implies precise.
            true,
            true,
        )
        .map(|(output, _stats)| output)
    }

    /// Run `main` with the native tier AND J0.2 precise resume forced on,
    /// regardless of `RSS_JIT_PRECISE_DEOPT`. Native code runs for real; when it
    /// bails at a real guard safepoint, the live interpreter register window is
    /// reconstructed and interpretation resumes AT the safepoint (instead of re-
    /// running the function from the top). The observable result must equal every
    /// non-precise backend. Test/validation entry point only — lets the test set
    /// `precise_deopt` deterministically without a (racy) process env var.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_precise(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<EvalOutput, EvalError> {
        self.eval_main_with_args_native_inner(
            args,
            0,
            false,
            std::env::var_os("RSS_JIT_STATS").is_some(),
            true,
            false,
        )
        .map(|(output, _stats)| output)
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
            false,
            false,
        )
        .map(|(output, _stats)| output)
    }

    /// Test/validation entry point for the lever-2 missed-optimization report. Runs
    /// `main` with the native tier + OSR forced on AND the report armed deterministically
    /// (independent of the `RSS_JIT_REPORT` env var), returning the report block lines
    /// alongside the stats. The report is observational, so the `EvalOutput` is byte-
    /// identical to [`Self::eval_main_with_args_native_osr`]; this just also hands the
    /// caller the report so a test can assert the per-region reasons.
    #[cfg(feature = "native-jit")]
    pub fn eval_main_with_args_native_osr_report(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<(EvalOutput, NativeStats, Vec<String>), EvalError> {
        self.eval_main_with_args_native_inner_reported(args, 0, false, true, true, true, true)
    }

    #[cfg(feature = "native-jit")]
    fn eval_main_with_args_native_inner(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
        precise_deopt_override: bool,
        osr_override: bool,
    ) -> Result<(EvalOutput, NativeStats), EvalError> {
        self.eval_main_with_args_native_inner_reported(
            args,
            tier_up_threshold,
            force_bail,
            collect_stats,
            precise_deopt_override,
            osr_override,
            false,
        )
        .map(|(output, stats, _lines)| (output, stats))
    }

    #[cfg(feature = "native-jit")]
    #[allow(clippy::too_many_arguments)]
    fn eval_main_with_args_native_inner_reported(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
        precise_deopt_override: bool,
        osr_override: bool,
        report_override: bool,
    ) -> Result<(EvalOutput, NativeStats, Vec<String>), EvalError> {
        let mut vm = RegVm::new(
            Rc::clone(&self.unit),
            args.into_iter().map(Into::into).collect(),
            std::iter::empty::<(String, NativeInterpreterFn)>().collect(),
        );
        // Native first, then tier-0, then interpreter.
        // `RSS_JIT_BASELINE=1` selects the Phase-2 path-B baseline tier
        // (`opt_level="none"`); default (unset) keeps the optimizing tier
        // (`opt_level="speed"`). Only the Cranelift opt flag changes — the
        // compiled subset, host helpers, and deopt oracle are identical, so the
        // differential (which never sets this var) is undisturbed.
        let baseline = std::env::var_os("RSS_JIT_BASELINE").is_some();
        // `RSS_JIT_PRECISE_DEOPT=1` (J0.2) makes a native bail resume the
        // interpreter at the safepoint's `resume_ip` (reconstructing the live
        // register window) instead of re-running from the function top. Default
        // (unset) keeps the byte-identical re-run-from-top baseline, so the
        // differential (which never sets this var) keeps full coverage. A caller
        // may also force it on deterministically (test entry points) via
        // `precise_deopt_override`, avoiding a racy process env var.
        let precise_deopt =
            precise_deopt_override || std::env::var_os("RSS_JIT_PRECISE_DEOPT").is_some();
        // `RSS_JIT_OSR=1` (J5.2) arms on-stack replacement: a function with a
        // qualifying native-subset hot loop runs that loop natively mid-function.
        // OSR-exit resumes via the precise-deopt path, so OSR implies precise. A
        // caller may force it deterministically via `osr_override` (test/bench
        // entry). Default (unset, not overridden) leaves the OSR hook unarmed.
        let osr_enabled = osr_override || std::env::var_os("RSS_JIT_OSR").is_some();
        let precise_deopt = precise_deopt || osr_enabled;
        // `RSS_JIT_REPORT=1` (lever 2) arms the developer-facing missed-optimization
        // report: a purely observational, read-only diagnostic printed to stderr
        // after the run. It changes NO compile decision (the differential is byte-
        // identical with it on or off); when unset the report machinery is inert.
        let report = report_override || std::env::var_os("RSS_JIT_REPORT").is_some();
        vm.native = Some(NativeState::new_with_opt(
            tier_up_threshold,
            force_bail,
            collect_stats,
            baseline,
            precise_deopt,
            osr_enabled,
            report,
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
        // Lever 2: `RSS_JIT_REPORT=1` (or `report_override`) prints the per-hot-region
        // missed-optimization report (why each function/loop did or didn't go
        // native/OSR/…). Purely observational; emitted once after the run, deduped per
        // function. When armed via the env var we print to stderr; the structured lines
        // are always returned so a test/caller can assert them.
        let report_lines = if let Some(native) = &vm.native
            && native.report
        {
            let lines = jit_missed_opt_report(&self.unit, native);
            if std::env::var_os("RSS_JIT_REPORT").is_some() {
                for line in &lines {
                    eprintln!("{line}");
                }
            }
            lines
        } else {
            Vec::new()
        };
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
            report_lines,
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

    /// Run `main` under explicit sandbox resource limits ([`VmLimits`]). This is
    /// the agent-facing entry point: untrusted callers tighten the depth cap and
    /// turn on the step/memory budgets so a hostile program returns a clean
    /// `EvalError::Runtime` instead of crashing or hanging the host. Output is
    /// otherwise identical to [`Self::eval_main_with_args`].
    pub fn eval_main_with_limits(
        &self,
        args: impl IntoIterator<Item = impl Into<String>>,
        limits: VmLimits,
    ) -> Result<EvalOutput, EvalError> {
        let mut vm = RegVm::new(
            Rc::clone(&self.unit),
            args.into_iter().map(Into::into).collect(),
            std::iter::empty::<(String, NativeInterpreterFn)>().collect(),
        );
        vm.set_limits(limits);
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
    /// Whether the program can ever *observe closure identity* — i.e. whether any
    /// user-source `==`/`!=` could compare operands whose static type is, or
    /// transitively contains, a `Fn`/closure value. Computed conservatively at
    /// lower time (see [`type_name_may_contain_fn`]): `true` whenever closure
    /// identity *might* leak, `false` only when it provably cannot.
    ///
    /// When `false`, the VM may share one cached `Rc<VmClosure>` for repeated
    /// `MakeClosure` of the same non-capturing function (a refcount bump instead
    /// of a heap allocation per iteration). That share is unobservable precisely
    /// because the sole remaining identity observer is `==`/`!=` → structural
    /// `PartialEq` → `Rc::ptr_eq` (closures are not `Hashable`, so they can never
    /// be `Map`/`Set` keys — see `is_hashable`), and this flag certifies no such
    /// comparison can reach a closure. When `true`, caching is disabled and every
    /// `MakeClosure` allocates a fresh `Rc`, exactly as the compiled backend does.
    closure_identity_observable: bool,
}




/// Call count at which a function becomes "warm" and starts collecting a
/// [`FunctionProfile`] (J1). Below this threshold a function allocates and
/// records nothing — cold code pays only a single saturating `Cell<u32>`
/// increment at frame entry. Tuned high enough that one-shot/setup functions
/// never profile, low enough that a genuinely hot dispatcher is observed within
/// the first handful of native-tier warm-ups.
const PROFILE_WARMUP: u32 = 50;

/// Per-function dynamic-call count at which J1 stops sampling: once a function's
/// `call_count` reaches this, [`record_call_site`] freezes (a single `Cell`
/// read + compare, then return) so a dynamic call driven by a hot loop has an
/// essentially-free steady state. The window `PROFILE_WARMUP..PROFILE_RECORD_LIMIT`
/// is more than enough samples to settle every site's mono/poly/mega state.
const PROFILE_RECORD_LIMIT: u32 = PROFILE_WARMUP + 256;

/// Maximum number of distinct callee identities tracked at one dynamic call
/// site before it is declared megamorphic. Past this the observed list stops
/// growing (bounded memory) and [`MonoState::Megamorphic`] sticks.
const PROFILE_MAX_CALLEES: usize = 4;

/// Per-call-site monomorphism state, derived from the number of *distinct*
/// callee identities observed at a dynamic call site.
///
/// Feeds J2 monomorphic-inlining COMPILE DECISIONS ONLY; it never feeds a
/// computed value and never alters control flow or results (determinism).
// Read by the J1 tests and (forthcoming) J2 inliner; not yet consumed by
// production interpreter code, which only *writes* feedback.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonoState {
    /// Exactly one distinct callee observed so far — inlinable.
    Monomorphic,
    /// Two or three distinct callees — a small polymorphic set.
    Polymorphic,
    /// More than [`PROFILE_MAX_CALLEES`] distinct callees — not inlinable.
    Megamorphic,
}

/// Type feedback recorded at a single dynamic call site (`CallDynamic` /
/// `CallClosure`): the set of resolved callee identities and how often each was
/// seen. Counts saturate; the observed list is capped at
/// [`PROFILE_MAX_CALLEES`].
///
/// Drives J2 compile decisions ONLY — never a computed value (determinism).
#[derive(Debug, Clone)]
struct CallSiteFeedback {
    /// `(callee_key, saturating_count)` for each distinct callee, in first-seen
    /// order. `callee_key` is the callee's underlying function id (stable
    /// identity), so "same callee every time" reads as exactly one entry.
    observed: Vec<(u64, u32)>,
    /// `true` once a distinct callee beyond [`PROFILE_MAX_CALLEES`] was seen, so
    /// the site is permanently megamorphic even though `observed` is capped.
    overflowed: bool,
    /// `false` once ANY observation at this site saw a closure with a non-scalar
    /// (heap) capture. Capturing-closure inlining (OSR × J2) materializes captures
    /// as scalars via the `closure_capture` host helper, so a site that ever saw a
    /// heap capture is not eligible — the gate then leaves it on the interpreter
    /// path (no inline, no OSR). Starts `true`; ANDed monotonically downward.
    captures_all_scalar: bool,
}

impl Default for CallSiteFeedback {
    fn default() -> Self {
        CallSiteFeedback {
            observed: Vec::new(),
            overflowed: false,
            captures_all_scalar: true,
        }
    }
}

impl CallSiteFeedback {
    /// Record one observation of `callee_key` (saturating). Pure bookkeeping:
    /// has no effect on the call dispatch decision or any value.
    fn record(&mut self, callee_key: u64, captures_scalar: bool) {
        // Monotone AND: one heap-capture observation disqualifies the site forever.
        self.captures_all_scalar &= captures_scalar;
        if let Some(entry) = self.observed.iter_mut().find(|(key, _)| *key == callee_key) {
            entry.1 = entry.1.saturating_add(1);
            return;
        }
        if self.observed.len() >= PROFILE_MAX_CALLEES {
            // Bounded memory: stop growing and remember we saw more than the cap.
            self.overflowed = true;
            return;
        }
        self.observed.push((callee_key, 1));
    }

    /// Monomorphism state derived from the distinct-callee count. Read by the J1
    /// tests and the forthcoming J2 inliner.
    #[allow(dead_code)]
    fn state(&self) -> MonoState {
        if self.overflowed || self.observed.len() > PROFILE_MAX_CALLEES {
            MonoState::Megamorphic
        } else if self.observed.len() <= 1 {
            MonoState::Monomorphic
        } else {
            MonoState::Polymorphic
        }
    }
}

/// Per-function type-feedback profile (J1): feedback for each dynamic call site,
/// keyed by the site's instruction index within the function's `code`.
///
/// Allocated lazily once a function crosses [`PROFILE_WARMUP`]; cold functions
/// never allocate one. Consumed by J2 monomorphic inlining to decide what to
/// compile — it NEVER feeds a computed value and NEVER changes program behavior
/// (determinism is non-negotiable).
#[derive(Debug, Clone, Default)]
struct FunctionProfile {
    call_sites: HashMap<usize, CallSiteFeedback>,
}

impl FunctionProfile {
    /// Record `callee_key` at the dynamic call site whose instruction index is
    /// `instr_idx`. Observation only — never affects dispatch or values.
    fn record_call(&mut self, instr_idx: usize, callee_key: u64, captures_scalar: bool) {
        self.call_sites
            .entry(instr_idx)
            .or_default()
            .record(callee_key, captures_scalar);
    }
}

/// Warm-gated, bounded J1 type-feedback recording. Called ONLY from the
/// `CallDynamic`/`CallClosure` interpreter handlers (the sole sites resolving a
/// dynamic callee); nothing is added to `try_exec_pure`, the frame-entry path,
/// or any branch/loop body, so the hot dispatch path is untouched.
///
/// Cost by phase, all driven off one `Cell<u32>` (`func.call_count`):
/// - **cold** (`count < PROFILE_WARMUP`): a single saturating `Cell` bump, no
///   allocation, no `RefCell` touch. A site executed only a few times (e.g. a
///   `main` that calls a closure once) never profiles.
/// - **warm-up crossing** (`count == PROFILE_WARMUP`): allocate the
///   `FunctionProfile` once.
/// - **sampling** (`PROFILE_WARMUP <= count < PROFILE_RECORD_LIMIT`): record the
///   resolved `callee_key`.
/// - **frozen** (`count >= PROFILE_RECORD_LIMIT`): a single `Cell` read +
///   compare, then return. A site driven by a hot loop (the dynamic call IS the
///   loop body) reaches this after a fixed sample budget, so its steady state
///   costs essentially nothing — this is why it does not regress the interpreter.
///
/// `PROFILE_RECORD_LIMIT` samples are far more than enough to settle the
/// mono/poly/mega classification J2 needs. Observation only: feeds J2 compile
/// decisions, never a computed value and never control flow (determinism).
fn record_call_site(func: &RegFunction, instr_idx: usize, callee_key: u64, captures_scalar: bool) {
    let count = func.call_count.get();
    if count >= PROFILE_RECORD_LIMIT {
        // Frozen: enough samples collected. Cheapest possible steady state.
        return;
    }
    func.call_count.set(count.saturating_add(1));
    if count < PROFILE_WARMUP {
        // Still cold (one bump above is the whole cost).
        if count + 1 == PROFILE_WARMUP {
            // Allocate the profile exactly once, on the warm-up crossing.
            if let Ok(mut slot) = func.profile.try_borrow_mut() {
                if slot.is_none() {
                    *slot = Some(Box::new(FunctionProfile::default()));
                }
            }
        }
        return;
    }
    // Warm and within the sample budget: record this observation.
    if let Ok(mut slot) = func.profile.try_borrow_mut() {
        if let Some(profile) = slot.as_mut() {
            profile.record_call(instr_idx, callee_key, captures_scalar);
        }
    }
}

/// Whether every capture of `closure` is a scalar (`Int`/`Float`/`Bool`) — the
/// precondition for materializing captures into an inlined native body via the
/// `closure_capture` host helper. A non-scalar (heap) capture makes the
/// capturing-closure inline ineligible; a `Managed` wrapper is unwrapped first.
fn closure_captures_all_scalar(closure: &VmClosure) -> bool {
    closure.captures.iter().all(|c| {
        fn scalar(v: &VmValue) -> bool {
            match v {
                VmValue::Int(_) | VmValue::Float(_) | VmValue::Bool(_) => true,
                VmValue::Managed(inner) => scalar(&inner.borrow()),
                _ => false,
            }
        }
        scalar(c)
    })
}

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
    /// J1 dynamic-call counter, bumped ONLY inside [`record_call_site`] (the
    /// `CallDynamic`/`CallClosure` handlers), never on the frame-entry path. Gates
    /// the warm-up ([`PROFILE_WARMUP`]) and sampling-budget
    /// ([`PROFILE_RECORD_LIMIT`]) phases; a function with no dynamic call site
    /// never reaches the helper, so its counter stays `0` and it pays nothing.
    /// Below `PROFILE_WARMUP` no profile is allocated.
    call_count: std::cell::Cell<u32>,
    /// Lazily-allocated type-feedback profile, populated once `call_count`
    /// crosses [`PROFILE_WARMUP`]. `None` for cold functions (zero allocation).
    /// Feeds J2 compile decisions only — never a value (determinism).
    profile: RefCell<Option<Box<FunctionProfile>>>,
    /// OSR hot-backedge auto-trigger state (Pending #2). Lazily resolved ONCE on
    /// first `drive` entry into this function: a cheap single-natural-loop
    /// detection decides `NotCandidate` (no loop / unanalyzable — the common case,
    /// which then pays nothing per-instruction) vs `Counting` (has a candidate
    /// header). For a candidate, the interpreter counts backedges to the header and
    /// at [`OSR_BACKEDGE_THRESHOLD`] calls `try_osr` (the real detect+compile); on
    /// success the loop runs native and the counter cost is bounded to the warm-up
    /// iterations, on failure the state goes `GaveUp` (never retried). A `Cell`
    /// (interior-mut, no allocation), so a non-candidate function pays one `Cell`
    /// read per call and zero per-instruction cost.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    osr_state: std::cell::Cell<OsrTrigger>,
}

/// Per-function OSR auto-trigger state machine (Pending #2). Lives in a `Cell` on
/// [`RegFunction`]; see `osr_state`.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OsrTrigger {
    /// Not yet inspected — resolved on first `drive` entry.
    Unknown,
    /// No qualifying single natural loop (or step/cancel budget armed): pays only a
    /// single hoisted `Cell` read per call, NO per-instruction cost.
    NotCandidate,
    /// Has a candidate loop header; the interpreter counts backedges to `header_ip`.
    /// At [`OSR_BACKEDGE_THRESHOLD`] it fires `try_osr`. `probe_cc` is the function's
    /// dynamic-call count (`call_count`) as of the LAST `try_osr` probe (0 before the
    /// first), used to gate re-probes: a pending-profile decline only resets the counter
    /// if the profile has ADVANCED (`call_count` increased) since then — otherwise the
    /// site is dynamically dead/stalled and we `GaveUp`. `call_count` is capped at
    /// `PROFILE_RECORD_LIMIT`, so the number of progress-resets is bounded.
    Counting {
        header_ip: usize,
        count: u32,
        probe_cc: u32,
    },
    /// OSR fired (or `try_osr` declined at threshold): stop counting. `GaveUp` and
    /// `Fired` collapse to the same terminal "do nothing" behavior, but are kept
    /// distinct for telemetry/clarity.
    GaveUp,
}

/// Mirror of `OsrTrigger` that is always present (so the non-`native-jit`
/// `RegFunction` constructor compiles). Only the `native-jit` build reads it.
#[cfg(not(feature = "native-jit"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OsrTrigger {
    Unknown,
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
            call_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
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
        /// The interned shared layout (V2.0), precomputed at lowering time from the
        /// canonical `(name, field_names)` so the hot construction path is a single
        /// refcount bump — no per-construction `(name, field_names)` re-hash.
        layout: Rc<crate::vm_value::TypeLayout>,
        fields: Vec<(String, Reg)>,
    },
    ResourceDrop {
        resource: Reg,
    },
    MakeVariant {
        dst: Reg,
        /// Interned shared layout (V2.0), precomputed at lowering time (see
        /// `MakeStruct::layout`).
        layout: Rc<crate::vm_value::TypeLayout>,
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
    CallClosure {
        dst: Reg,
        closure: Reg,
        args: Vec<Reg>,
        /// Argument positions passed with `mut` (the stored `Fn`'s `mut`
        /// parameters). After the closure returns, each such argument's
        /// (possibly mutated) value is written back to the caller's argument
        /// register, so a `mut Ctx` closure parameter's field mutations
        /// propagate to the caller — identical to `CallKnown`'s `mut_args` and to
        /// AOT's `&mut` argument semantics.
        mut_args: Vec<usize>,
    },
    /// Synthetic, native-JIT-only guard (J2 profile-guided monomorphic inlining).
    /// NEVER emitted by the lowerer and NEVER executed by the interpreter — it is
    /// synthesized only inside [`native_inline_leaf_calls`], in code consumed solely
    /// by [`translate_to_native_jit`]. Lowers to a [`vm_jit::JitInstr::GuardClosureId`]:
    /// reads `closure`'s underlying function id and, if it differs from `expected`,
    /// bails to the interpreter (the existing re-run-from-top fallback). Guards an
    /// inlined monomorphic `CallClosure` so a mispredicted callee never runs native.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    NativeGuardClosureId {
        closure: Reg,
        expected: usize,
    },
    /// Synthetic, native-JIT-only id read (J2.2 polymorphic inline cache). NEVER
    /// emitted by the lowerer and NEVER executed by the interpreter — synthesized
    /// only inside [`native_inline_leaf_calls`] and consumed solely by
    /// [`translate_to_native_jit`]. Lowers to a [`vm_jit::JitInstr::ClosureId`]:
    /// reads `closure`'s underlying function id once into the `Int` register `dst`,
    /// which the dispatcher then compares against each speculated callee key with
    /// ordinary integer compare/branch instructions (the no-match arm bails via the
    /// existing re-run-from-top fallback). `closure` is a parameter handle.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    NativeClosureId {
        dst: Reg,
        closure: Reg,
    },
    /// Synthetic, native-JIT-only capture materialization (OSR × J2 capturing-
    /// closure inlining). NEVER emitted by the lowerer and NEVER executed by the
    /// interpreter — synthesized only inside [`native_inline_leaf_calls`] (right
    /// after a [`NativeGuardClosureId`]) and consumed solely by
    /// [`translate_to_native_jit`]. Lowers to a [`vm_jit::JitInstr::ClosureCapture`]:
    /// reads the scalar bits of capture `index` of the param-handle `closure` into
    /// `dst` (the inlined callee body's capture register `base + index`). A
    /// non-scalar/out-of-range capture bails out-of-band (defensive — the inline
    /// gate only fires for profiled-scalar captures).
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    NativeClosureCapture {
        dst: Reg,
        closure: Reg,
        index: usize,
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
    TensorF32ToLeBytes,
    TensorF32FromLeBytes,
    TensorMatmul,
    TensorMatmulMetal,
    TensorMetalAvailable,
    TensorMetalDeviceName,
    TensorGpuRunMsl,
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
    TensorSin,
    TensorTrunc,
    TensorPow,
    // bmm+int/bit (ops D)
    TensorBmm,
    TensorIdiv,
    TensorMod,
    TensorFloordiv,
    TensorFloormod,
    TensorShl,
    TensorShr,
    TensorAnd,
    TensorOr,
    TensorXor,
    TensorBitcastF32ToI32,
    TensorBitcastI32ToF32,
    // rng (slice E)
    TensorRand,
    TensorRandint,
    TensorRandn,
    // nn (slice F)
    TensorIota,
    TensorOneHot,
    TensorSoftmax,
    TensorLogSoftmax,
    TensorCrossEntropy,
    // conv (slice G)
    TensorConv2d,
    TensorMaxPool2d,
    TensorAvgPool2d,
    // scatter
    TensorScatterAdd,
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
    MathSaturatingAdd,
    MathSaturatingMul,
    MathSaturatingSub,
    MathSin,
    MathSqrt,
    MathTanh,
    MathTruncFloat,
    MathWrappingAdd,
    MathWrappingMul,
    MathWrappingSub,
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
        // Whole-program closure-identity gate, OR-accumulated across every
        // function (and drop-body) lowering below.
        let closure_identity_observable = std::cell::Cell::new(false);
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
                    call_count: std::cell::Cell::new(0),
                    profile: RefCell::new(None),
                    osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
                },
                loop_stack: Vec::new(),
                cleanup_stack: Vec::new(),
                closure_identity_observable: &closure_identity_observable,
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
                    call_count: std::cell::Cell::new(0),
                    profile: RefCell::new(None),
                    osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
                },
                loop_stack: Vec::new(),
                cleanup_stack: Vec::new(),
                closure_identity_observable: &closure_identity_observable,
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
        // Self-tail-call optimization: rewrite a self-tail-call (a `CallKnown`
        // to this same function whose result is returned directly) into an
        // arg-rebind + backward `Jump`, turning self-tail-recursion into a loop.
        // Run BEFORE `compute_jit_eligibility` so the now-self-edge-free function
        // is seen as non-recursive and becomes native-eligible. Conservative: only
        // genuine tail position, no `mut`-args, self-only, and never a function
        // whose ONLY exits are self-tail-calls (no non-tail return) — see
        // `optimize_self_tail_calls` for the soundness gates, including how the
        // recursion-depth-limit (`VmLimits::max_depth`) observability is preserved.
        for (function_id, function) in functions.iter_mut().enumerate() {
            optimize_self_tail_calls(function, function_id);
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
        #[cfg(feature = "native-jit")]
        mark_predictably_native_ineligible(&functions);
        Ok(Self {
            functions: functions.into_iter().map(Rc::new).collect(),
            function_ids,
            resource_drop_functions,
            types: hir
                .types()
                .map(|type_info| (type_info.name.clone(), type_info.clone()))
                .collect(),
            closure_identity_observable: closure_identity_observable.get(),
        })
    }
}

/// The static type of an HIR expression, for the closure-identity gate. `None`
/// (unknown) is treated conservatively (observable) by callers. Mirrors the
/// analyzer's `hir_expr_type_name`; kept local so the gate has no cross-module
/// dependency. A `Closure` literal operand has no `type_name`, so it returns
/// `None` and is (correctly) treated as observable.
fn reg_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { type_name, .. }
        | HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Number { value, .. } => Some(crate::hir::number_literal_type_name(value)),
        HirExpr::String { .. } => Some("String"),
        HirExpr::ObjectLiteral { type_name, .. } | HirExpr::ArrayLiteral { type_name, .. } => {
            type_name.as_deref()
        }
        HirExpr::Binary { .. }
        | HirExpr::Index { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

/// Conservatively decide whether a static type *might* be, or transitively
/// contain, a `Fn`/closure value. Returns `true` (observable) whenever it cannot
/// *prove* the type is closure-free. Soundness rests on these facts:
///   * A function type is always spelled with the substring `"Fn("` (e.g.
///     `Fn(Int) -> Int`, `noescape Fn(...)`), and generic instantiations print
///     their argument types, so `List<Fn(...)>`, `Option<Fn(...)>`, `Map<K, Fn>`
///     etc. all contain that substring — one substring test catches every type
///     whose *spelling* exposes a function type.
///   * A named user struct/sum can hide a function field behind its name; we
///     resolve it via `types` and recurse through `fields_ordered` (with a
///     visited set to terminate on recursive types). A generic struct field
///     instantiated to a function type is rejected by the equality checker
///     (Fn is not `Eq`) and, where the instantiation is visible, is spelled with
///     `"Fn("`; an uninstantiated type parameter (`"T"`) is unresolved and so
///     falls through to the conservative `true`.
///   * Anything unresolved/unknown → `true`.
fn type_name_may_contain_fn(type_name: &str, hir: &Hir) -> bool {
    fn go(name: &str, hir: &Hir, visited: &mut Vec<String>) -> bool {
        let name = name.trim();
        // Any spelled function type anywhere in the (possibly generic) type.
        if name.contains("Fn(") {
            return true;
        }
        // Scalars and known closure-free builtins: provably closure-free.
        match type_name_root(name) {
            "Int" | "Float" | "Bool" | "Char" | "String" | "Bytes" | "Unit" | "Json"
            | "JsonLiteral" => return false,
            _ => {}
        }
        // A resolved user type: recurse into its fields. The `Fn(` test above
        // already covered any generic argument spelled in `name`.
        if let Some(info) = hir.type_info(type_name_root(name)) {
            if visited.iter().any(|seen| seen == &info.name) {
                return false; // already being inspected on this path
            }
            visited.push(info.name.clone());
            let result = info
                .fields_ordered
                .iter()
                .any(|field| go(&field.type_name, hir, visited));
            visited.pop();
            return result;
        }
        // Unresolved / unknown type: conservatively assume it may contain a Fn.
        true
    }
    go(type_name, hir, &mut Vec::new())
}

/// The leading identifier of a (possibly generic) type spelling, e.g. the `List`
/// of `List<Fn(Int) -> Int>` or the bare name otherwise. Used only to classify
/// scalars and to look named user types up in the `types` table.
fn type_name_root(type_name: &str) -> &str {
    let name = type_name.trim();
    let end = name
        .find(['<', '(', ' '])
        .unwrap_or(name.len());
    name[..end].trim()
}

struct RegLowerer<'a> {
    hir: &'a Hir,
    function_ids: &'a HashMap<String, usize>,
    functions: &'a mut Vec<RegFunction>,
    function: RegFunction,
    loop_stack: Vec<LoopPatch>,
    cleanup_stack: Vec<Reg>,
    /// Accumulator for the unit-wide closure-identity gate (see
    /// [`RegUnit::closure_identity_observable`]). OR-set whenever a user
    /// `==`/`!=` could compare a closure-containing operand. Shared across all
    /// function lowerings so it summarizes the whole program.
    closure_identity_observable: &'a std::cell::Cell<bool>,
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

    /// Build a `MakeStruct` instruction with its layout interned once at lowering
    /// time (V2.0), so the runtime construction path never re-hashes
    /// `(name, field_names)`. `fields` must already be in canonical slot order.
    fn make_struct_instr(&self, dst: Reg, name: String, fields: Vec<(String, Reg)>) -> RegInstr {
        let layout = intern_struct_layout(&name, &fields);
        RegInstr::MakeStruct {
            dst,
            layout,
            fields,
        }
    }

    /// Build a `MakeVariant` instruction with its layout interned at lowering time
    /// (V2.0). See [`Self::make_struct_instr`].
    fn make_variant_instr(&self, dst: Reg, name: String, fields: Vec<(String, Reg)>) -> RegInstr {
        let layout = intern_struct_layout(&name, &fields);
        RegInstr::MakeVariant {
            dst,
            layout,
            fields,
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
                let instr = self.make_variant_instr(dst, name.clone(), Vec::new());
                self.emit(instr);
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
                // Closure-identity gate: a user `==`/`!=` is the only observer of
                // closure pointer identity. If either operand's static type might
                // be, or transitively contain, a `Fn`, sharing cached closures
                // would make distinct allocations compare equal — so flag the
                // program as identity-observable (disabling the cache). Unknown
                // operand types are treated conservatively as observable.
                if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual)
                    && !self.closure_identity_observable.get()
                {
                    let observable =
                        [left.as_ref(), right.as_ref()].iter().any(|operand| {
                            match reg_expr_type_name(operand) {
                                Some(type_name) => type_name_may_contain_fn(type_name, self.hir),
                                None => true,
                            }
                        });
                    if observable {
                        self.closure_identity_observable.set(true);
                    }
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
                            call_count: std::cell::Cell::new(0),
                            profile: RefCell::new(None),
                            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
                        },
                        loop_stack: Vec::new(),
                        cleanup_stack: Vec::new(),
                        closure_identity_observable: self.closure_identity_observable,
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
                // A bare name that resolves to a LOCAL BINDING (and is not a
                // known free function) is a first-class closure value being
                // called: `let f = r.fxn; f(7)`. Calling it dispatches through
                // `CallClosure` on the stored `VmValue::Closure`. This is the VM
                // side of first-class `owned Fn` values.
                if self.function_ids.get(type_root_name(name)).is_none()
                    && let Some(&closure) = self.function.local_regs.get(name)
                {
                    // A `mut`-annotated argument at a closure call site
                    // (`f(read u, mut ctx)`) is an exclusive borrow for the
                    // call: the closure's matching `mut` parameter may mutate it,
                    // and the result is written back to the caller's binding. The
                    // call-site effect is the `HirExpr::Effect { Mut, .. }`
                    // wrapper (already type-checked against the stored `Fn`'s
                    // declared `mut` parameter), so mirror `CallKnown`'s
                    // `mut_args` from it.
                    let mut_args = call_arg_mut_positions(args);
                    self.emit(RegInstr::CallClosure {
                        dst,
                        closure,
                        args: arg_regs,
                        mut_args,
                    });
                    return Ok(dst);
                }
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
                    let instr = self.make_variant_instr(
                        dst,
                        type_root_name(name).to_string(),
                        vec![("value".to_string(), arg_regs[0])],
                    );
                    self.emit(instr);
                } else if self
                    .hir
                    .sum_type_for_variant(type_root_name(name))
                    .is_some()
                {
                    let variant_name = type_root_name(name);
                    let fields = self.hir.sum_variant_fields(variant_name).unwrap_or(&[]);
                    match fields.len() {
                        0 if arg_regs.is_empty() => {
                            let instr =
                                self.make_variant_instr(dst, variant_name.to_string(), Vec::new());
                            self.emit(instr);
                        }
                        1 if arg_regs.len() == 1 => {
                            let instr = self.make_variant_instr(
                                dst,
                                variant_name.to_string(),
                                vec![(fields[0].name.clone(), arg_regs[0])],
                            );
                            self.emit(instr);
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
                            let instr =
                                self.make_variant_instr(dst, variant_name.to_string(), fields);
                            self.emit(instr);
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
                    let instr = self.make_struct_instr(dst, type_name, fields);
                    self.emit(instr);
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
                    // `List<T>.new()` — TV1 construction metadata. Carry the static
                    // element type so an empty `List<Int>`/`List<Float>` starts in
                    // the matching flat typed kind. The type arg is optional (a bare
                    // `List.new()` with no annotation falls back to `Boxed`).
                    RegIntrinsic::ListNew => {
                        let type_arg = type_arg_names(name)
                            .and_then(|args| args.first().copied())
                            .map(|arg| type_root_name(arg).to_string())
                            .unwrap_or_default();
                        self.emit(RegInstr::CallTypedIntrinsic {
                            dst,
                            intrinsic,
                            type_arg,
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
        ("Tensor", "f32_to_le_bytes") => Some(RegIntrinsic::TensorF32ToLeBytes),
        ("Tensor", "f32_from_le_bytes") => Some(RegIntrinsic::TensorF32FromLeBytes),
        ("Tensor", "matmul") => Some(RegIntrinsic::TensorMatmul),
        ("Tensor", "matmul_metal") => Some(RegIntrinsic::TensorMatmulMetal),
        ("Tensor", "metal_available") => Some(RegIntrinsic::TensorMetalAvailable),
        ("Tensor", "metal_device_name") => Some(RegIntrinsic::TensorMetalDeviceName),
        ("Tensor", "gpu_run_msl") => Some(RegIntrinsic::TensorGpuRunMsl),
        // Metal.* aliases (non-colliding namespace; reuse the same exec arms).
        ("Metal", "available") => Some(RegIntrinsic::TensorMetalAvailable),
        ("Metal", "device_name") => Some(RegIntrinsic::TensorMetalDeviceName),
        ("Metal", "gpu_run_msl") => Some(RegIntrinsic::TensorGpuRunMsl),
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
        ("Tensor", "sin") => Some(RegIntrinsic::TensorSin),
        ("Tensor", "trunc") => Some(RegIntrinsic::TensorTrunc),
        ("Tensor", "pow") => Some(RegIntrinsic::TensorPow),
        // bmm+int/bit (ops D)
        ("Tensor", "bmm") => Some(RegIntrinsic::TensorBmm),
        ("Tensor", "idiv") => Some(RegIntrinsic::TensorIdiv),
        ("Tensor", "modulo") => Some(RegIntrinsic::TensorMod),
        ("Tensor", "floordiv") => Some(RegIntrinsic::TensorFloordiv),
        ("Tensor", "floormod") => Some(RegIntrinsic::TensorFloormod),
        ("Tensor", "shl") => Some(RegIntrinsic::TensorShl),
        ("Tensor", "shr") => Some(RegIntrinsic::TensorShr),
        ("Tensor", "bit_and") => Some(RegIntrinsic::TensorAnd),
        ("Tensor", "bit_or") => Some(RegIntrinsic::TensorOr),
        ("Tensor", "bit_xor") => Some(RegIntrinsic::TensorXor),
        ("Tensor", "bitcast_f32_to_i32") => Some(RegIntrinsic::TensorBitcastF32ToI32),
        ("Tensor", "bitcast_i32_to_f32") => Some(RegIntrinsic::TensorBitcastI32ToF32),
        // rng (slice E)
        ("Tensor", "rand") => Some(RegIntrinsic::TensorRand),
        ("Tensor", "randint") => Some(RegIntrinsic::TensorRandint),
        ("Tensor", "randn") => Some(RegIntrinsic::TensorRandn),
        // nn (slice F)
        ("Tensor", "iota") => Some(RegIntrinsic::TensorIota),
        ("Tensor", "one_hot") => Some(RegIntrinsic::TensorOneHot),
        ("Tensor", "softmax") => Some(RegIntrinsic::TensorSoftmax),
        ("Tensor", "log_softmax") => Some(RegIntrinsic::TensorLogSoftmax),
        ("Tensor", "cross_entropy") => Some(RegIntrinsic::TensorCrossEntropy),
        // conv (slice G)
        ("Tensor", "conv2d") => Some(RegIntrinsic::TensorConv2d),
        ("Tensor", "max_pool2d") => Some(RegIntrinsic::TensorMaxPool2d),
        ("Tensor", "avg_pool2d") => Some(RegIntrinsic::TensorAvgPool2d),
        // scatter
        ("Tensor", "scatter_add") => Some(RegIntrinsic::TensorScatterAdd),
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
        ("Math", "saturating_add") => Some(RegIntrinsic::MathSaturatingAdd),
        ("Math", "saturating_mul") => Some(RegIntrinsic::MathSaturatingMul),
        ("Math", "saturating_sub") => Some(RegIntrinsic::MathSaturatingSub),
        ("Math", "sin") => Some(RegIntrinsic::MathSin),
        ("Math", "sqrt") => Some(RegIntrinsic::MathSqrt),
        ("Math", "tanh") => Some(RegIntrinsic::MathTanh),
        ("Math", "trunc_float") => Some(RegIntrinsic::MathTruncFloat),
        ("Math", "wrapping_add") => Some(RegIntrinsic::MathWrappingAdd),
        ("Math", "wrapping_mul") => Some(RegIntrinsic::MathWrappingMul),
        ("Math", "wrapping_sub") => Some(RegIntrinsic::MathWrappingSub),
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

/// Sandbox resource limits for the reg-VM. rsscript runs untrusted,
/// agent-generated code, so the VM must never crash or hang the host: deep
/// recursion, infinite loops, and runaway allocation all become recoverable
/// `EvalError::Runtime` errors instead of a SIGSEGV / hang / OOM-kill.
///
/// Defaults are tuned so trusted long-running ML training loops are unaffected:
/// the depth cap is generous (never trips real code, always catches
/// `fn f(){f()}`), and the step/memory budgets are off unless a caller opts in
/// (typically only the untrusted/agent-facing entry points).
/// Note: not `Copy` — it carries an optional `Arc<AtomicBool>` cancel flag. All
/// fields are public and `Clone`/struct-update (`..VmLimits::default()`) keep
/// callers ergonomic; the scalar budget fields are read by value as before.
#[derive(Debug, Clone)]
pub struct VmLimits {
    /// Maximum simultaneous call frames (recursion depth). Default-on and
    /// generous; checked before every frame push. `usize::MAX` effectively
    /// disables the cap.
    pub max_depth: usize,
    /// Maximum number of executed instructions over the whole run. `None`
    /// (default) = unlimited. When `Some(limit)`, a run that executes more than
    /// `limit` instructions fails with a "step budget exceeded" error — this is
    /// what stops `while true {}`.
    pub step_budget: Option<u64>,
    /// Best-effort ceiling on bytes held in VM-managed containers (register
    /// stacks + list/map growth). `None` (default) = no accounting (near-zero
    /// overhead). See [`RegVm::live_bytes`] for the accounting approximation.
    pub mem_budget: Option<usize>,
    /// Host-level preemption hook. `None` (default) = no polling (the off path is
    /// near-free: `tick()` never touches the atomic). When `Some`, the host can
    /// set the flag to `true` from anywhere (e.g. a watchdog thread on timeout or
    /// an abort signal) and the running evaluation is preempted at the next
    /// throttled step check — even inside a tight `while true {}` loop that never
    /// awaits or checks the cooperative RSS `CancellationToken`. The eval then
    /// returns `EvalError::Runtime("evaluation cancelled")`. This stops the *whole*
    /// eval; see the note in `tick()` on per-task preemption.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Maximum total bytes a program may write to captured stdout (every
    /// `Log.write`/`Debug.print`/trace path funnels through `push_stdout`). `None`
    /// (default) = unlimited. When `Some(limit)`, the write that would push the
    /// cumulative output past `limit` fails with a "stdout budget exceeded" error
    /// — this stops a program that floods the host with output rather than looping
    /// silently (which `step_budget` already catches).
    pub stdout_budget: Option<usize>,
    /// Maximum number of stdlib/runtime intrinsic calls — every `Type.method`
    /// dispatch out of pure VM bytecode into host-provided library code (the
    /// `call_intrinsic`/`call_typed_intrinsic` boundary), which is where all file /
    /// process / network / clock / logging effects (and the pure stdlib) enter.
    /// `None` (default) = uncounted. When `Some(limit)`, the call that would exceed
    /// `limit` fails with a "host call budget exceeded" error. This caps the volume
    /// of host-library calls independently of raw instruction count (a single
    /// intrinsic can do unbounded I/O), so an agent program can be limited to N
    /// effectful operations even if each is individually cheap in `step_budget`
    /// terms.
    pub host_call_budget: Option<u64>,
}

/// Default recursion-depth cap: generous enough never to trip real code (deep
/// but finite recursion, the ML framework's call chains) yet finite, so an
/// unbounded self-recursive program is caught long before it can overflow the
/// native stack.
const DEFAULT_MAX_DEPTH: usize = 16_384;

/// How often `tick()` polls the ambient cancel flag (once every this many
/// instructions). A power of two so the modulo lowers to a mask. Small enough
/// that a watchdog preempts a tight loop within microseconds, large enough that
/// the relaxed atomic load is negligible amortized over real work.
const CANCEL_POLL_INTERVAL: u64 = 1024;

/// Estimated bytes charged per map entry: a key plus a `VmValue`, with hashmap
/// bookkeeping folded into the key term as a rough fudge factor.
const MAP_ENTRY_BYTES: usize = std::mem::size_of::<VmValue>() * 2;

impl Default for VmLimits {
    fn default() -> Self {
        Self {
            max_depth: DEFAULT_MAX_DEPTH,
            step_budget: None,
            mem_budget: None,
            cancel: None,
            stdout_budget: None,
            host_call_budget: None,
        }
    }
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
    /// Sandbox resource limits (recursion depth / step budget / memory ceiling).
    /// Defaults leave trusted runs unaffected; agent-facing callers tighten them.
    limits: VmLimits,
    /// Instructions executed so far in this run (the step budget's fuel gauge).
    /// Only consulted when `limits.step_budget` is `Some`; the unconditional
    /// increment is the entire overhead when the budget is off.
    steps: u64,
    /// Best-effort running estimate of bytes held in VM-managed containers.
    /// Approximation: we add the estimated size of *growth* (register-stack
    /// resizes and list/map element/entry additions) and do NOT subtract frees,
    /// so this is a cumulative high-water-ish figure, not a precise live-set. It
    /// exists only to trip `limits.mem_budget`; when that is `None` we skip all
    /// accounting so the overhead is zero. Accounted sites: `ensure_regs`,
    /// `MakeList`/`MakeMap` literal construction, and the `ListPush`/`ListAppend`
    /// growth handlers (the dominant allocators for adversarial blow-ups).
    live_bytes: usize,
    /// Number of stdlib/runtime intrinsic calls dispatched so far (the
    /// `host_call_budget` fuel gauge). Only consulted when that budget is `Some`;
    /// the unconditional increment is the entire overhead when it is off.
    host_calls: u64,
    /// Native (Cranelift) JIT state, `Some` when the native tier is enabled. The
    /// native tier compiles the integer/control core to machine code and is tried
    /// before the tier-0 executor; anything it can't compile (or bails on) falls
    /// back to tier-0 / the interpreter.
    #[cfg(feature = "native-jit")]
    native: Option<NativeState>,
    /// Cache of canonical non-capturing closures, indexed by function id. A
    /// `MakeClosure` with no captures builds `VmClosure { function, captures: [] }`
    /// — a value that is *identical* for a given function on every execution — so
    /// after the first allocation we hand out clones of the same `Rc` (a refcount
    /// bump) instead of allocating a fresh one each loop iteration.
    ///
    /// SOUNDNESS: sharing one `Rc` makes previously-distinct allocations compare
    /// equal under `Rc::ptr_eq`, which is observable ONLY through `==`/`!=` on a
    /// closure (closures are not `Hashable`, so never `Map`/`Set` keys). The cache
    /// is therefore populated only when `unit.closure_identity_observable` is
    /// `false`, i.e. the whole program provably never compares a closure-bearing
    /// value. When it is `true` the cache stays empty and every `MakeClosure`
    /// allocates fresh, matching the compiled backend bit-for-bit. `VmClosure` is
    /// immutable after construction (its `captures` Vec is never mutated in
    /// place — verified by grep), so a shared `Rc` can never diverge.
    noncapturing_closure_cache: Vec<Option<Rc<VmClosure>>>,
}

/// Outcome of a [`RegVm::try_native`] attempt.
///
/// `Completed` carries the native result (the caller finishes the frame exactly
/// like the `Return` arm). `Resumed` means a native bail was reconstructed into
/// the interpreter at the safepoint's `resume_ip` (J0.2, only under the
/// `precise_deopt` flag): the live register window has been restored and the
/// frame's `ip` advanced, so the caller just re-enters the interpreter loop.
/// `Fallback` means native did not produce a value (ineligible, arg mismatch, or
/// a bail that precise resume didn't apply): the frame `ip` is still `0`, so the
/// caller re-runs the function from the top on the interpreter — the safe,
/// behavior-preserving default.
#[cfg(feature = "native-jit")]
enum NativeAttempt {
    Completed(VmValue),
    Resumed,
    Fallback,
}

/// State for the native JIT tier: the Cranelift module owning the compiled code,
/// a per-function cache (`None` = known not native-eligible), and the tiering /
/// deopt knobs.
#[cfg(feature = "native-jit")]
struct NativeState {
    module: vm_jit::NativeModule,
    // `None` = known not native-eligible; `Some((id, ret, params, has_backedge))`
    // = compiled handle, return type (to box the 64-bit result), parameter types
    // (to unbox each argument: `Int`/`Bool` from their VM value, `Float` as bits),
    // and whether the function's body contains an internal back-edge (a loop). The
    // back-edge bit drives the no-amortization profitability gate
    // (`NATIVE_NOAMORTIZE_GIVEUP`): a loop-free body dispatched per loop iteration
    // can never amortize FFI cost, so it is demoted after `K` dispatches.
    #[allow(clippy::type_complexity)]
    cache: HashMap<usize, Option<(vm_jit::CompiledId, NativeTy, Vec<NativeTy>, bool)>>,
    /// Per-function call counts, for tiering: a function is compiled and run
    /// natively only once it has been entered more than `tier_up_threshold` times
    /// (a hot-function heuristic). `0` means "compile on first call" (force-all).
    counts: HashMap<usize, u32>,
    /// Per-function *consecutive* runtime-bail counts, keyed like `counts`/`cache`.
    /// Incremented on every bail after native was chosen (arg mismatch or runtime
    /// guard), reset to 0 on a successful native completion. At
    /// `NATIVE_BAIL_GIVEUP_THRESHOLD` the function is demoted to `NOT_ELIGIBLE` and
    /// dropped from `cache`, so the predict-and-skip path stops the wasted
    /// compile-marshal-bail churn (vm-jit-perf-plan §3.0).
    bail_counts: HashMap<usize, u32>,
    /// Per-function count of *native dispatches of a back-edge-free body*, keyed
    /// like `counts`/`cache`. Only loop-free bodies are counted here; at
    /// `NATIVE_NOAMORTIZE_GIVEUP` the function is demoted to `NOT_ELIGIBLE` and
    /// dropped from `cache` (the no-amortization profitability gate). Loop-bearing
    /// bodies are never inserted, so they are never demoted by this counter.
    noamortize_counts: HashMap<usize, u32>,
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
    /// J0.2 precise deopt: when set, a native bail at a known safepoint
    /// reconstructs the interpreter register window from the captured live values
    /// and resumes interpretation AT the safepoint's `resume_ip`, instead of
    /// re-running the function from the top. Default `false` ⇒ byte-identical
    /// re-run-from-top (the safe baseline). Wired from `RSS_JIT_PRECISE_DEOPT`.
    precise_deopt: bool,
    /// J5.2 OSR (on-stack replacement): when set, a function with a qualifying
    /// native-subset hot loop (see [`detect_single_natural_loop`]) runs that loop
    /// natively *mid-function* — the interpreter reaches the loop header, hands the
    /// register window to an OSR-compiled loop body, then resumes at the post-loop
    /// ip with the live-out window (OSR-exit / precise-deopt resume). Forced trigger
    /// only (fires whenever the interpreter reaches a qualifying header); wired from
    /// `RSS_JIT_OSR` or a deterministic test override. Default `false` ⇒ the OSR
    /// hook is never armed and the interpreter hot path is untouched.
    osr_enabled: bool,
    /// Per-function OSR compile cache, keyed like `cache`. `Some((id, loop, params))`
    /// is a compiled OSR-entry handle plus the loop it covers and the live-in param
    /// types (for window marshalling); `None` means "known not OSR-eligible" (don't
    /// re-analyze). Populated lazily the first time the interpreter reaches a header.
    #[allow(clippy::type_complexity)]
    osr_cache: HashMap<usize, Option<OsrEntry>>,
    /// Reusable per-call marshalling scratch buffers (TV2 arg/len words and the
    /// flat-list `Rc` keep-alive set). Held here and `mem::take`n into the call
    /// frame so a hot per-iteration native dispatch (e.g. a tiny leaf/closure
    /// called once per loop iteration) does not heap-allocate three `Vec`s on
    /// every call — that per-call allocation churn, not the native body, is what
    /// made marginal closure/leaf kernels slower than the interpreter.
    scratch_args: Vec<i64>,
    scratch_lens: Vec<i64>,
    scratch_flat_owned: Vec<Rc<RefCell<TypedVec>>>,
    /// Lever 2: `RSS_JIT_REPORT` missed-optimization report armed. Read ONCE from
    /// the env at construction (mirrors `collect_stats`), so the hot path pays only
    /// a single hoisted bool read. When `false` the report machinery does nothing —
    /// no allocation, no recording, no print. Purely observational: it never gates
    /// any compile decision (the differential proves byte-identical behavior on/off).
    report: bool,
    /// Per-function set of `native_key`s that actually ran natively to completion at
    /// least once this run. Populated ONLY when `report` is on (gated like the
    /// stats counters). Lets the report print an accurate `native: ok` positive that
    /// matches the real runtime outcome (vs the static eligibility re-derivation).
    report_native_ok: std::collections::HashSet<usize>,
    /// Per-function set of `native_key`s that actually OSR-entered at least once.
    /// Populated ONLY when `report` is on. Accurate positive for `osr: entered`.
    report_osr_ok: std::collections::HashSet<usize>,
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
    /// J5.2: OSR-entries that ran a loop natively mid-function and resumed at the
    /// post-loop ip (the forced-trigger success count).
    pub osr_entries: u64,
}

#[cfg(feature = "native-jit")]
impl NativeStats {
    fn summary(&self) -> String {
        format!(
            "native-jit: considered={} translated={} compiled={} not_eligible={} \
compile_failed={} calls={} bails={} arg_mismatch={} tier_deferred={} \
compile_ms={:.3} run_ms={:.3} osr_entries={}",
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
            self.osr_entries,
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
            "osr_entries": self.osr_entries,
        })
    }
}

/// Lever 2: the developer-facing missed-optimization report (`RSS_JIT_REPORT`).
///
/// Walks every function in `unit` and re-derives — **observationally, read-only** —
/// why each did or didn't go native / OSR / scalar-replace / inline / fold, with the
/// intrinsic-level reasons sourced from the central [`intrinsic_descriptor`] registry
/// (effect + notes). This RE-RUNS the same cheap predicates the real passes use
/// (`translate_to_native_jit`, `detect_single_natural_loop`, `native_subset_instruction`,
/// `native_inline_leaf_calls`) WITHOUT touching the passes themselves, so it cannot
/// change any compile decision — the proof is the byte-identical differential with the
/// report on or off. Positive verdicts (`native: ok`, `osr: entered`) are cross-checked
/// against the actual runtime outcome recorded in `report_native_ok` / `report_osr_ok`,
/// so a line the report prints as "ok"/"entered" really happened, and a "not …" line
/// really did not (the report-correctness tests assert this).
///
/// One block per function (deduped by construction — each function is visited once).
#[cfg(feature = "native-jit")]
fn jit_missed_opt_report(unit: &RegUnit, native: &NativeState) -> Vec<String> {
    let mut out = Vec::new();
    for func in &unit.functions {
        // Skip the synthetic/placeholder/trivial bodies: a body that is only the
        // lowerer's defensive `LoadUnit; Return` (≤ 2 instructions, no real work) is
        // not a "hot region" worth a block. Everything with real code gets one.
        if func.code.len() <= 2 {
            continue;
        }
        let key = Rc::as_ptr(func) as usize;
        let mut block = vec![format!("jit-report: fn `{}`", func.name)];

        // --- Native-tier verdict --------------------------------------------------
        match translate_to_native_jit(unit, func) {
            Some(_) => {
                if native.report_native_ok.contains(&key) {
                    block.push("  native: ok".to_string());
                } else {
                    // Statically eligible but never observed running natively this
                    // run (tier-deferred, not called hot, or demoted by another gate).
                    block.push("  native: eligible (not run natively this execution)".to_string());
                }
            }
            None => {
                block.push(format!("  not native: {}", native_decline_reason(unit, func)));
            }
        }

        // --- OSR verdict ----------------------------------------------------------
        // ACCURACY FIRST: if the function actually OSR-entered this run, the verdict is
        // `osr: entered` regardless of any static re-derivation — the recorded runtime
        // outcome is ground truth. (The OSR pipeline applies several region transforms —
        // combinator expansion, leaf inlining, string-length folding, Option/Result/
        // variant/struct scalar replacement — before the subset check, so a body with a
        // *raw* allocating string/Option op can still OSR once those passes dissolve it;
        // re-deriving that whole pipeline here would be fragile, so we trust the outcome
        // for the positive and use the cheap static re-derivation only to EXPLAIN a
        // genuine non-entry.)
        if native.report_osr_ok.contains(&key) {
            block.push("  osr: entered".to_string());
        } else {
            match detect_single_natural_loop(&func.code) {
                None => {
                    if jit_function_has_loop(&func.code) {
                        block.push(
                            "  not osr: loop shape not a single reducible natural loop"
                                .to_string(),
                        );
                    } else {
                        block.push("  not osr: no loop".to_string());
                    }
                }
                Some(lp) => {
                    // A candidate loop exists but it did not OSR. Surface the first
                    // disqualifier in the RAW loop body (registry-sourced for
                    // intrinsics) as the likely cause; if the raw body is already in
                    // the native subset, the decline was a downstream
                    // type/marshalling reason.
                    match first_non_subset_reason(&func.code[lp.header..lp.exit]) {
                        Some(reason) => block.push(format!("  not osr: loop body {reason}")),
                        None if native.report_native_ok.contains(&key) => block.push(
                            "  osr: n/a (whole function ran native; no mid-function OSR needed)"
                                .to_string(),
                        ),
                        None => block.push(
                            "  not osr: loop not lowered (type/marshalling decline)".to_string(),
                        ),
                    }
                }
            }
        }

        out.push(block.join("\n"));
    }
    out
}

/// Re-derive, observationally, the first reason whole-function native translation
/// declines `func`. Mirrors the early bails in [`translate_to_native_jit`] and then
/// scans the (leaf-inlined) reachable body for the first non-subset instruction —
/// reporting the intrinsic-level cause from the registry. Read-only.
#[cfg(feature = "native-jit")]
fn native_decline_reason(unit: &RegUnit, func: &RegFunction) -> String {
    if func.captures != 0 {
        return "function has captures (closure body, not a native leaf)".to_string();
    }
    // Re-run leaf inlining + Option scalar-replacement exactly as translation does, so
    // the reason reflects the FINAL body the native subset check sees. If either bails,
    // report that — these are the structural reasons (un-inlinable call / escaping
    // Option) the real pass declines on.
    let Some((code, _n_regs, _ip_map)) = native_inline_leaf_calls(unit, func, false, None) else {
        return "contains a non-inlinable call (callee not native-inlinable)".to_string();
    };
    let Some((code, _n_regs, _payload, _ip_map)) = native_scalar_replace_options(&code, _n_regs)
    else {
        return "not scalar-replaced: Option/variant/struct escapes the region".to_string();
    };
    let reachable = native_reachable_instructions(&code);
    for (i, instr) in code.iter().enumerate() {
        if reachable[i]
            && !native_subset_instruction(instr)
            && let Some(reason) = instr_decline_reason(instr)
        {
            return reason;
        }
    }
    // Translation declined for a shape reason the above re-derivation doesn't pinpoint
    // (e.g. type unification conflict, param/reg count). Generic but honest.
    "outside the native subset (shape/type not lowerable)".to_string()
}

/// A report reason for why `body` is outside the native subset. Prefers the most
/// *substantive* cause — a non-pure (allocate/write/suspend/read) `CallIntrinsic`,
/// whose registry effect/notes are the real missed-opt explanation — over an
/// incidental non-subset instruction (e.g. a `LoadString` constant load that the
/// subset also rejects). Falls back to the first non-subset instruction otherwise.
/// `None` ⇒ the whole body is in the native subset.
#[cfg(feature = "native-jit")]
fn first_non_subset_reason(body: &[RegInstr]) -> Option<String> {
    // First: a non-subset effectful intrinsic (the headline reason).
    if let Some(instr) = body.iter().find(|instr| {
        !native_subset_instruction(instr)
            && matches!(
                instr,
                RegInstr::CallIntrinsic { .. } | RegInstr::CallTypedIntrinsic { .. }
            )
            && match instr {
                RegInstr::CallIntrinsic { intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, .. } => {
                    intrinsic_descriptor(*intrinsic).effect != IntrinsicEffect::Pure
                }
                _ => false,
            }
    }) {
        return instr_decline_reason(instr);
    }
    // Otherwise: the first non-subset instruction, whatever it is.
    body.iter()
        .find(|instr| !native_subset_instruction(instr))
        .map(|instr| instr_decline_reason(instr).unwrap_or_else(|| "outside native subset".into()))
}

/// Human-readable reason a single instruction is outside the native subset, with
/// the intrinsic-level effect/notes pulled from the central [`intrinsic_descriptor`]
/// registry for `CallIntrinsic`/`CallTypedIntrinsic`. `None` for a subset instruction.
#[cfg(feature = "native-jit")]
fn instr_decline_reason(instr: &RegInstr) -> Option<String> {
    if native_subset_instruction(instr) {
        return None;
    }
    Some(match instr {
        RegInstr::CallIntrinsic { intrinsic, .. }
        | RegInstr::CallTypedIntrinsic { intrinsic, .. } => {
            let d = intrinsic_descriptor(*intrinsic);
            let effect = match d.effect {
                IntrinsicEffect::Pure => "pure",
                IntrinsicEffect::Read => "read",
                IntrinsicEffect::Allocate => "allocate",
                IntrinsicEffect::Write => "write",
                IntrinsicEffect::Suspend => "suspend",
            };
            format!(
                "contains CallIntrinsic {:?} (effect={}; {})",
                intrinsic, effect, d.notes
            )
        }
        RegInstr::CallClosure { .. } => {
            "contains a closure call (megamorphic / not native-inlinable)".to_string()
        }
        RegInstr::CallKnown { .. } | RegInstr::CallDynamic { .. } => {
            "contains a non-inlined call".to_string()
        }
        other => {
            // A non-call, non-subset instruction (heap construct, async, float-only
            // op the subset rejects, …). Name the opcode for the developer.
            let dbg = format!("{other:?}");
            let opcode = dbg.split([' ', '{']).next().unwrap_or("?");
            format!("contains {opcode} (outside native scalar/control subset)")
        }
    })
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

    /// Heap-result return ABI (heap-write S0): the per-call VM-owned **output table**
    /// from which the host materializes a native call's heap result. Mirrors
    /// `JIT_HEAP_ARGS` (the input table): VM-owned, per-call, indexed by an opaque
    /// handle, and cleared on EVERY exit by `JitHeapResultsGuard`.
    ///
    /// §7.2-safety: the host populates this table and materializes from it **only**
    /// after a clean `NativeOutcome::CompletedHandle` (bail flag clear). On **any**
    /// bail the host never touches it and the guard clears it on exit, so a bailed
    /// attempt leaves NO value here — the interpreter re-run produces the result
    /// itself, indistinguishable from never having attempted native. S0 does not let
    /// native allocate/mutate; it only *returns a heap value it was given*, so no
    /// observable effect precedes a possible bail and the §7.2 proof holds unchanged.
    static JIT_HEAP_RESULTS: RefCell<Vec<VmValue>> = const { RefCell::new(Vec::new()) };
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

/// Clears the per-call heap-**result** output table on drop (heap-write S0), so a
/// native attempt — clean OR bailed — never leaks a heap result past the call. This
/// is the §7.2 belt-and-suspenders for the output table: even on a bail (where the
/// host never populates it) the table is guaranteed empty for the next attempt, so
/// no stale value can be double-materialized.
#[cfg(feature = "native-jit")]
struct JitHeapResultsGuard;

#[cfg(feature = "native-jit")]
impl Drop for JitHeapResultsGuard {
    fn drop(&mut self) {
        JIT_HEAP_RESULTS.with(|table| table.borrow_mut().clear());
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
        field_float: rss_jit_field_float,
        list_get_float: rss_jit_list_get_float,
        closure_id: rss_jit_closure_id,
        closure_capture: rss_jit_closure_capture,
        field_handle: rss_jit_field_handle,
        list_get_handle: rss_jit_list_get_handle,
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
                VmValue::Int(v) => Some(v),
                _ => None,
            }
        }
        VmValue::Managed(inner) => jit_list_get_int(&inner.borrow(), index),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_struct_field_float(value: &VmValue, slot: usize) -> Option<f64> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => match data.fields.get(slot)? {
            VmValue::Float(v) => Some(*v),
            _ => None,
        },
        VmValue::Managed(inner) => jit_struct_field_float(&inner.borrow(), slot),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn jit_list_get_float(value: &VmValue, index: i64) -> Option<f64> {
    match value {
        VmValue::List(list) => {
            let index = usize::try_from(index).ok()?;
            match list.borrow().get(index)? {
                VmValue::Float(v) => Some(v),
                _ => None,
            }
        }
        VmValue::Managed(inner) => jit_list_get_float(&inner.borrow(), index),
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
extern "C" fn rss_jit_field_float(handle: i64, slot: i64) -> f64 {
    match usize::try_from(slot)
        .ok()
        .and_then(|slot| jit_heap_read(handle, |value| jit_struct_field_float(value, slot)))
    {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0.0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_get_float(handle: i64, index: i64) -> f64 {
    match jit_heap_read(handle, |value| jit_list_get_float(value, index)) {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0.0
        }
    }
}

/// The underlying function id of the closure behind `handle`, as `i64`. Used by the
/// J2 monomorphic-inlining guard ([`vm_jit::JitInstr::GuardClosureId`]). Total: a
/// non-closure / invalid handle, or a function id too large for `i64`, returns `-1`,
/// which never equals a real (`>= 0`) callee id, so the guard simply bails. Never
/// signals the out-of-band bail flag.
#[cfg(feature = "native-jit")]
fn jit_closure_function_id(value: &VmValue) -> Option<i64> {
    match value {
        VmValue::Closure(closure) => i64::try_from(closure.function).ok(),
        VmValue::Managed(inner) => jit_closure_function_id(&inner.borrow()),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_closure_id(handle: i64) -> i64 {
    jit_heap_read(handle, jit_closure_function_id).unwrap_or(-1)
}

/// The scalar bits of capture `index` of the closure behind `handle`, as `i64` (an
/// `Int` directly, a `Float` reinterpreted via [`f64::to_bits`], a `Bool` as 0/1).
/// Used by the capturing-closure inline support
/// ([`vm_jit::JitInstr::ClosureCapture`]) to materialize a scalar capture into the
/// inlined callee body. A non-scalar (heap) capture, an out-of-range index, or a
/// non-closure handle signals the out-of-band bail flag — defensive, since the
/// producer only emits `ClosureCapture` for captures it proved scalar.
#[cfg(feature = "native-jit")]
fn jit_closure_capture_scalar(value: &VmValue, index: usize) -> Option<i64> {
    match value {
        VmValue::Closure(closure) => match closure.captures.get(index)? {
            VmValue::Int(v) => Some(*v),
            VmValue::Float(v) => Some(v.to_bits() as i64),
            VmValue::Bool(b) => Some(i64::from(*b)),
            _ => None,
        },
        VmValue::Managed(inner) => jit_closure_capture_scalar(&inner.borrow(), index),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_closure_capture(handle: i64, index: i64) -> i64 {
    match usize::try_from(index)
        .ok()
        .and_then(|index| jit_heap_read(handle, |value| jit_closure_capture_scalar(value, index)))
    {
        Some(value) => value,
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

/// A clone of the struct/variant field `slot` IF it is itself a heap value (a
/// stored closure, struct, variant, or list) — the only fields a `FieldHandle`
/// read is allowed to fetch as a fresh handle. A scalar/absent field returns
/// `None` (→ bail), so a misclassified slot never produces a bogus handle.
#[cfg(feature = "native-jit")]
fn jit_struct_field_heap_value(value: &VmValue, slot: usize) -> Option<VmValue> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            let field = data.fields.get(slot)?;
            jit_heap_value_clone(field)
        }
        VmValue::Managed(inner) => jit_struct_field_heap_value(&inner.borrow(), slot),
        _ => None,
    }
}

/// A clone of list element `index` IF it is itself a heap value (e.g. a struct
/// holding a stored closure). A scalar/absent element returns `None` (→ bail).
#[cfg(feature = "native-jit")]
fn jit_list_get_heap_value(value: &VmValue, index: i64) -> Option<VmValue> {
    match value {
        VmValue::List(list) => {
            let index = usize::try_from(index).ok()?;
            let elem = list.borrow().get(index)?;
            jit_heap_value_clone(&elem)
        }
        VmValue::Managed(inner) => jit_list_get_heap_value(&inner.borrow(), index),
        _ => None,
    }
}

/// `Some(clone)` only when `value` is a heap value the host helpers can read
/// through a handle (closure/struct/variant/list, transparently unwrapping a
/// `Managed` cell). A scalar is `None`: handles only ever name heap values.
#[cfg(feature = "native-jit")]
fn jit_heap_value_clone(value: &VmValue) -> Option<VmValue> {
    match value {
        VmValue::Closure(_)
        | VmValue::Struct(_)
        | VmValue::Variant(_)
        | VmValue::List(_)
        | VmValue::Managed(_) => Some(value.clone()),
        _ => None,
    }
}

/// Push a freshly-fetched heap value into the per-call handle table and return its
/// index, or signal the standard re-run-from-top bail (returning 0) when the field/
/// element was not a heap value the helper could fetch.
#[cfg(feature = "native-jit")]
fn jit_push_heap_handle(value: Option<VmValue>) -> i64 {
    match value {
        Some(value) => JIT_HEAP_ARGS.with(|table| {
            let mut table = table.borrow_mut();
            table.push(value);
            (table.len() - 1) as i64
        }),
        None => {
            vm_jit::signal_bail();
            0
        }
    }
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_field_handle(handle: i64, slot: i64) -> i64 {
    let value = usize::try_from(slot)
        .ok()
        .and_then(|slot| jit_heap_read(handle, |value| jit_struct_field_heap_value(value, slot)));
    jit_push_heap_handle(value)
}

#[cfg(feature = "native-jit")]
extern "C" fn rss_jit_list_get_handle(handle: i64, index: i64) -> i64 {
    let value = jit_heap_read(handle, |value| jit_list_get_heap_value(value, index));
    jit_push_heap_handle(value)
}

#[cfg(feature = "native-jit")]
impl NativeState {
    // Used by the in-crate `#[cfg(test)]` unit tests; the optimizing/baseline
    // production paths go through `new_with_opt`, so the lib-only build sees no
    // caller. Keep the back-compat 3-arg constructor without a dead-code warning.
    #[allow(dead_code)]
    fn new(
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
    ) -> Result<Self, EvalError> {
        Self::new_with_opt(tier_up_threshold, force_bail, collect_stats, false, false, false, false)
    }

    /// Build the native state at a selectable optimization level. `baseline ==
    /// true` selects the Phase-2 path-B baseline tier (`opt_level="none"`):
    /// faster compiles, identical observable behavior (same IR, same host
    /// helpers, same deopt protocol — only the Cranelift opt flag differs). The
    /// compiled subset is unchanged (side-effect-free scalar + read-only heap),
    /// so the interpreter/`run_jit` deopt oracle stays valid verbatim.
    fn new_with_opt(
        tier_up_threshold: u32,
        force_bail: bool,
        collect_stats: bool,
        baseline: bool,
        precise_deopt: bool,
        osr_enabled: bool,
        report: bool,
    ) -> Result<Self, EvalError> {
        Ok(Self {
            module: vm_jit::NativeModule::new_with_opt(jit_host_helpers(), baseline)
                .map_err(|e| EvalError::Runtime(e.to_string()))?,
            cache: HashMap::new(),
            counts: HashMap::new(),
            bail_counts: HashMap::new(),
            noamortize_counts: HashMap::new(),
            tier_up_threshold,
            force_bail,
            stats: NativeStats::default(),
            collect_stats,
            precise_deopt,
            osr_enabled,
            osr_cache: HashMap::new(),
            scratch_args: Vec::new(),
            scratch_lens: Vec::new(),
            scratch_flat_owned: Vec::new(),
            report,
            report_native_ok: std::collections::HashSet::new(),
            report_osr_ok: std::collections::HashSet::new(),
        })
    }

    /// Records a consecutive runtime bail for a structurally-eligible function
    /// (called after native was chosen, on either an arg-type mismatch or a guard
    /// bail). At [`NATIVE_BAIL_GIVEUP_THRESHOLD`] consecutive bails the function is
    /// permanently demoted: `native_status` is set to `NOT_ELIGIBLE` (so the
    /// cheap-negative early-return in `try_native` short-circuits all future calls)
    /// and its compiled entry is dropped from the cache to free the code. Reusing
    /// `NOT_ELIGIBLE` is correct here: its only meaning is "don't attempt native",
    /// which is exactly the give-up verdict.
    fn record_bail(&mut self, native_key: usize, func: &RegFunction) {
        let count = self.bail_counts.entry(native_key).or_insert(0);
        *count += 1;
        if *count >= NATIVE_BAIL_GIVEUP_THRESHOLD {
            func.native_status.set(NATIVE_STATUS_NOT_ELIGIBLE);
            self.cache.remove(&native_key);
            self.bail_counts.remove(&native_key);
        }
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
            limits: VmLimits::default(),
            steps: 0,
            live_bytes: 0,
            host_calls: 0,
            #[cfg(feature = "native-jit")]
            native: None,
            noncapturing_closure_cache: Vec::new(),
        }
    }

    /// Return the canonical non-capturing `Rc<VmClosure>` for `function`,
    /// allocating it once on first use and cloning the `Rc` (refcount bump)
    /// thereafter. Only called when `unit.closure_identity_observable` is `false`,
    /// so the resulting pointer-sharing is unobservable (see the field doc on
    /// `noncapturing_closure_cache`).
    fn cached_noncapturing_closure(&mut self, function: usize) -> Rc<VmClosure> {
        if function >= self.noncapturing_closure_cache.len() {
            self.noncapturing_closure_cache.resize(function + 1, None);
        }
        if let Some(existing) = &self.noncapturing_closure_cache[function] {
            return Rc::clone(existing);
        }
        let closure = Rc::new(VmClosure {
            function,
            captures: Vec::new(),
        });
        self.noncapturing_closure_cache[function] = Some(Rc::clone(&closure));
        closure
    }

    /// Apply sandbox resource limits to this VM before it runs. Replaces the
    /// defaults wholesale (default construction already uses [`VmLimits::default`]
    /// — depth cap on, step/memory budgets off — so existing callers are
    /// unaffected and only agent-facing entry points need call this).
    fn set_limits(&mut self, limits: VmLimits) {
        self.limits = limits;
    }

    /// Push a call frame, enforcing the recursion-depth cap first. `frames.len()`
    /// is the current depth; a successful push would make it `len + 1`, so we
    /// reject when that would exceed `limits.max_depth`. Centralizes the check so
    /// every frame-push site (sync `run_frame`, `CallKnown`, `CallDynamic`) is
    /// covered identically. Returns the depth error as a value, never panics.
    fn push_frame(&mut self, frame: Frame) -> Result<(), EvalError> {
        if self.frames.len() + 1 > self.limits.max_depth {
            let max_depth = self.limits.max_depth;
            return Err(EvalError::Runtime(format!(
                "recursion depth limit exceeded ({max_depth} frames)"
            )));
        }
        self.frames.push(frame);
        Ok(())
    }

    /// Charge one instruction against the step budget. Always increments the
    /// fuel gauge (the single unconditional add is the whole cost when the budget
    /// is off), and — only when `limits.step_budget` is `Some` — trips once the
    /// count exceeds the limit. This is what stops an infinite loop (`while true
    /// {}`) from hanging the host: it returns a clean error instead.
    ///
    /// It is also the host-level *preemption* hook. When `limits.cancel` is
    /// `Some`, every `CANCEL_POLL_INTERVAL` steps we load the ambient
    /// `AtomicBool` (`Relaxed` — we only need eventual visibility, not ordering)
    /// and, if set, abort the eval with `EvalError::Runtime("evaluation
    /// cancelled")`. The throttle keeps both the off path (no atomic touched at
    /// all) and the on path (one relaxed load per 1024 instructions) cheap, so a
    /// tight loop stays fast while still being interruptible by a watchdog.
    ///
    /// Limitation: this stops the *entire* evaluation. Preemptively cancelling a
    /// single *sibling* task stuck in a tight loop — so a `select`/`task_group`
    /// can reach its winner while one branch spins — would require the scheduler
    /// to yield mid-instruction-stream (snapshot a `SavedTask` at an arbitrary
    /// `ip` and reschedule), a deeper redesign that is out of scope here. The RSS
    /// `CancellationToken` remains the cooperative, per-task mechanism (it only
    /// preempts at await points); this ambient flag is the blunt host-level kill.
    #[inline]
    fn tick(&mut self) -> Result<(), EvalError> {
        self.steps += 1;
        if let Some(limit) = self.limits.step_budget
            && self.steps > limit
        {
            return Err(EvalError::Runtime(format!(
                "step budget exceeded ({limit} instructions)"
            )));
        }
        if self.steps.is_multiple_of(CANCEL_POLL_INTERVAL)
            && let Some(flag) = self.limits.cancel.as_ref()
            && flag.load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(EvalError::Runtime("evaluation cancelled".into()));
        }
        Ok(())
    }

    /// Account `bytes` of container growth against the memory ceiling. A no-op
    /// (no add, no check) when `limits.mem_budget` is `None`, so the off path is
    /// near-free. When a budget is set and the cumulative estimate exceeds it,
    /// returns the memory-limit error. Best-effort: see [`RegVm::live_bytes`].
    #[inline]
    fn account_bytes(&mut self, bytes: usize) -> Result<(), EvalError> {
        if let Some(limit) = self.limits.mem_budget {
            self.live_bytes = self.live_bytes.saturating_add(bytes);
            if self.live_bytes > limit {
                return Err(EvalError::Runtime(format!(
                    "memory limit exceeded ({limit} bytes)"
                )));
            }
        }
        Ok(())
    }

    /// Charge one stdlib/runtime intrinsic dispatch against `host_call_budget`.
    /// Always increments the counter (the single unconditional add is the whole
    /// cost when the budget is off), and — only when `limits.host_call_budget` is
    /// `Some` — trips once the count exceeds the limit. Called once at the entry of
    /// both intrinsic dispatch functions, so it caps the number of host-library
    /// calls (file/process/net/clock/log effects all enter here) independently of
    /// raw instruction count.
    #[inline]
    fn charge_host_call(&mut self) -> Result<(), EvalError> {
        self.host_calls += 1;
        if let Some(limit) = self.limits.host_call_budget
            && self.host_calls > limit
        {
            return Err(EvalError::Runtime(format!(
                "host call budget exceeded ({limit} stdlib calls)"
            )));
        }
        Ok(())
    }

    /// Append program output to the captured `stdout` buffer, and — when live
    /// streaming is enabled (`rss dev --run`) — flush newly completed lines to the
    /// real process stdout immediately. The captured buffer is appended to exactly
    /// the same way regardless, so callers that read `EvalOutput.stdout` see no
    /// difference.
    ///
    /// Enforces `stdout_budget`: when set, a write that would push cumulative
    /// output past the ceiling fails with a clean error *before* appending, so the
    /// captured buffer never exceeds the budget and a flood of output can't exhaust
    /// host memory.
    fn push_stdout(&mut self, text: &str) -> Result<(), EvalError> {
        if let Some(limit) = self.limits.stdout_budget
            && self.stdout.len().saturating_add(text.len()) > limit
        {
            return Err(EvalError::Runtime(format!(
                "stdout budget exceeded ({limit} bytes)"
            )));
        }
        self.stdout.push_str(text);
        if self.stream_stdout {
            self.flush_stdout_stream();
        }
        Ok(())
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

    /// Try to run `func` on the native (Cranelift) tier. Returns
    /// [`NativeAttempt::Completed`] if the compiled code ran to completion;
    /// [`NativeAttempt::Resumed`] if a native bail was reconstructed into the
    /// interpreter at a safepoint (J0.2, only under `precise_deopt`); or
    /// [`NativeAttempt::Fallback`] when the function isn't native-eligible, an
    /// argument isn't the inferred type, or the native code bailed and precise
    /// resume did not (or could not) apply — in all of which cases the caller
    /// re-runs the function from the top on the interpreter, which produces the
    /// exact value or error. Safe because native-eligible functions are leaf and
    /// side-effect-free, so re-running them is observationally identical.
    #[cfg(feature = "native-jit")]
    #[allow(clippy::wrong_self_convention)]
    fn try_native(&mut self, func: &RegFunction, base: usize) -> NativeAttempt {
        // Native limit parity (execution spec §6.2, Model A): Cranelift code polls
        // neither the step budget nor the cancel flag, so a hot, tiered-up function
        // containing an unbounded loop would run natively and bypass `step_budget`
        // / `cancel`. When either preemption limit is armed we refuse to dispatch
        // natively and fall back to the tier-0 executor (and interpreter), which
        // `tick()` on every instruction. `mem_budget` is *not* in this gate: a
        // native-eligible function is a side-effect-free pure-scalar / read-heap
        // leaf that allocates no VM-managed container, so it cannot grow the
        // accounted live-set in the first place.
        if self.limits.step_budget.is_some() || self.limits.cancel.is_some() {
            return NativeAttempt::Fallback;
        }
        // Cheap negative path: a function known not native-eligible never compiles,
        // so skip all per-call tiering/cache/name-hash work and fall straight back
        // to the interpreter (keeps `jit-native` from being slower than the VM on
        // code the native tier can't take).
        if func.native_status.get() == NATIVE_STATUS_NOT_ELIGIBLE {
            return NativeAttempt::Fallback;
        }
        // The unit is needed to resolve inlinable callees; clone the `Rc` so the
        // mutable `self.native` borrow below doesn't conflict.
        let unit = Rc::clone(&self.unit);
        let native_key = func as *const RegFunction as usize;
        // Phase 1: tiering + resolve (and lazily compile) the native function.
        // `None` in the cache means "known not native-eligible".
        let (id, ret_type, param_types) = {
            let Some(native) = self.native.as_mut() else {
                return NativeAttempt::Fallback;
            };
            if native.force_bail {
                // Deopt stress mode: pretend the native code bailed at its first
                // guard, so the interpreter handles the function.
                return NativeAttempt::Fallback;
            }
            // Tiering: stay on the interpreter until the function is hot.
            let count = native.counts.entry(native_key).or_insert(0);
            *count += 1;
            if *count <= native.tier_up_threshold {
                if native.collect_stats {
                    native.stats.tier_deferred += 1;
                }
                return NativeAttempt::Fallback;
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
                                    // Static profitability signal: a body with an
                                    // internal back-edge (a loop) does O(n) work per
                                    // dispatch and amortizes FFI cost; a loop-free body
                                    // does O(1) and, if dispatched per loop iteration,
                                    // never does. Drives `NATIVE_NOAMORTIZE_GIVEUP`.
                                    let has_backedge = jit_function_has_loop(&func.code);
                                    Some((id, ret, params, has_backedge))
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
                            // J2: if translation failed *only* because a structurally
                            // inlinable `CallClosure` site hasn't yet warmed to a
                            // monomorphic decision, the verdict is NOT invariant —
                            // re-attempt on a later (warmer) call. Don't cache and
                            // don't mark NOT_ELIGIBLE; just fall back this once.
                            if native_translation_pending_on_profile(&unit, func) {
                                return NativeAttempt::Fallback;
                            }
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
            let (id, ret, params, has_backedge) = match entry {
                Some(entry) => entry,
                None => return NativeAttempt::Fallback,
            };
            // No-amortization profitability gate. A loop-free body does O(1) work
            // per dispatch; dispatched once per interpreter loop iteration it pays
            // FFI + marshalling cost it can never amortize. After
            // `NATIVE_NOAMORTIZE_GIVEUP` such dispatches, demote it to NOT_ELIGIBLE
            // (reusing the `record_bail` demotion machinery: set the status bit, drop
            // the cache + the counter) so the remainder of the loop takes the cheap
            // interpreter fallback. Loop-bearing bodies (`has_backedge`) do O(n) work
            // per dispatch, amortize the cost, and are never counted here — they are
            // dispatched `calls=1` (the whole loop compiled into one native body) and
            // so could never reach `K` anyway. This is the same predict-and-skip
            // pattern as the bail give-up, not a parallel system.
            if !has_backedge {
                let count = native.noamortize_counts.entry(native_key).or_insert(0);
                *count += 1;
                if *count >= NATIVE_NOAMORTIZE_GIVEUP {
                    func.native_status.set(NATIVE_STATUS_NOT_ELIGIBLE);
                    native.cache.remove(&native_key);
                    native.noamortize_counts.remove(&native_key);
                    return NativeAttempt::Fallback;
                }
            }
            (id, ret, params)
        };
        // Phase 2: marshal each argument to 64 bits per its inferred parameter
        // type. Scalars unbox directly; a `Handle` (struct/list) is registered in
        // the per-call heap table and passed as its index, for the host helpers to
        // read. (`NativeModule::call` resets its own bail flag.) A drop guard clears
        // the (possibly large) heap table on every exit path so cloned args aren't
        // retained after the call.
        let _heap_guard = JitHeapArgsGuard;
        // Heap-result return ABI (S0): clear the output table on EVERY exit too, so a
        // bailed attempt (where the host never populates it) still leaves it empty for
        // the next call — no stale heap result can be double-materialized (§7.2).
        let _heap_result_guard = JitHeapResultsGuard;
        // `args[i]` and `lens[i]` are parallel per-param words (TV2 ABI). A scalar
        // unboxes into `args[i]` (with `lens[i] = 0`); a `Handle` is a heap-table
        // index; a `FlatInt`/`FlatFloat` puts the raw buffer pointer in `args[i]`
        // and the element count in `lens[i]`.
        let n = param_types.len();
        // Reuse the pooled scratch buffers instead of allocating three `Vec`s on
        // every native call: a tiny leaf/closure dispatched once per loop iteration
        // would otherwise pay that per-call allocation churn (the actual cause of
        // marginal closure/leaf kernels running slower than the interpreter). The
        // buffers are taken from `self.native` and returned to it on every exit
        // path (`restore_scratch`), so they stay warm across calls.
        let (mut args, mut lens, mut flat_owned) = match self.native.as_mut() {
            Some(native) => (
                std::mem::take(&mut native.scratch_args),
                std::mem::take(&mut native.scratch_lens),
                std::mem::take(&mut native.scratch_flat_owned),
            ),
            None => return NativeAttempt::Fallback,
        };
        args.clear();
        args.resize(n, 0i64);
        lens.clear();
        lens.resize(n, 0i64);
        // TV2 borrow protocol: owned `Rc`s of every flat list arg, kept alive for
        // the whole call; we then pin a shared `Ref` borrow of each so no
        // `borrow_mut`/realloc can occur while native code holds the raw pointer.
        flat_owned.clear();
        // Returns the (drained) scratch buffers to the pool so the next call reuses
        // their capacity. `flat_owned` is cleared of its `Rc`s first so no list arg
        // is retained past the call.
        let restore_scratch =
            |this: &mut Self, mut args: Vec<i64>, mut lens: Vec<i64>, mut flat: Vec<Rc<RefCell<TypedVec>>>| {
                if let Some(native) = this.native.as_mut() {
                    args.clear();
                    lens.clear();
                    flat.clear();
                    native.scratch_args = args;
                    native.scratch_lens = lens;
                    native.scratch_flat_owned = flat;
                }
            };
        let bail_marshal = |this: &mut Self| {
            if let Some(native) = this.native.as_mut() {
                if native.collect_stats {
                    native.stats.arg_mismatch += 1;
                }
                native.record_bail(native_key, func);
            }
        };
        for index in 0..n {
            let param_type = param_types[index];
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
                // TV2 flat marshalling. The compiled code expects a flat buffer of
                // the param's kind; if the *runtime* list is that kind, clone its
                // `Rc` (to keep it alive) for the pin pass below. Otherwise (a
                // `Boxed` list — TV1 is non-canonical — or a non-list) fall back to
                // the interpreter, which is always correct.
                NativeTy::FlatInt | NativeTy::FlatFloat => match value {
                    VmValue::List(list) => {
                        let want_int = param_type == NativeTy::FlatInt;
                        let ok = {
                            let borrowed = list.borrow();
                            if want_int {
                                borrowed.as_ints_slice().is_some()
                            } else {
                                borrowed.as_floats_slice().is_some()
                            }
                        };
                        if ok {
                            flat_owned.push(Rc::clone(list));
                            // Placeholder; ptr+len filled in the pin pass once all
                            // borrows are held simultaneously.
                            Some(0)
                        } else {
                            None
                        }
                    }
                    _ => None,
                },
            };
            match bits {
                Some(bits) => args[index] = bits,
                None => {
                    bail_marshal(self);
                    restore_scratch(self, args, lens, flat_owned);
                    return NativeAttempt::Fallback;
                }
            }
        }
        // SAFETY (TV2 borrow protocol — the unsafe core, audited here in one place):
        // We pin a shared `Ref` borrow of every flat list arg's `RefCell<TypedVec>`
        // for the entire `module.call(...)` below. `flat_guards` holds those `Ref`s;
        // it borrows `flat_owned` (declared above, never moved/dropped before the
        // call), and the `Rc`s in `flat_owned` keep the `RefCell`s alive. While these
        // shared borrows are held, no `borrow_mut` can succeed, so the backing `Vec`
        // cannot reallocate or mutate — hence the raw `as_ptr()` we hand to native
        // code stays valid and immovable for the call's duration. Native-eligible
        // functions are side-effect-free (§7.2), so they never even attempt a write;
        // the pinned borrow is the belt-and-suspenders that makes the raw read sound
        // regardless. Every index the native code computes is bounds-checked against
        // the matching `lens` entry (→ fallback on OOB), so it never reads past the
        // buffer. The pointers are not retained past the call (the generated code
        // never stores them), and `flat_guards`/`flat_owned` drop right after.
        let flat_guards: Vec<std::cell::Ref<'_, TypedVec>> =
            flat_owned.iter().map(|rc| rc.borrow()).collect();
        {
            let mut next = 0usize;
            for index in 0..n {
                match param_types[index] {
                    NativeTy::FlatInt => {
                        let (ptr, len) = flat_guards[next].as_ints_slice().expect("Ints pinned");
                        next += 1;
                        args[index] = ptr as i64;
                        lens[index] = len as i64;
                    }
                    NativeTy::FlatFloat => {
                        let (ptr, len) =
                            flat_guards[next].as_floats_slice().expect("Floats pinned");
                        next += 1;
                        args[index] = ptr as i64;
                        lens[index] = len as i64;
                    }
                    _ => {}
                }
            }
        }
        // Phase 3: call. `call` returns `NativeOutcome::Deopt` if the native code
        // bailed at a guard *or* a host helper flagged an unsatisfiable heap read;
        // either way the interpreter re-runs the function. A clean
        // `NativeOutcome::Completed` result is boxed per the function's return type
        // (a float register stored its `f64` bit pattern). The call is scoped so
        // `flat_guards` (the pinned shared borrows of the flat list args) drops
        // immediately after, before the scratch buffers are returned to the pool.
        let (result, elapsed) = {
            let Some(native_ref) = self.native.as_ref() else {
                return NativeAttempt::Fallback;
            };
            let collect_stats = native_ref.collect_stats;
            let started = collect_stats.then(std::time::Instant::now);
            let result = native_ref.module.call(id, &args, &lens);
            let elapsed = started.map(|started| started.elapsed().as_nanos());
            (result, elapsed)
        };
        drop(flat_guards);
        // Return the scratch buffers (incl. the now-unborrowed `flat_owned`) to the
        // pool before the result-handling re-borrows `self.native`.
        restore_scratch(self, args, lens, flat_owned);
        let Some(native) = self.native.as_mut() else {
            return NativeAttempt::Fallback;
        };
        if let Some(elapsed) = elapsed {
            native.stats.run_nanos += elapsed;
        }
        match result {
            vm_jit::NativeOutcome::Completed(bits) => {
                if native.collect_stats {
                    native.stats.native_calls += 1;
                }
                // Lever 2 (observational): record that this function actually ran
                // natively to completion, so the report's `native: ok` positive
                // reflects the real runtime outcome. Gated on `report`; no effect
                // on any decision.
                if native.report {
                    native.report_native_ok.insert(native_key);
                }
                // Consecutive-bail semantics: a clean completion clears the
                // give-up counter, so only *sustained* failure demotes a function.
                native.bail_counts.insert(native_key, 0);
                debug_assert_ne!(
                    ret_type,
                    NativeTy::Handle,
                    "a Handle-returning native function must report CompletedHandle, not Completed",
                );
                NativeAttempt::Completed(match ret_type {
                    NativeTy::Float => VmValue::Float(f64::from_bits(bits as u64)),
                    _ => VmValue::Int(bits),
                })
            }
            // Heap-result return ABI (heap-write S0): the native call completed
            // cleanly (the vm-jit `call` reports this variant ONLY when the bail flag
            // is clear) and its result is a heap value at output-table handle `bits`.
            // S0's only producer is a pass-through: `bits` is the index of a heap
            // PARAMETER in the input table (`JIT_HEAP_ARGS`), which native returned
            // unchanged. We copy that value into the VM-owned OUTPUT table
            // (`JIT_HEAP_RESULTS`) — exercising the output-table substrate end-to-end —
            // and materialize the `VmValue` from it. §7.2: this runs ONLY on clean
            // completion; on any bail vm-jit returns `Deopt` instead and the output
            // table is left empty (and cleared by its guard on exit), so a bailed
            // attempt produces no value here and the interpreter re-run is the sole
            // source of truth — no observable effect precedes a bail.
            vm_jit::NativeOutcome::CompletedHandle(bits) => {
                if native.collect_stats {
                    native.stats.native_calls += 1;
                }
                if native.report {
                    native.report_native_ok.insert(native_key);
                }
                native.bail_counts.insert(native_key, 0);
                // Resolve the input-table handle to its heap value, then publish it
                // into the output table at index 0. A malformed/out-of-range handle
                // cannot occur for the S0 pass-through (codegen returns a real param
                // index), but if it ever did we fall back rather than panic.
                let materialized = JIT_HEAP_ARGS.with(|args| {
                    usize::try_from(bits)
                        .ok()
                        .and_then(|i| args.borrow().get(i).cloned())
                });
                match materialized {
                    Some(value) => {
                        let result = JIT_HEAP_RESULTS.with(|out| {
                            let mut out = out.borrow_mut();
                            out.push(value);
                            // Materialize from the output table (the VM-owned home of
                            // the result), proving the round-trip through it.
                            out[0].clone()
                        });
                        NativeAttempt::Completed(result)
                    }
                    None => {
                        // Treat an unresolvable handle exactly like a bail: re-run on
                        // the interpreter (always correct, no effect leaked).
                        native.record_bail(native_key, func);
                        NativeAttempt::Fallback
                    }
                }
            }
            vm_jit::NativeOutcome::Deopt { safepoint_id, live } => {
                // Bail bookkeeping is identical on both paths (precise or not):
                // a bail is still a bail for the give-up/demotion heuristic.
                if native.collect_stats {
                    native.stats.native_bails += 1;
                }
                native.record_bail(native_key, func);
                let precise_deopt = native.precise_deopt;
                // J0.2 precise resume: take it ONLY when the flag is on AND this is
                // a real, mapped safepoint (id ≥ 1 with a recorded site). Anything
                // else (flag off, anonymous/early bail, or a missing site) falls
                // back to the safe re-run-from-top default.
                if precise_deopt && safepoint_id.0 >= 1 {
                    // Re-borrow `native` immutably to look up the site; clone the
                    // `resume_ip` out so the borrow ends before we touch `self`.
                    let resume_ip = self
                        .native
                        .as_ref()
                        .and_then(|n| n.module.deopt_map(id))
                        .and_then(|m| m.sites.get(safepoint_id.0 as usize - 1))
                        .map(|site| site.resume_ip);
                    if let Some(resume_ip) = resume_ip {
                        // Restore the live register window from the captured values,
                        // SKIPPING parameter registers: their window slots
                        // `base..base+n_params` are already valid and may hold heap
                        // `VmValue`s the scalar deopt payload cannot represent.
                        let n_params = func.params;
                        for vm_jit::DeoptReg { reg, value } in live {
                            if (reg as usize) < n_params {
                                continue;
                            }
                            let vm_value = match value {
                                vm_jit::DeoptValue::Int(i) => VmValue::Int(i),
                                vm_jit::DeoptValue::Float(f) => VmValue::Float(f),
                            };
                            self.set_reg(base + reg as usize, vm_value);
                        }
                        // Resume interpretation AT the bailing instruction.
                        self.frames.last_mut().expect("active frame").ip = resume_ip as usize;
                        return NativeAttempt::Resumed;
                    }
                }
                NativeAttempt::Fallback
            }
        }
    }

    /// J5.2 OSR (on-stack replacement). The interpreter has reached `header_ip` —
    /// the entry of a qualifying native-subset hot loop in `func` — with the active
    /// frame's window at `base`. Hand that window to an OSR-compiled native loop
    /// body (OSR-entry loads the live-in registers from the window and jumps to the
    /// header; the loop runs natively); when the loop exits, native deopts at the
    /// post-loop ip with the live-out window. Restore that window and set the
    /// frame's `ip` to the post-loop ip, so the interpreter resumes there (running
    /// the rest of the function — the I/O / setup the loop was tangled with).
    ///
    /// Returns `true` iff OSR ran and the frame was resumed at the post-loop ip
    /// (the caller must re-read `ip` and keep interpreting). `false` means OSR did
    /// not apply (not eligible, marshalling mismatch, or an unexpected bail): the
    /// frame is untouched and the interpreter just keeps running the loop normally —
    /// the safe, behavior-preserving default. **Soundness:** the OSR loop body is
    /// identity-indexed with `func.code`, the loop region is fully native-subset,
    /// and the only native exit is the OSR-exit, whose `resume_ip` is the
    /// interpreter's own post-loop instruction index — so resuming there with the
    /// restored window is byte-identical to having interpreted the loop.
    /// Resolve a function's OSR auto-trigger state ONCE, on first `drive` entry
    /// (Pending #2). Returns the candidate loop header (if any) so the caller can
    /// hoist it into a single per-frame local. Runs a cheap single-natural-loop
    /// detection on `func.code` (no compile, no region passes — that is deferred to
    /// the threshold `try_osr` call); a function with no analyzable loop becomes
    /// `NotCandidate` and thereafter pays only one `Cell` read per call with NO
    /// per-instruction cost. The eager `RSS_JIT_OSR` path resolves the same way but
    /// fires at threshold 0 (handled by the caller), so its very first header hit
    /// triggers — preserving the forced-OSR behavior and the differential backend.
    ///
    /// Determinism: this only decides *whether/when* to attempt OSR; `try_osr` is
    /// byte-identical to interpretation, so triggering never changes a value.
    #[cfg(feature = "native-jit")]
    fn resolve_osr_candidate(&self, func: &RegFunction) -> Option<usize> {
        match func.osr_state.get() {
            OsrTrigger::Unknown => {
                // Preemption parity with `try_osr`: native loops poll neither the
                // step budget nor the cancel flag, so a function can never OSR while
                // either is armed. Resolve to `NotCandidate` WITHOUT caching it
                // permanently — a later run without the budget must re-resolve, so
                // leave the state `Unknown` (return `None` for this frame only).
                if self.limits.step_budget.is_some() || self.limits.cancel.is_some() {
                    return None;
                }
                // Cheap candidacy pre-check: does the ORIGINAL code have a single
                // analyzable natural loop? (No inline/region passes, no compile.)
                // The real native-subset/dissolvable verdict is only known after
                // `try_osr` at threshold; a detected-but-uncompilable loop simply
                // counts to threshold once, fails `try_osr`, and goes `GaveUp` —
                // bounded cost, only for functions that have a natural loop at all.
                let state = match detect_single_natural_loop(&func.code) {
                    Some(lp) => OsrTrigger::Counting {
                        header_ip: lp.header,
                        count: 0,
                        probe_cc: 0,
                    },
                    None => OsrTrigger::NotCandidate,
                };
                func.osr_state.set(state);
                match state {
                    OsrTrigger::Counting { header_ip, .. } => Some(header_ip),
                    _ => None,
                }
            }
            OsrTrigger::Counting { header_ip, .. } => Some(header_ip),
            OsrTrigger::NotCandidate | OsrTrigger::GaveUp => None,
        }
    }

    #[cfg(feature = "native-jit")]
    fn try_osr(&mut self, func: &RegFunction, base: usize, header_ip: usize) -> bool {
        // Preemption parity (as in `try_native`): native loops poll neither the
        // step budget nor the cancel flag, so refuse OSR while either is armed.
        if self.limits.step_budget.is_some() || self.limits.cancel.is_some() {
            return false;
        }
        let native_key = func as *const RegFunction as usize;

        // Phase 1: resolve (and lazily compile) the OSR loop body for this function,
        // then gate on being at the loop header. This runs at every instruction when
        // OSR is armed, so it must be cheap on the common (not-at-header) path: the
        // cache lookup + header compare returns without cloning anything.
        // `param_types` (the param-prefix of `reg_types`) is no longer consulted here:
        // OSR marshalling now classifies every live-in by the full per-register
        // `reg_types` (so a non-param flat list is marshalled correctly). It stays in
        // the cache entry for the non-OSR `try_native` path.
        let (id, trans_exit, orig_exit, n_jit_regs, _param_types, reg_types) = {
            // Fast path: cached and NOT at the header ⇒ nothing to do (no clone).
            if let Some(native) = self.native.as_ref() {
                if let Some(entry) = native.osr_cache.get(&native_key) {
                    match entry {
                        Some(e) if e.orig_header == header_ip => {}
                        _ => return false,
                    }
                }
            }
            // Clone the unit handle before borrowing `self.native` mutably: the OSR
            // pre-pass inlines leaf `CallKnown`s, which needs the callee bodies.
            let unit = Rc::clone(&self.unit);
            let Some(native) = self.native.as_mut() else {
                return false;
            };
            // Detect + compile the function's single OSR loop ONCE, keyed by the
            // function (independent of the current ip). The header gate decides when
            // to actually fire.
            //
            // OSR × J3: detect the loop on the ORIGINAL code (detection understands
            // `MatchOption` as a two-way branch, so an Option-bearing body is shaped
            // correctly), then scalar-replace any non-escaping scalar `Option` that
            // lives ENTIRELY inside that loop region — turning the alloc-bound body
            // into native-subset code. Re-detect on the TRANSFORMED stream (where the
            // Option ops are gone) and compile. The transformed→original ip-map
            // translates the OSR boundary back to `func.code` (where the interpreter
            // resumes). When the body has no replaceable Option the region pass
            // returns the code unchanged with an identity ip-map, so plain
            // native-subset OSR is byte-for-byte the old path.
            if !native.osr_cache.contains_key(&native_key) {
                // OSR × inline-leaf-calls (Pending #1): FIRST inline straight-line
                // leaf `CallKnown`/closure calls into the function body, so a value
                // that is built in one helper and matched in another — both called
                // from the loop (e.g. `variant_match_loop`'s `make_shape`/`area`) —
                // becomes loop-LOCAL and the Option/variant/struct region passes
                // below can dissolve it. `native_inline_leaf_calls` returns a
                // transformed→original ip-map (`ip_map0`); a body with no inlinable
                // call yields the code unchanged with an identity map, so the
                // non-inline OSR path is byte-for-byte the old behavior. Detect the
                // loop on the INLINED stream, then run the three region passes on it,
                // composing ALL FOUR ip-maps:
                // `ip_map[t] = ip_map0[ip_map1[ip_map2[ip_map3[t]]]]`.
                // Pre-detect the loop on the ORIGINAL code to bound the inline pass
                // to the hot region: only calls INSIDE `[header, exit)` must be
                // inlinable (they have to dissolve to reach the native subset). A
                // pre-/post-loop helper call (e.g. `bench_size`, which is NOT
                // native-inlinable) lies outside the region, runs on the interpreter,
                // and is copied through — it must not veto OSR for the hot loop. When
                // the original code has no analyzable loop there is nothing to OSR, so
                // bail before inlining.
                // OSR × J3 combinator expansion (deopt-before-heap, Slice 2): BEFORE
                // inlining, lower each Option/Result combinator intrinsic
                // (`Option.map`/`and_then`/`unwrap_or`, `Result.map`/`and_then`/
                // `unwrap_or`) in the loop region into primitive match/construct form
                // with the mapper call left as an in-region `CallClosure` to the
                // (loop-local) mapper closure. The inline pass below then SINKS each
                // mapper `MakeClosure` (inlining its body) and the Option/Result SR
                // passes dissolve the per-iteration Option/Result values, so the
                // combinator chain becomes pure scalar code and the loop OSRs.
                //
                // When the body has no combinator the pass returns the code unchanged
                // with an identity `expand_map`, and we keep using the REAL `func`
                // (byte-for-byte the old path). When it DOES fire, the rest of the
                // chain runs on a synthetic `func_e` carrying the expanded code (with
                // NO profile — combinator mappers are sunk statically, not via the
                // profile, so disabling profile-guided mono/poly inlining for an
                // expanded body is a conservative restriction, never unsound). The
                // final OSR boundary is composed back through `expand_map` to land in
                // the REAL `func.code` (where the interpreter resumes).
                let expanded = detect_single_natural_loop(&func.code).and_then(|lp_pre| {
                    native_expand_option_result_combinators_in_region(
                        &unit, func, &func.code, func.regs, lp_pre.header, lp_pre.exit,
                    )
                });
                let (eff_owned, expand_map): (Option<RegFunction>, Vec<usize>) = match expanded {
                    // The identity fast-path returns the code unchanged with
                    // `eregs == func.regs` and `ecode.len() == func.code.len()`; a real
                    // expansion always adds temp regs AND grows the stream. Detect "did
                    // it fire" by either growing.
                    Some((ecode, eregs, emap))
                        if eregs != func.regs || ecode.len() != func.code.len() =>
                    {
                        let f_e = RegFunction {
                            name: func.name.clone(),
                            params: func.params,
                            captures: func.captures,
                            regs: eregs,
                            local_regs: HashMap::new(),
                            code: ecode,
                            jit_analysis: std::cell::Cell::new(None),
                            native_status: std::cell::Cell::new(0),
                            call_count: std::cell::Cell::new(0),
                            profile: RefCell::new(None),
                            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
                        };
                        (Some(f_e), emap)
                    }
                    _ => (None, (0..func.code.len()).collect()),
                };
                let eff_func: &RegFunction = eff_owned.as_ref().unwrap_or(func);
                // `expand_map[eff_idx] = real func.code idx`. A combinator at a real
                // index maps MANY expanded indices back to itself; the OSR boundary
                // (loop header/exit) is copy-through control flow, so it maps 1:1 to a
                // non-combinator real index. Guard anyway: a boundary landing on a real
                // combinator `CallIntrinsic` (impossible for copy-through, but defended)
                // bails OSR rather than misresume mid-fragment.
                let real_code = &func.code;
                let entry = detect_single_natural_loop(&eff_func.code).and_then(|lp_orig| {
                native_inline_leaf_calls(&unit, eff_func, true, Some((lp_orig.header, lp_orig.exit))).and_then(
                    |(inlined_code, n_regs0, ip_map0)| {
                    // OSR × J3 for STRING LENGTH-LAW FOLDING: BEFORE the Result/Option/
                    // variant/struct passes, dissolve any non-escaping string built ONLY
                    // to be measured (`String.len` of `concat`/`slice`/`from_int`/literal/
                    // `Move`). Each `String.len` folds to arithmetic on operand byte
                    // lengths (verified laws — byte len, additive concat, ASCII slice
                    // clamp, `from_int` sign/zero/`i64::MIN` digit count) and the now-dead
                    // string allocations are DELETED — read-only (no heap write; Exec Spec
                    // §7.2 holds), turning a length-only string loop into pure-scalar Int
                    // code the native subset accepts. An escaping string, an unprovable
                    // length law (non-ASCII slice), or a `String.len` not traceable to a
                    // foldable producer bails the whole pass; a body with no foldable
                    // `String.len` returns the code unchanged with an identity ip-map, so
                    // a non-string (or plain) body is byte-for-byte the old path. This runs
                    // FIRST because it must see the RAW string ops (`StringConcat`/
                    // `StringFromInt`/`StringSlice`/`StringLen`) before any later pass; the
                    // transformed stream carries only Int arithmetic + branches in place of
                    // the string ops, which the Result-SR pass copies through verbatim.
                    detect_single_natural_loop(&inlined_code).and_then(|lp_sl| {
                    native_string_length_fold_in_region(
                        &inlined_code, n_regs0, lp_sl.header, lp_sl.exit,
                    )
                    .and_then(|(inlined_code, n_regs0, ip_map_sl)| {
                    // OSR × J3 for BYTES LENGTH-LAW FOLDING (read-only sibling of the
                    // string fold above): dissolve any non-escaping Bytes value built
                    // ONLY to be measured (`Bytes.len` of `Bytes.slice`/
                    // `Bytes.from_string`/`Move`/a constant-length source) into byte-
                    // length arithmetic, DELETING the dead Bytes allocation. Bytes carry
                    // no char boundary, so the slice law is the exact `bytes_slice` clamp
                    // with NO ASCII gate. Runs right after the string fold (it also needs
                    // the RAW `BytesSlice`/`BytesLen` ops before the Result/Option/variant/
                    // struct passes copy them through). A body with no foldable
                    // `Bytes.len` returns the code unchanged with an identity ip-map, so a
                    // non-Bytes (or plain) body is byte-for-byte the prior path.
                    detect_single_natural_loop(&inlined_code).and_then(|lp_by| {
                    native_bytes_length_fold_in_region(
                        &inlined_code, n_regs0, lp_by.header, lp_by.exit,
                    )
                    .and_then(|(inlined_code, n_regs0, ip_map_by)| {
                    // OSR × J3 for RESULTS (deopt-before-heap, Slice 1): scalar-replace
                    // any non-escaping, statically-always-`Ok` `Result<Scalar,_>` living
                    // entirely inside the region. An inlined leaf whose `Err` arm built a
                    // heap value (or a combinator's expanded `Err` arm) left a native
                    // `Bail` in its place, so the only Result constructor is
                    // `MakeVariant{Ok,[scalar]}` and the Result dissolves to a scalar
                    // payload (`MatchResult` → `Jump ok`). RESULT-SR runs BEFORE Option-SR
                    // because it tolerates in-region Option ops (it copies `MatchOption`/
                    // `MakeSome`/`UnwrapSome`/`LoadNone` through verbatim), whereas
                    // Option-SR requires every in-region instruction to be native-subset
                    // or an Option op — so a MIXED Option+Result body (the combinator
                    // chain) must dissolve its Results first. A live heap `Err` (or any
                    // non-dissolvable shape) returns the code unchanged with an identity
                    // ip-map (or bails), so a pure-Option/plain body is byte-for-byte the
                    // old path.
                    detect_single_natural_loop(&inlined_code).and_then(|lp_r| {
                    native_scalar_replace_results_in_region(
                        &inlined_code, n_regs0, lp_r.header, lp_r.exit,
                    )
                    .and_then(|(code_r, n_regs_r, ip_map_r)| {
                        // OSR × J3 for OPTIONS: dissolve any non-escaping scalar Option
                        // living entirely inside the region. After Result-SR the region
                        // carries only Option ops + native subset, so the strict
                        // subset-or-option gate is satisfied. Identity (no Option) ⇒
                        // unchanged.
                        let lp1 = detect_single_natural_loop(&code_r)?;
                        let (code1, n_regs1, ip_map1) = native_scalar_replace_options_in_region(
                            &code_r, n_regs_r, lp1.header, lp1.exit,
                        )?;
                        // OSR × J3 for VARIANTS: after dissolving Options/Results, re-detect
                        // the loop on the transformed stream and scalar-replace any
                        // non-escaping user variant whose arms carry only scalar fields
                        // (N>=0 fields per arm) living entirely inside that region
                        // (`MakeVariant`/`MatchVariant`/`UnwrapVariantValue`/`GetField`
                        // → LoadInt-tag + per-(arm,slot) Move). When there
                        // is no replaceable variant the pass returns the code unchanged
                        // with an identity ip-map, so an Option-only (or plain) body is
                        // byte-for-byte the old path. Compose the transformed→
                        // original ip-maps.
                        let lp_v = detect_single_natural_loop(&code1)?;
                        let (code2, n_regs2, ip_map2) = native_scalar_replace_variants_in_region(
                            &code1, n_regs1, lp_v.header, lp_v.exit,
                        )?;
                        // OSR × J3 for STRUCTS: after dissolving Options and variants,
                        // re-detect the loop on the transformed stream and scalar-replace
                        // any non-escaping flat user struct living entirely inside that
                        // region (`MakeStruct`/`GetFieldSlot` → per-slot `Move`). When
                        // there is no replaceable struct the pass returns the code
                        // unchanged with an identity ip-map, so an Option/variant-only (or
                        // plain) body is byte-for-byte the old path. Compose all three
                        // transformed→original ip-maps:
                        // `ip_map[t] = ip_map1[ip_map2[ip_map3[t]]]`.
                        let lp_s = detect_single_natural_loop(&code2)?;
                        let (code_s, n_regs_s, ip_map3) = native_scalar_replace_structs_in_region(
                            &code2, n_regs2, lp_s.header, lp_s.exit,
                        )?;
                        // OSR × J3 for LOOP-CARRIED STRUCTS: after the loop-LOCAL
                        // struct pass, dissolve a struct created in the pre-header,
                        // mutated in place across iterations (`SetFieldSlot`), and dead
                        // after the loop into loop-carried scalar leaf registers (the
                        // in-place heap writes become register writes). When there is no
                        // in-region `SetFieldSlot` the pass returns the code unchanged
                        // with an identity ip-map, so an earlier-dissolved (or plain)
                        // body is byte-for-byte the prior path. Compose its map too.
                        let lp_lc = detect_single_natural_loop(&code_s)?;
                        let (code, n_regs, ip_map3b) = native_loop_carried_struct_in_region(
                            &code_s, n_regs_s, lp_lc.header, lp_lc.exit,
                        )?;
                        // Compose all FIVE maps to land in the (effective) inlined
                        // `func.code` index space. The transform order is now
                        // result → option → variant → struct, so:
                        // `ip_map3` (struct) → `ip_map2` (variant) → `ip_map1` (option) →
                        // `ip_map_r` (result) index successive transformed streams; the
                        // final hop through `ip_map0` carries an inlined-stream ip back to
                        // the (effective) function's ip.
                        // The loop-carried struct pass (`ip_map3b`) runs LAST, so its
                        // index is the outermost hop. The string length-fold pass
                        // (`ip_map_sl`) runs FIRST (right after inlining), so its hop sits
                        // just inside `ip_map0`; the Bytes length-fold (`ip_map_by`) runs
                        // immediately after the string fold, so its hop sits just inside
                        // `ip_map_sl`:
                        // `ip_map[t] =
                        //   ip_map0[ip_map_sl[ip_map_by[ip_map_r[ip_map1[ip_map2[ip_map3[ip_map3b[t]]]]]]]]`.
                        let ip_map: Vec<usize> = ip_map3b
                            .iter()
                            .map(|&tb| {
                                ip_map0[ip_map_sl[ip_map_by[ip_map_r[ip_map1[ip_map2[ip_map3[tb]]]]]]]
                            })
                            .collect();
                        // Re-detect on the fully-transformed stream; its single loop is
                        // the same loop with both Option and variant ops dissolved (the
                        // body is now native-subset). Indices shift, so use `lp`.
                        detect_single_natural_loop(&code).and_then(|lp| {
                            // Map the OSR boundary back to the ORIGINAL code. The loop
                            // header and exit branches live in the OUTER function (not
                            // inside an inlined callee), so they are copy-through
                            // branches (`JumpIfIntCompare`/`JumpIfBool`/`Jump`) — never
                            // Option ops, never spliced callee body — and map one-to-one
                            // to their original index, making the boundary mapping
                            // unambiguous. If either ip cannot map back soundly, bail
                            // OSR (never misresume).
                            //
                            // Soundness (OSR × inline): the OSR boundary MUST be a
                            // copy-through instruction. An instruction spliced in from
                            // an inlined callee has its `ip_map0` entry pointing at the
                            // `CallKnown`/`CallClosure` site it was inlined from, so the
                            // boundary maps into an inlined region exactly when the
                            // original instruction at the mapped ip is a call. If the
                            // header or exit maps into an inlined region, bail OSR (the
                            // dissolved/inlined values must be strictly loop-internal,
                            // dead at both boundaries; the inlined-callee temp registers
                            // are fresh windows above `func.regs`, used only in the loop
                            // body). The struct/variant/Option region gates already
                            // enforce dead-at-boundary for the scalar-replaced regs.
                            // The inline-region check is against the EXPANDED stream
                            // (`eff_func.code`), since `ip_map0`/`ip_map` map back into
                            // it. A boundary that maps into an inlined call site bails.
                            let maps_into_inline = |eff_idx: usize| {
                                eff_func.code.get(eff_idx).is_some_and(|instr| {
                                    matches!(
                                        instr,
                                        RegInstr::CallKnown { .. } | RegInstr::CallClosure { .. }
                                    )
                                })
                            };
                            // Compose the final hop through `expand_map` to land in the
                            // REAL `func.code` (interpreter resume index). A boundary
                            // landing on a real combinator `CallIntrinsic` bails (cannot
                            // resume mid-expanded-fragment).
                            let to_real = |eff_idx: usize| -> Option<usize> {
                                if eff_idx == eff_func.code.len() {
                                    return Some(real_code.len());
                                }
                                let real = *expand_map.get(eff_idx)?;
                                let is_combinator = real_code.get(real).is_some_and(|instr| matches!(
                                    instr,
                                    RegInstr::CallIntrinsic { intrinsic, .. }
                                        | RegInstr::CallTypedIntrinsic { intrinsic, .. }
                                            if combinator_intrinsic_kind(*intrinsic).is_some()
                                ));
                                if is_combinator { None } else { Some(real) }
                            };
                            // For `lp.exit`, the loop exits to one-past the post-loop
                            // body; when that lands exactly at the end of the
                            // transformed stream it maps to the end of the original
                            // stream.
                            let eff_header = *ip_map.get(lp.header)?;
                            if maps_into_inline(eff_header) {
                                return None;
                            }
                            let orig_header = to_real(eff_header)?;
                            let orig_exit = if lp.exit < ip_map.len() {
                                let oe = ip_map[lp.exit];
                                if maps_into_inline(oe) {
                                    return None;
                                }
                                to_real(oe)?
                            } else if lp.exit == code.len() {
                                real_code.len()
                            } else {
                                return None;
                            };
                            translate_osr_loop(&code, n_regs, eff_func.params, eff_func.captures, lp)
                                .and_then(|(jit_fn, params, reg_types)| {
                                    let n_jit_regs = jit_fn.n_regs as usize;
                                    match native.module.compile_osr(&jit_fn, lp.header as u32) {
                                        Ok(id) => Some(OsrEntry {
                                            id,
                                            orig_header,
                                            trans_exit: lp.exit,
                                            orig_exit,
                                            n_jit_regs,
                                            param_types: params,
                                            reg_types,
                                        }),
                                        Err(_) => None,
                                    }
                                })
                        })
                    })
                    })
                    })
                    })
                    })
                    })
                })
                });
                // OSR × J2: a capturing/monomorphic closure inline is profile-
                // guided, so on the first header hit (cold profile) the inline gate
                // declines and `entry` is `None`. Caching that permanently would
                // disable OSR forever — exactly the `try_native` warmup hazard. If a
                // closure-inline site is still PENDING on its profile, leave the
                // cache unpopulated so a later (warmer) header hit retries; once the
                // profile settles (or there is no pending site) the `None`/`Some`
                // verdict is stable and we cache it.
                if entry.is_some() || !native_translation_pending_on_profile(&unit, func) {
                    native.osr_cache.insert(native_key, entry);
                }
            }
            match native.osr_cache.get(&native_key) {
                // Only OSR when the interpreter is *at* the cached loop's (original)
                // header ip.
                Some(Some(e)) if e.orig_header == header_ip => {
                    (
                        e.id,
                        e.trans_exit,
                        e.orig_exit,
                        e.n_jit_regs,
                        e.param_types.clone(),
                        e.reg_types.clone(),
                    )
                }
                _ => return false,
            }
        };

        // Phase 2: marshal the current register window into the OSR call. The OSR
        // ABI's `args_ptr` is the full (TRANSFORMED) `n_jit_regs`-wide window indexed
        // by register: a scalar register contributes its raw bits, a handle
        // (List/struct) param its heap-table index (the host helpers read through
        // it), and an unwritten / non-scalar slot contributes 0 (native only ever
        // loads the live-in subset, all of which are written by definite-assignment
        // at the header). Under OSR × J3 the window is wider than `func.regs` by the
        // fresh tag/payload registers; only original registers `0..func.regs` carry
        // an interpreter value, and the J3-added slots stay 0 — the loop body assigns
        // them (LoadBool tag, Move payload) before any read, so their live-in value
        // is irrelevant. A drop guard clears the heap table on every exit path.
        let _heap_guard = JitHeapArgsGuard;
        let n_regs = func.regs;
        let mut window = vec![0i64; n_jit_regs];
        let mut lens = vec![0i64; n_jit_regs];
        // TV2 flat live-in lists: each loop-invariant typed list classified
        // `FlatInt`/`FlatFloat` is marshalled as a raw (pointer, length) pair, with a
        // shared `Ref` borrow pinned for the whole `module.call` so the backing buffer
        // cannot reallocate/mutate under native code (see the borrow protocol in
        // `try_native`). `flat_owned` keeps the `Rc`s alive; `flat_slots` records each
        // pinned list's window slot and flat kind so the pin pass can fill ptr+len once
        // all borrows are held. If the runtime value is not the expected flat kind
        // (e.g. a `Boxed`/heap-element list, or not a list at all), we BAIL the whole
        // OSR attempt — the interpreter runs the loop, always correct.
        let mut flat_owned: Vec<Rc<RefCell<TypedVec>>> = Vec::new();
        let mut flat_slots: Vec<(usize, NativeTy)> = Vec::new();
        for reg in 0..n_regs {
            if !self.written.get(base + reg).copied().unwrap_or(false) {
                continue; // not live here; native won't read it
            }
            let value = self.reg(base + reg);
            // Classify by the full per-register native type (not only params): a flat
            // live-in list may be a non-param register on the OSR path.
            let ty = reg_types.get(reg).copied().unwrap_or(NativeTy::Int);
            if matches!(ty, NativeTy::FlatInt | NativeTy::FlatFloat) {
                let want_int = ty == NativeTy::FlatInt;
                match value {
                    VmValue::List(list) => {
                        let ok = {
                            let borrowed = list.borrow();
                            if want_int {
                                borrowed.as_ints_slice().is_some()
                            } else {
                                borrowed.as_floats_slice().is_some()
                            }
                        };
                        if !ok {
                            // Not the canonical flat kind ⇒ bail this OSR attempt.
                            return false;
                        }
                        flat_owned.push(Rc::clone(list));
                        flat_slots.push((reg, ty));
                        // window[reg]/lens[reg] filled in the pin pass below.
                        continue;
                    }
                    // A flat-classified register that isn't a List at runtime ⇒ bail.
                    _ => return false,
                }
            }
            let bits = match (ty, value) {
                (NativeTy::Float, VmValue::Float(f)) => f.to_bits() as i64,
                (NativeTy::Float, VmValue::Int(i)) => (*i as f64).to_bits() as i64,
                (_, VmValue::Int(i)) => *i,
                (_, VmValue::Bool(b)) => i64::from(*b),
                (_, VmValue::Float(f)) => f.to_bits() as i64,
                // A handle (List/struct/etc.): pass its heap-table index.
                (_, other) => JIT_HEAP_ARGS.with(|table| {
                    let mut table = table.borrow_mut();
                    table.push(other.clone());
                    (table.len() - 1) as i64
                }),
            };
            window[reg] = bits;
        }

        // SAFETY (TV2 borrow protocol — the same audited core as `try_native`): pin a
        // shared `Ref` borrow of every flat list's `RefCell<TypedVec>` for the entire
        // `module.call` below. While these shared borrows are held no `borrow_mut` can
        // succeed, so the backing `Vec` cannot reallocate/mutate — the raw `as_ptr()`
        // we hand to native stays valid and immovable for the call. The list register
        // is loop-invariant (never rewritten in-loop) and the native subset has no
        // list-mutating instruction, so the loop never even attempts a write; the
        // pinned borrow is belt-and-suspenders. Every index is bounds-checked against
        // the matching `lens` slot (→ deopt on OOB). The pointer is not retained past
        // the call; `flat_guards`/`flat_owned` drop right after.
        let flat_guards: Vec<std::cell::Ref<'_, TypedVec>> =
            flat_owned.iter().map(|rc| rc.borrow()).collect();
        for (i, &(reg, kind)) in flat_slots.iter().enumerate() {
            let (ptr, len) = match kind {
                NativeTy::FlatInt => {
                    let (p, l) = flat_guards[i].as_ints_slice().expect("Ints pinned");
                    (p as i64, l)
                }
                _ => {
                    let (p, l) = flat_guards[i].as_floats_slice().expect("Floats pinned");
                    (p as i64, l)
                }
            };
            window[reg] = ptr;
            lens[reg] = len as i64;
        }

        // Phase 3: run the OSR loop body natively.
        let Some(native_ref) = self.native.as_ref() else {
            return false;
        };
        let collect_stats = native_ref.collect_stats;
        let started = collect_stats.then(std::time::Instant::now);
        let result = native_ref.module.call(id, &window, &lens);
        let elapsed = started.map(|started| started.elapsed().as_nanos());
        // The pinned borrows are no longer needed once the native call returns.
        drop(flat_guards);
        if let Some(native) = self.native.as_mut() {
            if let Some(elapsed) = elapsed {
                native.stats.run_nanos += elapsed;
            }
        }

        // Phase 4: OSR-exit. The loop always exits via the `OsrExit` safepoint (a
        // deopt). Resume the interpreter at the post-loop ip with the restored
        // live-out window — the precise-deopt resume, reused verbatim.
        match result {
            vm_jit::NativeOutcome::Deopt { safepoint_id, live } if safepoint_id.0 >= 1 => {
                let resume_ip = self
                    .native
                    .as_ref()
                    .and_then(|n| n.module.deopt_map(id))
                    .and_then(|m| m.sites.get(safepoint_id.0 as usize - 1))
                    .map(|site| site.resume_ip);
                let Some(resume_ip) = resume_ip else {
                    return false;
                };
                // The OSR-exit's resume_ip (a TRANSFORMED-code ip) MUST be the loop's
                // post-loop exit ip; anything else is an OSR construction bug. Fall
                // back rather than misresume.
                if resume_ip as usize != trans_exit {
                    return false;
                }
                // Restore the live-out window, SKIPPING parameter registers (their
                // slots already hold valid — possibly heap — values the scalar deopt
                // payload cannot represent; non-param live-out regs are all scalar by
                // construction of `translate_osr_loop`) AND any J3-added tag/payload
                // register (index >= func.regs): those are strictly loop-internal,
                // dead at the exit, and have no slot in the interpreter's original
                // `func.regs`-wide window. ALSO skip any **Handle**-class register
                // (Pending #1 stored-closure broadening): a loop-internal handle (a
                // stored struct/closure fetched via `FieldHandle`/`ListGetHandle`) is
                // dead at the exit and its captured "value" is only a heap-table
                // index — writing it back as an `Int` would corrupt the interpreter
                // slot. The interpreter re-derives any value it still needs.
                let n_params = func.params;
                let n_orig_regs = func.regs;
                for vm_jit::DeoptReg { reg, value } in live {
                    if (reg as usize) < n_params || (reg as usize) >= n_orig_regs {
                        continue;
                    }
                    // Skip Handle (heap-table index) AND flat-array (raw buffer
                    // pointer bits) registers: neither's deopt payload word is a VM
                    // value; writing it back would corrupt the interpreter slot. A
                    // flat list is loop-invariant, so the original List already sits in
                    // its slot unchanged.
                    if matches!(
                        reg_types.get(reg as usize),
                        Some(&NativeTy::Handle | &NativeTy::FlatInt | &NativeTy::FlatFloat)
                    ) {
                        continue;
                    }
                    let vm_value = match value {
                        vm_jit::DeoptValue::Int(i) => VmValue::Int(i),
                        vm_jit::DeoptValue::Float(f) => VmValue::Float(f),
                    };
                    self.set_reg(base + reg as usize, vm_value);
                }
                // Resume in the ORIGINAL `func.code`, at the ip-mapped post-loop ip.
                self.frames.last_mut().expect("active frame").ip = orig_exit;
                if let Some(native) = self.native.as_mut() {
                    if native.collect_stats {
                        native.stats.osr_entries += 1;
                    }
                    // Lever 2 (observational): record this function actually OSR-
                    // entered, so the report's `osr: entered` positive matches the
                    // real outcome. Gated on `report`; no effect on any decision.
                    if native.report {
                        native.report_osr_ok.insert(func as *const RegFunction as usize);
                    }
                }
                true
            }
            // A completion (the OSR body has no `Return`) or an anonymous/early bail
            // is not a normal OSR-exit: leave the frame untouched and let the
            // interpreter run the loop. Safe and behavior-preserving.
            _ => false,
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
            self.tick()?;
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
                self.prepare_frame(next_base, callee.regs)?;
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
    // perf-plan §1.1 (interpreter dispatch — inline the match, minimal form):
    // force this single-instruction executor to expand into BOTH callers
    // (`drive`'s hot loop and `run_jit`), so VM state (ip/base/register ptr)
    // stays in registers across the hot arms without a manual match rewrite.
    // This is an empirical probe; §1.1 may not pay off until the §1.3 hot/cold
    // split, so the win (if any) is judged against run-to-run spread per §0.4.
    #[inline(always)]
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
            RegInstr::MakeStruct { dst, layout, fields } => {
                // Hot path: the layout is interned once at lowering time, so this is
                // a refcount bump + a field-value gather — no per-construction
                // `(name, field_names)` hashing. Field values are in canonical slot
                // order (the lowerer canonicalized them to match the layout).
                let mut values: Vec<VmValue> = Vec::with_capacity(fields.len());
                for (_field, reg) in fields {
                    values.push(self.reg(base + *reg).clone());
                }
                self.set_reg(
                    base + *dst,
                    VmValue::Struct(Rc::new(VmStruct::with_layout(Rc::clone(layout), values))),
                );
            }
            RegInstr::MakeVariant { dst, layout, fields } => {
                let mut values: Vec<VmValue> = Vec::with_capacity(fields.len());
                for (_field, reg) in fields {
                    values.push(self.reg(base + *reg).clone());
                }
                self.set_reg(
                    base + *dst,
                    VmValue::Variant(Rc::new(VmStruct::with_layout(Rc::clone(layout), values))),
                );
            }
            RegInstr::MakeList { dst, items } => {
                let mut list = Vec::with_capacity(items.len());
                for reg in items {
                    list.push(self.reg(base + *reg).clone());
                }
                let typed = TypedVec::from_values(list);
                // Per-kind accounting (TV1): charge the flat buffer cost of the kind
                // the literal specialized to (8 B/elem for a scalar list).
                self.account_bytes(typed.len() * typed.elem_bytes())?;
                self.set_reg(base + *dst, VmValue::List(Rc::new(RefCell::new(typed))));
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
                self.account_bytes(entries.len() * MAP_ENTRY_BYTES)?;
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
                self.set_reg(base + *dst, VmValue::some(value));
            }
            RegInstr::LoadNone { dst } => {
                self.set_reg(base + *dst, VmValue::OptionNone);
            }
            RegInstr::MakeClosure {
                dst,
                function: callee,
                captures,
            } => {
                // Non-capturing closures of the same function are value-identical
                // every time, so when closure identity is provably unobservable
                // (see `noncapturing_closure_cache`) we reuse one cached `Rc`
                // instead of heap-allocating a fresh one each iteration.
                let closure = if captures.is_empty() && !self.unit.closure_identity_observable {
                    self.cached_noncapturing_closure(*callee)
                } else {
                    let mut captured = Vec::with_capacity(captures.len());
                    for reg in captures {
                        captured.push(self.reg(base + *reg).clone());
                    }
                    Rc::new(VmClosure {
                        function: *callee,
                        captures: captured,
                    })
                };
                self.set_reg(base + *dst, VmValue::Closure(closure));
            }
            RegInstr::MatchOption {
                src,
                some_ip,
                none_ip,
            } => match self.reg(base + *src) {
                VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => *ip = *some_ip,
                VmValue::OptionNone => *ip = *none_ip,
                other => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM Option match expected Option, got `{}`.",
                        other.display()
                    )));
                }
            },
            RegInstr::MatchResult { src, ok_ip, err_ip } => match self.reg(base + *src) {
                VmValue::Variant(data) if data.name().as_ref() == "Ok" => *ip = *ok_ip,
                VmValue::Variant(data) if data.name().as_ref() == "Err" => *ip = *err_ip,
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
                VmValue::Variant(data) if data.name().as_ref() == expected.as_str() => {
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
                    some @ (VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_)) => {
                        some.unwrap_some().expect("Some arm yields a payload")
                    }
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
                    VmValue::Variant(data) if data.name().as_ref() == expected.as_str() => data
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
            // TV2.2 hot/cold split (perf-plan §1.3): `try_exec_pure` is
            // `#[inline(always)]` and inlines into `drive()`'s hot dispatch loop.
            // TV1+TV2 enlarged these list arms (TypedVec dispatch + accounting),
            // which bloated the inlined loop and taxed I-cache/regalloc on EVERY
            // kernel — including non-list scalar ones. Each heavy list arm is now a
            // thin call into an `#[inline(never)]` cold helper so the inlined hot
            // loop shrinks back to its scalar core (arith/compare/branch/move/load).
            RegInstr::ListGet { dst, list, index } => {
                self.exec_list_get(base, *dst, *list, *index)?
            }
            RegInstr::ListLen { dst, list } => self.exec_list_len(base, *dst, *list)?,
            RegInstr::ListPush { dst, list, value } => {
                self.exec_list_push(base, *dst, *list, *value)?
            }
            RegInstr::ListAppend { dst, list, values } => {
                self.exec_list_append(base, *dst, *list, *values)?
            }
            RegInstr::ListClear { dst, list } => self.exec_list_clear(base, *dst, *list)?,
            RegInstr::ListPop { dst, list } => self.exec_list_pop(base, *dst, *list)?,
            RegInstr::ListRemoveAt { dst, list, index } => {
                self.exec_list_remove_at(base, *dst, *list, *index)?
            }
            RegInstr::ListSet {
                dst,
                list,
                index,
                value,
            } => self.exec_list_set(base, *dst, *list, *index, *value)?,
            RegInstr::MapGet { dst, map, key } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let key = map_key_from_value(self.reg(base + *key))?;
                let value = map
                    .borrow()
                    .get(&key)
                    .cloned()
                    .map(|value| VmValue::some(value))
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
                    old.map(|value| VmValue::some(value))
                        .unwrap_or(VmValue::OptionNone),
                );
            }
            RegInstr::MapRemove { dst, map, key } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let key = map_key_from_value(self.reg(base + *key))?;
                let old = map.borrow_mut().remove(&key);
                self.set_reg(
                    base + *dst,
                    old.map(|value| VmValue::some(value))
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

    // ── TV2.2 cold list-opcode bodies (perf-plan §1.3 hot/cold split) ──
    // These are the heavy/cold bodies the TV1+TV2 TypedVec work added to the
    // `try_exec_pure` list arms. `try_exec_pure` is `#[inline(always)]` and
    // inlines into `drive()`'s hot dispatch loop; keeping these bodies inline
    // bloated the loop and taxed I-cache/regalloc on every kernel (including
    // non-list scalar ones). Marked `#[inline(never)]` so each arm is a thin
    // call and the inlined hot loop stays at its scalar core. Semantics are
    // byte-for-byte identical to the previous inline arms.

    #[inline(never)]
    fn exec_list_get(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        index: usize,
    ) -> Result<(), EvalError> {
        let list = expect_list_ref(self.reg(base + list))?;
        let index = expect_usize_ref(self.reg(base + index))?;
        let value = list.borrow().get(index).ok_or_else(|| {
            EvalError::Runtime(format!("reg VM List.get index {index} out of bounds."))
        })?;
        self.set_reg(base + dst, value);
        Ok(())
    }

    #[inline(never)]
    fn exec_list_len(&mut self, base: usize, dst: usize, list: usize) -> Result<(), EvalError> {
        let len = expect_list_ref(self.reg(base + list))?.borrow().len();
        self.set_reg(base + dst, VmValue::Int(len as i64));
        Ok(())
    }

    #[inline(never)]
    fn exec_list_push(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        value: usize,
    ) -> Result<(), EvalError> {
        // TV2.1 hot path: a single mutable borrow that fuses the typed direct
        // push with amortized capacity-growth accounting. The push operates
        // straight on the flat `i64`/`f64` buffer with no intermediate `VmValue`
        // materialization, and `account_bytes` is only reached on the (geometric,
        // amortized O(1)) reallocations — not once per element. The adversarial
        // `while true { list.push(x) }` still trips the ceiling: capacity (>= len)
        // growth is charged, so the bound is conservative.
        let list = expect_list_ref(self.reg(base + list))?;
        let value = self.reg(base + value).clone();
        let grew = list.borrow_mut().checked_push_accounted(value).map_err(|v| {
            EvalError::Runtime(format!(
                "reg VM List.push element kind mismatch (got `{}`).",
                v.display()
            ))
        })?;
        if grew != 0 {
            self.account_bytes(grew)?;
        }
        self.set_reg(base + dst, VmValue::Unit);
        Ok(())
    }

    #[inline(never)]
    fn exec_list_append(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        values: usize,
    ) -> Result<(), EvalError> {
        // In-place to mirror `List.append(mut list, ...)`: clone the source first
        // (handles append-to-self), then extend the receiver's existing buffer so
        // a `mut` param propagates.
        let append_values = expect_list_ref(self.reg(base + values))?.borrow().clone();
        // Account against the *destination's* real layout, not the source's: a flat
        // `Ints`/`Floats` source extended into a `Boxed` receiver stores 16 B
        // `VmValue` slots, which `extend_accounted` bills correctly (the old
        // source-`elem_bytes` charge under-counted that mixed-layout case).
        let grew = expect_list_ref(self.reg(base + list))?
            .borrow_mut()
            .extend_accounted(append_values);
        if grew != 0 {
            self.account_bytes(grew)?;
        }
        self.set_reg(base + dst, VmValue::Unit);
        Ok(())
    }

    #[inline(never)]
    fn exec_list_clear(&mut self, base: usize, dst: usize, list: usize) -> Result<(), EvalError> {
        expect_list_ref(self.reg(base + list))?.borrow_mut().clear();
        self.set_reg(base + dst, VmValue::Unit);
        Ok(())
    }

    #[inline(never)]
    fn exec_list_pop(&mut self, base: usize, dst: usize, list: usize) -> Result<(), EvalError> {
        let value = expect_list_ref(self.reg(base + list))?
            .borrow_mut()
            .pop()
            .map(|value| VmValue::some(value))
            .unwrap_or(VmValue::OptionNone);
        self.set_reg(base + dst, value);
        Ok(())
    }

    #[inline(never)]
    fn exec_list_remove_at(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        index: usize,
    ) -> Result<(), EvalError> {
        let index = expect_int_ref(self.reg(base + index))?;
        let list = expect_list_ref(self.reg(base + list))?.clone();
        let mut borrowed = list.borrow_mut();
        let value = if index < 0 || index as usize >= borrowed.len() {
            VmValue::OptionNone
        } else {
            VmValue::some(borrowed.remove(index as usize))
        };
        drop(borrowed);
        self.set_reg(base + dst, value);
        Ok(())
    }

    #[inline(never)]
    fn exec_list_set(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        index: usize,
        value: usize,
    ) -> Result<(), EvalError> {
        let index = expect_int_ref(self.reg(base + index))?;
        let new_value = self.reg(base + value).clone();
        let list = expect_list_ref(self.reg(base + list))?.clone();
        let mut borrowed = list.borrow_mut();
        if index < 0 || index as usize >= borrowed.len() {
            return Err(EvalError::Runtime(format!(
                "reg VM List.set index {index} out of bounds for length {}.",
                borrowed.len()
            )));
        }
        borrowed.checked_set(index as usize, new_value).map_err(|v| {
            EvalError::Runtime(format!(
                "reg VM List.set element kind mismatch (got `{}`).",
                v.display()
            ))
        })?;
        drop(borrowed);
        self.set_reg(base + dst, VmValue::Unit);
        Ok(())
    }

    /// Grow the shared register stack so that `stack[..upto]` is addressable.
    /// The stack only ever grows; frames are reused in place.
    fn ensure_regs(&mut self, upto: usize) -> Result<(), EvalError> {
        if self.stack.len() < upto {
            let grew = upto - self.stack.len();
            self.stack.resize(upto, VmValue::Unit);
            self.written.resize(upto, false);
            // Account the shared register stack's growth: one `VmValue` slot plus
            // the `written` bool per new register. Deep recursion that slips under
            // the depth cap (huge per-frame register windows) is still bounded.
            self.account_bytes(grew * (std::mem::size_of::<VmValue>() + 1))?;
        }
        Ok(())
    }

    fn prepare_frame(&mut self, base: usize, regs: usize) -> Result<(), EvalError> {
        self.ensure_regs(base + regs)?;
        // Clear the new window's written bits AND release any stale value left in a
        // reused slot. The register stack is append-only and reuses windows in
        // place (execution spec §4 rule 4), so a slot may still physically hold the
        // previous frame's `VmValue` — including an `Rc` to a heap list/map/string/
        // closure. The written bit alone only blocks stale *reads*; dropping the
        // value here is what makes an unwritten slot non-retaining for ownership and
        // memory accounting (execution spec §4.1). Args are written immediately
        // after `prepare_frame` via `set_reg`, so this never clobbers live inputs.
        for index in base..base + regs {
            self.written[index] = false;
            // Assigning `Unit` drops whatever the reused slot held; `Unit` owns
            // nothing, so heap refcounts fall here rather than at next overwrite.
            self.stack[index] = VmValue::Unit;
        }
        Ok(())
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
        self.ensure_regs(base + function.regs)?;
        let floor = self.frames.len();
        self.push_frame(Frame {
            func: function,
            ip: 0,
            base,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
        })?;
        match self.drive(unit, floor)? {
            Outcome::Completed(value) => Ok(value),
            Outcome::Suspended => Err(EvalError::Runtime(
                "reg VM cannot suspend (await/blocking op) inside a synchronous context."
                    .to_string(),
            )),
        }
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
            {
                match self.try_native(&func, base) {
                    NativeAttempt::Completed(value) => {
                        let frame = self.frames.pop().expect("active frame");
                        self.apply_mut_writeback(&frame);
                        if self.frames.len() == floor {
                            return Ok(Outcome::Completed(value));
                        }
                        self.set_reg(frame.ret_dst, value);
                        continue 'frames;
                    }
                    // J0.2 precise resume: `try_native` already restored the live
                    // register window and set this frame's `ip` to the safepoint
                    // `resume_ip`. Re-enter the interpreter loop; because `ip != 0`
                    // the re-entry skips the native/tier-0 dispatch and resumes
                    // interpretation mid-function.
                    NativeAttempt::Resumed => continue 'frames,
                    // Fall through to tier-0 + the interpreter loop. The frame's
                    // `ip` is still `0`, so the function re-runs from the top.
                    NativeAttempt::Fallback => {}
                }
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

            // OSR auto-trigger (Pending #2): resolve this function's OSR-candidate
            // state ONCE (lazy, cached in `func.osr_state`) and hoist the candidate
            // loop header into a single per-frame local. For the overwhelming common
            // case — a function with no analyzable natural loop — this is a single
            // `Cell` read that yields `None`, and the per-instruction guard below is
            // then EXACTLY today's hoisted never-taken branch (`if let Some(..)`),
            // so the non-candidate interpreter hot path is byte-for-byte unchanged.
            // Only a candidate function pays the per-`ip` header compare, and only
            // until OSR fires (after which the loop runs native).
            //
            // `osr_eager` (set by `RSS_JIT_OSR` / a test override) keeps the forced
            // path: threshold 0, so the FIRST header hit triggers `try_osr` — this
            // preserves the differential OSR backend and the deterministic
            // forced-OSR tests. NOTE: candidacy is NOT gated on `native_status`: OSR
            // targets functions that are native-INELIGIBLE as a whole (the loop is
            // wrapped by non-native I/O); the verdict is cached in `native.osr_cache`.
            #[cfg(feature = "native-jit")]
            let (osr_candidate, osr_eager) = if self.native.is_some() {
                (
                    self.resolve_osr_candidate(&func),
                    self.native.as_ref().is_some_and(|n| n.osr_enabled),
                )
            } else {
                (None, false)
            };
            #[cfg(not(feature = "native-jit"))]
            let _osr_candidate: Option<usize> = None;

            while let Some(instr) = func.code.get(ip) {
                // OSR trigger: only candidate functions enter this arm (`None` for
                // every non-loop / unanalyzable function ⇒ a hoisted never-taken
                // branch, no per-instruction work). When the interpreter reaches the
                // candidate header, count the backedge; at the threshold (or
                // immediately when eager) fire `try_osr`. `try_osr` is total — on any
                // non-applicability it leaves the frame untouched and returns `false`,
                // and we mark `GaveUp` so we never recompile-probe in a tight loop.
                #[cfg(feature = "native-jit")]
                if let Some(header) = osr_candidate {
                    if ip == header {
                        let fire = if osr_eager {
                            true
                        } else {
                            // Count this backedge/header hit; fire at threshold.
                            match func.osr_state.get() {
                                OsrTrigger::Counting {
                                    header_ip,
                                    count,
                                    probe_cc,
                                } => {
                                    let next = count.saturating_add(1);
                                    if next >= osr_backedge_threshold() {
                                        true
                                    } else {
                                        func.osr_state.set(OsrTrigger::Counting {
                                            header_ip,
                                            count: next,
                                            probe_cc,
                                        });
                                        false
                                    }
                                }
                                // Already fired/gave up: stop probing.
                                _ => false,
                            }
                        };
                        if fire {
                            self.frames.last_mut().expect("active frame").ip = ip;
                            if self.try_osr(&func, base, ip) {
                                continue 'frames;
                            }
                            // Declined. In COUNTING (auto) mode we must NOT give up
                            // forever if the decline is only because a profile-guided
                            // closure-inline site is still PENDING — `try_osr` leaves
                            // that verdict uncached (re-probable) so a warmer retry can
                            // succeed. But we must also not re-probe forever: a
                            // structurally-present but **dynamically dead** (never-taken)
                            // `CallClosure` stays `pending` indefinitely (no profile
                            // entry), and its `call_count` never advances toward the
                            // `PROFILE_RECORD_LIMIT` freeze. So re-probe ONLY when the
                            // profile has made PROGRESS — `call_count` (the dynamic-call
                            // count) increased since the previous probe. That is
                            // intrinsically bounded: `call_count` is capped at
                            // `PROFILE_RECORD_LIMIT`, so there can be at most that many
                            // progress-resets before the profile freezes (⇒ not pending
                            // ⇒ stable) or stalls (⇒ no progress ⇒ GaveUp here).
                            //   - PENDING **and** progressed ⇒ reset (record the new
                            //     progress point as `probe_cc`).
                            //   - STABLE decline, OR pending-but-stalled/dead ⇒ `GaveUp`.
                            // EAGER mode keeps firing every header hit (the cached `None`
                            // makes a stable retry cheap; a pending profile is re-probed).
                            if !osr_eager {
                                let cc = func.call_count.get();
                                let prev_probe_cc = match func.osr_state.get() {
                                    OsrTrigger::Counting { probe_cc, .. } => probe_cc,
                                    _ => cc,
                                };
                                if cc > prev_probe_cc
                                    && native_translation_pending_on_profile(&self.unit, &func)
                                {
                                    func.osr_state.set(OsrTrigger::Counting {
                                        header_ip: ip,
                                        count: 0,
                                        probe_cc: cc,
                                    });
                                } else {
                                    func.osr_state.set(OsrTrigger::GaveUp);
                                }
                            }
                        }
                    }
                }
                self.tick()?;
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
                            self.prepare_frame(next_base, callee.regs)?;
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
                            self.push_frame(Frame {
                                func: callee,
                                ip: 0,
                                base: next_base,
                                ret_dst: base + *dst,
                                mut_writeback,
                            })?;
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
                                VmValue::Struct(data) => Some(data.name().clone()),
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
                            // J1 type feedback (warm-gated + bounded inside the
                            // helper): record the resolved callee identity at this
                            // site. The dispatch DECISION above (`callee_id`) is
                            // unchanged — we only observe it. `ip` was already
                            // advanced past this instruction, so its index is
                            // `ip - 1`.
                            record_call_site(&func, ip - 1, callee_id as u64, true);
                            let callee = Rc::clone(&unit.functions[callee_id]);
                            self.prepare_frame(next_base, callee.regs)?;
                            for (index, reg) in args.iter().enumerate() {
                                let value = self.reg(base + *reg).clone();
                                self.set_reg(next_base + index, value);
                            }
                            let mut_writeback = mut_args
                                .iter()
                                .map(|&pos| (base + args[pos], next_base + pos))
                                .collect();
                            self.frames.last_mut().expect("active frame").ip = ip;
                            self.push_frame(Frame {
                                func: callee,
                                ip: 0,
                                base: next_base,
                                ret_dst: base + *dst,
                                mut_writeback,
                            })?;
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
                                        // Reap the slot — a handle is awaited at most
                                        // once (RS0030), so its value is now consumed
                                        // and the slot must not linger (else the task
                                        // table grows unboundedly across loop rounds,
                                        // turning the scheduler's per-step scans O(n²)).
                                        Some(result) => {
                                            self.tasks.remove(&task);
                                            self.set_reg(base + *dst, result);
                                        }
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
                        RegInstr::CallClosure {
                            dst,
                            closure,
                            args,
                            mut_args,
                        } => {
                            let closure = match self.reg(base + *closure) {
                                VmValue::Closure(closure) => Rc::clone(closure),
                                other => {
                                    return Err(EvalError::Runtime(format!(
                                        "reg VM expected Closure, got `{}`.",
                                        other.display()
                                    )));
                                }
                            };
                            // J1 type feedback (warm-gated + bounded inside the
                            // helper): the closure's underlying function id is its
                            // stable identity (one callee ⇒ monomorphic). Recording
                            // does not change which closure runs; `ip` already
                            // points past this instruction, so its index is
                            // `ip - 1`.
                            record_call_site(
                                &func,
                                ip - 1,
                                closure.function as u64,
                                closure_captures_all_scalar(&closure),
                            );
                            let result = self.call_closure_from_regs(
                                unit, &closure, args, mut_args, base, next_base,
                            )?;
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
                            // Sort a flat `Ints` list in place (kind-preserving); any
                            // other kind promotes to the boxed view and sorts via the
                            // shared `VmValue` comparator (parity-identical).
                            if let TypedVec::Ints(values) = &mut *borrowed {
                                values.sort_unstable();
                            } else {
                                sort_vm_values(borrowed.as_boxed_mut())?;
                            }
                            drop(borrowed);
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::ListSortBy {
                            dst,
                            list,
                            key,
                            compare,
                        } => {
                            let values = expect_list_ref(self.reg(base + *list))?.borrow().to_vec();
                            let key = expect_closure_rc(self.reg(base + *key))?;
                            let compare = expect_closure_rc(self.reg(base + *compare))?;
                            let sorted =
                                self.sort_list_by_closure(unit, values, &key, &compare, next_base)?;
                            self.set_reg(base + *dst, VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(sorted)))));
                        }
                        RegInstr::ListSortWith { dst, list, compare } => {
                            // Sort a detached copy first so the comparator closure can read
                            // the list without a RefCell double-borrow, then overwrite the
                            // receiver's buffer in place so `mut list` propagates.
                            let mut values =
                                expect_list_ref(self.reg(base + *list))?.borrow().to_vec();
                            let compare = expect_closure_rc(self.reg(base + *compare))?;
                            self.sort_list_with_closure(unit, &mut values, &compare, next_base)?;
                            *expect_list_ref(self.reg(base + *list))?.borrow_mut() =
                                TypedVec::from_values(values);
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
                                .map(|value| VmValue::some(value))
                                .unwrap_or(VmValue::OptionNone);
                            self.set_reg(base + *dst, value);
                        }
                        RegInstr::DequePopFront { dst, deque } => {
                            let value = expect_deque_ref(self.reg(base + *deque))?
                                .borrow_mut()
                                .pop_front() // O(1), unlike the old `Vec::remove(0)`
                                .map(|value| VmValue::some(value))
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
                            expect_map_ref(self.reg(base + *set))?.borrow_mut().clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SetForEach { dst, set, callback } => {
                            let set = expect_map_ref(self.reg(base + *set))?;
                            let callback = expect_closure_rc(self.reg(base + *callback))?;
                            let values = set
                                .borrow()
                                .keys()
                                .map(vm_value_from_map_key)
                                .collect::<Vec<_>>();
                            for value in values {
                                let _ = self.call_closure_one(unit, &callback, value, next_base)?;
                            }
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SetInsert { dst, set, value } => {
                            let key = map_key_from_value(self.reg(base + *value))?;
                            let map = expect_map_ref(self.reg(base + *set))?;
                            let inserted = map.borrow_mut().insert(key, VmValue::Unit).is_none();
                            self.set_reg(base + *dst, VmValue::Bool(inserted));
                        }
                        RegInstr::SetRemove { dst, set, value } => {
                            let key = map_key_from_value(self.reg(base + *value))?;
                            let map = expect_map_ref(self.reg(base + *set))?;
                            let removed = map.borrow_mut().remove(&key).is_some();
                            self.set_reg(base + *dst, VmValue::Bool(removed));
                        }
                        RegInstr::SortedSetClear { dst, set } => {
                            expect_list_ref(self.reg(base + *set))?.borrow_mut().clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SortedSetInsert { dst, set, value } => {
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *set))?;
                            let inserted = sorted_insert_vm(list.borrow_mut().as_boxed_mut(), value)?;
                            self.set_reg(base + *dst, VmValue::Bool(inserted));
                        }
                        RegInstr::SortedSetRemove { dst, set, value } => {
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *set))?;
                            let removed = sorted_remove_vm(list.borrow_mut().as_boxed_mut(), &value)?;
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
                            sorted_map_insert_in_place(list.borrow_mut().as_boxed_mut(), key, value)?;
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SortedMapRemove { dst, map, key } => {
                            let key = self.reg(base + *key).clone();
                            let list = expect_list_ref(self.reg(base + *map))?;
                            let removed = sorted_map_remove_in_place(list.borrow_mut().as_boxed_mut(), &key)?;
                            self.set_reg(
                                base + *dst,
                                removed
                                    .map(|value| VmValue::some(value))
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
                                VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => {
                                    self.set_reg(
                                        base + *dst,
                                        value.unwrap_some().expect("Some arm yields a payload"),
                                    );
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
            .get(data.name().as_ref())
            .copied()
        else {
            return Ok(());
        };
        let callee = Rc::clone(&unit.functions[function_id]);
        self.prepare_frame(base, callee.regs)?;
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
                data.name(),
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

fn expect_sorted_map_entries(value: &VmValue) -> Result<Vec<(VmValue, VmValue)>, EvalError> {
    let entries = expect_list_ref(value)?;
    entries
        .borrow()
        .iter()
        .map(|entry| {
            let pair = expect_list_ref(&entry)?;
            let pair = pair.borrow();
            if pair.len() != 2 {
                return Err(EvalError::Runtime(format!(
                    "reg VM expected SortedMap entry, got `{}`.",
                    entry.display()
                )));
            }
            Ok((pair.get(0).unwrap(), pair.get(1).unwrap()))
        })
        .collect()
}

fn join_string_values(values: &TypedVec, separator: &str) -> Result<String, EvalError> {
    Ok(values
        .iter()
        .map(|value| expect_string_ref(&value).map(str::to_string))
        .collect::<Result<Vec<_>, _>>()?
        .join(separator))
}

fn list_item_at(
    list: &Rc<RefCell<TypedVec>>,
    index: usize,
    operation: &str,
) -> Result<VmValue, EvalError> {
    let values = list.borrow();
    values.get(index).ok_or_else(|| {
        EvalError::Runtime(format!(
            "reg VM {operation} observed list length change at index {index}."
        ))
    })
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
        VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) | VmValue::OptionNone => {
            Ok(value)
        }
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Option, got `{}`.",
            other.display()
        ))),
    }
}

fn vm_value_from_map_key(key: &VmMapKey) -> VmValue {
    key.value().clone()
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

const POOL_LEASE_ID_FIELD: &str = "__rsscript_vm_pool_id";
const POOL_LEASE_DISCARDED_FIELD: &str = "__rsscript_vm_pool_discarded";

fn pool_error_message(value: &VmValue) -> Option<String> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "PoolError" => data
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
        Rc::clone(data.name()),
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
        Rc::clone(data.name()),
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
        value: VmValue::Struct(Rc::new(VmStruct::from_named(Rc::clone(data.name()), fields))),
    }))
}

fn clock_system_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or(std::time::Duration::ZERO)
        .as_millis() as i64
}

fn deadline_after_ms(ms: i64) -> i64 {
    clock_system_unix_ms().saturating_add(ms.max(0))
}

fn config_name_from_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("default")
        .to_string()
}

fn rules_from_text(text: &str) -> Vec<VmValue> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(rule_value)
        .collect()
}

enum WebSocketExpectedFrame {
    Text,
    Binary,
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

#[cfg(test)]
mod tests;
