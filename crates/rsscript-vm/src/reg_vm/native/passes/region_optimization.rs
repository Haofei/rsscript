use super::*;

type WholeOptionRewrite = (Vec<RegInstr>, usize, Vec<usize>, Vec<usize>);

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

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_readable_or_sinkable_closure_operand_candidate(
    _func: &RegFunction,
    _closure: usize,
) -> bool {
    false
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn monomorphic_closure_inline_target(
    _unit: &RegUnit,
    _func: &RegFunction,
    _profile: Option<&FunctionProfile>,
    _call_count: u32,
    _i: usize,
) -> Option<usize> {
    None
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn polymorphic_closure_inline_targets(
    _unit: &RegUnit,
    _func: &RegFunction,
    _profile: Option<&FunctionProfile>,
    _call_count: u32,
    _i: usize,
) -> Option<Vec<usize>> {
    None
}

#[cfg(feature = "native-jit")]
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
pub(in crate::reg_vm) fn option_regs_definitely_assigned_before_region_reads(
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
// Rewrites coordinate code, origins, and register facts by source index.
pub(in crate::reg_vm) fn native_scalar_replace_options(
    code: &[RegInstr],
    n_regs: usize,
) -> Option<WholeOptionRewrite> {
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
    for i in parallel_indices(0..code.len()) {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in parallel_indices(start..end) {
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
// Rewrites coordinate code and source-IP maps by the original instruction index.
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
    for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(0..code.len()) {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in parallel_indices(start..end) {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}
