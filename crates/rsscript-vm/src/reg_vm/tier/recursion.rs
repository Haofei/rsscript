use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarSlotKind {
    Unknown,
    Int,
    Bool,
    Unit,
}

pub(super) fn self_recursive_scalar_jit_candidate(
    unit: &RegUnit,
    jit_state: &mut JitState,
    function_id: usize,
) -> SelfRecursionKind {
    let Some(func) = unit.functions.get(function_id) else {
        return SelfRecursionKind::Ineligible;
    };
    if let Some(cached) = jit_state.self_recursion_kind(func) {
        return cached;
    }
    let kind = compute_self_recursive_scalar_jit_candidate(unit, function_id);
    jit_state.set_self_recursion_kind(func, kind);
    kind
}

fn require_scalar_kind(
    kinds: &mut [ScalarSlotKind],
    reg: usize,
    kind: ScalarSlotKind,
) -> Option<bool> {
    let slot = kinds.get_mut(reg)?;
    match (*slot, kind) {
        (ScalarSlotKind::Unknown, known) => {
            *slot = known;
            Some(true)
        }
        (known, expected) if known == expected => Some(false),
        _ => None,
    }
}

/// A call argument, call result, or return value must be a scalar VALUE kind —
/// `Int` or `Bool` (both i64-represented). `Unknown` is permitted here (it is
/// constrained by the reg's other uses, or defaults to `Int` in native lowering);
/// `Unit` is rejected. Used to widen recursion eligibility from Int-only to any
/// i64 scalar without forcing a specific kind at calls/returns.
fn require_scalar_value(kinds: &[ScalarSlotKind], reg: usize) -> Option<bool> {
    match kinds.get(reg)? {
        ScalarSlotKind::Int | ScalarSlotKind::Bool | ScalarSlotKind::Unknown => Some(false),
        ScalarSlotKind::Unit => None,
    }
}

fn propagate_same_kind(kinds: &mut [ScalarSlotKind], dst: usize, src: usize) -> Option<bool> {
    let dst_kind = *kinds.get(dst)?;
    let src_kind = *kinds.get(src)?;
    match (dst_kind, src_kind) {
        (ScalarSlotKind::Unknown, ScalarSlotKind::Unknown) => Some(false),
        (ScalarSlotKind::Unknown, known) => {
            kinds[dst] = known;
            Some(true)
        }
        (known, ScalarSlotKind::Unknown) => {
            kinds[src] = known;
            Some(true)
        }
        (left, right) if left == right => Some(false),
        _ => None,
    }
}

fn compute_self_recursive_scalar_jit_candidate(
    unit: &RegUnit,
    function_id: usize,
) -> SelfRecursionKind {
    let group: std::collections::HashSet<usize> = std::iter::once(function_id).collect();
    // Self-recursion admits the i64-representable scalar kinds (Int, Bool): on a
    // depth-cap/compile bail it falls back to the tier-0 i64 scalar executor, which
    // runs the body as an i64 machine and wraps the result per the return kind. Float
    // (and any non-i64 kind) is not classified here — it routes through the general
    // native path (which falls back to the full interpreter, not the i64 executor).
    match compute_recursive_int_member_inner(unit, function_id, &group) {
        Some(ScalarSlotKind::Int) => SelfRecursionKind::Int,
        Some(ScalarSlotKind::Bool) => SelfRecursionKind::Bool,
        _ => SelfRecursionKind::Ineligible,
    }
}

/// Scalar-`Int` recursion analysis for one member of a recursive `group`: the body
/// must be all-scalar-`Int`, return `Int`, and every `CallKnown` must target a group
/// member (self for self-recursion, or any sibling for mutual recursion) with scalar
/// args and matching arity. `group = {function_id}` is the self-recursive case.
/// Scalar recursion analysis for one member of a recursive `group`. Returns the
/// member's RETURN value kind (`Int` or `Bool`) when it is a valid all-scalar-i64
/// recursive machine whose every `CallKnown` targets a group member with scalar
/// value args; `None` when ineligible. The caller decides which return kinds it
/// accepts (self-recursion: `Int` only, since its tier-0 fallback is an i64 machine;
/// mutual recursion: `Int` or `Bool`, since its fallback is the full interpreter).
fn compute_recursive_int_member_inner(
    unit: &RegUnit,
    function_id: usize,
    group: &std::collections::HashSet<usize>,
) -> Option<ScalarSlotKind> {
    let func = unit.functions.get(function_id)?;
    if func.captures != 0 || func.params > func.regs {
        return None;
    }
    let mut kinds = vec![ScalarSlotKind::Unknown; func.regs];
    let reachable = scalar_reachable_instructions(&func.code);
    let mut saw_self_call = false;
    let mut return_srcs: Vec<usize> = Vec::new();

    loop {
        let mut changed = false;
        for (ip, instr) in func.code.iter().enumerate() {
            if !reachable[ip] {
                continue;
            }
            let step = match instr {
                RegInstr::LoadUnit { dst } => {
                    require_scalar_kind(&mut kinds, *dst, ScalarSlotKind::Unit)
                }
                RegInstr::LoadInt { dst, .. } => {
                    require_scalar_kind(&mut kinds, *dst, ScalarSlotKind::Int)
                }
                RegInstr::LoadBool { dst, .. } => {
                    require_scalar_kind(&mut kinds, *dst, ScalarSlotKind::Bool)
                }
                RegInstr::Move { dst, src } => propagate_same_kind(&mut kinds, *dst, *src),
                RegInstr::DeepCopy { reg } | RegInstr::DeepCopyElided { reg } => {
                    kinds.get(*reg).map(|_| false)
                }
                RegInstr::AddInt { dst, lhs, rhs }
                | RegInstr::SubInt { dst, lhs, rhs }
                | RegInstr::MulInt { dst, lhs, rhs }
                | RegInstr::DivInt { dst, lhs, rhs }
                | RegInstr::ModInt { dst, lhs, rhs } => {
                    let a = require_scalar_kind(&mut kinds, *lhs, ScalarSlotKind::Int)?;
                    let b = require_scalar_kind(&mut kinds, *rhs, ScalarSlotKind::Int)?;
                    let c = require_scalar_kind(&mut kinds, *dst, ScalarSlotKind::Int)?;
                    Some(a || b || c)
                }
                RegInstr::LessInt { dst, lhs, rhs }
                | RegInstr::LessEqualInt { dst, lhs, rhs }
                | RegInstr::GreaterInt { dst, lhs, rhs }
                | RegInstr::GreaterEqualInt { dst, lhs, rhs } => {
                    let a = require_scalar_kind(&mut kinds, *lhs, ScalarSlotKind::Int)?;
                    let b = require_scalar_kind(&mut kinds, *rhs, ScalarSlotKind::Int)?;
                    let c = require_scalar_kind(&mut kinds, *dst, ScalarSlotKind::Bool)?;
                    Some(a || b || c)
                }
                RegInstr::Equal { dst, lhs, rhs } | RegInstr::NotEqual { dst, lhs, rhs } => {
                    let a = require_scalar_kind(&mut kinds, *lhs, ScalarSlotKind::Int)?;
                    let b = require_scalar_kind(&mut kinds, *rhs, ScalarSlotKind::Int)?;
                    let c = require_scalar_kind(&mut kinds, *dst, ScalarSlotKind::Bool)?;
                    Some(a || b || c)
                }
                RegInstr::Jump { target } => func.code.get(*target).map(|_| false),
                RegInstr::JumpIfBool { cond, target, .. } => {
                    let a = require_scalar_kind(&mut kinds, *cond, ScalarSlotKind::Bool)?;
                    func.code.get(*target)?;
                    Some(a)
                }
                RegInstr::JumpIfIntCompare {
                    lhs, rhs, target, ..
                } => {
                    let a = require_scalar_kind(&mut kinds, *lhs, ScalarSlotKind::Int)?;
                    let b = require_scalar_kind(&mut kinds, *rhs, ScalarSlotKind::Int)?;
                    func.code.get(*target)?;
                    Some(a || b)
                }
                RegInstr::CallKnown {
                    dst,
                    function,
                    args,
                    mut_args,
                } if group.contains(function)
                    && mut_args.is_empty()
                    && unit
                        .functions
                        .get(*function)
                        .is_some_and(|f| f.params == args.len()) =>
                {
                    saw_self_call = true;
                    // Call args/result are scalar VALUE kinds (Int or Bool); the exact
                    // per-call kinds are pinned later by translate via declared sigs.
                    let mut local_changed = require_scalar_value(&kinds, *dst)?;
                    for &arg in args {
                        local_changed |= require_scalar_value(&kinds, arg)?;
                    }
                    Some(local_changed)
                }
                RegInstr::Return { src } => {
                    if !return_srcs.contains(src) {
                        return_srcs.push(*src);
                    }
                    require_scalar_value(&kinds, *src)
                }
                _ => return None,
            };
            let Some(step_changed) = step else {
                return None;
            };
            changed |= step_changed;
        }
        if !changed {
            break;
        }
    }

    if !saw_self_call {
        return None;
    }
    if !kinds
        .iter()
        .take(func.params)
        .all(|kind| matches!(kind, ScalarSlotKind::Int | ScalarSlotKind::Bool))
    {
        return None;
    }
    // The member's return kind is fixed by its returns. A return of a recursive
    // call's result is `Unknown` here (its kind is the callee's, pinned later by
    // translate via declared sigs) — skip those; the concrete returns (literals,
    // comparisons) must all agree on one scalar value kind, and there must be at
    // least one. `Unit` (or any non-value) makes the member ineligible.
    let mut return_kind = None;
    for &src in &return_srcs {
        match *kinds.get(src)? {
            ScalarSlotKind::Unknown => continue,
            kind @ (ScalarSlotKind::Int | ScalarSlotKind::Bool) => match return_kind {
                None => return_kind = Some(kind),
                Some(prev) if prev == kind => {}
                Some(_) => return None,
            },
            ScalarSlotKind::Unit => return None,
        }
    }
    return_kind
}

fn scalar_reachable_instructions(code: &[RegInstr]) -> Vec<bool> {
    let mut reachable = vec![false; code.len()];
    let mut stack = vec![0usize];
    while let Some(ip) = stack.pop() {
        if ip >= code.len() || reachable[ip] {
            continue;
        }
        reachable[ip] = true;
        match &code[ip] {
            RegInstr::Return { .. } | RegInstr::RuntimeError { .. } => {}
            RegInstr::Jump { target } => stack.push(*target),
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                stack.push(*target);
                stack.push(ip + 1);
            }
            RegInstr::MatchOption {
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
            RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                stack.push(*some_ip);
                stack.push(*none_ip);
            }
            _ => stack.push(ip + 1),
        }
    }
    reachable
}
