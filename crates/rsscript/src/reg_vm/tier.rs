use super::*;

#[cfg(feature = "native-jit")]
fn osr_loop_region_is_native_subset(code: &[RegInstr], lp: OsrLoop, n_params: usize) -> bool {
    code.get(lp.header..lp.exit).is_some_and(|region| {
        let region_defs = native_osr_region_defined_regs(region);
        region.iter().all(|instr| {
            native_subset_instruction(instr)
                && native_osr_growth_admissible(instr, &region_defs, n_params)
        })
    })
}

/// Registers bound to a freshly-produced value INSIDE a region. An in-place
/// `ListPush` mutates the list object without rebinding the list register, so it is
/// excluded — it must not count as a fresh definition of its target list.
#[cfg(feature = "native-jit")]
fn native_osr_region_defined_regs(region: &[RegInstr]) -> std::collections::HashSet<usize> {
    let mut defs = std::collections::HashSet::new();
    for instr in region {
        if matches!(instr, RegInstr::ListPush { .. }) {
            continue;
        }
        if let RegFootprint::Some(regs) = instr_written_reg(instr) {
            defs.extend(regs);
        }
    }
    defs
}

/// Whether a growing list mutation (`ListPush`) is safe to run in an OSR loop.
///
/// Growth reallocates a list's backing buffer. The ONLY lists pinned as a raw flat
/// buffer (whose pointer a realloc would dangle) are flat `List<Int>`/`List<Float>`
/// **parameters** (`reg < n_params`; the TV2 flat classification is params-only — see
/// `flat_param_kind` in `native/translate.rs`). So a `ListPush` is admissible when its
/// target list is EITHER region-local (built natively, never pinned) OR a non-parameter
/// register (`reg >= n_params`, a function-local accumulator that is handle-accessed,
/// not flat-pinned — e.g. a `List<String>` builder). A PARAMETER list stays vetoed
/// (conservative: it may be the pinned flat buffer; growing it is UB). This unblocks
/// native heap-value list building (J0.4 #1) while preserving flat-param safety and the
/// outer-loop selection for flat-param builder/consumer shapes.
#[cfg(feature = "native-jit")]
fn native_osr_growth_admissible(
    instr: &RegInstr,
    region_defs: &std::collections::HashSet<usize>,
    n_params: usize,
) -> bool {
    match instr {
        RegInstr::ListPush { list, .. } => region_defs.contains(list) || *list >= n_params,
        _ => true,
    }
}

#[cfg(feature = "native-jit")]
fn osr_loop_region_needs_optimized_native_subset_path(code: &[RegInstr], lp: OsrLoop) -> bool {
    code.get(lp.header..lp.exit)
        .is_some_and(|region| region.iter().any(native_instruction_touches_field_slot))
}

#[cfg(feature = "native-jit")]
fn mapped_osr_loop(code: &[RegInstr], ip_map: &[usize], old_header: usize) -> Option<OsrLoop> {
    let header = ip_map.iter().position(|&old| old == old_header)?;
    detect_natural_loop_at(code, header)
}

#[cfg(feature = "native-jit")]
fn osr_heap_input_regs(jit_fn: &vm_jit::JitFunction) -> Vec<usize> {
    let mut regs = Vec::new();
    let mut push_reg = |reg: u32| {
        let reg = reg as usize;
        if jit_fn.reg_types.get(reg) == Some(&vm_jit::JitValueType::Handle) && !regs.contains(&reg)
        {
            regs.push(reg);
        }
    };
    for instr in &jit_fn.code {
        match instr {
            vm_jit::JitInstr::HostCall { args, .. }
            | vm_jit::JitInstr::MemoizedHostCall { args, .. } => {
                for arg in args {
                    if let vm_jit::HostArg::Reg(reg) = arg {
                        push_reg(*reg);
                    }
                }
            }
            vm_jit::JitInstr::MatchMapGetInt { map, .. }
            | vm_jit::JitInstr::MatchSortedMapGetInt { map, .. }
            | vm_jit::JitInstr::GuardClosureId { base: map, .. } => push_reg(*map),
            _ => {}
        }
    }
    regs
}

#[cfg(feature = "native-jit")]
fn record_native_compile_stats(
    native: &mut NativeState,
    id: vm_jit::CompiledId,
    jit_fn: &vm_jit::JitFunction,
) {
    if !native.collect_stats {
        return;
    }
    let mut native_call_edges = 0;
    let mut profile_closure_guard_sites = 0;
    let mut profile_closure_id_reads = 0;
    let mut profile_closure_pic_sites = 0;
    let mut profile_closure_pic_arms = 0;
    let mut profile_branch_side_exits = 0;
    for (ip, instr) in jit_fn.code.iter().enumerate() {
        match instr {
            vm_jit::JitInstr::CallNative { .. } | vm_jit::JitInstr::CallSelf { .. } => {
                native_call_edges += 1
            }
            vm_jit::JitInstr::GuardClosureId { .. } => profile_closure_guard_sites += 1,
            vm_jit::JitInstr::ProfiledJumpIfBool { .. }
            | vm_jit::JitInstr::ProfiledJumpIfIntCompare { .. } => {
                profile_branch_side_exits += 1;
            }
            vm_jit::JitInstr::HostCall {
                helper: vm_jit::HostHelper::ClosureId,
                ..
            } => {
                profile_closure_id_reads += 1;
                profile_closure_pic_sites += 1;
                profile_closure_pic_arms += profile_closure_pic_arm_count(&jit_fn.code, ip);
            }
            _ => {}
        }
    }
    native.stats.compiled += 1;
    native.stats.compiled_ir_instrs += jit_fn.code.len() as u64;
    native.stats.compiled_code_bytes += native.module.code_size_bytes(id).unwrap_or(0);
    native.stats.profile_branch_cold_blocks += jit_fn.cold_blocks.len() as u64;
    native.stats.profile_branch_side_exits += profile_branch_side_exits;
    native.stats.native_call_edges += native_call_edges;
    native.stats.native_call_depth_max = native
        .stats
        .native_call_depth_max
        .max(native.module.native_call_depth(id).map_or(0, u64::from));
    native.stats.profile_closure_guard_sites += profile_closure_guard_sites;
    native.stats.profile_closure_id_reads += profile_closure_id_reads;
    native.stats.profile_closure_pic_sites += profile_closure_pic_sites;
    native.stats.profile_closure_pic_arms += profile_closure_pic_arms;
    native.stats.deopt_sites += native
        .module
        .deopt_map(id)
        .map_or(0, |map| map.sites.len() as u64);
}

#[cfg(feature = "native-jit")]
fn profile_closure_pic_arm_count(code: &[vm_jit::JitInstr], closure_id_ip: usize) -> u64 {
    let mut ip = closure_id_ip + 1;
    let mut arms = 0;
    while ip + 2 < code.len() {
        let arm = matches!(code[ip], vm_jit::JitInstr::LoadInt { .. })
            && matches!(code[ip + 1], vm_jit::JitInstr::Equal { .. })
            && matches!(
                code[ip + 2],
                vm_jit::JitInstr::JumpIfBool { expected: true, .. }
            );
        if !arm {
            break;
        }
        arms += 1;
        ip += 3;
    }
    arms
}

#[cfg(feature = "native-jit")]
/// Whether `jit_fn` contains a heap op that the **interpreter charges to `mem_budget`**
/// per loop iteration but native does not yet account for in generated code.
///
/// J0.5 mem parity (the native tier must charge EXACTLY what the interpreter charges,
/// no more, no less). The interpreter's only per-iteration `account_bytes` sites are
/// flat list capacity growth (`List.push`/`List.append`) and list/map LITERAL
/// construction (`MakeList`/`MakeMap`); strings, JSON, and map/set/deque/sorted-map
/// inserts are charged ZERO. Of those charged ops, only `List.push` is reachable in the
/// native subset (`MakeList`/`MakeMap`/`ListAppend` are not native-lowerable, so a loop
/// using them is not native-eligible at all). So a native OSR loop charges `mem_budget`
/// exactly the interpreter's amount UNLESS it calls a `ListPush*` helper — every other
/// allocation (string build, map/set insert, …) is uncharged by BOTH, hence parity-safe
/// to run under an armed `mem_budget`. A `ListPush*` loop still declines (native list-
/// growth byte accounting is future). NOTE: live-in `ListPush` is already vetoed by the
/// OSR growth-admissibility rule, so this is belt-and-suspenders for the region-local
/// case.
#[cfg(feature = "native-jit")]
fn jit_fn_has_unaccounted_mem_charge(jit_fn: &vm_jit::JitFunction) -> bool {
    jit_fn.code.iter().any(|instr| {
        matches!(
            instr,
            vm_jit::JitInstr::HostCall { helper, .. }
            | vm_jit::JitInstr::MemoizedHostCall { helper, .. }
                if matches!(
                    helper,
                    vm_jit::HostHelper::ListPushInt
                        | vm_jit::HostHelper::ListPushFloat
                        | vm_jit::HostHelper::ListPushHandle
                )
        )
    })
}

fn jit_function_scalar_leaf_callable(jit_fn: &vm_jit::JitFunction) -> bool {
    jit_fn.reg_types.iter().all(|ty| {
        matches!(
            ty,
            vm_jit::JitValueType::Int
                | vm_jit::JitValueType::Float
                | vm_jit::JitValueType::Handle
                | vm_jit::JitValueType::FlatInt
                | vm_jit::JitValueType::FlatFloat
        )
    }) && jit_fn.code.iter().all(|instr| {
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
                | vm_jit::JitInstr::BitAnd { .. }
                | vm_jit::JitInstr::BitOr { .. }
                | vm_jit::JitInstr::BitXor { .. }
                | vm_jit::JitInstr::Shl { .. }
                | vm_jit::JitInstr::Shr { .. }
                | vm_jit::JitInstr::Compare { .. }
                | vm_jit::JitInstr::Equal { .. }
                | vm_jit::JitInstr::NotEqual { .. }
                | vm_jit::JitInstr::Jump { .. }
                | vm_jit::JitInstr::JumpIfBool { .. }
                | vm_jit::JitInstr::JumpIfIntCompare { .. }
                | vm_jit::JitInstr::ProfiledJumpIfBool { .. }
                | vm_jit::JitInstr::ProfiledJumpIfIntCompare { .. }
                | vm_jit::JitInstr::CallNative { .. }
                | vm_jit::JitInstr::CallSelf { .. }
                | vm_jit::JitInstr::HostCall { .. }
                | vm_jit::JitInstr::MemoizedHostCall { .. }
                | vm_jit::JitInstr::Return { .. }
                | vm_jit::JitInstr::Bail
                | vm_jit::JitInstr::ListGetIntDirect { .. }
                | vm_jit::JitInstr::ListSetIntDirect { .. }
                | vm_jit::JitInstr::ListGetFloatDirect { .. }
                | vm_jit::JitInstr::ListLenDirect { .. }
        ) && !matches!(
            instr,
            vm_jit::JitInstr::HostCall { helper, .. }
                | vm_jit::JitInstr::MemoizedHostCall { helper, .. }
                if helper.heap_effect().extends_input_handles()
        )
    })
}

#[cfg(feature = "native-jit")]
fn native_ty_is_callable_param_abi(ty: NativeTy) -> bool {
    matches!(
        ty,
        NativeTy::Int
            | NativeTy::Bool
            | NativeTy::Float
            | NativeTy::Handle
            | NativeTy::FlatInt
            | NativeTy::FlatIntMut
            | NativeTy::FlatFloat
            | NativeTy::FlatFloatMut
    )
}

#[cfg(feature = "native-jit")]
fn native_ty_is_callable_return_abi(ty: NativeTy) -> bool {
    matches!(
        ty,
        NativeTy::Int | NativeTy::Bool | NativeTy::Float | NativeTy::Handle
    )
}

#[cfg(feature = "native-jit")]
fn native_call_mut_args_supported(mut_args: &[usize], param_tys: &[NativeTy]) -> bool {
    mut_args.iter().all(|&pos| {
        param_tys.get(pos).is_some_and(|ty| {
            matches!(
                ty,
                NativeTy::Handle | NativeTy::FlatIntMut | NativeTy::FlatFloatMut
            )
        })
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_scalar_callee_pending_on_branch_profile(
    func: &RegFunction,
) -> bool {
    let has_branch =
        NativeRegionAnalysis::compute_prefix(&func.code, func.regs, 0, func.code.len())
            .map(|analysis| analysis.has_reachable_conditional_branch(&func.code))
            .unwrap_or_else(|| {
                func.code.iter().any(|instr| {
                    matches!(
                        instr,
                        RegInstr::JumpIfBool { .. } | RegInstr::JumpIfIntCompare { .. }
                    )
                })
            });
    has_branch && func.branch_count.get() < PROFILE_WARMUP + PROFILE_BRANCH_MIN_SAMPLES
}

#[cfg(feature = "native-jit")]
fn native_compiled_entry_call_descriptor(
    entry: &NativeCompiledEntry,
) -> Option<NativeCompiledCallee> {
    let (id, ret_ty, param_tys, _has_backedge, scalar_leaf_callable, _literals, _precise) = entry;
    if !*scalar_leaf_callable
        || !native_ty_is_callable_return_abi(*ret_ty)
        || !param_tys
            .iter()
            .copied()
            .all(native_ty_is_callable_param_abi)
    {
        return None;
    }
    Some(NativeCompiledCallee {
        id: *id,
        ret_ty: *ret_ty,
        param_tys: param_tys.clone(),
    })
}

#[cfg(feature = "native-jit")]
fn native_compile_direct_scalar_callee(
    native: &mut NativeState,
    unit: &RegUnit,
    callee: &RegFunction,
    callee_key: usize,
    stack: &mut std::collections::HashSet<usize>,
) -> Option<NativeCompiledCallee> {
    if let Some(cached) = native.cache.get(&callee_key) {
        return cached
            .as_ref()
            .and_then(native_compiled_entry_call_descriptor);
    }
    if native_scalar_callee_pending_on_branch_profile(callee) {
        return None;
    }
    if !stack.insert(callee_key) {
        return None;
    }

    let nested_call_sites =
        native_compiled_call_sites_inner(native, unit, callee, callee_key, stack);
    let translated = if nested_call_sites.is_empty() {
        translate_to_native_jit(unit, callee)
    } else {
        translate_to_native_jit_with_compiled_callees(unit, callee, &nested_call_sites)
            .or_else(|| translate_to_native_jit(unit, callee))
    };
    let Some((jit_fn, ret, params, string_literals, precise_resume_safe)) = translated else {
        stack.remove(&callee_key);
        return None;
    };
    let scalar_leaf_callable = jit_function_scalar_leaf_callable(&jit_fn);
    if !scalar_leaf_callable
        || !native_ty_is_callable_return_abi(ret)
        || !params.iter().copied().all(native_ty_is_callable_param_abi)
    {
        stack.remove(&callee_key);
        return None;
    }

    if native.collect_stats {
        native.stats.translated += 1;
    }
    let started = native.collect_stats.then(std::time::Instant::now);
    let compiled = if native.force_all_safepoints {
        native.module.compile_forcing_all_bails(&jit_fn)
    } else {
        match native.forced_safepoint {
            Some(site) => native.module.compile_forcing_bail(&jit_fn, site),
            None => native.module.compile(&jit_fn),
        }
    };
    if let Some(started) = started {
        native.stats.compile_nanos += started.elapsed().as_nanos();
    }
    let id = match compiled {
        Ok(id) => id,
        Err(_) => {
            if native.collect_stats {
                native.stats.compile_failed += 1;
            }
            stack.remove(&callee_key);
            return None;
        }
    };
    let verify_native = cfg!(debug_assertions) || jit_native_verify_is_strict();
    if verify_native {
        if let Err(err) =
            jit_verify_compiled_native(&native.module, id, &jit_fn, native.forced_safepoint)
        {
            debug_assert!(false, "native verifier failed: {err}");
            if jit_native_verify_is_strict() {
                if native.collect_stats {
                    native.stats.compile_failed += 1;
                }
                stack.remove(&callee_key);
                return None;
            }
        }
    }
    record_native_compile_stats(native, id, &jit_fn);
    let has_backedge = jit_function_has_loop(&callee.code);
    let entry = (
        id,
        ret,
        params.clone(),
        has_backedge,
        scalar_leaf_callable,
        string_literals,
        precise_resume_safe,
    );
    native.cache.insert(callee_key, Some(entry));
    stack.remove(&callee_key);
    Some(NativeCompiledCallee {
        id,
        ret_ty: ret,
        param_tys: params,
    })
}

#[cfg(feature = "native-jit")]
fn native_compiled_call_sites(
    native: &mut NativeState,
    unit: &RegUnit,
    func: &RegFunction,
    self_key: usize,
) -> std::collections::HashMap<usize, NativeCompiledCallee> {
    let mut stack = std::collections::HashSet::new();
    stack.insert(self_key);
    native_compiled_call_sites_inner(native, unit, func, self_key, &mut stack)
}

#[cfg(feature = "native-jit")]
fn native_compiled_call_sites_inner(
    native: &mut NativeState,
    unit: &RegUnit,
    func: &RegFunction,
    self_key: usize,
    stack: &mut std::collections::HashSet<usize>,
) -> std::collections::HashMap<usize, NativeCompiledCallee> {
    let mut out = std::collections::HashMap::new();
    for (ip, instr) in func.code.iter().enumerate() {
        let RegInstr::CallKnown {
            function,
            args,
            mut_args,
            ..
        } = instr
        else {
            continue;
        };
        let Some(callee) = unit.functions.get(*function) else {
            continue;
        };
        let callee_key = callee.as_ref() as *const RegFunction as usize;
        if callee_key == self_key {
            continue;
        }
        let Some(descriptor) =
            native_compile_direct_scalar_callee(native, unit, callee, callee_key, stack)
        else {
            continue;
        };
        if args.len() != descriptor.param_tys.len() {
            continue;
        }
        if !native_call_mut_args_supported(mut_args, &descriptor.param_tys) {
            continue;
        }
        out.insert(ip, descriptor);
    }
    out
}

#[cfg(feature = "native-jit")]
struct NativeCallScratch {
    args: Vec<i64>,
    lens: Vec<i64>,
    flat_owned: Vec<Rc<RefCell<TypedVec>>>,
    flat_mut_owned: Vec<Rc<RefCell<TypedVec>>>,
    heap_input_slots: Vec<(usize, usize)>,
}

#[cfg(feature = "native-jit")]
fn take_native_call_scratch(native: &mut NativeState, n_params: usize) -> NativeCallScratch {
    let mut args = std::mem::take(&mut native.scratch_args);
    let mut lens = std::mem::take(&mut native.scratch_lens);
    let mut flat_owned = std::mem::take(&mut native.scratch_flat_owned);
    let mut flat_mut_owned = std::mem::take(&mut native.scratch_flat_mut_owned);
    let mut heap_input_slots = std::mem::take(&mut native.scratch_heap_input_slots);

    args.clear();
    args.resize(n_params, 0i64);
    lens.clear();
    lens.resize(n_params, 0i64);
    flat_owned.clear();
    flat_mut_owned.clear();
    heap_input_slots.clear();

    NativeCallScratch {
        args,
        lens,
        flat_owned,
        flat_mut_owned,
        heap_input_slots,
    }
}

#[cfg(feature = "native-jit")]
impl NativeCallScratch {
    fn restore(mut self, native: Option<&mut NativeState>) {
        let Some(native) = native else {
            return;
        };
        self.args.clear();
        self.lens.clear();
        self.flat_owned.clear();
        self.flat_mut_owned.clear();
        self.heap_input_slots.clear();
        native.scratch_args = self.args;
        native.scratch_lens = self.lens;
        native.scratch_flat_owned = self.flat_owned;
        native.scratch_flat_mut_owned = self.flat_mut_owned;
        native.scratch_heap_input_slots = self.heap_input_slots;
    }
}

#[cfg(feature = "native-jit")]
struct OsrNativeCallScratch {
    window: Vec<i64>,
    lens: Vec<i64>,
    flat_owned: Vec<Rc<RefCell<TypedVec>>>,
    flat_mut_owned: Vec<Rc<RefCell<TypedVec>>>,
    flat_slots: Vec<(usize, NativeTy)>,
    flat_mut_slots: Vec<(usize, usize)>,
    heap_input_slots: Vec<(usize, usize)>,
}

#[cfg(feature = "native-jit")]
fn take_osr_native_call_scratch(
    native: &mut NativeState,
    n_jit_regs: usize,
) -> OsrNativeCallScratch {
    let mut window = std::mem::take(&mut native.scratch_osr_window);
    let mut lens = std::mem::take(&mut native.scratch_osr_lens);
    let mut flat_owned = std::mem::take(&mut native.scratch_osr_flat_owned);
    let mut flat_mut_owned = std::mem::take(&mut native.scratch_osr_flat_mut_owned);
    let mut flat_slots = std::mem::take(&mut native.scratch_osr_flat_slots);
    let mut flat_mut_slots = std::mem::take(&mut native.scratch_osr_flat_mut_slots);
    let mut heap_input_slots = std::mem::take(&mut native.scratch_osr_heap_input_slots);

    window.clear();
    window.resize(n_jit_regs, 0i64);
    lens.clear();
    lens.resize(n_jit_regs, 0i64);
    flat_owned.clear();
    flat_mut_owned.clear();
    flat_slots.clear();
    flat_mut_slots.clear();
    heap_input_slots.clear();

    OsrNativeCallScratch {
        window,
        lens,
        flat_owned,
        flat_mut_owned,
        flat_slots,
        flat_mut_slots,
        heap_input_slots,
    }
}

#[cfg(feature = "native-jit")]
impl OsrNativeCallScratch {
    fn restore(mut self, native: Option<&mut NativeState>) {
        let Some(native) = native else {
            return;
        };
        self.window.clear();
        self.lens.clear();
        self.flat_owned.clear();
        self.flat_mut_owned.clear();
        self.flat_slots.clear();
        self.flat_mut_slots.clear();
        self.heap_input_slots.clear();
        native.scratch_osr_window = self.window;
        native.scratch_osr_lens = self.lens;
        native.scratch_osr_flat_owned = self.flat_owned;
        native.scratch_osr_flat_mut_owned = self.flat_mut_owned;
        native.scratch_osr_flat_slots = self.flat_slots;
        native.scratch_osr_flat_mut_slots = self.flat_mut_slots;
        native.scratch_osr_heap_input_slots = self.heap_input_slots;
    }
}

#[cfg(feature = "native-jit")]
fn osr_loop_region_is_transform_candidate(unit: &RegUnit, func: &RegFunction, lp: OsrLoop) -> bool {
    let has_elidable_full_list_slice = native_region_has_readonly_full_list_slice_elision(
        &func.code, func.regs, lp.header, lp.exit,
    );
    let mut direct_call_results: Vec<usize> = Vec::new();
    let mut direct_await_results: Vec<usize> = Vec::new();
    if let Some(region) = func.code.get(lp.header..lp.exit) {
        for instr in region {
            match instr {
                RegInstr::CallKnown { dst, .. } => direct_call_results.push(*dst),
                RegInstr::SpawnTask {
                    dst,
                    function,
                    args,
                } if unit.functions.get(*function).is_some_and(|callee| {
                    native_callee_inlinable_j3_with_spawns(unit, callee, args.len())
                }) =>
                {
                    direct_call_results.push(*dst);
                }
                RegInstr::AwaitJoin { dst, src } if direct_call_results.contains(src) => {
                    direct_await_results.push(*dst);
                }
                _ => {}
            }
        }
    }
    let checked_payload_rewrite_ips =
        native_checked_payload_rewrite_ips_in_region(&func.code, func.regs, lp.header, lp.exit)
            .unwrap_or_else(|| vec![false; func.code.len()]);
    func.code.get(lp.header..lp.exit).is_some_and(|region| {
        let region_defs = native_osr_region_defined_regs(region);
        region.iter().enumerate().all(|(offset, instr)| {
            let ip = lp.header + offset;
            // A `ListPush` that grows a flat-param pinned buffer is vetoed; a region-local
            // or non-parameter (handle-accessed) list grows freely. A rejected push falls
            // through to the match below (`_ => false`) and vetoes the loop.
            if native_subset_instruction(instr)
                && native_osr_growth_admissible(instr, &region_defs, func.params)
            {
                return true;
            }
            if checked_payload_rewrite_ips
                .get(ip)
                .copied()
                .unwrap_or(false)
            {
                return true;
            }
            if has_elidable_full_list_slice
                && matches!(
                    instr,
                    RegInstr::CallIntrinsic {
                        intrinsic: RegIntrinsic::ListSlice,
                        ..
                    } | RegInstr::CallTypedIntrinsic {
                        intrinsic: RegIntrinsic::ListSlice,
                        ..
                    }
                )
            {
                return true;
            }
            match instr {
                RegInstr::CallKnown {
                    function,
                    args,
                    mut_args: _,
                    ..
                } => unit.functions.get(*function).is_some_and(|callee| {
                    native_callee_inlinable_j3_with_spawns(unit, callee, args.len())
                }),
                RegInstr::SpawnTask { function, args, .. } => {
                    unit.functions.get(*function).is_some_and(|callee| {
                        native_callee_inlinable_j3_with_spawns(unit, callee, args.len())
                    })
                }
                RegInstr::CallClosure {
                    closure, mut_args, ..
                } => {
                    mut_args.is_empty()
                        && native_readable_or_sinkable_closure_operand_candidate(func, *closure)
                }
                RegInstr::MakeClosure { .. } => true,
                RegInstr::AwaitJoin { src, .. } => direct_call_results.contains(src),
                RegInstr::TryResult { src, .. } => {
                    direct_await_results.contains(src) || direct_call_results.contains(src)
                }
                _ if is_option_op(instr) => true,
                _ if is_variant_op(instr) => true,
                RegInstr::MatchResult { .. } => true,
                _ if is_make_struct_op(instr) => true,
                // Multi-field variant arm destructuring (`Rect { w, h } => ...`)
                // lowers to `MatchVariant` + `GetField` reads of the matched
                // variant; `native_scalar_replace_variants_in_region` dissolves a
                // `GetField` on a VAR base. Admit it so the loop can ARM — the
                // precise VAR-base verdict stays in the scalar-replacement pass.
                RegInstr::GetField { .. } => true,
                // Option/Result combinator intrinsics (`map`/`and_then`/`unwrap_or`):
                // `try_osr` expands these in-region via
                // `native_expand_option_result_combinators_in_region` before inline +
                // scalar replacement, so they are transformable candidates.
                RegInstr::CallIntrinsic { intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, .. }
                    if combinator_intrinsic_kind(*intrinsic).is_some() =>
                {
                    true
                }
                _ => false,
            }
        })
    })
}

#[cfg(feature = "native-jit")]
fn osr_loop_candidate_score(code: &[RegInstr], lp: OsrLoop) -> (u8, u8, u8, usize, usize) {
    let Some(region) = code.get(lp.header..lp.exit) else {
        return (0, 0, 0, 0, lp.header);
    };
    let has_closure_call = region
        .iter()
        .any(|instr| matches!(instr, RegInstr::CallClosure { .. }));
    let has_heap_write = region
        .iter()
        .any(|instr| native_instruction_has_heap_write(instr));
    let has_list_read = region
        .iter()
        .any(|instr| matches!(instr, RegInstr::ListGet { .. } | RegInstr::ListLen { .. }));
    (
        has_closure_call as u8,
        (!has_heap_write) as u8,
        has_list_read as u8,
        region.len(),
        lp.header,
    )
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn select_osr_candidate_loop(
    unit: &RegUnit,
    func: &RegFunction,
) -> Option<OsrLoop> {
    detect_natural_loops(&func.code)
        .into_iter()
        .filter(|lp| osr_loop_region_is_transform_candidate(unit, func, *lp))
        .max_by_key(|lp| osr_loop_candidate_score(&func.code, *lp))
}

impl RegVm {
    #[cfg(feature = "native-jit")]
    fn native_deopt_resume_ip(
        &self,
        function: vm_jit::CompiledId,
        safepoint_id: vm_jit::SafepointId,
    ) -> Option<usize> {
        self.native
            .as_ref()
            .and_then(|native| native.module.deopt_map(function))
            .and_then(|map| map.sites.get(safepoint_id.0 as usize - 1))
            .map(|site| site.resume_ip as usize)
    }

    #[cfg(feature = "native-jit")]
    fn native_compiled_id_for_func(&self, func: &RegFunction) -> Option<vm_jit::CompiledId> {
        let key = func as *const RegFunction as usize;
        self.native
            .as_ref()
            .and_then(|native| native.cache.get(&key))
            .and_then(|entry| entry.as_ref())
            .map(|entry| entry.0)
    }

    #[cfg(feature = "native-jit")]
    fn restore_native_deopt_live_regs(
        &mut self,
        base: usize,
        n_regs: usize,
        live: &[vm_jit::DeoptReg],
    ) {
        for vm_jit::DeoptReg { reg, value } in live {
            // Heap-aware deopt (J0.1): every entry in `live` is a TRUE scalar —
            // `decode_deopt_live` drops `Handle`/`FlatInt`/`FlatFloat` regs — so a
            // reassigned SCALAR param is safe to restore (no `< n_params` guard).
            // Heap / flat-pointer params never reach here, so the interpreter frame
            // keeps its own heap value.
            if (*reg as usize) >= n_regs {
                continue;
            }
            let vm_value = match value {
                vm_jit::DeoptValue::Int(i) => VmValue::Int(*i),
                vm_jit::DeoptValue::Float(f) => VmValue::Float(*f),
            };
            self.set_reg(base + *reg as usize, vm_value);
        }
    }

    #[cfg(feature = "native-jit")]
    fn push_native_child_deopt_frame(
        &mut self,
        unit: &RegUnit,
        caller: &RegFunction,
        caller_base: usize,
        call_ip: usize,
        child: &vm_jit::DeoptFrame,
    ) -> bool {
        let Some(RegInstr::CallKnown {
            dst,
            function,
            args,
            mut_args,
        }) = caller.code.get(call_ip)
        else {
            return false;
        };
        if !mut_args.is_empty() {
            return false;
        }
        let Some(callee) = unit.functions.get(*function).cloned() else {
            return false;
        };
        if self.native_compiled_id_for_func(&callee) != Some(child.function) {
            return false;
        }
        let Some(child_resume_ip) = self.native_deopt_resume_ip(child.function, child.safepoint_id)
        else {
            return false;
        };
        let child_base = caller_base + caller.regs;
        if self.prepare_frame(child_base, callee.regs).is_err() {
            return false;
        }
        for (index, reg) in args.iter().enumerate() {
            let value = self.reg(caller_base + *reg).clone();
            self.set_reg(child_base + index, value);
        }
        self.restore_native_deopt_live_regs(child_base, callee.regs, &child.live);

        let Some(caller_frame) = self.frames.last_mut() else {
            return false;
        };
        caller_frame.ip = call_ip + 1;
        self.push_frame(Frame {
            func: Rc::clone(&callee),
            ip: child_resume_ip,
            base: child_base,
            ret_dst: caller_base + *dst,
            mut_writeback: Vec::new(),
        })
        .is_ok()
            && child.child.as_deref().is_none_or(|grandchild| {
                self.push_native_child_deopt_frame(
                    unit,
                    &callee,
                    child_base,
                    child_resume_ip,
                    grandchild,
                )
            })
    }

    #[cfg(feature = "native-jit")]
    fn try_resume_native_child_deopt_chain(
        &mut self,
        unit: &RegUnit,
        func: &RegFunction,
        base: usize,
        resume_ip: usize,
        live: &[vm_jit::DeoptReg],
        child: &vm_jit::DeoptFrame,
    ) -> bool {
        let original_len = self.frames.len();
        let original_ip = self.frames.last().map(|frame| frame.ip).unwrap_or_default();
        self.restore_native_deopt_live_regs(base, func.regs, live);
        let resumed = self.push_native_child_deopt_frame(unit, func, base, resume_ip, child);
        if !resumed {
            self.frames.truncate(original_len);
            if let Some(frame) = self.frames.last_mut() {
                frame.ip = original_ip;
            }
        }
        resumed
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
    pub(super) fn try_native(&mut self, func: &RegFunction, base: usize) -> NativeAttempt {
        // Native limit parity (execution spec §6.2, Model A): Cranelift code polls
        // neither the step budget nor the cancel flag, so a hot, tiered-up function
        // containing an unbounded loop would run natively and bypass `step_budget`
        // / `cancel`. When either preemption limit is armed we refuse to dispatch
        // natively and fall back to the tier-0 executor (and interpreter), which
        // `tick()` on every instruction. `mem_budget` is ALSO in this gate now that
        // the native subset can mutate/allocate heap (§7.1 rule 9 write helpers): an
        // allocating native attempt runs off the meter and would not trip
        // `mem_budget`, so a function could grow the accounted live-set past the
        // limit without erroring. Refusing native while `mem_budget` is armed keeps
        // the limit exact (Model A; matches the tier-0 self-recursive gate).
        if !self.native_limits_unarmed() {
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
        let (id, ret_type, param_types, string_literals, precise_resume_safe) = {
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
                    let compiled_call_sites =
                        native_compiled_call_sites(native, &unit, func, native_key);
                    let translated = if compiled_call_sites.is_empty() {
                        translate_to_native_jit(&unit, func)
                    } else {
                        translate_to_native_jit_with_compiled_callees(
                            &unit,
                            func,
                            &compiled_call_sites,
                        )
                        .or_else(|| translate_to_native_jit(&unit, func))
                    };
                    let entry = match translated {
                        Some((jit_fn, ret, params, string_literals, precise_resume_safe)) => {
                            if native.collect_stats {
                                native.stats.translated += 1;
                            }
                            let scalar_leaf_callable = jit_function_scalar_leaf_callable(&jit_fn);
                            let started = native.collect_stats.then(std::time::Instant::now);
                            let compiled = if native.force_all_safepoints {
                                native.module.compile_forcing_all_bails(&jit_fn)
                            } else {
                                match native.forced_safepoint {
                                    Some(site) => native.module.compile_forcing_bail(&jit_fn, site),
                                    None => native.module.compile(&jit_fn),
                                }
                            };
                            if let Some(started) = started {
                                native.stats.compile_nanos += started.elapsed().as_nanos();
                            }
                            match compiled {
                                Ok(id) => {
                                    let verify_native =
                                        cfg!(debug_assertions) || jit_native_verify_is_strict();
                                    if verify_native {
                                        if let Err(err) = jit_verify_compiled_native(
                                            &native.module,
                                            id,
                                            &jit_fn,
                                            native.forced_safepoint,
                                        ) {
                                            debug_assert!(false, "native verifier failed: {err}");
                                            if jit_native_verify_is_strict() {
                                                if native.collect_stats {
                                                    native.stats.compile_failed += 1;
                                                }
                                                return NativeAttempt::Fallback;
                                            }
                                        }
                                    }
                                    record_native_compile_stats(native, id, &jit_fn);
                                    // Static profitability signal: a body with an
                                    // internal back-edge (a loop) does O(n) work per
                                    // dispatch and amortizes FFI cost; a loop-free body
                                    // does O(1) and, if dispatched per loop iteration,
                                    // never does. Drives `NATIVE_NOAMORTIZE_GIVEUP`.
                                    let has_backedge = jit_function_has_loop(&func.code);
                                    Some((
                                        id,
                                        ret,
                                        params,
                                        has_backedge,
                                        scalar_leaf_callable,
                                        string_literals,
                                        precise_resume_safe,
                                    ))
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
            let (
                id,
                ret,
                params,
                has_backedge,
                _scalar_leaf_callable,
                string_literals,
                precise_resume_safe,
            ) = match entry {
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
            (id, ret, params, string_literals, precise_resume_safe)
        };
        // Phase 2: marshal each argument to 64 bits per its inferred parameter
        // type. Scalars unbox directly; a `Handle` (struct/list) is registered in
        // the per-call heap table and passed as its index, for the host helpers to
        // read. (`NativeModule::call` resets its own bail flag.) A drop guard clears
        // the (possibly large) heap table on every exit path so cloned args aren't
        // retained after the call.
        // Heap-result transaction: native heap-producing helpers publish into a
        // per-call scratch table. The values are committed only after a clean native
        // completion; every fallback/deopt path aborts, so speculative heap writes
        // stay invisible to the interpreter re-run.
        let mut heap_tx = JitNativeCallFrame::begin();
        // `args[i]` and `lens[i]` are parallel per-param words (TV2 ABI). A scalar
        // unboxes into `args[i]` (with `lens[i] = 0`); a `Handle` is a heap-table
        // index; a `FlatInt`/`FlatFloat` puts the raw buffer pointer in `args[i]`
        // and the element count in `lens[i]`.
        let n = param_types.len();
        // Reuse the pooled scratch buffers instead of allocating three `Vec`s on
        // every native call: a tiny leaf/closure dispatched once per loop iteration
        // would otherwise pay that per-call allocation churn (the actual cause of
        // marginal closure/leaf kernels running slower than the interpreter). The
        // buffers are taken from `self.native` and returned to it on every exit path
        // through the shared call-scratch helpers, so they stay warm across calls.
        let mut scratch = match self.native.as_mut() {
            Some(native) => take_native_call_scratch(native, n),
            None => return NativeAttempt::Fallback,
        };
        // TV2 borrow protocol: owned `Rc`s of every flat list arg, kept alive for
        // the whole call; we then pin a shared `Ref` borrow of each so no
        // `borrow_mut`/realloc can occur while native code holds the raw pointer.
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
                NativeTy::Handle => {
                    let input = heap_tx.push_heap_arg(value.clone());
                    scratch.heap_input_slots.push((input, base + index));
                    Some(input as i64)
                }
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
                            scratch.flat_owned.push(Rc::clone(list));
                            // Placeholder; ptr+len filled in the pin pass once all
                            // borrows are held simultaneously.
                            Some(0)
                        } else {
                            None
                        }
                    }
                    _ => None,
                },
                NativeTy::FlatIntMut => match value {
                    VmValue::List(list) => {
                        let ok = list.borrow().as_ints_slice().is_some();
                        if ok {
                            if !jit_snapshot_list_before_write(index as i64, list) {
                                None
                            } else {
                                scratch.flat_mut_owned.push(Rc::clone(list));
                                Some(0)
                            }
                        } else {
                            None
                        }
                    }
                    _ => None,
                },
                NativeTy::FlatFloatMut => match value {
                    VmValue::List(list) => {
                        let ok = list.borrow().as_floats_slice().is_some();
                        if ok {
                            if !jit_snapshot_list_before_write(index as i64, list) {
                                None
                            } else {
                                scratch.flat_mut_owned.push(Rc::clone(list));
                                Some(0)
                            }
                        } else {
                            None
                        }
                    }
                    _ => None,
                },
            };
            match bits {
                Some(bits) => scratch.args[index] = bits,
                None => {
                    bail_marshal(self);
                    scratch.restore(self.native.as_mut());
                    return NativeAttempt::Fallback;
                }
            }
        }
        let flat_alias = scratch.flat_mut_owned.iter().enumerate().any(|(i, lhs)| {
            scratch
                .flat_mut_owned
                .iter()
                .skip(i + 1)
                .any(|rhs| Rc::ptr_eq(lhs, rhs))
                || scratch.flat_owned.iter().any(|rhs| Rc::ptr_eq(lhs, rhs))
        }) || jit_heap_inputs_alias_flat_mut(
            &scratch.heap_input_slots,
            &scratch.flat_mut_owned,
        );
        if flat_alias {
            bail_marshal(self);
            scratch.restore(self.native.as_mut());
            return NativeAttempt::Fallback;
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
            scratch.flat_owned.iter().map(|rc| rc.borrow()).collect();
        let mut flat_mut_guards: Vec<std::cell::RefMut<'_, TypedVec>> = scratch
            .flat_mut_owned
            .iter()
            .map(|rc| rc.borrow_mut())
            .collect();
        {
            let mut next = 0usize;
            let mut next_mut = 0usize;
            for index in 0..n {
                match param_types[index] {
                    NativeTy::FlatInt => {
                        let (ptr, len) = flat_guards[next].as_ints_slice().expect("Ints pinned");
                        next += 1;
                        scratch.args[index] = ptr as i64;
                        scratch.lens[index] = len as i64;
                    }
                    NativeTy::FlatIntMut => {
                        let slice = flat_mut_guards[next_mut]
                            .as_ints_mut_slice()
                            .expect("Ints mut pinned");
                        next_mut += 1;
                        scratch.args[index] = slice.as_mut_ptr() as i64;
                        scratch.lens[index] = slice.len() as i64;
                    }
                    NativeTy::FlatFloat => {
                        let (ptr, len) =
                            flat_guards[next].as_floats_slice().expect("Floats pinned");
                        next += 1;
                        scratch.args[index] = ptr as i64;
                        scratch.lens[index] = len as i64;
                    }
                    NativeTy::FlatFloatMut => {
                        let slice = flat_mut_guards[next_mut]
                            .as_floats_mut_slice()
                            .expect("Floats mut pinned");
                        next_mut += 1;
                        scratch.args[index] = slice.as_mut_ptr() as i64;
                        scratch.lens[index] = slice.len() as i64;
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
                heap_tx.abort();
                drop(flat_guards);
                drop(flat_mut_guards);
                scratch.restore(self.native.as_mut());
                return NativeAttempt::Fallback;
            };
            let collect_stats = native_ref.collect_stats;
            let started = collect_stats.then(std::time::Instant::now);
            let _literal_guard = jit_install_string_literals(&string_literals);
            let result = native_ref.module.call_with_host_ctx(
                id,
                &scratch.args,
                &scratch.lens,
                heap_tx.host_ctx(),
            );
            let elapsed = started.map(|started| started.elapsed().as_nanos());
            (result, elapsed)
        };
        drop(flat_guards);
        drop(flat_mut_guards);
        // The pooled scratch buffers stay available through result handling because
        // heap writeback commit still needs `heap_input_slots`; each exit arm returns
        // them through the scratch object's restore method.
        if let Some(elapsed) = elapsed
            && let Some(native) = self.native.as_mut()
        {
            native.stats.run_nanos += elapsed;
        }
        match result {
            vm_jit::NativeOutcome::Completed(bits) => {
                let Some(writebacks) =
                    heap_tx.commit_scalar_with_writebacks(&scratch.heap_input_slots)
                else {
                    heap_tx.abort();
                    if let Some(native) = self.native.as_mut() {
                        native.record_bail(native_key, func);
                    }
                    scratch.restore(self.native.as_mut());
                    return NativeAttempt::Fallback;
                };
                for (slot, value) in writebacks {
                    self.set_reg(slot, value);
                }
                if let Some(native) = self.native.as_mut() {
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
                }
                debug_assert_ne!(
                    ret_type,
                    NativeTy::Handle,
                    "a Handle-returning native function must report CompletedHandle, not Completed",
                );
                let result = NativeAttempt::Completed(match ret_type {
                    NativeTy::Float => VmValue::Float(f64::from_bits(bits as u64)),
                    NativeTy::Bool => VmValue::Bool(bits != 0),
                    _ => VmValue::Int(bits),
                });
                scratch.restore(self.native.as_mut());
                result
            }
            // Heap-result return ABI: the native call completed cleanly (vm-jit
            // reports this variant ONLY when the bail flag is clear) and its result
            // is a heap value at handle `bits`. New native allocations publish into
            // the call context's heap-result table. The older pass-through slice
            // returned an input-table handle, so keep that fallback and materialize it
            // from the input table. The transaction commits only here; on any bail
            // vm-jit returns `Deopt` and the transaction aborts before speculative
            // heap results can be observed.
            vm_jit::NativeOutcome::CompletedHandle(bits) => {
                let materialized =
                    heap_tx.commit_handle_with_writebacks(bits, &scratch.heap_input_slots);
                match materialized {
                    Some((value, writebacks)) => {
                        for (slot, value) in writebacks {
                            self.set_reg(slot, value);
                        }
                        if let Some(native) = self.native.as_mut() {
                            if native.collect_stats {
                                native.stats.native_calls += 1;
                            }
                            if native.report {
                                native.report_native_ok.insert(native_key);
                            }
                            native.bail_counts.insert(native_key, 0);
                        }
                        let result = NativeAttempt::Completed(value);
                        scratch.restore(self.native.as_mut());
                        result
                    }
                    None => {
                        // Treat an unresolvable handle exactly like a bail: re-run on
                        // the interpreter (always correct, no effect leaked).
                        heap_tx.abort();
                        if let Some(native) = self.native.as_mut() {
                            native.record_bail(native_key, func);
                        }
                        scratch.restore(self.native.as_mut());
                        NativeAttempt::Fallback
                    }
                }
            }
            vm_jit::NativeOutcome::Deopt {
                safepoint_id,
                live,
                child,
            } => {
                let child_deopt = child.is_some();
                let can_precise_deopt_resume = heap_tx.can_precise_deopt_resume();
                heap_tx.abort();
                // Bail bookkeeping is identical on both paths (precise or not):
                // a bail is still a bail for the give-up/demotion heuristic.
                let precise_deopt = if let Some(native) = self.native.as_mut() {
                    if native.collect_stats {
                        native.stats.native_bails += 1;
                        if child_deopt {
                            native.stats.native_child_bails += 1;
                        }
                    }
                    native.record_bail(native_key, func);
                    native.precise_deopt
                } else {
                    false
                };
                // J0.2 precise resume: take it ONLY when the flag is on AND this is
                // a real, mapped safepoint (id ≥ 1 with a recorded site). Anything
                // else (flag off, anonymous/early bail, or a missing site) falls
                // back to the safe re-run-from-top default.
                if precise_deopt
                    && precise_resume_safe
                    && can_precise_deopt_resume
                    && safepoint_id.0 >= 1
                {
                    // Re-borrow `native` immutably to look up the site; clone the
                    // `resume_ip` out so the borrow ends before we touch `self`.
                    let resume_ip = self
                        .native
                        .as_ref()
                        .and_then(|n| n.module.deopt_map(id))
                        .and_then(|m| m.sites.get(safepoint_id.0 as usize - 1))
                        .map(|site| site.resume_ip);
                    if let Some(resume_ip) = resume_ip {
                        if let Some(child) = child.as_deref() {
                            if self.try_resume_native_child_deopt_chain(
                                &unit,
                                func,
                                base,
                                resume_ip as usize,
                                &live,
                                child,
                            ) {
                                if let Some(native) = self.native.as_mut()
                                    && native.collect_stats
                                {
                                    native.stats.native_child_resumes += 1;
                                }
                                scratch.restore(self.native.as_mut());
                                return NativeAttempt::Resumed;
                            }
                            scratch.restore(self.native.as_mut());
                            return NativeAttempt::Fallback;
                        }
                        // Restore the live register window from the captured values,
                        // SKIPPING parameter registers: their window slots
                        // `base..base+n_params` are already valid and may hold heap
                        // `VmValue`s the scalar deopt payload cannot represent.
                        self.restore_native_deopt_live_regs(base, func.regs, &live);
                        // Resume interpretation AT the bailing instruction.
                        self.frames.last_mut().expect("active frame").ip = resume_ip as usize;
                        scratch.restore(self.native.as_mut());
                        return NativeAttempt::Resumed;
                    }
                }
                scratch.restore(self.native.as_mut());
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
    pub(super) fn resolve_osr_candidate(&self, func: &RegFunction) -> Option<usize> {
        match func.osr_state.get() {
            OsrTrigger::Unknown => {
                // J0.5: no limit blocks candidacy here anymore. `step_budget`/`cancel`
                // are enforced inside the armed OSR variant; `mem_budget` is decided in
                // `try_osr` AFTER translation (a non-allocating loop runs, an allocating
                // one declines), since candidacy alone cannot see the heap effects.
                // Cheap candidacy pre-check. Use the unified scorer even when the
                // function has an apparently "single" loop: helper-backed mutation
                // loops, read loops, and profile-guided closure-inline loops have
                // different payoff, and the legacy first-loop shortcut can arm a
                // setup loop before the real hot transformable loop is considered.
                let candidate = select_osr_candidate_loop(&self.unit, func);
                let state = match candidate {
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
    pub(super) fn try_osr(&mut self, func: &RegFunction, base: usize, header_ip: usize) -> bool {
        // Memory parity: an allocating OSR loop runs off the `mem_budget` meter (§7.1
        // rule 9), and native allocations are not yet charged (J0.5 mem accounting is
        // future / tied to S4), so refuse OSR while `mem_budget` is armed. `step_budget`
        // and `cancel` are NOT refused anymore: J0.5 emits the per-instruction step
        // accumulator + per-header `step_budget` test + `cancel` poll directly in the
        // armed OSR variant (Exec-Spec §6.2, "enforce or be ineligible" — the *enforce*
        // branch), bailing to the interpreter which remains the sole limit authority.
        //
        // `mem_budget` is no longer a blanket refusal either: a loop that allocates /
        // mutates nothing charges `mem_budget` exactly zero (identical to the
        // interpreter), so it runs natively; an ALLOCATING loop still declines after
        // translation (in-code byte accounting is future) — see `mem_armed` below.
        let emit_step = self.limits.step_budget.is_some();
        let emit_cancel = self.limits.cancel.is_some();
        let mem_armed = self.limits.mem_budget.is_some();
        if let Some(native) = self.native.as_mut() {
            native.osr_dynamic_bail = false;
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
        let (
            id,
            trans_exit,
            orig_exit,
            n_jit_regs,
            _param_types,
            derived_liveins,
            scalar_fields,
            heap_input_regs,
            reg_types,
            written_regs,
            string_literals,
            variant_reconstructs,
            some_option_reconstructs,
        ) = {
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
                let direct_entry = detect_natural_loop_at(&func.code, header_ip)
                    .filter(|lp| osr_loop_region_is_native_subset(&func.code, *lp, func.params))
                    .filter(|lp| {
                        !osr_loop_region_needs_optimized_native_subset_path(&func.code, *lp)
                    })
                    .and_then(|lp| {
                        let identity_ip_map: Vec<usize> = (0..func.code.len()).collect();
                        translate_osr_loop_profiled(
                            func,
                            &func.code,
                            func.regs,
                            func.params,
                            func.captures,
                            lp,
                            &identity_ip_map,
                        )
                        .and_then(
                            |(
                                jit_fn,
                                params,
                                derived_liveins,
                                scalar_fields,
                                reg_types,
                                written_regs,
                                string_literals,
                            )| {
                                let n_jit_regs = jit_fn.n_regs as usize;
                                // J0.5 mem: decline only a loop that charges `mem_budget`
                                // in a way native does not yet account (a `ListPush*`
                                // growth); every other allocation is uncharged by the
                                // interpreter too, so running it natively is exact parity.
                                if mem_armed && jit_fn_has_unaccounted_mem_charge(&jit_fn) {
                                    return None;
                                }
                                let heap_input_regs = osr_heap_input_regs(&jit_fn);
                                match native.module.compile_osr(&jit_fn, lp.header as u32, emit_step, emit_cancel) {
                                    Ok(id) => {
                                        let verify_native =
                                            cfg!(debug_assertions) || jit_native_verify_is_strict();
                                        if verify_native {
                                            if let Err(err) = jit_verify_compiled_osr(
                                                &native.module,
                                                id,
                                                &jit_fn,
                                                lp.exit,
                                            ) {
                                                debug_assert!(
                                                    false,
                                                    "native OSR verifier failed: {err}"
                                                );
                                                if jit_native_verify_is_strict() {
                                                    return None;
                                                }
                                            }
                                        }
                                        record_native_compile_stats(native, id, &jit_fn);
                                        Some(OsrEntry {
                                            id,
                                            orig_header: lp.header,
                                            trans_exit: lp.exit,
                                            orig_exit: lp.exit,
                                            n_jit_regs,
                                            param_types: params,
                                            derived_liveins,
                                            scalar_fields,
                                            heap_input_regs,
                                            reg_types,
                                            written_regs,
                                            string_literals,
                                            // Direct path runs no RESULT-SR/OPTION-SR ⇒ no
                                            // live-after reconstruction recipes.
                                            variant_reconstructs: Vec::new(),
                                            some_option_reconstructs: Vec::new(),
                                        })
                                    }
                                    Err(_) => None,
                                }
                            },
                        )
                    });
                let entry = direct_entry.or_else(|| {
                let expanded = detect_natural_loop_at(&func.code, header_ip).and_then(|lp_pre| {
                    native_expand_option_result_combinators_in_region(
                        &unit,
                        func,
                        &func.code,
                        func.regs,
                        lp_pre.header,
                        lp_pre.exit,
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
                            jit_self_recursion_kind: std::cell::Cell::new(None),
                            native_status: std::cell::Cell::new(0),
                            call_count: std::cell::Cell::new(0),
                            branch_count: std::cell::Cell::new(0),
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
                mapped_osr_loop(&eff_func.code, &expand_map, header_ip).and_then(|lp_orig| {
                native_inline_leaf_calls(&unit, eff_func, true, Some((lp_orig.header, lp_orig.exit))).and_then(
                    |(inlined_code, n_regs0, ip_map0)| {
                    // OSR × stored-closure helper fusion: after the closure inline pass
                    // has introduced `NativeClosureId`/`NativeClosureCapture`, collapse
                    // `GetFieldSlot`-materialized closure handles that are used only for
                    // those metadata reads. This avoids a per-iteration `FieldHandle`
                    // helper call while preserving an index-stable stream.
                    let (inlined_code, n_regs0, ip_map_fc) =
                        native_fuse_field_closure_metadata_reads(&inlined_code, n_regs0)?;
                    let ip_map0: Vec<usize> = ip_map_fc
                        .iter()
                        .map(|&idx| ip_map0[idx])
                        .collect();
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
                    mapped_osr_loop(&inlined_code, &ip_map0, lp_orig.header).and_then(|lp_sl| {
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
                    mapped_osr_loop(&inlined_code, &ip_map_sl, lp_sl.header).and_then(|lp_by| {
                    native_bytes_length_fold_in_region(
                        &inlined_code, n_regs0, lp_by.header, lp_by.exit,
                    )
                    .and_then(|(inlined_code, n_regs0, ip_map_by)| {
                    // OSR × J3 for LIST FULL-SLICE QUERY FOLDING: dissolve a
                    // non-escaping `List.slice(list, 0, List.len(list))` whose result
                    // is only used by read-only list queries. The materialized shallow
                    // copy is observably equivalent to a handle alias while the source
                    // list is unwritten in the region, removing an allocating intrinsic
                    // from copy/read hot loops before native-subset checking.
                    let lp_list = mapped_osr_loop(&inlined_code, &ip_map_by, lp_by.header)?;
                    let (inlined_code, n_regs0, ip_map_list) =
                        native_elide_readonly_full_list_slices_in_region(
                            &inlined_code,
                            n_regs0,
                            lp_list.header,
                            lp_list.exit,
                        )?;
                    let lp_checked = mapped_osr_loop(&inlined_code, &ip_map_list, lp_list.header)?;
                    let (inlined_code, n_regs0, _ip_map_checked) =
                        native_lower_checked_payload_intrinsics_in_region(
                            &inlined_code,
                            n_regs0,
                            lp_checked.header,
                            lp_checked.exit,
                        )?;
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
                    mapped_osr_loop(&inlined_code, &_ip_map_checked, lp_checked.header).and_then(|lp_r| {
                    native_scalar_replace_results_in_region(
                        &inlined_code, n_regs0, lp_r.header, lp_r.exit,
                    )
                    .and_then(|(code_r, n_regs_r, ip_map_r, recipes_r)| {
                        // OSR × J3 for OPTIONS: dissolve any non-escaping scalar Option
                        // living entirely inside the region. After Result-SR the region
                        // carries only Option ops + native subset, so the strict
                        // subset-or-option gate is satisfied. Identity (no Option) ⇒
                        // unchanged.
                        let lp1 = mapped_osr_loop(&code_r, &ip_map_r, lp_r.header)?;
                        let (code1, n_regs1, ip_map1, option_recipes1) =
                            native_scalar_replace_options_in_region(
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
                        let lp_v = mapped_osr_loop(&code1, &ip_map1, lp1.header)?;
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
                        let lp_s = mapped_osr_loop(&code2, &ip_map2, lp_v.header)?;
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
                        let lp_lc = mapped_osr_loop(&code_s, &ip_map3, lp_s.header)?;
                        let (code, n_regs, ip_map3b) = native_loop_carried_struct_in_region(
                            &code_s, n_regs_s, lp_lc.header, lp_lc.exit,
                        )
                        .unwrap_or_else(|| {
                            (code_s.clone(), n_regs_s, (0..code_s.len()).collect())
                        });
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
                        mapped_osr_loop(&code, &ip_map3b, lp_lc.header).and_then(|lp| {
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
                            let maps_into_inline = |trans_idx: usize, eff_idx: usize| {
                                let Some(eff_instr) = eff_func.code.get(eff_idx) else {
                                    return false;
                                };
                                let eff_is_call = matches!(
                                    eff_instr,
                                    RegInstr::CallKnown { .. } | RegInstr::CallClosure { .. }
                                );
                                if !eff_is_call {
                                    return false;
                                }
                                let copied_boundary_call = matches!(
                                    (code.get(trans_idx), eff_instr),
                                    (
                                        Some(RegInstr::CallKnown { .. }),
                                        RegInstr::CallKnown { .. }
                                    ) | (
                                        Some(RegInstr::CallClosure { .. }),
                                        RegInstr::CallClosure { .. }
                                    )
                                );
                                !copied_boundary_call
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
                            if maps_into_inline(lp.header, eff_header) {
                                return None;
                            }
                            let orig_header = to_real(eff_header)?;
                            let orig_exit = if lp.exit < ip_map.len() {
                                let oe = ip_map[lp.exit];
                                if maps_into_inline(lp.exit, oe) {
                                    return None;
                                }
                                to_real(oe)?
                            } else if lp.exit == code.len() {
                                real_code.len()
                            } else {
                                return None;
                            };
                            let mut real_ip_map = Vec::with_capacity(code.len());
                            for transformed_ip in 0..code.len() {
                                let eff_ip = *ip_map.get(transformed_ip)?;
                                if maps_into_inline(transformed_ip, eff_ip) {
                                    real_ip_map.push(usize::MAX);
                                } else {
                                    real_ip_map.push(to_real(eff_ip).unwrap_or(usize::MAX));
                                }
                            }
                            translate_osr_loop_profiled(
                                func,
                                &code,
                                n_regs,
                                eff_func.params,
                                eff_func.captures,
                                lp,
                                &real_ip_map,
                            )
                                .and_then(|(jit_fn, params, derived_liveins, scalar_fields, reg_types, written_regs, string_literals)| {
                                    let n_jit_regs = jit_fn.n_regs as usize;
                                    // J0.5 mem: decline only a `ListPush*`-charging loop
                                    // (checked on the post-dissolution body, which reflects
                                    // what actually runs); every other allocation is
                                    // uncharged by the interpreter too — exact parity.
                                    if mem_armed && jit_fn_has_unaccounted_mem_charge(&jit_fn) {
                                        return None;
                                    }
                                    let heap_input_regs = osr_heap_input_regs(&jit_fn);
                                    match native.module.compile_osr(&jit_fn, lp.header as u32, emit_step, emit_cancel) {
                                        Ok(id) => {
                                            let verify_native =
                                                cfg!(debug_assertions) || jit_native_verify_is_strict();
                                            if verify_native {
                                                if let Err(err) = jit_verify_compiled_osr(
                                                    &native.module,
                                                    id,
                                                    &jit_fn,
                                                    lp.exit,
                                                ) {
                                                    debug_assert!(false, "native OSR verifier failed: {err}");
                                                    if jit_native_verify_is_strict() {
                                                        return None;
                                                    }
                                                }
                                            }
                                            record_native_compile_stats(native, id, &jit_fn);
                                            // J0.1(b): every live-after recipe must
                                            // reconstruct from SCALAR registers — the
                                            // deopt ABI carries only Int/Float. A
                                            // non-scalar payload (struct handle, flat) or
                                            // tag cannot be rematerialized, so decline OSR.
                                            let scalar = |reg: usize| {
                                                matches!(
                                                    reg_types.get(reg),
                                                    Some(NativeTy::Int | NativeTy::Float)
                                                )
                                            };
                                            // The tag is a boolean — deopt-representable as
                                            // an Int (vm-jit has no Bool class), so allow
                                            // `Bool` for it in addition to Int/Float.
                                            let scalar_or_bool = |reg: usize| {
                                                scalar(reg)
                                                    || matches!(reg_types.get(reg), Some(NativeTy::Bool))
                                            };
                                            for (_r, payload_reg, tag_reg) in &recipes_r {
                                                if !scalar(*payload_reg) {
                                                    return None;
                                                }
                                                if let Some(tag_reg) = tag_reg {
                                                    if !scalar_or_bool(*tag_reg) {
                                                        return None;
                                                    }
                                                }
                                            }
                                            for (_r, payload_reg) in &option_recipes1 {
                                                if !scalar(*payload_reg) {
                                                    return None;
                                                }
                                            }
                                            Some(OsrEntry {
                                                id,
                                                orig_header,
                                                trans_exit: lp.exit,
                                                orig_exit,
                                                n_jit_regs,
                                                param_types: params,
                                                derived_liveins,
                                                scalar_fields,
                                                heap_input_regs,
                                                reg_types,
                                                written_regs,
                                                string_literals,
                                                variant_reconstructs: recipes_r,
                                                some_option_reconstructs: option_recipes1,
                                            })
                                        }
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
                Some(Some(e)) if e.orig_header == header_ip => (
                    e.id,
                    e.trans_exit,
                    e.orig_exit,
                    e.n_jit_regs,
                    e.param_types.clone(),
                    e.derived_liveins.clone(),
                    e.scalar_fields.clone(),
                    e.heap_input_regs.clone(),
                    e.reg_types.clone(),
                    e.written_regs.clone(),
                    e.string_literals.clone(),
                    e.variant_reconstructs.clone(),
                    e.some_option_reconstructs.clone(),
                ),
                _ => {
                    return false;
                }
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
        let mut heap_tx = JitNativeCallFrame::begin();
        let n_regs = func.regs;
        let mut scratch = match self.native.as_mut() {
            Some(native) => take_osr_native_call_scratch(native, n_jit_regs),
            None => return false,
        };
        macro_rules! bail_osr {
            () => {{
                heap_tx.abort();
                scratch.restore(self.native.as_mut());
                return false;
            }};
        }
        // TV2 flat live-in lists: each loop-invariant typed list classified
        // `FlatInt`/`FlatFloat` is marshalled as a raw (pointer, length) pair with a
        // shared borrow pinned for the whole call. A `FlatIntMut` live-in uses the
        // same ABI but pins one mutable borrow and snapshots the input list before
        // native writes, so a non-exit deopt restores the interpreter-visible list.
        // `flat_owned`/`flat_mut_owned` keep the `Rc`s alive; the slot vectors record
        // which window/lens entries the pin pass must fill.
        for reg in 0..n_regs {
            if !self.written.get(base + reg).copied().unwrap_or(false) {
                continue; // not live here; native won't read it
            }
            let value = self.reg(base + reg);
            // Classify by the full per-register native type (not only params): a flat
            // live-in list may be a non-param register on the OSR path.
            let ty = reg_types.get(reg).copied().unwrap_or(NativeTy::Int);
            if matches!(
                ty,
                NativeTy::FlatInt
                    | NativeTy::FlatFloat
                    | NativeTy::FlatIntMut
                    | NativeTy::FlatFloatMut
            ) {
                let want_int = ty == NativeTy::FlatInt;
                match value {
                    VmValue::List(list) => {
                        let ok = {
                            let borrowed = list.borrow();
                            if want_int || ty == NativeTy::FlatIntMut {
                                borrowed.as_ints_slice().is_some()
                            } else {
                                borrowed.as_floats_slice().is_some()
                            }
                        };
                        if !ok {
                            // Not the canonical flat kind ⇒ bail this OSR attempt.
                            bail_osr!();
                        }
                        if ty == NativeTy::FlatIntMut || ty == NativeTy::FlatFloatMut {
                            if !jit_snapshot_list_before_write(reg as i64, list) {
                                bail_osr!();
                            }
                            if let Some(index) = scratch
                                .flat_mut_owned
                                .iter()
                                .position(|rc| Rc::ptr_eq(rc, list))
                            {
                                scratch.flat_mut_slots.push((index, reg));
                            } else {
                                let index = scratch.flat_mut_owned.len();
                                scratch.flat_mut_owned.push(Rc::clone(list));
                                scratch.flat_mut_slots.push((index, reg));
                            }
                        } else {
                            scratch.flat_owned.push(Rc::clone(list));
                            scratch.flat_slots.push((reg, ty));
                        }
                        // window[reg]/lens[reg] filled in the pin pass below.
                        continue;
                    }
                    // A flat-classified register that isn't a List at runtime ⇒ bail.
                    _ => bail_osr!(),
                }
            }
            let bits = match (ty, value) {
                (NativeTy::Float, VmValue::Float(f)) => f.to_bits() as i64,
                (NativeTy::Float, VmValue::Int(i)) => (*i as f64).to_bits() as i64,
                (_, VmValue::Int(i)) => *i,
                (_, VmValue::Bool(b)) => i64::from(*b),
                (_, VmValue::Float(f)) => f.to_bits() as i64,
                // A handle (List/struct/etc.): pass its heap-table index.
                (_, other) => {
                    let input = heap_tx.push_heap_arg(other.clone());
                    scratch.heap_input_slots.push((input, base + reg));
                    input as i64
                }
            };
            scratch.window[reg] = bits;
        }
        for livein in &derived_liveins {
            if livein.native_reg >= n_jit_regs || livein.base_reg >= n_regs {
                bail_osr!();
            }
            if !self
                .written
                .get(base + livein.base_reg)
                .copied()
                .unwrap_or(false)
            {
                bail_osr!();
            }
            let value = self.reg(base + livein.base_reg);
            let Some(list) = jit_struct_field_list(value, livein.field_slot) else {
                bail_osr!();
            };
            let ok = {
                let borrowed = list.borrow();
                match livein.ty {
                    NativeTy::FlatInt | NativeTy::FlatIntMut => borrowed.as_ints_slice().is_some(),
                    NativeTy::FlatFloat | NativeTy::FlatFloatMut => {
                        borrowed.as_floats_slice().is_some()
                    }
                    _ => false,
                }
            };
            if !ok {
                bail_osr!();
            }
            if livein.ty == NativeTy::FlatIntMut || livein.ty == NativeTy::FlatFloatMut {
                if !jit_snapshot_input_list_before_write(&list) {
                    bail_osr!();
                }
                if let Some(index) = scratch
                    .flat_mut_owned
                    .iter()
                    .position(|rc| Rc::ptr_eq(rc, &list))
                {
                    scratch.flat_mut_slots.push((index, livein.native_reg));
                } else {
                    let index = scratch.flat_mut_owned.len();
                    scratch.flat_mut_owned.push(Rc::clone(&list));
                    scratch.flat_mut_slots.push((index, livein.native_reg));
                }
            } else {
                scratch.flat_owned.push(Rc::clone(&list));
                scratch.flat_slots.push((livein.native_reg, livein.ty));
            }
        }
        for field in &scalar_fields {
            if field.native_reg >= n_jit_regs || field.base_reg >= n_regs {
                bail_osr!();
            }
            if !self
                .written
                .get(base + field.base_reg)
                .copied()
                .unwrap_or(false)
            {
                bail_osr!();
            }
            let Some(value) =
                jit_struct_field_int(self.reg(base + field.base_reg), field.field_slot)
            else {
                bail_osr!();
            };
            scratch.window[field.native_reg] = value;
        }

        if !scratch.flat_owned.is_empty() && !scratch.flat_mut_owned.is_empty() {
            let old_flat_owned = std::mem::take(&mut scratch.flat_owned);
            let old_flat_slots = std::mem::take(&mut scratch.flat_slots);
            for (list, (reg, kind)) in old_flat_owned.into_iter().zip(old_flat_slots) {
                if kind == NativeTy::FlatInt {
                    if let Some(index) = scratch
                        .flat_mut_owned
                        .iter()
                        .position(|candidate| Rc::ptr_eq(candidate, &list))
                    {
                        scratch.flat_mut_slots.push((index, reg));
                        continue;
                    }
                }
                scratch.flat_owned.push(list);
                scratch.flat_slots.push((reg, kind));
            }
        }

        let flat_alias = scratch
            .flat_mut_owned
            .iter()
            .any(|lhs| scratch.flat_owned.iter().any(|rhs| Rc::ptr_eq(lhs, rhs)))
            || jit_selected_heap_inputs_alias_flat_mut(
                &scratch.heap_input_slots,
                &scratch.flat_mut_owned,
                base,
                &heap_input_regs,
            );
        if flat_alias {
            bail_osr!();
        }

        // SAFETY (TV2 borrow protocol — the same audited core as `try_native`): pin
        // shared borrows for read-only flat lists and mutable borrows for
        // `FlatIntMut` lists for the whole native call. Alias checks above prevent
        // simultaneous shared/mutable borrows of the same list. Direct native reads
        // and writes are bounds-checked against the matching `lens` slot; a deopt
        // before the normal OSR exit aborts `heap_tx` and restores mutable inputs.
        let flat_guards: Vec<std::cell::Ref<'_, TypedVec>> =
            scratch.flat_owned.iter().map(|rc| rc.borrow()).collect();
        let mut flat_mut_guards: Vec<std::cell::RefMut<'_, TypedVec>> = scratch
            .flat_mut_owned
            .iter()
            .map(|rc| rc.borrow_mut())
            .collect();
        for (i, &(reg, kind)) in scratch.flat_slots.iter().enumerate() {
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
            scratch.window[reg] = ptr;
            scratch.lens[reg] = len as i64;
        }
        for &(i, reg) in &scratch.flat_mut_slots {
            let guard = flat_mut_guards.get_mut(i).expect("flat mut slot index");
            // `flat_mut_slots` carries no element kind; pick the buffer by what the
            // pinned list actually is (an Int-mut or Float-mut flat list).
            let (ptr, len) = if let Some(slice) = guard.as_ints_mut_slice() {
                (slice.as_mut_ptr() as i64, slice.len() as i64)
            } else {
                let slice = guard
                    .as_floats_mut_slice()
                    .expect("flat mut list is Ints or Floats");
                (slice.as_mut_ptr() as i64, slice.len() as i64)
            };
            scratch.window[reg] = ptr;
            scratch.lens[reg] = len;
        }

        // Phase 3: run the OSR loop body natively.
        // J0.5: seed the limits cell for an armed variant. `emit_step`/`emit_cancel`
        // were fixed at the top of this call from `self.limits`; the compiled variant
        // matches (same eval-constant limits), so a non-null cell is required exactly
        // when armed. `steps` flows in here and back out below into `self.steps`.
        let armed = emit_step || emit_cancel;
        if armed {
            let step_budget = self.limits.step_budget.map_or(-1, |b| b as i64);
            let cancel_addr = self
                .limits
                .cancel
                .as_ref()
                .map_or(0, |flag| std::sync::Arc::as_ptr(flag) as i64);
            jit_set_limits_cell(self.steps as i64, step_budget, cancel_addr);
        }
        let Some(native_ref) = self.native.as_ref() else {
            heap_tx.abort();
            drop(flat_guards);
            drop(flat_mut_guards);
            scratch.restore(self.native.as_mut());
            return false;
        };
        let collect_stats = native_ref.collect_stats;
        let started = collect_stats.then(std::time::Instant::now);
        let _literal_guard = jit_install_string_literals(&string_literals);
        let result = if armed {
            native_ref.module.call_with_limits(
                id,
                &scratch.window,
                &scratch.lens,
                heap_tx.host_ctx(),
                jit_limits_cell_ptr(),
            )
        } else {
            native_ref.module.call_with_host_ctx(
                id,
                &scratch.window,
                &scratch.lens,
                heap_tx.host_ctx(),
            )
        };
        let elapsed = started.map(|started| started.elapsed().as_nanos());
        // The pinned borrows are no longer needed once the native call returns.
        drop(flat_guards);
        drop(flat_mut_guards);
        // J0.5: fold the steps native paid (clean completion OR deopt both wrote it
        // back) into the interpreter's counter, so resuming the interpreter continues
        // the single tick stream with no double-/under-count.
        if emit_step {
            self.steps = jit_limits_cell_steps() as u64;
        }
        if let Some(native) = self.native.as_mut() {
            if let Some(elapsed) = elapsed {
                native.stats.run_nanos += elapsed;
            }
        }

        // Phase 4: OSR-exit. The loop always exits via the `OsrExit` safepoint (a
        // deopt). Resume the interpreter at the post-loop ip with the restored
        // live-out window — the precise-deopt resume, reused verbatim.
        match result {
            vm_jit::NativeOutcome::Deopt {
                safepoint_id, live, ..
            } if safepoint_id.0 >= 1 => {
                let resume_ip = self
                    .native
                    .as_ref()
                    .and_then(|n| n.module.deopt_map(id))
                    .and_then(|m| m.sites.get(safepoint_id.0 as usize - 1))
                    .map(|site| site.resume_ip);
                let Some(resume_ip) = resume_ip else {
                    heap_tx.abort();
                    scratch.restore(self.native.as_mut());
                    return false;
                };
                // The OSR-exit's resume_ip (a TRANSFORMED-code ip) MUST be the loop's
                // post-loop exit ip; anything else is an OSR construction bug. Fall
                // back rather than misresume.
                if resume_ip as usize != trans_exit {
                    heap_tx.abort();
                    scratch.restore(self.native.as_mut());
                    return false;
                }
                let Some(writebacks) =
                    heap_tx.commit_scalar_with_writebacks(&scratch.heap_input_slots)
                else {
                    heap_tx.abort();
                    scratch.restore(self.native.as_mut());
                    return false;
                };
                for (slot, value) in writebacks {
                    self.set_reg(slot, value);
                }
                let mut scalar_writebacks: Vec<(usize, Vec<(usize, i64)>)> = Vec::new();
                for field in scalar_fields.iter().filter(|field| field.writeback) {
                    let Some(value) = live.iter().find_map(|live_reg| {
                        (live_reg.reg as usize == field.native_reg).then(|| {
                            match live_reg.value {
                                vm_jit::DeoptValue::Int(value) => Some(value),
                                vm_jit::DeoptValue::Float(_) => None,
                            }
                        })?
                    }) else {
                        heap_tx.abort();
                        scratch.restore(self.native.as_mut());
                        return false;
                    };
                    if let Some((_, updates)) = scalar_writebacks
                        .iter_mut()
                        .find(|(base_reg, _)| *base_reg == field.base_reg)
                    {
                        updates.push((field.field_slot, value));
                    } else {
                        scalar_writebacks.push((field.base_reg, vec![(field.field_slot, value)]));
                    }
                }
                for (base_reg, updates) in scalar_writebacks {
                    let slot = base + base_reg;
                    let Some(updated) = jit_struct_with_int_field_updates(self.reg(slot), &updates)
                    else {
                        heap_tx.abort();
                        scratch.restore(self.native.as_mut());
                        return false;
                    };
                    self.set_reg(slot, updated);
                }
                // Restore only original non-param registers that the native OSR loop
                // actually wrote. Live-through slots that were assigned before the
                // loop are already correct in the interpreter window; restoring their
                // native payload would corrupt heap/list slots whose native word is
                // only an opaque handle/pointer/zero. Also skip J3-added tag/payload
                // registers (index >= func.regs) and Handle/flat classes, whose
                // scalar deopt payload cannot represent a VM heap value.
                // J0.1(b): rebuild any live-after always-`Ok` Result from its scalar
                // payload register's live-out value into the original variant slot. The
                // RESULT-SR pass dissolved the Result to a scalar `payload_reg` and the
                // native loop never wrote the original `variant_reg`, so the interpreter
                // slot is stale (its pre-loop value). The pass guaranteed the `Ok` def is
                // reached unconditionally, so after >=1 iteration `payload_reg` is in the
                // deopt live set and we reconstruct `Ok(payload)`. If `payload_reg` is
                // ABSENT, the native loop ran 0 iterations and the pre-loop value already
                // in the slot is exactly correct, so we leave it untouched.
                // For a two-armed Result (`tag_reg` is `Some`), the live-out tag selects
                // the arm: non-zero ⇒ `Ok(payload)`, zero ⇒ `Err(payload)`. An always-`Ok`
                // Result (`tag_reg` is `None`) always reconstructs `Ok(payload)`.
                for (variant_reg, payload_reg, tag_reg) in &variant_reconstructs {
                    if let Some(dr) = live.iter().find(|d| d.reg as usize == *payload_reg) {
                        let payload = match dr.value {
                            vm_jit::DeoptValue::Int(i) => VmValue::Int(i),
                            vm_jit::DeoptValue::Float(f) => VmValue::Float(f),
                        };
                        let is_ok = match tag_reg {
                            None => true,
                            Some(tag_reg) => live
                                .iter()
                                .find(|d| d.reg as usize == *tag_reg)
                                .map(|d| matches!(d.value, vm_jit::DeoptValue::Int(i) if i != 0))
                                .unwrap_or(true),
                        };
                        let layout = if is_ok {
                            result_ok_layout()
                        } else {
                            result_err_layout()
                        };
                        let value = VmValue::Variant(std::rc::Rc::new(
                            crate::vm_value::VmStruct::with_layout(layout, vec![payload]),
                        ));
                        self.set_reg(base + *variant_reg, value);
                    }
                }
                // J0.1(b): the Option analog — rebuild `Some(payload)` for a live-after
                // always-`Some` Option from its scalar payload register. Absent payload
                // ⇒ 0 native iterations ⇒ pre-loop value already correct, leave it.
                for (opt_reg, payload_reg) in &some_option_reconstructs {
                    if let Some(dr) = live.iter().find(|d| d.reg as usize == *payload_reg) {
                        let payload = match dr.value {
                            vm_jit::DeoptValue::Int(i) => VmValue::Int(i),
                            vm_jit::DeoptValue::Float(f) => VmValue::Float(f),
                        };
                        self.set_reg(base + *opt_reg, VmValue::some(payload));
                    }
                }
                let n_params = func.params;
                let n_orig_regs = func.regs;
                for vm_jit::DeoptReg { reg, value } in live {
                    if (reg as usize) < n_params || (reg as usize) >= n_orig_regs {
                        continue;
                    }
                    if !written_regs.get(reg as usize).copied().unwrap_or(false) {
                        continue;
                    }
                    // Skip Handle (heap-table index) AND flat-array (raw buffer
                    // pointer bits) registers: neither's deopt payload word is a VM
                    // value; writing it back would corrupt the interpreter slot. A
                    // flat list is loop-invariant, so the original List already sits in
                    // its slot unchanged.
                    if matches!(
                        reg_types.get(reg as usize),
                        Some(
                            &NativeTy::Handle
                                | &NativeTy::FlatInt
                                | &NativeTy::FlatIntMut
                                | &NativeTy::FlatFloat
                                | &NativeTy::FlatFloatMut,
                        )
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
                        native
                            .report_osr_ok
                            .insert(func as *const RegFunction as usize);
                    }
                }
                scratch.restore(self.native.as_mut());
                true
            }
            // A completion (the OSR body has no `Return`) or an anonymous/early bail
            // is not a normal OSR-exit: leave the frame untouched and let the
            // interpreter run the loop. Safe and behavior-preserving.
            _ => {
                heap_tx.abort();
                if let Some(native) = self.native.as_mut() {
                    native.osr_dynamic_bail = true;
                }
                scratch.restore(self.native.as_mut());
                false
            }
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
    pub(super) fn run_jit(
        &mut self,
        unit: &RegUnit,
        func: &RegFunction,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let mut ip = 0usize;
        while let Some(instr) = func.code.get(ip) {
            self.tick()?;
            #[cfg(feature = "native-jit")]
            let instr_ip = ip;
            ip += 1;
            #[cfg(feature = "native-jit")]
            self.record_native_branch_feedback(func, instr, base, instr_ip)?;
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
                let result =
                    if mut_args.is_empty() && callee.code.iter().all(jit_supported_instruction) {
                        self.run_jit_pure_leaf(&callee, next_base)?
                    } else {
                        self.run_frame(unit, callee, next_base)?
                    };
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

    fn run_jit_pure_leaf(&mut self, func: &RegFunction, base: usize) -> Result<VmValue, EvalError> {
        let mut ip = 0usize;
        while let Some(instr) = func.code.get(ip) {
            self.tick()?;
            #[cfg(feature = "native-jit")]
            let instr_ip = ip;
            ip += 1;
            #[cfg(feature = "native-jit")]
            self.record_native_branch_feedback(func, instr, base, instr_ip)?;
            match self.try_exec_pure(instr, base, &mut ip)? {
                PureStep::Next => {}
                PureStep::Return(value) => return Ok(value),
                PureStep::NotPure => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM JIT leaf fast path reached non-pure instruction `{instr:?}`."
                    )));
                }
            }
        }
        Ok(VmValue::Unit)
    }

    /// Native self-recursion (native-call-ABI slice 3; generalized in Phase 2):
    /// compile `func` with `CallSelf` via the *general* native subset and run it
    /// natively for SCALAR args (Int/Bool/Float). Marshalling/wrapping is driven by
    /// the compiled parameter/return `NativeTy`s, so a Float (or match/heap-read-bodied)
    /// self-recursive function runs natively — not just the Int-arith whitelist.
    /// Returns the wrapped result on a clean completion, or `None` to fall back
    /// (compile failure, non-scalar param/return, deopt incl. the entry depth-cap
    /// bail that keeps deep recursion off the host stack). Compiled once and cached.
    #[cfg(feature = "native-jit")]
    fn try_native_self_recursive(
        &mut self,
        unit: &RegUnit,
        function_id: usize,
        func: &RegFunction,
        caller_base: usize,
        args: &[usize],
    ) -> Option<VmValue> {
        let key = func as *const RegFunction as usize;
        let (id, param_tys, ret) = {
            let native = self.native.as_mut()?;
            // Deopt-stress / forced-bail modes run the scalar/interpreter path so the
            // differential stress backends exercise the always-correct fallback.
            if native.force_bail || native.forced_safepoint.is_some() || native.force_all_safepoints
            {
                return None;
            }
            match native.self_recursive_native.get(&key) {
                Some(cached) => cached.clone()?,
                None => {
                    let self_call_sites: std::collections::HashSet<usize> = func
                        .code
                        .iter()
                        .enumerate()
                        .filter_map(|(ip, instr)| {
                            matches!(
                                instr,
                                RegInstr::CallKnown { function, mut_args, .. }
                                    if *function == function_id && mut_args.is_empty()
                            )
                            .then_some(ip)
                        })
                        .collect();
                    // Not self-recursive ⇒ this path doesn't apply (cheap negative
                    // cache; avoids attempting the heavy translate on ordinary calls).
                    let compiled = if self_call_sites.is_empty() {
                        None
                    } else {
                        translate_to_native_jit_with_calls(
                            unit,
                            func,
                            &std::collections::HashMap::new(),
                            &self_call_sites,
                            &std::collections::HashMap::new(),
                        )
                        .and_then(
                            |(jit_fn, ret, param_tys, _literals, _precise)| {
                                // Scalar-only ABI: params and return must be i64/f64
                                // scalars (Int/Bool/Float). Heap (Handle) params/returns
                                // route through the fallback — their cross-call
                                // marshalling/reconstruction is out of scope here.
                                let is_scalar = |t: &NativeTy| {
                                    matches!(t, NativeTy::Int | NativeTy::Bool | NativeTy::Float)
                                };
                                (is_scalar(&ret) && param_tys.iter().all(is_scalar))
                                    .then(|| native.module.compile(&jit_fn).ok())
                                    .flatten()
                                    .map(|id| (id, param_tys, ret))
                            },
                        )
                    };
                    native.self_recursive_native.insert(key, compiled.clone());
                    compiled?
                }
            }
        };
        if param_tys.len() != args.len() {
            return None;
        }
        // Marshal scalar args to i64 slots, driven by the compiled parameter type
        // (Float reinterpreted via `to_bits`; an Int value for a Float param is
        // converted first) — identical to the general native call marshalling.
        let mut int_args = Vec::with_capacity(args.len());
        for (&arg, pty) in args.iter().zip(param_tys.iter()) {
            let bits = match (pty, self.reg(caller_base + arg)) {
                (NativeTy::Float, VmValue::Float(f)) => f.to_bits() as i64,
                (NativeTy::Float, VmValue::Int(i)) => (*i as f64).to_bits() as i64,
                (_, VmValue::Int(i)) => *i,
                (_, VmValue::Bool(b)) => i64::from(*b),
                (_, VmValue::Float(f)) => f.to_bits() as i64,
                _ => return None,
            };
            int_args.push(bits);
        }
        let lens = vec![0i64; int_args.len()];
        let mut heap_tx = JitNativeCallFrame::begin();
        let outcome = {
            let native = self.native.as_ref()?;
            native
                .module
                .call_with_host_ctx(id, &int_args, &lens, heap_tx.host_ctx())
        };
        match outcome {
            vm_jit::NativeOutcome::Completed(bits) => {
                heap_tx.commit_scalar_with_writebacks(&[]);
                if let Some(native) = self.native.as_mut()
                    && native.collect_stats
                {
                    native.stats.native_calls += 1;
                }
                Some(match ret {
                    NativeTy::Float => VmValue::Float(f64::from_bits(bits as u64)),
                    NativeTy::Bool => VmValue::Bool(bits != 0),
                    _ => VmValue::Int(bits),
                })
            }
            _ => {
                heap_tx.abort();
                None
            }
        }
    }

    /// Native mutual recursion (native-call-ABI slice 4; generalized to scalar Float):
    /// if `function_id` is part of a mutually-recursive cycle of scalar functions,
    /// compile the whole group together (declare the cycle, then define each) and
    /// dispatch the called member natively. Arg/return marshalling follows each
    /// member's compiled scalar parameter/return `NativeTy`s (Int/Bool/Float — Float
    /// via `to_bits`/`from_bits`), exactly like `try_native_self_recursive`. Returns
    /// the result on a clean completion, or `None` to fall back to the interpreter
    /// (not eligible, non-scalar param/return, or a deopt incl. the depth-cap bail).
    /// The group is compiled once and every member cached.
    #[cfg(feature = "native-jit")]
    pub(super) fn try_native_mutual_recursive_int(
        &mut self,
        unit: &RegUnit,
        function_id: usize,
        caller_base: usize,
        args: &[usize],
    ) -> Option<VmValue> {
        let func = unit.functions.get(function_id)?;
        if args.len() != func.params {
            return None;
        }
        let key = Rc::as_ptr(func) as usize;
        // Resolve the called member's native id, its parameter types (to marshal each
        // scalar arg) and its return type (to wrap the i64 result). Cached per member;
        // compiling any member compiles the whole group.
        let (id, param_tys, ret) = {
            let native = self.native.as_mut()?;
            if native.force_bail || native.forced_safepoint.is_some() || native.force_all_safepoints
            {
                return None;
            }
            if let Some(cached) = native.mutual_recursive_native.get(&key) {
                cached.clone()?
            } else {
                let group = native_recursive_group(unit, function_id);
                let is_scalar =
                    |t: &NativeTy| matches!(t, NativeTy::Int | NativeTy::Bool | NativeTy::Float);
                let compiled = group.as_ref().and_then(|scc| {
                    let index_of: std::collections::HashMap<usize, u32> = scc
                        .iter()
                        .enumerate()
                        .map(|(i, &fid)| (fid, i as u32))
                        .collect();
                    let mut jit_funcs = Vec::with_capacity(scc.len());
                    let mut member_sigs = Vec::with_capacity(scc.len());
                    for &member in scc {
                        let mfunc = unit.functions.get(member)?;
                        let group_call_sites: std::collections::HashMap<usize, u32> = mfunc
                            .code
                            .iter()
                            .enumerate()
                            .filter_map(|(ip, instr)| match instr {
                                RegInstr::CallKnown {
                                    function, mut_args, ..
                                } if mut_args.is_empty() && index_of.contains_key(function) => {
                                    Some((ip, index_of[function]))
                                }
                                _ => None,
                            })
                            .collect();
                        let (jit_fn, ret, param_tys, _l, _pr) = translate_to_native_jit_with_calls(
                            unit,
                            mfunc,
                            &std::collections::HashMap::new(),
                            &std::collections::HashSet::new(),
                            &group_call_sites,
                        )?;
                        // Scalar-only ABI: params and return must be Int/Bool/Float
                        // (each wraps to/from an i64 slot). Heap params/returns decline.
                        if !is_scalar(&ret) || !param_tys.iter().all(is_scalar) {
                            return None;
                        }
                        member_sigs.push((param_tys, ret));
                        jit_funcs.push(jit_fn);
                    }
                    let ids = native.module.compile_recursive_group(&jit_funcs).ok()?;
                    Some((ids, member_sigs))
                });
                match (group, compiled) {
                    (Some(scc), Some((ids, member_sigs))) if ids.len() == scc.len() => {
                        let mut mine = None;
                        for (i, &member) in scc.iter().enumerate() {
                            let mkey = Rc::as_ptr(&unit.functions[member]) as usize;
                            let (param_tys, ret) = member_sigs[i].clone();
                            let entry = (ids[i], param_tys, ret);
                            native
                                .mutual_recursive_native
                                .insert(mkey, Some(entry.clone()));
                            if member == function_id {
                                mine = Some(entry);
                            }
                        }
                        mine?
                    }
                    (group, _) => {
                        // Cache ineligibility (for the whole detected group, or this key).
                        match group {
                            Some(scc) => {
                                for member in scc {
                                    let mkey = Rc::as_ptr(&unit.functions[member]) as usize;
                                    native.mutual_recursive_native.insert(mkey, None);
                                }
                            }
                            None => {
                                native.mutual_recursive_native.insert(key, None);
                            }
                        }
                        return None;
                    }
                }
            }
        };
        if param_tys.len() != args.len() {
            return None;
        }
        let mut int_args = Vec::with_capacity(args.len());
        for (&arg, pty) in args.iter().zip(param_tys.iter()) {
            // Scalar value args marshal to an i64 slot, driven by the compiled
            // parameter type (Float reinterpreted via `to_bits`; an Int value for a
            // Float param converted first) — identical to the self-recursion path.
            let bits = match (pty, self.reg(caller_base + arg)) {
                (NativeTy::Float, VmValue::Float(f)) => f.to_bits() as i64,
                (NativeTy::Float, VmValue::Int(i)) => (*i as f64).to_bits() as i64,
                (_, VmValue::Int(i)) => *i,
                (_, VmValue::Bool(b)) => i64::from(*b),
                (_, VmValue::Float(f)) => f.to_bits() as i64,
                _ => return None,
            };
            int_args.push(bits);
        }
        let lens = vec![0i64; int_args.len()];
        let mut heap_tx = JitNativeCallFrame::begin();
        let outcome = {
            let native = self.native.as_ref()?;
            native
                .module
                .call_with_host_ctx(id, &int_args, &lens, heap_tx.host_ctx())
        };
        match outcome {
            vm_jit::NativeOutcome::Completed(bits) => {
                heap_tx.commit_scalar_with_writebacks(&[]);
                if let Some(native) = self.native.as_mut()
                    && native.collect_stats
                {
                    native.stats.native_calls += 1;
                }
                Some(match ret {
                    NativeTy::Float => VmValue::Float(f64::from_bits(bits as u64)),
                    NativeTy::Bool => VmValue::Bool(bits != 0),
                    _ => VmValue::Int(bits),
                })
            }
            _ => {
                heap_tx.abort();
                None
            }
        }
    }

    pub(super) fn run_jit_self_recursive_int(
        &mut self,
        unit: &RegUnit,
        function_id: usize,
        caller_base: usize,
        args: &[usize],
    ) -> Result<Option<VmValue>, EvalError> {
        let func = Rc::clone(&unit.functions[function_id]);
        if args.len() != func.params {
            return Ok(None);
        }

        // General native fast path (native-call-ABI slice 3, generalized in Phase 2):
        // compile + run this function natively with self-recursive `CallSelf` via the
        // general native subset, which admits scalar Int/Bool/Float bodies (incl.
        // `match` and heap reads), not just the Int-arith whitelist. Marshalling and
        // result wrapping follow the compiled scalar parameter/return types. On a
        // clean completion return the result; otherwise fall through to the fallback
        // below (compile failure, non-scalar shape, or a depth-cap/deopt bail).
        #[cfg(feature = "native-jit")]
        if let Some(value) =
            self.try_native_self_recursive(unit, function_id, func.as_ref(), caller_base, args)
        {
            return Ok(Some(value));
        }

        // Fallback selection. The i64 tier-0 scalar executor can run only an
        // i64-representable (Int/Bool) Int-arith body — exactly what the scalar
        // candidate recognises; it handles arbitrary depth without touching the host
        // C stack. A non-i64 body (Float, or one using `match`/heap that the i64
        // machine cannot run) routes to the full interpreter (`Ok(None)`).
        let returns_bool = match self_recursive_scalar_jit_candidate(unit, function_id) {
            SelfRecursionKind::Ineligible => return Ok(None),
            SelfRecursionKind::Int => false,
            SelfRecursionKind::Bool => true,
        };

        // Marshal the scalar value args to i64 (Bool as 0/1). Both Int and Bool
        // params are i64-representable by the tier-0 machine.
        let mut stack = vec![0i64; func.regs];
        for (param, arg) in args.iter().enumerate() {
            let bits = match self.reg(caller_base + *arg) {
                VmValue::Int(value) => *value,
                VmValue::Bool(value) => i64::from(*value),
                _ => return Ok(None),
            };
            stack[param] = bits;
        }
        // Wrap an i64 result per the function's return kind.
        let wrap = |bits: i64| {
            if returns_bool {
                VmValue::Bool(bits != 0)
            } else {
                VmValue::Int(bits)
            }
        };

        #[derive(Debug, Clone, Copy)]
        struct ScalarFrame {
            ip: usize,
            base: usize,
            ret_dst: Option<usize>,
        }

        let mut frames: Vec<ScalarFrame> = Vec::new();
        if self.frames.len() + 1 > self.limits.max_depth {
            let max_depth = self.limits.max_depth;
            return Err(EvalError::Runtime(format!(
                "recursion depth limit exceeded ({max_depth} frames)"
            )));
        }
        frames.push(ScalarFrame {
            ip: 0,
            base: 0,
            ret_dst: None,
        });

        loop {
            let Some(frame) = frames.last_mut() else {
                return Ok(Some(VmValue::Unit));
            };
            let Some(instr) = func.code.get(frame.ip) else {
                let frame = frames.pop().expect("active scalar frame");
                stack.truncate(frame.base);
                if let Some(ret_dst) = frame.ret_dst {
                    stack[ret_dst] = 0;
                    continue;
                }
                return Ok(Some(VmValue::Unit));
            };
            self.tick()?;
            let base = frame.base;
            frame.ip += 1;
            match instr {
                RegInstr::LoadUnit { dst } => stack[base + *dst] = 0,
                RegInstr::LoadInt { dst, value } => stack[base + *dst] = *value,
                RegInstr::LoadBool { dst, value } => stack[base + *dst] = i64::from(*value),
                RegInstr::Move { dst, src } => {
                    stack[base + *dst] = stack[base + *src];
                }
                RegInstr::DeepCopy { .. } => {
                    // Primitive scalar values are already independent; the bytecode
                    // instruction exists for heap/value isolation in the generic VM.
                }
                RegInstr::AddInt { dst, lhs, rhs } => {
                    stack[base + *dst] = stack[base + *lhs]
                        .checked_add(stack[base + *rhs])
                        .ok_or_else(|| {
                            int_overflow_error("addition", stack[base + *lhs], stack[base + *rhs])
                        })?;
                }
                RegInstr::SubInt { dst, lhs, rhs } => {
                    stack[base + *dst] = stack[base + *lhs]
                        .checked_sub(stack[base + *rhs])
                        .ok_or_else(|| {
                            int_overflow_error(
                                "subtraction",
                                stack[base + *lhs],
                                stack[base + *rhs],
                            )
                        })?;
                }
                RegInstr::MulInt { dst, lhs, rhs } => {
                    stack[base + *dst] = stack[base + *lhs]
                        .checked_mul(stack[base + *rhs])
                        .ok_or_else(|| {
                            int_overflow_error(
                                "multiplication",
                                stack[base + *lhs],
                                stack[base + *rhs],
                            )
                        })?;
                }
                RegInstr::DivInt { dst, lhs, rhs } => {
                    let rhs_value = stack[base + *rhs];
                    if rhs_value == 0 {
                        return Err(EvalError::Runtime("integer division by zero".to_string()));
                    }
                    stack[base + *dst] =
                        stack[base + *lhs].checked_div(rhs_value).ok_or_else(|| {
                            int_overflow_error("division", stack[base + *lhs], rhs_value)
                        })?;
                }
                RegInstr::ModInt { dst, lhs, rhs } => {
                    let rhs_value = stack[base + *rhs];
                    if rhs_value == 0 {
                        return Err(EvalError::Runtime("integer modulo by zero".to_string()));
                    }
                    stack[base + *dst] =
                        stack[base + *lhs].checked_rem(rhs_value).ok_or_else(|| {
                            int_overflow_error("modulo", stack[base + *lhs], rhs_value)
                        })?;
                }
                RegInstr::LessInt { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] < stack[base + *rhs]);
                }
                RegInstr::LessEqualInt { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] <= stack[base + *rhs]);
                }
                RegInstr::GreaterInt { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] > stack[base + *rhs]);
                }
                RegInstr::GreaterEqualInt { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] >= stack[base + *rhs]);
                }
                RegInstr::Equal { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] == stack[base + *rhs]);
                }
                RegInstr::NotEqual { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] != stack[base + *rhs]);
                }
                RegInstr::Jump { target } => {
                    frames.last_mut().expect("active scalar frame").ip = *target;
                }
                RegInstr::JumpIfBool {
                    cond,
                    expected,
                    target,
                } => {
                    let taken = (stack[base + *cond] != 0) == *expected;
                    #[cfg(feature = "native-jit")]
                    if self.native.is_some() {
                        record_branch_site(&func, frame.ip - 1, taken);
                    }
                    if taken {
                        frames.last_mut().expect("active scalar frame").ip = *target;
                    }
                }
                RegInstr::JumpIfIntCompare {
                    lhs,
                    rhs,
                    op,
                    expected,
                    target,
                } => {
                    let taken =
                        eval_int_compare(*op, stack[base + *lhs], stack[base + *rhs]) == *expected;
                    #[cfg(feature = "native-jit")]
                    if self.native.is_some() {
                        record_branch_site(&func, frame.ip - 1, taken);
                    }
                    if taken {
                        frames.last_mut().expect("active scalar frame").ip = *target;
                    }
                }
                RegInstr::CallKnown {
                    dst,
                    function,
                    args,
                    mut_args,
                } if *function == function_id
                    && mut_args.is_empty()
                    && args.len() == func.params =>
                {
                    if self.frames.len() + frames.len() + 1 > self.limits.max_depth {
                        let max_depth = self.limits.max_depth;
                        return Err(EvalError::Runtime(format!(
                            "recursion depth limit exceeded ({max_depth} frames)"
                        )));
                    }
                    let ret_dst = base + *dst;
                    let callee_base = stack.len();
                    stack.resize(callee_base + func.regs, 0);
                    for (param, arg) in args.iter().enumerate() {
                        stack[callee_base + param] = stack[base + *arg];
                    }
                    frames.push(ScalarFrame {
                        ip: 0,
                        base: callee_base,
                        ret_dst: Some(ret_dst),
                    });
                }
                RegInstr::Return { src } => {
                    let value = stack[base + *src];
                    let frame = frames.pop().expect("active scalar frame");
                    stack.truncate(frame.base);
                    if let Some(ret_dst) = frame.ret_dst {
                        stack[ret_dst] = value;
                    } else {
                        return Ok(Some(wrap(value)));
                    }
                }
                _ => return Ok(None),
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScalarSlotKind {
    Unknown,
    Int,
    Bool,
    Unit,
}

fn self_recursive_scalar_jit_candidate(unit: &RegUnit, function_id: usize) -> SelfRecursionKind {
    let Some(func) = unit.functions.get(function_id) else {
        return SelfRecursionKind::Ineligible;
    };
    if let Some(cached) = func.jit_self_recursion_kind.get() {
        return cached;
    }
    let kind = compute_self_recursive_scalar_jit_candidate(unit, function_id);
    func.jit_self_recursion_kind.set(Some(kind));
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
                RegInstr::DeepCopy { reg } => kinds.get(*reg).map(|_| false),
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

/// The mutually-recursive group (call-graph SCC) containing `function_id`, if it is
/// a cycle of >= 2 functions (native-call-ABI slice 4). Returned sorted; `None` for
/// non-cyclic functions and pure self-recursion (handled by the self-recursive
/// path). Per-member native eligibility (scalar params/return, native-subset body)
/// is NOT decided here — the caller's `translate_to_native_jit_with_calls` per member
/// is the single eligibility gate, so the group admits any cycle the general native
/// path can compile (Int/Bool/Float bodies, members that also call inlinable leaves).
#[cfg(feature = "native-jit")]
fn native_recursive_group(unit: &RegUnit, function_id: usize) -> Option<Vec<usize>> {
    use std::collections::HashSet;
    let callees = |fid: usize| -> Vec<usize> {
        unit.functions.get(fid).map_or_else(Vec::new, |f| {
            f.code
                .iter()
                .filter_map(|instr| match instr {
                    RegInstr::CallKnown { function, .. } => Some(*function),
                    _ => None,
                })
                .collect()
        })
    };
    // Forward-reachable from `function_id` via CallKnown edges.
    let mut fwd = HashSet::new();
    let mut stack = vec![function_id];
    while let Some(f) = stack.pop() {
        if fwd.insert(f) {
            stack.extend(callees(f));
        }
    }
    // Backward-reachable: functions that can transitively reach `function_id`.
    let mut bwd = HashSet::new();
    let mut stack = vec![function_id];
    while let Some(target) = stack.pop() {
        if bwd.insert(target) {
            for caller in 0..unit.functions.len() {
                if callees(caller).contains(&target) {
                    stack.push(caller);
                }
            }
        }
    }
    // The SCC is the intersection (mutually reachable with `function_id`).
    let mut scc: Vec<usize> = fwd.intersection(&bwd).copied().collect();
    scc.sort_unstable();
    if scc.len() < 2 {
        return None;
    }
    Some(scc)
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
