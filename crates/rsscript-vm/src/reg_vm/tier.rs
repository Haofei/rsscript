use super::*;

#[cfg(feature = "native-jit")]
mod admission;
#[cfg(feature = "native-jit")]
mod call_scratch;
#[cfg(feature = "native-jit")]
mod compile_result;
#[cfg(feature = "native-jit")]
mod deopt_resume;
mod jit_entry;
#[cfg(feature = "native-jit")]
mod osr_plan;
mod recursion;
mod state;

#[cfg(feature = "native-jit")]
use admission::*;
#[cfg(feature = "native-jit")]
use call_scratch::*;
#[cfg(all(test, feature = "native-jit"))]
#[allow(unused_imports)]
pub(in crate::reg_vm) use compile_result::NativeCompileTelemetry;
#[cfg(feature = "native-jit")]
use compile_result::{native_region_is_promotion_eligible, record_native_compile_stats};
#[cfg(feature = "native-jit")]
use osr_plan::*;
pub(crate) use state::JitState;

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
pub(in crate::reg_vm) fn native_scalar_callee_pending_on_branch_profile(
    jit_state: &JitState,
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
    has_branch && jit_state.branch_count(func) < PROFILE_WARMUP + PROFILE_BRANCH_MIN_SAMPLES
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
    jit_state: &JitState,
    native: &mut NativeState,
    unit: &RegUnit,
    callee: &RegFunction,
    callee_key: usize,
    stack: &mut std::collections::HashSet<usize>,
) -> Option<NativeCompiledCallee> {
    let version_key = NativeVersionKey {
        function: callee_key,
        shape: ShapeKey::default(),
    };
    if let Some(cached) = native.cache.get(&version_key) {
        return cached
            .as_ref()
            .and_then(native_compiled_entry_call_descriptor);
    }
    if native.whole_shape_count(callee_key) >= MAX_NATIVE_SHAPE_VERSIONS {
        return None;
    }
    if native_scalar_callee_pending_on_branch_profile(jit_state, callee) {
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
        translate_to_native_jit(unit, callee, profile, call_count)
    } else {
        translate_to_native_jit_with_compiled_callees(
            unit,
            callee,
            profile,
            call_count,
            &nested_call_sites,
        )
        .or_else(|| translate_to_native_jit(unit, callee, profile, call_count))
    };
    let Some((jit_fn, ret, params, string_literals, precise_resume_safe)) = translated else {
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
            None => native.baseline_module.compile(&jit_fn),
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
    native.cache.insert(version_key, Some(entry));
    if native.collect_stats {
        native.stats.shape_versions += 1;
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
        let callee_key = jit_state.function_ordinal(callee);
        if callee_key == self_key {
            continue;
        }
        let Some(descriptor) =
            native_compile_direct_scalar_callee(jit_state, native, unit, callee, callee_key, stack)
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

#[cfg(feature = "native-jit")]
#[allow(dead_code)]
pub(in crate::reg_vm) fn select_osr_candidate_loop(
    unit: &RegUnit,
    func: &RegFunction,
) -> Option<OsrLoop> {
    select_osr_candidate_loops(unit, func).into_iter().next()
}

impl RegVm {
    #[cfg(feature = "native-jit")]
    /// Try to run `func` on the native (Cranelift) tier. Returns
    /// [`NativeAttempt::Completed`] if the compiled code ran to completion;
    /// [`NativeAttempt::Resumed`] if a native bail was reconstructed into the
    /// interpreter at a safepoint (precise deopt, only under `precise_deopt`); or
    /// [`NativeAttempt::Fallback`] when the function isn't native-eligible, an
    /// argument isn't the inferred type, or the native code bailed and precise
    /// resume did not (or could not) apply — in all of which cases the caller
    /// re-runs the function from the top on the interpreter, which produces the
    /// exact value or error. Safe because native-eligible functions are leaf and
    /// side-effect-free, so re-running them is observationally identical.
    #[cfg(feature = "native-jit")]
    #[allow(clippy::wrong_self_convention)]
    pub(super) fn try_native(&mut self, func: &RegFunction, base: usize) -> NativeAttempt {
        // Host helpers may re-enter the VM, but the native heap tables, transaction,
        // literals, and deopt state are one top-level frame rather than a frame stack.
        // Preserve the outer call by interpreting the nested invocation.
        if JitCallCtx::is_active() {
            return NativeAttempt::Fallback;
        }
        // The current internal ABI carries only a host-stack cap, not the user's
        // logical frame limit. Custom max_depth therefore remains interpreter-only.
        if self.limits.max_depth != DEFAULT_MAX_DEPTH {
            return NativeAttempt::Fallback;
        }
        // Native limit parity (execution spec §6.2, Model A): Cranelift code polls
        // neither the step budget nor the cancel flag, so a hot, tiered-up function
        // containing an unbounded loop would run natively and bypass `step_budget`
        // / `cancel`. When either preemption limit is armed we refuse to dispatch
        // natively and fall back to the tier-0 executor (and interpreter), which
        // `tick()` on every instruction. `allocation_budget` is ALSO in this gate now that
        // the native subset can mutate/allocate heap (the native heap-helper contract write helpers): an
        // allocating native attempt runs off the meter and would not trip
        // `allocation_budget`, so a function could grow the accounted cumulative allocation total past the
        // limit without erroring. Refusing native while `allocation_budget` is armed keeps
        // the limit exact (Model A; matches the tier-0 self-recursive gate).
        if !self.native_limits_unarmed() {
            return NativeAttempt::Fallback;
        }
        // Cheap negative path: a function known not native-eligible never compiles,
        // so skip all per-call tiering/cache/name-hash work and fall straight back
        // to the interpreter (keeps `jit-native` from being slower than the VM on
        // code the native tier can't take).
        let native_status = self.jit_state.native_status(func);
        if native_status == NATIVE_STATUS_NOT_ELIGIBLE {
            return NativeAttempt::Fallback;
        }
        if native_status == NATIVE_STATUS_PROFILE_PENDING {
            if self.jit_state.call_count(func) < PROFILE_RECORD_LIMIT {
                return NativeAttempt::Fallback;
            }
            // The bounded profile is now immutable. Re-open translation once;
            // the result will become either a compiled cache entry or a stable
            // negative verdict.
            self.jit_state.set_native_status(func, 0);
        }
        // The unit is needed to resolve inlinable callees; clone the `Rc` so the
        // mutable `self.native` borrow below doesn't conflict.
        let unit = Rc::clone(&self.unit);
        let native_key = self.jit_state.function_ordinal(func);
        let profile = self.jit_state.profile(func);
        let call_count = self.jit_state.call_count(func);
        let shape = ShapeKey::from_values((0..func.params).map(|index| self.reg(base + index)));
        let version_key = NativeVersionKey {
            function: native_key,
            shape,
        };
        // Phase 1: tiering + resolve (and lazily compile) the native function.
        // `None` in the cache means "known not native-eligible".
        let (id, ret_type, param_types, string_literals, precise_resume_safe, selected_tier) = {
            let Some(native) = self.native.as_mut() else {
                return NativeAttempt::Fallback;
            };
            if native.force_bail {
                // Deopt stress mode: pretend the native code bailed at its first
                // guard, so the interpreter handles the function.
                return NativeAttempt::Fallback;
            }
            // Tiering: accumulate deterministic source-work, then stay on the
            // interpreter until the lower baseline threshold is crossed.
            let count = native.counts.entry(native_key).or_insert(0);
            *count = count.saturating_add(u64::from(interpreted_region_work(&func.code)));
            if *count <= u64::from(native.tier_up_threshold) {
                if native.collect_stats {
                    native.stats.tier_deferred += 1;
                }
                return NativeAttempt::Fallback;
            }
            if native.collect_stats {
                native.stats.considered += 1;
            }
            let entry = match native.cache.get(&version_key) {
                Some(entry) => {
                    if native.collect_stats {
                        native.stats.shape_cache_hits += 1;
                    }
                    entry.clone()
                }
                None => {
                    if native.whole_shape_count(native_key) >= MAX_NATIVE_SHAPE_VERSIONS {
                        if native.collect_stats {
                            native.stats.shape_limit_fallbacks += 1;
                        }
                        return NativeAttempt::Fallback;
                    }
                    let compiled_call_sites = native_compiled_call_sites(
                        &self.jit_state,
                        native,
                        &unit,
                        func,
                        native_key,
                    );
                    let translated = if compiled_call_sites.is_empty() {
                        translate_to_native_jit(&unit, func, profile, call_count)
                    } else {
                        translate_to_native_jit_with_compiled_callees(
                            &unit,
                            func,
                            profile,
                            call_count,
                            &compiled_call_sites,
                        )
                        .or_else(|| translate_to_native_jit(&unit, func, profile, call_count))
                    };
                    let entry = match translated {
                        Some((jit_fn, ret, params, string_literals, precise_resume_safe)) => {
                            if native.collect_stats {
                                native.stats.translated += 1;
                            }
                            let scalar_leaf_callable = vm_jit::is_native_callable_leaf(&jit_fn);
                            // Step 1 cost model (eligibility already proven by `translate`):
                            // in `enforce` mode, decline an unprofitable region and keep the
                            // function on the interpreter (cached below as not-native). `off`
                            // and `report` modes never change execution here.
                            let has_backedge = jit_function_has_loop(&func.code);
                            if consult_profitability(
                                native,
                                &jit_fn,
                                has_backedge,
                                "whole-fn",
                                &func.name,
                            ) {
                                // Profitability is a property of the translated
                                // function body, not of the runtime ABI shape.
                                // Keep the existing per-version `None` entry for
                                // diagnostics, but also enable the function-level
                                // negative fast path. Otherwise every future call
                                // still builds/hashes a ShapeKey only to rediscover
                                // the same decline (notably tiny closure dispatchers
                                // invoked from an interpreted loop).
                                self.jit_state
                                    .set_native_status(func, NATIVE_STATUS_NOT_ELIGIBLE);
                                None
                            } else {
                                let Some(admission) =
                                    begin_native_compile(native, 1, NativeCodeTier::Baseline)
                                else {
                                    native.cache.insert(version_key.clone(), None);
                                    return NativeAttempt::Fallback;
                                };
                                let compiled = if native.force_all_safepoints {
                                    native.baseline_module.compile_forcing_all_bails(&jit_fn)
                                } else {
                                    match native.forced_safepoint {
                                        Some(site) => native
                                            .baseline_module
                                            .compile_forcing_bail(&jit_fn, site),
                                        None => native.baseline_module.compile(&jit_fn),
                                    }
                                };
                                match compiled {
                                    Ok(id) => {
                                        if !finish_native_compile(
                                            native,
                                            admission,
                                            &[id],
                                            NativeCodeTier::Baseline,
                                        ) {
                                            native.cache.insert(version_key.clone(), None);
                                            return NativeAttempt::Fallback;
                                        }
                                        let verify_native =
                                            cfg!(debug_assertions) || jit_native_verify_is_strict();
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
                                                return NativeAttempt::Fallback;
                                            }
                                        }
                                        record_native_compile_stats(
                                            native,
                                            id,
                                            &jit_fn,
                                            NativeCodeTier::Baseline,
                                        );
                                        if native.optimized_module.is_some()
                                            && native_region_is_promotion_eligible(&jit_fn)
                                        {
                                            native
                                                .optimization_sources
                                                .insert(version_key.clone(), jit_fn.clone());
                                        }
                                        // `has_backedge` (hoisted above for the cost-model gate)
                                        // also drives `NATIVE_NOAMORTIZE_GIVEUP`: a loop-free body
                                        // dispatched per interpreter iteration never amortizes FFI.
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
                                    Err(err) => {
                                        finish_native_compile_failure(native, admission);
                                        if native.report {
                                            eprintln!(
                                                "jit-report: fn `{}` compile failed: {err}",
                                                func.name,
                                            );
                                        }
                                        if native.collect_stats {
                                            native.stats.compile_failed += 1;
                                        }
                                        None
                                    }
                                }
                            }
                        }
                        None => {
                            // profile-guided inlining: if translation failed *only* because a structurally
                            // inlinable `CallClosure` site hasn't yet warmed to a
                            // monomorphic decision, the verdict is NOT invariant —
                            // re-attempt on a later (warmer) call. Don't cache and
                            // don't mark NOT_ELIGIBLE; just fall back this once.
                            if native_translation_pending_on_profile(
                                &unit, func, profile, call_count,
                            ) {
                                if matches!(native.cost_model, NativeCostModel::Enforce) {
                                    self.jit_state
                                        .set_native_status(func, NATIVE_STATUS_PROFILE_PENDING);
                                }
                                return NativeAttempt::Fallback;
                            }
                            if native.collect_stats {
                                native.stats.not_eligible += 1;
                            }
                            // Invariant verdict — cache it on the function so future
                            // calls take the cheap negative path above.
                            self.jit_state
                                .set_native_status(func, NATIVE_STATUS_NOT_ELIGIBLE);
                            None
                        }
                    };
                    native.cache.insert(version_key.clone(), entry.clone());
                    if entry.is_some() && native.collect_stats {
                        native.stats.shape_versions += 1;
                    }
                    entry
                }
            };
            let mut selected_tier = NativeCodeTier::Baseline;
            let mut selected_entry = entry;
            if native.optimized_module.is_some() {
                let work = u64::from(interpreted_region_work(&func.code));
                let accumulated = native
                    .promotion_work
                    .entry(version_key.clone())
                    .or_insert(0);
                *accumulated = accumulated.saturating_add(work);
                if *accumulated >= native.optimize_work_threshold
                    && !native.optimized_cache.contains_key(&version_key)
                    && let Some(jit_fn) = native.optimization_sources.remove(&version_key)
                    && let Some(admission) =
                        begin_native_compile(native, 1, NativeCodeTier::Optimized)
                {
                    let compiled = if native.force_all_safepoints {
                        native
                            .optimized_module
                            .as_mut()
                            .expect("optimized module")
                            .compile_forcing_all_bails(&jit_fn)
                    } else {
                        match native.forced_safepoint {
                            Some(site) => native
                                .optimized_module
                                .as_mut()
                                .expect("optimized module")
                                .compile_forcing_bail(&jit_fn, site),
                            None => native
                                .optimized_module
                                .as_mut()
                                .expect("optimized module")
                                .compile(&jit_fn),
                        }
                    };
                    match compiled {
                        Ok(optimized_id) => {
                            if finish_native_compile(
                                native,
                                admission,
                                &[optimized_id],
                                NativeCodeTier::Optimized,
                            ) {
                                let verify_native =
                                    cfg!(debug_assertions) || jit_native_verify_is_strict();
                                let verified = !verify_native
                                    || jit_verify_compiled_native(
                                        native.optimized_module.as_ref().expect("optimized module"),
                                        optimized_id,
                                        &jit_fn,
                                        native.forced_safepoint,
                                    )
                                    .is_ok();
                                if verified || !jit_native_verify_is_strict() {
                                    record_native_compile_stats(
                                        native,
                                        optimized_id,
                                        &jit_fn,
                                        NativeCodeTier::Optimized,
                                    );
                                    if let Some(mut promoted) = selected_entry.clone() {
                                        promoted.0 = optimized_id;
                                        native
                                            .optimized_cache
                                            .insert(version_key.clone(), promoted);
                                        if native.collect_stats {
                                            native.stats.promotions += 1;
                                        }
                                    }
                                }
                            }
                        }
                        Err(_) => finish_native_compile_failure(native, admission),
                    }
                }
                if let Some(promoted) = native.optimized_cache.get(&version_key) {
                    selected_tier = NativeCodeTier::Optimized;
                    selected_entry = Some(promoted.clone());
                }
            }
            let (
                id,
                ret,
                params,
                has_backedge,
                _scalar_leaf_callable,
                string_literals,
                precise_resume_safe,
            ) = match selected_entry {
                Some(entry) => entry,
                None => return NativeAttempt::Fallback,
            };
            // No-amortization profitability gate. A loop-free body does O(1) work
            // per dispatch; dispatched once per interpreter loop iteration it pays
            // FFI + marshalling cost it can never amortize. After
            // `NATIVE_NOAMORTIZE_GIVEUP` such dispatches, negative-cache this shape
            // so the remainder of the loop takes the cheap interpreter fallback.
            // Loop-bearing bodies (`has_backedge`) do O(n) work
            // per dispatch, amortize the cost, and are never counted here — they are
            // dispatched `calls=1` (the whole loop compiled into one native body) and
            // so could never reach `K` anyway. This is the same predict-and-skip
            // pattern as the bail give-up, not a parallel system.
            if !has_backedge {
                let count = native
                    .noamortize_counts
                    .entry(version_key.clone())
                    .or_insert(0);
                *count += 1;
                if *count >= NATIVE_NOAMORTIZE_GIVEUP {
                    native.cache.insert(version_key.clone(), None);
                    native.optimized_cache.remove(&version_key);
                    native.optimization_sources.remove(&version_key);
                    native.noamortize_counts.remove(&version_key);
                    // `has_backedge` and the bounded O(1) body cost are
                    // function properties. Once repeated dispatch proves that
                    // native entry cannot amortize for one ABI shape, avoid
                    // rebuilding/hashing shape keys for every later call.
                    self.jit_state
                        .set_native_status(func, NATIVE_STATUS_NOT_ELIGIBLE);
                    return NativeAttempt::Fallback;
                }
            }
            (
                id,
                ret,
                params,
                string_literals,
                precise_resume_safe,
                selected_tier,
            )
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
                native.record_bail(&version_key);
            }
        };
        for (index, param_type) in param_types.iter().copied().enumerate().take(n) {
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
        ) || ((!scratch.flat_owned.is_empty())
            && func.code.iter().any(|instr| {
                native_instruction_has_heap_write(instr)
                    || matches!(instr, RegInstr::CallKnown { .. })
            })
            && jit_heap_inputs_alias_flat_mut(&scratch.heap_input_slots, &scratch.flat_owned));
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
        // functions are side-effect-free (the transactional fallback contract), so they never even attempt a write;
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
        let mut flat_args = Vec::with_capacity(flat_guards.len() + flat_mut_guards.len());
        {
            let mut flat_iter = flat_guards.iter();
            let mut flat_mut_iter = flat_mut_guards.iter_mut();
            for (index, param_type) in param_types.iter().copied().enumerate().take(n) {
                match param_type {
                    NativeTy::FlatInt => {
                        let slice = flat_iter
                            .next()
                            .and_then(|v| v.as_ints_slice())
                            .expect("Ints pinned");
                        scratch.args[index] = slice.as_ptr() as i64;
                        scratch.lens[index] = slice.len() as i64;
                        flat_args.push(vm_jit::FlatBufferArg::Int(slice));
                    }
                    NativeTy::FlatIntMut => {
                        let slice = flat_mut_iter
                            .next()
                            .and_then(|v| v.as_ints_mut_slice())
                            .expect("Ints mut pinned");
                        scratch.args[index] = slice.as_mut_ptr() as i64;
                        scratch.lens[index] = slice.len() as i64;
                        flat_args.push(vm_jit::FlatBufferArg::IntMut(slice));
                    }
                    NativeTy::FlatFloat => {
                        let slice = flat_iter
                            .next()
                            .and_then(|v| v.as_floats_slice())
                            .expect("Floats pinned");
                        scratch.args[index] = slice.as_ptr() as i64;
                        scratch.lens[index] = slice.len() as i64;
                        flat_args.push(vm_jit::FlatBufferArg::Float(slice));
                    }
                    NativeTy::FlatFloatMut => {
                        let slice = flat_mut_iter
                            .next()
                            .and_then(|v| v.as_floats_mut_slice())
                            .expect("Floats mut pinned");
                        scratch.args[index] = slice.as_mut_ptr() as i64;
                        scratch.lens[index] = slice.len() as i64;
                        flat_args.push(vm_jit::FlatBufferArg::FloatMut(slice));
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
        let initial_depth = self.frames.len();
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
            let module = match selected_tier {
                NativeCodeTier::Baseline => &native_ref.baseline_module,
                NativeCodeTier::Optimized => native_ref
                    .optimized_module
                    .as_ref()
                    .expect("optimized dispatch requires optimized module"),
            };
            let result = module.call_with_host_ctx_at_depth(
                id,
                &scratch.args,
                &scratch.lens,
                heap_tx.host_ctx(),
                &mut flat_args,
                vm_jit::LogicalCallDepth {
                    current: initial_depth,
                    limit: self.limits.max_depth,
                },
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
                        native.record_bail(&version_key);
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
                        match selected_tier {
                            NativeCodeTier::Baseline => native.stats.baseline_calls += 1,
                            NativeCodeTier::Optimized => native.stats.optimized_calls += 1,
                        }
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
                    native.bail_counts.insert(version_key.clone(), 0);
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
                                match selected_tier {
                                    NativeCodeTier::Baseline => native.stats.baseline_calls += 1,
                                    NativeCodeTier::Optimized => native.stats.optimized_calls += 1,
                                }
                            }
                            if native.report {
                                native.report_native_ok.insert(native_key);
                            }
                            native.bail_counts.insert(version_key.clone(), 0);
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
                            native.record_bail(&version_key);
                        }
                        scratch.restore(self.native.as_mut());
                        NativeAttempt::Fallback
                    }
                }
            }
            // Whole-function entries never contain RegionExit. Treat a mismatched
            // compiled handle as a fail-closed fallback rather than committing a
            // transaction under continuation semantics.
            vm_jit::NativeOutcome::Yield { .. } => {
                heap_tx.abort();
                if let Some(native) = self.native.as_mut() {
                    native.record_bail(&version_key);
                }
                scratch.restore(self.native.as_mut());
                NativeAttempt::Fallback
            }
            vm_jit::NativeOutcome::Deopt {
                safepoint_id,
                live,
                child,
                ..
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
                    native.record_bail(&version_key);
                    native.precise_deopt
                } else {
                    false
                };
                // precise resume: take it ONLY when the flag is on AND this is
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
                        .and_then(|native| match selected_tier {
                            NativeCodeTier::Baseline => native.baseline_module.deopt_map(id),
                            NativeCodeTier::Optimized => native
                                .optimized_module
                                .as_ref()
                                .and_then(|module| module.deopt_map(id)),
                        })
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
                                heap_tx.host_ctx(),
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
                        if !self.restore_native_deopt_live_regs(
                            base,
                            func.regs,
                            &live,
                            heap_tx.host_ctx(),
                        ) {
                            scratch.restore(self.native.as_mut());
                            return NativeAttempt::Fallback;
                        }
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
    pub(super) fn resolve_osr_candidates(&mut self, func: &RegFunction) -> OsrCandidates {
        let function = self.jit_state.function_ordinal(func);
        if let Some(candidates) = self
            .native
            .as_ref()
            .and_then(|native| native.osr_candidates.get(&function))
        {
            return *candidates;
        }

        // Resource gates remain in `try_osr`: candidacy cannot determine transformed
        // allocation effects, and eager tests must use the same candidate ordering.
        let loops = select_osr_candidate_loops(&self.unit, func);
        let mut candidates = OsrCandidates::default();
        for (slot, lp) in candidates.entries.iter_mut().zip(loops) {
            let iteration_work = func
                .code
                .get(lp.header..lp.exit)
                .map(interpreted_region_work)
                .unwrap_or(1);
            *slot = Some(OsrCandidate {
                header_ip: lp.header,
                iteration_work,
            });
        }
        if let Some(native) = self.native.as_mut() {
            for candidate in candidates.iter() {
                native.osr_triggers.insert(
                    RegionKey {
                        function,
                        header: candidate.header_ip,
                    },
                    OsrTrigger::Counting {
                        count: 0,
                        probe_cc: 0,
                    },
                );
            }
            native.osr_candidates.insert(function, candidates);
        }
        candidates
    }

    #[cfg(feature = "native-jit")]
    #[allow(dead_code)]
    pub(super) fn resolve_osr_candidate(&mut self, func: &RegFunction) -> Option<usize> {
        self.resolve_osr_candidates(func).first_header()
    }

    /// Run one conservative scalar continuation region beginning at the current
    /// interpreter IP. The region commits only register-local scalar work and
    /// yields before the VM-owned barrier (`CallKnown` or `Return`).
    #[cfg(feature = "native-jit")]
    pub(super) fn try_continuation_region(
        &mut self,
        func: &RegFunction,
        base: usize,
        entry_ip: usize,
    ) -> bool {
        if JitCallCtx::is_active()
            || !self.native_limits_unarmed()
            || self.limits.max_depth != DEFAULT_MAX_DEPTH
        {
            return false;
        }
        let function = self.jit_state.function_ordinal(func);
        let region = {
            let Some(native) = self.native.as_mut() else {
                return false;
            };
            native
                .continuation_plans
                .entry((function, entry_ip))
                .or_insert_with(|| detect_scalar_continuation_region(&func.code, entry_ip))
                .clone()
        };
        let Some(region) = region else {
            return false;
        };

        // The first slice is scalar-only. Reject any initialized non-scalar frame
        // slot before translation/caching; later region slices can add heap table
        // and transaction support without weakening this boundary.
        let region_active_regs = {
            let mut active = vec![false; func.regs];
            for (ip, instr) in func.code.iter().enumerate() {
                if !region.included[ip] {
                    continue;
                }
                let Some(regs) = native_continuation_registers(instr) else {
                    return false;
                };
                for reg in regs {
                    let Some(slot) = active.get_mut(reg) else {
                        return false;
                    };
                    *slot = true;
                }
            }
            active
        };
        let shape = ShapeKey::from_shapes((0..func.regs).map(|reg| {
            let slot = base + reg;
            if !region_active_regs[reg] || !self.written.get(slot).copied().unwrap_or(false) {
                NativeParamShape::Unsupported
            } else {
                native_param_shape(&self.stack[slot])
            }
        }));
        let scalar_frame = (0..func.regs).all(|reg| {
            let slot = base + reg;
            !region_active_regs[reg]
                || !self.written.get(slot).copied().unwrap_or(false)
                || matches!(
                    self.stack.get(slot),
                    Some(VmValue::Int(_) | VmValue::Bool(_) | VmValue::Float(_))
                )
        });
        if !scalar_frame {
            return false;
        }
        let version_key = ContinuationVersionKey {
            function,
            entry: entry_ip,
            shape,
        };
        let param_native_types: Vec<Option<NativeTy>> = (0..func.params)
            .map(|reg| match self.reg(base + reg) {
                VmValue::Int(_) => Some(NativeTy::Int),
                VmValue::Bool(_) => Some(NativeTy::Bool),
                VmValue::Float(_) => Some(NativeTy::Float),
                _ => None,
            })
            .collect();

        let entry = {
            let Some(native) = self.native.as_mut() else {
                return false;
            };
            if !native.continuation_cache.contains_key(&version_key) {
                let versions = native
                    .continuation_cache
                    .keys()
                    .filter(|key| key.function == function && key.entry == entry_ip)
                    .count();
                if versions >= MAX_NATIVE_SHAPE_VERSIONS {
                    if native.collect_stats {
                        native.stats.shape_limit_fallbacks += 1;
                    }
                    return false;
                }
                let compiled =
                    translate_scalar_continuation_region(func, &region, &param_native_types)
                        .and_then(|(jit_fn, active_regs, reg_types, written_regs)| {
                            let admission =
                                begin_native_compile(native, 1, NativeCodeTier::Baseline)?;
                            match native.baseline_module.compile_osr(
                                &jit_fn,
                                u32::try_from(region.entry).ok()?,
                                false,
                                false,
                            ) {
                                Ok(id) => {
                                    if !finish_native_compile(
                                        native,
                                        admission,
                                        &[id],
                                        NativeCodeTier::Baseline,
                                    ) {
                                        return None;
                                    }
                                    record_native_compile_stats(
                                        native,
                                        id,
                                        &jit_fn,
                                        NativeCodeTier::Baseline,
                                    );
                                    if native.collect_stats {
                                        native.stats.shape_versions += 1;
                                    }
                                    Some(ContinuationEntry {
                                        id,
                                        entry: region.entry,
                                        exits: region.exits.clone(),
                                        n_jit_regs: jit_fn.n_regs as usize,
                                        active_regs,
                                        reg_types,
                                        written_regs,
                                    })
                                }
                                Err(_) => {
                                    finish_native_compile_failure(native, admission);
                                    None
                                }
                            }
                        });
                native
                    .continuation_cache
                    .insert(version_key.clone(), compiled);
            } else if native.collect_stats {
                native.stats.shape_cache_hits += 1;
            }
            native
                .continuation_cache
                .get(&version_key)
                .and_then(Clone::clone)
        };
        let Some(entry) = entry else {
            return false;
        };
        debug_assert_eq!(entry.entry, entry_ip);

        let mut window = vec![0i64; entry.n_jit_regs];
        let lens = vec![0i64; entry.n_jit_regs];
        for (reg, ty) in entry.reg_types.iter().copied().enumerate() {
            if !entry.active_regs.get(reg).copied().unwrap_or(false) {
                continue;
            }
            let slot = base + reg;
            if !self.written.get(slot).copied().unwrap_or(false) {
                continue;
            }
            window[reg] = match (ty, self.stack.get(slot)) {
                (NativeTy::Int, Some(VmValue::Int(value))) => *value,
                (NativeTy::Bool, Some(VmValue::Bool(value))) => i64::from(*value),
                (NativeTy::Float, Some(VmValue::Float(value))) => value.to_bits() as i64,
                _ => return false,
            };
        }

        let started = self
            .native
            .as_ref()
            .is_some_and(|native| native.collect_stats)
            .then(std::time::Instant::now);
        let result = self
            .native
            .as_ref()
            .map(|native| native.baseline_module.call(entry.id, &window, &lens));
        if let Some(native) = self.native.as_mut()
            && let Some(started) = started
        {
            native.stats.run_nanos = native
                .stats
                .run_nanos
                .saturating_add(started.elapsed().as_nanos());
        }
        let Some(vm_jit::NativeOutcome::Yield { exit_id, live, .. }) = result else {
            return false;
        };
        let exit = exit_id as usize;
        let Some(barrier) = entry.exits.get(&exit).copied() else {
            return false;
        };

        let mut updates = Vec::new();
        for vm_jit::DeoptReg { reg, value } in live {
            let reg = reg as usize;
            if reg >= func.regs || !entry.written_regs.get(reg).copied().unwrap_or(false) {
                continue;
            }
            let value = match (entry.reg_types.get(reg), value) {
                (Some(NativeTy::Int), vm_jit::DeoptValue::Int(value)) => VmValue::Int(value),
                (Some(NativeTy::Bool), vm_jit::DeoptValue::Bool(value)) => VmValue::Bool(value),
                (Some(NativeTy::Float), vm_jit::DeoptValue::Float(value)) => VmValue::Float(value),
                _ => return false,
            };
            updates.push((base + reg, value));
        }
        for (slot, value) in updates {
            self.set_reg(slot, value);
        }
        self.frames.last_mut().expect("active frame").ip = exit;
        if let Some(native) = self.native.as_mut()
            && native.collect_stats
        {
            native.stats.continuation_entries += 1;
            native.stats.continuation_yields += 1;
            native.stats.baseline_calls += 1;
            *native
                .stats
                .native_barrier_counts
                .entry(barrier.as_str().to_string())
                .or_default() += 1;
        }
        true
    }

    #[cfg(feature = "native-jit")]
    pub(super) fn has_continuation_region(&mut self, func: &RegFunction) -> bool {
        let function = self.jit_state.function_ordinal(func);
        let Some(native) = self.native.as_mut() else {
            return false;
        };
        if let Some(&has_region) = native.continuation_functions.get(&function) {
            return has_region;
        }
        let has_region = (0..func.code.len()).any(|entry| {
            native
                .continuation_plans
                .entry((function, entry))
                .or_insert_with(|| detect_scalar_continuation_region(&func.code, entry))
                .is_some()
        });
        native.continuation_functions.insert(function, has_region);
        has_region
    }

    #[cfg(feature = "native-jit")]
    pub(super) fn try_osr(&mut self, func: &RegFunction, base: usize, header_ip: usize) -> bool {
        if JitCallCtx::is_active() {
            return false;
        }
        // Until transformed instructions carry source step, host-call, and
        // allocation costs, no armed resource mode is allowed through OSR. The
        // interpreter remains the semantic authority. Cancellation also stays on
        // that path even though vm-jit's raw cancel load is now atomic.
        if !osr_execution_controls_unarmed(&self.limits) {
            return false;
        }
        // These values remain false after the gate above. Keeping the compile-time
        // plumbing intact allows a future source-cost implementation to re-enable
        // proven limit-aware OSR without changing the cache shape again.
        let emit_step = self.limits.step_budget.is_some();
        let emit_cancel = self.limits.cancel.is_some();
        let allocation_armed = self.limits.allocation_budget.is_some();
        if let Some(native) = self.native.as_mut() {
            native.osr_dynamic_bail = false;
        }
        let native_key = self.jit_state.function_ordinal(func);
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
            let shape = native_param_shape(&self.stack[slot]);
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
            materialize_recipes,
            selected_tier,
        ) = {
            // Fast path: cached and NOT at the header ⇒ nothing to do (no clone).
            if let Some(native) = self.native.as_ref()
                && let Some(entry) = native.osr_cache.get(&osr_version_key)
            {
                match entry {
                    Some(e) if e.orig_header == header_ip => {}
                    _ => return false,
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
            // OSR × scalar replacement: detect the loop on the ORIGINAL code (detection understands
            // `MatchOption` as a two-way branch, so an Option-bearing body is shaped
            // correctly), then scalar-replace any non-escaping scalar `Option` that
            // lives ENTIRELY inside that loop region — turning the alloc-bound body
            // into native-subset code. Re-detect on the TRANSFORMED stream (where the
            // Option ops are gone) and compile. The transformed→original ip-map
            // translates the OSR boundary back to `func.code` (where the interpreter
            // resumes). When the body has no replaceable Option the region pass
            // returns the code unchanged with an identity ip-map, so plain
            // native-subset OSR is byte-for-byte the old path.
            if !native.osr_cache.contains_key(&osr_version_key) {
                if native.osr_shape_count(region_key) >= MAX_NATIVE_SHAPE_VERSIONS {
                    if native.collect_stats {
                        native.stats.shape_limit_fallbacks += 1;
                    }
                    return false;
                }
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
                // OSR × scalar replacement combinator expansion (deopt-before-heap, Slice 2): BEFORE
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
                            profile,
                            &func.code,
                            func.regs,
                            func.params,
                            func.captures,
                            lp,
                            &identity_ip_map,
                            &param_native_types,
                            &immutable_leaf_params,
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
                                // native limit accounting mem: a `ListPush*` flat-capacity growth now charges
                                // `allocation_budget` in its host helper (the only native-subset op
                                // the interpreter bills), so an allocating loop runs natively
                                // under an armed budget and bails to the interpreter at the
                                // exact over-budget push — no blanket decline needed.
                                // Step 1 cost model: an OSR loop is always a back-edge region;
                                // in `enforce` mode decline an unprofitable loop and resume on
                                // the interpreter (correctness-safe).
                                if consult_profitability(native, &jit_fn, true, "osr", &func.name) {
                                    return None;
                                }
                                let heap_input_regs = osr_heap_input_regs(&jit_fn);
                                let admission =
                                    begin_native_compile(native, 1, NativeCodeTier::Baseline)?;
                                match native.baseline_module.compile_osr(
                                    &jit_fn,
                                    lp.header as u32,
                                    emit_step,
                                    emit_cancel,
                                ) {
                                    Ok(id) => {
                                        if !finish_native_compile(
                                            native,
                                            admission,
                                            &[id],
                                            NativeCodeTier::Baseline,
                                        ) {
                                            return None;
                                        }
                                        let verify_native =
                                            cfg!(debug_assertions) || jit_native_verify_is_strict();
                                        if verify_native
                                            && let Err(err) = jit_verify_compiled_osr(
                                                &native.baseline_module,
                                                id,
                                                &jit_fn,
                                                lp.exit,
                                            )
                                        {
                                            debug_assert!(
                                                false,
                                                "native OSR verifier failed: {err}"
                                            );
                                            if jit_native_verify_is_strict() {
                                                return None;
                                            }
                                        }
                                        record_native_compile_stats(
                                            native,
                                            id,
                                            &jit_fn,
                                            NativeCodeTier::Baseline,
                                        );
                                        if native.optimized_module.is_some()
                                            && native_region_is_promotion_eligible(&jit_fn)
                                        {
                                            native.osr_optimization_sources.insert(
                                                osr_version_key.clone(),
                                                OsrOptimizationSource {
                                                    jit_fn: jit_fn.clone(),
                                                    header: lp.header as u32,
                                                    exit: lp.exit,
                                                },
                                            );
                                        }
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
                                            materialize_recipes: Vec::new(),
                                        })
                                    }
                                    Err(_) => {
                                        finish_native_compile_failure(native, admission);
                                        None
                                    }
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
                native_inline_leaf_calls(
                    &unit,
                    eff_func,
                    if eff_owned.is_some() { None } else { profile },
                    call_count,
                    true,
                    Some((lp_orig.header, lp_orig.exit)),
                ).and_then(
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
                    // OSR × scalar replacement for STRING LENGTH-LAW FOLDING: BEFORE the Result/Option/
                    // variant/struct passes, dissolve any non-escaping string built ONLY
                    // to be measured (`String.len` of `concat`/`slice`/`from_int`/literal/
                    // `Move`). Each `String.len` folds to arithmetic on operand byte
                    // lengths (verified laws — byte len, additive concat, ASCII slice
                    // clamp, `from_int` sign/zero/`i64::MIN` digit count) and the now-dead
                    // string allocations are DELETED — read-only (no heap write; Exec Spec
                    // the transactional fallback contract holds), turning a length-only string loop into pure-scalar Int
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
                    // OSR × scalar replacement for BYTES LENGTH-LAW FOLDING (read-only sibling of the
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
                    // OSR × scalar replacement for LIST FULL-SLICE QUERY FOLDING: dissolve a
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
                    // OSR × scalar replacement for RESULTS (deopt-before-heap, Slice 1): scalar-replace
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
                        // OSR × scalar replacement for OPTIONS: dissolve any non-escaping scalar Option
                        // living entirely inside the region. After Result-SR the region
                        // carries only Option ops + native subset, so the strict
                        // subset-or-option gate is satisfied. Identity (no Option) ⇒
                        // unchanged.
                        let lp1 = mapped_osr_loop(&code_r, &ip_map_r, lp_r.header)?;
                        let (code1, n_regs1, ip_map1, option_recipes1) =
                            native_scalar_replace_options_in_region(
                                &code_r, n_regs_r, lp1.header, lp1.exit,
                            )?;
                        // OSR × scalar replacement for VARIANTS: after dissolving Options/Results, re-detect
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
                        let (code2, n_regs2, ip_map2, variant_recipes2) =
                            native_scalar_replace_variants_in_region(
                                &code1, n_regs1, lp_v.header, lp_v.exit,
                            )?;
                        // OSR × scalar replacement for STRUCTS: after dissolving Options and variants,
                        // re-detect the loop on the transformed stream and scalar-replace
                        // any non-escaping flat user struct living entirely inside that
                        // region (`MakeStruct`/`GetFieldSlot` → per-slot `Move`). When
                        // there is no replaceable struct the pass returns the code
                        // unchanged with an identity ip-map, so an Option/variant-only (or
                        // plain) body is byte-for-byte the old path. Compose all three
                        // transformed→original ip-maps:
                        // `ip_map[t] = ip_map1[ip_map2[ip_map3[t]]]`.
                        let lp_s = mapped_osr_loop(&code2, &ip_map2, lp_v.header)?;
                        let (code_s, n_regs_s, ip_map3, struct_recipes3) =
                            native_scalar_replace_structs_in_region(
                                &code2, n_regs2, lp_s.header, lp_s.exit,
                            )?;
                        // OSR × scalar replacement for LOOP-CARRIED STRUCTS: after the loop-LOCAL
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
                                profile,
                                &code,
                                n_regs,
                                eff_func.params,
                                eff_func.captures,
                                lp,
                                &real_ip_map,
                                &param_native_types,
                                &immutable_leaf_params,
                            )
                                .and_then(|(jit_fn, params, derived_liveins, scalar_fields, reg_types, written_regs, string_literals)| {
                                    let n_jit_regs = jit_fn.n_regs as usize;
                                    // native limit accounting mem: `ListPush*` now charges `allocation_budget` in its
                                    // helper (the only native-subset billed op), so an
                                    // allocating loop runs natively and bails at the exact
                                    // over-budget push — no blanket decline needed.
                                    // Step 1 cost model: an OSR loop is always a back-edge
                                    // region; in `enforce` mode decline an unprofitable loop
                                    // and resume on the interpreter (correctness-safe).
                                    if consult_profitability(native, &jit_fn, true, "osr", &func.name) {
                                        return None;
                                    }
                                    let heap_input_regs = osr_heap_input_regs(&jit_fn);
                                    let admission = begin_native_compile(
                                        native,
                                        1,
                                        NativeCodeTier::Baseline,
                                    )?;
                                    match native.baseline_module.compile_osr(&jit_fn, lp.header as u32, emit_step, emit_cancel) {
                                        Ok(id) => {
                                            if !finish_native_compile(
                                                native,
                                                admission,
                                                &[id],
                                                NativeCodeTier::Baseline,
                                            ) {
                                                return None;
                                            }
                                            let verify_native =
                                                cfg!(debug_assertions) || jit_native_verify_is_strict();
                                            if verify_native
                                                && let Err(err) = jit_verify_compiled_osr(
                                                    &native.baseline_module,
                                                    id,
                                                    &jit_fn,
                                                    lp.exit,
                                                ) {
                                                    debug_assert!(false, "native OSR verifier failed: {err}");
                                                    if jit_native_verify_is_strict() {
                                                        return None;
                                                    }
                                                }
                                            record_native_compile_stats(
                                                native,
                                                id,
                                                &jit_fn,
                                                NativeCodeTier::Baseline,
                                            );
                                            if native.optimized_module.is_some()
                                                && native_region_is_promotion_eligible(&jit_fn)
                                            {
                                                native.osr_optimization_sources.insert(
                                                    osr_version_key.clone(),
                                                    OsrOptimizationSource {
                                                        jit_fn: jit_fn.clone(),
                                                        header: lp.header as u32,
                                                        exit: lp.exit,
                                                    },
                                                );
                                            }
                                            let mut materialize_recipes = option_recipes1;
                                            materialize_recipes.extend(variant_recipes2);
                                            materialize_recipes.extend(struct_recipes3);
                                            for (
                                                dst_reg,
                                                ok_payload,
                                                err_payload,
                                                tag_reg,
                                            ) in recipes_r
                                            {
                                                let mut arms = vec![
                                                    OsrMaterializeVariantArm {
                                                        tag: 1,
                                                        layout: result_ok_layout(),
                                                        fields: vec![
                                                            OsrMaterializeValue::Register(
                                                                ok_payload,
                                                            ),
                                                        ],
                                                    },
                                                ];
                                                if tag_reg.is_some() {
                                                    arms.push(OsrMaterializeVariantArm {
                                                        tag: 0,
                                                        layout: result_err_layout(),
                                                        fields: vec![
                                                            OsrMaterializeValue::Register(
                                                                err_payload,
                                                            ),
                                                        ],
                                                    });
                                                }
                                                materialize_recipes.push(
                                                    OsrMaterializeRecipe {
                                                        dst_reg,
                                                        value:
                                                            OsrMaterializeValue::Variant {
                                                                tag_reg,
                                                                arms,
                                                            },
                                                    },
                                                );
                                            }
                                            for recipe in &materialize_recipes {
                                                let mut nodes = 0;
                                                if !osr_materialize_recipe_is_supported(
                                                    &recipe.value,
                                                    &reg_types,
                                                    0,
                                                    &mut nodes,
                                                ) {
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
                                                materialize_recipes,
                                            })
                                        }
                                        Err(_) => {
                                            finish_native_compile_failure(native, admission);
                                            None
                                        },
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
                // OSR × profile-guided inlining: a capturing/monomorphic closure inline is profile-
                // guided, so on the first header hit (cold profile) the inline gate
                // declines and `entry` is `None`. Caching that permanently would
                // disable OSR forever — exactly the `try_native` warmup hazard. If a
                // closure-inline site is still PENDING on its profile, leave the
                // cache unpopulated so a later (warmer) header hit retries; once the
                // profile settles (or there is no pending site) the `None`/`Some`
                // verdict is stable and we cache it.
                if entry.is_some()
                    || !native_translation_pending_on_profile(
                        &unit,
                        func,
                        self.jit_state.profile(func),
                        self.jit_state.call_count(func),
                    )
                {
                    if entry.is_some() && native.collect_stats {
                        native.stats.shape_versions += 1;
                    }
                    native.osr_cache.insert(osr_version_key.clone(), entry);
                }
            } else if native.collect_stats {
                native.stats.shape_cache_hits += 1;
            }
            if native.optimized_module.is_some() {
                let iteration_work = native
                    .osr_candidates
                    .get(&native_key)
                    .and_then(|candidates| {
                        candidates
                            .iter()
                            .find(|candidate| candidate.header_ip == header_ip)
                    })
                    .map_or(1, |candidate| candidate.iteration_work);
                let trigger_work = match native.osr_triggers.get(&region_key) {
                    Some(OsrTrigger::Counting { count, .. }) => u64::from(*count),
                    _ => 0,
                };
                let accumulated = match native.osr_promotion_work.entry(osr_version_key.clone()) {
                    std::collections::hash_map::Entry::Vacant(entry) => {
                        entry.insert(trigger_work.max(u64::from(iteration_work)))
                    }
                    std::collections::hash_map::Entry::Occupied(entry) => {
                        let accumulated = entry.into_mut();
                        *accumulated = accumulated.saturating_add(u64::from(iteration_work));
                        accumulated
                    }
                };
                if *accumulated >= native.optimize_work_threshold
                    && !native.optimized_osr_cache.contains_key(&osr_version_key)
                    && let Some(source) = native.osr_optimization_sources.remove(&osr_version_key)
                    && let Some(admission) =
                        begin_native_compile(native, 1, NativeCodeTier::Optimized)
                {
                    let compiled = native
                        .optimized_module
                        .as_mut()
                        .expect("optimized module")
                        .compile_osr(&source.jit_fn, source.header, emit_step, emit_cancel);
                    match compiled {
                        Ok(optimized_id) => {
                            if finish_native_compile(
                                native,
                                admission,
                                &[optimized_id],
                                NativeCodeTier::Optimized,
                            ) {
                                let verify_native =
                                    cfg!(debug_assertions) || jit_native_verify_is_strict();
                                let verified = !verify_native
                                    || jit_verify_compiled_osr(
                                        native.optimized_module.as_ref().expect("optimized module"),
                                        optimized_id,
                                        &source.jit_fn,
                                        source.exit,
                                    )
                                    .is_ok();
                                if verified || !jit_native_verify_is_strict() {
                                    record_native_compile_stats(
                                        native,
                                        optimized_id,
                                        &source.jit_fn,
                                        NativeCodeTier::Optimized,
                                    );
                                    if let Some(Some(baseline)) =
                                        native.osr_cache.get(&osr_version_key)
                                    {
                                        let mut promoted = baseline.clone();
                                        promoted.id = optimized_id;
                                        native
                                            .optimized_osr_cache
                                            .insert(osr_version_key.clone(), promoted);
                                        if native.collect_stats {
                                            native.stats.promotions += 1;
                                        }
                                    }
                                }
                            }
                        }
                        Err(_) => finish_native_compile_failure(native, admission),
                    }
                }
            }
            let (entry, selected_tier) =
                if let Some(entry) = native.optimized_osr_cache.get(&osr_version_key) {
                    (Some(entry), NativeCodeTier::Optimized)
                } else {
                    (
                        native
                            .osr_cache
                            .get(&osr_version_key)
                            .and_then(Option::as_ref),
                        NativeCodeTier::Baseline,
                    )
                };
            match entry {
                // Only OSR when the interpreter is *at* the cached loop's (original)
                // header ip.
                Some(e) if e.orig_header == header_ip => (
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
                    e.materialize_recipes.clone(),
                    selected_tier,
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
        // at the header). Under OSR × scalar replacement the window is wider than `func.regs` by the
        // fresh tag/payload registers; only original registers `0..func.regs` carry
        // an interpreter value, and the scalar replacement-added slots stay 0 — the loop body assigns
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
                    flat_args.push(vm_jit::FlatBufferArg::Int(slice));
                }
                NativeTy::FlatFloat => {
                    let Some(slice) = guard.as_floats_slice() else {
                        unreachable!("flat kind validated before pinning")
                    };
                    scratch.window[*reg] = slice.as_ptr() as i64;
                    scratch.lens[*reg] = slice.len() as i64;
                    flat_args.push(vm_jit::FlatBufferArg::Float(slice));
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
                    flat_args.push(vm_jit::FlatBufferArg::IntMut(slice));
                }
                NativeTy::FlatFloatMut => {
                    let Some(slice) = guard.as_floats_mut_slice() else {
                        unreachable!("flat kind validated before pinning")
                    };
                    flat_args.push(vm_jit::FlatBufferArg::FloatMut(slice));
                }
                _ => unreachable!("mutable flat owner has non-mutable type"),
            }
        }

        // Phase 3: run the OSR loop body natively.
        // native limit accounting: seed the limits cell for an armed variant. `emit_step`/`emit_cancel`
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
                .map_or(0, |flag| flag.as_atomic() as *const _ as i64);
            jit_set_limits_cell(self.steps as i64, step_budget, cancel_addr);
        }
        // native limit accounting mem: seed the mem cell before EVERY OSR call (armed budget or `-1` to
        // disarm). The `ListPush*` helper charges flat-capacity growth against it; on a
        // clean exit we read `allocated_bytes` back to commit, on a bail the rollback+rerun
        // discards it. Independent of the step `limits_ptr` (helper-side).
        {
            let allocation_budget = self.limits.allocation_budget.map_or(-1, |b| b as i64);
            jit_set_mem_cell(self.allocated_bytes as i64, allocation_budget);
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
        debug_assert!(!armed, "armed OSR is rejected before compilation/dispatch");
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
        let result = module.call_with_host_ctx_at_depth(
            id,
            &scratch.window,
            &scratch.lens,
            heap_tx.host_ctx(),
            &mut flat_args,
            vm_jit::LogicalCallDepth {
                current: initial_depth,
                limit: self.limits.max_depth,
            },
        );
        let elapsed = started.map(|started| started.elapsed().as_nanos());
        // The pinned borrows are no longer needed once the native call returns.
        drop(flat_guards);
        drop(flat_mut_guards);
        // native limit accounting: fold the steps native paid (clean completion OR deopt both wrote it
        // back) into the interpreter's counter, so resuming the interpreter continues
        // the single tick stream with no double-/under-count.
        if emit_step {
            self.steps = jit_limits_cell_steps() as u64;
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
                // The OSR-exit's resume_ip (a TRANSFORMED-code ip) MUST be the loop's
                // post-loop exit ip; anything else is an OSR construction bug. Fall
                // back rather than misresume.
                if resume_ip as usize != trans_exit {
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
                    let Some(value) = JitHostCallCtx::from_token(heap_tx.host_ctx())
                        .and_then(|ctx| ctx.heap_read_handle(handle, |value| Some(value.clone())))
                    else {
                        heap_tx.abort();
                        scratch.restore(self.native.as_mut());
                        return false;
                    };
                    handle_liveouts.push((base + reg, value));
                }
                let Some(materialize_ctx) = JitHostCallCtx::from_token(heap_tx.host_ctx()) else {
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
                    self.allocated_bytes = jit_allocation_cell_allocated_bytes().max(0) as usize;
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
                    native.osr_bail_counts.insert(osr_version_key.clone(), 0);
                    // Lever 2 (observational): record this function actually OSR-
                    // entered, so the report's `osr: entered` positive matches the
                    // real outcome. Gated on `report`; no effect on any decision.
                    if native.report {
                        native
                            .report_osr_ok
                            .insert(self.jit_state.function_ordinal(func));
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
                    let count = native
                        .osr_bail_counts
                        .entry(osr_version_key.clone())
                        .or_insert(0);
                    *count = count.saturating_add(1);
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

/// The mutually-recursive group (call-graph SCC) containing `function_id`, if it is
/// a cycle of >= 2 functions (native-call-ABI slice 4). Returned sorted; `None` for
/// non-cyclic functions and pure self-recursion (handled by the self-recursive
/// path). Per-member native eligibility (scalar params/return, native-subset body)
/// is NOT decided here — the caller's `translate_to_native_jit_with_calls` per member
/// is the single eligibility gate, so the group admits any cycle the general native
/// path can compile (Int/Bool/Float bodies, members that also call inlinable leaves).
#[cfg(feature = "native-jit")]
#[cfg(feature = "jit-recursion-experimental")]
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

#[cfg(all(test, feature = "native-jit"))]
mod tests {
    use super::{osr_committed_tail_calls, osr_initial_logical_depth};

    #[test]
    fn osr_logical_depth_round_trips_accumulated_tail_calls() {
        let initial = osr_initial_logical_depth(3, 500);
        assert_eq!(initial, 503);
        assert_eq!(osr_committed_tail_calls(initial + 17, 3), 517);
    }
}
