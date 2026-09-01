use super::*;

#[cfg(feature = "native-jit")]
mod admission;
mod attempt_native;
mod osr_plan_builder;
#[cfg(feature = "native-jit")]
use osr_plan_builder::OsrPlanInputs;
#[cfg(feature = "native-jit")]
mod call_scratch;
#[cfg(feature = "native-jit")]
mod compile_result;
#[cfg(feature = "native-jit")]
mod continuation;
#[cfg(feature = "native-jit")]
mod deopt_resume;
mod jit_entry;
#[cfg(feature = "native-jit")]
mod osr_plan;
mod recursion;
mod state;

#[cfg(feature = "native-jit")]
use deopt_resume::NativeChildDeoptResume;

#[cfg(feature = "native-jit")]
use admission::*;
#[cfg(feature = "native-jit")]
use call_scratch::*;
#[cfg(feature = "native-jit")]
use compile_result::{native_region_is_promotion_eligible, record_native_compile_stats};
#[cfg(feature = "native-jit")]
use osr_plan::*;
pub(crate) use state::JitState;

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn osr_controls_unarmed_for_dispatch(limits: &VmLimits) -> bool {
    osr_execution_controls_supported(limits)
}

/// Conservative pre-translation proof for whole-function memory controls. Stable
/// native entry is allowed only when the source body cannot grow retained storage:
/// scalar operations, shape-preserving field/list stores, and read-only helpers.
/// Regions that need allocation accounting are handled by the OSR transaction path.
#[cfg(feature = "native-jit")]
fn whole_function_memory_controls_supported(func: &RegFunction, limits: &VmLimits) -> bool {
    if limits.allocation_budget.is_none() && limits.live_memory_limit.is_none() {
        return true;
    }
    func.code.iter().all(|instr| {
        if native_instruction_has_heap_write(instr)
            && !matches!(
                instr,
                RegInstr::SetFieldSlot { .. } | RegInstr::ListSet { .. }
            )
        {
            return false;
        }
        match instr {
            RegInstr::CallIntrinsic {
                intrinsic, args, ..
            } => native_host_typed_intrinsic(*intrinsic, None).is_none_or(|spec| {
                args.len() == spec.arg_tys().len()
                    && spec.helper.heap_effect() == vm_jit::HostHeapEffect::ReadOnly
            }),
            RegInstr::CallTypedIntrinsic {
                intrinsic,
                type_arg,
                args,
                ..
            } => native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str())).is_none_or(
                |spec| {
                    args.len() == spec.arg_tys().len()
                        && spec.helper.heap_effect() == vm_jit::HostHeapEffect::ReadOnly
                },
            ),
            RegInstr::CallKnown { .. }
            | RegInstr::CallDynamic { .. }
            | RegInstr::CallClosure { .. }
            | RegInstr::CallExternal { .. } => false,
            _ => true,
        }
    })
}

/// Post-translation memory proof used by OSR. Direct flat-buffer writes and
/// shape-preserving list stores do not allocate. `List.push` is the sole growing
/// helper admitted here because its exact capacity delta is charged through the
/// transaction-local memory cell and committed only on a clean OSR exit.
#[cfg(feature = "native-jit")]
fn osr_memory_controls_supported(function: &vm_jit::JitFunction, memory_armed: bool) -> bool {
    if !memory_armed {
        return true;
    }
    function.code.iter().all(|instr| match instr {
        vm_jit::JitInstr::HostCall { helper, .. } => {
            helper.heap_effect() == vm_jit::HostHeapEffect::ReadOnly
                || matches!(
                    helper,
                    vm_jit::HostHelper::ListSetInt
                        | vm_jit::HostHelper::ListSetFloat
                        | vm_jit::HostHelper::ListPushInt
                        | vm_jit::HostHelper::ListPushFloat
                )
        }
        vm_jit::JitInstr::MemoizedHostCall { helper, .. } => {
            helper.heap_effect() == vm_jit::HostHeapEffect::ReadOnly
        }
        vm_jit::JitInstr::CallNative { .. } => false,
        _ => true,
    })
}

/// Step 1 cost model. Consult the profitability gate for a region that already
/// translated (i.e. is ELIGIBLE native code). Returns `true` only when the gate is
/// in `enforce` mode AND the region is unprofitable — the caller then keeps the
/// function on the interpreter. In `off` mode this is a no-op (`false`); in
/// `report` mode it records telemetry but always returns `false`, so execution is
/// unchanged. Declining is correctness-safe, so
/// this never affects the differential/eligibility contract.
#[cfg(feature = "native-jit")]
fn consult_profitability(
    native: &mut NativeState,
    jit_fn: &vm_jit::JitFunction,
    has_backedge: bool,
    region: &str,
    func_name: &str,
) -> bool {
    let mode = native.cost_model;
    if !mode.active() {
        return false;
    }
    let p = native_region_profitability(jit_fn, has_backedge);
    if !p.decline {
        return false;
    }
    if native.collect_stats {
        native.stats.unprofitable_declines += 1;
        *native
            .stats
            .unprofitable_decline_reasons
            .entry(p.reason(region))
            .or_insert(0) += 1;
        // Runtime ATTRIBUTION: record which actual function/region was declined, so
        // the report can say per-function "declined by cost model" from ground truth
        // rather than a fragile re-derivation (which loses profile-guided PICs).
        native
            .stats
            .unprofitable_declined_fns
            .entry(func_name.to_string())
            .or_insert_with(|| p.reason(region));
    }
    // `report` observes but never changes execution; only `enforce` declines.
    matches!(mode, NativeCostModel::Enforce)
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
        param_tys
            .get(pos)
            .is_some_and(|ty| matches!(ty, NativeTy::FlatIntMut | NativeTy::FlatFloatMut))
    })
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
fn controlled_static_inline_candidate(
    callee: &RegFunction,
    facts: &VerifiedFunctionFacts,
    n_args: usize,
    mut_args: &[usize],
) -> bool {
    const MAX_STATIC_INLINE_INSTRUCTIONS: usize = 24;
    mut_args.is_empty()
        && !callee.code.is_empty()
        && callee.code.len() <= MAX_STATIC_INLINE_INSTRUCTIONS
        && !jit_function_has_loop(&callee.code)
        && native_callee_inlinable(callee, n_args)
        && facts
            .reg_types
            .iter()
            .take(callee.params)
            .all(|ty| {
                matches!(
                    ty,
                    VerifiedStorageType::Int
                        | VerifiedStorageType::Bool
                        | VerifiedStorageType::Float
                )
            })
        && callee.code.iter().all(|instr| {
            !matches!(instr, RegInstr::Return { src } if !matches!(facts.reg_types.get(*src), Some(VerifiedStorageType::Int | VerifiedStorageType::Bool | VerifiedStorageType::Float)))
        })
        && facts.effects.iter().all(|effect| {
            !effect.writes_heap
                && !effect.may_allocate
                && !effect.may_call_provider
                && !effect.may_suspend
                && !effect.may_spawn
                && !effect.touches_resource
        })
}

#[cfg(feature = "native-jit")]
fn native_compile_direct_scalar_callee(
    jit_state: &JitState,
    native: &mut NativeState,
    unit: &RegUnit,
    callee: &RegFunction,
    callee_key: usize,
    call_site: Option<&VerifiedCallSite>,
    stack: &mut std::collections::HashSet<usize>,
) -> Option<NativeCompiledCallee> {
    let facts = Rc::clone(native.verified_facts.as_ref()?);
    let base_facts = facts.function(callee_key)?;
    let specialized;
    let callee_facts = if let Some(call) = call_site.filter(|call| !call.type_arguments.is_empty())
    {
        specialized = base_facts.instantiate_parameter_storage(callee.params, call)?;
        &specialized
    } else {
        base_facts
    };
    // Call-site substitutions select only a bounded code-cache identity. The
    // translator still consumes `callee_facts`, whose storage/signature facts
    // were independently cross-checked against the executable.
    let instance = call_site.map_or_else(
        || JitInstanceKey::from_type_arguments(callee_key, &[]),
        |call| JitInstanceKey::from_call_site(callee_key, call),
    )?;
    let version_key = NativeVersionKey {
        instance: instance.clone(),
        shape: ShapeKey::default(),
    };
    if let Some(cached) = native.cache.get(&version_key) {
        return cached
            .as_ref()
            .and_then(native_compiled_entry_call_descriptor);
    }
    if native.whole_shape_count(&instance) >= MAX_NATIVE_SHAPE_VERSIONS {
        return None;
    }
    if !stack.insert(callee_key) {
        return None;
    }

    let nested_call_sites =
        native_compiled_call_sites_inner(jit_state, native, unit, callee, callee_key, stack);
    let profile = jit_state.profile(callee);
    let call_count = jit_state.call_count(callee);
    let translated = if nested_call_sites.is_empty() {
        translate_to_native_jit(unit, callee, callee_facts, profile, call_count)
    } else {
        translate_to_native_jit_with_compiled_callees(
            unit,
            callee,
            callee_facts,
            profile,
            call_count,
            &nested_call_sites,
        )
        .or_else(|| translate_to_native_jit(unit, callee, callee_facts, profile, call_count))
    };
    let Some(NativeTranslation {
        jit_fn,
        return_ty: ret,
        param_tys: params,
        string_literals,
        precise_resume_safe,
    }) = translated
    else {
        stack.remove(&callee_key);
        return None;
    };
    let scalar_leaf_callable = vm_jit::is_native_callable_leaf(&jit_fn);
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
    let Some(admission) = begin_native_compile(native, 1, NativeCodeTier::Baseline) else {
        stack.remove(&callee_key);
        return None;
    };
    let compiled = if native.force_all_safepoints {
        native.baseline_module.compile_forcing_all_bails(&jit_fn)
    } else {
        match native.forced_safepoint {
            Some(site) => native.baseline_module.compile_forcing_bail(&jit_fn, site),
            None => native.baseline_module.compile_native_callee(&jit_fn),
        }
    };
    let id = match compiled {
        Ok(id) => id,
        Err(err) => {
            finish_native_compile_failure(native, admission);
            if native.report {
                eprintln!("jit-report: native callee compile failed: {err}");
            }
            if native.collect_stats {
                native.stats.compile_failed += 1;
            }
            stack.remove(&callee_key);
            return None;
        }
    };
    if !finish_native_compile(native, admission, &[id], NativeCodeTier::Baseline) {
        stack.remove(&callee_key);
        return None;
    }
    let verify_native = cfg!(debug_assertions) || jit_native_verify_is_strict();
    if verify_native
        && let Err(err) = jit_verify_compiled_native(
            &native.baseline_module,
            id,
            &jit_fn,
            native.forced_safepoint,
        )
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
    record_native_compile_stats(native, id, &jit_fn, NativeCodeTier::Baseline);
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
    let first_static_instance = version_key.instance.type_arguments.is_known()
        && !native.has_whole_instance(&version_key.instance);
    native.cache.insert(version_key, Some(entry));
    if native.collect_stats {
        native.stats.shape_versions += 1;
        if first_static_instance {
            native.stats.static_type_instances += 1;
        }
    }
    stack.remove(&callee_key);
    Some(NativeCompiledCallee {
        id,
        ret_ty: ret,
        param_tys: params,
    })
}

#[cfg(feature = "native-jit")]
fn native_compiled_call_sites(
    jit_state: &JitState,
    native: &mut NativeState,
    unit: &RegUnit,
    func: &RegFunction,
    self_key: usize,
) -> std::collections::HashMap<usize, NativeCompiledCallee> {
    let mut stack = std::collections::HashSet::new();
    stack.insert(self_key);
    native_compiled_call_sites_inner(jit_state, native, unit, func, self_key, &mut stack)
}

#[cfg(feature = "native-jit")]
fn native_compiled_call_sites_inner(
    jit_state: &JitState,
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
        let callee_key = callee.ordinal;
        if callee_key == self_key {
            continue;
        }
        let call_site = native
            .verified_facts
            .as_ref()
            .and_then(|facts| facts.function(self_key))
            .and_then(|facts| facts.call_site(ip))
            .cloned();
        let Some(callee_facts) = native
            .verified_facts
            .as_ref()
            .and_then(|facts| facts.function(callee_key))
        else {
            continue;
        };
        if controlled_static_inline_candidate(callee, callee_facts, args.len(), mut_args) {
            if native.collect_stats {
                native.stats.static_inline_candidates += 1;
            }
            // Omitting this site from the preserve map routes it through the
            // existing origin-aware leaf inliner. No second callee compile or
            // child ABI edge is emitted for the small, pure scalar body.
            continue;
        }
        let Some(descriptor) = native_compile_direct_scalar_callee(
            jit_state,
            native,
            unit,
            callee,
            callee_key,
            call_site.as_ref(),
            stack,
        ) else {
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
                    // #7 foldable cold-arm sub-case: check inlinability of the
                    // string-folded body (the inline pass folds it identically before
                    // splicing). The fold is semantics-preserving and a no-op for ordinary
                    // bodies, so this only ADMITS more leaves, never changes existing ones.
                    let folded = native_string_folded_callee(callee);
                    let effective = folded.as_ref().unwrap_or(callee);
                    native_callee_inlinable_j3_with_spawns(unit, effective, args.len())
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
    let has_heap_write = region.iter().any(native_instruction_has_heap_write);
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
pub(in crate::reg_vm) fn select_osr_candidate_loops(
    unit: &RegUnit,
    func: &RegFunction,
) -> Vec<OsrLoop> {
    let mut candidates: Vec<_> = detect_natural_loops(&func.code)
        .into_iter()
        .filter(|lp| osr_loop_region_is_transform_candidate(unit, func, *lp))
        .collect();
    candidates.sort_by_key(|lp| std::cmp::Reverse(osr_loop_candidate_score(&func.code, *lp)));
    candidates.truncate(MAX_OSR_REGIONS_PER_FUNCTION);
    candidates
}

impl RegVm {

    /// OSR (on-stack replacement). The interpreter has reached `header_ip` —
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
    /// Resolve up to four deterministically ranked OSR candidates on first entry
    /// during this evaluation. Detection does not compile; each region compiles only
    /// when its own threshold is reached. A non-candidate function caches an empty
    /// fixed-size set and retains a hoisted never-taken interpreter branch.
    ///
    /// Determinism: this only decides *whether/when* to attempt OSR; `try_osr` is
    /// byte-identical to interpretation, so triggering never changes a value.
    #[cfg(feature = "native-jit")]
    pub(super) fn try_osr(
        &mut self,
        function: usize,
        func: &RegFunction,
        base: usize,
        header_ip: usize,
    ) -> bool {
        if JitCallCtx::is_active() {
            return false;
        }
        // Until transformed instructions carry source step, host-call, and
        // allocation costs, no armed resource mode is allowed through OSR. The
        // interpreter remains the semantic authority. Cancellation also stays on
        // that path even though vm-jit's raw cancel load is now atomic.
        if !osr_execution_controls_supported(&self.limits) {
            return false;
        }
        // These values remain false after the gate above. Keeping the compile-time
        // plumbing intact allows a future source-cost implementation to re-enable
        // proven limit-aware OSR without changing the cache shape again.
        let emit_step = self.limits.step_budget.is_some()
            || self.limits.cancel.is_some()
            || self.limits.deadline.is_some();
        let emit_cancel = self.limits.cancel.is_some();
        let emit_deadline = self.limits.deadline.is_some();
        let allocation_armed = self.limits.allocation_budget.is_some();
        let live_memory_armed = self.limits.live_memory_limit.is_some();
        let memory_armed = allocation_armed || live_memory_armed;
        if let Some(native) = self.native.as_mut() {
            native.osr_dynamic_bail = false;
        }
        let native_key = function;
        let Some(verified_facts) = self
            .native
            .as_ref()
            .and_then(|native| native.verified_facts.as_ref())
            .cloned()
        else {
            return false;
        };
        let Some(function_facts) = verified_facts.function(native_key) else {
            return false;
        };
        let Some(instance) = JitInstanceKey::from_facts(native_key, function_facts) else {
            if let Some(native) = self.native.as_mut()
                && native.collect_stats
            {
                native.stats.static_instance_limit_fallbacks += 1;
            }
            return false;
        };
        let profile = self.jit_state.profile(func);
        let call_count = self.jit_state.call_count(func);
        let region_key = RegionKey {
            function: native_key,
            header: header_ip,
        };
        let shape = ShapeKey::from_shapes((0..func.regs).map(|index| {
            let slot = base + index;
            if !self.written.get(slot).copied().unwrap_or(false) {
                return NativeParamShape::Unsupported;
            }
            let shape = native_param_shape_with_fact(
                &self.stack[slot],
                function_facts
                    .reg_types
                    .get(index)
                    .copied()
                    .unwrap_or_default(),
            );
            if index < func.params
                || matches!(
                    shape,
                    NativeParamShape::Closure
                        | NativeParamShape::Struct(_)
                        | NativeParamShape::Variant(_)
                )
            {
                shape
            } else {
                NativeParamShape::Unsupported
            }
        }));
        let osr_version_key = OsrVersionKey {
            region: region_key,
            type_arguments: instance.type_arguments.clone(),
            shape,
        };

        // heap-aware deopt #7 (live-after heap payload): classify each param by its LIVE value, for the
        // OSR translator to seed a param used in-region ONLY as a dissolved Result/Option
        // payload `Move` (instruction inference can't type it — no typed use). `None` =
        // don't seed: a `List`/`Deque` may be flat-classified (Handle≠Flat conflicts), and
        // other kinds are left conservative. The translator additionally taint-gates the
        // seed by in-region usage so a heap collection key/value param (typed Int as the
        // handle-index by the helper spec) is never seeded Handle.
        let param_native_types: Vec<Option<NativeTy>> = (0..func.params)
            .map(|i| match self.reg(base + i) {
                VmValue::Int(_) => Some(NativeTy::Int),
                VmValue::Bool(_) => Some(NativeTy::Bool),
                VmValue::Float(_) => Some(NativeTy::Float),
                VmValue::String(_) | VmValue::Struct(_) | VmValue::Variant(_) | VmValue::Map(_) => {
                    Some(NativeTy::Handle)
                }
                _ => None,
            })
            .collect();
        // For the DeepCopy soundness guard: a non-`mut` param whose runtime value is an
        // immutable `String`/`Bytes` is safe to share (it cannot be mutated and holds no
        // mutable sub-value), so it must NOT be tainted — otherwise the guard would decline
        // the common `read` string into `mut` collection pattern. Everything else (mutable
        // containers, structs that may hold them) stays seeded.
        let immutable_leaf_params: Vec<bool> = (0..func.params)
            .map(|i| matches!(self.reg(base + i), VmValue::String(_) | VmValue::Bytes(_)))
            .collect();

        // Phase 1: resolve (and lazily compile) the OSR loop body for this function,
        // then gate on being at the loop header. This runs at every instruction when
        // OSR is armed, so it must be cheap on the common (not-at-header) path: the
        // cache lookup + header compare returns without cloning anything.
        // `param_types` (the param-prefix of `reg_types`) is no longer consulted here:
        // OSR marshalling now classifies every live-in by the full per-register
        // `reg_types` (so a non-param flat list is marshalled correctly). It stays in
        // the cache entry for the non-OSR `try_native` path.
        let function_facts_for_plan = function_facts.clone();
        let profile_owned = profile.cloned();
        let (
            id,
            _trans_exit,
            orig_exit,
            n_jit_regs,
            _param_types,
            derived_liveins,
            scalar_fields,
            heap_input_regs,
            reg_types,
            written_regs,
            string_literals,
            materialize_recipes,
            selected_tier,
        ) = match self.build_osr_plan(OsrPlanInputs {
            func,
            native_key,
            header_ip,
            function_facts: function_facts_for_plan,
            instance,
            call_count,
            profile: profile_owned.as_ref(),
            emit_step,
            emit_cancel,
            emit_deadline,
            memory_armed,
            immutable_leaf_params: &immutable_leaf_params,
            param_native_types: &param_native_types,
            osr_version_key: &osr_version_key,
            region_key,
        }) {
            Some(plan) => plan,
            None => return false,
        };

        // Phase 2: marshal the current register window into the OSR call. The OSR
        // ABI's `args_ptr` is the full (TRANSFORMED) `n_jit_regs`-wide window indexed
        // by register: a scalar register contributes its raw bits, a handle
        // (List/struct) param its heap-table index (the host helpers read through
        // it), and an unwritten / non-scalar slot contributes 0 (native only ever
        // loads the live-in subset, all of which are written by definite-assignment
        // at the header). Under OSR × scalar replacement the window is wider than `func.regs` by the
        // fresh tag/payload registers; only original registers `0..func.regs` carry
        // an interpreter value, and the scalar replacement-added slots stay 0 — the loop body assigns
        // them (LoadBool tag, Move payload) before any read, so their live-in value
        // is irrelevant. A drop guard clears the heap table on every exit path.
        let mut heap_tx = JitNativeCallFrame::begin(self.limits.deadline);
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

        let flat_alias = scratch
            .flat_mut_owned
            .iter()
            .any(|lhs| scratch.flat_owned.iter().any(|rhs| Rc::ptr_eq(lhs, rhs)))
            || jit_selected_heap_inputs_alias_flat_mut(
                &scratch.heap_input_slots,
                &scratch.flat_mut_owned,
                base,
                &heap_input_regs,
            )
            || ((!scratch.flat_owned.is_empty())
                && func.code.iter().any(|instr| {
                    native_instruction_has_heap_write(instr)
                        || matches!(instr, RegInstr::CallKnown { .. })
                })
                && jit_selected_heap_inputs_alias_flat_mut(
                    &scratch.heap_input_slots,
                    &scratch.flat_owned,
                    base,
                    &heap_input_regs,
                ));
        if flat_alias {
            bail_osr!();
        }

        // One Rust borrow proves one unique backing buffer, while multiple native
        // registers may intentionally carry the same pointer. Validate the slot map
        // before taking any Ref/RefMut so malformed transform metadata falls back
        // without panicking inside the marshaller.
        if scratch.flat_owned.len() != scratch.flat_slots.len()
            || scratch.flat_mut_slots.iter().any(|(owner, reg)| {
                *owner >= scratch.flat_mut_owned.len()
                    || !matches!(
                        reg_types.get(*reg),
                        Some(NativeTy::FlatIntMut | NativeTy::FlatFloatMut)
                    )
            })
            || (0..scratch.flat_mut_owned.len()).any(|owner| {
                let mut slots = scratch
                    .flat_mut_slots
                    .iter()
                    .filter(|(candidate, _)| *candidate == owner);
                let Some((_, first_reg)) = slots.next() else {
                    return true;
                };
                let first_ty = reg_types.get(*first_reg);
                slots.any(|(_, reg)| reg_types.get(*reg) != first_ty)
            })
        {
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
        let mut flat_args = Vec::with_capacity(flat_guards.len() + flat_mut_guards.len());
        for (guard, (reg, ty)) in flat_guards.iter().zip(&scratch.flat_slots) {
            match ty {
                NativeTy::FlatInt => {
                    let Some(slice) = guard.as_ints_slice() else {
                        unreachable!("flat kind validated before pinning")
                    };
                    scratch.window[*reg] = slice.as_ptr() as i64;
                    scratch.lens[*reg] = slice.len() as i64;
                    flat_args.push(vm_jit::IndexedFlatBufferArg::new(
                        *reg,
                        vm_jit::FlatBufferArg::Int(slice),
                    ));
                }
                NativeTy::FlatFloat => {
                    let Some(slice) = guard.as_floats_slice() else {
                        unreachable!("flat kind validated before pinning")
                    };
                    scratch.window[*reg] = slice.as_ptr() as i64;
                    scratch.lens[*reg] = slice.len() as i64;
                    flat_args.push(vm_jit::IndexedFlatBufferArg::new(
                        *reg,
                        vm_jit::FlatBufferArg::Float(slice),
                    ));
                }
                _ => unreachable!("shared flat slot has mutable/non-flat type"),
            }
        }
        for &(owner, reg) in &scratch.flat_mut_slots {
            let guard = &flat_mut_guards[owner];
            match reg_types[reg] {
                NativeTy::FlatIntMut => {
                    let Some(slice) = guard.as_ints_slice() else {
                        unreachable!("flat kind validated before pinning")
                    };
                    scratch.window[reg] = slice.as_ptr() as i64;
                    scratch.lens[reg] = slice.len() as i64;
                }
                NativeTy::FlatFloatMut => {
                    let Some(slice) = guard.as_floats_slice() else {
                        unreachable!("flat kind validated before pinning")
                    };
                    scratch.window[reg] = slice.as_ptr() as i64;
                    scratch.lens[reg] = slice.len() as i64;
                }
                _ => unreachable!("mutable flat slot has non-mutable type"),
            }
        }
        for (owner, guard) in flat_mut_guards.iter_mut().enumerate() {
            let (_, reg) = scratch
                .flat_mut_slots
                .iter()
                .find(|(candidate, _)| *candidate == owner)
                .expect("mutable owner validated to have a slot");
            match reg_types[*reg] {
                NativeTy::FlatIntMut => {
                    let Some(slice) = guard.as_ints_mut_slice() else {
                        unreachable!("flat kind validated before pinning")
                    };
                    flat_args.push(vm_jit::IndexedFlatBufferArg::new(
                        *reg,
                        vm_jit::FlatBufferArg::IntMut(slice),
                    ));
                }
                NativeTy::FlatFloatMut => {
                    let Some(slice) = guard.as_floats_mut_slice() else {
                        unreachable!("flat kind validated before pinning")
                    };
                    flat_args.push(vm_jit::IndexedFlatBufferArg::new(
                        *reg,
                        vm_jit::FlatBufferArg::FloatMut(slice),
                    ));
                }
                _ => unreachable!("mutable flat owner has non-mutable type"),
            }
        }

        // Phase 3: run the OSR loop body natively.
        // native limit accounting: seed the limits cell for an armed variant. `emit_step`/`emit_cancel`
        // were fixed at the top of this call from `self.limits`; the compiled variant
        // matches (same eval-constant limits), so a non-null cell is required exactly
        // when armed. `steps` flows in here and back out below into `self.steps`.
        let armed = emit_step || emit_cancel || emit_deadline;
        // native limit accounting mem: seed the mem cell before EVERY OSR call (with
        // an optional full-width budget). The `ListPush*` helper charges flat-capacity growth against it; on a
        // clean exit we read `allocated_bytes` back to commit, on a bail the rollback+rerun
        // discards it. Independent of the step `limits_ptr` (helper-side).
        jit_set_mem_cell(self.allocated_bytes, self.limits.allocation_budget);
        let Some(native_ref) = self.native.as_mut() else {
            heap_tx.abort();
            drop(flat_guards);
            drop(flat_mut_guards);
            scratch.restore(self.native.as_mut());
            return false;
        };
        let collect_stats = native_ref.collect_stats;
        let started = collect_stats.then(std::time::Instant::now);
        let _literal_guard = jit_install_string_literals(&string_literals);
        let physical_depth = self.frames.len();
        let prior_tail_calls = self.frames.last().map_or(0, |frame| frame.tail_calls);
        let initial_depth = osr_initial_logical_depth(physical_depth, prior_tail_calls);
        let module = match selected_tier {
            NativeCodeTier::Baseline => &native_ref.baseline_module,
            NativeCodeTier::Optimized => native_ref
                .optimized_module
                .as_ref()
                .expect("optimized OSR dispatch requires optimized module"),
        };
        flat_args.sort_unstable_by_key(|proof| proof.index);
        let initial_steps = i64::try_from(self.steps).unwrap_or(i64::MAX);
        let step_budget = self
            .limits
            .step_budget
            .and_then(|budget| i64::try_from(budget).ok());
        let (result, native_steps) = if armed {
            module.call_with_indexed_flat_args_and_controls_in_session_at_depth(
                &mut native_ref.call_session,
                id,
                &scratch.window,
                &scratch.lens,
                &mut flat_args,
                vm_jit::RegionCallControls {
                    host_ctx: heap_tx.host_ctx(),
                    logical_depth: vm_jit::LogicalCallDepth {
                        current: initial_depth,
                        limit: self.limits.max_depth,
                    },
                    initial_steps,
                    step_budget,
                    cancel: self.limits.cancel.as_ref().map(|token| token.as_atomic()),
                },
            )
        } else {
            (
                module.call_with_indexed_flat_args_at_depth(
                    id,
                    &scratch.window,
                    &scratch.lens,
                    heap_tx.host_ctx(),
                    &mut flat_args,
                    vm_jit::LogicalCallDepth {
                        current: initial_depth,
                        limit: self.limits.max_depth,
                    },
                ),
                initial_steps,
            )
        };
        let elapsed = started.map(|started| started.elapsed().as_nanos());
        // The pinned borrows are no longer needed once the native call returns.
        drop(flat_guards);
        drop(flat_mut_guards);
        // native limit accounting: fold the steps native paid (clean completion OR deopt both wrote it
        // back) into the interpreter's counter, so resuming the interpreter continues
        // the single tick stream with no double-/under-count.
        if emit_step {
            self.steps = native_steps.max(0) as u64;
        }
        if let Some(native) = self.native.as_mut()
            && let Some(elapsed) = elapsed
        {
            native.stats.run_nanos += elapsed;
        }

        // Phase 4: OSR-exit. The loop always exits via the `OsrExit` safepoint (a
        // deopt). Resume the interpreter at the post-loop ip with the restored
        // live-out window — the precise-deopt resume, reused verbatim.
        match result {
            vm_jit::NativeOutcome::Deopt {
                safepoint_id,
                live,
                logical_depth: Some(final_logical_depth),
                ..
            } if safepoint_id.0 >= 1 => {
                let resume_ip = self
                    .native
                    .as_ref()
                    .and_then(|native| match selected_tier {
                        NativeCodeTier::Baseline => native.baseline_module.deopt_map(id),
                        NativeCodeTier::Optimized => native
                            .optimized_module
                            .as_ref()
                            .and_then(|module| module.deopt_map(id)),
                    })
                    .and_then(|m| m.sites.get(safepoint_id.0 as usize - 1))
                    .map(|site| site.resume_ip);
                let Some(resume_ip) = resume_ip else {
                    heap_tx.abort();
                    scratch.restore(self.native.as_mut());
                    return false;
                };
                // The OSR-exit's explicit resume_ip MUST be the original bytecode
                // post-loop exit; anything else is an OSR construction bug. Fall
                // back rather than misresume.
                if resume_ip as usize != orig_exit {
                    heap_tx.abort();
                    scratch.restore(self.native.as_mut());
                    return false;
                }
                // Materialize ordinary Handle live-outs before committing the heap
                // transaction: commit clears the per-call handle tables. Keep the
                // cloned VmValues aside until every handle has resolved and the
                // transaction has committed, so a partial resolution failure cannot
                // update the interpreter frame.
                let mut handle_liveouts = Vec::new();
                for live_reg in &live {
                    let reg = live_reg.reg as usize;
                    if reg < func.params
                        || reg >= func.regs
                        || !written_regs.get(reg).copied().unwrap_or(false)
                        || reg_types.get(reg) != Some(&NativeTy::Handle)
                    {
                        continue;
                    }
                    let vm_jit::DeoptValue::Handle(handle) = live_reg.value else {
                        heap_tx.abort();
                        scratch.restore(self.native.as_mut());
                        return false;
                    };
                    let Some(value) = JitHostCallCtx::active()
                        .and_then(|ctx| ctx.heap_read_handle(handle, |value| Some(value.clone())))
                    else {
                        heap_tx.abort();
                        scratch.restore(self.native.as_mut());
                        return false;
                    };
                    handle_liveouts.push((base + reg, value));
                }
                let Some(materialize_ctx) = JitHostCallCtx::active() else {
                    heap_tx.abort();
                    scratch.restore(self.native.as_mut());
                    return false;
                };
                let mut aggregate_liveouts = Vec::with_capacity(materialize_recipes.len());
                for recipe in &materialize_recipes {
                    let mut nodes = 0;
                    let Some(value) =
                        osr_materialize_value(&recipe.value, &live, materialize_ctx, 0, &mut nodes)
                    else {
                        heap_tx.abort();
                        scratch.restore(self.native.as_mut());
                        return false;
                    };
                    aggregate_liveouts.push((base + recipe.dst_reg, value));
                }
                // Native helpers mutate journaled VM-owned containers in place.
                // Under a live-memory limit, measure that exact tentative graph
                // before committing the journal. An over-limit result aborts and
                // lets the interpreter replay the source operation; restore the
                // diagnostic counters so the failed native attempt is invisible.
                let live_memory_snapshot = live_memory_armed.then_some((
                    self.live_memory_bytes,
                    self.peak_live_memory_bytes,
                    self.live_memory_dirty,
                ));
                if live_memory_armed {
                    self.live_memory_dirty = true;
                    if self.refresh_live_memory_usage().is_err() {
                        if let Some((live, peak, dirty)) = live_memory_snapshot {
                            self.live_memory_bytes = live;
                            self.peak_live_memory_bytes = peak;
                            self.live_memory_dirty = dirty;
                        }
                        heap_tx.abort();
                        scratch.restore(self.native.as_mut());
                        return false;
                    }
                }
                let Some(writebacks) =
                    heap_tx.commit_scalar_with_writebacks(&scratch.heap_input_slots)
                else {
                    if let Some((live, peak, dirty)) = live_memory_snapshot {
                        self.live_memory_bytes = live;
                        self.peak_live_memory_bytes = peak;
                        self.live_memory_dirty = dirty;
                    }
                    heap_tx.abort();
                    scratch.restore(self.native.as_mut());
                    return false;
                };
                for (slot, value) in writebacks {
                    self.set_reg(slot, value);
                }
                for (slot, value) in handle_liveouts {
                    self.set_reg(slot, value);
                }
                for (slot, value) in aggregate_liveouts {
                    self.set_reg(slot, value);
                }
                // native limit accounting mem: this is the CLEAN OSR exit (heap writes committed above), so
                // commit the `ListPush*` byte charges the native loop accumulated into the
                // interpreter's cumulative allocation total. (A bail takes the `_` arm below, which aborts the
                // heap tx and reruns on the interpreter — discarding the native charges.)
                if allocation_armed {
                    self.allocated_bytes = jit_allocation_cell_allocated_bytes();
                }
                let mut scalar_writebacks: Vec<(usize, Vec<(usize, i64)>)> = Vec::new();
                for field in scalar_fields.iter().filter(|field| field.writeback) {
                    let Some(value) = live.iter().find_map(|live_reg| {
                        (live_reg.reg as usize == field.native_reg).then_some({
                            match live_reg.value {
                                vm_jit::DeoptValue::Int(value) => Some(value),
                                vm_jit::DeoptValue::Bool(_) => None,
                                vm_jit::DeoptValue::Float(_) => None,
                                vm_jit::DeoptValue::Handle(_) => None,
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
                // actually wrote. Aggregate destinations were rebuilt above, while
                // their source Handle leaves were cloned before the heap transaction
                // commit preserved ownership.
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
                        vm_jit::DeoptValue::Bool(b) => VmValue::Bool(b),
                        vm_jit::DeoptValue::Float(f) => VmValue::Float(f),
                        // Handle regs are already skipped above via `reg_types`; this arm
                        // keeps the match exhaustive and never writes a raw index back.
                        vm_jit::DeoptValue::Handle(_) => continue,
                    };
                    self.set_reg(base + reg as usize, vm_value);
                }
                // Resume in the ORIGINAL `func.code`, at the ip-mapped post-loop ip.
                let frame = self.frames.last_mut().expect("active frame");
                frame.tail_calls = osr_committed_tail_calls(final_logical_depth, physical_depth);
                frame.ip = orig_exit;
                if let Some(native) = self.native.as_mut() {
                    if native.collect_stats {
                        native.stats.osr_entries += 1;
                        match selected_tier {
                            NativeCodeTier::Baseline => native.stats.baseline_calls += 1,
                            NativeCodeTier::Optimized => native.stats.optimized_calls += 1,
                        }
                    }
                    native
                        .osr_controllers
                        .entry(osr_version_key.clone())
                        .or_default()
                        .successful_entry();
                    // Lever 2 (observational): record this function actually OSR-
                    // entered, so the report's `osr: entered` positive matches the
                    // real outcome. Gated on `report`; no effect on any decision.
                    if native.report {
                        native.report_osr_ok.insert(func.ordinal);
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
                    let disabled = native
                        .osr_controllers
                        .entry(osr_version_key.clone())
                        .or_default()
                        .dynamic_bail(NATIVE_BAIL_GIVEUP_THRESHOLD);
                    if disabled {
                        native.osr_cache.insert(osr_version_key.clone(), None);
                        native.optimized_osr_cache.remove(&osr_version_key);
                        native.osr_optimization_sources.remove(&osr_version_key);
                    }
                    if native.collect_stats {
                        native.stats.shape_bails += 1;
                    }
                }
                scratch.restore(self.native.as_mut());
                false
            }
        }
    }
}


#[cfg(all(test, feature = "native-jit"))]
mod tests {
    use super::{osr_committed_tail_calls, osr_initial_logical_depth};
    use crate::reg_vm::{
        JitInstanceKey, MAX_JIT_TYPE_ARGUMENTS, NativeParamShape, ShapeKey, VerifiedCallSite,
        VerifiedCallTarget, VerifiedParamEffect, VerifiedStorageType, VerifiedTypeArgsKey, VmValue,
        native_param_shape_with_fact,
    };
    use rsscript_abi_model::WireType;

    #[test]
    fn osr_logical_depth_round_trips_accumulated_tail_calls() {
        let initial = osr_initial_logical_depth(3, 500);
        assert_eq!(initial, 503);
        assert_eq!(osr_committed_tail_calls(initial + 17, 3), 517);
    }

    #[test]
    fn verified_scalars_do_not_create_runtime_shape_versions() {
        assert_eq!(
            native_param_shape_with_fact(&VmValue::Int(1), VerifiedStorageType::Int),
            NativeParamShape::StaticScalar
        );
        assert_eq!(
            native_param_shape_with_fact(&VmValue::Bool(true), VerifiedStorageType::Bool),
            NativeParamShape::StaticScalar
        );
        assert_eq!(
            native_param_shape_with_fact(&VmValue::Float(1.0), VerifiedStorageType::Float),
            NativeParamShape::StaticScalar
        );
        assert_eq!(
            native_param_shape_with_fact(&VmValue::Int(1), VerifiedStorageType::Unknown),
            NativeParamShape::Int
        );
    }

    #[test]
    fn verified_type_arguments_are_a_static_instance_dimension() {
        let int = VerifiedTypeArgsKey::from_verified(&[WireType::Int {
            bits: 64,
            signed: true,
        }])
        .expect("one verifier-owned type argument is bounded");
        let float = VerifiedTypeArgsKey::from_verified(&[WireType::Float { bits: 64 }])
            .expect("one verifier-owned type argument is bounded");
        assert_ne!(int, float);
        assert!(int.is_known());
        assert_eq!(
            VerifiedTypeArgsKey::from_verified(&[]),
            Some(VerifiedTypeArgsKey::Unavailable),
            "empty v1 substitutions remain explicitly unavailable"
        );

        let int_instance = JitInstanceKey {
            function: 7,
            type_arguments: int,
        };
        let float_instance = JitInstanceKey {
            function: 7,
            type_arguments: float,
        };
        assert_ne!(int_instance, float_instance);
        assert_eq!(
            ShapeKey::from_shapes([NativeParamShape::StaticScalar]),
            ShapeKey::from_shapes([NativeParamShape::StaticScalar]),
            "static scalar payloads do not create runtime shape versions"
        );
        assert_eq!(
            native_param_shape_with_fact(&VmValue::Int(17), VerifiedStorageType::Int),
            NativeParamShape::StaticScalar,
            "changing a specialization identity cannot authorize storage or runtime shape"
        );
    }

    #[test]
    fn excessive_type_argument_vectors_fail_closed() {
        let arguments = vec![WireType::Bool; MAX_JIT_TYPE_ARGUMENTS + 1];
        assert_eq!(VerifiedTypeArgsKey::from_verified(&arguments), None);
    }

    #[test]
    fn ordered_and_duplicate_generic_instances_do_not_collide() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let float = WireType::Float { bits: 64 };
        let site = |arguments: Vec<WireType>| VerifiedCallSite {
            target: VerifiedCallTarget::Known(1),
            params: vec![VerifiedStorageType::Int, VerifiedStorageType::Float].into_boxed_slice(),
            result: VerifiedStorageType::Int,
            param_effects: vec![VerifiedParamEffect::Unknown; 2].into_boxed_slice(),
            type_arguments: arguments.into_boxed_slice(),
        };
        let ordered = JitInstanceKey::from_call_site(1, &site(vec![int.clone(), float.clone()]))
            .expect("ordered instance");
        let swapped = JitInstanceKey::from_call_site(1, &site(vec![float, int.clone()]))
            .expect("swapped instance");
        let duplicate = JitInstanceKey::from_call_site(1, &site(vec![int.clone(), int]))
            .expect("duplicate concrete arguments are valid and bounded");
        assert_ne!(ordered, swapped);
        assert_ne!(ordered, duplicate);
        assert_ne!(swapped, duplicate);
    }
}
