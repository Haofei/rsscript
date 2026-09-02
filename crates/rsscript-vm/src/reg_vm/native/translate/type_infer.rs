//! `native_infer_types` — the whole-function native type-inference fixed point,
//! extracted from translate_to_native_jit_with_calls for module-size partitioning.

use super::*;

/// Read-only inputs to [`native_infer_types`], bundled so the fixed point is a
/// single-argument call (the mutable `ty` window is passed separately).
pub(super) struct TypeInferInputs<'a> {
    pub(super) code: &'a [RegInstr],
    pub(super) func: &'a RegFunction,
    pub(super) facts: &'a VerifiedFunctionFacts,
    pub(super) ip_map: &'a [usize],
    pub(super) reachable: &'a [bool],
    pub(super) declared_return_ty: Option<NativeTy>,
    pub(super) compiled_callees: &'a HashMap<usize, NativeCompiledCallee>,
    pub(super) self_call_sites: &'a HashSet<usize>,
    pub(super) group_call_sites: &'a HashMap<usize, u32>,
}

pub(super) fn native_infer_types(
    inputs: TypeInferInputs<'_>,
    ty: &mut Vec<Option<NativeTy>>,
) -> Option<()> {
    let TypeInferInputs {
        code,
        func,
        facts,
        ip_map,
        reachable,
        declared_return_ty,
        compiled_callees,
        self_call_sites,
        group_call_sites,
    } = inputs;
    let mut changed = true;
    while changed {
        changed = false;
        for (i, instr) in code.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            let ty = &mut *ty;
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
                    // this function, so its args and result use the same verified
                    // call-site signature consumed by the group-call arm. A
                    // Bool/Float-typed self-recursive function thus types its self-call
                    // correctly instead of being forced to `Int`. Types erased by
                    // v1 still flow from their other uses via unification.
                    let call = facts.call_site(ip_map[i]);
                    let mut ok = mut_args.is_empty() && args.len() == func.params;
                    for (pi, arg) in args.iter().enumerate() {
                        if let Some(pty) = call
                            .and_then(|call| call.params.get(pi))
                            .and_then(|ty| ty.native_ty())
                        {
                            ok = ok && native_set_ty(ty, *arg, pty, c);
                        }
                    }
                    if let Some(ret) = call.and_then(|call| call.result.native_ty()) {
                        ok = ok && native_set_ty(ty, *dst, ret, c);
                    }
                    ok
                }
                RegInstr::CallKnown {
                    dst,
                    function: _,
                    args,
                    mut_args,
                } if group_call_sites.contains_key(&ip_map[i]) => {
                    // Mutually-recursive group call (native-call-ABI slice 4): args and
                    // result take the *callee* member's verified call-site types,
                    // so a Bool-returning member (e.g. `is_even`/`is_odd`) types its
                    // result `Bool` rather than being forced to `Int`.
                    let call = facts.call_site(ip_map[i]);
                    let mut ok = mut_args.is_empty();
                    for (pi, arg) in args.iter().enumerate() {
                        if let Some(pty) = call
                            .and_then(|call| call.params.get(pi))
                            .and_then(|ty| ty.native_ty())
                        {
                            ok = ok && native_set_ty(ty, *arg, pty, c);
                        }
                    }
                    if let Some(ret) = call.and_then(|call| call.result.native_ty()) {
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
                // polymorphic inline cache dispatch: the closure operand is a native handle; the read
                // function id is an `Int` (consumed by integer compares/branches).
                RegInstr::NativeClosureId { dst, closure } => {
                    native_set_ty(ty, *closure, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::NativeFieldClosureId { dst, base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                // Capturing-closure inline (OSR × profile-guided inlining): the closure operand is a
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
    Some(())
}
