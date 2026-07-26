//! Native-JIT IR producers and OSR-loop detection. Pure code-movement out of
//! `reg_vm::mod` (Phase 2); every item retains its original
//! `#[cfg(feature = "native-jit")]` attribute verbatim.
#![allow(unused_imports)]

use super::super::*;
use super::passes::*;
use super::*;
use crate::text_util::strip_fresh_type;
use std::collections::{HashMap, HashSet};

#[cfg(feature = "native-jit")]
#[derive(Clone)]
pub(in crate::reg_vm) struct NativeCompiledCallee {
    pub(in crate::reg_vm) id: vm_jit::CompiledId,
    pub(in crate::reg_vm) ret_ty: NativeTy,
    pub(in crate::reg_vm) param_tys: Vec<NativeTy>,
}

#[cfg(feature = "native-jit")]
fn native_call_mut_args_supported(mut_args: &[usize], param_tys: &[NativeTy]) -> bool {
    mut_args.iter().all(|&pos| {
        param_tys
            .get(pos)
            .is_some_and(|ty| matches!(ty, NativeTy::FlatIntMut | NativeTy::FlatFloatMut))
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_whole_function_region_exit(code: &[RegInstr]) -> usize {
    let reachable = native_reachable_instructions(code);
    let mut exit = code.len();
    while exit > 0 && !reachable[exit - 1] {
        exit -= 1;
    }
    exit
}

#[cfg(feature = "native-jit")]
fn native_profile_guidance_with_analysis(
    func: &RegFunction,
    code: &[RegInstr],
    ip_map: &[usize],
    analysis: &NativeRegionAnalysis,
) -> NativeProfileGuidance {
    let Ok(profile) = func.profile.try_borrow() else {
        return NativeProfileGuidance::default();
    };
    let Some(profile) = profile.as_ref() else {
        return NativeProfileGuidance::default();
    };
    analysis.profile_guidance(code, profile, ip_map)
}

#[cfg(feature = "native-jit")]
fn native_osr_profile_guidance(
    func: &RegFunction,
    code: &[RegInstr],
    n_regs: usize,
    lp: OsrLoop,
    ip_map: &[usize],
) -> NativeProfileGuidance {
    let analysis_exit = lp
        .exit
        .checked_add(1)
        .filter(|&exit| exit <= code.len())
        .unwrap_or(lp.exit);
    let Some(analysis) =
        NativeRegionAnalysis::compute_region(code, n_regs, lp.header, analysis_exit)
    else {
        return NativeProfileGuidance::default();
    };
    let mut guidance = native_profile_guidance_with_analysis(func, code, ip_map, &analysis);
    guidance.hot_branch_edges.retain(|ip, hot_target| {
        let cold_ip = match code.get(*ip) {
            Some(RegInstr::JumpIfBool { target, .. })
            | Some(RegInstr::JumpIfIntCompare { target, .. }) => {
                if *hot_target {
                    *ip + 1
                } else {
                    *target
                }
            }
            _ => return false,
        };
        cold_ip != lp.exit
    });
    guidance
}

#[cfg(feature = "native-jit")]
fn native_declared_type_name_to_ty(type_name: &str) -> Option<NativeTy> {
    let root = type_root_name(strip_fresh_type(type_name));
    match root {
        "Int" => Some(NativeTy::Int),
        "Bool" => Some(NativeTy::Bool),
        "Float" => Some(NativeTy::Float),
        "Unit" => None,
        name if name.len() == 1 && name.chars().all(|ch| ch.is_ascii_uppercase()) => None,
        _ => Some(NativeTy::Handle),
    }
}

#[cfg(feature = "native-jit")]
fn native_set_compiled_call_arg_ty(
    ty: &mut [Option<NativeTy>],
    reg: usize,
    expected: NativeTy,
    n_params: usize,
    changed: &mut bool,
) -> bool {
    match (ty[reg], expected) {
        (
            Some(NativeTy::Handle),
            NativeTy::FlatInt | NativeTy::FlatIntMut | NativeTy::FlatFloat | NativeTy::FlatFloatMut,
        ) if reg < n_params => {
            ty[reg] = Some(expected);
            *changed = true;
            true
        }
        _ => native_set_ty(ty, reg, expected, changed),
    }
}

#[cfg(feature = "native-jit")]
fn native_set_list_read_base_ty(
    ty: &mut [Option<NativeTy>],
    reg: usize,
    changed: &mut bool,
) -> bool {
    match ty[reg] {
        Some(
            NativeTy::FlatInt | NativeTy::FlatIntMut | NativeTy::FlatFloat | NativeTy::FlatFloatMut,
        ) => true,
        _ => native_set_ty(ty, reg, NativeTy::Handle, changed),
    }
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
pub(in crate::reg_vm) fn translate_to_native_jit(
    unit: &RegUnit,
    func: &RegFunction,
) -> Option<(
    vm_jit::JitFunction,
    NativeTy,
    Vec<NativeTy>,
    Vec<Rc<String>>,
    bool,
)> {
    translate_to_native_jit_with_compiled_callees(unit, func, &HashMap::new())
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn translate_to_native_jit_with_compiled_callees(
    unit: &RegUnit,
    func: &RegFunction,
    compiled_callees: &HashMap<usize, NativeCompiledCallee>,
) -> Option<(
    vm_jit::JitFunction,
    NativeTy,
    Vec<NativeTy>,
    Vec<Rc<String>>,
    bool,
)> {
    translate_to_native_jit_with_calls(
        unit,
        func,
        compiled_callees,
        &HashSet::new(),
        &HashMap::new(),
    )
}

/// Like [`translate_to_native_jit_with_compiled_callees`], but `self_call_sites`
/// names the original ips of `CallKnown` instructions that call `func` itself —
/// emitted as `JitInstr::CallSelf` for native self-recursion (native-call-ABI
/// slice 3) — and `group_call_sites` maps the original ip of a `CallKnown` to a
/// *mutually-recursive group member* to that member's group index, emitted as
/// `JitInstr::CallGroup` (slice 4). Such functions use re-run-from-top deopt
/// (`precise_resume_safe` forced off), so a bail anywhere in the recursion unwinds
/// to the interpreter.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn translate_to_native_jit_with_calls(
    unit: &RegUnit,
    func: &RegFunction,
    compiled_callees: &HashMap<usize, NativeCompiledCallee>,
    self_call_sites: &HashSet<usize>,
    group_call_sites: &HashMap<usize, u32>,
) -> Option<(
    vm_jit::JitFunction,
    NativeTy,
    Vec<NativeTy>,
    Vec<Rc<String>>,
    bool,
)> {
    use vm_jit::{JitCompare, JitInstr};

    if func.captures != 0 {
        return None;
    }
    // Inline straight-line leaf calls first, so a function that only leaves the
    // native subset via small helper calls still qualifies (the calls vanish).
    // Whole-function native translation: the ENTIRE body runs natively, so every
    // call must be inlinable (`None` region ⇒ whole function in-scope).
    let preserve_call_known: HashSet<usize> = compiled_callees
        .keys()
        .copied()
        .chain(self_call_sites.iter().copied())
        .chain(group_call_sites.keys().copied())
        .collect();
    let (code, n_regs, mut ip_map) = native_inline_leaf_calls_preserving_known_calls(
        unit,
        func,
        false,
        None,
        &preserve_call_known,
    )?;
    let region_exit = native_whole_function_region_exit(&code);
    let (code, n_regs, next_ip_map) =
        native_elide_readonly_full_list_slices_in_region(&code, n_regs, 0, region_exit)?;
    ip_map = native_compose_ip_maps(&ip_map, &next_ip_map)?;
    let region_exit = native_whole_function_region_exit(&code);
    let (code, n_regs, next_ip_map) =
        native_lower_checked_payload_intrinsics_in_region(&code, n_regs, 0, region_exit)?;
    ip_map = native_compose_ip_maps(&ip_map, &next_ip_map)?;
    // J3/J4: scalar-replace non-escaping aggregate values on the fully-inlined body,
    // using the same transform order as OSR. Result runs before Option because it
    // tolerates Option ops while dissolving always-Ok Results; Option is stricter and
    // expects only native-subset instructions plus Option ops. Variant and struct SR
    // then remove user aggregate constructors/accessors exposed by inlining.
    let region_exit = native_whole_function_region_exit(&code);
    let (code, n_regs, next_ip_map, _recipes) =
        native_scalar_replace_results_in_region(&code, n_regs, 0, region_exit)?;
    ip_map = native_compose_ip_maps(&ip_map, &next_ip_map)?;
    let (code, n_regs, scalar_payload_regs, next_ip_map) =
        native_scalar_replace_options(&code, n_regs)?;
    ip_map = native_compose_ip_maps(&ip_map, &next_ip_map)?;
    let region_exit = native_whole_function_region_exit(&code);
    let (code, n_regs, next_ip_map, _recipes) =
        native_scalar_replace_variants_in_region(&code, n_regs, 0, region_exit)?;
    ip_map = native_compose_ip_maps(&ip_map, &next_ip_map)?;
    let region_exit = native_whole_function_region_exit(&code);
    let (code, n_regs, next_ip_map, _recipes) =
        native_scalar_replace_structs_in_region(&code, n_regs, 0, region_exit)?;
    ip_map = native_compose_ip_maps(&ip_map, &next_ip_map)?;
    if func.params > n_regs {
        return None;
    }
    // Reachability from `ip == 0` over the control-flow graph. The lowerer appends
    // a defensive `LoadUnit; Return(unit)` to every function body even when the
    // body always returns earlier; that tail is unreachable. Restricting analysis
    // (and codegen) to reachable instructions lets such functions still qualify —
    // dead instructions become `Nop`.
    let analysis = NativeRegionAnalysis::compute_prefix(&code, n_regs, 0, code.len())?;
    let reachable = analysis.reachable_mask();
    let profile_guidance = native_profile_guidance_with_analysis(func, &code, &ip_map, &analysis);
    let profile_hot_branch_edges = &profile_guidance.hot_branch_edges;

    // Every *reachable* instruction must be in the native subset.
    for (i, instr) in code.iter().enumerate() {
        let compiled_call = matches!(instr, RegInstr::CallKnown { .. })
            && (compiled_callees.contains_key(&ip_map[i])
                || self_call_sites.contains(&ip_map[i])
                || group_call_sites.contains_key(&ip_map[i]));
        if reachable[i] && !compiled_call && !native_subset_instruction(instr) {
            return None;
        }
    }

    // Type inference by unification (fixpoint, to handle loop back-edges).
    // Parameters start untyped and acquire their type from the operands they are
    // combined with — so a float-parameter function is inferred correctly rather
    // than forced to `Int`.
    let mut ty: Vec<Option<NativeTy>> = vec![None; n_regs];
    let declared_signature = unit.native_signatures.get(&func.name);
    if let Some(signature) = declared_signature {
        for (reg, declared) in signature.params.iter().take(func.params).enumerate() {
            if let Some(native_ty) = native_declared_type_name_to_ty(declared) {
                ty[reg] = Some(native_ty);
            }
        }
    }
    let declared_return_ty = declared_signature
        .and_then(|signature| signature.return_type.as_deref())
        .and_then(native_declared_type_name_to_ty);
    // The whole-function tier has no per-call runtime param classification (unlike OSR's
    // `try_osr`), so it cannot identify a heap-typed param register here (there is no
    // `heap_param` predicate on this path; heap collection key/value PARAMS are not specially
    // typed). A heap key/value LOCAL still flows its `Handle` type from its in-region definition.
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
                RegInstr::LoadString { dst, .. } => native_set_ty(ty, *dst, NativeTy::Handle, c),
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
                RegInstr::Return { src } if declared_return_ty.is_some() => {
                    native_set_ty(ty, *src, declared_return_ty.unwrap(), c)
                }
                RegInstr::CallKnown {
                    dst,
                    args,
                    mut_args,
                    ..
                } if self_call_sites.contains(&ip_map[i]) => {
                    // Self-recursive call (native-call-ABI slice 3): the callee *is*
                    // this function, so its args and result take this function's own
                    // declared parameter/return types — the same signature-driven
                    // typing the group-call arm uses for mutual recursion. A
                    // Bool/Float-typed self-recursive function thus types its self-call
                    // correctly instead of being forced to `Int`. Params/return left
                    // undeclared flow from their other uses via unification.
                    let mut ok = mut_args.is_empty() && args.len() == func.params;
                    for (pi, arg) in args.iter().enumerate() {
                        if let Some(pty) = declared_signature
                            .and_then(|s| s.params.get(pi))
                            .and_then(|d| native_declared_type_name_to_ty(d))
                        {
                            ok = ok && native_set_ty(ty, *arg, pty, c);
                        }
                    }
                    if let Some(ret) = declared_return_ty {
                        ok = ok && native_set_ty(ty, *dst, ret, c);
                    }
                    ok
                }
                RegInstr::CallKnown {
                    dst,
                    function,
                    args,
                    mut_args,
                } if group_call_sites.contains_key(&ip_map[i]) => {
                    // Mutually-recursive group call (native-call-ABI slice 4): args and
                    // result take the *callee* member's declared parameter/return types,
                    // so a Bool-returning member (e.g. `is_even`/`is_odd`) types its
                    // result `Bool` rather than being forced to `Int`.
                    let callee_sig = unit
                        .functions
                        .get(*function)
                        .and_then(|cf| unit.native_signatures.get(&cf.name));
                    let mut ok = mut_args.is_empty();
                    for (pi, arg) in args.iter().enumerate() {
                        if let Some(pty) = callee_sig
                            .and_then(|s| s.params.get(pi))
                            .and_then(|d| native_declared_type_name_to_ty(d))
                        {
                            ok = ok && native_set_ty(ty, *arg, pty, c);
                        }
                    }
                    if let Some(ret) = callee_sig
                        .and_then(|s| s.return_type.as_deref())
                        .and_then(native_declared_type_name_to_ty)
                    {
                        ok = ok && native_set_ty(ty, *dst, ret, c);
                    }
                    ok
                }
                RegInstr::CallKnown {
                    dst,
                    args,
                    mut_args,
                    ..
                } if compiled_callees.contains_key(&ip_map[i]) => {
                    let callee = compiled_callees.get(&ip_map[i])?;
                    let mut ok = args.len() == callee.param_tys.len()
                        && native_call_mut_args_supported(mut_args, &callee.param_tys);
                    if ok {
                        for (arg, expected) in args.iter().zip(callee.param_tys.iter()) {
                            ok = ok
                                && native_set_compiled_call_arg_ty(
                                    ty,
                                    *arg,
                                    *expected,
                                    func.params,
                                    c,
                                );
                        }
                    }
                    ok && native_set_ty(ty, *dst, callee.ret_ty, c)
                }
                RegInstr::JumpIfBool { cond, .. } => native_set_ty(ty, *cond, NativeTy::Bool, c),
                RegInstr::JumpIfIntCompare { lhs, rhs, .. } => native_unify(ty, *lhs, *rhs, c),
                // Heap reads: the base is a handle, the list index an Int. The
                // read *result* (`dst`) is left to flow from its uses — an `Int`
                // result picks the Int helper, a `Float` result the Float helper.
                // We do not force `dst` to `Int` here (that would reject float
                // reads); lowering admits only a provably-Int-or-Float `dst` and
                // bails otherwise.
                RegInstr::GetFieldSlot { dst: _, base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                }
                RegInstr::SetFieldSlot { dst, base, .. } => {
                    // The written `value`'s type is left to flow from its definition
                    // (Int or Float) — lowering then picks `FieldSetInt`/`FieldSetFloat`
                    // accordingly, and a runtime field-type mismatch bails. `dst` is the
                    // Unit result sentinel (the native write returns the new handle into
                    // `base`); an Int placeholder is never read.
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListLen { dst, list } => {
                    native_set_list_read_base_ty(ty, *list, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListGet {
                    dst: _,
                    list,
                    index,
                } => {
                    native_set_list_read_base_ty(ty, *list, c)
                        && native_set_ty(ty, *index, NativeTy::Int, c)
                }
                RegInstr::ListSet {
                    dst,
                    list,
                    index,
                    value,
                } => {
                    // The element type flows from the value's definition or declared
                    // signature. Preserve a proven Float; otherwise retain the
                    // untyped register VM's historical Int default.
                    let value_ty = if ty[*value] == Some(NativeTy::Float) {
                        NativeTy::Float
                    } else {
                        NativeTy::Int
                    };
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *index, NativeTy::Int, c)
                        && native_set_ty(ty, *value, value_ty, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListPush {
                    dst,
                    list,
                    value: _,
                } => {
                    // The pushed `value`'s type flows from its definition (Int or
                    // Float); lowering picks `ListPushInt`/`ListPushFloat` and a
                    // wrong-element-type list bails at the helper. `dst` is the Int
                    // result (0 on success).
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListSort { dst, list } => {
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::MapInsert {
                    dst,
                    map,
                    key,
                    value: _,
                } => {
                    // The value type flows from its definition (Int or Float);
                    // lowering picks MapInsertInt/MapInsertFloat, and a wrong-value-type
                    // map bails at the helper.
                    native_set_ty(ty, *map, NativeTy::Handle, c)
                        && native_set_ty(ty, *key, NativeTy::Int, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::SetInsert { dst, set, value } => {
                    native_set_ty(ty, *set, NativeTy::Handle, c)
                        && native_set_ty(ty, *value, NativeTy::Int, c)
                        && native_set_ty(ty, *dst, NativeTy::Bool, c)
                }
                RegInstr::SortedSetInsert { dst, set, value } => {
                    native_set_ty(ty, *set, NativeTy::Handle, c)
                        && native_set_ty(ty, *value, NativeTy::Int, c)
                        && native_set_ty(ty, *dst, NativeTy::Bool, c)
                }
                RegInstr::SortedMapInsert {
                    dst,
                    map,
                    key,
                    value,
                } => {
                    native_set_ty(ty, *map, NativeTy::Handle, c)
                        && native_set_ty(ty, *key, NativeTy::Int, c)
                        && native_set_ty(ty, *value, NativeTy::Int, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::DequePushBack {
                    dst,
                    deque,
                    value: _,
                }
                | RegInstr::DequePushFront {
                    dst,
                    deque,
                    value: _,
                } => {
                    // The value type flows (Int or Float); lowering picks the helper.
                    native_set_ty(ty, *deque, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::DequePopFront { dst: _, deque }
                | RegInstr::DequePopBack { dst: _, deque } => {
                    // dst (popped value) flows (Int or Float); lowering picks the helper.
                    native_set_ty(ty, *deque, NativeTy::Handle, c)
                }
                RegInstr::MatchMapGet { map, key, .. } => {
                    // value_dst flows from its uses (Int or Float); lowering picks
                    // MatchMapGetInt/MatchMapGetFloat, and a wrong-value-type map
                    // bails at the helper.
                    native_set_ty(ty, *map, NativeTy::Handle, c)
                        && native_set_ty(ty, *key, NativeTy::Int, c)
                }
                RegInstr::MatchSortedMapGet { map, key, .. } => {
                    // value_dst flows (Int or Float); lowering picks
                    // MatchSortedMapGetInt/MatchSortedMapGetFloat.
                    native_set_ty(ty, *map, NativeTy::Handle, c)
                        && native_set_ty(ty, *key, NativeTy::Int, c)
                }
                RegInstr::StringConcat { dst, left, right } => {
                    native_set_ty(ty, *left, NativeTy::Handle, c)
                        && native_set_ty(ty, *right, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Handle, c)
                }
                // The guarded closure operand is a native handle (a closure passed
                // in as a parameter handle); the guard reads its function id.
                RegInstr::NativeGuardClosureId { closure, .. } => {
                    native_set_ty(ty, *closure, NativeTy::Handle, c)
                }
                // J2.2 dispatch: the closure operand is a native handle; the read
                // function id is an `Int` (consumed by integer compares/branches).
                RegInstr::NativeClosureId { dst, closure } => {
                    native_set_ty(ty, *closure, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::NativeFieldClosureId { dst, base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                // Capturing-closure inline (OSR × J2): the closure operand is a
                // native handle; the materialized capture `dst` carries the
                // capture's scalar bits (the `closure_capture` helper returns the
                // raw i64 bit pattern: an `Int` directly, a `Bool` as 0/1, a
                // `Float` reinterpreted via `f64::to_bits`). Leave `dst`'s class to
                // flow from its uses — exactly like a `GetFieldSlot` read — so an
                // Int/Bool capture stays Int-class and a Float capture becomes
                // Float-class. Lowering admits only a provably Int/Bool/Float `dst`
                // (and the Float arm bit-reinterprets the i64 slot to f64).
                RegInstr::NativeClosureCapture {
                    dst: _, closure, ..
                } => native_set_ty(ty, *closure, NativeTy::Handle, c),
                RegInstr::NativeFieldClosureCapture { base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                }
                // `Int.to_float`: single Int arg → Float dst (signed-int→f64
                // conversion via `fcvt_from_sint` at lowering).
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::IntToFloat,
                    args,
                    dst,
                } => {
                    native_set_ty(ty, args[0], NativeTy::Int, c)
                        && native_set_ty(ty, *dst, NativeTy::Float, c)
                }
                // `Math.floor`/`Math.ceil`: single Float arg → Int dst (round, then
                // saturating f64→i64 cast via `FloatToInt` at lowering).
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::MathFloor | RegIntrinsic::MathCeil,
                    args,
                    dst,
                } => {
                    native_set_ty(ty, args[0], NativeTy::Float, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::CallIntrinsic {
                    intrinsic,
                    args,
                    dst,
                } if native_host_intrinsic(*intrinsic).is_some() => {
                    let spec = native_host_intrinsic(*intrinsic)?;
                    let arg_tys = spec.arg_tys();
                    let mut ok = args.len() == arg_tys.len();
                    if ok {
                        for (arg, expected) in args.iter().zip(arg_tys) {
                            ok = ok && native_set_ty(ty, *arg, expected, c);
                        }
                    }
                    ok && native_set_ty(ty, *dst, spec.result_ty, c)
                }
                RegInstr::CallTypedIntrinsic {
                    intrinsic,
                    type_arg,
                    args,
                    dst,
                } if native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str())).is_some() => {
                    let spec = native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))?;
                    let arg_tys = spec.arg_tys();
                    let mut ok = args.len() == arg_tys.len();
                    if ok {
                        for (arg, expected) in args.iter().zip(arg_tys) {
                            ok = ok && native_set_ty(ty, *arg, expected, c);
                        }
                    }
                    ok && native_set_ty(ty, *dst, spec.result_ty, c)
                }
                _ => true,
            };
            if !ok {
                return None; // conflicting register types
            }
        }
    }

    // TV2 flat-array classification. A `Handle` *parameter* whose uses are list
    // reads (`ListGet`/`ListLen`) can be reclassified as read-only `FlatInt`/
    // `FlatFloat`. If the same Int list param is also written only through
    // `ListSet<Int>`, classify it as `FlatIntMut` so native can bounds-check and
    // write the buffer directly under the heap transaction snapshot.
    let flat_param_kind: Vec<Option<NativeTy>> = {
        #[derive(Clone, Copy, PartialEq)]
        enum S {
            Unseen,
            Flat(NativeTy),
            Disq,
        }
        let mut st = vec![S::Unseen; n_regs];
        let is_handle_param = |reg: usize| ty[reg] == Some(NativeTy::Handle) && reg < func.params;
        for (i, instr) in code.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            match instr {
                RegInstr::GetFieldSlot { base, .. } if is_handle_param(*base) => {
                    st[*base] = S::Disq;
                }
                RegInstr::ListGet { dst, list, .. } if is_handle_param(*list) => {
                    let kind = if ty[*dst] == Some(NativeTy::Float) {
                        NativeTy::FlatFloat
                    } else {
                        NativeTy::FlatInt
                    };
                    st[*list] = match st[*list] {
                        S::Unseen => S::Flat(kind),
                        S::Flat(k) if k == kind => S::Flat(kind),
                        S::Flat(NativeTy::FlatIntMut) if kind == NativeTy::FlatInt => {
                            S::Flat(NativeTy::FlatIntMut)
                        }
                        S::Flat(NativeTy::FlatFloatMut) if kind == NativeTy::FlatFloat => {
                            S::Flat(NativeTy::FlatFloatMut)
                        }
                        _ => S::Disq,
                    };
                }
                RegInstr::ListSet { list, .. } if is_handle_param(*list) => {
                    st[*list] = match st[*list] {
                        S::Unseen | S::Flat(NativeTy::FlatInt) | S::Flat(NativeTy::FlatIntMut) => {
                            S::Flat(NativeTy::FlatIntMut)
                        }
                        S::Flat(NativeTy::FlatFloat) | S::Flat(NativeTy::FlatFloatMut) => {
                            S::Flat(NativeTy::FlatFloatMut)
                        }
                        _ => S::Disq,
                    };
                }
                RegInstr::ListPush { list, .. } if is_handle_param(*list) => {
                    st[*list] = S::Disq;
                }
                RegInstr::ListSort { list, .. } if is_handle_param(*list) => {
                    st[*list] = S::Disq;
                }
                // `List.is_empty<T>` reads only the length (the `lens` slot), never
                // elements, so it can ride the flat path. The typed call names the
                // element type, letting us pin the flat kind even when the param is
                // used ONLY via is_empty (a length read is element-type-agnostic, so
                // the kind only sets how the buffer pointer is marshalled — never
                // dereferenced here). Non-Int/Float element types stay neutral.
                RegInstr::CallTypedIntrinsic {
                    intrinsic: RegIntrinsic::ListIsEmpty,
                    args,
                    type_arg,
                    ..
                } if !args.is_empty()
                    && is_handle_param(args[0])
                    && matches!(type_arg.as_str(), "Int" | "Float") =>
                {
                    let kind = if type_arg.as_str() == "Float" {
                        NativeTy::FlatFloat
                    } else {
                        NativeTy::FlatInt
                    };
                    st[args[0]] = match st[args[0]] {
                        S::Unseen => S::Flat(kind),
                        S::Flat(k) if k == kind => S::Flat(kind),
                        S::Flat(NativeTy::FlatIntMut) if kind == NativeTy::FlatInt => {
                            S::Flat(NativeTy::FlatIntMut)
                        }
                        S::Flat(NativeTy::FlatFloatMut) if kind == NativeTy::FlatFloat => {
                            S::Flat(NativeTy::FlatFloatMut)
                        }
                        _ => S::Disq,
                    };
                }
                // `ListLen` is kind-neutral — neither pins nor disqualifies.
                _ => {}
            }
        }
        st.into_iter()
            .map(|s| match s {
                S::Flat(k) => Some(k),
                _ => None,
            })
            .collect()
    };
    for reg in 0..func.params {
        if let Some(kind) = flat_param_kind[reg] {
            ty[reg] = Some(kind);
        }
    }

    // SOUNDNESS: the interpreter deep-copies every non-`mut` heap param at the prologue
    // (`DeepCopy`); native lowers `DeepCopy` to a Nop but now performs in-place heap
    // writes. Decline native if a DeepCopy'd heap param's value could be mutated in place,
    // stored, returned, or otherwise leaked (directly or via an alias) — otherwise it would
    // propagate to the caller while the interpreter only touches the copy. A `String`/`Bytes`
    // param is exempt (immutable, safe to share). Types are now final.
    let immutable_leaf_params: Vec<bool> = (0..func.params)
        .map(|p| {
            declared_signature
                .and_then(|s| s.params.get(p))
                .is_some_and(|t| native_declared_type_is_immutable_leaf(t))
        })
        .collect();
    if native_deepcopy_param_unsoundly_mutated(&code, &ty, n_regs, &immutable_leaf_params, |i| {
        reachable[i]
    }) {
        return None;
    }

    // J3: a scalar-replaced Option's payload register must be a SCALAR (Int/Float/
    // Bool, or fully unconstrained — defaults to Int). If inference proved it a
    // `Handle`/flat array, the Some payload was a heap value, so the Option was not
    // truly scalar-replaceable ⇒ bail and leave the function on the interpreter
    // path. (Conservative: any doubt ⇒ don't scalar-replace.)
    for &payload in &scalar_payload_regs {
        if matches!(
            ty[payload],
            Some(
                NativeTy::Handle
                    | NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut
            )
        ) {
            return None;
        }
    }

    // Output-handle provenance. A Handle produced by an output-allocating host
    // helper names the call context's heap-result table and may escape as the function's return or
    // feed another native host helper that consumes heap handles. Handles fetched by
    // `FieldHandle`/`ListGetHandle` still name scratch heap args and may only feed
    // native reads, not escape. Propagate through simple moves.
    let mut output_handle = vec![false; n_regs];
    let mut changed_output = true;
    while changed_output {
        changed_output = false;
        for (i, instr) in code.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            match instr {
                RegInstr::CallIntrinsic { intrinsic, dst, .. }
                    if native_host_intrinsic(*intrinsic)
                        .is_some_and(|spec| spec.produces_output_handle())
                        && !output_handle[*dst] =>
                {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                RegInstr::CallTypedIntrinsic {
                    intrinsic,
                    type_arg,
                    dst,
                    ..
                } if native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))
                    .is_some_and(|spec| spec.produces_output_handle())
                    && !output_handle[*dst] =>
                {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                RegInstr::StringConcat { dst, .. } if !output_handle[*dst] => {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                RegInstr::CallKnown { dst, .. }
                    if compiled_callees
                        .get(&ip_map[i])
                        .is_some_and(|callee| callee.ret_ty == NativeTy::Handle)
                        && !output_handle[*dst] =>
                {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                RegInstr::LoadString { dst, .. } if !output_handle[*dst] => {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                // NOTE: a `SetFieldSlot` base must NOT be seeded as an output handle.
                // A `FieldHandle`/`ListGetHandle`-derived base is call-scoped scratch
                // (a heap-arg-table index), not a heap-result-table value; marking it
                // here let it escape via `Return` and the host materialized garbage.
                // Genuine output structs are already marked by their allocating helper,
                // and param bases are returnable via `handle_param`.
                RegInstr::Move { dst, src } if output_handle[*src] && !output_handle[*dst] => {
                    output_handle[*dst] = true;
                    changed_output = true;
                }
                _ => {}
            }
        }
    }
    let mut escaping_output_handle = vec![false; n_regs];
    let mut changed_escape = true;
    while changed_escape {
        changed_escape = false;
        for (i, instr) in code.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            match instr {
                RegInstr::Return { src }
                    if output_handle[*src] && !escaping_output_handle[*src] =>
                {
                    escaping_output_handle[*src] = true;
                    changed_escape = true;
                }
                RegInstr::CallIntrinsic {
                    intrinsic, args, ..
                } if native_host_intrinsic(*intrinsic)
                    .is_some_and(|spec| spec.consumes_output_handles()) =>
                {
                    let spec = native_host_intrinsic(*intrinsic)?;
                    for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                        if expected == NativeTy::Handle
                            && output_handle[*arg]
                            && !escaping_output_handle[*arg]
                        {
                            escaping_output_handle[*arg] = true;
                            changed_escape = true;
                        }
                    }
                }
                RegInstr::CallTypedIntrinsic {
                    intrinsic,
                    type_arg,
                    args,
                    ..
                } if native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))
                    .is_some_and(|spec| spec.consumes_output_handles()) =>
                {
                    let spec = native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))?;
                    for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                        if expected == NativeTy::Handle
                            && output_handle[*arg]
                            && !escaping_output_handle[*arg]
                        {
                            escaping_output_handle[*arg] = true;
                            changed_escape = true;
                        }
                    }
                }
                RegInstr::StringConcat { left, right, .. } => {
                    for arg in [*left, *right] {
                        if output_handle[arg] && !escaping_output_handle[arg] {
                            escaping_output_handle[arg] = true;
                            changed_escape = true;
                        }
                    }
                }
                RegInstr::CallKnown { args, .. } if compiled_callees.contains_key(&ip_map[i]) => {
                    let callee = compiled_callees.get(&ip_map[i])?;
                    for (arg, expected) in args.iter().zip(callee.param_tys.iter()) {
                        if *expected == NativeTy::Handle
                            && output_handle[*arg]
                            && !escaping_output_handle[*arg]
                        {
                            escaping_output_handle[*arg] = true;
                            changed_escape = true;
                        }
                    }
                }
                RegInstr::ListLen { list, .. }
                | RegInstr::ListGet { list, .. }
                | RegInstr::ListPush { list, .. }
                | RegInstr::ListSort { list, .. } => {
                    if output_handle[*list] && !escaping_output_handle[*list] {
                        escaping_output_handle[*list] = true;
                        changed_escape = true;
                    }
                }
                RegInstr::GetFieldSlot { base, .. } => {
                    if output_handle[*base] && !escaping_output_handle[*base] {
                        escaping_output_handle[*base] = true;
                        changed_escape = true;
                    }
                }
                RegInstr::Move { dst, src }
                    if escaping_output_handle[*dst] && !escaping_output_handle[*src] =>
                {
                    escaping_output_handle[*src] = true;
                    changed_escape = true;
                }
                _ => {}
            }
        }
    }

    let int = |reg: usize| ty[reg] == Some(NativeTy::Int);
    let float = |reg: usize| ty[reg] == Some(NativeTy::Float);
    // A heap-read result register that is either provably `Int` or fully
    // unconstrained (no use pins its type) — both lower via the Int read helper
    // (an unconstrained register defaults to `Int` everywhere else too). This
    // keeps the "ambiguous must not silently pick Float" rule: Float is emitted
    // only when `float(dst)` is provably true.
    let int_or_free = |reg: usize| matches!(ty[reg], None | Some(NativeTy::Int));
    let bool_ty = |reg: usize| ty[reg] == Some(NativeTy::Bool);
    // Numeric = Int or Float; `same` = both operands typed and identical (so a
    // polymorphic op lowers consistently and native equality matches `VmValue`).
    let numeric = |reg: usize| matches!(ty[reg], Some(NativeTy::Int | NativeTy::Float));
    let same = |a: usize, b: usize| ty[a].is_some() && ty[a] == ty[b];
    let int_pair_or_same_numeric =
        |a: usize, b: usize| (numeric(a) && same(a, b)) || (int_or_free(a) && int_or_free(b));
    let int_triple_or_same_numeric = |a: usize, b: usize, c: usize| {
        (numeric(a) && same(a, b) && same(a, c))
            || (int_or_free(a) && int_or_free(b) && int_or_free(c))
    };
    // A parameter handle enters via the caller's heap args (`try_native`) and may be
    // returned unchanged by the pass-through slice.
    let handle_param = |reg: usize| ty[reg] == Some(NativeTy::Handle) && reg < func.params;
    // A *native-readable* handle (Pending #1): any Handle register — a param/live-in
    // (marshalled into the heap-arg window) or a loop-internal handle produced by a
    // `FieldHandle`/`ListGetHandle` read (a stored struct/closure fetched as a fresh
    // table index). Used by the heap reads and closure ops, whose runtime helper +
    // identity guard/bail make a wrong handle sound (re-run from the top).
    let handle_reg = |reg: usize| ty[reg] == Some(NativeTy::Handle);
    // A TV2 flat-array param (pointer + length, read directly in-register).
    let flat_param = |reg: usize| {
        matches!(
            ty[reg],
            Some(
                NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut
            )
        ) && reg < func.params
    };
    let r = |reg: usize| reg as u32;
    let cmp = |op: &RegIntCompare| match op {
        RegIntCompare::Less => JitCompare::Lt,
        RegIntCompare::LessEqual => JitCompare::Le,
        RegIntCompare::Greater => JitCompare::Gt,
        RegIntCompare::GreaterEqual => JitCompare::Ge,
    };

    let split_len_source = native_split_len_sources(&code, &reachable, n_regs);
    let pad_left_len_source = native_pad_left_len_sources(&code, &reachable, n_regs);
    let mut string_literals: Vec<Rc<String>> = Vec::new();
    let mut string_literal_ids: HashMap<Rc<String>, i64> = HashMap::new();
    let intern_string_literal = |literals: &mut Vec<Rc<String>>,
                                 ids: &mut HashMap<Rc<String>, i64>,
                                 value: &Rc<String>|
     -> Option<i64> {
        if let Some(index) = ids.get(value) {
            return Some(*index);
        }
        let index = i64::try_from(literals.len()).ok()?;
        literals.push(Rc::clone(value));
        ids.insert(Rc::clone(value), index);
        Some(index)
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
            RegInstr::LoadString { dst, value } => {
                require(escaping_output_handle[*dst])?;
                let literal_id =
                    intern_string_literal(&mut string_literals, &mut string_literal_ids, value)?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringLiteral,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::ImmI64(literal_id)],
                }
            }
            RegInstr::Move { dst, src } => {
                ty[*src]?; // src must be typed
                if (split_len_source[*dst].is_some()
                    && split_len_source[*dst] == split_len_source[*src])
                    || (pad_left_len_source[*dst].is_some()
                        && pad_left_len_source[*dst] == pad_left_len_source[*src])
                {
                    JitInstr::Nop
                } else {
                    JitInstr::Move {
                        dst: r(*dst),
                        src: r(*src),
                    }
                }
            }
            RegInstr::CallKnown {
                dst,
                args,
                mut_args,
                ..
            } if self_call_sites.contains(&ip_map[i]) => {
                // Self-recursive native call (native-call-ABI slice 3): lower to
                // `CallSelf`, which the codegen resolves to THIS function's own
                // FuncId. Scalar params only (no `mut`); arity/types are re-checked by
                // vm-jit's `validate` against this function's own signature.
                require(mut_args.is_empty() && args.len() == func.params)?;
                require(ty[*dst].is_some())?;
                for arg in args {
                    require(ty[*arg].is_some())?;
                }
                JitInstr::CallSelf {
                    dst: r(*dst),
                    args: args.iter().map(|arg| r(*arg)).collect(),
                }
            }
            RegInstr::CallKnown {
                dst,
                args,
                mut_args,
                ..
            } if group_call_sites.contains_key(&ip_map[i]) => {
                // Mutually-recursive native call (native-call-ABI slice 4): lower to
                // `CallGroup` targeting the member's group index. Scalar params only.
                let group_index = group_call_sites[&ip_map[i]];
                require(mut_args.is_empty())?;
                require(ty[*dst].is_some())?;
                for arg in args {
                    require(ty[*arg].is_some())?;
                }
                JitInstr::CallGroup {
                    group_index,
                    dst: r(*dst),
                    args: args.iter().map(|arg| r(*arg)).collect(),
                }
            }
            RegInstr::CallKnown {
                dst,
                args,
                mut_args,
                ..
            } if compiled_callees.contains_key(&ip_map[i]) => {
                let callee = compiled_callees.get(&ip_map[i])?;
                require(
                    args.len() == callee.param_tys.len()
                        && native_call_mut_args_supported(mut_args, &callee.param_tys),
                )?;
                for (arg, expected) in args.iter().zip(callee.param_tys.iter()) {
                    require(ty[*arg] == Some(*expected))?;
                }
                require(ty[*dst] == Some(callee.ret_ty))?;
                JitInstr::CallNative {
                    callee: callee.id,
                    dst: r(*dst),
                    args: args.iter().map(|arg| r(*arg)).collect(),
                }
            }
            RegInstr::DeepCopy { .. } | RegInstr::DeepCopyElided { .. } => {
                // Always a Nop in a native-eligible function: these are pure, leaf,
                // side-effect-free, and never mutate a container, so an independent
                // copy is never observably distinct from the original — for a scalar
                // register or a heap handle/flat param alike. (The previous `ty[reg]?`
                // also *rejected the whole function* when `reg` was untyped — e.g. an
                // unused parameter pins no type — needlessly disqualifying otherwise
                // eligible functions; an untyped register defaults to a scalar `Int`
                // everywhere else, so a copy of it is likewise a no-op.)
                JitInstr::Nop
            }
            RegInstr::TailCallGuard => JitInstr::TailCallGuard {
                max_depth: u32::try_from(DEFAULT_MAX_DEPTH).ok()?,
            },
            RegInstr::AddInt { dst, lhs, rhs } => {
                require(int_triple_or_same_numeric(*dst, *lhs, *rhs))?;
                JitInstr::Add {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::SubInt { dst, lhs, rhs } => {
                require(int_triple_or_same_numeric(*dst, *lhs, *rhs))?;
                JitInstr::Sub {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::MulInt { dst, lhs, rhs } => {
                require(int_triple_or_same_numeric(*dst, *lhs, *rhs))?;
                JitInstr::Mul {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::DivInt { dst, lhs, rhs } => {
                require(int_triple_or_same_numeric(*dst, *lhs, *rhs))?;
                JitInstr::Div {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::ModInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::Mod {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::BitAndInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::BitAnd {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::BitOrInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::BitOr {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::BitXorInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::BitXor {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::ShiftLeftInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
                JitInstr::Shl {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::ShiftRightInt { dst, lhs, rhs } => {
                require(int_or_free(*lhs) && int_or_free(*rhs) && int_or_free(*dst))?;
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
                require(int_pair_or_same_numeric(*lhs, *rhs))?;
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
                if let Some(&hot_target) = profile_hot_branch_edges.get(&i) {
                    JitInstr::ProfiledJumpIfBool {
                        cond: r(*cond),
                        expected: *expected,
                        target: r(*target),
                        hot_target,
                    }
                } else {
                    JitInstr::JumpIfBool {
                        cond: r(*cond),
                        expected: *expected,
                        target: r(*target),
                    }
                }
            }
            RegInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            } => {
                require(int_pair_or_same_numeric(*lhs, *rhs))?;
                if let Some(&hot_target) = profile_hot_branch_edges.get(&i) {
                    JitInstr::ProfiledJumpIfIntCompare {
                        lhs: r(*lhs),
                        rhs: r(*rhs),
                        op: cmp(op),
                        expected: *expected,
                        target: r(*target),
                        hot_target,
                    }
                } else {
                    JitInstr::JumpIfIntCompare {
                        lhs: r(*lhs),
                        rhs: r(*rhs),
                        op: cmp(op),
                        expected: *expected,
                        target: r(*target),
                    }
                }
            }
            RegInstr::Return { src } => {
                // The native ABI returns 64 bits boxed by the caller as the
                // function's return type. A scalar (`Int`/`Float`) return is the
                // unchanged path. Heap-result return ABI also accepts returning a
                // heap parameter unchanged, or a handle freshly allocated into
                // the call context's heap-result table by an output-allocating host helper. Handles
                // produced by `FieldHandle`/`ListGetHandle` remain scratch-only and
                // cannot escape.
                require(
                    int_or_free(*src)
                        || float(*src)
                        || bool_ty(*src)
                        || handle_param(*src)
                        || escaping_output_handle[*src],
                )?;
                JitInstr::Return { src: r(*src) }
            }
            RegInstr::RuntimeError { .. } => JitInstr::Bail,
            RegInstr::StringConcat { dst, left, right } => {
                require(handle_reg(*left) && handle_reg(*right) && escaping_output_handle[*dst])?;
                JitInstr::HostCall {
                    helper: native_string_concat_host().helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*left)),
                        vm_jit::HostArg::Reg(r(*right)),
                    ],
                }
            }
            RegInstr::GetFieldSlot { dst, base, slot } => {
                require(handle_reg(*base))?;
                if handle_reg(*dst) {
                    // A heap-valued field (a stored closure) fetched as a fresh
                    // handle so a downstream closure read can address it.
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::FieldHandle,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        ],
                    }
                } else if float(*dst) {
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::FieldFloat,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        ],
                    }
                } else {
                    // Int or unconstrained → Int helper (a non-Int field then
                    // bails at the helper). A Bool dst is rejected here.
                    require(int_or_free(*dst))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::FieldInt,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        ],
                    }
                }
            }
            RegInstr::SetFieldSlot {
                dst: _,
                base,
                slot,
                value,
            } => {
                // Pick the store helper by the value's type, mirroring the read side:
                // Float → `FieldSetFloat`; Int/unconstrained → `FieldSetInt`; (J0.4 #1) a
                // heap value → `FieldSetHandle` (sets the field to a resolved heap value).
                // A non-matching field then bails at the helper.
                require(handle_reg(*base))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::FieldSetFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::FieldSetInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::FieldSetHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*base),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*base)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::ListLen { dst, list } => {
                require(int(*dst))?;
                if let Some((value, delimiter)) = split_len_source[*list] {
                    require(handle_reg(value) && handle_reg(delimiter))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::StringSplitCount,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(value)),
                            vm_jit::HostArg::Reg(r(delimiter)),
                        ],
                    }
                } else if flat_param(*list) {
                    JitInstr::ListLenDirect {
                        dst: r(*dst),
                        base: r(*list),
                    }
                } else {
                    require(handle_reg(*list))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListLen,
                        dst: r(*dst),
                        args: vec![vm_jit::HostArg::Reg(r(*list))],
                    }
                }
            }
            RegInstr::ListGet { dst, list, index } => {
                require(int(*index))?;
                // TV2 flat params lower to direct in-register reads; the flat kind
                // (set by classification) matches the dst kind by construction.
                if matches!(
                    ty[*list],
                    Some(NativeTy::FlatFloat | NativeTy::FlatFloatMut)
                ) {
                    require(float(*dst))?;
                    JitInstr::ListGetFloatDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                    }
                } else if matches!(ty[*list], Some(NativeTy::FlatInt | NativeTy::FlatIntMut)) {
                    require(int_or_free(*dst))?;
                    JitInstr::ListGetIntDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                    }
                } else if handle_reg(*dst) {
                    // A heap-valued element (a struct holding a stored closure)
                    // fetched as a fresh handle for a downstream field/closure read.
                    require(handle_reg(*list))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListGetHandle,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                        ],
                    }
                } else if float(*dst) {
                    require(handle_reg(*list))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListGetFloat,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                        ],
                    }
                } else {
                    require(handle_reg(*list) && int_or_free(*dst))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListGetInt,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                        ],
                    }
                }
            }
            RegInstr::ListSet {
                dst,
                list,
                index,
                value,
            } => {
                require(int(*index) && int(*dst) && (int_or_free(*value) || float(*value)))?;
                if ty[*list] == Some(NativeTy::FlatIntMut) {
                    require(int_or_free(*value))?;
                    JitInstr::ListSetIntDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                        value: r(*value),
                    }
                } else if ty[*list] == Some(NativeTy::FlatFloatMut) {
                    require(float(*value))?;
                    JitInstr::ListSetFloatDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                        value: r(*value),
                    }
                } else {
                    require(handle_reg(*list))?;
                    let helper = if float(*value) {
                        vm_jit::HostHelper::ListSetFloat
                    } else {
                        vm_jit::HostHelper::ListSetInt
                    };
                    JitInstr::HostCall {
                        helper,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                            vm_jit::HostArg::Reg(r(*value)),
                        ],
                    }
                }
            }
            RegInstr::ListPush { dst, list, value } => {
                // Float → ListPushFloat; Int/unconstrained → ListPushInt; (J0.4 #1) heap
                // value (e.g. `String`/nested collection) → ListPushHandle, resolving the
                // value handle and appending it. A wrong-element-type list bails.
                require(handle_reg(*list) && int(*dst))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::ListPushFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::ListPushInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::ListPushHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*list)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::ListSort { dst, list } => {
                require(handle_reg(*list) && int(*dst))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ListSortInt,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*list))],
                }
            }
            RegInstr::MapInsert {
                dst,
                map,
                key,
                value,
            } => {
                require(handle_reg(*map) && int(*dst))?;
                // Three shapes: Int-key/Float-value, Int-key/Int-value, and (J0.4 #1)
                // heap-key (e.g. `String`)/Int-value. A heap key is resolved + hashed by
                // the host's own `VmMapKey` in the helper, never re-hashed in native.
                let helper = if int(*key) && float(*value) {
                    vm_jit::HostHelper::MapInsertFloat
                } else if int(*key) && int_or_free(*value) {
                    vm_jit::HostHelper::MapInsertInt
                } else if handle_reg(*key) && int(*value) {
                    vm_jit::HostHelper::MapInsertHandleKeyInt
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*map)),
                        vm_jit::HostArg::Reg(r(*key)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::SetInsert { dst, set, value } => {
                require(handle_reg(*set) && bool_ty(*dst))?;
                // Int value → SetInsertInt; (J0.4 #1) heap value (e.g. `String`) →
                // SetInsertHandle, which resolves + hashes the value via the host's key.
                let helper = if int(*value) {
                    vm_jit::HostHelper::SetInsertInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::SetInsertHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*set)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::SortedSetInsert { dst, set, value } => {
                require(handle_reg(*set) && bool_ty(*dst))?;
                // Int value → SortedSetInsertInt; (J0.4 #1) heap value → SortedSetInsertHandle.
                let helper = if int(*value) {
                    vm_jit::HostHelper::SortedSetInsertInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::SortedSetInsertHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*set)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::SortedMapInsert {
                dst,
                map,
                key,
                value,
            } => {
                require(handle_reg(*map) && int(*value) && int(*dst))?;
                // Int key → SortedMapInsertInt; (J0.4 #1) heap key → SortedMapInsertHandleKeyInt.
                let helper = if int(*key) {
                    vm_jit::HostHelper::SortedMapInsertInt
                } else if handle_reg(*key) {
                    vm_jit::HostHelper::SortedMapInsertHandleKeyInt
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*map)),
                        vm_jit::HostArg::Reg(r(*key)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::DequePushBack { dst, deque, value } => {
                require(handle_reg(*deque) && int(*dst))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::DequePushBackFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::DequePushBackInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::DequePushBackHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*deque)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::DequePushFront { dst, deque, value } => {
                require(handle_reg(*deque) && int(*dst))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::DequePushFrontFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::DequePushFrontInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::DequePushFrontHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*deque)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::DequePopFront { dst, deque } => {
                require(handle_reg(*deque) && (int_or_free(*dst) || float(*dst)))?;
                let helper = if float(*dst) {
                    vm_jit::HostHelper::DequePopFrontFloat
                } else {
                    vm_jit::HostHelper::DequePopFrontInt
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*deque))],
                }
            }
            RegInstr::DequePopBack { dst, deque } => {
                require(handle_reg(*deque) && (int_or_free(*dst) || float(*dst)))?;
                let helper = if float(*dst) {
                    vm_jit::HostHelper::DequePopBackFloat
                } else {
                    vm_jit::HostHelper::DequePopBackInt
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*deque))],
                }
            }
            RegInstr::MatchMapGet {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require(
                    handle_reg(*map) && int(*key) && (int_or_free(*value_dst) || float(*value_dst)),
                )?;
                if float(*value_dst) {
                    JitInstr::MatchMapGetFloat {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                } else {
                    JitInstr::MatchMapGetInt {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                }
            }
            RegInstr::MatchSortedMapGet {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require(
                    handle_reg(*map) && int(*key) && (int_or_free(*value_dst) || float(*value_dst)),
                )?;
                if float(*value_dst) {
                    JitInstr::MatchSortedMapGetFloat {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                } else {
                    JitInstr::MatchSortedMapGetInt {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                }
            }
            RegInstr::NativeGuardClosureId { closure, expected } => {
                // The closure handle is a native-readable handle (a param, or a
                // stored closure fetched via `FieldHandle`/`ListGetHandle`); the
                // guard reads its function id and bails on mismatch.
                require(handle_reg(*closure))?;
                let expected = i64::try_from(*expected).ok()?;
                JitInstr::GuardClosureId {
                    base: r(*closure),
                    expected,
                }
            }
            RegInstr::NativeClosureId { dst, closure } => {
                // The closure handle is a native-readable handle; reads its
                // function id once into `dst` for the polymorphic dispatcher.
                require(handle_reg(*closure))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ClosureId,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*closure))],
                }
            }
            RegInstr::NativeFieldClosureId { dst, base, slot } => {
                require(handle_reg(*base) && int(*dst))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::FieldClosureId,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*base)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                    ],
                }
            }
            RegInstr::NativeClosureCapture {
                dst,
                closure,
                index,
            } => {
                // The closure handle is a native-readable handle; materialize
                // capture `index`'s scalar bits into `dst` (the inlined body's
                // capture register). `dst` may be Int/Bool (i64 used directly) or
                // Float (the i64 slot is `f64::to_bits`, bit-reinterpreted to f64 in
                // codegen); an unconstrained `dst` defaults to Int. A non-scalar
                // (Handle/flat) `dst` bails. A non-scalar capture VALUE additionally
                // bails out-of-band in the host helper at runtime.
                require(
                    handle_reg(*closure) && (int_or_free(*dst) || bool_ty(*dst) || float(*dst)),
                )?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ClosureCapture,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*closure)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*index).ok()?),
                    ],
                }
            }
            RegInstr::NativeFieldClosureCapture {
                dst,
                base,
                slot,
                index,
            } => {
                require(handle_reg(*base) && (int_or_free(*dst) || bool_ty(*dst) || float(*dst)))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::FieldClosureCapture,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*base)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        vm_jit::HostArg::ImmI64(i64::try_from(*index).ok()?),
                    ],
                }
            }
            // `Int.to_float`: signed-int→f64 conversion (`fcvt_from_sint`). The
            // src register holds an Int (i64) and dst a Float (f64). The
            // interpreter's `IntToFloat` does `i as f64`; this is the identical
            // value-preserving conversion (not a bitcast).
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::IntToFloat,
                args,
                dst,
            } => {
                require(int(args[0]) && float(*dst))?;
                JitInstr::IntToFloat {
                    dst: r(*dst),
                    src: r(args[0]),
                }
            }
            // `Math.floor`/`Math.ceil`: round the f64 arg then a saturating f64→i64
            // cast, identical to the interpreter's `f.floor()/.ceil() as i64`. src is
            // a Float (f64), dst an Int (i64).
            RegInstr::CallIntrinsic {
                intrinsic: rounding_intrinsic @ (RegIntrinsic::MathFloor | RegIntrinsic::MathCeil),
                args,
                dst,
            } => {
                require(float(args[0]) && int(*dst))?;
                let rounding = match rounding_intrinsic {
                    RegIntrinsic::MathFloor => vm_jit::FloatRounding::Floor,
                    _ => vm_jit::FloatRounding::Ceil,
                };
                JitInstr::FloatToInt {
                    dst: r(*dst),
                    src: r(args[0]),
                    rounding,
                }
            }
            // `List.is_empty` on a flat-array param reads the length directly from the
            // param's `lens` slot and compares to zero — no per-iteration host call.
            // (Non-flat/`Handle` receivers fall through to the host-helper path below.)
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::ListIsEmpty,
                args,
                dst,
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::ListIsEmpty,
                args,
                dst,
                ..
            } if args.len() == 1 && flat_param(args[0]) => {
                require(bool_ty(*dst))?;
                JitInstr::ListIsEmptyDirect {
                    dst: r(*dst),
                    base: r(args[0]),
                }
            }
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringSplit,
                dst,
                ..
            } if split_len_source[*dst].is_some() => JitInstr::Nop,
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringPadLeft,
                dst,
                ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::StringPadLeft,
                dst,
                ..
            } if pad_left_len_source[*dst].is_some() => JitInstr::Nop,
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args,
                dst,
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args,
                dst,
                ..
            } if args.len() == 1 && pad_left_len_source[args[0]].is_some() => {
                require(int(*dst))?;
                let (value, width, fill) = pad_left_len_source[args[0]]?;
                require(handle_reg(value) && int(width) && handle_reg(fill))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringPadLeftLen,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(value)),
                        vm_jit::HostArg::Reg(r(width)),
                        vm_jit::HostArg::Reg(r(fill)),
                    ],
                }
            }
            RegInstr::CallIntrinsic {
                intrinsic,
                args,
                dst,
            } if native_host_intrinsic(*intrinsic).is_some() => {
                let spec = native_host_intrinsic(*intrinsic)?;
                require(args.len() == spec.arg_tys().len())?;
                for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                    require(match expected {
                        NativeTy::Int => int(*arg),
                        NativeTy::Bool => bool_ty(*arg),
                        NativeTy::Float => float(*arg),
                        NativeTy::Handle => handle_reg(*arg),
                        NativeTy::FlatInt
                        | NativeTy::FlatIntMut
                        | NativeTy::FlatFloat
                        | NativeTy::FlatFloatMut => false,
                    })?;
                }
                require(match spec.result_ty {
                    NativeTy::Int => int(*dst),
                    NativeTy::Bool => bool_ty(*dst),
                    NativeTy::Float => float(*dst),
                    NativeTy::Handle => handle_reg(*dst),
                    NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut => false,
                })?;
                if spec.produces_output_handle() {
                    require(escaping_output_handle[*dst])?;
                }
                JitInstr::HostCall {
                    helper: spec.helper,
                    dst: r(*dst),
                    args: args
                        .iter()
                        .map(|arg| vm_jit::HostArg::Reg(r(*arg)))
                        .collect(),
                }
            }
            RegInstr::CallTypedIntrinsic {
                intrinsic,
                type_arg,
                args,
                dst,
            } if native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str())).is_some() => {
                let spec = native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))?;
                require(args.len() == spec.arg_tys().len())?;
                for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                    require(match expected {
                        NativeTy::Int => int(*arg),
                        NativeTy::Bool => bool_ty(*arg),
                        NativeTy::Float => float(*arg),
                        NativeTy::Handle => handle_reg(*arg),
                        NativeTy::FlatInt
                        | NativeTy::FlatIntMut
                        | NativeTy::FlatFloat
                        | NativeTy::FlatFloatMut => false,
                    })?;
                }
                require(match spec.result_ty {
                    NativeTy::Int => int(*dst),
                    NativeTy::Bool => bool_ty(*dst),
                    NativeTy::Float => float(*dst),
                    NativeTy::Handle => handle_reg(*dst),
                    NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut => false,
                })?;
                if spec.produces_output_handle() {
                    require(escaping_output_handle[*dst])?;
                }
                JitInstr::HostCall {
                    helper: spec.helper,
                    dst: r(*dst),
                    args: args
                        .iter()
                        .map(|arg| vm_jit::HostArg::Reg(r(*arg)))
                        .collect(),
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

    let native_reg_types: Vec<NativeTy> = (0..n_regs)
        .map(|reg| ty[reg].unwrap_or(NativeTy::Int))
        .collect();
    let memo_scopes = native_memoize_loop_invariant_host_calls(
        &code,
        &reachable,
        &mut jit_code,
        &native_reg_types,
        func.params,
    );
    native_forward_direct_list_store_loads(&mut jit_code);

    let reg_types = native_reg_types
        .iter()
        .map(|ty| ty.jit_value_type())
        .collect();

    // Parameter types (for the caller's argument unboxing); an unconstrained
    // parameter defaults to `Int` (and a mismatching argument then just falls back).
    let param_types: Vec<NativeTy> = native_reg_types[..func.params].to_vec();

    let jit_fn = vm_jit::JitFunction {
        n_params: func.params as u32,
        n_regs: native_reg_types.len() as u32,
        reg_types,
        zero_init_regs: scalar_payload_regs.iter().map(|&reg| reg as u32).collect(),
        code: jit_code,
        memo_scopes,
        cold_blocks: profile_guidance.cold_blocks,
    };
    // A self-recursive function uses re-run-from-top deopt (its `CallSelf` is
    // non-chaining and its native frame chain has no bounded deopt payload), so it is
    // never precise-resumable regardless of ip-map identity.
    let precise_resume_safe = self_call_sites.is_empty()
        && group_call_sites.is_empty()
        && n_regs == func.regs
        && ip_map.iter().enumerate().all(|(ip, &orig)| ip == orig);
    Some((
        jit_fn,
        ret_type,
        param_types,
        string_literals,
        precise_resume_safe,
    ))
}

#[cfg(feature = "native-jit")]
fn native_compose_ip_maps(
    previous_to_original: &[usize],
    next_to_previous: &[usize],
) -> Option<Vec<usize>> {
    next_to_previous
        .iter()
        .map(|&previous| previous_to_original.get(previous).copied())
        .collect()
}

#[cfg(feature = "native-jit")]
/// Forward flat-list stores to later matching loads within a basic block.
///
/// Direct stores retain the required bounds guard. Loads become `Move`s so source
/// IPs and instruction count stay unchanged.
fn native_forward_direct_list_store_loads(jit_code: &mut [vm_jit::JitInstr]) {
    #[derive(Clone, Copy)]
    struct AvailableStore {
        base: u32,
        index: u32,
        value: u32,
    }

    impl AvailableStore {
        fn clobbered_by(self, reg: u32) -> bool {
            self.base == reg || self.index == reg || self.value == reg
        }
    }

    let mut block_entry = vec![false; jit_code.len()];
    for instr in jit_code.iter() {
        let targets: &[u32] = match instr {
            vm_jit::JitInstr::Jump { target }
            | vm_jit::JitInstr::JumpIfBool { target, .. }
            | vm_jit::JitInstr::ProfiledJumpIfBool { target, .. }
            | vm_jit::JitInstr::JumpIfIntCompare { target, .. }
            | vm_jit::JitInstr::ProfiledJumpIfIntCompare { target, .. } => {
                std::slice::from_ref(target)
            }
            vm_jit::JitInstr::MatchMapGetInt {
                some_ip, none_ip, ..
            }
            | vm_jit::JitInstr::MatchMapGetFloat {
                some_ip, none_ip, ..
            }
            | vm_jit::JitInstr::MatchSortedMapGetInt {
                some_ip, none_ip, ..
            }
            | vm_jit::JitInstr::MatchSortedMapGetFloat {
                some_ip, none_ip, ..
            } => {
                for target in [some_ip, none_ip] {
                    if let Some(entry) = block_entry.get_mut(*target as usize) {
                        *entry = true;
                    }
                }
                continue;
            }
            _ => continue,
        };
        for target in targets {
            if let Some(entry) = block_entry.get_mut(*target as usize) {
                *entry = true;
            }
        }
    }

    let mut int_store: Option<AvailableStore> = None;
    let mut float_store: Option<AvailableStore> = None;
    for ip in 0..jit_code.len() {
        if block_entry[ip] {
            int_store = None;
            float_store = None;
        }

        let replacement = match (&jit_code[ip], int_store, float_store) {
            (vm_jit::JitInstr::ListGetIntDirect { dst, base, index }, Some(store), _)
                if store.base == *base && store.index == *index =>
            {
                Some(vm_jit::JitInstr::Move {
                    dst: *dst,
                    src: store.value,
                })
            }
            (vm_jit::JitInstr::ListGetFloatDirect { dst, base, index }, _, Some(store))
                if store.base == *base && store.index == *index =>
            {
                Some(vm_jit::JitInstr::Move {
                    dst: *dst,
                    src: store.value,
                })
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            jit_code[ip] = replacement;
        }

        match &jit_code[ip] {
            vm_jit::JitInstr::ListSetIntDirect {
                dst,
                base,
                index,
                value,
            } => {
                int_store =
                    (*dst != *base && *dst != *index && *dst != *value).then_some(AvailableStore {
                        base: *base,
                        index: *index,
                        value: *value,
                    });
                if float_store.is_some_and(|store| store.clobbered_by(*dst)) {
                    float_store = None;
                }
            }
            vm_jit::JitInstr::ListSetFloatDirect {
                dst,
                base,
                index,
                value,
            } => {
                float_store =
                    (*dst != *base && *dst != *index && *dst != *value).then_some(AvailableStore {
                        base: *base,
                        index: *index,
                        value: *value,
                    });
                if int_store.is_some_and(|store| store.clobbered_by(*dst)) {
                    int_store = None;
                }
            }
            instr if native_direct_store_forwarding_scalar(instr) => {
                if let Some(dst) = native_jit_written_reg(instr) {
                    if int_store.is_some_and(|store| store.clobbered_by(dst)) {
                        int_store = None;
                    }
                    if float_store.is_some_and(|store| store.clobbered_by(dst)) {
                        float_store = None;
                    }
                }
            }
            _ => {
                int_store = None;
                float_store = None;
            }
        }
    }
}

#[cfg(feature = "native-jit")]
fn native_direct_store_forwarding_scalar(instr: &vm_jit::JitInstr) -> bool {
    matches!(
        instr,
        vm_jit::JitInstr::Nop
            | vm_jit::JitInstr::LoadInt { .. }
            | vm_jit::JitInstr::LoadFloat { .. }
            | vm_jit::JitInstr::LoadBool { .. }
            | vm_jit::JitInstr::Move { .. }
            | vm_jit::JitInstr::Add { .. }
            | vm_jit::JitInstr::Sub { .. }
            | vm_jit::JitInstr::Mul { .. }
            | vm_jit::JitInstr::Div { .. }
            | vm_jit::JitInstr::Mod { .. }
            | vm_jit::JitInstr::IntToFloat { .. }
            | vm_jit::JitInstr::FloatToInt { .. }
            | vm_jit::JitInstr::BitAnd { .. }
            | vm_jit::JitInstr::BitOr { .. }
            | vm_jit::JitInstr::BitXor { .. }
            | vm_jit::JitInstr::Shl { .. }
            | vm_jit::JitInstr::Shr { .. }
            | vm_jit::JitInstr::Compare { .. }
            | vm_jit::JitInstr::Equal { .. }
            | vm_jit::JitInstr::NotEqual { .. }
            | vm_jit::JitInstr::ListGetIntDirect { .. }
            | vm_jit::JitInstr::ListGetFloatDirect { .. }
            | vm_jit::JitInstr::ListLenDirect { .. }
            | vm_jit::JitInstr::ListIsEmptyDirect { .. }
    )
}

#[cfg(feature = "native-jit")]
fn native_split_len_sources(
    code: &[RegInstr],
    reachable: &[bool],
    n_regs: usize,
) -> Vec<Option<(usize, usize)>> {
    native_query_only_sources(
        code,
        reachable,
        n_regs,
        |instr| match instr {
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringSplit,
                args,
                dst,
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::StringSplit,
                args,
                dst,
                ..
            } if args.len() == 2 => Some((*dst, (args[0], args[1]))),
            _ => None,
        },
        |instr| match instr {
            RegInstr::ListLen { list, .. } => Some(*list),
            _ => None,
        },
    )
}

#[cfg(feature = "native-jit")]
fn native_pad_left_len_sources(
    code: &[RegInstr],
    reachable: &[bool],
    n_regs: usize,
) -> Vec<Option<(usize, usize, usize)>> {
    native_query_only_sources(
        code,
        reachable,
        n_regs,
        |instr| match instr {
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringPadLeft,
                args,
                dst,
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::StringPadLeft,
                args,
                dst,
                ..
            } if args.len() == 3 => Some((*dst, (args[0], args[1], args[2]))),
            _ => None,
        },
        |instr| match instr {
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args,
                ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::StringLen,
                args,
                ..
            } if args.len() == 1 => Some(args[0]),
            _ => None,
        },
    )
}

#[cfg(feature = "native-jit")]
fn native_query_only_sources<T>(
    code: &[RegInstr],
    reachable: &[bool],
    n_regs: usize,
    producer: impl Fn(&RegInstr) -> Option<(usize, T)>,
    query_read: impl Fn(&RegInstr) -> Option<usize>,
) -> Vec<Option<T>>
where
    T: Copy + PartialEq,
{
    let mut source = vec![None; n_regs];
    let mut producer_ip = vec![None; n_regs];
    let mut ok = vec![false; n_regs];
    for (ip, instr) in code.iter().enumerate() {
        if !reachable[ip] {
            continue;
        }
        if let Some((dst, value)) = producer(instr)
            && dst < n_regs
        {
            source[dst] = Some(value);
            producer_ip[dst] = Some(ip);
            ok[dst] = true;
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for (ip, instr) in code.iter().enumerate() {
            if !reachable[ip] {
                continue;
            }
            if let RegInstr::Move { dst, src } = instr
                && let Some(src_source) = source[*src]
                && source[*dst].is_none()
            {
                source[*dst] = Some(src_source);
                producer_ip[*dst] = Some(ip);
                ok[*dst] = true;
                changed = true;
            }
        }
    }
    for (ip, instr) in code.iter().enumerate() {
        if !reachable[ip] {
            continue;
        }
        if let RegFootprint::Some(writes) = instr_written_reg(instr) {
            for written in writes {
                if ok[written] && producer_ip[written] != Some(ip) {
                    ok[written] = false;
                }
            }
        } else {
            for flag in &mut ok {
                *flag = false;
            }
            break;
        }
        let allowed_read = query_read(instr).or_else(|| match instr {
            RegInstr::DeepCopy { reg } | RegInstr::DeepCopyElided { reg } => Some(*reg),
            RegInstr::Move { dst, src }
                if source[*dst].is_some() && source[*dst] == source[*src] =>
            {
                Some(*src)
            }
            _ => None,
        });
        match instr_read_regs(instr) {
            RegFootprint::Some(reads) => {
                for read in reads {
                    if ok[read] && allowed_read != Some(read) {
                        ok[read] = false;
                    }
                }
            }
            RegFootprint::All => {
                for flag in &mut ok {
                    *flag = false;
                }
                break;
            }
        }
    }
    for reg in 0..n_regs {
        if !ok[reg] {
            source[reg] = None;
        }
    }
    source
}

#[cfg(feature = "native-jit")]
fn native_memoize_loop_invariant_host_calls(
    code: &[RegInstr],
    reachable: &[bool],
    jit_code: &mut [vm_jit::JitInstr],
    native_reg_types: &[NativeTy],
    n_params: usize,
) -> Vec<vm_jit::MemoScope> {
    let original_n_regs = native_reg_types.len();
    let mut next_memo_slot = 0_u32;
    let mut memo_scopes = Vec::new();
    let heap_provenance =
        NativeHeapProvenanceFacts::compute(code, jit_code, n_params, native_reg_types);
    let loops = detect_natural_loops(code);
    for lp in &loops {
        // Scope lowering marks only unconditional jumps as backedges. This covers
        // structured `while` loops without splitting conditional CFG edges.
        if !native_memo_scope_representable(code, lp.header, lp.exit) {
            continue;
        }
        let first_memo_slot = next_memo_slot;
        let Some(mut invariants) =
            native_loop_invariant_regs(code, reachable, lp.header, lp.exit, original_n_regs)
        else {
            continue;
        };
        for ip in lp.header..lp.exit {
            if !reachable.get(ip).copied().unwrap_or(false) {
                continue;
            }
            native_propagate_derived_loop_invariant(&code[ip], &mut invariants, ip);
            let Some((helper, dst, args)) = native_memoizable_host_call(&jit_code[ip]) else {
                continue;
            };
            let dst = *dst;
            let args = args.clone();
            let field_load_eligible = native_memoizable_field_load_helper(helper)
                && native_field_load_args_loop_stable(
                    &args,
                    &invariants,
                    &jit_code,
                    heap_provenance.as_ref(),
                    lp.header,
                    lp.exit,
                    ip,
                    original_n_regs,
                );
            let collection_metadata_eligible = native_collection_metadata_helper(helper)
                && native_loop_preserves_heap_query(
                    &args,
                    NativeHeapDomain::Projection(vm_jit::HostHeapProjection::CollectionLen),
                    &jit_code,
                    heap_provenance.as_ref(),
                    lp.header,
                    lp.exit,
                    ip,
                );
            let args_loop_stable = if field_load_eligible {
                true
            } else {
                native_host_args_loop_invariant(&args, &invariants, ip)
            };
            if !(native_memoizable_helper(helper)
                || field_load_eligible
                || collection_metadata_eligible)
                || !args_loop_stable
            {
                continue;
            }
            let Some(&result_ty) = native_reg_types.get(dst as usize) else {
                continue;
            };
            if !native_memoizable_result_type(helper, result_ty)
                && !(field_load_eligible
                    && matches!(result_ty, NativeTy::Int | NativeTy::Bool | NativeTy::Float))
            {
                continue;
            }
            jit_code[ip] = vm_jit::JitInstr::MemoizedHostCall {
                helper,
                dst,
                args,
                memo_slot: next_memo_slot,
            };
            next_memo_slot += 1;
            if invariants
                .write_count
                .get(dst as usize)
                .is_some_and(|count| *count == 1)
            {
                if let Some(derived) = invariants.derived_invariant.get_mut(dst as usize) {
                    *derived = true;
                }
            }
        }
        if next_memo_slot > first_memo_slot {
            memo_scopes.push(vm_jit::MemoScope {
                header: lp.header as u32,
                exit: lp.exit as u32,
                memo_slots: (first_memo_slot..next_memo_slot).collect(),
            });
        }
    }
    memo_scopes
}

#[cfg(feature = "native-jit")]
fn native_memoizable_host_call(
    instr: &vm_jit::JitInstr,
) -> Option<(vm_jit::HostHelper, &u32, &Vec<vm_jit::HostArg>)> {
    match instr {
        vm_jit::JitInstr::HostCall { helper, dst, args } => Some((*helper, dst, args)),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn native_memoizable_helper(helper: vm_jit::HostHelper) -> bool {
    if helper.heap_effect().writes_existing_heap() {
        return false;
    }
    native_memoizable_scalar_result_helper(helper)
}

#[cfg(feature = "native-jit")]
fn native_memoizable_result_type(_helper: vm_jit::HostHelper, result_ty: NativeTy) -> bool {
    matches!(result_ty, NativeTy::Int | NativeTy::Bool | NativeTy::Float)
}

#[cfg(feature = "native-jit")]
fn native_memoizable_scalar_result_helper(helper: vm_jit::HostHelper) -> bool {
    matches!(
        helper,
        vm_jit::HostHelper::StringLen
            | vm_jit::HostHelper::StringPadLeftLen
            | vm_jit::HostHelper::StringSplitCount
            | vm_jit::HostHelper::StringStartsWith
            | vm_jit::HostHelper::BytesLen
            | vm_jit::HostHelper::JsonFieldInt
    )
}

#[cfg(feature = "native-jit")]
fn native_memoizable_field_load_helper(helper: vm_jit::HostHelper) -> bool {
    matches!(
        helper,
        vm_jit::HostHelper::FieldInt | vm_jit::HostHelper::FieldFloat
    )
}

#[cfg(feature = "native-jit")]
fn native_collection_metadata_helper(helper: vm_jit::HostHelper) -> bool {
    helper.heap_reads().iter().any(|access| {
        access.arg == 0 && access.projection == vm_jit::HostHeapProjection::CollectionLen
    })
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeHeapDomain {
    Projection(vm_jit::HostHeapProjection),
    FieldSlot(i64),
}

#[cfg(feature = "native-jit")]
fn native_heap_domains_may_overlap(lhs: NativeHeapDomain, rhs: NativeHeapDomain) -> bool {
    use vm_jit::HostHeapProjection;

    match (lhs, rhs) {
        (NativeHeapDomain::Projection(HostHeapProjection::Unknown), _)
        | (_, NativeHeapDomain::Projection(HostHeapProjection::Unknown)) => true,
        (NativeHeapDomain::FieldSlot(lhs), NativeHeapDomain::FieldSlot(rhs)) => lhs == rhs,
        (
            NativeHeapDomain::FieldSlot(_),
            NativeHeapDomain::Projection(HostHeapProjection::Fields),
        )
        | (
            NativeHeapDomain::Projection(HostHeapProjection::Fields),
            NativeHeapDomain::FieldSlot(_),
        ) => true,
        (NativeHeapDomain::Projection(lhs), NativeHeapDomain::Projection(rhs)) => lhs == rhs,
        _ => false,
    }
}

#[cfg(feature = "native-jit")]
fn native_heap_roots_may_alias(lhs: NativeHeapProvenance, rhs: NativeHeapProvenance) -> bool {
    match (lhs, rhs) {
        (NativeHeapProvenance::Fresh(lhs), NativeHeapProvenance::Fresh(rhs)) => lhs == rhs,
        (NativeHeapProvenance::Fresh(_), NativeHeapProvenance::External)
        | (NativeHeapProvenance::External, NativeHeapProvenance::Fresh(_)) => false,
        _ => true,
    }
}

#[cfg(feature = "native-jit")]
fn native_heap_receiver_arg(args: &[vm_jit::HostArg], index: usize) -> Option<u32> {
    match args.get(index) {
        Some(vm_jit::HostArg::Reg(reg)) => Some(*reg),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
fn native_host_write_domain(
    helper: vm_jit::HostHelper,
    args: &[vm_jit::HostArg],
    projection: vm_jit::HostHeapProjection,
) -> NativeHeapDomain {
    if projection == vm_jit::HostHeapProjection::Fields
        && is_native_field_set_helper(helper)
        && let Some(vm_jit::HostArg::ImmI64(slot)) = args.get(1)
    {
        NativeHeapDomain::FieldSlot(*slot)
    } else {
        NativeHeapDomain::Projection(projection)
    }
}

#[cfg(feature = "native-jit")]
fn native_loop_preserves_heap_query(
    query_args: &[vm_jit::HostArg],
    query_domain: NativeHeapDomain,
    jit_code: &[vm_jit::JitInstr],
    provenance: Option<&NativeHeapProvenanceFacts>,
    header: usize,
    exit: usize,
    query_ip: usize,
) -> bool {
    let Some(query_reg) = native_heap_receiver_arg(query_args, 0) else {
        return false;
    };
    let query_root = provenance
        .map(|facts| facts.before(query_reg, query_ip))
        .unwrap_or(NativeHeapProvenance::Unknown);

    for (ip, instr) in jit_code[header..exit].iter().enumerate() {
        let ip = header + ip;
        match instr {
            vm_jit::JitInstr::HostCall { helper, args, .. }
            | vm_jit::JitInstr::MemoizedHostCall { helper, args, .. } => {
                for access in helper.heap_writes() {
                    let write_domain = native_host_write_domain(*helper, args, access.projection);
                    if !native_heap_domains_may_overlap(query_domain, write_domain) {
                        continue;
                    }
                    let Some(write_reg) = native_heap_receiver_arg(args, access.arg as usize)
                    else {
                        return false;
                    };
                    let write_root = provenance
                        .map(|facts| facts.before(write_reg, ip))
                        .unwrap_or(NativeHeapProvenance::Unknown);
                    if native_heap_roots_may_alias(query_root, write_root) {
                        return false;
                    }
                }
            }
            vm_jit::JitInstr::CallNative { .. }
            | vm_jit::JitInstr::CallSelf { .. }
            | vm_jit::JitInstr::CallGroup { .. } => return false,
            vm_jit::JitInstr::ListSetIntDirect { base, .. }
            | vm_jit::JitInstr::ListSetFloatDirect { base, .. } => {
                let write_domain =
                    NativeHeapDomain::Projection(vm_jit::HostHeapProjection::Elements);
                if native_heap_domains_may_overlap(query_domain, write_domain) {
                    let write_root = provenance
                        .map(|facts| facts.before(*base, ip))
                        .unwrap_or(NativeHeapProvenance::Unknown);
                    if native_heap_roots_may_alias(query_root, write_root) {
                        return false;
                    }
                }
            }
            _ => {}
        }
    }
    true
}

#[cfg(all(feature = "native-jit", test))]
pub(crate) fn native_loop_preserves_heap_projection(
    jit_code: &[vm_jit::JitInstr],
    header: usize,
    exit: usize,
    projection: vm_jit::HostHeapProjection,
) -> bool {
    for instr in &jit_code[header..exit] {
        match instr {
            vm_jit::JitInstr::HostCall { helper, .. }
            | vm_jit::JitInstr::MemoizedHostCall { helper, .. } => {
                if helper.heap_writes().iter().any(|access| {
                    access.projection == projection
                        || access.projection == vm_jit::HostHeapProjection::Unknown
                }) {
                    return false;
                }
            }
            vm_jit::JitInstr::CallNative { .. }
            | vm_jit::JitInstr::CallSelf { .. }
            | vm_jit::JitInstr::CallGroup { .. } => return false,
            // Direct flat-list stores preserve the view's length.
            vm_jit::JitInstr::ListSetIntDirect { .. }
            | vm_jit::JitInstr::ListSetFloatDirect { .. } => {
                if projection == vm_jit::HostHeapProjection::Elements {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

#[cfg(all(feature = "native-jit", test))]
pub(crate) fn native_field_load_slot_not_stored_in_loop(
    args: &[vm_jit::HostArg],
    jit_code: &[vm_jit::JitInstr],
    header: usize,
    exit: usize,
) -> bool {
    let Some(read_slot) = args.get(1).copied() else {
        return false;
    };
    let vm_jit::HostArg::ImmI64(read_slot) = read_slot else {
        return false;
    };
    for instr in &jit_code[header..exit] {
        if matches!(instr, vm_jit::JitInstr::CallNative { .. }) {
            return false;
        }
        let vm_jit::JitInstr::HostCall {
            helper,
            args: store_args,
            ..
        } = instr
        else {
            continue;
        };
        if !is_native_field_set_helper(*helper) {
            continue;
        }
        match store_args.get(1).copied() {
            Some(vm_jit::HostArg::ImmI64(store_slot)) if store_slot != read_slot => {}
            _ => return false,
        }
    }
    true
}

#[cfg(all(feature = "native-jit", test))]
pub(crate) fn native_loop_preserves_field_slot_for_receiver(
    code: &[RegInstr],
    jit_code: &[vm_jit::JitInstr],
    native_reg_types: &[NativeTy],
    n_params: usize,
    query_args: &[vm_jit::HostArg],
    header: usize,
    exit: usize,
    query_ip: usize,
) -> bool {
    let Some(vm_jit::HostArg::ImmI64(slot)) = query_args.get(1).copied() else {
        return false;
    };
    let provenance = NativeHeapProvenanceFacts::compute(code, jit_code, n_params, native_reg_types);
    native_loop_preserves_heap_query(
        query_args,
        NativeHeapDomain::FieldSlot(slot),
        jit_code,
        provenance.as_ref(),
        header,
        exit,
        query_ip,
    )
}

#[cfg(feature = "native-jit")]
fn native_field_load_args_loop_stable(
    args: &[vm_jit::HostArg],
    invariants: &NativeLoopInvariants,
    jit_code: &[vm_jit::JitInstr],
    provenance: Option<&NativeHeapProvenanceFacts>,
    header: usize,
    exit: usize,
    helper_ip: usize,
    _n_regs: usize,
) -> bool {
    let Some(vm_jit::HostArg::Reg(base)) = args.first().copied() else {
        return false;
    };
    let Some(vm_jit::HostArg::ImmI64(slot)) = args.get(1).copied() else {
        return false;
    };
    if !native_loop_preserves_heap_query(
        args,
        NativeHeapDomain::FieldSlot(slot),
        jit_code,
        provenance,
        header,
        exit,
        helper_ip,
    ) {
        return false;
    }
    native_reg_loop_invariant_at(base as usize, invariants, helper_ip)
}

/// Whether a host helper is a copy-on-write struct/variant field store
/// (`FieldSetInt` and its Float/Handle counterparts). Field-read stability must
/// treat all three as stores or a differently typed write could leave a stale memo.
#[cfg(feature = "native-jit")]
fn is_native_field_set_helper(helper: vm_jit::HostHelper) -> bool {
    matches!(
        helper,
        vm_jit::HostHelper::FieldSetInt
            | vm_jit::HostHelper::FieldSetFloat
            | vm_jit::HostHelper::FieldSetHandle
    )
}

#[cfg(feature = "native-jit")]
fn native_jit_written_reg(instr: &vm_jit::JitInstr) -> Option<u32> {
    match instr {
        vm_jit::JitInstr::LoadInt { dst, .. }
        | vm_jit::JitInstr::LoadFloat { dst, .. }
        | vm_jit::JitInstr::LoadBool { dst, .. }
        | vm_jit::JitInstr::Move { dst, .. }
        | vm_jit::JitInstr::Add { dst, .. }
        | vm_jit::JitInstr::Sub { dst, .. }
        | vm_jit::JitInstr::Mul { dst, .. }
        | vm_jit::JitInstr::Div { dst, .. }
        | vm_jit::JitInstr::Mod { dst, .. }
        | vm_jit::JitInstr::BitAnd { dst, .. }
        | vm_jit::JitInstr::BitOr { dst, .. }
        | vm_jit::JitInstr::BitXor { dst, .. }
        | vm_jit::JitInstr::Shl { dst, .. }
        | vm_jit::JitInstr::Shr { dst, .. }
        | vm_jit::JitInstr::Compare { dst, .. }
        | vm_jit::JitInstr::Equal { dst, .. }
        | vm_jit::JitInstr::NotEqual { dst, .. }
        | vm_jit::JitInstr::IntToFloat { dst, .. }
        | vm_jit::JitInstr::FloatToInt { dst, .. }
        | vm_jit::JitInstr::HostCall { dst, .. }
        | vm_jit::JitInstr::MemoizedHostCall { dst, .. }
        | vm_jit::JitInstr::ListGetIntDirect { dst, .. }
        | vm_jit::JitInstr::ListSetIntDirect { dst, .. }
        | vm_jit::JitInstr::ListGetFloatDirect { dst, .. }
        | vm_jit::JitInstr::ListSetFloatDirect { dst, .. }
        | vm_jit::JitInstr::ListLenDirect { dst, .. }
        | vm_jit::JitInstr::ListIsEmptyDirect { dst, .. }
        | vm_jit::JitInstr::MatchMapGetInt { value_dst: dst, .. }
        | vm_jit::JitInstr::MatchMapGetFloat { value_dst: dst, .. }
        | vm_jit::JitInstr::MatchSortedMapGetInt { value_dst: dst, .. }
        | vm_jit::JitInstr::MatchSortedMapGetFloat { value_dst: dst, .. }
        | vm_jit::JitInstr::CallNative { dst, .. }
        | vm_jit::JitInstr::CallSelf { dst, .. } => Some(*dst),
        _ => None,
    }
}

#[cfg(feature = "native-jit")]
#[derive(Clone)]
struct NativeLoopInvariants {
    written: Vec<bool>,
    constant_int: Vec<Option<i64>>,
    constant_string: Vec<Option<Rc<String>>>,
    first_write_ip: Vec<Option<usize>>,
    write_count: Vec<u32>,
    derived_invariant: Vec<bool>,
}

#[cfg(feature = "native-jit")]
fn native_loop_invariant_regs(
    code: &[RegInstr],
    reachable: &[bool],
    header: usize,
    exit: usize,
    n_regs: usize,
) -> Option<NativeLoopInvariants> {
    let mut written = vec![false; n_regs];
    let mut constant_int = vec![None; n_regs];
    let mut constant_string: Vec<Option<Rc<String>>> = vec![None; n_regs];
    let mut constant_ok = vec![true; n_regs];
    let mut first_write_ip = vec![None; n_regs];
    let mut write_count = vec![0_u32; n_regs];
    for ip in header..exit {
        if !reachable.get(ip).copied().unwrap_or(false) {
            continue;
        }
        let writes = match instr_written_reg(&code[ip]) {
            RegFootprint::Some(writes) => writes,
            RegFootprint::All => return None,
        };
        for written_reg in writes {
            if written_reg >= n_regs {
                continue;
            }
            if matches!(
                &code[ip],
                RegInstr::Move { dst, src } if *dst == written_reg && dst == src
            ) {
                continue;
            }
            written[written_reg] = true;
            write_count[written_reg] = write_count[written_reg].saturating_add(1);
            if first_write_ip[written_reg].is_none() {
                first_write_ip[written_reg] = Some(ip);
            }
            match &code[ip] {
                RegInstr::LoadInt { dst, value } if *dst == written_reg => {
                    match constant_int[written_reg] {
                        Some(existing) if existing != *value => constant_ok[written_reg] = false,
                        Some(_) => {}
                        None => constant_int[written_reg] = Some(*value),
                    }
                }
                RegInstr::LoadString { dst, value } if *dst == written_reg => {
                    match &constant_string[written_reg] {
                        Some(existing) if **existing != **value => constant_ok[written_reg] = false,
                        Some(_) => {}
                        None => constant_string[written_reg] = Some(Rc::clone(value)),
                    }
                }
                _ => constant_ok[written_reg] = false,
            }
        }
    }
    for reg in 0..n_regs {
        if !constant_ok[reg] {
            constant_int[reg] = None;
            constant_string[reg] = None;
        }
    }
    Some(NativeLoopInvariants {
        written,
        constant_int,
        constant_string,
        first_write_ip,
        write_count,
        derived_invariant: vec![false; n_regs],
    })
}

#[cfg(feature = "native-jit")]
fn native_host_args_loop_invariant(
    args: &[vm_jit::HostArg],
    invariants: &NativeLoopInvariants,
    helper_ip: usize,
) -> bool {
    args.iter().all(|arg| match arg {
        vm_jit::HostArg::ImmI64(_) => true,
        vm_jit::HostArg::Reg(reg) => {
            native_reg_loop_invariant_at(*reg as usize, invariants, helper_ip)
        }
    })
}

#[cfg(feature = "native-jit")]
fn native_propagate_derived_loop_invariant(
    instr: &RegInstr,
    invariants: &mut NativeLoopInvariants,
    ip: usize,
) {
    let RegInstr::Move { dst, src } = instr else {
        return;
    };
    if dst == src {
        return;
    }
    if invariants
        .write_count
        .get(*dst)
        .is_some_and(|count| *count == 1)
        && native_reg_loop_invariant_at(*src, invariants, ip)
    {
        if let Some(derived) = invariants.derived_invariant.get_mut(*dst) {
            *derived = true;
        }
    }
}

#[cfg(feature = "native-jit")]
fn native_reg_loop_invariant_at(
    reg: usize,
    invariants: &NativeLoopInvariants,
    use_ip: usize,
) -> bool {
    if !invariants.written.get(reg).copied().unwrap_or(true) {
        return true;
    }
    let written_before_use = invariants
        .first_write_ip
        .get(reg)
        .and_then(|ip| *ip)
        .is_some_and(|ip| ip < use_ip);
    written_before_use
        && (invariants
            .constant_int
            .get(reg)
            .is_some_and(|value| value.is_some())
            || invariants
                .constant_string
                .get(reg)
                .is_some_and(|value| value.is_some())
            || invariants
                .derived_invariant
                .get(reg)
                .copied()
                .unwrap_or(false))
}

#[cfg(feature = "native-jit")]
fn native_memo_scope_representable(code: &[RegInstr], header: usize, exit: usize) -> bool {
    (header..exit).any(|ip| {
        matches!(
            code.get(ip),
            Some(RegInstr::Jump { target }) if *target == header
        )
    }) && !(header..exit).any(|ip| {
        let targets_header = match code.get(ip) {
            Some(RegInstr::JumpIfBool { target, .. })
            | Some(RegInstr::JumpIfIntCompare { target, .. }) => *target == header,
            Some(RegInstr::MatchOption {
                some_ip, none_ip, ..
            })
            | Some(RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            })
            | Some(RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            }) => *some_ip == header || *none_ip == header,
            _ => false,
        };
        targets_header
    })
}

/// `Some(())` if the condition holds, else `None` — lets the translator use `?`
/// to bail out of a non-eligible function.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn require(condition: bool) -> Option<()> {
    condition.then_some(())
}

/// A single natural loop identified for OSR (J5.2): the conservative shape this
/// slice compiles. `header` is the loop's entry instruction (a conditional branch
/// that is the target of the loop's backedge); `exit` is the post-loop instruction
/// the header's branch leaves to. Native execution OSR-enters at `header` and
/// OSR-exits (deopts) at `exit`.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) struct OsrLoop {
    pub(in crate::reg_vm) header: usize,
    pub(in crate::reg_vm) exit: usize,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) struct OsrDerivedLiveIn {
    pub(in crate::reg_vm) native_reg: usize,
    pub(in crate::reg_vm) base_reg: usize,
    pub(in crate::reg_vm) field_slot: usize,
    pub(in crate::reg_vm) ty: NativeTy,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) struct OsrScalarField {
    pub(in crate::reg_vm) native_reg: usize,
    pub(in crate::reg_vm) base_reg: usize,
    pub(in crate::reg_vm) field_slot: usize,
    pub(in crate::reg_vm) writeback: bool,
}

/// A compiled OSR loop cached per function. The OSR loop is detected and compiled
/// on the (possibly J3-scalar-replaced) `code`, so its native `resume_ip` indexes
/// that transformed stream. The interpreter, however, executes the ORIGINAL
/// `func.code`; the two stored `orig_*` ips translate the OSR boundary back:
///   - `orig_header`: the original-code header ip the interpreter must be at for
///     the OSR to fire (the header gate). When no Option was scalar-replaced this
///     equals `trans_exit`'s loop header in original space (identity ip-map).
///   - `trans_exit`: the transformed-code exit ip (= the native `resume_ip` the
///     OSR-exit deopt reports). Used to validate the deopt resumed at the loop's
///     single exit.
///   - `orig_exit`: the ORIGINAL-code post-loop ip the interpreter resumes at —
///     `ip_map[trans_exit]`. Set the frame ip to this after an OSR-exit.
/// Loop-carried (live-in/out) registers keep their original indices (J3 only adds
/// fresh tag/payload regs used strictly inside the loop and dead at both
/// boundaries), so the marshalling window and live-out restore are unchanged.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone)]
pub(in crate::reg_vm) struct OsrEntry {
    pub(in crate::reg_vm) id: vm_jit::CompiledId,
    pub(in crate::reg_vm) orig_header: usize,
    pub(in crate::reg_vm) trans_exit: usize,
    pub(in crate::reg_vm) orig_exit: usize,
    /// Width of the OSR register window the native ABI expects — the TRANSFORMED
    /// register count (`func.regs` plus any J3-added tag/payload regs). The
    /// marshalling window and `lens` slice must be exactly this wide.
    pub(in crate::reg_vm) n_jit_regs: usize,
    pub(in crate::reg_vm) param_types: Vec<NativeTy>,
    pub(in crate::reg_vm) derived_liveins: Vec<OsrDerivedLiveIn>,
    pub(in crate::reg_vm) scalar_fields: Vec<OsrScalarField>,
    pub(in crate::reg_vm) heap_input_regs: Vec<usize>,
    /// Per-register native types of the compiled OSR body. Used at OSR-exit to skip
    /// restoring **Handle**-class registers: a loop-internal handle (a stored
    /// struct/closure fetched via `FieldHandle`/`ListGetHandle`) is dead at the exit
    /// and its live-out "value" is only a heap-table index — restoring it as an Int
    /// into the interpreter slot would corrupt the register. The interpreter re-
    /// derives any still-needed heap value; a dead one is simply never read.
    pub(in crate::reg_vm) reg_types: Vec<NativeTy>,
    /// Registers written by the native OSR loop body. Live-through registers that
    /// are assigned before the loop but never written natively must not be restored
    /// from the scalar deopt payload: a heap/list live-through slot is already
    /// correct in the interpreter window, while its native payload word may be an
    /// opaque handle-table index or an untyped zero.
    pub(in crate::reg_vm) written_regs: Vec<bool>,
    pub(in crate::reg_vm) string_literals: Vec<Rc<String>>,
    /// Bounded clean-exit reconstruction trees for scalar-replaced aggregates that
    /// remain live after the OSR region. Every leaf is verified as a scalar or Handle
    /// register before this entry is cached.
    pub(in crate::reg_vm) materialize_recipes: Vec<super::passes::OsrMaterializeRecipe>,
}

/// Detect one natural loop at a specific header, allowing other disjoint loops
/// elsewhere in the same function. This is intentionally narrower than arbitrary
/// CFG loop discovery: it uses the same single-entry/single-exit validation as
/// [`detect_single_natural_loop`] for the selected `[header, exit)` region, but it
/// does not reject merely because a setup loop exists before the hot loop.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn detect_natural_loop_at(
    code: &[RegInstr],
    header: usize,
) -> Option<OsrLoop> {
    let n = code.len();
    if header >= n {
        return None;
    }
    let backedges = NativeRegionCfg::prefix(code, code.len())?.backedges_to(header);
    if backedges.is_empty() {
        return None;
    }
    let body_end = backedges.into_iter().max().unwrap();

    let mut cond_ip = header;
    while cond_ip < n
        && !matches!(
            code[cond_ip],
            RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. }
        )
    {
        cond_ip += 1;
    }
    if cond_ip > body_end {
        return None;
    }
    let exit = match &code[cond_ip] {
        RegInstr::JumpIfIntCompare { target, .. } | RegInstr::JumpIfBool { target, .. } => *target,
        _ => return None,
    };
    if exit <= body_end || exit > n {
        return None;
    }

    for i in header..=body_end {
        let in_region = |t: usize| t >= header && t < exit;
        match &code[i] {
            RegInstr::Jump { target } => {
                if !in_region(*target) {
                    return None;
                }
            }
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                if i == cond_ip {
                    continue;
                }
                if !in_region(*target) {
                    return None;
                }
            }
            RegInstr::Return { .. } => return None,
            RegInstr::RuntimeError { .. } => {}
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                if !in_region(*some_ip) || !in_region(*none_ip) {
                    return None;
                }
            }
            _ => {}
        }
    }

    for (i, instr) in code.iter().enumerate() {
        if i >= header && i < exit {
            continue;
        }
        let enters_interior = |t: usize| t > header && t < exit;
        let bad = match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => enters_interior(*target),
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => enters_interior(*some_ip) || enters_interior(*none_ip),
            _ => false,
        };
        if bad {
            return None;
        }
    }
    Some(OsrLoop { header, exit })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn detect_natural_loops(code: &[RegInstr]) -> Vec<OsrLoop> {
    let mut headers = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        let mut push_header = |target: usize| {
            if target <= i && !headers.contains(&target) {
                headers.push(target);
            }
        };
        match instr {
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                push_header(*some_ip);
                push_header(*none_ip);
            }
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => push_header(*target),
            _ => {}
        }
    }
    headers.sort_unstable();
    headers
        .into_iter()
        .filter_map(|header| detect_natural_loop_at(code, header))
        .collect()
}

/// Identify the single natural loop OSR will compile, **conservatively** (any
/// shape we cannot analyze soundly returns `None`, so OSR does not apply).
///
/// The accepted shape is a **reducible natural loop with a single header `h`**,
/// lowered as `while cond { body }` (the body may contain internal forward control
/// flow, e.g. an `if x { ... }` reset):
///   - `header` `h`: a `JumpIfIntCompare`/`JumpIfBool` at `h` whose `target` is the
///     post-loop `exit` (the branch *leaves* the loop; fall-through stays in body),
///   - one or more **backedges** `b → h` (a `Jump`/`JumpIf*`/`MatchOption` arm whose
///     target is `≤ b`), **ALL targeting the same header `h`** (multiple backedges
///     are collapsed; backedges to two different headers ⇒ nested/multiple loops ⇒
///     reject), and
///   - the contiguous region `[h, exit)` is **single-exit**: the ONLY edge leaving
///     it is the header's exit edge to `exit`. Every other in-body branch (forward
///     `if`/`match`, or a backedge) stays within `[h, exit)`. No in-body
///     `Return` (a value-producing extra exit) is allowed.
///
/// A single header (all backedges collapsed), a single exit edge, and a contiguous
/// `[h, exit)` body make the region single-entry/single-exit — the only thing we can
/// OSR soundly. Multi-header / multi-exit / non-contiguous shapes return `None`.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn detect_single_natural_loop(code: &[RegInstr]) -> Option<OsrLoop> {
    let n = code.len();
    // Collect backedges from the shared native CFG descriptor. This matters when
    // this runs on UNTRANSFORMED code (OSR x J3, before scalar replacement): a
    // match arm that jumps backward must be treated like any other control edge.
    let backedges = NativeRegionCfg::prefix(code, code.len())?.backedges();
    // At least one backedge ⇒ a loop exists.
    if backedges.is_empty() {
        return None;
    }
    // Collapse multiple backedges to the SAME header. Backedges to two DIFFERENT
    // headers mean nested/sibling loops — out of scope, reject. The single shared
    // header `h` is the loop entry; `body_end` is the furthest backedge source, so
    // the contiguous loop body is `h..=body_end`.
    let header = backedges[0].1;
    if backedges.iter().any(|&(_, h)| h != header) {
        return None;
    }
    if header >= n {
        return None;
    }
    let body_end = backedges.iter().map(|&(from, _)| from).max().unwrap();
    // The header BLOCK is a (possibly empty) leading run of STRAIGHT-LINE instructions
    // (no jump/branch/match/return) followed by the loop's conditional branch. This
    // admits a `while cond { body }` whose CONDITION computes a value before the
    // compare — e.g. `while i < Bytes.len(data)` lowers to `BytesLen -> t; JumpIf i <
    // t` with the backedge targeting the `BytesLen`. The Bytes length-fold then (a)
    // rewrites that in-header `Bytes.len` to a `Move` from a constant length register
    // and (b) materializes the constant length as a `LoadInt` AT the header, so the
    // value is definitely-assigned on entry to the native OSR header block (which is
    // where OSR-entry lands) and dominates the condition's read. Because the prefix is
    // straight-line, the block is single-entry (the backedge targets `header`, the
    // sole entry); if the prefix is not foldable to the native subset, `translate_osr_
    // loop` rejects it and the loop simply stays on the interpreter (safe). A loop
    // whose condition is a bare compare has `cond_ip == header`, exactly as before, so
    // ordinary loops are unaffected.
    let mut cond_ip = header;
    while cond_ip < n
        && !matches!(
            code[cond_ip],
            RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. }
        )
    {
        cond_ip += 1;
    }
    if cond_ip > body_end {
        return None;
    }
    // The condition must be a `JumpIfIntCompare`/`JumpIfBool` whose `target` is the
    // post-loop exit (the fall-through stays in the loop body). The exit must lie
    // outside the body.
    let exit = match &code[cond_ip] {
        RegInstr::JumpIfIntCompare { target, .. } | RegInstr::JumpIfBool { target, .. } => *target,
        _ => return None,
    };
    // The loop body is `header..=body_end`; the exit must be after it (the loop's
    // only way out). A header whose exit target points back inside the body is not
    // the while-shape we accept.
    if exit <= body_end || exit > n {
        return None;
    }
    // The set of backedge source indices (each must be a Jump/JumpIf* back to the
    // header; checked in-region below — a backedge to `header` is in `[header, exit)`,
    // so it is NOT an escaping edge and needs no special exemption).
    //
    // No instruction in the body `header..=body_end` may transfer control outside
    // `[header, exit)` except the header's own exit edge. (Any other escape would
    // mean multiple exits / an irreducible shape.) Internal forward branches and
    // backedges to `header` stay in-region, so they pass the same `in_region` test.
    for i in header..=body_end {
        let in_region = |t: usize| t >= header && t < exit;
        match &code[i] {
            RegInstr::Jump { target } => {
                if !in_region(*target) {
                    return None;
                }
            }
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                // The header condition's exit edge to `exit` is the sole permitted
                // escape (the condition sits at `cond_ip`, after any `LoadInt` prefix).
                if i == cond_ip {
                    continue;
                }
                if !in_region(*target) {
                    return None;
                }
            }
            // A `Return` inside the loop is a value-producing exit we do not model in
            // the single OSR-exit — bail conservatively.
            RegInstr::Return { .. } => return None,
            // A `RuntimeError` inside the loop is a trap, not a normal loop exit. It
            // is compiled to `JitInstr::Bail` (deopt to the interpreter, which then
            // re-runs the loop and raises the error itself if actually reached). The
            // exhaustive-match lowering emits a statically-reachable-but-dynamically-
            // dead `RuntimeError` after an `Option` match, so accepting it (as a bail)
            // is what lets Option-bearing loops OSR at all.
            RegInstr::RuntimeError { .. } => {}
            // `MatchOption` (untransformed-code path): both arms must stay in-region.
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                if !in_region(*some_ip) || !in_region(*none_ip) {
                    return None;
                }
            }
            // Any non-straight-line call inside the body is rejected by the subset
            // check in `translate_osr_loop`; control-flow-wise it falls through,
            // which stays in-region.
            _ => {}
        }
    }
    // Single-ENTRY check: OSR enters the region only at `header`. No instruction
    // OUTSIDE `[header, exit)` may branch INTO the body interior `(header, exit)`
    // (an edge to `header` itself is the legal loop entry / fall-through). An
    // external edge into the middle would make the region multi-entry and the
    // contiguous-region/ip-map assumptions unsound, so reject. (Lowered while-loops
    // never do this; this guards an irreducible CFG defensively.)
    for (i, instr) in code.iter().enumerate() {
        if i >= header && i < exit {
            continue; // in-body edges already validated above
        }
        let enters_interior = |t: usize| t > header && t < exit;
        let bad = match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => enters_interior(*target),
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => enters_interior(*some_ip) || enters_interior(*none_ip),
            _ => false,
        };
        if bad {
            return None;
        }
    }
    Some(OsrLoop { header, exit })
}

/// Build an OSR [`vm_jit::JitFunction`] for the loop `lp` in `code`. `code` is the
/// (possibly J3-scalar-replaced) instruction stream; `lp.header`/`lp.exit` index
/// into it. The JIT is index-aligned with `code` so a native OSR-exit's
/// `resume_ip` is the `code` instruction index — which the caller maps back to the
/// ORIGINAL `func.code` post-loop ip via the J3 ip-map (identity when no Option was
/// replaced, so the keystone holds: indices into `code` track the interpreter's
/// resume position through the ip-map).
///
/// Every instruction in the loop body region must be in the native subset. The
/// exit instruction becomes [`vm_jit::JitInstr::OsrExit`] (deopt back to the
/// interpreter with the live-out window). All instructions outside the loop region
/// (the pre-loop, the post-loop) become [`vm_jit::JitInstr::Bail`]/`Nop`: they are
/// never reached natively (entry is the header, the only exit is `OsrExit`), but
/// they keep the index alignment. Returns the function plus the per-register types
/// so the caller can marshal the live-in window. `None` if the loop body leaves
/// the subset or types don't unify.
#[cfg(feature = "native-jit")]
#[allow(dead_code)]
pub(in crate::reg_vm) fn translate_osr_loop(
    code: &[RegInstr],
    n_regs: usize,
    n_params: usize,
    captures: usize,
    lp: OsrLoop,
) -> Option<(
    vm_jit::JitFunction,
    Vec<NativeTy>,
    Vec<OsrDerivedLiveIn>,
    Vec<OsrScalarField>,
    Vec<NativeTy>,
    Vec<bool>,
    Vec<Rc<String>>,
)> {
    translate_osr_loop_inner(
        code,
        n_regs,
        n_params,
        captures,
        lp,
        Vec::new(),
        HashMap::new(),
        &[],
        // No signature available on the bare entry (unit tests): treat every param
        // conservatively (none proven an immutable leaf) so the guard never under-declines.
        &[],
    )
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn translate_osr_loop_profiled(
    func: &RegFunction,
    code: &[RegInstr],
    n_regs: usize,
    n_params: usize,
    captures: usize,
    lp: OsrLoop,
    ip_map: &[usize],
    param_native_types: &[Option<NativeTy>],
    immutable_leaf_params: &[bool],
) -> Option<(
    vm_jit::JitFunction,
    Vec<NativeTy>,
    Vec<OsrDerivedLiveIn>,
    Vec<OsrScalarField>,
    Vec<NativeTy>,
    Vec<bool>,
    Vec<Rc<String>>,
)> {
    let profile_guidance = native_osr_profile_guidance(func, code, n_regs, lp, ip_map);
    translate_osr_loop_inner(
        code,
        n_regs,
        n_params,
        captures,
        lp,
        profile_guidance.cold_blocks,
        profile_guidance.hot_branch_edges,
        param_native_types,
        immutable_leaf_params,
    )
}

/// Union-find `find` with path-halving, used by the OSR translator's Handle-`Move`
/// alias-class computations. A proper union-find is REQUIRED there: the older
/// `alias[dst] = alias[src]` propagation-to-fixpoint oscillates forever on a cyclic
/// Handle-`Move` graph (which a two-armed `Result<Handle,_>` dissolution produces),
/// hanging the translator.
#[cfg(feature = "native-jit")]
fn osr_uf_find(a: &mut [usize], mut x: usize) -> usize {
    while a[x] != x {
        a[x] = a[a[x]]; // path-halving
        x = a[x];
    }
    x
}

#[cfg(feature = "native-jit")]
fn translate_osr_loop_inner(
    code: &[RegInstr],
    n_regs: usize,
    n_params: usize,
    captures: usize,
    lp: OsrLoop,
    cold_blocks: Vec<u32>,
    profile_hot_branch_edges: HashMap<usize, bool>,
    param_native_types: &[Option<NativeTy>],
    immutable_leaf_params: &[bool],
) -> Option<(
    vm_jit::JitFunction,
    Vec<NativeTy>,
    Vec<OsrDerivedLiveIn>,
    Vec<OsrScalarField>,
    Vec<NativeTy>,
    Vec<bool>,
    Vec<Rc<String>>,
)> {
    use vm_jit::{JitCompare, JitInstr};

    if captures != 0 {
        return None;
    }
    let n = code.len();
    if lp.header >= n || lp.exit > n {
        return None;
    }

    // The set of instruction indices that belong to the loop region (header..exit).
    // Only these run natively; the type inference and subset check apply to them.
    let in_loop = |i: usize| i >= lp.header && i < lp.exit;

    // Every loop-region instruction must be a native-subset instruction. (The exit
    // and everything outside the region may be anything — they don't run natively.)
    for i in lp.header..lp.exit {
        if !native_subset_instruction(&code[i]) {
            return None;
        }
    }

    // Type inference by unification over the loop region only (a fixpoint to handle
    // the backedge). Same rules as `translate_to_native_jit`; registers live-in to
    // the header acquire their type from the operands they combine with.
    let mut ty: Vec<Option<NativeTy>> = vec![None; n_regs];
    // Seed live-in scalar types from simple preheader definitions. This is deliberately
    // conservative: constants and moves are type-stable and dominate the OSR entry
    // in normal lowered `while` code, while opaque heap/call/list writes are still
    // inferred from in-loop typed uses or left to runtime marshalling to decline.
    let mut pre_changed = true;
    while pre_changed {
        pre_changed = false;
        for instr in &code[..lp.header] {
            let ok = match instr {
                RegInstr::LoadInt { dst, .. } => {
                    native_set_ty(&mut ty, *dst, NativeTy::Int, &mut pre_changed)
                }
                RegInstr::LoadFloat { dst, .. } => {
                    native_set_ty(&mut ty, *dst, NativeTy::Float, &mut pre_changed)
                }
                RegInstr::LoadBool { dst, .. } => {
                    native_set_ty(&mut ty, *dst, NativeTy::Bool, &mut pre_changed)
                }
                RegInstr::LoadString { dst, .. } => {
                    native_set_ty(&mut ty, *dst, NativeTy::Handle, &mut pre_changed)
                }
                RegInstr::Move { dst, src } => native_unify(&mut ty, *dst, *src, &mut pre_changed),
                _ => true,
            };
            if !ok {
                return None;
            }
        }
    }
    // J0.1 #7 / J0.4 #1: seed each param the preheader inference left untyped from its
    // runtime native type (`try_osr` classifies the live value: Int/Float scalars, heap
    // values → Handle; flat-capable List/Deque withheld as `None`). This types a param
    // used in-region with no typed-use to unify against — a dissolved Result/Option
    // payload `Move` source (live-after reconstruction), OR a heap collection value pushed
    // by an unconstrained-value op (`ListPush`/`MapInsert`-value/`DequePush`/`SetFieldSlot`,
    // which leave the value to flow). It does NOT conflict with the collection-op inference:
    // that inference now types a heap key/value operand `Handle` (via `heap_param`) too, so
    // the seed and the inference agree. Fill-`None` only, so a flat-list param the region
    // pass classifies wins.
    for (reg, seed) in param_native_types.iter().enumerate().take(n_params) {
        if let Some(t) = seed {
            if reg < n_regs && ty[reg].is_none() {
                ty[reg] = Some(*t);
            }
        }
    }
    // J0.4 #1 correctness: a heap collection key/value that is a PARAM is classified
    // `Handle` by `try_osr` from its live value. The collection-op inference below must
    // type such an operand `Handle` (not the default `Int`), or the lowering's
    // `handle_reg` test never fires and a heap key/value is mis-lowered to the Int helper
    // (storing the operand's heap-table INDEX as an Int — a silent mis-key/mis-value the
    // original single-element tests didn't catch). A non-heap-param operand keeps its
    // prior typing.
    let heap_param = |reg: usize| {
        reg < n_params && matches!(param_native_types.get(reg), Some(Some(NativeTy::Handle)))
    };
    let mut changed = true;
    while changed {
        changed = false;
        for i in lp.header..lp.exit {
            let instr = &code[i];
            let ty = &mut ty;
            let c = &mut changed;
            let ok = match instr {
                RegInstr::LoadInt { dst, .. } => native_set_ty(ty, *dst, NativeTy::Int, c),
                RegInstr::LoadFloat { dst, .. } => native_set_ty(ty, *dst, NativeTy::Float, c),
                RegInstr::LoadBool { dst, .. } => native_set_ty(ty, *dst, NativeTy::Bool, c),
                RegInstr::LoadString { dst, .. } => native_set_ty(ty, *dst, NativeTy::Handle, c),
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
                RegInstr::ListLen { dst, list } => {
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListGet { list, index, .. } => {
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *index, NativeTy::Int, c)
                }
                RegInstr::ListSet {
                    dst,
                    list,
                    index,
                    value,
                } => {
                    // Preserve Float element facts from the OSR live-in/value flow;
                    // otherwise retain the existing heap-live-in/Int defaults.
                    let value_ty = if ty[*value] == Some(NativeTy::Float) {
                        NativeTy::Float
                    } else if heap_param(*value) {
                        NativeTy::Handle
                    } else {
                        NativeTy::Int
                    };
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *index, NativeTy::Int, c)
                        && native_set_ty(ty, *value, value_ty, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListPush {
                    dst,
                    list,
                    value: _,
                } => {
                    // The pushed `value`'s type flows from its definition (Int or
                    // Float); lowering picks `ListPushInt`/`ListPushFloat` and a
                    // wrong-element-type list bails at the helper. `dst` is the Int
                    // result (0 on success).
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListSort { dst, list } => {
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::MapInsert {
                    dst,
                    map,
                    key,
                    value: _,
                } => {
                    // The value type flows from its definition (Int or Float);
                    // lowering picks MapInsertInt/MapInsertFloat, and a wrong-value-type
                    // map bails at the helper.
                    native_set_ty(ty, *map, NativeTy::Handle, c)
                        && native_set_ty(
                            ty,
                            *key,
                            if heap_param(*key) {
                                NativeTy::Handle
                            } else {
                                NativeTy::Int
                            },
                            c,
                        )
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::SetInsert { dst, set, value } => {
                    native_set_ty(ty, *set, NativeTy::Handle, c)
                        && native_set_ty(
                            ty,
                            *value,
                            if heap_param(*value) {
                                NativeTy::Handle
                            } else {
                                NativeTy::Int
                            },
                            c,
                        )
                        && native_set_ty(ty, *dst, NativeTy::Bool, c)
                }
                RegInstr::SortedSetInsert { dst, set, value } => {
                    native_set_ty(ty, *set, NativeTy::Handle, c)
                        && native_set_ty(
                            ty,
                            *value,
                            if heap_param(*value) {
                                NativeTy::Handle
                            } else {
                                NativeTy::Int
                            },
                            c,
                        )
                        && native_set_ty(ty, *dst, NativeTy::Bool, c)
                }
                RegInstr::SortedMapInsert {
                    dst,
                    map,
                    key,
                    value,
                } => {
                    native_set_ty(ty, *map, NativeTy::Handle, c)
                        && native_set_ty(
                            ty,
                            *key,
                            if heap_param(*key) {
                                NativeTy::Handle
                            } else {
                                NativeTy::Int
                            },
                            c,
                        )
                        && native_set_ty(
                            ty,
                            *value,
                            if heap_param(*value) {
                                NativeTy::Handle
                            } else {
                                NativeTy::Int
                            },
                            c,
                        )
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::DequePushBack {
                    dst,
                    deque,
                    value: _,
                }
                | RegInstr::DequePushFront {
                    dst,
                    deque,
                    value: _,
                } => {
                    // The value type flows (Int or Float); lowering picks the helper.
                    native_set_ty(ty, *deque, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::DequePopFront { dst: _, deque }
                | RegInstr::DequePopBack { dst: _, deque } => {
                    // dst (popped value) flows (Int or Float); lowering picks the helper.
                    native_set_ty(ty, *deque, NativeTy::Handle, c)
                }
                RegInstr::MatchMapGet { map, key, .. } => {
                    // value_dst flows from its uses (Int or Float); lowering picks
                    // MatchMapGetInt/MatchMapGetFloat, and a wrong-value-type map
                    // bails at the helper.
                    native_set_ty(ty, *map, NativeTy::Handle, c)
                        && native_set_ty(
                            ty,
                            *key,
                            if heap_param(*key) {
                                NativeTy::Handle
                            } else {
                                NativeTy::Int
                            },
                            c,
                        )
                }
                RegInstr::MatchSortedMapGet { map, key, .. } => {
                    // value_dst flows (Int or Float); lowering picks
                    // MatchSortedMapGetInt/MatchSortedMapGetFloat.
                    native_set_ty(ty, *map, NativeTy::Handle, c)
                        && native_set_ty(
                            ty,
                            *key,
                            if heap_param(*key) {
                                NativeTy::Handle
                            } else {
                                NativeTy::Int
                            },
                            c,
                        )
                }
                RegInstr::GetFieldSlot { base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                }
                RegInstr::SetFieldSlot { dst, base, .. } => {
                    // The written `value`'s type is left to flow from its definition
                    // (Int or Float) — lowering then picks `FieldSetInt`/`FieldSetFloat`
                    // accordingly, and a runtime field-type mismatch bails. `dst` is the
                    // Unit result sentinel (the native write returns the new handle into
                    // `base`); an Int placeholder is never read.
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                // Synthetic closure-inline ops (only present when an inlined
                // capturing/monomorphic closure body landed in the OSR region):
                // the closure operand is a native Handle param; an id read is
                // `Int`-class. A materialized capture's `dst` is left to flow from
                // its uses (Int/Bool/Float) — the helper returns the raw scalar bit
                // pattern (a `Float` via `f64::to_bits`), and lowering admits only a
                // provably Int/Bool/Float `dst`, bit-reinterpreting a Float slot.
                RegInstr::NativeGuardClosureId { closure, .. } => {
                    native_set_ty(ty, *closure, NativeTy::Handle, c)
                }
                RegInstr::NativeClosureId { dst, closure } => {
                    native_set_ty(ty, *closure, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::NativeFieldClosureId { dst, base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::NativeClosureCapture {
                    dst: _, closure, ..
                } => native_set_ty(ty, *closure, NativeTy::Handle, c),
                RegInstr::NativeFieldClosureCapture { base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                }
                // `Int.to_float`: single Int arg → Float dst (signed-int→f64
                // conversion via `fcvt_from_sint` at lowering).
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::IntToFloat,
                    args,
                    dst,
                } => {
                    native_set_ty(ty, args[0], NativeTy::Int, c)
                        && native_set_ty(ty, *dst, NativeTy::Float, c)
                }
                // `Math.floor`/`Math.ceil`: single Float arg → Int dst (round, then
                // saturating f64→i64 cast via `FloatToInt` at lowering).
                RegInstr::CallIntrinsic {
                    intrinsic: RegIntrinsic::MathFloor | RegIntrinsic::MathCeil,
                    args,
                    dst,
                } => {
                    native_set_ty(ty, args[0], NativeTy::Float, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::CallIntrinsic {
                    intrinsic,
                    args,
                    dst,
                } if native_host_intrinsic(*intrinsic).is_some() => {
                    let spec = native_host_intrinsic(*intrinsic)?;
                    let arg_tys = spec.arg_tys();
                    let mut ok = args.len() == arg_tys.len();
                    if ok {
                        for (arg, expected) in args.iter().zip(arg_tys) {
                            ok = ok && native_set_ty(ty, *arg, expected, c);
                        }
                    }
                    ok && native_set_ty(ty, *dst, spec.result_ty, c)
                }
                RegInstr::CallTypedIntrinsic {
                    intrinsic,
                    type_arg,
                    args,
                    dst,
                } if native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str())).is_some() => {
                    let spec = native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))?;
                    let arg_tys = spec.arg_tys();
                    let mut ok = args.len() == arg_tys.len();
                    if ok {
                        for (arg, expected) in args.iter().zip(arg_tys) {
                            ok = ok && native_set_ty(ty, *arg, expected, c);
                        }
                    }
                    ok && native_set_ty(ty, *dst, spec.result_ty, c)
                }
                _ => true,
            };
            if !ok {
                return None;
            }
        }
    }
    // SOUNDNESS (OSR): mirror the whole-function DeepCopy guard. Only the loop region runs
    // natively (the prologue, which the interpreter deep-copies, stays interpreted), so
    // scope the in-place-mutation / leak check to `[header, exit)`. Taint still seeds from
    // the prologue `DeepCopy` and propagates through prologue + in-region aliasing. Types
    // are settled here (list/heap operands are `Handle`); the flat reclassification below
    // only narrows `Handle`→`Flat*`, both of which `native_is_heap_reg` already accepts.
    // `immutable_leaf_params` (plumbed from `try_osr`'s signature lookup) exempts
    // `String`/`Bytes` params so a `read` string inserted into a `mut` collection still OSRs.
    if native_deepcopy_param_unsoundly_mutated(code, &ty, n_regs, immutable_leaf_params, |i| {
        i >= lp.header && i < lp.exit
    }) {
        return None;
    }
    // TV2 flat-array classification on the OSR path. A `Handle` register that is
    // loop-invariant (never rewritten in-loop) and used as a scalar list can be
    // reclassified to a direct flat buffer. Read-only lists become `FlatInt`/
    // `FlatFloat`; lists written only through in-bounds `ListSet<Int>` become
    // `FlatIntMut`. A `ListPush<Int>` on such a candidate is treated as an uncommon
    // growth trap: the direct native loop bails at the push safepoint and the
    // interpreter re-runs the iteration with the transaction rolled back. This lets
    // steady-state fixed-capacity list loops avoid per-iteration host helpers without
    // assuming growth cannot happen. Anything ambiguous (mixed kind, struct alias,
    // heap element, sort, non-Int write) stays on the Handle helper path.
    let (flat_osr_kind, derived_liveins): (Vec<Option<NativeTy>>, Vec<OsrDerivedLiveIn>) = {
        #[derive(Clone, Copy, PartialEq)]
        enum S {
            Unseen,
            Flat(NativeTy),
            Disq,
        }
        #[derive(Clone, Copy, PartialEq, Eq)]
        struct FieldRead {
            base: usize,
            slot: usize,
        }
        // Group Handle registers connected by in-loop `Move`s into alias classes via
        // union-find. This MUST be a union-find (not a `handle_alias[dst] =
        // handle_alias[src]` propagation-to-fixpoint): a cyclic Handle-`Move` graph —
        // which the two-armed `Result<Handle,_>` dissolution produces (per-arm payload
        // Moves plus the loop-carried back-edge) — makes the naive propagation OSCILLATE
        // forever between equivalent labelings, hanging the translator. Union-find by
        // minimum representative is monotone, so it converges in one pass + a flatten,
        // and the downstream only compares class representatives for EQUALITY, so the
        // grouping (hence behavior) is identical to the old converging cases.
        let mut handle_alias: Vec<usize> = (0..n_regs).collect();
        for i in lp.header..lp.exit {
            if let RegInstr::Move { dst, src } = &code[i] {
                if *dst < n_regs
                    && *src < n_regs
                    && ty[*dst] == Some(NativeTy::Handle)
                    && ty[*src] == Some(NativeTy::Handle)
                {
                    let rd = osr_uf_find(&mut handle_alias, *dst);
                    let rs = osr_uf_find(&mut handle_alias, *src);
                    if rd != rs {
                        let (lo, hi) = if rd < rs { (rd, rs) } else { (rs, rd) };
                        handle_alias[hi] = lo;
                    }
                }
            }
        }
        for r in 0..n_regs {
            handle_alias[r] = osr_uf_find(&mut handle_alias, r);
        }
        // A register written by ANY in-loop instruction is not loop-invariant.
        let mut written_in_loop = vec![false; n_regs];
        let mut field_reads: Vec<Option<FieldRead>> = vec![None; n_regs];
        let mut field_read_disq = vec![false; n_regs];
        for i in lp.header..lp.exit {
            if let Some(dst) = native_subset_dst(&code[i]) {
                if dst < n_regs {
                    written_in_loop[dst] = true;
                }
            }
            if let RegInstr::GetFieldSlot { dst, base, slot } = &code[i] {
                if *dst < n_regs && *base < n_regs {
                    let read = FieldRead {
                        base: handle_alias[*base],
                        slot: *slot,
                    };
                    match field_reads[*dst] {
                        None => field_reads[*dst] = Some(read),
                        Some(existing) if existing == read => {}
                        Some(_) => field_read_disq[*dst] = true,
                    }
                }
            }
        }
        let is_handle_reg = |reg: usize| ty[reg] == Some(NativeTy::Handle);
        let mut st = vec![S::Unseen; n_regs];
        let mut field_slot_written = Vec::<FieldRead>::new();
        for i in lp.header..lp.exit {
            match &code[i] {
                // A struct read disqualifies the base (it is not a flat list).
                RegInstr::GetFieldSlot { base, .. } if is_handle_reg(*base) => {
                    st[*base] = S::Disq;
                }
                RegInstr::SetFieldSlot { base, slot, .. } if *base < n_regs => {
                    field_slot_written.push(FieldRead {
                        base: handle_alias[*base],
                        slot: *slot,
                    });
                }
                RegInstr::ListGet { dst, list, .. } if is_handle_reg(*list) => {
                    // A heap-valued element (e.g. a stored closure/struct) means the
                    // list is NOT a flat scalar buffer ⇒ disqualify.
                    if is_handle_reg(*dst) {
                        st[*list] = S::Disq;
                    } else {
                        let kind = if ty[*dst] == Some(NativeTy::Float) {
                            NativeTy::FlatFloat
                        } else {
                            NativeTy::FlatInt
                        };
                        st[*list] = match st[*list] {
                            S::Unseen => S::Flat(kind),
                            S::Flat(k) if k == kind => S::Flat(kind),
                            S::Flat(NativeTy::FlatIntMut) if kind == NativeTy::FlatInt => {
                                S::Flat(NativeTy::FlatIntMut)
                            }
                            S::Flat(NativeTy::FlatFloatMut) if kind == NativeTy::FlatFloat => {
                                S::Flat(NativeTy::FlatFloatMut)
                            }
                            _ => S::Disq,
                        };
                    }
                }
                RegInstr::ListSet { list, value, .. } if is_handle_reg(*list) => {
                    if ty[*value] == Some(NativeTy::Int) {
                        st[*list] = match st[*list] {
                            S::Unseen
                            | S::Flat(NativeTy::FlatInt)
                            | S::Flat(NativeTy::FlatIntMut) => S::Flat(NativeTy::FlatIntMut),
                            _ => S::Disq,
                        };
                    } else if ty[*value] == Some(NativeTy::Float) {
                        st[*list] = match st[*list] {
                            S::Unseen
                            | S::Flat(NativeTy::FlatFloat)
                            | S::Flat(NativeTy::FlatFloatMut) => S::Flat(NativeTy::FlatFloatMut),
                            _ => S::Disq,
                        };
                    } else {
                        st[*list] = S::Disq;
                    }
                }
                RegInstr::ListPush { list, value, .. } if is_handle_reg(*list) => {
                    if ty[*value] != Some(NativeTy::Int) {
                        st[*list] = S::Disq;
                    } else if matches!(st[*list], S::Flat(NativeTy::FlatFloat)) {
                        st[*list] = S::Disq;
                    }
                }
                RegInstr::ListSort { list, .. } if is_handle_reg(*list) => {
                    st[*list] = S::Disq;
                }
                // `ListLen` is kind-neutral — neither pins nor disqualifies.
                // Any closure read off a handle disqualifies (not a flat list).
                RegInstr::NativeGuardClosureId { closure, .. }
                | RegInstr::NativeClosureId { closure, .. }
                | RegInstr::NativeClosureCapture { closure, .. }
                    if is_handle_reg(*closure) =>
                {
                    st[*closure] = S::Disq;
                }
                _ => {}
            }
        }
        let field_is_stable =
            |read: FieldRead| !field_slot_written.iter().any(|written| *written == read);
        let mut field_kinds: Vec<(FieldRead, NativeTy)> = Vec::new();
        for reg in 0..n_regs {
            let Some(read) = field_reads[reg] else {
                continue;
            };
            if field_read_disq[reg] || !is_handle_reg(read.base) || !field_is_stable(read) {
                continue;
            }
            let S::Flat(kind) = st[reg] else {
                continue;
            };
            if let Some((_, existing)) = field_kinds
                .iter_mut()
                .find(|(existing_read, _)| *existing_read == read)
            {
                if *existing == kind
                    || (*existing == NativeTy::FlatIntMut && kind == NativeTy::FlatInt)
                    || (*existing == NativeTy::FlatFloatMut && kind == NativeTy::FlatFloat)
                {
                    continue;
                }
                if *existing == NativeTy::FlatInt && kind == NativeTy::FlatIntMut {
                    *existing = NativeTy::FlatIntMut;
                    continue;
                }
                if *existing == NativeTy::FlatFloat && kind == NativeTy::FlatFloatMut {
                    *existing = NativeTy::FlatFloatMut;
                    continue;
                }
                continue;
            }
            field_kinds.push((read, kind));
        }
        let field_kind = |read: FieldRead| {
            field_kinds
                .iter()
                .find_map(|(candidate, kind)| (*candidate == read).then_some(*kind))
        };
        let mut derived = Vec::new();
        let flat = (0..n_regs)
            .map(|reg| match st[reg] {
                // Only a loop-invariant (never-rewritten) handle qualifies. A handle
                // produced INSIDE the loop (ListGetHandle/FieldHandle dst) is written
                // in-loop ⇒ excluded here, staying on the Handle path.
                S::Flat(k) if !written_in_loop[reg] => Some(k),
                // A loop-internal handle loaded from a stable struct field can also
                // be marshalled once at OSR entry. The native register is prefilled
                // from `base.field_slot`, and the in-loop `GetFieldSlot` becomes a
                // no-op. This is the field form of the existing flat-list live-in
                // mechanism, not a mailbox-specific helper.
                S::Flat(k) if written_in_loop[reg] => {
                    let read = field_reads[reg]?;
                    let k = field_kind(read).unwrap_or(k);
                    if field_read_disq[reg] || !is_handle_reg(read.base) || !field_is_stable(read) {
                        None
                    } else {
                        derived.push(OsrDerivedLiveIn {
                            native_reg: reg,
                            base_reg: read.base,
                            field_slot: read.slot,
                            ty: k,
                        });
                        Some(k)
                    }
                }
                S::Unseen if written_in_loop[reg] => {
                    let read = field_reads[reg]?;
                    let k = field_kind(read)?;
                    if field_read_disq[reg] || !is_handle_reg(read.base) || !field_is_stable(read) {
                        None
                    } else {
                        derived.push(OsrDerivedLiveIn {
                            native_reg: reg,
                            base_reg: read.base,
                            field_slot: read.slot,
                            ty: k,
                        });
                        Some(k)
                    }
                }
                _ => None,
            })
            .collect();
        (flat, derived)
    };
    for reg in 0..n_regs {
        if let Some(kind) = flat_osr_kind[reg] {
            ty[reg] = Some(kind);
        }
    }
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct ScalarFieldKey {
        base: usize,
        slot: usize,
    }
    let handle_alias_for_scalar: Vec<usize> = {
        // Union-find (NOT propagation-to-fixpoint): a cyclic Handle-`Move` graph would
        // oscillate forever otherwise — see the matching note on the `handle_alias`
        // computation above. Same class grouping, but guaranteed to converge.
        let mut alias: Vec<usize> = (0..n_regs).collect();
        for i in lp.header..lp.exit {
            if let RegInstr::Move { dst, src } = &code[i] {
                if *dst < n_regs
                    && *src < n_regs
                    && ty[*dst] == Some(NativeTy::Handle)
                    && ty[*src] == Some(NativeTy::Handle)
                {
                    let rd = osr_uf_find(&mut alias, *dst);
                    let rs = osr_uf_find(&mut alias, *src);
                    if rd != rs {
                        let (lo, hi) = if rd < rs { (rd, rs) } else { (rs, rd) };
                        alias[hi] = lo;
                    }
                }
            }
        }
        for r in 0..n_regs {
            alias[r] = osr_uf_find(&mut alias, r);
        }
        alias
    };
    let mut scalar_keys: Vec<(ScalarFieldKey, bool)> = Vec::new();
    let mut scalar_disq: Vec<ScalarFieldKey> = Vec::new();
    for i in lp.header..lp.exit {
        match &code[i] {
            RegInstr::GetFieldSlot { dst, base, slot }
                if *dst < n_regs
                    && *base < n_regs
                    && matches!(ty[*dst], None | Some(NativeTy::Int)) =>
            {
                let key = ScalarFieldKey {
                    base: handle_alias_for_scalar[*base],
                    slot: *slot,
                };
                if ty[key.base] != Some(NativeTy::Handle) {
                    scalar_disq.push(key);
                } else if !scalar_keys.iter().any(|(existing, _)| *existing == key) {
                    scalar_keys.push((key, false));
                }
            }
            RegInstr::GetFieldSlot { dst, base, slot } if *base < n_regs => {
                if *dst < n_regs && ty[*dst] != Some(NativeTy::Int) {
                    scalar_disq.push(ScalarFieldKey {
                        base: handle_alias_for_scalar[*base],
                        slot: *slot,
                    });
                }
            }
            RegInstr::SetFieldSlot {
                base, slot, value, ..
            } if *base < n_regs => {
                let key = ScalarFieldKey {
                    base: handle_alias_for_scalar[*base],
                    slot: *slot,
                };
                if *value >= n_regs
                    || ty[key.base] != Some(NativeTy::Handle)
                    || ty[*value] != Some(NativeTy::Int)
                {
                    scalar_disq.push(key);
                } else if let Some((_, writeback)) = scalar_keys
                    .iter_mut()
                    .find(|(existing, _)| *existing == key)
                {
                    *writeback = true;
                } else {
                    scalar_keys.push((key, true));
                }
            }
            _ => {}
        }
    }
    scalar_keys.retain(|(key, _)| !scalar_disq.iter().any(|disq| disq == key));
    let scalar_fields: Vec<OsrScalarField> = scalar_keys
        .iter()
        .enumerate()
        .map(|(index, (key, writeback))| OsrScalarField {
            native_reg: n_regs + index,
            base_reg: key.base,
            field_slot: key.slot,
            writeback: *writeback,
        })
        .collect();
    let scalar_field_reg = |base: usize, slot: usize| {
        let canonical = handle_alias_for_scalar.get(base).copied().unwrap_or(base);
        scalar_fields.iter().find_map(|field| {
            (field.base_reg == canonical && field.field_slot == slot).then_some(field.native_reg)
        })
    };

    let int = |reg: usize| ty[reg] == Some(NativeTy::Int);
    let int_or_free = |reg: usize| matches!(ty[reg], None | Some(NativeTy::Int));
    let float = |reg: usize| ty[reg] == Some(NativeTy::Float);
    let bool_ty = |reg: usize| ty[reg] == Some(NativeTy::Bool);
    let numeric = |reg: usize| matches!(ty[reg], Some(NativeTy::Int | NativeTy::Float));
    let same = |a: usize, b: usize| ty[a].is_some() && ty[a] == ty[b];
    let numeric_pair_or_int_free =
        |a: usize, b: usize| (numeric(a) && same(a, b)) || (int_or_free(a) && int_or_free(b));
    // A TV2 flat-array register (pointer + length, read directly in-register). On the
    // OSR path a flat base may be any loop-invariant live-in register (not only a
    // param): it is marshalled into the `n_regs`-wide window by register index.
    let flat_reg = |reg: usize| {
        matches!(
            ty[reg],
            Some(
                NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut
            )
        )
    };
    let derived_livein = |reg: usize, _base: usize, slot: usize| {
        derived_liveins
            .iter()
            .any(|livein| livein.native_reg == reg && livein.field_slot == slot)
    };
    // A *native-readable* handle (Pending #1, stored-closure broadening): any Handle
    // register. A param/live-in handle is marshalled into the heap-arg window by
    // `try_osr` (phase 2); a loop-INTERNAL handle is produced by a `FieldHandle`/
    // `ListGetHandle` read (a stored struct/closure fetched as a fresh table index)
    // and is dead at the OSR boundary (the exit-restore skips Handle registers, so
    // its index bits never leak back into an interpreter slot). Sound because every
    // closure read goes through the runtime helper + identity guard/bail.
    let handle_reg = |reg: usize| ty[reg] == Some(NativeTy::Handle);
    let r = |reg: usize| reg as u32;
    let cmp = |op: &RegIntCompare| match op {
        RegIntCompare::Less => JitCompare::Lt,
        RegIntCompare::LessEqual => JitCompare::Le,
        RegIntCompare::Greater => JitCompare::Gt,
        RegIntCompare::GreaterEqual => JitCompare::Ge,
    };
    let mut string_literals: Vec<Rc<String>> = Vec::new();
    // O(1)-amortized interning, mirroring the whole-function translator: a content-keyed
    // `HashMap<Rc<String>, i64>` instead of an O(L) linear scan per literal (O(L²) total).
    let mut string_literal_ids: HashMap<Rc<String>, i64> = HashMap::new();
    let intern_string_literal = |literals: &mut Vec<Rc<String>>,
                                 ids: &mut HashMap<Rc<String>, i64>,
                                 value: &Rc<String>|
     -> Option<i64> {
        if let Some(index) = ids.get(value) {
            return Some(*index);
        }
        let index = i64::try_from(literals.len()).ok()?;
        literals.push(Rc::clone(value));
        ids.insert(Rc::clone(value), index);
        Some(index)
    };

    let mut jit_code: Vec<JitInstr> = Vec::with_capacity(n);
    for (i, instr) in code.iter().enumerate() {
        if i == lp.exit {
            // The loop's single exit edge: deopt back to the interpreter here with
            // the live-out window (precise-deopt resume at this ip).
            jit_code.push(JitInstr::OsrExit);
            continue;
        }
        if !in_loop(i) {
            // Outside the loop region: never reached natively under OSR. Keep an
            // index-aligned `Bail` so any unexpected arrival is a safe fallback.
            jit_code.push(JitInstr::Bail);
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
            RegInstr::LoadString { dst, value } => {
                let literal_id =
                    intern_string_literal(&mut string_literals, &mut string_literal_ids, value)?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::StringLiteral,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::ImmI64(literal_id)],
                }
            }
            RegInstr::Move { dst, src } => {
                ty[*src]?;
                JitInstr::Move {
                    dst: r(*dst),
                    src: r(*src),
                }
            }
            RegInstr::DeepCopy { .. } | RegInstr::DeepCopyElided { .. } => JitInstr::Nop,
            RegInstr::TailCallGuard => JitInstr::TailCallGuard {
                max_depth: u32::try_from(DEFAULT_MAX_DEPTH).ok()?,
            },
            RegInstr::AddInt { dst, lhs, rhs } => {
                require(numeric_pair_or_int_free(*lhs, *rhs))?;
                JitInstr::Add {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::SubInt { dst, lhs, rhs } => {
                require(numeric_pair_or_int_free(*lhs, *rhs))?;
                JitInstr::Sub {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::MulInt { dst, lhs, rhs } => {
                require(numeric_pair_or_int_free(*lhs, *rhs))?;
                JitInstr::Mul {
                    dst: r(*dst),
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                }
            }
            RegInstr::DivInt { dst, lhs, rhs } => {
                require(numeric_pair_or_int_free(*lhs, *rhs))?;
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
                require(numeric_pair_or_int_free(*lhs, *rhs))?;
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
            RegInstr::Jump { target } => {
                // Every in-region jump target was checked to stay in `[header, exit)`
                // by `detect_single_natural_loop`, and that range is all native here.
                JitInstr::Jump { target: r(*target) }
            }
            RegInstr::JumpIfBool {
                cond,
                expected,
                target,
            } => {
                require(bool_ty(*cond))?;
                if let Some(&hot_target) = profile_hot_branch_edges.get(&i) {
                    JitInstr::ProfiledJumpIfBool {
                        cond: r(*cond),
                        expected: *expected,
                        target: r(*target),
                        hot_target,
                    }
                } else {
                    JitInstr::JumpIfBool {
                        cond: r(*cond),
                        expected: *expected,
                        target: r(*target),
                    }
                }
            }
            RegInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            } => {
                require(numeric_pair_or_int_free(*lhs, *rhs))?;
                if let Some(&hot_target) = profile_hot_branch_edges.get(&i) {
                    JitInstr::ProfiledJumpIfIntCompare {
                        lhs: r(*lhs),
                        rhs: r(*rhs),
                        op: cmp(op),
                        expected: *expected,
                        target: r(*target),
                        hot_target,
                    }
                } else {
                    JitInstr::JumpIfIntCompare {
                        lhs: r(*lhs),
                        rhs: r(*rhs),
                        op: cmp(op),
                        expected: *expected,
                        target: r(*target),
                    }
                }
            }
            // A `Return` inside the loop was rejected by `detect_single_natural_loop`
            // (single-exit only), so we never reach one here. A `RuntimeError`
            // (e.g. the dynamically-dead exhaustive-match trap an `Option` match
            // lowers to) compiles to `Bail`: if ever reached natively it deopts to
            // the interpreter, which re-runs the loop and raises the error itself —
            // so OSR never has to model the trap's semantics.
            RegInstr::RuntimeError { .. } => JitInstr::Bail,
            RegInstr::GetFieldSlot { dst, base, slot } => {
                require(handle_reg(*base))?;
                if let Some(field_reg) = scalar_field_reg(*base, *slot) {
                    require(int_or_free(*dst))?;
                    JitInstr::Move {
                        dst: r(*dst),
                        src: r(field_reg),
                    }
                } else if derived_livein(*dst, *base, *slot) {
                    JitInstr::Nop
                } else if handle_reg(*dst) {
                    // A heap-valued field (a stored closure) fetched as a fresh
                    // handle so a downstream closure read can address it.
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::FieldHandle,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        ],
                    }
                } else if float(*dst) {
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::FieldFloat,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        ],
                    }
                } else {
                    require(int_or_free(*dst))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::FieldInt,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        ],
                    }
                }
            }
            RegInstr::SetFieldSlot {
                dst: _,
                base,
                slot,
                value,
            } => {
                require(handle_reg(*base))?;
                let value_is_scalar = int_or_free(*value) || float(*value);
                if value_is_scalar && scalar_field_reg(*base, *slot).is_some() {
                    // Scalar-replaced field: a plain register copy carries either an
                    // Int or a Float (the field register's class follows the value).
                    let field_reg = scalar_field_reg(*base, *slot).expect("just checked");
                    JitInstr::Move {
                        dst: r(field_reg),
                        src: r(*value),
                    }
                } else {
                    // Float → FieldSetFloat; Int/unconstrained → FieldSetInt; (J0.4 #1)
                    // heap value → FieldSetHandle.
                    let helper = if float(*value) {
                        vm_jit::HostHelper::FieldSetFloat
                    } else if int_or_free(*value) {
                        vm_jit::HostHelper::FieldSetInt
                    } else if handle_reg(*value) {
                        vm_jit::HostHelper::FieldSetHandle
                    } else {
                        require(false)?;
                        unreachable!()
                    };
                    JitInstr::HostCall {
                        helper,
                        dst: r(*base),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*base)),
                            vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                            vm_jit::HostArg::Reg(r(*value)),
                        ],
                    }
                }
            }
            RegInstr::ListLen { dst, list } => {
                require(int(*dst))?;
                // A flat-classified (loop-invariant typed) list reads its length
                // directly from the marshalled `lens` slot — hoisted to a single load,
                // no per-iteration host call.
                if flat_reg(*list) {
                    JitInstr::ListLenDirect {
                        dst: r(*dst),
                        base: r(*list),
                    }
                } else {
                    require(handle_reg(*list))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListLen,
                        dst: r(*dst),
                        args: vec![vm_jit::HostArg::Reg(r(*list))],
                    }
                }
            }
            RegInstr::ListGet { dst, list, index } => {
                require(int(*index))?;
                // TV2 flat (loop-invariant typed) list ⇒ bounds-checked direct read.
                if matches!(
                    ty[*list],
                    Some(NativeTy::FlatFloat | NativeTy::FlatFloatMut)
                ) {
                    require(float(*dst))?;
                    JitInstr::ListGetFloatDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                    }
                } else if matches!(ty[*list], Some(NativeTy::FlatInt | NativeTy::FlatIntMut)) {
                    require(int_or_free(*dst))?;
                    JitInstr::ListGetIntDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                    }
                } else if handle_reg(*dst) {
                    // A heap-valued element (a struct holding a stored closure)
                    // fetched as a fresh handle for a downstream field/closure read.
                    require(handle_reg(*list))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListGetHandle,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                        ],
                    }
                } else if float(*dst) {
                    require(handle_reg(*list))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListGetFloat,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                        ],
                    }
                } else {
                    require(handle_reg(*list) && int_or_free(*dst))?;
                    JitInstr::HostCall {
                        helper: vm_jit::HostHelper::ListGetInt,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                        ],
                    }
                }
            }
            RegInstr::ListSet {
                dst,
                list,
                index,
                value,
            } => {
                require(int(*index) && int(*dst) && (int_or_free(*value) || float(*value)))?;
                if ty[*list] == Some(NativeTy::FlatIntMut) {
                    require(int_or_free(*value))?;
                    JitInstr::ListSetIntDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                        value: r(*value),
                    }
                } else if ty[*list] == Some(NativeTy::FlatFloatMut) {
                    require(float(*value))?;
                    JitInstr::ListSetFloatDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                        value: r(*value),
                    }
                } else {
                    require(handle_reg(*list))?;
                    let helper = if float(*value) {
                        vm_jit::HostHelper::ListSetFloat
                    } else {
                        vm_jit::HostHelper::ListSetInt
                    };
                    JitInstr::HostCall {
                        helper,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*index)),
                            vm_jit::HostArg::Reg(r(*value)),
                        ],
                    }
                }
            }
            RegInstr::ListPush { dst, list, value } => {
                require(int(*dst) && (int_or_free(*value) || float(*value)))?;
                if flat_reg(*list) {
                    JitInstr::Bail
                } else {
                    require(handle_reg(*list))?;
                    let helper = if float(*value) {
                        vm_jit::HostHelper::ListPushFloat
                    } else {
                        vm_jit::HostHelper::ListPushInt
                    };
                    JitInstr::HostCall {
                        helper,
                        dst: r(*dst),
                        args: vec![
                            vm_jit::HostArg::Reg(r(*list)),
                            vm_jit::HostArg::Reg(r(*value)),
                        ],
                    }
                }
            }
            RegInstr::ListSort { dst, list } => {
                require(handle_reg(*list) && int(*dst))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ListSortInt,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*list))],
                }
            }
            RegInstr::MapInsert {
                dst,
                map,
                key,
                value,
            } => {
                require(handle_reg(*map) && int(*dst))?;
                // Three shapes: Int-key/Float-value, Int-key/Int-value, and (J0.4 #1)
                // heap-key (e.g. `String`)/Int-value. A heap key is resolved + hashed by
                // the host's own `VmMapKey` in the helper, never re-hashed in native.
                let helper = if int(*key) && float(*value) {
                    vm_jit::HostHelper::MapInsertFloat
                } else if int(*key) && int_or_free(*value) {
                    vm_jit::HostHelper::MapInsertInt
                } else if handle_reg(*key) && int(*value) {
                    vm_jit::HostHelper::MapInsertHandleKeyInt
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*map)),
                        vm_jit::HostArg::Reg(r(*key)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::SetInsert { dst, set, value } => {
                require(handle_reg(*set) && bool_ty(*dst))?;
                // Int value → SetInsertInt; (J0.4 #1) heap value (e.g. `String`) →
                // SetInsertHandle, which resolves + hashes the value via the host's key.
                let helper = if int(*value) {
                    vm_jit::HostHelper::SetInsertInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::SetInsertHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*set)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::SortedSetInsert { dst, set, value } => {
                require(handle_reg(*set) && bool_ty(*dst))?;
                // Int value → SortedSetInsertInt; (J0.4 #1) heap value → SortedSetInsertHandle.
                let helper = if int(*value) {
                    vm_jit::HostHelper::SortedSetInsertInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::SortedSetInsertHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*set)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::SortedMapInsert {
                dst,
                map,
                key,
                value,
            } => {
                require(handle_reg(*map) && int(*value) && int(*dst))?;
                // Int key → SortedMapInsertInt; (J0.4 #1) heap key → SortedMapInsertHandleKeyInt.
                let helper = if int(*key) {
                    vm_jit::HostHelper::SortedMapInsertInt
                } else if handle_reg(*key) {
                    vm_jit::HostHelper::SortedMapInsertHandleKeyInt
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*map)),
                        vm_jit::HostArg::Reg(r(*key)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::DequePushBack { dst, deque, value } => {
                require(handle_reg(*deque) && int(*dst))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::DequePushBackFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::DequePushBackInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::DequePushBackHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*deque)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::DequePushFront { dst, deque, value } => {
                require(handle_reg(*deque) && int(*dst))?;
                let helper = if float(*value) {
                    vm_jit::HostHelper::DequePushFrontFloat
                } else if int_or_free(*value) {
                    vm_jit::HostHelper::DequePushFrontInt
                } else if handle_reg(*value) {
                    vm_jit::HostHelper::DequePushFrontHandle
                } else {
                    require(false)?;
                    unreachable!()
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*deque)),
                        vm_jit::HostArg::Reg(r(*value)),
                    ],
                }
            }
            RegInstr::DequePopFront { dst, deque } => {
                require(handle_reg(*deque) && (int_or_free(*dst) || float(*dst)))?;
                let helper = if float(*dst) {
                    vm_jit::HostHelper::DequePopFrontFloat
                } else {
                    vm_jit::HostHelper::DequePopFrontInt
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*deque))],
                }
            }
            RegInstr::DequePopBack { dst, deque } => {
                require(handle_reg(*deque) && (int_or_free(*dst) || float(*dst)))?;
                let helper = if float(*dst) {
                    vm_jit::HostHelper::DequePopBackFloat
                } else {
                    vm_jit::HostHelper::DequePopBackInt
                };
                JitInstr::HostCall {
                    helper,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*deque))],
                }
            }
            RegInstr::MatchMapGet {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require(
                    handle_reg(*map) && int(*key) && (int_or_free(*value_dst) || float(*value_dst)),
                )?;
                if float(*value_dst) {
                    JitInstr::MatchMapGetFloat {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                } else {
                    JitInstr::MatchMapGetInt {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                }
            }
            RegInstr::MatchSortedMapGet {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require(
                    handle_reg(*map) && int(*key) && (int_or_free(*value_dst) || float(*value_dst)),
                )?;
                if float(*value_dst) {
                    JitInstr::MatchSortedMapGetFloat {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                } else {
                    JitInstr::MatchSortedMapGetInt {
                        map: r(*map),
                        key: r(*key),
                        value_dst: r(*value_dst),
                        some_ip: r(*some_ip),
                        none_ip: r(*none_ip),
                    }
                }
            }
            RegInstr::NativeGuardClosureId { closure, expected } => {
                require(handle_reg(*closure))?;
                let expected = i64::try_from(*expected).ok()?;
                JitInstr::GuardClosureId {
                    base: r(*closure),
                    expected,
                }
            }
            RegInstr::NativeClosureId { dst, closure } => {
                require(handle_reg(*closure) && int(*dst))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ClosureId,
                    dst: r(*dst),
                    args: vec![vm_jit::HostArg::Reg(r(*closure))],
                }
            }
            RegInstr::NativeFieldClosureId { dst, base, slot } => {
                require(handle_reg(*base) && int(*dst))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::FieldClosureId,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*base)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                    ],
                }
            }
            RegInstr::NativeClosureCapture {
                dst,
                closure,
                index,
            } => {
                // The capture's `dst` may be Int/Bool (i64 slot used directly) or
                // Float (the i64 slot is `f64::to_bits`, bit-reinterpreted to f64
                // in codegen). An unconstrained `dst` defaults to Int. A non-scalar
                // `dst` (Handle/flat array) cannot hold a scalar capture ⇒ bail.
                require(
                    handle_reg(*closure) && (int_or_free(*dst) || bool_ty(*dst) || float(*dst)),
                )?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::ClosureCapture,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*closure)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*index).ok()?),
                    ],
                }
            }
            RegInstr::NativeFieldClosureCapture {
                dst,
                base,
                slot,
                index,
            } => {
                require(handle_reg(*base) && (int_or_free(*dst) || bool_ty(*dst) || float(*dst)))?;
                JitInstr::HostCall {
                    helper: vm_jit::HostHelper::FieldClosureCapture,
                    dst: r(*dst),
                    args: vec![
                        vm_jit::HostArg::Reg(r(*base)),
                        vm_jit::HostArg::ImmI64(i64::try_from(*slot).ok()?),
                        vm_jit::HostArg::ImmI64(i64::try_from(*index).ok()?),
                    ],
                }
            }
            // `Int.to_float`: signed-int→f64 conversion (`fcvt_from_sint`). src is
            // an Int (i64), dst a Float (f64). Identical to the interpreter's
            // `i as f64` (a value-preserving conversion, not a bitcast).
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::IntToFloat,
                args,
                dst,
            } => {
                require(int(args[0]) && float(*dst))?;
                JitInstr::IntToFloat {
                    dst: r(*dst),
                    src: r(args[0]),
                }
            }
            // `Math.floor`/`Math.ceil`: round the f64 arg then a saturating f64→i64
            // cast, identical to the interpreter's `f.floor()/.ceil() as i64`. src is
            // a Float (f64), dst an Int (i64).
            RegInstr::CallIntrinsic {
                intrinsic: rounding_intrinsic @ (RegIntrinsic::MathFloor | RegIntrinsic::MathCeil),
                args,
                dst,
            } => {
                require(float(args[0]) && int(*dst))?;
                let rounding = match rounding_intrinsic {
                    RegIntrinsic::MathFloor => vm_jit::FloatRounding::Floor,
                    _ => vm_jit::FloatRounding::Ceil,
                };
                JitInstr::FloatToInt {
                    dst: r(*dst),
                    src: r(args[0]),
                    rounding,
                }
            }
            // `List.is_empty` on a flat-classified list: direct `len == 0` from the
            // marshalled `lens` slot, no per-iteration host call. Non-flat receivers
            // fall through to the host-helper path below.
            RegInstr::CallIntrinsic {
                intrinsic: RegIntrinsic::ListIsEmpty,
                args,
                dst,
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic: RegIntrinsic::ListIsEmpty,
                args,
                dst,
                ..
            } if args.len() == 1 && flat_reg(args[0]) => {
                require(bool_ty(*dst))?;
                JitInstr::ListIsEmptyDirect {
                    dst: r(*dst),
                    base: r(args[0]),
                }
            }
            RegInstr::CallIntrinsic {
                intrinsic,
                args,
                dst,
            } if native_host_intrinsic(*intrinsic).is_some() => {
                let spec = native_host_intrinsic(*intrinsic)?;
                require(args.len() == spec.arg_tys().len())?;
                for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                    require(match expected {
                        NativeTy::Int => int(*arg),
                        NativeTy::Bool => bool_ty(*arg),
                        NativeTy::Float => float(*arg),
                        NativeTy::Handle => handle_reg(*arg),
                        NativeTy::FlatInt
                        | NativeTy::FlatIntMut
                        | NativeTy::FlatFloat
                        | NativeTy::FlatFloatMut => false,
                    })?;
                }
                require(match spec.result_ty {
                    NativeTy::Int => int(*dst),
                    NativeTy::Bool => bool_ty(*dst),
                    NativeTy::Float => float(*dst),
                    NativeTy::Handle => handle_reg(*dst),
                    NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut => false,
                })?;
                JitInstr::HostCall {
                    helper: spec.helper,
                    dst: r(*dst),
                    args: args
                        .iter()
                        .map(|arg| vm_jit::HostArg::Reg(r(*arg)))
                        .collect(),
                }
            }
            RegInstr::CallTypedIntrinsic {
                intrinsic,
                type_arg,
                args,
                dst,
            } if native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str())).is_some() => {
                let spec = native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))?;
                require(args.len() == spec.arg_tys().len())?;
                for (arg, expected) in args.iter().zip(spec.arg_tys()) {
                    require(match expected {
                        NativeTy::Int => int(*arg),
                        NativeTy::Bool => bool_ty(*arg),
                        NativeTy::Float => float(*arg),
                        NativeTy::Handle => handle_reg(*arg),
                        NativeTy::FlatInt
                        | NativeTy::FlatIntMut
                        | NativeTy::FlatFloat
                        | NativeTy::FlatFloatMut => false,
                    })?;
                }
                require(match spec.result_ty {
                    NativeTy::Int => int(*dst),
                    NativeTy::Bool => bool_ty(*dst),
                    NativeTy::Float => float(*dst),
                    NativeTy::Handle => handle_reg(*dst),
                    NativeTy::FlatInt
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloat
                    | NativeTy::FlatFloatMut => false,
                })?;
                JitInstr::HostCall {
                    helper: spec.helper,
                    dst: r(*dst),
                    args: args
                        .iter()
                        .map(|arg| vm_jit::HostArg::Reg(r(*arg)))
                        .collect(),
                }
            }
            // Any other (non-subset) instruction in-region was already rejected.
            _ => return None,
        };
        jit_code.push(jit);
    }

    // Native type per register, then the JIT-class projection for codegen.
    let mut native_reg_types: Vec<NativeTy> = (0..n_regs)
        .map(|reg| ty[reg].unwrap_or(NativeTy::Int))
        .collect();
    for _ in &scalar_fields {
        native_reg_types.push(NativeTy::Int);
    }
    let reachable = native_reachable_instructions(code);
    let memo_scopes = native_memoize_loop_invariant_host_calls(
        code,
        &reachable,
        &mut jit_code,
        &native_reg_types,
        n_params,
    );
    native_forward_direct_list_store_loads(&mut jit_code);
    let mut written_regs = vec![false; native_reg_types.len()];
    for (ip, instr) in jit_code.iter().enumerate() {
        if in_loop(ip)
            && let Some(dst) = native_jit_written_reg(instr)
            && let Some(slot) = written_regs.get_mut(dst as usize)
        {
            *slot = true;
        }
    }
    let reg_types = native_reg_types
        .iter()
        .map(|t| t.jit_value_type())
        .collect();
    // Param types so the caller can marshal handle params (List/struct) in the
    // window; scalar live-in regs marshal directly by `reg_types`.
    let param_types: Vec<NativeTy> = native_reg_types[..n_params].to_vec();

    let jit_fn = vm_jit::JitFunction {
        n_params: n_params as u32,
        n_regs: native_reg_types.len() as u32,
        reg_types,
        zero_init_regs: Vec::new(),
        code: jit_code,
        memo_scopes,
        cold_blocks,
    };
    Some((
        jit_fn,
        param_types,
        derived_liveins,
        scalar_fields,
        native_reg_types,
        written_regs,
        string_literals,
    ))
}

#[cfg(all(test, feature = "native-jit"))]
mod direct_store_forwarding_tests {
    use super::*;

    fn int_store(base: u32, index: u32, value: u32) -> vm_jit::JitInstr {
        vm_jit::JitInstr::ListSetIntDirect {
            dst: 30,
            base,
            index,
            value,
        }
    }

    fn int_load(dst: u32, base: u32, index: u32) -> vm_jit::JitInstr {
        vm_jit::JitInstr::ListGetIntDirect { dst, base, index }
    }

    fn assert_move(instr: &vm_jit::JitInstr, expected_dst: u32, expected_src: u32) {
        assert!(matches!(
            instr,
            vm_jit::JitInstr::Move { dst, src }
                if *dst == expected_dst && *src == expected_src
        ));
    }

    fn assert_int_load(instr: &vm_jit::JitInstr, expected_base: u32, expected_index: u32) {
        assert!(matches!(
            instr,
            vm_jit::JitInstr::ListGetIntDirect { base, index, .. }
                if *base == expected_base && *index == expected_index
        ));
    }

    #[test]
    fn forwards_int_and_float_stores_across_scalar_instructions() {
        let mut code = vec![
            int_store(0, 1, 2),
            vm_jit::JitInstr::Add {
                dst: 10,
                lhs: 11,
                rhs: 12,
            },
            int_load(3, 0, 1),
            vm_jit::JitInstr::ListSetFloatDirect {
                dst: 31,
                base: 4,
                index: 5,
                value: 6,
            },
            vm_jit::JitInstr::ListLenDirect { dst: 13, base: 4 },
            vm_jit::JitInstr::ListGetFloatDirect {
                dst: 7,
                base: 4,
                index: 5,
            },
        ];

        native_forward_direct_list_store_loads(&mut code);

        assert_eq!(code.len(), 6);
        assert_move(&code[2], 3, 2);
        assert_move(&code[5], 7, 6);
    }

    #[test]
    fn operand_clobbers_kill_available_store() {
        for clobbered in [0, 1, 2] {
            let mut code = vec![
                int_store(0, 1, 2),
                vm_jit::JitInstr::LoadInt {
                    dst: clobbered,
                    value: 99,
                },
                int_load(3, 0, 1),
            ];

            native_forward_direct_list_store_loads(&mut code);

            assert_int_load(&code[2], 0, 1);
        }
    }

    #[test]
    fn compatible_store_with_different_base_kills_available_store() {
        let mut code = vec![int_store(0, 1, 2), int_store(4, 5, 6), int_load(3, 0, 1)];

        native_forward_direct_list_store_loads(&mut code);

        assert_int_load(&code[2], 0, 1);
    }

    #[test]
    fn calls_and_unknown_heap_effects_kill_available_store() {
        let barriers = [
            vm_jit::JitInstr::HostCall {
                helper: vm_jit::HostHelper::StringLen,
                dst: 19,
                args: vec![vm_jit::HostArg::Reg(18)],
            },
            vm_jit::JitInstr::CallSelf {
                dst: 20,
                args: vec![],
            },
            vm_jit::JitInstr::GuardClosureId {
                base: 21,
                expected: 1,
            },
        ];
        for barrier in barriers {
            let mut code = vec![int_store(0, 1, 2), barrier, int_load(3, 0, 1)];

            native_forward_direct_list_store_loads(&mut code);

            assert_int_load(&code[2], 0, 1);
        }
    }

    #[test]
    fn branch_target_starts_without_linear_predecessor_facts() {
        let mut code = vec![
            vm_jit::JitInstr::Jump { target: 3 },
            int_store(0, 1, 2),
            vm_jit::JitInstr::LoadInt { dst: 10, value: 0 },
            int_load(3, 0, 1),
        ];

        native_forward_direct_list_store_loads(&mut code);

        assert_int_load(&code[3], 0, 1);
    }
}
