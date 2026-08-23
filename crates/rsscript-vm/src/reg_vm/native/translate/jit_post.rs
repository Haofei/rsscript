//! Post-lowering analyses and rewrites for native JIT instruction streams.

// These analyses intentionally keep instruction indices aligned across rewrites.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

use super::*;

#[cfg(feature = "native-jit")]
pub(super) fn native_compose_ip_maps(
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
pub(super) fn native_forward_direct_list_store_loads(jit_code: &mut [vm_jit::JitInstr]) {
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
pub(super) fn native_split_len_sources(
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
pub(super) fn native_pad_left_len_sources(
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
            ok.fill(false);
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
                ok.fill(false);
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
pub(super) fn native_memoize_loop_invariant_runtime_helper_calls(
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
            let Some((helper, dst, args)) = native_memoizable_runtime_helper_call(&jit_code[ip])
            else {
                continue;
            };
            let dst = *dst;
            let args = args.clone();
            let field_load_eligible = native_memoizable_field_load_helper(helper)
                && native_field_load_args_loop_stable(
                    &args,
                    &invariants,
                    jit_code,
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
                    jit_code,
                    heap_provenance.as_ref(),
                    lp.header,
                    lp.exit,
                    ip,
                );
            let args_loop_stable = if field_load_eligible {
                true
            } else {
                native_runtime_helper_args_loop_invariant(&args, &invariants, ip)
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
            if !(native_memoizable_result_type(helper, result_ty)
                || field_load_eligible
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
                && let Some(derived) = invariants.derived_invariant.get_mut(dst as usize)
            {
                *derived = true;
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
fn native_memoizable_runtime_helper_call(
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
#[allow(dead_code)]
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
pub(super) fn native_jit_written_reg(instr: &vm_jit::JitInstr) -> Option<u32> {
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
fn native_runtime_helper_args_loop_invariant(
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
        && let Some(derived) = invariants.derived_invariant.get_mut(*dst)
    {
        *derived = true;
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
    }) && !(header..exit).any(|ip| match code.get(ip) {
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
    })
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
