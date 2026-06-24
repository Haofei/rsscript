//! Native-JIT IR producers and OSR-loop detection. Pure code-movement out of
//! `reg_vm::mod` (Phase 2); every item retains its original
//! `#[cfg(feature = "native-jit")]` attribute verbatim.
#![allow(unused_imports)]

use super::super::*;
use super::*;

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
) -> Option<(vm_jit::JitFunction, NativeTy, Vec<NativeTy>)> {
    use vm_jit::{JitCompare, JitInstr};

    if func.captures != 0 {
        return None;
    }
    // Inline straight-line leaf calls first, so a function that only leaves the
    // native subset via small helper calls still qualifies (the calls vanish).
    // Whole-function native translation: the ENTIRE body runs natively, so every
    // call must be inlinable (`None` region ⇒ whole function in-scope).
    let (code, n_regs, _ip_map) = native_inline_leaf_calls(unit, func, false, None)?;
    // J3: then scalar-replace any non-escaping (scalar-payload) `Option` on the
    // fully-inlined body, dissolving `MakeSome`/`LoadNone`/`MatchOption`/`UnwrapSome`
    // into tag + payload scalar registers so the function compiles through the
    // native subset with no allocation. Run AFTER inlining so it rewrites the final
    // jump layout (and can dissolve Options exposed by an inlined callee). `None`
    // here means an Option escapes ⇒ leave the whole function on the interpreter
    // path (its bail/fallback re-runs from the top and reconstructs the Option).
    let (code, n_regs, scalar_payload_regs, _ip_map) =
        native_scalar_replace_options(&code, n_regs)?;
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
                // Heap reads: the base is a handle, the list index an Int. The
                // read *result* (`dst`) is left to flow from its uses — an `Int`
                // result picks the Int helper, a `Float` result the Float helper.
                // We do not force `dst` to `Int` here (that would reject float
                // reads); lowering admits only a provably-Int-or-Float `dst` and
                // bails otherwise.
                RegInstr::GetFieldSlot { dst: _, base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
                }
                RegInstr::ListLen { dst, list } => {
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *dst, NativeTy::Int, c)
                }
                RegInstr::ListGet { dst: _, list, index } => {
                    native_set_ty(ty, *list, NativeTy::Handle, c)
                        && native_set_ty(ty, *index, NativeTy::Int, c)
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
                // Capturing-closure inline (OSR × J2): the closure operand is a
                // native handle; the materialized capture `dst` carries the
                // capture's scalar bits (the `closure_capture` helper returns the
                // raw i64 bit pattern: an `Int` directly, a `Bool` as 0/1, a
                // `Float` reinterpreted via `f64::to_bits`). Leave `dst`'s class to
                // flow from its uses — exactly like a `GetFieldSlot` read — so an
                // Int/Bool capture stays Int-class and a Float capture becomes
                // Float-class. Lowering admits only a provably Int/Bool/Float `dst`
                // (and the Float arm bit-reinterprets the i64 slot to f64).
                RegInstr::NativeClosureCapture { dst: _, closure, .. } => {
                    native_set_ty(ty, *closure, NativeTy::Handle, c)
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
                _ => true,
            };
            if !ok {
                return None; // conflicting register types
            }
        }
    }

    // TV2 flat-array classification. A `Handle` *parameter* whose every use is a
    // list read (`ListGet`/`ListLen`, never a `GetFieldSlot` struct read) with a
    // *consistent* element kind is reclassified `FlatInt`/`FlatFloat`, so its
    // `ListGet`/`ListLen` lower to direct in-register loads (no per-element host
    // call). Mixed-kind reads, or a handle also used as a struct base, stay
    // `Handle` (the helper path). Marshalling (`try_native`) honors the chosen kind
    // at call time and falls back if the runtime list isn't the flat kind.
    let flat_param_kind: Vec<Option<NativeTy>> = {
        #[derive(Clone, Copy, PartialEq)]
        enum S {
            Unseen,
            Flat(NativeTy),
            Disq,
        }
        let mut st = vec![S::Unseen; n_regs];
        let is_handle_param =
            |reg: usize| ty[reg] == Some(NativeTy::Handle) && reg < func.params;
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

    // J3: a scalar-replaced Option's payload register must be a SCALAR (Int/Float/
    // Bool, or fully unconstrained — defaults to Int). If inference proved it a
    // `Handle`/flat array, the Some payload was a heap value, so the Option was not
    // truly scalar-replaceable ⇒ bail and leave the function on the interpreter
    // path. (Conservative: any doubt ⇒ don't scalar-replace.)
    for &payload in &scalar_payload_regs {
        if matches!(
            ty[payload],
            Some(NativeTy::Handle | NativeTy::FlatInt | NativeTy::FlatFloat)
        ) {
            return None;
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
    // A handle register must be a *parameter*: handles only enter via the caller's
    // heap args (`try_native`), never produced by a native instruction.
    let handle_param = |reg: usize| ty[reg] == Some(NativeTy::Handle) && reg < func.params;
    // A *native-readable* handle (Pending #1): any Handle register — a param/live-in
    // (marshalled into the heap-arg window) or a loop-internal handle produced by a
    // `FieldHandle`/`ListGetHandle` read (a stored struct/closure fetched as a fresh
    // table index). Used by the heap reads and closure ops, whose runtime helper +
    // identity guard/bail make a wrong handle sound (re-run from the top).
    let handle_reg = |reg: usize| ty[reg] == Some(NativeTy::Handle);
    // A TV2 flat-array param (pointer + length, read directly in-register).
    let flat_param = |reg: usize| {
        matches!(ty[reg], Some(NativeTy::FlatInt | NativeTy::FlatFloat)) && reg < func.params
    };
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
            RegInstr::DeepCopy { .. } => {
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
                // function's return type. A scalar (`Int`/`Float`) return is the
                // unchanged path. Heap-result return ABI (heap-write S0): also accept
                // returning a heap **parameter** unchanged (`handle_param`) — a pure
                // pass-through with NO allocation and NO mutation. The returned i64 is
                // the input heap-arg table index; the host materializes the result
                // from its VM-owned output table on a clean completion only, so §7.2's
                // no-effect-before-bail proof is unaffected. (Returning a Handle
                // *produced* by a native read — `FieldHandle`/`ListGetHandle` — is NOT
                // accepted here: those table indices are call-scoped scratch, not a
                // re-materializable value. That is a later slice.)
                require(numeric(*src) || handle_param(*src))?;
                JitInstr::Return { src: r(*src) }
            }
            RegInstr::RuntimeError { .. } => JitInstr::Bail,
            RegInstr::GetFieldSlot { dst, base, slot } => {
                require(handle_reg(*base))?;
                if handle_reg(*dst) {
                    // A heap-valued field (a stored closure) fetched as a fresh
                    // handle so a downstream closure read can address it.
                    JitInstr::FieldHandle {
                        dst: r(*dst),
                        base: r(*base),
                        slot: *slot as u32,
                    }
                } else if float(*dst) {
                    JitInstr::FieldFloat {
                        dst: r(*dst),
                        base: r(*base),
                        slot: *slot as u32,
                    }
                } else {
                    // Int or unconstrained → Int helper (a non-Int field then
                    // bails at the helper). A Bool dst is rejected here.
                    require(int_or_free(*dst))?;
                    JitInstr::FieldInt {
                        dst: r(*dst),
                        base: r(*base),
                        slot: *slot as u32,
                    }
                }
            }
            RegInstr::ListLen { dst, list } => {
                require(int(*dst))?;
                if flat_param(*list) {
                    JitInstr::ListLenDirect {
                        dst: r(*dst),
                        base: r(*list),
                    }
                } else {
                    require(handle_param(*list))?;
                    JitInstr::ListLen {
                        dst: r(*dst),
                        base: r(*list),
                    }
                }
            }
            RegInstr::ListGet { dst, list, index } => {
                require(int(*index))?;
                // TV2 flat params lower to direct in-register reads; the flat kind
                // (set by classification) matches the dst kind by construction.
                if ty[*list] == Some(NativeTy::FlatFloat) {
                    require(float(*dst))?;
                    JitInstr::ListGetFloatDirect {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                    }
                } else if ty[*list] == Some(NativeTy::FlatInt) {
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
                    JitInstr::ListGetHandle {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                    }
                } else if float(*dst) {
                    require(handle_reg(*list))?;
                    JitInstr::ListGetFloat {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
                    }
                } else {
                    require(handle_reg(*list) && int_or_free(*dst))?;
                    JitInstr::ListGetInt {
                        dst: r(*dst),
                        base: r(*list),
                        index: r(*index),
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
                JitInstr::ClosureId {
                    dst: r(*dst),
                    base: r(*closure),
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
                    handle_reg(*closure)
                        && (int_or_free(*dst) || bool_ty(*dst) || float(*dst)),
                )?;
                let index = i64::try_from(*index).ok()?;
                let index = u32::try_from(index).ok()?;
                JitInstr::ClosureCapture {
                    dst: r(*dst),
                    base: r(*closure),
                    index,
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
    /// Per-register native types of the compiled OSR body. Used at OSR-exit to skip
    /// restoring **Handle**-class registers: a loop-internal handle (a stored
    /// struct/closure fetched via `FieldHandle`/`ListGetHandle`) is dead at the exit
    /// and its live-out "value" is only a heap-table index — restoring it as an Int
    /// into the interpreter slot would corrupt the register. The interpreter re-
    /// derives any still-needed heap value; a dead one is simply never read.
    pub(in crate::reg_vm) reg_types: Vec<NativeTy>,
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
    // Collect backedges: a jump/branch whose target is at or before it.
    let mut backedges: Vec<(usize, usize)> = Vec::new(); // (from, header)
    for (i, instr) in code.iter().enumerate() {
        // `MatchOption` is a two-way control transfer (some_ip / none_ip); count a
        // backedge for either target that jumps backward. This matters when this
        // runs on UNTRANSFORMED code (OSR × J3, before scalar replacement): a
        // forward in-loop `match` must not be mistaken for straight-line code.
        if let RegInstr::MatchOption { some_ip, none_ip, .. } = instr {
            if *some_ip <= i {
                backedges.push((i, *some_ip));
            }
            if *none_ip <= i {
                backedges.push((i, *none_ip));
            }
            continue;
        }
        let target = match instr {
            RegInstr::Jump { target } => Some(*target),
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                Some(*target)
            }
            _ => None,
        };
        if let Some(t) = target {
            if t <= i {
                backedges.push((i, t));
            }
        }
    }
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
            RegInstr::MatchOption { some_ip, none_ip, .. } => {
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
            RegInstr::MatchOption { some_ip, none_ip, .. } => {
                enters_interior(*some_ip) || enters_interior(*none_ip)
            }
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
pub(in crate::reg_vm) fn translate_osr_loop(
    code: &[RegInstr],
    n_regs: usize,
    n_params: usize,
    captures: usize,
    lp: OsrLoop,
) -> Option<(vm_jit::JitFunction, Vec<NativeTy>, Vec<NativeTy>)> {
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
                RegInstr::GetFieldSlot { base, .. } => {
                    native_set_ty(ty, *base, NativeTy::Handle, c)
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
                RegInstr::NativeClosureCapture { dst: _, closure, .. } => {
                    native_set_ty(ty, *closure, NativeTy::Handle, c)
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
                _ => true,
            };
            if !ok {
                return None;
            }
        }
    }

    // TV2 flat-array classification on the OSR path. A `Handle` register that is
    // (a) **loop-invariant** — never the `dst` of any in-loop instruction, so the
    // list value it holds is fixed for the loop's whole native run (and, because the
    // native subset has NO list-mutating instruction — no `ListSet`/`ListPush` — a
    // list whose register is invariant cannot have its contents mutated in-loop
    // either; any such mutation would have left the subset and declined OSR), and
    // (b) read in-loop ONLY via `ListGet`/`ListLen` with a *consistent* element kind
    // (never a `GetFieldSlot` struct base, never a heap-element `ListGetHandle` /
    // closure read), is reclassified `FlatInt`/`FlatFloat`. Its `ListGet`/`ListLen`
    // then lower to bounds-checked direct in-register loads (no per-iteration host
    // call) and `try_osr` marshals it into the live-in window as a borrow-pinned flat
    // buffer (pointer + length). Anything ambiguous (mixed-kind, mutated/rewritten,
    // struct-aliased, heap element) BAILS to the `Handle` host-helper path, which is
    // always correct. Read-only (§7.2 holds; OOB still bails).
    let flat_osr_kind: Vec<Option<NativeTy>> = {
        #[derive(Clone, Copy, PartialEq)]
        enum S {
            Unseen,
            Flat(NativeTy),
            Disq,
        }
        // A register written by ANY in-loop instruction is not loop-invariant.
        let mut written_in_loop = vec![false; n_regs];
        for i in lp.header..lp.exit {
            if let Some(dst) = native_subset_dst(&code[i]) {
                if dst < n_regs {
                    written_in_loop[dst] = true;
                }
            }
        }
        let is_handle_reg = |reg: usize| ty[reg] == Some(NativeTy::Handle);
        let mut st = vec![S::Unseen; n_regs];
        for i in lp.header..lp.exit {
            match &code[i] {
                // A struct read disqualifies the base (it is not a flat list).
                RegInstr::GetFieldSlot { base, .. } if is_handle_reg(*base) => {
                    st[*base] = S::Disq;
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
                            _ => S::Disq,
                        };
                    }
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
        (0..n_regs)
            .map(|reg| match st[reg] {
                // Only a loop-invariant (never-rewritten) handle qualifies. A handle
                // produced INSIDE the loop (ListGetHandle/FieldHandle dst) is written
                // in-loop ⇒ excluded here, staying on the Handle path.
                S::Flat(k) if !written_in_loop[reg] => Some(k),
                _ => None,
            })
            .collect()
    };
    for reg in 0..n_regs {
        if let Some(kind) = flat_osr_kind[reg] {
            ty[reg] = Some(kind);
        }
    }

    let int = |reg: usize| ty[reg] == Some(NativeTy::Int);
    let int_or_free = |reg: usize| matches!(ty[reg], None | Some(NativeTy::Int));
    let float = |reg: usize| ty[reg] == Some(NativeTy::Float);
    let bool_ty = |reg: usize| ty[reg] == Some(NativeTy::Bool);
    let numeric = |reg: usize| matches!(ty[reg], Some(NativeTy::Int | NativeTy::Float));
    let same = |a: usize, b: usize| ty[a].is_some() && ty[a] == ty[b];
    // A TV2 flat-array register (pointer + length, read directly in-register). On the
    // OSR path a flat base may be any loop-invariant live-in register (not only a
    // param): it is marshalled into the `n_regs`-wide window by register index.
    let flat_reg = |reg: usize| {
        matches!(ty[reg], Some(NativeTy::FlatInt | NativeTy::FlatFloat))
    };
    // Handles only enter via the caller's heap-arg window; for OSR the window
    // carries whatever the interpreter has, so a Handle base must be a *parameter*
    // register (consistent with `translate_to_native_jit`, and marshalled the same).
    let handle_param = |reg: usize| ty[reg] == Some(NativeTy::Handle) && reg < n_params;
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
            RegInstr::LoadInt { dst, value } => JitInstr::LoadInt { dst: r(*dst), value: *value },
            RegInstr::LoadFloat { dst, value } => JitInstr::LoadFloat { dst: r(*dst), value: *value },
            RegInstr::LoadBool { dst, value } => JitInstr::LoadBool { dst: r(*dst), value: *value },
            RegInstr::Move { dst, src } => {
                ty[*src]?;
                JitInstr::Move { dst: r(*dst), src: r(*src) }
            }
            RegInstr::DeepCopy { .. } => JitInstr::Nop,
            RegInstr::AddInt { dst, lhs, rhs } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::Add { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::SubInt { dst, lhs, rhs } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::Sub { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::MulInt { dst, lhs, rhs } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::Mul { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::DivInt { dst, lhs, rhs } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::Div { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::ModInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::Mod { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::BitAndInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::BitAnd { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::BitOrInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::BitOr { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::BitXorInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::BitXor { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::ShiftLeftInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::Shl { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::ShiftRightInt { dst, lhs, rhs } => {
                require(int(*lhs) && int(*rhs))?;
                JitInstr::Shr { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
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
                JitInstr::Compare { dst: r(*dst), op, lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::Equal { dst, lhs, rhs } => {
                require(same(*lhs, *rhs))?;
                JitInstr::Equal { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::NotEqual { dst, lhs, rhs } => {
                require(same(*lhs, *rhs))?;
                JitInstr::NotEqual { dst: r(*dst), lhs: r(*lhs), rhs: r(*rhs) }
            }
            RegInstr::Jump { target } => {
                // Every in-region jump target was checked to stay in `[header, exit)`
                // by `detect_single_natural_loop`, and that range is all native here.
                JitInstr::Jump { target: r(*target) }
            }
            RegInstr::JumpIfBool { cond, expected, target } => {
                require(bool_ty(*cond))?;
                JitInstr::JumpIfBool { cond: r(*cond), expected: *expected, target: r(*target) }
            }
            RegInstr::JumpIfIntCompare { lhs, rhs, op, expected, target } => {
                require(numeric(*lhs) && same(*lhs, *rhs))?;
                JitInstr::JumpIfIntCompare {
                    lhs: r(*lhs),
                    rhs: r(*rhs),
                    op: cmp(op),
                    expected: *expected,
                    target: r(*target),
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
                if handle_reg(*dst) {
                    // A heap-valued field (a stored closure) fetched as a fresh
                    // handle so a downstream closure read can address it.
                    JitInstr::FieldHandle { dst: r(*dst), base: r(*base), slot: *slot as u32 }
                } else if float(*dst) {
                    JitInstr::FieldFloat { dst: r(*dst), base: r(*base), slot: *slot as u32 }
                } else {
                    require(int_or_free(*dst))?;
                    JitInstr::FieldInt { dst: r(*dst), base: r(*base), slot: *slot as u32 }
                }
            }
            RegInstr::ListLen { dst, list } => {
                require(int(*dst))?;
                // A flat-classified (loop-invariant typed) list reads its length
                // directly from the marshalled `lens` slot — hoisted to a single load,
                // no per-iteration host call.
                if flat_reg(*list) {
                    JitInstr::ListLenDirect { dst: r(*dst), base: r(*list) }
                } else {
                    require(handle_reg(*list))?;
                    JitInstr::ListLen { dst: r(*dst), base: r(*list) }
                }
            }
            RegInstr::ListGet { dst, list, index } => {
                require(int(*index))?;
                // TV2 flat (loop-invariant typed) list ⇒ bounds-checked direct read.
                if ty[*list] == Some(NativeTy::FlatFloat) {
                    require(float(*dst))?;
                    JitInstr::ListGetFloatDirect { dst: r(*dst), base: r(*list), index: r(*index) }
                } else if ty[*list] == Some(NativeTy::FlatInt) {
                    require(int_or_free(*dst))?;
                    JitInstr::ListGetIntDirect { dst: r(*dst), base: r(*list), index: r(*index) }
                } else if handle_reg(*dst) {
                    // A heap-valued element (a struct holding a stored closure)
                    // fetched as a fresh handle for a downstream field/closure read.
                    require(handle_reg(*list))?;
                    JitInstr::ListGetHandle { dst: r(*dst), base: r(*list), index: r(*index) }
                } else if float(*dst) {
                    require(handle_reg(*list))?;
                    JitInstr::ListGetFloat { dst: r(*dst), base: r(*list), index: r(*index) }
                } else {
                    require(handle_reg(*list) && int_or_free(*dst))?;
                    JitInstr::ListGetInt { dst: r(*dst), base: r(*list), index: r(*index) }
                }
            }
            RegInstr::NativeGuardClosureId { closure, expected } => {
                require(handle_reg(*closure))?;
                let expected = i64::try_from(*expected).ok()?;
                JitInstr::GuardClosureId { base: r(*closure), expected }
            }
            RegInstr::NativeClosureId { dst, closure } => {
                require(handle_reg(*closure) && int(*dst))?;
                JitInstr::ClosureId { dst: r(*dst), base: r(*closure) }
            }
            RegInstr::NativeClosureCapture { dst, closure, index } => {
                // The capture's `dst` may be Int/Bool (i64 slot used directly) or
                // Float (the i64 slot is `f64::to_bits`, bit-reinterpreted to f64
                // in codegen). An unconstrained `dst` defaults to Int. A non-scalar
                // `dst` (Handle/flat array) cannot hold a scalar capture ⇒ bail.
                require(
                    handle_reg(*closure)
                        && (int_or_free(*dst) || bool_ty(*dst) || float(*dst)),
                )?;
                let index = u32::try_from(*index).ok()?;
                JitInstr::ClosureCapture { dst: r(*dst), base: r(*closure), index }
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
                JitInstr::IntToFloat { dst: r(*dst), src: r(args[0]) }
            }
            // Any other (non-subset) instruction in-region was already rejected.
            _ => return None,
        };
        jit_code.push(jit);
    }

    // Native type per register, then the JIT-class projection for codegen.
    let native_reg_types: Vec<NativeTy> = (0..n_regs)
        .map(|reg| ty[reg].unwrap_or(NativeTy::Int))
        .collect();
    let reg_types = native_reg_types
        .iter()
        .map(|t| t.jit_value_type())
        .collect();
    // Param types so the caller can marshal handle params (List/struct) in the
    // window; scalar live-in regs marshal directly by `reg_types`.
    let param_types: Vec<NativeTy> = native_reg_types[..n_params].to_vec();

    let jit_fn = vm_jit::JitFunction {
        n_params: n_params as u32,
        n_regs: n_regs as u32,
        reg_types,
        code: jit_code,
    };
    Some((jit_fn, param_types, native_reg_types))
}
