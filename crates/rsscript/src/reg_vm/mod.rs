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
mod lower;
mod model;
mod exec;
mod resource_io;
mod resources;
mod scheduler;
mod tier;
mod runtime_resources;
mod runtime_values;
mod value_access;
mod value_convert;
mod value_ops;
#[cfg(feature = "native-jit")]
mod native;
#[cfg(feature = "native-jit")]
use native::*;
pub(crate) use model::*;
pub(crate) use lower::*;
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
