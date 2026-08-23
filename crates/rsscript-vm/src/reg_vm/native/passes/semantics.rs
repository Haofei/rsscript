use super::*;

#[cfg(feature = "native-jit")]
#[derive(Clone, Debug, Default)]
pub(super) struct NativeInstrSemantics {
    pub(super) native_subset: bool,
    pub(super) dst: Option<usize>,
    pub(super) list_write: Option<usize>,
    pub(super) heap_write: bool,
    pub(super) field_slot_access: bool,
    pub(super) control: NativeControlFlow,
    pub(super) reads: RegFootprint,
    pub(super) writes: RegFootprint,
}

#[cfg(feature = "native-jit")]
pub(super) fn native_instr_semantics(instr: &RegInstr) -> NativeInstrSemantics {
    use RegFootprint::Some as S;

    let list_write = match instr {
        RegInstr::ListSet { list, .. }
        | RegInstr::ListPush { list, .. }
        | RegInstr::ListAppend { list, .. }
        | RegInstr::ListClear { list, .. }
        | RegInstr::ListPop { list, .. }
        | RegInstr::ListSort { list, .. }
        | RegInstr::ListRemoveAt { list, .. } => Some(*list),
        _ => None,
    };

    let heap_write = matches!(
        instr,
        RegInstr::SetFieldSlot { .. }
            | RegInstr::ListSet { .. }
            | RegInstr::ListPush { .. }
            | RegInstr::ListAppend { .. }
            | RegInstr::ListClear { .. }
            | RegInstr::ListPop { .. }
            | RegInstr::ListSort { .. }
            | RegInstr::ListRemoveAt { .. }
            | RegInstr::MapInsert { .. }
            | RegInstr::SetInsert { .. }
            | RegInstr::SortedSetInsert { .. }
            | RegInstr::SortedMapInsert { .. }
            | RegInstr::DequePushBack { .. }
            | RegInstr::DequePushFront { .. }
            | RegInstr::DequePopFront { .. }
            | RegInstr::DequePopBack { .. }
    );

    let field_slot_access = matches!(
        instr,
        RegInstr::GetFieldSlot { .. } | RegInstr::SetFieldSlot { .. }
    );

    let control = match instr {
        RegInstr::Jump { target } => NativeControlFlow::Jump(*target),
        RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
            NativeControlFlow::Branch { target: *target }
        }
        RegInstr::MatchOption {
            some_ip, none_ip, ..
        }
        | RegInstr::MatchMapGet {
            some_ip, none_ip, ..
        }
        | RegInstr::MatchSortedMapGet {
            some_ip, none_ip, ..
        } => NativeControlFlow::Split {
            first: *some_ip,
            second: *none_ip,
        },
        RegInstr::MatchResult { ok_ip, err_ip, .. } => NativeControlFlow::Split {
            first: *ok_ip,
            second: *err_ip,
        },
        RegInstr::MatchVariant {
            match_ip, else_ip, ..
        } => NativeControlFlow::Split {
            first: *match_ip,
            second: *else_ip,
        },
        RegInstr::Return { .. } | RegInstr::RuntimeError { .. } => NativeControlFlow::Terminal,
        _ => NativeControlFlow::Fallthrough,
    };

    let reads = match instr {
        RegInstr::LoadUnit { .. }
        | RegInstr::LoadInt { .. }
        | RegInstr::LoadFloat { .. }
        | RegInstr::LoadBool { .. }
        | RegInstr::LoadString { .. }
        | RegInstr::LoadChar { .. }
        | RegInstr::LoadNone { .. }
        | RegInstr::TailCallGuard
        | RegInstr::Jump { .. }
        | RegInstr::RuntimeError { .. } => S(vec![]),
        RegInstr::Move { src, .. }
        | RegInstr::Manage { src, .. }
        | RegInstr::DeepCopy { reg: src }
        | RegInstr::DeepCopyElided { reg: src }
        | RegInstr::UnwrapSome { src, .. }
        | RegInstr::UnwrapVariantValue { src, .. }
        | RegInstr::AwaitJoin { src, .. } => S(vec![*src]),
        RegInstr::ResourceDrop { resource } => S(vec![*resource]),
        RegInstr::GetField { base, .. } | RegInstr::GetFieldSlot { base, .. } => S(vec![*base]),
        RegInstr::NativeGuardClosureId { closure, .. }
        | RegInstr::NativeClosureId { closure, .. }
        | RegInstr::NativeClosureCapture { closure, .. } => S(vec![*closure]),
        RegInstr::NativeFieldClosureId { base, .. }
        | RegInstr::NativeFieldClosureCapture { base, .. } => S(vec![*base]),
        RegInstr::SetField { base, value, .. } | RegInstr::SetFieldSlot { base, value, .. } => {
            S(vec![*base, *value])
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
        | RegInstr::JumpIfIntCompare { lhs, rhs, .. } => S(vec![*lhs, *rhs]),
        RegInstr::JumpIfBool { cond, .. } => S(vec![*cond]),
        RegInstr::MatchOption { src, .. }
        | RegInstr::MatchResult { src, .. }
        | RegInstr::MatchVariant { src, .. } => S(vec![*src]),
        RegInstr::TryResult { src, cleanup, .. } => {
            let mut reads = Vec::with_capacity(cleanup.len() + 1);
            reads.push(*src);
            reads.extend(cleanup.iter().copied());
            S(reads)
        }
        RegInstr::MatchMapGet { map, key, .. } | RegInstr::MatchSortedMapGet { map, key, .. } => {
            S(vec![*map, *key])
        }
        RegInstr::ListLen { list, .. } => S(vec![*list]),
        RegInstr::ListGet { list, index, .. } => S(vec![*list, *index]),
        RegInstr::ListSet {
            list, index, value, ..
        } => S(vec![*list, *index, *value]),
        RegInstr::ListPush { list, value, .. } => S(vec![*list, *value]),
        RegInstr::ListSort { list, .. } => S(vec![*list]),
        RegInstr::MapInsert {
            map, key, value, ..
        } => S(vec![*map, *key, *value]),
        RegInstr::SetInsert { set, value, .. } => S(vec![*set, *value]),
        RegInstr::SortedSetInsert { set, value, .. } => S(vec![*set, *value]),
        RegInstr::SortedMapInsert {
            map, key, value, ..
        } => S(vec![*map, *key, *value]),
        RegInstr::DequePushBack { deque, value, .. }
        | RegInstr::DequePushFront { deque, value, .. } => S(vec![*deque, *value]),
        RegInstr::DequePopFront { deque, .. } | RegInstr::DequePopBack { deque, .. } => {
            S(vec![*deque])
        }
        RegInstr::MakeSome { value, .. } => S(vec![*value]),
        RegInstr::StringConcat { left, right, .. } => S(vec![*left, *right]),
        RegInstr::Return { src } => S(vec![*src]),
        // Call / construction families: their value operands are the arg/field/item
        // register vectors (the `dst` is written, not read).
        RegInstr::CallKnown { args, .. }
        | RegInstr::CallDynamic { args, .. }
        | RegInstr::SpawnTask { args, .. }
        | RegInstr::CallExternal { args, .. }
        | RegInstr::CallIntrinsic { args, .. }
        | RegInstr::CallTypedIntrinsic { args, .. } => S(args.clone()),
        RegInstr::CallClosure { closure, args, .. } => {
            let mut v = args.clone();
            v.push(*closure);
            S(v)
        }
        RegInstr::MakeList { items, .. } => S(items.clone()),
        RegInstr::MakeStruct { fields, .. }
        | RegInstr::MakeVariant { fields, .. }
        | RegInstr::MakeObject { fields, .. } => S(fields.iter().map(|(_, r)| *r).collect()),
        RegInstr::MakeMap { entries, .. } => {
            S(entries.iter().flat_map(|(k, v)| [*k, *v]).collect())
        }
        RegInstr::MakeClosure { captures, .. } => S(captures.clone()),
        // Anything not modelled above (collection mutators, select, try, …) ⇒
        // conservatively "reads everything".
        _ => RegFootprint::All,
    };

    let dst = match instr {
        RegInstr::LoadInt { dst, .. }
        | RegInstr::LoadFloat { dst, .. }
        | RegInstr::LoadBool { dst, .. }
        | RegInstr::Move { dst, .. }
        | RegInstr::DeepCopy { reg: dst, .. }
        | RegInstr::DeepCopyElided { reg: dst, .. }
        | RegInstr::AddInt { dst, .. }
        | RegInstr::SubInt { dst, .. }
        | RegInstr::MulInt { dst, .. }
        | RegInstr::DivInt { dst, .. }
        | RegInstr::ModInt { dst, .. }
        | RegInstr::BitAndInt { dst, .. }
        | RegInstr::BitOrInt { dst, .. }
        | RegInstr::BitXorInt { dst, .. }
        | RegInstr::ShiftLeftInt { dst, .. }
        | RegInstr::ShiftRightInt { dst, .. }
        | RegInstr::LessInt { dst, .. }
        | RegInstr::LessEqualInt { dst, .. }
        | RegInstr::GreaterInt { dst, .. }
        | RegInstr::GreaterEqualInt { dst, .. }
        | RegInstr::Equal { dst, .. }
        | RegInstr::NotEqual { dst, .. }
        | RegInstr::GetFieldSlot { dst, .. }
        | RegInstr::ListLen { dst, .. }
        | RegInstr::ListGet { dst, .. }
        | RegInstr::ListSet { dst, .. }
        | RegInstr::ListPush { dst, .. }
        | RegInstr::ListSort { dst, .. }
        | RegInstr::MapInsert { dst, .. }
        | RegInstr::SetInsert { dst, .. }
        | RegInstr::SortedSetInsert { dst, .. }
        | RegInstr::SortedMapInsert { dst, .. }
        | RegInstr::DequePushBack { dst, .. }
        | RegInstr::DequePushFront { dst, .. }
        | RegInstr::DequePopFront { dst, .. }
        | RegInstr::DequePopBack { dst, .. }
        | RegInstr::StringConcat { dst, .. }
        | RegInstr::NativeClosureId { dst, .. }
        | RegInstr::NativeClosureCapture { dst, .. }
        | RegInstr::NativeFieldClosureId { dst, .. }
        | RegInstr::NativeFieldClosureCapture { dst, .. }
        | RegInstr::CallIntrinsic { dst, .. }
        | RegInstr::CallTypedIntrinsic { dst, .. } => Some(*dst),
        RegInstr::MatchMapGet { value_dst, .. } | RegInstr::MatchSortedMapGet { value_dst, .. } => {
            Some(*value_dst)
        }
        RegInstr::SetFieldSlot { base, .. } => Some(*base),
        _ => None,
    };

    let writes = match instr {
        RegInstr::Jump { .. }
        | RegInstr::JumpIfBool { .. }
        | RegInstr::JumpIfIntCompare { .. }
        | RegInstr::MatchOption { .. }
        | RegInstr::MatchResult { .. }
        | RegInstr::MatchVariant { .. }
        | RegInstr::RuntimeError { .. }
        | RegInstr::NativeGuardClosureId { .. }
        | RegInstr::ResourceDrop { .. }
        | RegInstr::DeepCopy { .. }
        | RegInstr::DeepCopyElided { .. }
        | RegInstr::Return { .. } => S(vec![]),
        RegInstr::LoadUnit { dst }
        | RegInstr::LoadInt { dst, .. }
        | RegInstr::LoadFloat { dst, .. }
        | RegInstr::LoadBool { dst, .. }
        | RegInstr::LoadString { dst, .. }
        | RegInstr::LoadChar { dst, .. }
        | RegInstr::LoadNone { dst }
        | RegInstr::Move { dst, .. }
        | RegInstr::Manage { dst, .. }
        | RegInstr::TryResult { dst, .. }
        | RegInstr::GetField { dst, .. }
        | RegInstr::GetFieldSlot { dst, .. }
        | RegInstr::SetField { dst, .. }
        | RegInstr::SetFieldSlot { dst, .. }
        | RegInstr::MakeStruct { dst, .. }
        | RegInstr::MakeVariant { dst, .. }
        | RegInstr::MakeList { dst, .. }
        | RegInstr::MakeObject { dst, .. }
        | RegInstr::MakeMap { dst, .. }
        | RegInstr::MakeClosure { dst, .. }
        | RegInstr::MakeSome { dst, .. }
        | RegInstr::AddInt { dst, .. }
        | RegInstr::SubInt { dst, .. }
        | RegInstr::MulInt { dst, .. }
        | RegInstr::DivInt { dst, .. }
        | RegInstr::ModInt { dst, .. }
        | RegInstr::BitAndInt { dst, .. }
        | RegInstr::BitOrInt { dst, .. }
        | RegInstr::BitXorInt { dst, .. }
        | RegInstr::ShiftLeftInt { dst, .. }
        | RegInstr::ShiftRightInt { dst, .. }
        | RegInstr::LessInt { dst, .. }
        | RegInstr::LessEqualInt { dst, .. }
        | RegInstr::GreaterInt { dst, .. }
        | RegInstr::GreaterEqualInt { dst, .. }
        | RegInstr::Equal { dst, .. }
        | RegInstr::NotEqual { dst, .. }
        | RegInstr::UnwrapSome { dst, .. }
        | RegInstr::UnwrapVariantValue { dst, .. }
        | RegInstr::ListLen { dst, .. }
        | RegInstr::ListGet { dst, .. }
        | RegInstr::ListSet { dst, .. }
        | RegInstr::ListPush { dst, .. }
        | RegInstr::ListSort { dst, .. }
        | RegInstr::MapInsert { dst, .. }
        | RegInstr::SetInsert { dst, .. }
        | RegInstr::SortedSetInsert { dst, .. }
        | RegInstr::SortedMapInsert { dst, .. }
        | RegInstr::DequePushBack { dst, .. }
        | RegInstr::DequePushFront { dst, .. }
        | RegInstr::DequePopFront { dst, .. }
        | RegInstr::DequePopBack { dst, .. }
        | RegInstr::CallKnown { dst, .. }
        | RegInstr::CallDynamic { dst, .. }
        | RegInstr::SpawnTask { dst, .. }
        | RegInstr::AwaitJoin { dst, .. }
        | RegInstr::CallExternal { dst, .. }
        | RegInstr::CallClosure { dst, .. }
        | RegInstr::CallIntrinsic { dst, .. }
        | RegInstr::CallTypedIntrinsic { dst, .. }
        | RegInstr::NativeClosureId { dst, .. }
        | RegInstr::NativeClosureCapture { dst, .. }
        | RegInstr::NativeFieldClosureId { dst, .. }
        | RegInstr::NativeFieldClosureCapture { dst, .. }
        | RegInstr::StringConcat { dst, .. } => S(vec![*dst]),
        RegInstr::MatchMapGet { value_dst, .. } | RegInstr::MatchSortedMapGet { value_dst, .. } => {
            S(vec![*value_dst])
        }
        // `SelectWait` writes winner/value; calls with `mut_args` write back to arg
        // registers — all modelled conservatively as `All` so OSR × scalar replacement bails rather
        // than risk a missed boundary write.
        _ => RegFootprint::All,
    };

    let native_subset = match instr {
        RegInstr::CallIntrinsic {
            intrinsic, args, ..
        } => {
            native_host_typed_intrinsic(*intrinsic, None)
                .is_some_and(|spec| args.len() == spec.arg_tys().len())
                || native_inline_convert_intrinsic(*intrinsic)
                    .is_some_and(|arity| args.len() == arity)
        }
        RegInstr::CallTypedIntrinsic {
            intrinsic,
            type_arg,
            args,
            ..
        } => native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))
            .is_some_and(|spec| args.len() == spec.arg_tys().len()),
        RegInstr::LoadInt { .. }
        | RegInstr::LoadFloat { .. }
        | RegInstr::LoadBool { .. }
        | RegInstr::LoadString { .. }
        | RegInstr::Move { .. }
        | RegInstr::DeepCopy { .. }
        | RegInstr::DeepCopyElided { .. }
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
        | RegInstr::TailCallGuard
        | RegInstr::Jump { .. }
        | RegInstr::JumpIfBool { .. }
        | RegInstr::JumpIfIntCompare { .. }
        | RegInstr::Return { .. }
        | RegInstr::RuntimeError { .. }
        | RegInstr::StringConcat { .. }
        | RegInstr::SetFieldSlot { .. }
        | RegInstr::GetFieldSlot { .. }
        | RegInstr::ListLen { .. }
        | RegInstr::ListGet { .. }
        | RegInstr::ListSet { .. }
        | RegInstr::ListPush { .. }
        | RegInstr::ListSort { .. }
        | RegInstr::MapInsert { .. }
        | RegInstr::SetInsert { .. }
        | RegInstr::SortedSetInsert { .. }
        | RegInstr::SortedMapInsert { .. }
        | RegInstr::DequePushBack { .. }
        | RegInstr::DequePushFront { .. }
        | RegInstr::DequePopFront { .. }
        | RegInstr::DequePopBack { .. }
        | RegInstr::MatchMapGet { .. }
        | RegInstr::MatchSortedMapGet { .. }
        | RegInstr::NativeGuardClosureId { .. }
        | RegInstr::NativeClosureId { .. }
        | RegInstr::NativeClosureCapture { .. }
        | RegInstr::NativeFieldClosureId { .. }
        | RegInstr::NativeFieldClosureCapture { .. } => true,
        _ => false,
    };

    NativeInstrSemantics {
        native_subset,
        dst,
        list_write,
        heap_write,
        field_slot_access,
        control,
        reads,
        writes,
    }
}

/// Whether an instruction is in the *native* JIT subset (integer/boolean/control
/// core, no heap/calls/async/floats). Tighter than [`jit_supported_instruction`].
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_subset_instruction(instr: &RegInstr) -> bool {
    native_instr_semantics(instr).native_subset
}

/// The register a native-subset instruction definitely writes (its `dst`), if any.
/// Control/jump instructions and pure side-effect-free reads-with-no-dst write
/// nothing. Used by the OSR flat-list pass to detect loop-invariant (never-written)
/// list registers. Conservative: an instruction whose write shape isn't modeled here
/// (i.e. not in the native subset) never appears in a translated OSR loop, so a
/// `None` for it cannot cause a non-invariant register to be misclassified.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_subset_dst(instr: &RegInstr) -> Option<usize> {
    native_instr_semantics(instr).dst
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_instruction_has_heap_write(instr: &RegInstr) -> bool {
    native_instr_semantics(instr).heap_write
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_instruction_touches_field_slot(instr: &RegInstr) -> bool {
    native_instr_semantics(instr).field_slot_access
}

/// The register holding the heap object an instruction mutates IN PLACE, if any.
/// Only the ops that mutate a shared `Rc<RefCell<..>>` in place qualify — `SetFieldSlot`
/// is excluded (it is a copy-on-write struct rebuild, not an in-place mutation). Used by
/// [`native_deepcopy_param_unsoundly_mutated`] to detect mutation of a DeepCopy'd param.
#[cfg(feature = "native-jit")]
fn native_heap_mutation_receiver(instr: &RegInstr) -> Option<usize> {
    match instr {
        RegInstr::ListSet { list, .. }
        | RegInstr::ListPush { list, .. }
        | RegInstr::ListAppend { list, .. }
        | RegInstr::ListClear { list, .. }
        | RegInstr::ListPop { list, .. }
        | RegInstr::ListSort { list, .. }
        | RegInstr::ListRemoveAt { list, .. } => Some(*list),
        RegInstr::MapInsert { map, .. } | RegInstr::SortedMapInsert { map, .. } => Some(*map),
        RegInstr::SetInsert { set, .. } | RegInstr::SortedSetInsert { set, .. } => Some(*set),
        RegInstr::DequePushBack { deque, .. }
        | RegInstr::DequePushFront { deque, .. }
        | RegInstr::DequePopFront { deque, .. }
        | RegInstr::DequePopBack { deque, .. } => Some(*deque),
        _ => None,
    }
}

/// Whether register `r` carries a heap value (a shared `Rc`) under the inferred native
/// types. Scalars (`Int`/`Float`/`Bool`) copy by value and cannot alias, so a `DeepCopy`
/// of one is a no-op and never a leak vector.
#[cfg(feature = "native-jit")]
fn native_is_heap_reg(ty: &[Option<NativeTy>], r: usize) -> bool {
    matches!(
        ty.get(r).copied().flatten(),
        Some(
            NativeTy::Handle
                | NativeTy::FlatInt
                | NativeTy::FlatIntMut
                | NativeTy::FlatFloat
                | NativeTy::FlatFloatMut
        )
    )
}

/// Whether an intrinsic's result is a freshly-allocated, PROVABLY-IMMUTABLE `String`/`Bytes`
/// leaf. Deliberately an explicit allow-list, NOT `cold_arm_pure_builder`: that flag also
/// marks the pure-but-MUTABLE collection constructors (`Map.new`/`Set.new`/`Deque.new`), and
/// treating those as immutable in a soundness guard would be unsound (a `Map.new()` result
/// flowing into a DeepCopy'd root could be wrongly proven immutable and left untainted).
/// Conservative: an unlisted String/Bytes producer is simply not proven (over-declines, safe).
#[cfg(feature = "native-jit")]
fn native_intrinsic_produces_immutable_leaf(intrinsic: &RegIntrinsic) -> bool {
    matches!(
        intrinsic,
        RegIntrinsic::StringFromInt
            | RegIntrinsic::StringFromBool
            | RegIntrinsic::StringFromFloat
            | RegIntrinsic::StringCopy
            | RegIntrinsic::StringSlice
            | RegIntrinsic::StringPadLeft
            | RegIntrinsic::BytesFromString
            | RegIntrinsic::BytesSlice
    )
}

/// Classify every register that PROVABLY holds an immutable `String`/`Bytes` value (so its
/// `Rc` is safe to share). Used by the DeepCopy soundness guard to decide which DeepCopy'd
/// roots are safe to leave untainted — crucially, this works AFTER inlining: an inlined
/// callee parameter is defined by the arg-marshalling `Move`, so its immutability flows from
/// the argument (which itself resolves back to an outer param or a `String`/`Bytes` producer).
///
/// Soundness: a register is proven immutable ONLY when it has at least one def and EVERY def
/// produces a `String`/`Bytes` — an immutable producer (`LoadString`/`StringConcat`/a
/// `cold_arm_pure_builder` String/Bytes intrinsic), a `Move` from a proven-immutable source,
/// or an outer `String`/`Bytes` parameter. Any other writer (container/struct/list builders,
/// extractions, calls, a `Move` from a non-proven source, or a parameter not flagged
/// immutable) leaves the register unproven ⇒ the guard taints it conservatively. A register
/// with no def is unproven. If any instruction's write footprint cannot be modeled, nothing
/// is proven (everything stays taintable).
#[cfg(feature = "native-jit")]
fn native_reg_proven_immutable_leaf(
    code: &[RegInstr],
    n_regs: usize,
    immutable_leaf_params: &[bool],
) -> Vec<bool> {
    let mut has_def = vec![false; n_regs];
    // `blocked` = the register has a def that is definitely NOT a `String`/`Bytes` leaf.
    let mut blocked = vec![false; n_regs];
    let mut move_srcs: Vec<Vec<usize>> = vec![Vec::new(); n_regs];
    // Outer parameters have an implicit "def" (their incoming value); flagged immutable iff
    // the caller proved their type a `String`/`Bytes` leaf.
    for (p, &imm) in immutable_leaf_params.iter().enumerate() {
        if p < n_regs {
            has_def[p] = true;
            if !imm {
                blocked[p] = true;
            }
        }
    }
    let mark_blocked = |w: usize, has_def: &mut [bool], blocked: &mut [bool]| {
        if w < n_regs {
            has_def[w] = true;
            blocked[w] = true;
        }
    };
    for instr in code {
        match instr {
            // Immutable `String`/`Bytes` producers — record a def, do NOT block.
            RegInstr::LoadString { dst, .. } | RegInstr::StringConcat { dst, .. } => {
                if *dst < n_regs {
                    has_def[*dst] = true;
                }
            }
            RegInstr::CallIntrinsic { dst, intrinsic, .. }
                if native_intrinsic_produces_immutable_leaf(intrinsic) =>
            {
                if *dst < n_regs {
                    has_def[*dst] = true;
                }
            }
            // Aliasing copy: immutability flows from the source (resolved at fixpoint).
            RegInstr::Move { dst, src } => {
                if *dst < n_regs {
                    has_def[*dst] = true;
                    move_srcs[*dst].push(*src);
                }
            }
            // Any other writer produces a non-`String`/`Bytes` value (container, struct,
            // extraction, call result, scalar) ⇒ block its destination(s).
            other => match instr_written_reg(other) {
                RegFootprint::Some(ws) => {
                    for w in ws {
                        mark_blocked(w, &mut has_def, &mut blocked);
                    }
                }
                RegFootprint::All => {
                    // Unmodeled write footprint: cannot prove anything safely.
                    return vec![false; n_regs];
                }
            },
        }
    }
    let mut proven: Vec<bool> = (0..n_regs).map(|r| has_def[r] && !blocked[r]).collect();
    // Monotone retraction: a register stops being proven if any `Move` source is unproven.
    let mut changed = true;
    while changed {
        changed = false;
        for r in 0..n_regs {
            if proven[r] && !move_srcs[r].iter().all(|&s| s < n_regs && proven[s]) {
                proven[r] = false;
                changed = true;
            }
        }
    }
    proven
}

/// SOUNDNESS GUARD. The interpreter deep-copies every non-`mut` heap parameter at the
/// function prologue (`DeepCopy`), giving the callee an isolated copy; a `mut` param is
/// by-reference (no copy). Native lowers `DeepCopy` to a Nop. That was sound when native
/// was read-only, but native now performs in-place heap WRITES — so mutating (or leaking)
/// a DeepCopy'd param's value, directly or through an alias, would propagate to the caller
/// while the interpreter would mutate only the copy.
///
/// Returns `true` (⇒ caller must DECLINE native) if such an unsound mutation/leak is
/// possible. Taint every DeepCopy'd heap param register EXCEPT those proven to be an
/// immutable `String`/`Bytes` (`immutable_leaf_params[reg] == true`) — sharing an immutable
/// value's `Rc` is unobservable, so a `read` string stored into a `mut` collection is sound.
/// Everything else (mutable containers, structs that may hold them, and any param whose type
/// is unknown — e.g. inlined-leaf params) is tainted conservatively. Propagate the taint
/// forward through `Move` and heap-extraction ops (the result aliases the source's inner
/// `Rc`), then decline if any in-scope native op (a) mutates a tainted receiver in place,
/// (b) passes a tainted value as a `mut` call arg, (c) STORES a tainted value into a
/// container/struct (it would be reloaded as the caller's original `Rc` and mutated — the
/// store/reload leak), or (d) RETURNS a tainted value. `mutation_in_scope(i)` selects the
/// instructions that actually run natively (whole function vs. the OSR region).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_deepcopy_param_unsoundly_mutated(
    code: &[RegInstr],
    ty: &[Option<NativeTy>],
    n_regs: usize,
    immutable_leaf_params: &[bool],
    mutation_in_scope: impl Fn(usize) -> bool,
) -> bool {
    // `DeepCopy` is emitted ONLY for non-`mut`, non-Copy (i.e. heap) parameters — scalars
    // are Copy and never deep-copied (RS0008) — so every DeepCopy'd register is a heap root.
    // Seed one taint set from every DeepCopy'd root that is NOT proven to hold an immutable
    // `String`/`Bytes` (sharing an immutable value's `Rc` is unobservable). The proven-
    // immutable analysis runs over the post-inlining code, so it classifies INLINED-leaf
    // params too (via their arg-marshalling `Move`) — a `read List` param spliced in from a
    // callee is unproven and thus tainted, closing the inlined store/reload leak; a `read
    // String` one is proven and stays untainted.
    let proven_immutable = native_reg_proven_immutable_leaf(code, n_regs, immutable_leaf_params);
    let mut tainted = vec![false; n_regs];
    let mut any = false;
    for instr in code {
        if let RegInstr::DeepCopy { reg } | RegInstr::DeepCopyElided { reg } = instr {
            // Seed only HEAP roots that are not proven immutable. The `native_is_heap_reg`
            // gate matters for a generic `read T` param: the lowerer emits `DeepCopy` for it
            // unconditionally, but when `T` instantiates to a scalar the register is typed
            // `Int`/`Float` and cannot alias — so it must not be tainted. A genuinely-heap
            // mutated param is typed `Handle`/`Flat*` and is still seeded. (An untyped heap
            // param is unused — hence never mutated — so skipping it is sound.)
            if *reg < n_regs && native_is_heap_reg(ty, *reg) && !proven_immutable[*reg] {
                tainted[*reg] = true;
                any = true;
            }
        }
    }
    if !any {
        return false;
    }
    // Forward alias closure: `dst` aliases `src`'s inner `Rc` (heap dsts only).
    let mut changed = true;
    while changed {
        changed = false;
        for instr in code {
            if let RegInstr::Move { dst, src }
            | RegInstr::ListGet { dst, list: src, .. }
            | RegInstr::MapGet { dst, map: src, .. }
            | RegInstr::GetField { dst, base: src, .. }
            | RegInstr::GetFieldSlot { dst, base: src, .. }
            | RegInstr::UnwrapVariantValue { dst, src, .. }
            | RegInstr::DequePopFront { dst, deque: src }
            | RegInstr::DequePopBack { dst, deque: src } = instr
                && *src < n_regs
                && *dst < n_regs
                && tainted[*src]
                && !tainted[*dst]
                && native_is_heap_reg(ty, *dst)
            {
                tainted[*dst] = true;
                changed = true;
            }
        }
    }
    let is_tainted = |r: &usize| *r < n_regs && tainted[*r];
    for (i, instr) in code.iter().enumerate() {
        if !mutation_in_scope(i) {
            continue;
        }
        // (a) IN-PLACE mutation of a tainted heap container — the leak: native mutates the
        // shared `Rc` the interpreter would have left untouched (it mutated only the copy).
        if let Some(recv) = native_heap_mutation_receiver(instr)
            && recv < n_regs
            && tainted[recv]
        {
            return true;
        }
        // (b) passing a tainted value as a `mut` arg to a (non-inlined) call: the callee
        // mutates it by reference, leaking to our caller. (`read` args are safe.)
        if let RegInstr::CallKnown { args, mut_args, .. }
        | RegInstr::CallClosure { args, mut_args, .. } = instr
            && mut_args
                .iter()
                .any(|&p| args.get(p).is_some_and(is_tainted))
        {
            return true;
        }
        // (c) STORING a tainted value into caller-visible heap, or (d) RETURNING it. Native
        // would store/return the caller's original `Rc` (the interpreter stores/returns the
        // deep copy); the value can then be reloaded and mutated, or mutated by the caller
        // through the escaping aggregate. Taint already excludes proven-immutable
        // `String`/`Bytes`, so this does not decline the sound `read` string → `mut`
        // collection pattern.
        match instr {
            RegInstr::Return { src } if is_tainted(src) => return true,
            RegInstr::SetFieldSlot { value, .. }
            | RegInstr::ListSet { value, .. }
            | RegInstr::ListPush { value, .. }
            | RegInstr::SetInsert { value, .. }
            | RegInstr::SortedSetInsert { value, .. }
            | RegInstr::DequePushBack { value, .. }
            | RegInstr::DequePushFront { value, .. }
            | RegInstr::MakeSome { value, .. } => {
                if is_tainted(value) {
                    return true;
                }
            }
            RegInstr::ListAppend { values, .. } => {
                if is_tainted(values) {
                    return true;
                }
            }
            RegInstr::MapInsert { key, value, .. }
            | RegInstr::SortedMapInsert { key, value, .. } => {
                if is_tainted(key) || is_tainted(value) {
                    return true;
                }
            }
            RegInstr::MakeStruct { fields, .. } | RegInstr::MakeVariant { fields, .. } => {
                if fields.iter().any(|(_, r)| is_tainted(r)) {
                    return true;
                }
            }
            RegInstr::MakeList { items, .. } => {
                if items.iter().any(is_tainted) {
                    return true;
                }
            }
            RegInstr::MakeMap { entries, .. }
                if entries.iter().any(|(k, v)| is_tainted(k) || is_tainted(v)) =>
            {
                return true;
            }
            _ => {}
        }
    }
    false
}

/// Whether a declared HIR parameter type names a proven-immutable heap leaf — a `String` or
/// `Bytes` (or their view forms) — whose `Rc` is safe to share because it can neither be
/// mutated nor hold a mutable sub-value. Anything else (mutable containers, user structs/
/// variants that may contain them, or an unrecognized name) is NOT proven immutable, so the
/// [`native_deepcopy_param_unsoundly_mutated`] guard treats it conservatively.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_declared_type_is_immutable_leaf(type_name: &str) -> bool {
    matches!(
        type_name.trim(),
        "String" | "Bytes" | "StringView" | "BytesView"
    )
}

/// Assign type `t` to register `reg`; return `false` on a conflicting reassignment.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_set_ty(
    ty: &mut [Option<NativeTy>],
    reg: usize,
    t: NativeTy,
    changed: &mut bool,
) -> bool {
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
pub(in crate::reg_vm) fn native_unify(
    ty: &mut [Option<NativeTy>],
    a: usize,
    b: usize,
    changed: &mut bool,
) -> bool {
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
pub(in crate::reg_vm) fn native_reachable_instructions(code: &[RegInstr]) -> Vec<bool> {
    if code.is_empty() {
        return Vec::new();
    }
    NativeRegionCfg::prefix(code, code.len())
        .map(|cfg| cfg.reachable_mask())
        .unwrap_or_else(|| vec![false; code.len()])
}

/// Clone a *pure* (branch-free, call-free) native-subset instruction with every
/// register shifted by `base` — used to splice a callee body into the caller's
/// register window during inlining. `None` for anything outside that pure subset.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_offset_regs(instr: &RegInstr, b: usize) -> Option<RegInstr> {
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
        RegInstr::DeepCopyElided { reg } => RegInstr::DeepCopyElided { reg: reg + b },
        RegInstr::GetFieldSlot { dst, base, slot } => RegInstr::GetFieldSlot {
            dst: dst + b,
            base: base + b,
            slot: *slot,
        },
        RegInstr::SetFieldSlot {
            dst,
            base,
            slot,
            value,
        } => RegInstr::SetFieldSlot {
            dst: dst + b,
            base: base + b,
            slot: *slot,
            value: value + b,
        },
        RegInstr::NativeFieldClosureId { dst, base, slot } => RegInstr::NativeFieldClosureId {
            dst: dst + b,
            base: base + b,
            slot: *slot,
        },
        RegInstr::NativeFieldClosureCapture {
            dst,
            base,
            slot,
            index,
        } => RegInstr::NativeFieldClosureCapture {
            dst: dst + b,
            base: base + b,
            slot: *slot,
            index: *index,
        },
        RegInstr::ListLen { dst, list } => RegInstr::ListLen {
            dst: dst + b,
            list: list + b,
        },
        RegInstr::ListGet { dst, list, index } => RegInstr::ListGet {
            dst: dst + b,
            list: list + b,
            index: index + b,
        },
        RegInstr::ListSet {
            dst,
            list,
            index,
            value,
        } => RegInstr::ListSet {
            dst: dst + b,
            list: list + b,
            index: index + b,
            value: value + b,
        },
        RegInstr::ListPush { dst, list, value } => RegInstr::ListPush {
            dst: dst + b,
            list: list + b,
            value: value + b,
        },
        RegInstr::ListSort { dst, list } => RegInstr::ListSort {
            dst: dst + b,
            list: list + b,
        },
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

/// Register-offset an instruction for the OSR×inline path, which (unlike the plain
/// native subset) ALSO accepts the scalar replacement-dissolvable value ops: a callee that builds /
/// destructures a non-escaping `Option`/variant/struct is spliced into the loop body
/// so the scalar replacement region passes can dissolve it to scalars. The branch-shaped match ops
/// (`MatchOption`/`MatchResult`/`MatchVariant`/`MatchMapGet`) are remapped by the
/// splicer (they carry callee ip targets), not here. `None` if neither the native
/// subset nor a scalar replacement value op.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_offset_regs_j3(instr: &RegInstr, b: usize) -> Option<RegInstr> {
    if let Some(offset) = native_offset_regs(instr, b) {
        return Some(offset);
    }
    Some(match instr {
        RegInstr::MakeVariant {
            dst,
            layout,
            fields,
        } => RegInstr::MakeVariant {
            dst: dst + b,
            layout: layout.clone(),
            fields: fields.iter().map(|(n, r)| (n.clone(), r + b)).collect(),
        },
        RegInstr::MakeStruct {
            dst,
            layout,
            fields,
        } => RegInstr::MakeStruct {
            dst: dst + b,
            layout: layout.clone(),
            fields: fields.iter().map(|(n, r)| (n.clone(), r + b)).collect(),
        },
        RegInstr::UnwrapVariantValue { dst, src, expected } => RegInstr::UnwrapVariantValue {
            dst: dst + b,
            src: src + b,
            expected: expected.clone(),
        },
        RegInstr::MakeSome { dst, value } => RegInstr::MakeSome {
            dst: dst + b,
            value: value + b,
        },
        RegInstr::LoadNone { dst } => RegInstr::LoadNone { dst: dst + b },
        RegInstr::UnwrapSome { dst, src } => RegInstr::UnwrapSome {
            dst: dst + b,
            src: src + b,
        },
        _ => return None,
    })
}

/// Whether `intrinsic` is a PURE, side-effect-free heap value-builder that the
/// deopt-before-heap cold-arm classifier is allowed to see inside a cold arm. These
/// allocate a fresh `String` from their (read-only) operands and observe/mutate
/// nothing else — so re-running the arm on the interpreter after a native `Bail`
/// reproduces it exactly (the transactional fallback contract). Deliberately a tight whitelist: any
/// intrinsic that touches I/O, the environment, collections, time, or RNG is impure
/// and must NOT appear in a bailable cold arm. Unknown ⇒ false (reject).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn cold_arm_pure_intrinsic(intrinsic: &RegIntrinsic) -> bool {
    // Classification reads the central registry's two cold-arm whitelists: the pure heap
    // BUILDERS (`cold_arm_pure_builder` — StringCopy/StringFrom*/StringSlice/StringPadLeft/
    // BytesFromString/BytesSlice) and the pure first-order scalar READERS
    // (`cold_arm_pure_reader` — String.count/contains/index_of/starts_with). Both are
    // side-effect-free and faithfully re-runnable on the interpreter after a native
    // `Bail`; the cold-arm pass keeps its exact arm-detection mechanism.
    let d = intrinsic_descriptor(*intrinsic);
    // Also admit the Option/Result COMBINATORS (`Option.map`/`and_then`/`unwrap_or`,
    // `Result.*`). They are higher-order (invoke a closure), but that is safe in a bailable
    // cold arm: native never executes the arm, so the combinator and its closure run ONLY on
    // the interpreter replay — any effect of the closure happens exactly as without the JIT.
    d.cold_arm_pure_builder || d.cold_arm_pure_reader || d.combinator_kind.is_some()
}

/// Whether `instr` is a pure, side-effect-free value-construction instruction that
/// the deopt-before-heap cold-arm classifier permits inside a bailable cold arm. It
/// covers the scalar replacement-dissolvable value ops (`native_offset_regs_j3`-class), plus the
/// recognized pure HEAP value-builders a cold arm may use to construct its returned
/// value: `LoadString`, a whitelisted pure `CallIntrinsic` (e.g. `String.copy`),
/// `StringConcat`, and `MakeVariant`/`MakeStruct` (the `Err(..)` / record the arm
/// returns). Every such op only READS its operands and writes its single `dst`; none
/// has an observable effect, so an arm built from them can be discarded by a native
/// `Bail` and faithfully re-run on the interpreter. `Move` is included (scalar copy).
/// Branches, returns, calls, suspends, and collection mutators are NOT pure here.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn cold_arm_pure_value_op(instr: &RegInstr) -> bool {
    if native_offset_regs_j3(instr, 0).is_some() {
        return true;
    }
    match instr {
        RegInstr::LoadString { .. }
        | RegInstr::LoadUnit { .. }
        | RegInstr::StringConcat { .. }
        | RegInstr::MakeVariant { .. }
        | RegInstr::MakeStruct { .. }
        | RegInstr::MakeSome { .. }
        // Collection build / mutate / read: a `List`/`Map`/`Set`/`Deque` the arm builds,
        // writes, and queries, e.g. `let t = []; t.push(x); return List.len(t)`. The
        // insert ops are heap WRITES but are SAFE in a deopt-replaceable cold arm — native
        // bails at the arm start `s` and NEVER executes the arm, and a cold-arm `Bail`
        // always takes the abort+replay fallback (see the soundness note in
        // `deopt_replaceable_cold_arms`), so the interpreter re-runs the whole arm and
        // performs the write itself. This holds even for caller-aliased collections.
        // `ListAppend` etc. are intentionally NOT yet admitted — extend per-case.
        | RegInstr::MakeList { .. }
        | RegInstr::ListPush { .. }
        | RegInstr::ListLen { .. }
        | RegInstr::MakeMap { .. }
        | RegInstr::MapInsert { .. }
        | RegInstr::SetInsert { .. }
        | RegInstr::DequePushBack { .. }
        | RegInstr::DequePushFront { .. }
        // A closure construction: builds a closure value the arm passes to a combinator.
        // Sound in a bailable cold arm by the same argument — native never executes the arm;
        // the interpreter rebuilds the closure and runs it once on replay.
        | RegInstr::MakeClosure { .. } => true,
        RegInstr::CallIntrinsic { intrinsic, .. } => cold_arm_pure_intrinsic(intrinsic),
        // A nested CALL to a known function is admissible in a bailable cold arm: native
        // never executes the arm (it bails at `s`), and a cold-arm `Bail` always takes the
        // abort+replay fallback (see the soundness note in `deopt_replaceable_cold_arms`),
        // so the interpreter runs the call ONCE on replay — correct even if the callee does
        // I/O, allocates, or mutates (those effects happen only on the interpreter, exactly
        // as without the JIT). `mut`-arg calls are included: the writeback into the caller's
        // register only ever happens on the cold/bail path (the interpreter replay), never
        // in native — the same situation as a caller-aliased heap write, which is likewise
        // sound under abort+replay.
        RegInstr::CallKnown { .. } => true,
        _ => false,
    }
}

/// Deopt-before-heap classifier. Identify every **deopt-replaceable cold arm** in a
/// callee about to be inlined into an OSR loop: a maximal straight-line suffix
/// `[s..=e]` such that
///   1. `code[e]` is a `Return`,
///   2. every instruction in `[s..=e]` is a pure value-construction op
///      ([`cold_arm_pure_value_op`]) — including the recognized pure HEAP builders —
///      and none falls outside that set (no branch/call/suspend/collection-mutation),
///   3. control enters the arm ONLY at `s` via a branch boundary: no reachable
///      instruction OUTSIDE `[s..=e]` jumps or falls into any ip in `[s..=e]` (the
///      interior is private to the arm, and `s` is reached purely as the taken/
///      not-taken edge of a preceding branch — never by the native path falling
///      straight through), and
///   4. register isolation: no register written inside `[s..=e]` is read by any
///      reachable instruction OUTSIDE `[s..=e]`.
///
/// Such an arm is replaced at splice time by a single native `Bail` (a `RuntimeError`
/// sentinel) at `s`: because every op in the arm is side-effect-free, native does
/// NOTHING observable before bailing, so the existing abandon-and-reinterpret-the-
/// loop fallback re-runs the whole loop on the interpreter — which rebuilds the heap
/// value itself (the transactional fallback contract holds unchanged; no rollback, no resume-ip).
///
/// Returns `(cold, arm_start)` where `cold[i]` marks every ip in some cold arm and
/// `arm_start[i]` marks the single `s` of each arm (where the `Bail` is emitted). The
/// caller treats a reachable, non-cold instruction that is neither native-subset nor
/// a scalar replacement op / match / branch as a veto (no inline). Conservative throughout: anything
/// not provably a clean pure tail is simply NOT marked cold (so it must be otherwise
/// classifiable, else the inline is vetoed).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn deopt_replaceable_cold_arms(
    code: &[RegInstr],
    reachable: &[bool],
) -> (Vec<bool>, Vec<bool>) {
    let n = code.len();
    let mut cold = vec![false; n];
    let mut arm_start = vec![false; n];

    // Find each reachable `Return` and walk backward to the maximal pure-value
    // straight-line prefix. A candidate arm `[s..=e]` is only ACCEPTED after the
    // entry + isolation checks below; failure leaves it unmarked (the inline gate
    // then decides whether the leaf is still classifiable some other way).
    for e in 0..n {
        if !reachable[e] || !matches!(code[e], RegInstr::Return { .. }) {
            continue;
        }
        if cold[e] {
            continue; // already part of an accepted arm (returns can't overlap)
        }
        // Extend `s` downward while the predecessor is a pure value op. Stop at the
        // first non-pure / non-reachable instruction (or the function start).
        let mut s = e;
        while s > 0 && reachable[s - 1] && cold_arm_pure_value_op(&code[s - 1]) {
            s -= 1;
        }
        // The arm must build a heap value that the scalar replacement region passes CANNOT dissolve —
        // otherwise it is a SUPPORTED arm (e.g. the `Ok(scalar)` arm, a
        // `MakeVariant`/`MakeStruct`/`MakeSome` with a scalar payload) that should
        // inline normally and dissolve, NOT bail. The undissolvable builders are the
        // String/heap-scalar producers (`LoadString`, `StringConcat`, a `CallIntrinsic`
        // such as `String.copy`): a `MakeVariant{Err, [String]}` is only cold because
        // its payload flows from one of these. Require at least one such op in
        // `[s..=e]`; otherwise leave the arm to the normal scalar replacement path (do not mark cold).
        let has_undissolvable_heap_builder = (s..=e).any(|j| {
            matches!(
                &code[j],
                RegInstr::LoadString { .. }
                    | RegInstr::StringConcat { .. }
                    | RegInstr::CallIntrinsic { .. }
                    | RegInstr::CallTypedIntrinsic { .. }
                    | RegInstr::MakeList { .. }
                    | RegInstr::MakeMap { .. }
                    | RegInstr::CallKnown { .. }
            )
        });
        if !has_undissolvable_heap_builder {
            continue;
        }
        // Entry/interior isolation: no reachable instruction OUTSIDE `[s..=e]` may
        // transfer control into the interior `(s..=e]`, and the native path must not
        // fall straight into `s`. We check both by enumerating every reachable
        // instruction's control-flow successors and rejecting any that lands in
        // `(s..=e]` from outside, plus requiring the textual predecessor `s-1` (if
        // reachable) to be a NON-fallthrough terminator/branch so the native path
        // cannot fall into `s`.
        let in_arm = |ip: usize| ip >= s && ip <= e;
        let mut ok = true;
        // `s` must be entered only via an explicit branch edge: its textual
        // predecessor must not fall through into it. A `JumpIf*`/`Match*` whose
        // not-taken/fallthrough edge is `s` IS an explicit branch boundary (the arm
        // is the cold side), which is exactly the shape we want — those are allowed.
        // A plain pure op or `Jump`/`Return`/`RuntimeError` predecessor: a pure op
        // would fall through into `s` on the native path (reject); `Jump`/`Return`/
        // `RuntimeError`/match/branch do not fall through, so `s` is only reached via
        // an explicit target/edge (allow). `s == 0` is a function entry (allow only
        // if entry is genuinely a branch target, which for a leaf it is not — but a
        // whole-body cold function would have been native-translated already; be
        // conservative and reject `s == 0`).
        if s == 0 || reachable[s - 1] && !native_instr_is_control_boundary(&code[s - 1]) {
            ok = false;
        }
        // No external control-flow edge into the interior `(s..=e]`. (An edge to `s`
        // itself from a preceding branch is fine — that is the cold-arm entry.)
        if ok {
            for (ip, instr) in code.iter().enumerate() {
                if !reachable[ip] || in_arm(ip) {
                    continue;
                }
                let lands_inside = |t: usize| t > s && t <= e;
                let mut hits = false;
                native_instr_successors(instr, ip, n, |target| {
                    if lands_inside(target) {
                        hits = true;
                    }
                });
                if hits {
                    ok = false;
                    break;
                }
            }
        }
        // Register isolation: every register the arm WRITES must be dead outside the
        // arm (read by no reachable out-of-arm instruction). A write we cannot model
        // (`RegFootprint::All`) cannot occur — the arm is all pure value ops — but be
        // defensive and reject if it ever appears.
        if ok {
            let mut written: Vec<usize> = Vec::new();
            for j in s..=e {
                match instr_written_reg(&code[j]) {
                    RegFootprint::Some(ws) => written.extend(ws),
                    RegFootprint::All => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                'outer: for (ip, instr) in code.iter().enumerate() {
                    if !reachable[ip] || in_arm(ip) {
                        continue;
                    }
                    match instr_read_regs(instr) {
                        RegFootprint::Some(rs) => {
                            for r in rs {
                                if written.contains(&r) {
                                    ok = false;
                                    break 'outer;
                                }
                            }
                        }
                        RegFootprint::All => {
                            ok = false;
                            break 'outer;
                        }
                    }
                }
            }
        }
        // Heap WRITE ops (`ListPush`/`MapInsert`/`SetInsert`/`DequePush*`) ARE permitted
        // in the arm, INCLUDING writes to a caller-aliased / live-in collection. Soundness
        // (no arm-local restriction needed):
        //   1. Cold-arm splicing only ever runs on an INLINED leaf (`deopt_replaceable_
        //      cold_arms` is called exclusively on a `callee.code` being inlined), so a
        //      cold-arm `Bail` always lives inside a spliced region.
        //   2. Splicing pushes the origin call-ip for every spliced instruction, so the
        //      `ip_map` is NON-IDENTITY → `precise_resume_safe` is forced off
        //      (translate.rs), and the precise-resume deopt path (tier.rs) is structurally
        //      unreachable for a cold-arm `Bail`.
        //   3. The OSR-exit handler honours a deopt only at the clean post-loop exit
        //      (`resume_ip == trans_exit`); ANY mid-loop bail (a cold-arm `Bail`) takes the
        //      fallback that `heap_tx.abort()`s and re-runs the loop on the interpreter.
        // So native NEVER executes the arm and ALL of its journaled native heap writes (incl.
        // aliased in-place writes elsewhere in the loop, the transactional fallback contract) are rolled back before the
        // interpreter replays — the interpreter performs the arm's write itself. Verified:
        // the aliased directed test OSRs + matches the interpreter, and the full
        // differential/soak suites stay green. (Register isolation above still applies to
        // the arm's REGISTER writes.)
        if ok {
            for j in s..=e {
                cold[j] = true;
            }
            arm_start[s] = true;
        }
    }

    (cold, arm_start)
}

/// Whether `callee` can be inlined into the OSR loop body: like
/// [`native_callee_inlinable`] but ALSO permitting the scalar replacement-dissolvable value ops
/// ([`native_offset_regs_j3`]) and the branch-shaped match ops, so a leaf that
/// builds/destructures a non-escaping `Option`/variant/struct (e.g.
/// `make_shape`/`area`) qualifies — the value becomes loop-local once inlined and
/// the scalar replacement region passes dissolve it. Still captureless, arity-matched, side-effect-
/// free (no calls/suspends/heap-collection/runtime-error/non-scalar replacement ops).
///
/// Deopt-before-heap extension: a reachable instruction that is part of a
/// **deopt-replaceable cold arm** ([`deopt_replaceable_cold_arms`]) is also accepted
/// — at splice time that arm is replaced by a native `Bail`, so a leaf whose COLD arm
/// builds a heap value (e.g. `Err(String.copy(..))`) qualifies as long as its
/// SUPPORTED (non-cold) arms are fully native/scalar replacement-dissolvable. When unsure, REJECT.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_callee_inlinable_j3(callee: &RegFunction, n_args: usize) -> bool {
    if callee.captures != 0 || callee.params != n_args {
        return false;
    }
    let reachable = native_reachable_instructions(&callee.code);
    let (cold, _arm_start) = deopt_replaceable_cold_arms(&callee.code, &reachable);
    callee.code.iter().enumerate().all(|(i, instr)| {
        !reachable[i]
            || cold[i]
            || matches!(
                instr,
                RegInstr::Jump { .. }
                    | RegInstr::JumpIfBool { .. }
                    | RegInstr::JumpIfIntCompare { .. }
                    | RegInstr::Return { .. }
                    | RegInstr::RuntimeError { .. }
                    | RegInstr::MatchOption { .. }
                    | RegInstr::MatchResult { .. }
                    | RegInstr::MatchVariant { .. }
                    | RegInstr::MatchMapGet { .. }
                    | RegInstr::MatchSortedMapGet { .. }
            )
            || native_offset_regs_j3(instr, 0).is_some()
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_callee_inlinable_j3_with_spawns(
    unit: &RegUnit,
    callee: &RegFunction,
    n_args: usize,
) -> bool {
    fn go(unit: &RegUnit, callee: &RegFunction, n_args: usize, depth: usize) -> bool {
        if depth > unit.functions.len() || callee.captures != 0 || callee.params != n_args {
            return false;
        }
        let reachable = native_reachable_instructions(&callee.code);
        let (cold, _arm_start) = deopt_replaceable_cold_arms(&callee.code, &reachable);
        let mut direct_spawn_results: Vec<usize> = Vec::new();
        let mut direct_await_results: Vec<usize> = Vec::new();
        for (i, instr) in callee.code.iter().enumerate() {
            if !reachable[i] || cold[i] {
                continue;
            }
            match instr {
                RegInstr::SpawnTask {
                    dst,
                    function,
                    args,
                } => {
                    let Some(spawned) = unit.functions.get(*function) else {
                        return false;
                    };
                    if !go(unit, spawned, args.len(), depth + 1) {
                        return false;
                    }
                    direct_spawn_results.push(*dst);
                }
                RegInstr::AwaitJoin { dst, src } if direct_spawn_results.contains(src) => {
                    direct_await_results.push(*dst);
                }
                RegInstr::Move { dst, src } if direct_spawn_results.contains(src) => {
                    direct_spawn_results.push(*dst);
                }
                RegInstr::TryResult { src, cleanup, .. }
                    if cleanup.is_empty() && direct_await_results.contains(src) => {}
                _ => {
                    let ok = matches!(
                        instr,
                        RegInstr::Jump { .. }
                            | RegInstr::JumpIfBool { .. }
                            | RegInstr::JumpIfIntCompare { .. }
                            | RegInstr::Return { .. }
                            | RegInstr::RuntimeError { .. }
                            | RegInstr::MatchOption { .. }
                            | RegInstr::MatchResult { .. }
                            | RegInstr::MatchVariant { .. }
                            | RegInstr::MatchMapGet { .. }
                            | RegInstr::MatchSortedMapGet { .. }
                    ) || native_offset_regs_j3(instr, 0).is_some();
                    if !ok {
                        return false;
                    }
                }
            }
        }
        true
    }

    go(unit, callee, n_args, 0)
}

/// Return a copy of `callee` with the string-length-law fold applied to its WHOLE body,
/// or `None` if the fold is a no-op (the common case — use the original callee then).
///
/// Motivation (#7 foldable cold-arm sub-case): a leaf whose only heap is a measured
/// throwaway string (e.g. an `if`-arm `return String.len(String.from_int(x))`) is NOT
/// leaf-inlinable as written — the heap builder makes `deopt_replaceable_cold_arms`
/// reject the arm (its `Return` value is live), so the loop calling it declines OSR.
/// The string-fold (already used in-region, semantics-preserving) dissolves
/// `String.len`-of-foldable into digit-count/byte-length arithmetic and DELETES the dead
/// allocation, turning such a body into pure native-subset scalar code. Folding the
/// callee BEFORE the inlinability check + splice lets it inline and the loop OSR, with NO
/// deopt involved (there is no longer a heap arm to bail on). The fold is
/// semantics-preserving, so splicing the folded body is always correct; at worst an
/// un-foldable body returns `None` and nothing changes.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_string_folded_callee(callee: &RegFunction) -> Option<RegFunction> {
    if callee.code.is_empty() {
        return None;
    }
    // Chain the semantics-preserving length-law folds over the WHOLE callee body: first
    // the string-length fold (`String.len`-of-foldable → digit/concat/slice arithmetic),
    // then the Bytes-length fold (`Bytes.len`-of-foldable → byte-length arithmetic) on its
    // result. Both DELETE the dissolved allocation, so a measured-throwaway string OR bytes
    // cold arm becomes pure native-subset scalar code. Each is a no-op for a body lacking
    // its pattern, returning the input unchanged.
    let (s_code, s_regs, _s_map) =
        native_string_length_fold_in_region(&callee.code, callee.regs, 0, callee.code.len())?;
    let (folded_code, folded_regs, _b_map) =
        native_bytes_length_fold_in_region(&s_code, s_regs, 0, s_code.len())?;
    // No-op chain ⇒ original (a real fold shrinks the stream and/or grows the reg file;
    // equal length AND regs vs the ORIGINAL ⇒ nothing folded by either pass).
    if folded_code.len() == callee.code.len() && folded_regs == callee.regs {
        return None;
    }
    let mut folded = callee.clone();
    folded.code = folded_code;
    folded.regs = folded_regs;
    Some(folded)
}

/// Whether `callee` can be inlined into a native function: captureless, arity
/// matches, and every reachable instruction is a pure native-subset op, native
/// control flow (jump/branch), or a `Return`. Unlike the original straight-line
/// restriction this permits internal branches and loops; calls, suspends,
/// matches, heap ops and runtime errors still make the caller fall back.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_callee_inlinable(callee: &RegFunction, n_args: usize) -> bool {
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
