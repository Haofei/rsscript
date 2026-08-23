use super::*;

/// Like [`native_callee_inlinable`] but permits a **capturing** closure callee
/// (OSR × profile-guided inlining): every capture must be materialized as a scalar at the inline site
/// (the gate enforces scalarity via the profile's `captures_all_scalar` bit), so
/// the body addresses its capture registers `0..captures` exactly like ordinary
/// scalar params. `n_args` is the call's argument count, which must equal the
/// callee's PARAM count (captures are bound separately). Every reachable
/// instruction must still be a pure native-subset op / native control flow /
/// `Return`; the uniform `base`-offset splice places capture reg `k` at `base + k`.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_capturing_callee_inlinable(
    callee: &RegFunction,
    n_args: usize,
) -> bool {
    if callee.params != n_args {
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
/// If the `CallClosure` at instruction index `i` in `func` qualifies for profile-guided inlining
/// profile-guided monomorphic inlining, return the observed callee's function id
/// `k` (into `unit.functions`); otherwise `None`.
///
/// All of the following must hold (any failure ⇒ leave the site on its normal,
/// interpreter path — behavior unchanged):
/// - `func.code[i]` is a `CallClosure` with **no `mut` args** and a closure operand
///   that is a **parameter** (`< func.params`), i.e. a native-visible handle (the
///   higher-order "take a closure, call it" shape);
/// - bounded profile collection's profile for site `i` is **Monomorphic** with exactly one observed callee
///   key `k` (so the identity guard has a single speculated target);
/// - `unit.functions[k]` is **non-capturing** and [`native_callee_inlinable`] at the
///   call's arity (side-effect-free, splice-able).
///
/// Read-only over the profile (a `try_borrow`, never a panic); never mutates state
/// or feeds a computed value.
/// Whether `closure` is a **native-readable closure handle** operand for a
/// `CallClosure` (Pending #1, stored-closure broadening). Either:
/// - a **parameter** handle (`closure < func.params`) — the shipped higher-order
///   "take a closure, call it" shape (marshalled into the heap-arg window); or
/// - a register produced by an in-function native-subset heap read — a
///   `GetFieldSlot` (`f = op.apply`) or `ListGet` (`op = List.get(ops, i)`) — i.e.
///   a **stored** closure fetched each iteration. Such a register is lowered to a
///   `FieldHandle`/`ListGetHandle` read (a fresh heap-table index) and consumed by
///   the closure guard/dispatch, so the closure identity is checked at runtime and
///   a wrong handle simply bails. We require the producer to be exactly one of those
///   reads so the operand is provably a fetched heap value (never a scalar register
///   reinterpreted as a handle).
#[cfg(all(feature = "native-jit", feature = "jit-speculation"))]
pub(in crate::reg_vm) fn native_readable_closure_operand(
    func: &RegFunction,
    closure: usize,
) -> bool {
    if closure < func.params {
        return true;
    }
    // A non-param operand must be produced by a reachable native-subset heap read
    // (`GetFieldSlot`/`ListGet`), possibly through `Move` temporaries the lowerer
    // introduces. This shares the same CFG-backed alias closure as the other native
    // eligibility analyses.
    let Some(analysis) =
        NativeRegionAnalysis::compute_prefix(&func.code, func.regs, 0, func.code.len())
    else {
        return false;
    };
    let Some(read_regs) = analysis.reachable_heap_read_defs_closed_under_moves(&func.code) else {
        return false;
    };
    read_regs.get(closure).copied().unwrap_or(false)
}

#[cfg(all(feature = "native-jit", feature = "jit-speculation"))]
pub(in crate::reg_vm) fn native_readable_or_sinkable_closure_operand_candidate(
    func: &RegFunction,
    closure: usize,
) -> bool {
    if closure < func.params {
        return true;
    }
    let Some(analysis) =
        NativeRegionAnalysis::compute_prefix(&func.code, func.regs, 0, func.code.len())
    else {
        return false;
    };
    let Some(value_regs) = analysis.native_readable_or_sinkable_closure_operands(&func.code) else {
        return false;
    };
    value_regs.get(closure).copied().unwrap_or(false)
}

#[cfg(all(feature = "native-jit", not(feature = "jit-speculation")))]
pub(in crate::reg_vm) fn native_readable_or_sinkable_closure_operand_candidate(
    _func: &RegFunction,
    _closure: usize,
) -> bool {
    false
}

#[cfg(all(feature = "native-jit", feature = "jit-speculation"))]
pub(in crate::reg_vm) fn monomorphic_closure_inline_target(
    unit: &RegUnit,
    func: &RegFunction,
    profile: Option<&FunctionProfile>,
    call_count: u32,
    i: usize,
) -> Option<usize> {
    let (closure, args, mut_args) = match func.code.get(i)? {
        RegInstr::CallClosure {
            closure,
            args,
            mut_args,
            ..
        } => (*closure, args, mut_args),
        _ => return None,
    };
    // Conservative shape gate: side-effect-free call (no write-backs) whose closure
    // is a native-readable handle (a parameter, or a stored closure fetched via a
    // `GetFieldSlot`/`ListGet` heap read).
    if !mut_args.is_empty() || !native_readable_closure_operand(func, closure) {
        return None;
    }
    // bounded type profile: compile only after the bounded sampling window freezes. Before
    // then a one-callee observation can still mature into a polymorphic site, and
    // caching an early monomorphic native body would permanently hide that shape.
    if call_count < PROFILE_RECORD_LIMIT {
        return None;
    }
    // Frozen profile: this site must have settled on exactly one callee.
    let feedback = profile?.call_sites.get(&i)?;
    if feedback.state() != MonoState::Monomorphic {
        return None;
    }
    // Monomorphic ⇒ exactly one observed callee key (its function id).
    let &(key, _) = feedback.observed.first()?;
    if feedback.observed.len() != 1 {
        return None;
    }
    let k = usize::try_from(key).ok()?;
    let callee = unit.functions.get(k)?;
    // Non-capturing callee: the original (shipped) path. A capturing callee is
    // allowed ONLY when every observed capture at this site was scalar (the
    // profile's monotone `captures_all_scalar` bit) — then each capture is
    // materialized as a scalar at the inline site via the `closure_capture` host
    // helper. A heap capture (or a profile that ever saw one) leaves the site on
    // its interpreter path: no inline, no OSR.
    if callee.captures == 0 {
        if !native_callee_inlinable(callee, args.len()) {
            return None;
        }
    } else if !feedback.captures_all_scalar
        || !native_capturing_callee_inlinable(callee, args.len())
    {
        return None;
    }
    Some(k)
}

#[cfg(all(feature = "native-jit", not(feature = "jit-speculation")))]
pub(in crate::reg_vm) fn monomorphic_closure_inline_target(
    _unit: &RegUnit,
    _func: &RegFunction,
    _profile: Option<&FunctionProfile>,
    _call_count: u32,
    _i: usize,
) -> Option<usize> {
    None
}

/// polymorphic inline cache gate. If the `CallClosure` at instruction index
/// `i` qualifies, return its 2–3 observed callee ids (into `unit.functions`);
/// otherwise `None`. Sibling to [`monomorphic_closure_inline_target`] — after the
/// bounded bounded profile collection sampling window freezes, a site is EITHER mono (single-guard path)
/// OR poly (this dispatch path), never both.
///
/// All of the following must hold (any failure ⇒ leave the site on its normal
/// interpreter path — behavior unchanged):
/// - `func.code[i]` is a `CallClosure` with **no `mut` args** and a closure operand
///   that is a **parameter** (`< func.params`), i.e. a native-visible handle (same
///   shape gate as the monomorphic case);
/// - bounded profile collection's profile for site `i` is **Polymorphic** — by construction (bounded profile collection caps the
///   observed set at 4 and marks >3 as Megamorphic) this means **2 or 3** distinct
///   observed callee keys;
/// - **EVERY** observed callee is **non-capturing** and [`native_callee_inlinable`]
///   at the call's arity. If any single observed callee fails, the WHOLE site is
///   disqualified (no partial inlining): a no-match bail must be able to re-run the
///   exact same side-effect-free subset on the interpreter.
///
/// Read-only over the profile (a `try_borrow`, never a panic); never mutates state.
#[cfg(all(feature = "native-jit", feature = "jit-speculation"))]
pub(in crate::reg_vm) fn polymorphic_closure_inline_targets(
    unit: &RegUnit,
    func: &RegFunction,
    profile: Option<&FunctionProfile>,
    call_count: u32,
    i: usize,
) -> Option<Vec<usize>> {
    let (closure, args, mut_args) = match func.code.get(i)? {
        RegInstr::CallClosure {
            closure,
            args,
            mut_args,
            ..
        } => (*closure, args, mut_args),
        _ => return None,
    };
    // Same conservative shape gate as the monomorphic case.
    if !mut_args.is_empty() || !native_readable_closure_operand(func, closure) {
        return None;
    }
    // bounded type profile: compile only after the bounded sampling window freezes, so the
    // 2- or 3-target PIC is derived from a stable observed set.
    if call_count < PROFILE_RECORD_LIMIT {
        return None;
    }
    let feedback = profile?.call_sites.get(&i)?;
    if feedback.state() != MonoState::Polymorphic {
        return None;
    }
    // Polymorphic ⇒ 2 or 3 distinct observed callees (bounded profile collection caps at 4 / >3 ⇒ Mega).
    let n = feedback.observed.len();
    if !(2..=3).contains(&n) {
        return None;
    }
    let mut ranked: Vec<(usize, u32, usize)> = Vec::with_capacity(n);
    for (first_seen, &(key, count)) in feedback.observed.iter().enumerate() {
        let k = usize::try_from(key).ok()?;
        let callee = unit.functions.get(k)?;
        // EVERY observed callee must be inlinable, else disqualify the whole site
        // (no partial inlining — a no-match bail must re-run the exact same side-
        // effect-free subset on the interpreter). A CAPTURING callee is allowed
        // ONLY when every observed capture at this site was scalar (the profile's
        // monotone `captures_all_scalar` bit), so each capture materializes via the
        // `closure_capture` host helper. A non-scalar (heap) capture, or any callee
        // not native-inlinable at the call's arity, disqualifies the whole site.
        let ok = if callee.captures == 0 {
            native_callee_inlinable(callee, args.len())
        } else {
            feedback.captures_all_scalar && native_capturing_callee_inlinable(callee, args.len())
        };
        if !ok {
            return None;
        }
        ranked.push((k, count, first_seen));
    }
    // Hottest-first PIC arm order: the common callee gets the first compare/branch.
    // Preserve first-seen order for equal counts so the emitted code is stable.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.2.cmp(&b.2)));
    Some(ranked.into_iter().map(|(k, _, _)| k).collect())
}

#[cfg(all(feature = "native-jit", not(feature = "jit-speculation")))]
pub(in crate::reg_vm) fn polymorphic_closure_inline_targets(
    _unit: &RegUnit,
    _func: &RegFunction,
    _profile: Option<&FunctionProfile>,
    _call_count: u32,
    _i: usize,
) -> Option<Vec<usize>> {
    None
}

/// Whether a closure-call site could become eligible after its bounded profile
/// freezes. The implementation is compiled only for the research feature; the
/// stable native tier has no retry state for speculative closure shapes.
#[cfg(all(feature = "native-jit", feature = "jit-speculation"))]
pub(in crate::reg_vm) fn native_translation_pending_on_profile(
    _unit: &RegUnit,
    func: &RegFunction,
    profile: Option<&FunctionProfile>,
    call_count: u32,
) -> bool {
    if call_count >= PROFILE_RECORD_LIMIT {
        return false;
    }
    func.code.iter().enumerate().any(|(i, instr)| match instr {
        RegInstr::CallClosure {
            closure, mut_args, ..
        } => {
            if !mut_args.is_empty() || !native_readable_closure_operand(func, *closure) {
                return false;
            }
            profile
                .and_then(|profile| profile.call_sites.get(&i))
                .is_none_or(|feedback| feedback.state() != MonoState::Megamorphic)
        }
        _ => false,
    })
}

#[cfg(all(feature = "native-jit", not(feature = "jit-speculation")))]
pub(in crate::reg_vm) fn native_translation_pending_on_profile(
    _unit: &RegUnit,
    _func: &RegFunction,
    _profile: Option<&FunctionProfile>,
    _call_count: u32,
) -> bool {
    false
}

/// Whether `instr` is one of the four `Option` register-ops that the scalar-
/// replacement pre-pass dissolves into tag + payload scalar registers.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn is_option_op(instr: &RegInstr) -> bool {
    matches!(
        instr,
        RegInstr::MakeSome { .. }
            | RegInstr::LoadNone { .. }
            | RegInstr::MatchOption { .. }
            | RegInstr::UnwrapSome { .. }
    )
}

#[cfg(feature = "native-jit")]
fn option_regs_definitely_assigned_before_region_reads(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
    opt: &[bool],
) -> bool {
    let Some(analysis) = NativeRegionAnalysis::compute_region(code, n_regs, header, exit) else {
        return false;
    };

    fn mark_option_defs(instr: &RegInstr, opt: &[bool], assigned: &mut [bool]) {
        match instr {
            RegInstr::LoadNone { dst } | RegInstr::MakeSome { dst, .. } if opt[*dst] => {
                assigned[*dst] = true;
            }
            RegInstr::DequePopFront { dst, .. } | RegInstr::DequePopBack { dst, .. }
                if opt[*dst] =>
            {
                assigned[*dst] = true;
            }
            RegInstr::Move { dst, src } if opt[*dst] && opt[*src] => {
                assigned[*dst] = true;
            }
            _ => {}
        }
    }

    analysis
        .forward_definite_regs(code, |_ip, instr, assigned| {
            let reads = match instr_read_regs(instr) {
                RegFootprint::Some(reads) => reads,
                RegFootprint::All => return None,
            };
            if reads
                .into_iter()
                .any(|reg| reg < n_regs && opt[reg] && !assigned[reg])
            {
                return None;
            }
            mark_option_defs(instr, opt, assigned);
            Some(())
        })
        .is_some()
}

/// The register *read* positions (value operands) of an instruction that is in the
/// native subset ([`native_subset_instruction`]) or is one of the four `Option`
/// ops ([`is_option_op`]). Used by scalar replacement escape analysis to find every use of an
/// Option register. Deliberately NOT a full enumeration of every `RegInstr` — the
/// scalar-replacement pass only ever calls it after confirming every reachable
/// instruction is in exactly this set, so a future enum addition outside the subset
/// cannot silently reach here (the pass bails first). `None` for any instruction
/// outside that set, which the caller treats as conservatively escaping.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn subset_or_option_reads(instr: &RegInstr) -> Option<Vec<usize>> {
    Some(match instr {
        RegInstr::LoadInt { .. }
        | RegInstr::LoadFloat { .. }
        | RegInstr::LoadBool { .. }
        | RegInstr::LoadString { .. }
        | RegInstr::Jump { .. }
        | RegInstr::RuntimeError { .. }
        | RegInstr::LoadNone { .. } => vec![],
        RegInstr::Move { src, .. } => vec![*src],
        RegInstr::DeepCopy { reg } | RegInstr::DeepCopyElided { reg } => vec![*reg],
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
        | RegInstr::JumpIfIntCompare { lhs, rhs, .. } => vec![*lhs, *rhs],
        RegInstr::JumpIfBool { cond, .. } => vec![*cond],
        RegInstr::Return { src } => vec![*src],
        RegInstr::GetFieldSlot { base, .. } => vec![*base],
        RegInstr::SetFieldSlot { base, value, .. } => vec![*base, *value],
        RegInstr::ListLen { list, .. } => vec![*list],
        RegInstr::ListGet { list, index, .. } => vec![*list, *index],
        RegInstr::ListSet {
            list, index, value, ..
        } => vec![*list, *index, *value],
        RegInstr::ListPush { list, value, .. } => vec![*list, *value],
        RegInstr::ListSort { list, .. } => vec![*list],
        RegInstr::MapInsert {
            map, key, value, ..
        } => vec![*map, *key, *value],
        RegInstr::SetInsert { set, value, .. } => vec![*set, *value],
        RegInstr::SortedSetInsert { set, value, .. } => vec![*set, *value],
        RegInstr::SortedMapInsert {
            map, key, value, ..
        } => vec![*map, *key, *value],
        RegInstr::DequePushBack { deque, value, .. }
        | RegInstr::DequePushFront { deque, value, .. } => vec![*deque, *value],
        RegInstr::DequePopFront { deque, .. } | RegInstr::DequePopBack { deque, .. } => {
            vec![*deque]
        }
        RegInstr::MatchMapGet { map, key, .. } | RegInstr::MatchSortedMapGet { map, key, .. } => {
            vec![*map, *key]
        }
        RegInstr::StringConcat { left, right, .. } => vec![*left, *right],
        RegInstr::CallIntrinsic { args, .. } | RegInstr::CallTypedIntrinsic { args, .. } => {
            args.clone()
        }
        RegInstr::NativeGuardClosureId { closure, .. } => vec![*closure],
        RegInstr::NativeClosureId { closure, .. } => vec![*closure],
        RegInstr::NativeClosureCapture { closure, .. } => vec![*closure],
        RegInstr::NativeFieldClosureId { base, .. }
        | RegInstr::NativeFieldClosureCapture { base, .. } => vec![*base],
        // Option ops and `?` success projection (value operands).
        RegInstr::MakeSome { value, .. } => vec![*value],
        RegInstr::MatchOption { src, .. } => vec![*src],
        RegInstr::UnwrapSome { src, .. } => vec![*src],
        RegInstr::TryResult { src, cleanup, .. } => {
            let mut reads = Vec::with_capacity(cleanup.len() + 1);
            reads.push(*src);
            reads.extend(cleanup.iter().copied());
            reads
        }
        // Variant ops (value operands). `MakeVariant`'s value operands are its
        // field registers; `MatchVariant`/`UnwrapVariantValue` read `src`.
        RegInstr::MakeVariant { fields, .. } => fields.iter().map(|(_, r)| *r).collect(),
        RegInstr::MatchVariant { src, .. } => vec![*src],
        RegInstr::UnwrapVariantValue { src, .. } => vec![*src],
        _ => return None,
    })
}

/// Whether `instr` is one of the three variant register-ops that the scalar-
/// replacement dissolves into a tag + payload scalar register pair.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn is_variant_op(instr: &RegInstr) -> bool {
    matches!(
        instr,
        RegInstr::MakeVariant { .. }
            | RegInstr::MatchVariant { .. }
            | RegInstr::UnwrapVariantValue { .. }
    )
}

/// Whether `instr` is a struct construction op that the scalar replacement struct scalar-replacement
/// dissolves into one scalar register per field slot. `GetFieldSlot` is already in
/// the native subset (it is also a heap-read on a handle param), so the struct pass
/// only needs to additionally accept `MakeStruct` inside the region.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn is_make_struct_op(instr: &RegInstr) -> bool {
    matches!(instr, RegInstr::MakeStruct { .. })
}

/// scalar replacement (escape analysis + scalar replacement). Identify `Option` registers that are
/// NON-ESCAPING with a *scalar* payload and rewrite the function's `RegInstr` code
/// so each such Option is dissolved into two scalar registers — `tag` (Bool,
/// true=Some / false=None) and `payload` — leaving only native-subset ops (so the
/// function then compiles through the existing native path with no heap allocation).
///
/// Returns `Some((code, n_regs, payload_regs))` when the function contains either no
/// Option ops (code returned essentially unchanged) OR only scalar-replaceable ones.
/// `payload_regs` are the freshly-allocated payload registers, which the caller must
/// verify (post type-inference) are scalar (Int/Float/Bool) — a Handle/flat payload
/// is a heap value and disqualifies the function.
///
/// Returns `None` (⇒ leave the WHOLE function on its current interpreter path, never
/// partially transformed) if the function has any Option op that is NOT scalar-
/// replaceable. Conservative: any unrecognized use of an Option register, or any
/// non-subset / non-Option instruction in the body, makes it escaping.
///
/// Non-escaping criterion for an Option register `R` (defined by `MakeSome`/
/// `LoadNone`, transitively including `Move`-aliases): EVERY use of `R` is one of
/// `MatchOption{src:R}`, `UnwrapSome{src:R}`, `TryResult{src:R}`, or `Move{src:R}`
/// (whose dst is itself an Option register); `R` is never a value operand of
/// anything else; and `R`'s only definitions are `MakeSome`/`LoadNone`/`Move`-
/// from-Option. A `MakeSome` payload that is itself an Option register is
/// non-scalar ⇒ escaping.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_scalar_replace_options(
    code: &[RegInstr],
    n_regs: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<usize>)> {
    let reachable = native_reachable_instructions(code);

    // Fast path: no Option ops at all — nothing to do, no payload regs to verify.
    if !code
        .iter()
        .enumerate()
        .any(|(i, instr)| reachable[i] && is_option_op(instr))
    {
        // Identity ip-map: transformed code == original code, so each transformed
        // ip maps to itself.
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, Vec::new(), ip_map));
    }

    // Every reachable instruction must be either in the native subset or one of the
    // four Option ops; anything else makes the function ineligible anyway (and would
    // also defeat the read-enumeration below). Bail (interpreter path) if not.
    for (i, instr) in code.iter().enumerate() {
        if reachable[i]
            && !native_subset_instruction(instr)
            && !is_option_op(instr)
            && !matches!(instr, RegInstr::TryResult { .. })
        {
            return None;
        }
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, 0, code.len())?;

    // OPT = the set of registers that carry an Option value. Seed with every
    // `MakeSome`/`LoadNone` destination, then close under `Move` aliasing
    // (`Move{dst,src}` with `src` ∈ OPT ⇒ `dst` ∈ OPT).
    let mut opt = vec![false; n_regs];
    for (i, instr) in code.iter().enumerate() {
        if !reachable[i] {
            continue;
        }
        match instr {
            RegInstr::MakeSome { dst, .. } | RegInstr::LoadNone { dst } => opt[*dst] = true,
            _ => {}
        }
    }
    analysis.close_reachable_move_aliases(code, &mut opt)?;

    // Validate: every OPT register is only ever DEFINED by `MakeSome`/`LoadNone`/
    // `Move`-from-OPT, and only ever USED by the recognized consumers. Any other
    // definition or use ⇒ escaping ⇒ bail (leave the whole function on its path).
    for (i, instr) in code.iter().enumerate() {
        if !reachable[i] {
            continue;
        }
        match instr {
            // Recognized definitions of OPT registers.
            RegInstr::LoadNone { dst } if opt[*dst] => {}
            RegInstr::MakeSome { dst, value } if opt[*dst] => {
                // The payload must be a scalar — an Option payload is non-scalar.
                if opt[*value] {
                    return None;
                }
            }
            RegInstr::Move { dst, src } if opt[*dst] => {
                // A Move that DEFINES an OPT register must copy from an OPT register
                // (pure alias). `src` ∈ OPT by the fixpoint unless `dst` was seeded
                // by a non-Move def and is also Move-assigned a non-Option — reject.
                if !opt[*src] {
                    return None;
                }
            }
            // Recognized uses (consumers) of OPT registers — fine.
            RegInstr::MatchOption { src, .. } if opt[*src] => {}
            RegInstr::UnwrapSome { src, dst } if opt[*src] => {
                // The unwrapped payload `dst` must NOT itself be an Option register
                // (would mean a non-scalar payload slipped through).
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::TryResult { dst, src, .. } if opt[*src] => {
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if opt[*src] => {
                // A Move that READS an OPT register: its dst is in OPT (fixpoint), so
                // it is a recognized alias and handled by the def arm above.
            }
            // Any OTHER instruction must not touch an OPT register at all.
            other => {
                let reads = subset_or_option_reads(other)?;
                if reads.into_iter().any(|r| opt[r]) {
                    return None; // an OPT register escapes into a non-recognized use
                }
                // It also must not (re)define an OPT register through an
                // unrecognized destination. The only writers of OPT registers are
                // handled above; a non-recognized instruction whose dst is an OPT
                // register would mean the register is not purely an Option, so bail.
                if let RegInstr::UnwrapSome { dst, .. }
                | RegInstr::MakeSome { dst, .. }
                | RegInstr::LoadNone { dst } = other
                    && opt[*dst]
                {
                    return None;
                }
            }
        }
    }

    // Allocate two fresh registers per OPT register: tag (Int) and payload.
    let mut tag_reg = vec![0usize; n_regs];
    let mut payload_reg = vec![0usize; n_regs];
    let mut payload_regs: Vec<usize> = Vec::new();
    let mut next_reg = n_regs;
    for (reg, is_opt) in opt.iter().enumerate() {
        if *is_opt {
            tag_reg[reg] = next_reg;
            payload_reg[reg] = next_reg + 1;
            payload_regs.push(next_reg + 1);
            next_reg += 2;
        }
    }

    // Rewrite, remapping jump targets (indices shift because `MakeSome`/
    // `MatchOption` expand to two instructions). Same index-map + fixup discipline
    // as `native_inline_leaf_calls`.
    enum Fix {
        Target(usize),
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        match instr {
            RegInstr::MakeSome { dst, value } if opt[*dst] => {
                // tag = Some (Bool true). The tag is a `Bool` (stored as i64 0/1) so
                // it can drive a native `JumpIfBool` directly; `LoadBool true` is the
                // native-subset form of "tag = 1".
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: *value,
                });
            }
            RegInstr::LoadNone { dst } if opt[*dst] => {
                // None: tag = false. Payload left undefined; the None arm never reads it.
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: false,
                });
            }
            RegInstr::Move { dst, src } if opt[*dst] => {
                // Alias copy: payload scratch registers have an explicit typed-zero
                // entry value in the lowered JIT IR, so None aliases remain defined.
                new_code.push(RegInstr::Move {
                    dst: tag_reg[*dst],
                    src: tag_reg[*src],
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: payload_reg[*src],
                });
            }
            RegInstr::MatchOption {
                src,
                some_ip,
                none_ip,
            } if opt[*src] => {
                // tag true (Some) → some_ip; else fall to an explicit jump to none_ip.
                fixups.push((new_code.len(), Fix::Target(*some_ip)));
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: 0,
                });
                fixups.push((new_code.len(), Fix::Target(*none_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::UnwrapSome { dst, src } if opt[*src] => {
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            RegInstr::TryResult { dst, src, .. } if opt[*src] => {
                let some_target = new_code.len() + 2;
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: some_target,
                });
                new_code.push(RegInstr::RuntimeError {
                    message: String::new(),
                });
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            // Copy-through, remapping any jump target (these never touch OPT regs).
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, Fix::Target(t)) in fixups {
        let target = index_map[t];
        match &mut new_code[pos] {
            RegInstr::Jump { target: dst }
            | RegInstr::JumpIfBool { target: dst, .. }
            | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
            _ => {}
        }
    }
    // Inverse ip-map: `ip_map[transformed_ip] = original_ip`. Each original
    // instruction `i` expanded to the consecutive transformed range
    // `[index_map[i], index_map[i+1])` (or `..new_code.len()` for the last), so
    // every transformed ip in that range maps back to `i`. Each original
    // instruction always emits at least one transformed instruction (the rewrite
    // loop pushes unconditionally), so the ranges tile `0..new_code.len()`
    // exactly. A rewritten Option op (e.g. `MatchOption` → `JumpIfBool;Jump`)
    // thus maps every fragment to the original op's index; copy-through
    // instructions map one-to-one.
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, payload_regs, ip_map))
}

/// The interned `Ok { value }` variant layout, used by the combinator-expansion
/// pass when lowering `Result.map` into `MakeVariant{Ok,[mapped]}` (the same shape
/// the lowerer and the interpreter's `value_ok` produce).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn result_ok_layout() -> Rc<crate::vm_value::TypeLayout> {
    crate::vm_value::intern_layout(Rc::from("Ok"), vec![Rc::from("value")])
}

/// The interned `Err(value)` layout — the `Err` analog of [`result_ok_layout`] (the
/// lowerer builds both `Ok` and `Err` with a single field named `value`). Used to
/// reconstruct a live-after two-armed Result's `Err` arm at OSR-exit.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn result_err_layout() -> Rc<crate::vm_value::TypeLayout> {
    crate::vm_value::intern_layout(Rc::from("Err"), vec![Rc::from("value")])
}

/// Whether `intrinsic` is one of the six Option/Result combinator intrinsics that
/// the combinator-expansion pass (deopt-before-heap Slice 2) lowers into primitive
/// match/construct form with the mapper closure inlined. Recognition now reads the
/// central [`intrinsic_descriptor`] table's `combinator_kind`; the pass keeps the
/// exact per-kind match/construct lowering.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn combinator_intrinsic_kind(
    intrinsic: RegIntrinsic,
) -> Option<CombinatorKind> {
    intrinsic_descriptor(intrinsic).combinator_kind
}

#[cfg(feature = "native-jit")]
impl CombinatorKind {
    /// Whether `arg[1]` is a mapper *closure* (map/and_then) rather than a scalar
    /// default value (unwrap_or).
    fn has_mapper(self) -> bool {
        !matches!(
            self,
            CombinatorKind::OptionUnwrapOr | CombinatorKind::ResultUnwrapOr
        )
    }
}

/// OSR × scalar replacement (deopt-before-heap, Slice 2): expand the six Option/Result combinator
/// intrinsics (`Option.map`/`and_then`/`unwrap_or`, `Result.map`/`and_then`/
/// `unwrap_or`) that appear inside the loop region `[header, exit)` into primitive
/// match/construct form, leaving the mapper closure call as an in-region
/// `CallClosure{closure: mapper_reg, args:[payload]}`. The downstream
/// [`native_inline_leaf_calls`] then SINKS each loop-local mapper `MakeClosure`
/// (inlining its body), and the Option / Result scalar-replacement passes dissolve
/// the per-iteration Option/Result values — so the combinator chain becomes pure
/// scalar code and the loop OSRs.
///
/// The lowering replicates the interpreter's exact combinator semantics
/// (`exec_option_intrinsics` / `exec_result_intrinsics`):
/// - `OptionMap(o, f)`     → `match o { Some(v) => MakeSome(f(v)), None => LoadNone }`
/// - `OptionAndThen(o, f)` → `match o { Some(v) => f(v) /*already Option*/, None => LoadNone }`
/// - `OptionUnwrapOr(o,d)` → `match o { Some(v) => v, None => d }`
/// - `ResultMap(r, f)`     → `match r { Ok(v) => MakeVariant{Ok,[f(v)]}, Err(_) => Bail }`
/// - `ResultAndThen(r, f)` → `match r { Ok(v) => f(r) /*already Result*/, Err(_) => Bail }`
/// - `ResultUnwrapOr(r,d)` → `match r { Ok(v) => v, Err(_) => d }`
///
/// The `Result` `Err` arm rebuilds a HEAP `Err` in the interpreter; building heap is
/// forbidden on the native path (the transactional fallback contract), so that arm becomes a native
/// `Bail` (a `RuntimeError` sentinel — identical to the Slice-1 cold-arm splice).
/// Because the inlined `checked` leaf's own `Err` arm already bailed, the `Result`
/// reaching `ResultMap`/`ResultAndThen` is statically always-`Ok` after inlining,
/// so Slice-1 Result scalar-replacement dissolves it (the `Err` arm goes dead). The
/// `Ok` arm constructs `MakeVariant{Ok,[scalar]}`, exactly the shape Result-SR
/// dissolves. `ResultUnwrapOr`'s `Err` arm only moves the scalar default (no heap),
/// so it need not bail; but a live heap `Err` reaching it would have been dissolved
/// upstream — if not, the surrounding Result-SR/escape gates bail.
///
/// Conservative — returns `None` (⇒ no OSR; the loop stays on the interpreter)
/// when ANY in-region combinator's mapper is not a loop-local, native-inlinable
/// `MakeClosure` (a stored/param `Fn`, a capturing-with-heap closure, …): such a
/// mapper cannot be sunk/inlined, so leaving a bare `CallClosure` would block the
/// native subset anyway. A non-combinator body returns the code unchanged with an
/// identity ip-map (byte-for-byte the old path). Escape / dead-at-boundary of the
/// produced Option/Result values is enforced by the downstream SR passes, exactly
/// as for a hand-written `match`.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→
/// original `ip_map` discipline as the sibling region passes.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_expand_option_result_combinators_in_region(
    unit: &RegUnit,
    _func: &RegFunction,
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no combinator intrinsic in the region ⇒ identity transform.
    let has_combinator = (header..exit).any(|i| {
        matches!(
            &code[i],
            RegInstr::CallIntrinsic { intrinsic, .. } | RegInstr::CallTypedIntrinsic { intrinsic, .. }
                if combinator_intrinsic_kind(*intrinsic).is_some()
        )
    });
    if !has_combinator {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }

    // For a mapper-bearing combinator, the mapper arg register MUST be the dst of an
    // in-region `MakeClosure` whose callee is native-inlinable at arity 1 (the
    // single payload argument). The closure is loop-local and only fed into the
    // combinator, so `loop_local_sinkable_closures` will sink + inline it. If we
    // cannot prove this, bail the whole expansion (no OSR): a bare `CallClosure`
    // would block the native subset and a non-inlinable mapper is genuinely opaque.
    let mapper_callee = |mapper_reg: usize| -> Option<usize> {
        // Single defining `MakeClosure` in-region (no copy-Move forwarding needed for
        // the literal-closure shape the lowerer emits; a forwarded/redefined mapper
        // conservatively fails this lookup and bails).
        let mut found: Option<usize> = None;
        for (mi, instr) in code.iter().enumerate() {
            if let RegInstr::MakeClosure { dst, function, .. } = instr
                && *dst == mapper_reg
            {
                if !in_region(mi) || found.is_some() {
                    return None;
                }
                found = Some(*function);
            }
        }
        let k = found?;
        let callee = unit.functions.get(k)?;
        // The mapper is invoked with exactly one argument (the matched payload).
        let inlinable = if callee.captures == 0 {
            native_callee_inlinable_j3(callee, 1)
        } else {
            // A capturing mapper needs all-scalar captures to be sinkable; we cannot
            // see the profile's `captures_all_scalar` bit here, so accept only
            // captureless mappers (the common literal `|v| {...}` shape). A capturing
            // mapper bails the expansion (conservative ⇒ no OSR).
            false
        };
        if !inlinable {
            return None;
        }
        Some(k)
    };

    // Validate every in-region combinator up front; bail the whole pass on the first
    // one we cannot expand. Collect the (operand, mapper/default) regs for the rewrite.
    for i in header..exit {
        let (intrinsic, args) = match &code[i] {
            RegInstr::CallIntrinsic {
                intrinsic, args, ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic, args, ..
            } => (*intrinsic, args),
            _ => continue,
        };
        let Some(kind) = combinator_intrinsic_kind(intrinsic) else {
            continue;
        };
        if args.len() != 2 {
            return None;
        }
        if kind.has_mapper() {
            mapper_callee(args[1])?;
        }
    }

    // Rewrite the WHOLE code: each in-region combinator becomes a primitive
    // match/construct fragment (fresh temp + payload regs allocated above `n_regs`);
    // everything else copies through with jump/match targets remapped through the
    // index map (identical discipline to the sibling SR passes).
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
        // A forward jump to the join point AFTER the combinator fragment at original
        // index `orig`: resolved to the start of the next original instruction.
        JoinAfter(usize),
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len() + 16);
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    let mut next_reg = n_regs;

    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        let combinator = if region {
            match instr {
                RegInstr::CallIntrinsic {
                    intrinsic,
                    args,
                    dst,
                }
                | RegInstr::CallTypedIntrinsic {
                    intrinsic,
                    args,
                    dst,
                    ..
                } => combinator_intrinsic_kind(*intrinsic).map(|k| (k, args.clone(), *dst)),
                _ => None,
            }
        } else {
            None
        };
        if let Some((kind, args, dst)) = combinator {
            let operand = args[0];
            let other = args[1]; // mapper closure OR scalar default
            match kind {
                CombinatorKind::OptionMap
                | CombinatorKind::OptionAndThen
                | CombinatorKind::OptionUnwrapOr => {
                    // match operand { Some(v) => <some arm> ; None => <none arm> }
                    let payload = next_reg;
                    next_reg += 1;
                    // MatchOption operand → some_ip(next) / none_ip(patched)
                    let match_pos = new_code.len();
                    new_code.push(RegInstr::MatchOption {
                        src: operand,
                        some_ip: 0,
                        none_ip: 0,
                    });
                    // --- Some arm (falls through from the match's some_ip) ---
                    let some_ip = new_code.len();
                    new_code.push(RegInstr::UnwrapSome {
                        dst: payload,
                        src: operand,
                    });
                    match kind {
                        CombinatorKind::OptionMap => {
                            // dst = Some(mapper(payload))
                            let mapped = next_reg;
                            next_reg += 1;
                            new_code.push(RegInstr::CallClosure {
                                dst: mapped,
                                closure: other,
                                args: vec![payload],
                                mut_args: Vec::new(),
                            });
                            new_code.push(RegInstr::MakeSome { dst, value: mapped });
                        }
                        CombinatorKind::OptionAndThen => {
                            // dst = mapper(payload) (already an Option)
                            new_code.push(RegInstr::CallClosure {
                                dst,
                                closure: other,
                                args: vec![payload],
                                mut_args: Vec::new(),
                            });
                        }
                        CombinatorKind::OptionUnwrapOr => {
                            // dst = payload (the Some value)
                            new_code.push(RegInstr::Move { dst, src: payload });
                        }
                        _ => unreachable!(),
                    }
                    // jump to join (after this combinator)
                    fixups.push((new_code.len(), Fix::JoinAfter(i)));
                    new_code.push(RegInstr::Jump { target: 0 });
                    // --- None arm ---
                    let none_ip = new_code.len();
                    match kind {
                        CombinatorKind::OptionMap | CombinatorKind::OptionAndThen => {
                            new_code.push(RegInstr::LoadNone { dst });
                        }
                        CombinatorKind::OptionUnwrapOr => {
                            new_code.push(RegInstr::Move { dst, src: other });
                        }
                        _ => unreachable!(),
                    }
                    // (falls through to join)
                    if let RegInstr::MatchOption {
                        some_ip: s,
                        none_ip: nn,
                        ..
                    } = &mut new_code[match_pos]
                    {
                        *s = some_ip;
                        *nn = none_ip;
                    }
                }
                CombinatorKind::ResultMap
                | CombinatorKind::ResultAndThen
                | CombinatorKind::ResultUnwrapOr => {
                    let payload = next_reg;
                    next_reg += 1;
                    let match_pos = new_code.len();
                    new_code.push(RegInstr::MatchResult {
                        src: operand,
                        ok_ip: 0,
                        err_ip: 0,
                    });
                    // --- Ok arm ---
                    let ok_ip = new_code.len();
                    new_code.push(RegInstr::UnwrapVariantValue {
                        dst: payload,
                        src: operand,
                        expected: "Ok".to_string(),
                    });
                    match kind {
                        CombinatorKind::ResultMap => {
                            // dst = Ok(mapper(payload))
                            let mapped = next_reg;
                            next_reg += 1;
                            new_code.push(RegInstr::CallClosure {
                                dst: mapped,
                                closure: other,
                                args: vec![payload],
                                mut_args: Vec::new(),
                            });
                            new_code.push(RegInstr::MakeVariant {
                                dst,
                                layout: result_ok_layout(),
                                fields: vec![("value".to_string(), mapped)],
                            });
                        }
                        CombinatorKind::ResultAndThen => {
                            // dst = mapper(payload) (already a Result)
                            new_code.push(RegInstr::CallClosure {
                                dst,
                                closure: other,
                                args: vec![payload],
                                mut_args: Vec::new(),
                            });
                        }
                        CombinatorKind::ResultUnwrapOr => {
                            new_code.push(RegInstr::Move { dst, src: payload });
                        }
                        _ => unreachable!(),
                    }
                    fixups.push((new_code.len(), Fix::JoinAfter(i)));
                    new_code.push(RegInstr::Jump { target: 0 });
                    // --- Err arm ---
                    let err_ip = new_code.len();
                    match kind {
                        CombinatorKind::ResultMap | CombinatorKind::ResultAndThen => {
                            // Heap Err rebuild ⇒ native Bail (Slice-1 cold-arm path).
                            new_code.push(RegInstr::RuntimeError {
                                message: String::new(),
                            });
                        }
                        CombinatorKind::ResultUnwrapOr => {
                            // dst = default (scalar; no heap build).
                            new_code.push(RegInstr::Move { dst, src: other });
                        }
                        _ => unreachable!(),
                    }
                    if let RegInstr::MatchResult {
                        ok_ip: o,
                        err_ip: e,
                        ..
                    } = &mut new_code[match_pos]
                    {
                        *o = ok_ip;
                        *e = err_ip;
                    }
                }
            }
            continue;
        }
        // Copy-through, remapping jump/match targets (same as the SR passes).
        match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::JoinAfter(orig) => {
                // The instruction AFTER the combinator at `orig` (its successor); for
                // the last instruction this is one-past-the-end (handled below).
                let target = if orig + 1 < code.len() {
                    index_map[orig + 1]
                } else {
                    new_code.len()
                };
                if let RegInstr::Jump { target: dst } = &mut new_code[pos] {
                    *dst = target;
                }
            }
            Fix::Match { a, b } => {
                let (na, nb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = na;
                        *err_ip = nb;
                    }
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = na;
                        *none_ip = nb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = na;
                        *else_ip = nb;
                    }
                    _ => {}
                }
            }
        }
    }
    // Inverse ip-map: every fragment of a combinator at original index `i` maps back
    // to `i` (a deopt inside the expanded fragment resumes by re-running the original
    // combinator on the interpreter); copy-through maps one-to-one.
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}

/// OSR × scalar replacement (string length-law folding): a non-escaping string register `s`
/// inside the loop region `[header, exit)` whose EVERY use is `String.len(s)`,
/// another foldable producer's operand, or a `Move` to a foldable register, can
/// have its allocation DELETED — every `String.len(s)` is replaced by arithmetic
/// on operand lengths, and the producer instruction(s) are dropped. This stays
/// READ-ONLY (it removes allocations, never performs a heap write — Exec Spec
/// the transactional fallback contract holds), so a length-only string loop becomes pure-scalar and OSRs.
///
/// VERIFIED length laws (against the interpreter's exact `String` semantics):
/// - `String.len` is the BYTE length (`str::len`, see [`RegIntrinsic::StringLen`]).
/// - `String.concat` is byte concatenation (see [`RegInstr::StringConcat`]), so
///   `len(concat(a,b)) = len(a) + len(b)` exactly, REGARDLESS of encoding.
/// - `String.from_int(k)` is `i64::to_string()`: ASCII decimal digits with a
///   leading `-` for negatives, `"0"` for zero, all bytes 1-wide. Its byte length
///   is the decimal-digit count (`+1` for the sign when `k < 0`), computed natively
///   by a forward branch ladder that handles `0`, negatives, and `i64::MIN` (which
///   cannot be negated) by comparing `k` directly against ± powers of ten.
/// - `String.slice(s, start, n)` (see [`string_slice_range`]) clamps to CHAR
///   boundaries in BYTE units: `bs = clamp_cb(s, max(start,0))`,
///   `be = clamp_cb(s, min(bs + max(n,0), len(s)))`, result byte-length `be - bs`.
///   The char-boundary clamp depends on the actual bytes of `s`, so the law is only
///   provable when `s` is ASCII (every byte is a boundary ⇒ clamp is identity):
///   `len = min(min(max(start,0), L) + max(n,0), L) - min(max(start,0), L)` with
///   `L = len(s)`. A slice of a NON-ASCII (unprovably-ASCII) string ⇒ NOT foldable.
/// - `LoadString` / string `Move`: constant byte length / alias of the source.
///
/// Conservative bails (when unsure REJECT ⇒ no OSR, never unsound): any escaping
/// string use (stored / returned / captured / compared / passed to a non-`len`
/// intrinsic / live at a loop boundary); a `String.slice` of an unprovably-ASCII
/// string; a `String.len` whose source is not a fully-foldable producer. A leaf
/// non-foldable `String.len` is simply left un-folded — the `StringLen` host helper
/// IS native-subset (a plain `String.len` loop OSRs), so it runs as a host call rather
/// than blocking OSR; this pass only declines to FOLD it. `RegFootprint::All` ⇒ bail.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→
/// original `ip_map` discipline as the sibling region passes. Identity (no
/// foldable `String.len`) ⇒ code unchanged with an identity ip-map.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_string_length_fold_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // The only thing this pass targets is an in-region `String.len`. Without one,
    // there is nothing to fold ⇒ identity (plain OSR, byte-for-byte the old path).
    // The length-query *classification* reads the central registry's
    // `string_fold_role`; the single-arg shape check stays here.
    let is_string_len = |instr: &RegInstr| {
        matches!(
            instr,
            RegInstr::CallIntrinsic { intrinsic, args, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, args, .. }
                if args.len() == 1
                    && intrinsic_descriptor(*intrinsic).string_fold_role
                        == Some(StringFoldRole::LengthQuery)
        )
    };
    if !(header..exit).any(|i| is_string_len(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // Classify each in-region string producer. A producer register is a candidate
    // iff it is defined EXACTLY ONCE in-region by a foldable op and never defined
    // out-of-region. `ascii` records whether the produced string is provably ASCII
    // (needed only for the slice length law).
    #[derive(Clone)]
    enum Producer {
        Literal {
            len: i64,
            ascii: bool,
        },
        FromInt {
            src: usize,
        },
        Concat {
            left: usize,
            right: usize,
        },
        Slice {
            src: usize,
            start: usize,
            len: usize,
        },
        Alias {
            src: usize,
        }, // string `Move`
    }
    let mut producer: Vec<Option<Producer>> = vec![None; n_regs];
    let mut multiply_defined = vec![false; n_regs];

    let slice_args = |args: &[usize]| -> Option<(usize, usize, usize)> {
        if args.len() == 3 {
            Some((args[0], args[1], args[2]))
        } else {
            None
        }
    };
    for i in header..exit {
        let dst_prod: Option<(usize, Producer)> = match &code[i] {
            RegInstr::LoadString { dst, value } => Some((
                *dst,
                Producer::Literal {
                    len: value.len() as i64,
                    ascii: value.is_ascii(),
                },
            )),
            RegInstr::StringConcat { dst, left, right } => Some((
                *dst,
                Producer::Concat {
                    left: *left,
                    right: *right,
                },
            )),
            // Foldable string *producer* intrinsics — recognized via the central
            // registry's `string_fold_role`; the per-role operand extraction and the
            // length laws stay here. (`String.from_int` is recognized only in the
            // untyped `CallIntrinsic` form, exactly as before; `String.slice` in both
            // the untyped and typed forms.)
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            } if intrinsic_descriptor(*intrinsic).string_fold_role
                == Some(StringFoldRole::ProducerFromInt)
                && args.len() == 1 =>
            {
                Some((*dst, Producer::FromInt { src: args[0] }))
            }
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            }
            | RegInstr::CallTypedIntrinsic {
                dst,
                intrinsic,
                args,
                ..
            } if intrinsic_descriptor(*intrinsic).string_fold_role
                == Some(StringFoldRole::ProducerSlice) =>
            {
                slice_args(args)
                    .map(|(src, start, len)| (*dst, Producer::Slice { src, start, len }))
            }
            // A `Move` whose src is a (candidate) string is a potential alias; we
            // only mark it a producer if the src is itself a string producer (below,
            // after all defs are seen). Record it provisionally as an Alias.
            RegInstr::Move { dst, src } => Some((*dst, Producer::Alias { src: *src })),
            _ => None,
        };
        if let Some((dst, prod)) = dst_prod {
            if dst >= n_regs {
                return None;
            }
            if analysis.region_def_count(dst)? > 1 {
                multiply_defined[dst] = true;
            }
            producer[dst] = Some(prod);
        }
    }

    // A register defined out-of-region, or defined more than once in-region, cannot
    // be a sound single-producer string ⇒ drop it from the candidate set. (Out-of-
    // region defs would change the value the loop observes.)
    analysis.mark_external_writes(code, &mut multiply_defined)?;
    for r in 0..n_regs {
        if multiply_defined[r] {
            producer[r] = None;
        }
    }

    // Resolve foldability with a fixpoint: a producer is FOLDABLE iff all the
    // operands its length law needs are themselves foldable (or, for slice/from_int,
    // satisfy the ASCII / integer requirements). `Move`/`Concat`/`Slice` of a non-
    // foldable string is itself non-foldable. `ascii` is tracked alongside.
    let mut foldable = vec![false; n_regs];
    let mut ascii = vec![false; n_regs];
    let mut changed = true;
    while changed {
        changed = false;
        for r in 0..n_regs {
            if foldable[r] {
                continue;
            }
            let Some(prod) = &producer[r] else { continue };
            let (ok, is_ascii) = match prod {
                Producer::Literal { ascii, .. } => (true, *ascii),
                // `from_int` is always ASCII; its operand is an Int (not a string),
                // so no string dependency.
                Producer::FromInt { .. } => (true, true),
                Producer::Concat { left, right } => {
                    let ok =
                        *left < n_regs && *right < n_regs && foldable[*left] && foldable[*right];
                    (ok, ok && ascii[*left] && ascii[*right])
                }
                Producer::Slice { src, .. } => {
                    // The slice length law needs the SOURCE to be provably ASCII
                    // (so the char-boundary clamp is the identity). A slice of an
                    // ASCII string is itself ASCII.
                    let ok = *src < n_regs && foldable[*src] && ascii[*src];
                    (ok, ok)
                }
                Producer::Alias { src } => {
                    let ok = *src < n_regs && foldable[*src];
                    (ok, ok && ascii[*src])
                }
            };
            if ok && !foldable[r] {
                foldable[r] = true;
                ascii[r] = is_ascii;
                changed = true;
            }
        }
    }

    // STRING = registers we (provisionally) treat as string-valued and intend to
    // dissolve: every foldable producer register. For soundness we now require that
    // EVERY use of a STRING register is itself foldable — i.e. an operand of another
    // foldable producer, a `Move` to a foldable register, or a `String.len`. Any
    // other in-region use, or ANY out-of-region use, ESCAPES ⇒ that register cannot
    // be dissolved. We don't need partial dissolution: if a `String.len` source is
    // foldable but the foldable register also escapes elsewhere, the producer must
    // stay live, so we cannot delete it — bail that whole register out of `foldable`
    // and re-resolve, then finally require every in-region `String.len` to be
    // foldable (else bail the pass: a live `StringLen` is not native-subset).
    //
    // Compute "escapes": a foldable register read by an instruction that is neither
    // (a) a foldable producer consuming it as a string operand, nor (b) a
    // `String.len`. Iterate to a fixpoint (dropping an escaping register can make a
    // consumer's operand non-foldable, propagating).
    loop {
        let mut escaped = vec![false; n_regs];
        // Out-of-region reads of any foldable register ⇒ escape.
        analysis.mark_external_reads_touching(code, &foldable, &mut escaped)?;
        // In-region uses: each read of a foldable register must be a sanctioned
        // string consumer.
        for i in header..exit {
            match &code[i] {
                // Sanctioned: foldable producers consuming foldable string operands.
                RegInstr::StringConcat { dst, left, right } if foldable[*dst] => {
                    // operands consumed as strings — fine (handled by being foldable)
                    let _ = (left, right);
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic,
                    args,
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic,
                    args,
                    ..
                } if foldable[*dst]
                    && args.len() == 3
                    && intrinsic_descriptor(*intrinsic).string_fold_role
                        == Some(StringFoldRole::ProducerSlice) =>
                {
                    // start/len (args[1], args[2]) are Int operands, not strings.
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {
                    // alias of a foldable string into a foldable register — fine.
                }
                // Sanctioned: the query itself.
                _ if is_string_len(&code[i]) => {
                    // its single arg is consumed as a string — fine.
                }
                // Any other instruction: any foldable register it reads escapes.
                other => match instr_read_regs(other) {
                    RegFootprint::Some(rs) => {
                        for r in rs {
                            if r < n_regs && foldable[r] {
                                escaped[r] = true;
                            }
                        }
                    }
                    RegFootprint::All => return None,
                },
            }
        }
        if !escaped.iter().any(|&e| e) {
            break;
        }
        // Drop escaped registers and re-resolve foldability (a dropped operand can
        // un-fold its consumers).
        for r in 0..n_regs {
            if escaped[r] {
                foldable[r] = false;
                ascii[r] = false;
            }
        }
        let mut changed2 = true;
        while changed2 {
            changed2 = false;
            for r in 0..n_regs {
                if !foldable[r] {
                    continue;
                }
                let Some(prod) = &producer[r] else { continue };
                let still = match prod {
                    Producer::Literal { .. } | Producer::FromInt { .. } => true,
                    Producer::Concat { left, right } => foldable[*left] && foldable[*right],
                    Producer::Slice { src, .. } => foldable[*src] && ascii[*src],
                    Producer::Alias { src } => foldable[*src],
                };
                if !still {
                    foldable[r] = false;
                    ascii[r] = false;
                    changed2 = true;
                }
            }
        }
    }

    // A `String.len` whose source is NOT foldable is left UNCHANGED: the real
    // `StringLen` intrinsic IS in the native subset (it lowers to the host `string_len`
    // helper), so a surviving `StringLen` does NOT block OSR — only foldable-source
    // lengths are dissolved below (the rewrite arm guards on `foldable[args[0]]`). This
    // is what lets an EXPANDED-path loop (e.g. a two-armed `Result<String,_>` whose arms
    // call `String.len` on a live heap payload) OSR instead of declining the whole loop.

    // Nothing dissolvable after escape analysis ⇒ identity (no fold): every in-region
    // `String.len` had a non-foldable source (all left in place), or there was no
    // foldable producer at all.
    if !foldable.iter().any(|&f| f) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }

    // Allocate a fresh Int `len_reg` per foldable string register: it will hold that
    // string's byte length, computed at the producer site. `from_int` needs scratch
    // registers for its digit-count ladder; reserve them lazily.
    let mut len_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for r in 0..n_regs {
        if foldable[r] {
            len_reg[r] = next_reg;
            next_reg += 1;
        }
    }

    // Emit the byte-length computation for `from_int(src)` into `out`, appending to
    // `out_code` (using absolute new-code indices for intra-fragment forward jumps,
    // back-patched after emission). Allocates scratch registers from `next_reg`.
    //
    // Matches `i64::to_string().len()`: `1` for `0`; `digits(k)` for `k > 0`;
    // `1 + digits(|k|)` for `k < 0`. Avoids negating `i64::MIN` by comparing `k`
    // directly against negative powers of ten.
    fn emit_from_int_len(out_code: &mut Vec<RegInstr>, out: usize, k: usize, next_reg: &mut usize) {
        // result accumulator is `out`. Strategy:
        //   if k >= 0:  out = pos_digits(k)
        //   else:       out = 1 + neg_digits(k)
        // pos_digits(k): 1 + count of thresholds {10,100,...,1e18} that k >= t.
        // neg_digits(k): 1 + count of thresholds {-10,...,-1e18} that k <= t.
        // Both are computed branchlessly-by-cascade using comparisons that write a
        // Bool then a conditional add — but Bool isn't Int-addable, so we instead use
        // a forward branch ladder: for k >= 0, test largest threshold first and on
        // the first hit LoadInt the digit count and Jump to the merge.
        let zero = *next_reg;
        *next_reg += 1;
        let thr = *next_reg;
        *next_reg += 1;
        // Positive powers of ten (10^1 .. 10^18); 10^19 overflows i64, so 19-digit
        // positives (>= 10^18) are the final else of the positive ladder.
        const POW10: [i64; 18] = [
            10,
            100,
            1_000,
            10_000,
            100_000,
            1_000_000,
            10_000_000,
            100_000_000,
            1_000_000_000,
            10_000_000_000,
            100_000_000_000,
            1_000_000_000_000,
            10_000_000_000_000,
            100_000_000_000_000,
            1_000_000_000_000_000,
            10_000_000_000_000_000,
            100_000_000_000_000_000,
            1_000_000_000_000_000_000,
        ];
        out_code.push(RegInstr::LoadInt {
            dst: zero,
            value: 0,
        });
        // Branch: if k < 0 jump to neg-ladder.
        let neg_start_patch = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: k,
            rhs: zero,
            op: RegIntCompare::Less,
            expected: true,
            target: 0, // back-patched
        });
        // --- positive (and zero) ladder: emit largest threshold first ---
        // For d in 19..=2: if k >= 10^(d-1) -> out = d; Jump merge.
        let mut to_merge: Vec<usize> = Vec::new();
        for d in (2..=19usize).rev() {
            let t = POW10[d - 2];
            out_code.push(RegInstr::LoadInt { dst: thr, value: t });
            // if k >= t -> set out=d, jump merge
            let skip_patch = out_code.len();
            out_code.push(RegInstr::JumpIfIntCompare {
                lhs: k,
                rhs: thr,
                op: RegIntCompare::GreaterEqual,
                expected: false, // if NOT (k>=t) skip the assignment
                target: 0,       // back-patched to the next threshold test
            });
            out_code.push(RegInstr::LoadInt {
                dst: out,
                value: d as i64,
            });
            to_merge.push(out_code.len());
            out_code.push(RegInstr::Jump { target: 0 }); // -> merge
            // back-patch skip to here (next threshold test / final else)
            let here = out_code.len();
            if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[skip_patch] {
                *target = here;
            }
        }
        // positive final else: k in [0,10) -> 1 digit.
        out_code.push(RegInstr::LoadInt { dst: out, value: 1 });
        to_merge.push(out_code.len());
        out_code.push(RegInstr::Jump { target: 0 }); // -> merge
        // --- negative ladder ---
        let neg_start = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[neg_start_patch] {
            *target = neg_start;
        }
        // out (magnitude digits) then +1 for sign. For d in 19..=2: if k <= -10^(d-1)
        // -> magnitude d. Final else -> magnitude 1.
        let mut neg_to_add: Vec<usize> = Vec::new();
        for d in (2..=19usize).rev() {
            let t = -POW10[d - 2];
            out_code.push(RegInstr::LoadInt { dst: thr, value: t });
            let skip_patch = out_code.len();
            out_code.push(RegInstr::JumpIfIntCompare {
                lhs: k,
                rhs: thr,
                op: RegIntCompare::LessEqual,
                expected: false, // if NOT (k<=t) skip
                target: 0,
            });
            out_code.push(RegInstr::LoadInt {
                dst: out,
                value: d as i64,
            });
            neg_to_add.push(out_code.len());
            out_code.push(RegInstr::Jump { target: 0 }); // -> add-sign
            let here = out_code.len();
            if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[skip_patch] {
                *target = here;
            }
        }
        out_code.push(RegInstr::LoadInt { dst: out, value: 1 });
        // fallthrough to add-sign
        let add_sign = out_code.len();
        for p in neg_to_add {
            if let RegInstr::Jump { target } = &mut out_code[p] {
                *target = add_sign;
            }
        }
        // out = out + 1 (sign byte). Reuse `thr` as the constant 1.
        out_code.push(RegInstr::LoadInt { dst: thr, value: 1 });
        out_code.push(RegInstr::AddInt {
            dst: out,
            lhs: out,
            rhs: thr,
        });
        // fallthrough to merge
        let merge = out_code.len();
        for p in to_merge {
            if let RegInstr::Jump { target } = &mut out_code[p] {
                *target = merge;
            }
        }
    }

    // Emit the slice byte-length law for an ASCII source into `out_code`, writing
    // `out`. `l_src` is the source's length register; `start`,`len` the Int operands.
    //   sc = max(start,0); s_clamp = min(sc, L); ec = s_clamp + max(len,0);
    //   e_clamp = min(ec, L); out = e_clamp - s_clamp.
    fn emit_slice_len(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        l_src: usize,
        start: usize,
        len: usize,
        next_reg: &mut usize,
    ) {
        let zero = *next_reg;
        let sc = *next_reg + 1;
        let sclamp = *next_reg + 2;
        let lc = *next_reg + 3;
        let ec = *next_reg + 4;
        *next_reg += 5;
        out_code.push(RegInstr::LoadInt {
            dst: zero,
            value: 0,
        });
        // sc = max(start, 0)
        emit_max(out_code, sc, start, zero, next_reg);
        // s_clamp = min(sc, L)
        emit_min(out_code, sclamp, sc, l_src, next_reg);
        // lc = max(len, 0)
        emit_max(out_code, lc, len, zero, next_reg);
        // ec = s_clamp + lc
        out_code.push(RegInstr::AddInt {
            dst: ec,
            lhs: sclamp,
            rhs: lc,
        });
        // e_clamp = min(ec, L)  -> reuse `ec`
        emit_min(out_code, ec, ec, l_src, next_reg);
        // out = e_clamp - s_clamp
        out_code.push(RegInstr::SubInt {
            dst: out,
            lhs: ec,
            rhs: sclamp,
        });
    }

    // out = max(a, b): if a >= b -> out=a else out=b (forward branch).
    fn emit_max(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        a: usize,
        b: usize,
        _next_reg: &mut usize,
    ) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::GreaterEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }

    // out = min(a, b): if a <= b -> out=a else out=b.
    fn emit_min(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        a: usize,
        b: usize,
        _next_reg: &mut usize,
    ) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::LessEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }

    // Rewrite the whole stream. In-region foldable producers are replaced by the
    // length computation writing `len_reg[dst]` (the original heap allocation is
    // DELETED); each `String.len(s)` becomes `Move(dst, len_reg[s])`. Everything
    // else is copied through, remapping inter-instruction jump/match targets through
    // `index_map`. Intra-fragment jumps emitted by the helpers above already carry
    // absolute new-code positions (back-patched at emit time) and must NOT be
    // remapped, so producer fragments are spliced AFTER recording `index_map[i]` and
    // the fragment's internal jumps are left untouched.
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
        MapGet { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        // A foldable producer in-region: emit its length computation, drop the alloc.
        if region {
            let folded = match instr {
                RegInstr::LoadString { dst, .. } if foldable[*dst] => {
                    if let Some(Producer::Literal { len, .. }) = &producer[*dst] {
                        new_code.push(RegInstr::LoadInt {
                            dst: len_reg[*dst],
                            value: *len,
                        });
                    }
                    true
                }
                RegInstr::StringConcat { dst, .. } if foldable[*dst] => {
                    if let Some(Producer::Concat { left, right }) = &producer[*dst] {
                        new_code.push(RegInstr::AddInt {
                            dst: len_reg[*dst],
                            lhs: len_reg[*left],
                            rhs: len_reg[*right],
                        });
                    }
                    true
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {
                    new_code.push(RegInstr::Move {
                        dst: len_reg[*dst],
                        src: len_reg[*src],
                    });
                    true
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::StringFromInt,
                    ..
                } if foldable[*dst] => {
                    if let Some(Producer::FromInt { src }) = &producer[*dst] {
                        emit_from_int_len(&mut new_code, len_reg[*dst], *src, &mut next_reg);
                    }
                    true
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::StringSlice,
                    ..
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::StringSlice,
                    ..
                } if foldable[*dst] => {
                    if let Some(Producer::Slice { src, start, len }) = &producer[*dst] {
                        emit_slice_len(
                            &mut new_code,
                            len_reg[*dst],
                            len_reg[*src],
                            *start,
                            *len,
                            &mut next_reg,
                        );
                    }
                    true
                }
                _ if is_string_len(instr)
                    && matches!(instr,
                        RegInstr::CallIntrinsic { args, .. }
                        | RegInstr::CallTypedIntrinsic { args, .. }
                        if args[0] < n_regs && foldable[args[0]]) =>
                {
                    let (dst, src) = match instr {
                        RegInstr::CallIntrinsic { dst, args, .. }
                        | RegInstr::CallTypedIntrinsic { dst, args, .. } => (*dst, args[0]),
                        _ => unreachable!(),
                    };
                    new_code.push(RegInstr::Move {
                        dst,
                        src: len_reg[src],
                    });
                    true
                }
                _ => false,
            };
            if folded {
                continue;
            }
        }
        // Copy-through, remapping jump/match targets to the new index space.
        match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::MapGet {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = sa;
                        *err_ip = sb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = sa;
                        *else_ip = sb;
                    }
                    _ => {}
                }
            }
            Fix::MapGet { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    _ => {}
                }
            }
        }
    }
    // Inverse ip-map: every fragment instruction maps back to the producer's
    // original index (`String.len` → its own index; copy-through 1:1).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}

/// OSR × scalar replacement for BYTES LENGTH-LAW FOLDING (the read-only sibling of
/// [`native_string_length_fold_in_region`]): dissolve a non-escaping Bytes value built
/// ONLY to be measured (`Bytes.len` of `Bytes.slice`/`Bytes.from_string`/`Move`/a
/// constant-length source) into byte-length arithmetic, DELETING the now-dead Bytes
/// allocation. Read-only (no heap write; the transactional fallback contract holds), turning a length-only
/// Bytes loop into pure-scalar Int code the native subset accepts.
///
/// Why a separate pass from the String fold: Bytes are RAW bytes with NO char/grapheme
/// boundary, so the slice length law is the EXACT clamp arithmetic of [`bytes_slice`]
/// with NO ASCII gate — verified identical: `bytes_slice` does `s'=max(start,0); if
/// s'>=L {0} else { min(s'+max(len,0), L) - s' }`, which is precisely the
/// overflow-free `emit_slice_len` law (`sc=max(start,0); if sc>=L {0} else
/// {min(max(len,0),L-sc)}`). `Bytes.len` is
/// `value.len()` (raw byte count) and `Bytes.from_string(s)` is `s.as_bytes().len()`,
/// so a from-string's byte length equals the source String's byte length.
///
/// A length source may be (a) an in-region foldable Bytes producer, (b) ANY register
/// whose byte length resolves to a COMPILE-TIME CONSTANT through its unique global def,
/// or (c) a dynamic Bytes input with no in-region definition. Constants are materialized
/// as in-region `LoadInt`s. Dynamic inputs retain a validating `Bytes.len` at each folded
/// slice site; whole-function loop memoization can make that helper activation-local
/// O(1) work. The constant trace requires every register in the chain to have exactly one
/// whole-function definition.
///
/// Conservative bail: any escaping foldable Bytes register or unmodelled read/write
/// footprint ⇒ `None` (no fold). Unrelated direct dynamic `Bytes.len` calls remain
/// ordinary helpers.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_bytes_length_fold_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    let is_bytes_len = |instr: &RegInstr| {
        matches!(
            instr,
            RegInstr::CallIntrinsic { intrinsic, args, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, args, .. }
                if args.len() == 1
                    && intrinsic_descriptor(*intrinsic).bytes_fold_role
                        == Some(BytesFoldRole::LengthQuery)
        )
    };
    // No in-region Bytes.len ⇒ nothing to fold ⇒ identity (plain OSR, byte-for-byte).
    if !(header..exit).any(|i| is_bytes_len(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;
    // Preserve this pass's old whole-function conservatism: an unknown write footprint
    // makes the unique-def tracer unverifiable, so the fold does not fire.
    analysis.global_def_counts.as_ref()?;

    // --- Constant byte-length tracer -------------------------------------------------
    // `const_len[r] = Some(L)` iff register `r` is defined EXACTLY ONCE in the whole
    // function by a chain of immutable constant ops whose byte length is the compile-
    // time constant `L`. Multiply-defined (or `All`-footprint) registers get `None`.
    //
    //   LoadString lit                 -> lit.as_bytes().len()      (== lit.len())
    //   Bytes.from_string(s)           -> const_len[s]
    //   Bytes.slice(s, start, len)     -> clamp(const_len[s], start, len)  [const args]
    //   Move(dst, src)                 -> const_len[src]
    //
    // `start`/`len` for a constant slice must themselves be constant Ints (LoadInt) for
    // the result to be a compile-time constant. Anything else ⇒ not a constant.
    // Single-def constant-Int values (for slice start/len constant args).
    let int_const = |r: usize| -> Option<i64> {
        let ip = analysis.single_def_ip_of(code, r)?;
        match &code[ip] {
            RegInstr::LoadInt { dst, value } if *dst == r => Some(*value),
            _ => None,
        }
    };
    // Resolve a register's constant byte length with a depth-bounded trace over its
    // unique def. Depth bound guards against any pathological chain; immutability is
    // guaranteed by `analysis.global_def_count(r) == 1` at each hop.
    fn const_byte_len(
        r: usize,
        depth: usize,
        code: &[RegInstr],
        n_regs: usize,
        analysis: &NativeRegionAnalysis,
        int_const: &dyn Fn(usize) -> Option<i64>,
    ) -> Option<i64> {
        if depth == 0 || r >= n_regs || analysis.global_def_count(r)? != 1 {
            return None;
        }
        let def = &code[analysis.single_def_ip_of(code, r)?];
        match def {
            RegInstr::LoadString { value, .. } => Some(value.len() as i64),
            RegInstr::Move { src, .. } => {
                const_byte_len(*src, depth - 1, code, n_regs, analysis, int_const)
            }
            RegInstr::CallIntrinsic {
                intrinsic, args, ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic, args, ..
            } => {
                match intrinsic_descriptor(*intrinsic).bytes_fold_role {
                    Some(BytesFoldRole::ProducerFromString) if args.len() == 1 => {
                        const_byte_len(args[0], depth - 1, code, n_regs, analysis, int_const)
                    }
                    Some(BytesFoldRole::ProducerSlice) if args.len() == 3 => {
                        let l =
                            const_byte_len(args[0], depth - 1, code, n_regs, analysis, int_const)?;
                        let start = int_const(args[1])?;
                        let len = int_const(args[2])?;
                        // Mirror `bytes_slice` exactly on the constant operands.
                        let sc = start.max(0);
                        if sc >= l {
                            return Some(0);
                        }
                        let lc = len.max(0);
                        let end = sc.saturating_add(lc).min(l);
                        Some(end - sc)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
    let mut const_len: Vec<Option<i64>> = vec![None; n_regs];
    for r in 0..n_regs {
        const_len[r] = const_byte_len(r, 32, code, n_regs, &analysis, &int_const);
    }

    // --- In-region foldable Bytes producers ------------------------------------------
    #[derive(Clone)]
    enum BProducer {
        FromString {
            src: usize,
        },
        Slice {
            src: usize,
            start: usize,
            len: usize,
        },
        Alias {
            src: usize,
        },
    }
    let mut producer: Vec<Option<BProducer>> = vec![None; n_regs];
    let mut multiply_defined = vec![false; n_regs];
    for i in header..exit {
        let dst_prod: Option<(usize, BProducer)> = match &code[i] {
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            }
            | RegInstr::CallTypedIntrinsic {
                dst,
                intrinsic,
                args,
                ..
            } if intrinsic_descriptor(*intrinsic).bytes_fold_role
                == Some(BytesFoldRole::ProducerFromString)
                && args.len() == 1 =>
            {
                Some((*dst, BProducer::FromString { src: args[0] }))
            }
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            }
            | RegInstr::CallTypedIntrinsic {
                dst,
                intrinsic,
                args,
                ..
            } if intrinsic_descriptor(*intrinsic).bytes_fold_role
                == Some(BytesFoldRole::ProducerSlice)
                && args.len() == 3 =>
            {
                Some((
                    *dst,
                    BProducer::Slice {
                        src: args[0],
                        start: args[1],
                        len: args[2],
                    },
                ))
            }
            RegInstr::Move { dst, src } => Some((*dst, BProducer::Alias { src: *src })),
            _ => None,
        };
        if let Some((dst, prod)) = dst_prod {
            if dst >= n_regs {
                return None;
            }
            if analysis.region_def_count(dst)? > 1 {
                multiply_defined[dst] = true;
            }
            producer[dst] = Some(prod);
        }
    }
    // A register defined out-of-region or multiply-defined in-region is not a sound
    // single-producer in-region Bytes value (its out-of-region def may differ). Drop
    // it — but it can still serve as a CONSTANT-length source via `const_len`.
    analysis.mark_external_writes(code, &mut multiply_defined)?;
    for r in 0..n_regs {
        if multiply_defined[r] {
            producer[r] = None;
        }
    }

    // A loop-invariant dynamic Bytes input can also provide its length without
    // materializing a slice. Keep this deliberately narrow: only operands already
    // proven to be Bytes by `Bytes.len`/`Bytes.slice`, and only registers with no
    // in-region definition. The validating `Bytes.len` helper remains at the original
    // slice site, preserving the first possible failure point; the later native
    // memoization pass may cache it when the surrounding loop proves invariance.
    let mut dynamic_len_source = vec![false; n_regs];
    for instr in &code[header..exit] {
        let (role, args) = match instr {
            RegInstr::CallIntrinsic {
                intrinsic, args, ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic, args, ..
            } => (intrinsic_descriptor(*intrinsic).bytes_fold_role, args),
            _ => continue,
        };
        if role == Some(BytesFoldRole::ProducerSlice) && !args.is_empty() {
            let src = args[0];
            if src < n_regs && const_len[src].is_none() && analysis.region_def_count(src)? == 0 {
                dynamic_len_source[src] = true;
            }
        }
    }

    // A length source resolves iff it is an in-region foldable producer, has a
    // constant byte length, or is a loop-invariant dynamic Bytes input.
    let resolves = |r: usize, foldable: &[bool]| -> bool {
        r < n_regs && (foldable[r] || const_len[r].is_some() || dynamic_len_source[r])
    };
    let mut foldable = vec![false; n_regs];
    let mut changed = true;
    while changed {
        changed = false;
        for r in 0..n_regs {
            if foldable[r] {
                continue;
            }
            let ok = match &producer[r] {
                Some(BProducer::FromString { src }) => resolves(*src, &foldable),
                Some(BProducer::Slice { src, .. }) => resolves(*src, &foldable),
                Some(BProducer::Alias { src }) => resolves(*src, &foldable),
                None => false,
            };
            if ok {
                foldable[r] = true;
                changed = true;
            }
        }
    }

    // Escape analysis: every use of a foldable (to-be-dissolved) Bytes register must be
    // a sanctioned Bytes consumer — a foldable producer operand, a `Move` to a foldable
    // register, or a `Bytes.len`. Any other read (in OR out of region) escapes; drop it
    // and re-resolve to a fixpoint. (Constant-length sources are NOT dissolved, so their
    // other uses — e.g. the still-live `data` slice arg — do not need to be sanctioned.)
    loop {
        let mut escaped = vec![false; n_regs];
        let note_reads = |rs: &[usize], escaped: &mut Vec<bool>| {
            for &r in rs {
                if r < n_regs && foldable[r] {
                    escaped[r] = true;
                }
            }
        };
        analysis.mark_external_reads_touching(code, &foldable, &mut escaped)?;
        for i in header..exit {
            match &code[i] {
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic,
                    args,
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic,
                    args,
                    ..
                } if foldable[*dst]
                    && intrinsic_descriptor(*intrinsic).bytes_fold_role
                        == Some(BytesFoldRole::ProducerFromString)
                    && args.len() == 1 => {}
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic,
                    args,
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic,
                    args,
                    ..
                } if foldable[*dst]
                    && intrinsic_descriptor(*intrinsic).bytes_fold_role
                        == Some(BytesFoldRole::ProducerSlice)
                    && args.len() == 3 =>
                {
                    // args[0] is the (foldable or constant) source consumed as Bytes;
                    // args[1]/args[2] are Int operands. Fine.
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {}
                _ if is_bytes_len(&code[i]) => {}
                other => match instr_read_regs(other) {
                    RegFootprint::Some(rs) => note_reads(&rs, &mut escaped),
                    RegFootprint::All => return None,
                },
            }
        }
        if !escaped.iter().any(|&e| e) {
            break;
        }
        for r in 0..n_regs {
            if escaped[r] {
                foldable[r] = false;
            }
        }
        let mut c2 = true;
        while c2 {
            c2 = false;
            for r in 0..n_regs {
                if !foldable[r] {
                    continue;
                }
                let still = match &producer[r] {
                    Some(BProducer::FromString { src })
                    | Some(BProducer::Slice { src, .. })
                    | Some(BProducer::Alias { src }) => resolves(*src, &foldable),
                    None => false,
                };
                if !still {
                    foldable[r] = false;
                    c2 = true;
                }
            }
        }
    }

    // Unresolved direct `Bytes.len` reads remain ordinary native helpers. This lets the
    // pass dissolve an independent non-escaping slice without making unrelated dynamic
    // length reads a precondition for the whole region.

    // --- Length registers + constant materialization ---------------------------------
    // One fresh Int `len_reg` per resolvable register (foldable producer OR constant
    // source). Constant sources get a `LoadInt` materialized at the region head so the
    // value is live after the native OSR entry (which lands at `header`).
    let needs_len = |r: usize, foldable: &[bool]| -> bool {
        r < n_regs && (foldable[r] || const_len[r].is_some() || dynamic_len_source[r])
    };
    let mut len_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    let mut const_sources: Vec<usize> = Vec::new();
    for r in 0..n_regs {
        if needs_len(r, &foldable) {
            len_reg[r] = next_reg;
            next_reg += 1;
            if !foldable[r] {
                // a pure constant source (not an in-region producer we dissolve)
                if const_len[r].is_some() {
                    const_sources.push(r);
                }
            }
        }
    }

    fn emit_max_b(out_code: &mut Vec<RegInstr>, out: usize, a: usize, b: usize) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::GreaterEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }
    fn emit_min_b(out_code: &mut Vec<RegInstr>, out: usize, a: usize, b: usize) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::LessEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }
    // out = clamp slice length, byte-exact mirror of `bytes_slice`:
    //   sc = max(start,0); if sc >= L { 0 } else { min(max(len,0), L-sc) }.
    // This form cannot overflow for `len == i64::MAX`, unlike computing `sc + len`,
    // and matches the runtime's saturating `usize` addition.
    fn emit_slice_len_b(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        l_src: usize,
        start: usize,
        len: usize,
        next_reg: &mut usize,
    ) {
        let zero = *next_reg;
        let sc = *next_reg + 1;
        let available = *next_reg + 2;
        let lc = *next_reg + 3;
        *next_reg += 4;
        out_code.push(RegInstr::LoadInt {
            dst: zero,
            value: 0,
        });
        emit_max_b(out_code, sc, start, zero);
        let empty = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: sc,
            rhs: l_src,
            op: RegIntCompare::GreaterEqual,
            expected: true,
            target: 0,
        });
        out_code.push(RegInstr::SubInt {
            dst: available,
            lhs: l_src,
            rhs: sc,
        });
        emit_max_b(out_code, lc, len, zero);
        emit_min_b(out_code, out, lc, available);
        let done = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let empty_target = out_code.len();
        out_code.push(RegInstr::LoadInt { dst: out, value: 0 });
        let merge = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[empty] {
            *target = empty_target;
        }
        if let RegInstr::Jump { target } = &mut out_code[done] {
            *target = merge;
        }
    }

    // --- Rewrite the stream ----------------------------------------------------------
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
        MapGet { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        // Materialize the constant-length sources AT the header position, emitted BEFORE
        // the header instruction is pushed but AFTER `index_map[header]` is set — so
        // `index_map[header]` points at these `LoadInt`s. This makes them dominate EVERY
        // in-region use INCLUDING the header's own folded instruction: a loop whose
        // CONDITION reads a folded `Bytes.len` (e.g. `while i < Bytes.len(data)`) lowers
        // the header to read `len_reg[src]`, which must already be initialized when the
        // header runs. Native OSR entry lands at the header's mapped ip
        // (`index_map[header]`), and the loop back-edge also targets it, so on every
        // iteration (incl. the first OSR one) these constants run before the condition.
        // Re-running these idempotent `LoadInt`s per iteration is cheap.
        if i == header {
            for &r in &const_sources {
                if let Some(value) = const_len[r] {
                    new_code.push(RegInstr::LoadInt {
                        dst: len_reg[r],
                        value,
                    });
                }
            }
        }
        if in_region(i) {
            let folded = match instr {
                RegInstr::CallIntrinsic { dst, intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { dst, intrinsic, .. }
                    if foldable[*dst]
                        && intrinsic_descriptor(*intrinsic).bytes_fold_role
                            == Some(BytesFoldRole::ProducerFromString) =>
                {
                    if let Some(BProducer::FromString { src }) = &producer[*dst] {
                        new_code.push(RegInstr::Move {
                            dst: len_reg[*dst],
                            src: len_reg[*src],
                        });
                    }
                    true
                }
                RegInstr::CallIntrinsic { dst, intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { dst, intrinsic, .. }
                    if foldable[*dst]
                        && intrinsic_descriptor(*intrinsic).bytes_fold_role
                            == Some(BytesFoldRole::ProducerSlice) =>
                {
                    if let Some(BProducer::Slice { src, start, len }) = &producer[*dst] {
                        if dynamic_len_source[*src] {
                            new_code.push(RegInstr::CallIntrinsic {
                                dst: len_reg[*src],
                                intrinsic: RegIntrinsic::BytesLen,
                                args: vec![*src],
                            });
                        }
                        emit_slice_len_b(
                            &mut new_code,
                            len_reg[*dst],
                            len_reg[*src],
                            *start,
                            *len,
                            &mut next_reg,
                        );
                    }
                    true
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {
                    new_code.push(RegInstr::Move {
                        dst: len_reg[*dst],
                        src: len_reg[*src],
                    });
                    true
                }
                _ if is_bytes_len(instr) => {
                    let (dst, src) = match instr {
                        RegInstr::CallIntrinsic { dst, args, .. }
                        | RegInstr::CallTypedIntrinsic { dst, args, .. } => (*dst, args[0]),
                        _ => unreachable!(),
                    };
                    if foldable[src] || const_len[src].is_some() {
                        new_code.push(RegInstr::Move {
                            dst,
                            src: len_reg[src],
                        });
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if folded {
                continue;
            }
        }
        match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::MapGet {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = sa;
                        *err_ip = sb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = sa;
                        *else_ip = sb;
                    }
                    _ => {}
                }
            }
            Fix::MapGet { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    _ => {}
                }
            }
        }
    }
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}

/// A bounded clean-OSR-exit reconstruction tree. Aggregate scalar replacement
/// emits only register leaves; the cache builder later verifies that every leaf
/// is represented by the deopt ABI as a scalar or owned heap handle.
#[cfg(feature = "native-jit")]
#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct OsrMaterializeRecipe {
    pub(in crate::reg_vm) dst_reg: usize,
    pub(in crate::reg_vm) value: OsrMaterializeValue,
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct OsrMaterializeVariantArm {
    pub(in crate::reg_vm) tag: i64,
    pub(in crate::reg_vm) layout: Rc<crate::vm_value::TypeLayout>,
    pub(in crate::reg_vm) fields: Vec<OsrMaterializeValue>,
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Debug)]
pub(in crate::reg_vm) enum OsrMaterializeValue {
    Register(usize),
    OptionSome(Box<OsrMaterializeValue>),
    #[cfg(any(test, feature = "jit-struct-sr-experimental"))]
    Struct {
        layout: Rc<crate::vm_value::TypeLayout>,
        fields: Vec<OsrMaterializeValue>,
    },
    Variant {
        /// `None` is a statically selected single arm (used by always-Ok Result).
        tag_reg: Option<usize>,
        arms: Vec<OsrMaterializeVariantArm>,
    },
}

pub(in crate::reg_vm) const MAX_OSR_MATERIALIZE_DEPTH: usize = 8;
pub(in crate::reg_vm) const MAX_OSR_MATERIALIZE_NODES: usize = 64;

/// OSR × scalar replacement: scalar-replace non-escaping scalar `Option`s that live entirely
/// inside the loop region `[header, exit)` of an otherwise native-INELIGIBLE
/// function (one whose pre/post-loop code does I/O — calls, `Output.write`, …, which
/// the whole-function [`native_scalar_replace_options`] would reject).
///
/// Soundness model (region-scoped, conservative): an `Option` register is
/// scalar-replaced only if EVERY one of its definitions and uses lies strictly
/// inside `[header, exit)`, every in-region instruction is native-subset, one of
/// the four Option ops, or a `TryResult` consuming a scalar-replaced Option, and
/// the register never appears outside the region (so it is dead at both OSR
/// boundaries and the non-subset I/O outside never touches it). Anything we cannot
/// prove ⇒ `None` (no OSR; the interpreter runs the loop). The loop-carried
/// (boundary) registers keep their original indices; only fresh tag/payload regs
/// (>= `n_regs`) are added, and they are loop-internal.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` where
/// `ip_map[transformed_ip] = original_ip` (each rewritten Option op's fragments map
/// to that op's original index; copy-through maps one-to-one). Out-of-region
/// instructions are copied through verbatim, so the I/O before/after the loop is
/// preserved exactly.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_scalar_replace_options_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<OsrMaterializeRecipe>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no Option op inside the region ⇒ identity transform (plain OSR).
    if !(header..exit).any(|i| is_option_op(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map, Vec::new()));
    }

    // Every in-region instruction must be native-subset or one of the four Option
    // ops; otherwise the loop body cannot become a native loop anyway — bail.
    for i in header..exit {
        if !native_subset_instruction(&code[i])
            && !is_option_op(&code[i])
            && !matches!(&code[i], RegInstr::TryResult { .. })
        {
            return None;
        }
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // OPT = registers carrying an Option value: seed from in-region
    // `MakeSome`/`LoadNone` dsts, close under in-region `Move` aliasing.
    let mut opt = vec![false; n_regs];
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeSome { dst, .. }
            | RegInstr::LoadNone { dst }
            | RegInstr::DequePopFront { dst, .. }
            | RegInstr::DequePopBack { dst, .. } => opt[*dst] = true,
            _ => {}
        }
    }
    analysis.close_region_move_aliases(code, &mut opt)?;

    // Validate in-region uses/defs of OPT registers (identical recognition rules to
    // the whole-function pass), and require a SCALAR payload.
    for i in header..exit {
        match &code[i] {
            RegInstr::LoadNone { dst } if opt[*dst] => {}
            RegInstr::MakeSome { dst, value } if opt[*dst] => {
                if opt[*value] {
                    return None; // Option payload ⇒ non-scalar
                }
            }
            RegInstr::DequePopFront { dst, .. } | RegInstr::DequePopBack { dst, .. }
                if opt[*dst] => {}
            RegInstr::Move { dst, src } if opt[*dst] => {
                if !opt[*src] {
                    return None;
                }
            }
            RegInstr::MatchOption { src, .. } if opt[*src] => {}
            RegInstr::UnwrapSome { dst, src } if opt[*src] => {
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::TryResult { dst, src, .. } if opt[*src] => {
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if opt[*src] => {}
            other => {
                let reads = subset_or_option_reads(other)?;
                if reads.into_iter().any(|r| opt[r]) {
                    return None;
                }
                if let RegInstr::UnwrapSome { dst, .. }
                | RegInstr::MakeSome { dst, .. }
                | RegInstr::LoadNone { dst }
                | RegInstr::DequePopFront { dst, .. }
                | RegInstr::DequePopBack { dst, .. } = other
                    && opt[*dst]
                {
                    return None;
                }
            }
        }
    }

    // CRITICAL boundary soundness. After scalar replacement the ORIGINAL Option
    // register `o` is NEVER written inside the transformed region (its defs became
    // tag/payload writes), so the interpreter slot for `o` is stale after a clean OSR
    // exit. That is still sound when:
    //
    //   1. every in-region read of an OPT register is definitely assigned by an
    //      in-region Option def before the read, so native never needs a live-in
    //      interpreter Option value, and
    //   2. no out-of-region instruction reads an OPT register after native exits.
    //
    // Out-of-region writes are harmless under those two facts: pre-loop writes are
    // overwritten before every in-loop read, and post-loop writes overwrite the stale
    // interpreter slot before it can be observed. This matters for OSR × inlining,
    // where a call result register is commonly reused outside the loop even though
    // the Option value produced by the inlined callee is loop-local.
    if !option_regs_definitely_assigned_before_region_reads(code, n_regs, header, exit, &opt) {
        return None;
    }

    // heap-aware deopt(b) live-after always-`Some` Option reconstruction (the Option analog of the
    // Result pass). Originally ANY out-of-region read of an OPT register bailed. We now
    // allow a read AFTER the region by reconstructing `Some(payload)` at OSR-exit from
    // its scalar payload register, with the same soundness obligations as the Result
    // pass plus the always-`Some` requirement:
    //   * no OPT register written at ip >= exit (pre-loop init, ip < header, is fine);
    //   * a read BEFORE the region (live-in Option) is out of scope;
    //   * the OPT register is always-`Some` (no in-region `LoadNone` def) — a `None`
    //     outcome has no scalar payload to reconstruct and would make the payload only
    //     maybe-assigned; and
    //   * a single in-region `MakeSome` def reached UNCONDITIONALLY each iteration (so
    //     the payload is definitely-assigned after >=1 iteration).
    // (Conservative register footprints still hold: an unanalyzable instruction reports
    // `RegFootprint::All`, which bails. The scalar-payload-TYPE check is deferred to the
    // OsrEntry build site.)
    let mut reconstruct = vec![false; n_regs];
    for i in 0..code.len() {
        if i >= header && i < exit {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && opt[r]) {
                    return None;
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && opt[r] {
                        if i < header {
                            return None; // live-in Option (read before the loop)
                        }
                        reconstruct[r] = true;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }
    for (reg, &needs) in reconstruct.iter().enumerate() {
        if !needs {
            continue;
        }
        // Always-`Some`: no in-region `LoadNone` def for this register.
        if (header..exit).any(|i| matches!(&code[i], RegInstr::LoadNone { dst } if *dst == reg)) {
            return None;
        }
        let in_region_defs: Vec<usize> = analysis
            .writer_ips_of(code, reg)?
            .into_iter()
            .filter(|&i| i >= header && i < exit)
            .collect();
        if in_region_defs.len() != 1 {
            return None;
        }
        let def_ip = in_region_defs[0];
        for i in header..def_ip {
            match &code[i] {
                RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. }
                    if *target >= exit => {}
                RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. } => return None,
                _ => {}
            }
        }
    }

    // Allocate fresh tag/payload regs per OPT register.
    let mut tag_reg = vec![0usize; n_regs];
    let mut payload_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for (reg, is_opt) in opt.iter().enumerate() {
        if *is_opt {
            tag_reg[reg] = next_reg;
            payload_reg[reg] = next_reg + 1;
            next_reg += 2;
        }
    }

    // heap-aware deopt(b) Some-Option reconstruction recipes.
    let option_recipes: Vec<OsrMaterializeRecipe> = reconstruct
        .iter()
        .enumerate()
        .filter(|&(_, &needs)| needs)
        .map(|(reg, _)| OsrMaterializeRecipe {
            dst_reg: reg,
            value: OsrMaterializeValue::OptionSome(Box::new(OsrMaterializeValue::Register(
                payload_reg[reg],
            ))),
        })
        .collect();

    // Rewrite the WHOLE code, scalar-replacing in-region Option ops and copying
    // everything else through verbatim; remap all jump/match targets through the
    // index map. (Out-of-region jumps keep pointing at the right place after the
    // region's instructions expand.)
    enum Fix {
        Target(usize),
        Match { some_ip: usize, none_ip: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeSome { dst, value } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: *value,
                });
            }
            RegInstr::LoadNone { dst } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: false,
                });
            }
            RegInstr::DequePopFront { dst, deque } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::DequePopFront {
                    dst: payload_reg[*dst],
                    deque: *deque,
                });
            }
            RegInstr::DequePopBack { dst, deque } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::DequePopBack {
                    dst: payload_reg[*dst],
                    deque: *deque,
                });
            }
            RegInstr::Move { dst, src } if region && opt[*dst] => {
                new_code.push(RegInstr::Move {
                    dst: tag_reg[*dst],
                    src: tag_reg[*src],
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: payload_reg[*src],
                });
            }
            RegInstr::MatchOption {
                src,
                some_ip,
                none_ip,
            } if region && opt[*src] => {
                fixups.push((new_code.len(), Fix::Target(*some_ip)));
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: 0,
                });
                fixups.push((new_code.len(), Fix::Target(*none_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::UnwrapSome { dst, src } if region && opt[*src] => {
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            RegInstr::TryResult { dst, src, .. } if region && opt[*src] => {
                let some_target = new_code.len() + 2;
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: some_target,
                });
                new_code.push(RegInstr::RuntimeError {
                    message: String::new(),
                });
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            // Copy-through, remapping jump targets (covers both in-region native
            // branches and the pre/post-loop control flow). `MatchOption` outside the
            // region (or on a non-OPT src) is copied with BOTH targets remapped.
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        some_ip: *some_ip,
                        none_ip: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { some_ip, none_ip } => {
                let (s, n) = (index_map[some_ip], index_map[none_ip]);
                if let RegInstr::MatchOption {
                    some_ip: sd,
                    none_ip: nd,
                    ..
                } = &mut new_code[pos]
                {
                    *sd = s;
                    *nd = n;
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, option_recipes))
}

/// Whether a `MakeVariant` layout name is a `Result` constructor (`Ok`/`Err`). These
/// are reserved by the language for `Result`, are matched by the dedicated
/// `MatchResult` op (not `MatchVariant`), and are dissolved by the Result region pass
/// — never by the user-variant pass.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn is_result_ctor_name(name: &str) -> bool {
    name == "Ok" || name == "Err"
}

/// OSR × scalar replacement for RESULTS (deopt-before-heap, Slice 1): scalar-replace a non-escaping
/// `Result<Scalar, _>` that is **statically always-`Ok`** on the native path and
/// lives entirely inside the loop region `[header, exit)` of an otherwise native-
/// INELIGIBLE function. Mirrors [`native_scalar_replace_options_in_region`] but for
/// the `Result` shape (`MakeVariant{Ok|Err}` + `MatchResult` +
/// `UnwrapVariantValue`/`TryResult`).
///
/// KEY (the deopt-before-heap interplay): when a leaf's `Err` arm built a heap value,
/// [`native_inline_leaf_calls`] already replaced that arm with a native `Bail`, so the
/// inlined stream contains NO reachable `MakeVariant{Err}` — the only constructor of
/// the Result register is `MakeVariant{Ok, [scalar]}`. The Result is therefore
/// statically always-`Ok`, and this pass dissolves it to a single scalar **payload**
/// register (no tag needed): every `MatchResult{src:R}` becomes an unconditional
/// `Jump → ok_ip` (the `Err` arm goes dead) and every `UnwrapVariantValue{src:R,
/// expected:"Ok"}` becomes a `Move` from the payload. A LIVE heap `Err` (a reachable
/// `MakeVariant{Err}` def of `R`) ⇒ BAIL the pass (leave the loop on the interpreter):
/// such a Result is genuinely two-armed with a heap payload and cannot be scalarized.
///
/// Soundness: identical region discipline to the sibling passes. `R` is replaced only
/// if every def/use lies strictly inside `[header, exit)`, the `Ok` payload is scalar,
/// and `R` is dead at both OSR boundaries (`instr_read_regs`/`instr_written_reg`, with
/// `RegFootprint::All ⇒ bail`). The always-`Ok` rewrite is sound because the only way
/// the program reaches the dissolved `MatchResult` is by having built an `Ok` (the
/// `Err` constructor bailed to the interpreter before any heap op — the transactional fallback contract), so
/// the `Err` arm is dynamically unreachable on the native path; rewriting it to a
/// statically-dead `Jump`/`Bail` cannot change observable behavior.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→original
/// `ip_map` discipline as the other region passes.
///
/// A live-after Result reconstruction recipe:
/// `(variant_reg, ok_payload_reg, err_payload_reg, tag_reg)`. `tag_reg` is `None` for an
/// always-`Ok` Result (reconstruct `Ok(ok_payload)`; `err_payload` is unused, set equal
/// to `ok_payload`) and `Some(tag)` for a two-armed Result (reconstruct
/// `Ok(ok_payload)` if the tag's live value is non-zero, else `Err(err_payload)`).
/// PER-ARM payloads: the `Ok` and `Err` arms keep SEPARATE payload registers so arms
/// of different native types (e.g. `Result<Int, String>`) don't force a single payload
/// register into conflicting types. Only the arm matching the live `tag` is read at
/// reconstruction, so the other (possibly stale) payload is never observed.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) type ResultRecipe = (usize, usize, usize, Option<usize>);

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_scalar_replace_results_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<ResultRecipe>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no `MatchResult` and no `Result` constructor in the region ⇒ nothing
    // for THIS pass to do (identity transform; preserves the byte-for-byte old path).
    let has_result_op = (header..exit).any(|i| {
        matches!(&code[i], RegInstr::MatchResult { .. })
            || matches!(&code[i], RegInstr::MakeVariant { layout, .. } if is_result_ctor_name(&layout.name))
    });
    if !has_result_op {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map, Vec::new()));
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // RES = registers carrying a (replaceable) Result value: seed from in-region
    // `MakeVariant{Ok|Err}` dsts, close under in-region `Move` aliasing.
    let mut res = vec![false; n_regs];
    for i in header..exit {
        if let RegInstr::MakeVariant { dst, layout, .. } = &code[i]
            && is_result_ctor_name(&layout.name)
        {
            res[*dst] = true;
        }
    }
    analysis.close_region_move_aliases(code, &mut res)?;
    // A `MatchResult{src}` whose `src` is not (yet) a RES register means a Result we
    // cannot see being constructed in-region (it flows in from outside, e.g. a heap
    // Result param) ⇒ this pass cannot dissolve it. Bail so the loop stays on the
    // interpreter (the boundary/escape gates below would also catch it, but bailing
    // early is clearer and conservative).
    for i in header..exit {
        if let RegInstr::MatchResult { src, .. } = &code[i]
            && !res[*src]
        {
            return None;
        }
    }

    // Two-armed (heap-aware deopt #7 follow-up): a reachable `MakeVariant{Err}` on a RES register
    // means the Result is NOT statically always-`Ok`. Instead of bailing, dissolve it
    // with an explicit `Ok`/`Err` tag + a shared scalar payload register (the tag routes
    // the `MatchResult` and selects which arm's `UnwrapVariantValue` reads the payload).
    // Scoped to dead-at-boundary: a two-armed RES that is live-after, or short-circuited
    // by `?` (`TryResult`), bails (live-after Ok/Err reconstruction stays future).
    let two_armed = (header..exit).any(|i| {
        matches!(&code[i],
            RegInstr::MakeVariant { dst, layout, .. }
                if res[*dst] && layout.name.as_ref() == "Err")
    });
    if two_armed {
        return native_scalar_replace_two_armed_results_in_region(code, n_regs, header, exit, &res);
    }

    // Validate every in-region def/use of a RES register. The Result must be
    // statically always-`Ok`: a reachable `MakeVariant{Err}` def is a LIVE heap Err
    // ⇒ bail. `Ok` payload must be scalar (not itself a RES register). Recognized uses:
    // `MatchResult{src:R}`, `UnwrapVariantValue{src:R}` (Ok scalar payload, or the dead
    // Err-arm unwrap which the rewrite drops), `TryResult{src:R}` (the `?` success
    // projection), and `Move` aliases. Anything else that touches a RES register ⇒ bail.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeVariant {
                dst,
                layout,
                fields,
            } if res[*dst] => {
                if layout.name.as_ref() == "Err" {
                    return None; // live heap Err ⇒ not always-Ok ⇒ leave on interpreter
                }
                // Ok constructor: exactly one scalar field `value`.
                if fields.len() != 1 || fields.iter().any(|(_, r)| res[*r]) {
                    return None;
                }
            }
            RegInstr::Move { dst, src } if res[*dst] => {
                if !res[*src] {
                    return None;
                }
            }
            RegInstr::MatchResult { src, .. } if res[*src] => {}
            RegInstr::TryResult { dst, src, .. } if res[*src] => {
                if res[*dst] {
                    return None;
                }
            }
            RegInstr::UnwrapVariantValue { dst, src, expected } if res[*src] => {
                // The Ok-arm unwrap yields the scalar payload; its `dst` must not be a
                // RES register (a Result payload would be non-scalar). The Err-arm
                // unwrap (`expected == "Err"`) lies on the statically-dead arm — its
                // `dst` is unused on the native path; allow it (rewritten to a Bail).
                let _ = expected;
                if res[*dst] {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if res[*src] => {}
            other => {
                // Any other instruction must not read a RES register, nor (re)define one
                // through an unrecognized destination.
                match instr_read_regs(other) {
                    RegFootprint::Some(reads) => {
                        if reads.into_iter().any(|r| r < n_regs && res[r]) {
                            return None;
                        }
                    }
                    RegFootprint::All => return None,
                }
                if let RegInstr::UnwrapVariantValue { dst, .. } | RegInstr::MakeVariant { dst, .. } =
                    other
                    && res[*dst]
                {
                    return None;
                }
            }
        }
    }

    // Boundary soundness + heap-aware deopt(b) live-after reconstruction. Originally every RES
    // register had to be DEAD outside `[header, exit)`. We now also allow a RES
    // register that is only READ after the region by reconstructing `Ok(payload)` at
    // OSR-exit from its scalar payload register, because the pass already proved every
    // RES register is always-`Ok` with a scalar `Ok` payload (a heap `Err` became a
    // native `Bail`), so a completed native loop guarantees the value is `Ok(payload)`.
    // Conditions to keep it sound:
    //   * No RES register may be WRITTEN at ip >= exit (post-loop reassignment is out
    //     of scope). A write BEFORE the region (pre-loop `let mut r = Ok(..)`) is fine:
    //     native never touches the original RES slot and reconstruction overwrites it
    //     at exit, or — after 0 native iterations — the pre-loop value already in the
    //     slot is exactly correct.
    //   * A RES register read BEFORE the region (a live-in Result) is out of scope.
    //   * Each live-after RES register needs a single in-region definition reached
    //     UNCONDITIONALLY each iteration (no branch between the header and the def
    //     except the header's own loop-exit condition), so its payload register is
    //     definitely-assigned after >=1 iteration (hence present in the OSR-exit deopt
    //     live set). A conditional/multiply-defined RES register would leave the
    //     payload only maybe-assigned ⇒ bail (conservative).
    // The scalar-payload-TYPE check is deferred to the OsrEntry build site (where
    // native register types are known); a non-scalar `Ok` payload declines OSR there.
    let mut reconstruct = vec![false; n_regs];
    for i in 0..code.len() {
        if in_region(i) {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && res[r]) {
                    return None; // post-loop reassignment of a dissolved Result
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && res[r] {
                        if i < header {
                            return None; // live-in Result (read before the loop)
                        }
                        reconstruct[r] = true;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }
    // Require a single, unconditionally-reached in-region def for every RES register we
    // must reconstruct.
    for (reg, &needs) in reconstruct.iter().enumerate() {
        if !needs {
            continue;
        }
        let in_region_defs: Vec<usize> = analysis
            .writer_ips_of(code, reg)?
            .into_iter()
            .filter(|&i| in_region(i))
            .collect();
        if in_region_defs.len() != 1 {
            return None;
        }
        let def_ip = in_region_defs[0];
        for i in header..def_ip {
            match &code[i] {
                // The header's loop-exit condition (target outside the loop) is fine.
                RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. }
                    if *target >= exit => {}
                // Any other branch/match/return between the header and the def could
                // skip the def on some iteration ⇒ payload not definitely-assigned.
                RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. } => return None,
                _ => {}
            }
        }
    }

    // Allocate one fresh payload register per RES register (always-Ok ⇒ no tag).
    let mut payload_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for (reg, is_res) in res.iter().enumerate() {
        if *is_res {
            payload_reg[reg] = next_reg;
            next_reg += 1;
        }
    }

    // heap-aware deopt(b) recipes for each live-after RES register. Always-`Ok` recipes carry no
    // tag (`None`) ⇒ reconstruct `Ok(payload)`; this path has a single payload register,
    // so the (unused) err-payload slot mirrors it.
    let recipes: Vec<ResultRecipe> = reconstruct
        .iter()
        .enumerate()
        .filter(|&(_, &needs)| needs)
        .map(|(reg, _)| (reg, payload_reg[reg], payload_reg[reg], None))
        .collect();

    // Rewrite the WHOLE code, dissolving in-region Result ops and copying everything
    // else through verbatim; remap all jump/match targets through the index map.
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeVariant { dst, fields, .. } if region && res[*dst] => {
                // Always-Ok constructor: payload = the single scalar field.
                let (_, field_reg) = &fields[0];
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: *field_reg,
                });
            }
            RegInstr::Move { dst, src } if region && res[*dst] => {
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: payload_reg[*src],
                });
            }
            RegInstr::MatchResult { src, ok_ip, err_ip } if region && res[*src] => {
                // Statically always-Ok ⇒ unconditional jump to the Ok arm. The Err arm
                // (`err_ip`) becomes unreachable.
                let _ = err_ip;
                fixups.push((new_code.len(), Fix::Target(*ok_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::TryResult { dst, src, .. } if region && res[*src] => {
                // Statically always-Ok `?` ⇒ payload projection. The short-circuit arm is
                // unreachable on the native path; any real Err was already replaced by a
                // Bail before constructing heap state, so the interpreter rerun performs
                // the normal cleanup/return behavior.
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            RegInstr::UnwrapVariantValue { dst, src, expected } if region && res[*src] => {
                if expected.as_str() == "Err" {
                    // Dead Err-arm unwrap: unreachable after the always-Ok rewrite. Emit
                    // a Bail sentinel so that, even if some path reached it, native would
                    // safely deopt rather than read a non-existent heap Err payload.
                    new_code.push(RegInstr::RuntimeError {
                        message: String::new(),
                    });
                } else {
                    // Ok-arm unwrap ⇒ the scalar payload.
                    new_code.push(RegInstr::Move {
                        dst: *dst,
                        src: payload_reg[*src],
                    });
                }
            }
            // Copy-through, remapping jump targets (in-region native branches and the
            // pre/post-loop control flow). A `MatchResult` outside the region (or on a
            // non-RES src) is copied with BOTH targets remapped; same for the other
            // match ops the body may carry.
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { a, b } => {
                let (na, nb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = na;
                        *err_ip = nb;
                    }
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = na;
                        *none_ip = nb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = na;
                        *else_ip = nb;
                    }
                    _ => {}
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, recipes))
}
