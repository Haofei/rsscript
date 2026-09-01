//! `RegVm::attempt_native` — native compilation attempt, split from `tier.rs`
//! for module-size partitioning (a second impl RegVm block).

use super::*;

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
    pub(in crate::reg_vm) fn attempt_native(&mut self, func: &RegFunction, base: usize) -> NativeAttempt {
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
        if !self.native_preemption_controls_supported()
            || !whole_function_memory_controls_supported(func, &self.limits)
        {
            return NativeAttempt::Fallback;
        }
        if self.limits.live_memory_limit.is_some() {
            // The interpreter normally observes the freshly installed frame on
            // its first tick. Whole-function native entry skips that tick, so take
            // the same exact root snapshot before machine code can complete and
            // pop the frame. A failure falls back; the interpreter then reports
            // the authoritative typed execution error.
            self.live_memory_dirty = true;
            if self.refresh_live_memory_usage().is_err() {
                return NativeAttempt::Fallback;
            }
        }
        let compile_controls = vm_jit::RegionCompileControls {
            step: self.limits.step_budget.is_some()
                || self.limits.cancel.is_some()
                || self.limits.deadline.is_some(),
            cancel: self.limits.cancel.is_some(),
            deadline: self.limits.deadline.is_some(),
        };
        // Cheap negative path: a function known not native-eligible never compiles,
        // so skip all per-call tiering/cache/name-hash work and fall straight back
        // to the interpreter (keeps `jit-native` from being slower than the VM on
        // code the native tier can't take).
        let native_status = self.jit_state.native_status(func);
        if native_status == NATIVE_STATUS_NOT_ELIGIBLE {
            return NativeAttempt::Fallback;
        }
        // The unit is needed to resolve inlinable callees; clone the `Rc` so the
        // mutable `self.native` borrow below doesn't conflict.
        let unit = Rc::clone(&self.unit);
        let native_key = func.ordinal;
        let profile = self.jit_state.profile(func);
        let call_count = self.jit_state.call_count(func);
        let Some(verified_facts) = self
            .native
            .as_ref()
            .and_then(|native| native.verified_facts.as_ref())
            .cloned()
        else {
            return NativeAttempt::Fallback;
        };
        let Some(function_facts) = verified_facts.function(native_key) else {
            return NativeAttempt::Fallback;
        };
        let Some(instance) = JitInstanceKey::from_facts(native_key, function_facts) else {
            if let Some(native) = self.native.as_mut()
                && native.collect_stats
            {
                native.stats.static_instance_limit_fallbacks += 1;
            }
            return NativeAttempt::Fallback;
        };
        let shape = ShapeKey::from_shapes((0..func.params).map(|index| {
            native_param_shape_with_fact(
                self.reg(base + index),
                function_facts
                    .reg_types
                    .get(index)
                    .copied()
                    .unwrap_or_default(),
            )
        }));
        let version_key = NativeVersionKey {
            instance: instance.clone(),
            shape,
        };
        // Phase 1: tiering + resolve (and lazily compile) the native function.
        // `None` in the cache means "known not native-eligible".
        let (id, ret_type, param_types, string_literals, precise_resume_safe, selected_tier) = {
            let Some(native) = self.native.as_mut() else {
                return NativeAttempt::Fallback;
            };
            if compile_controls != vm_jit::RegionCompileControls::default() && !native.precise_deopt
            {
                return NativeAttempt::Fallback;
            }
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
                    if native.whole_instance_count(native_key) >= MAX_JIT_INSTANCES_PER_FUNCTION
                        && !native.has_whole_instance(&instance)
                    {
                        if native.collect_stats {
                            native.stats.static_instance_limit_fallbacks += 1;
                        }
                        return NativeAttempt::Fallback;
                    }
                    if native.whole_shape_count(&instance) >= MAX_NATIVE_SHAPE_VERSIONS {
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
                    let translated = native.measure_translation(|| {
                        if compiled_call_sites.is_empty() {
                            translate_to_native_jit(
                                &unit,
                                func,
                                function_facts,
                                profile,
                                call_count,
                            )
                        } else {
                            translate_to_native_jit_with_compiled_callees(
                                &unit,
                                func,
                                function_facts,
                                profile,
                                call_count,
                                &compiled_call_sites,
                            )
                            .or_else(|| {
                                translate_to_native_jit(
                                    &unit,
                                    func,
                                    function_facts,
                                    profile,
                                    call_count,
                                )
                            })
                        }
                    });
                    let entry = match translated {
                        Some(translation) => {
                            let Some(analyzed) = NativeRegion::whole(translation).analyze() else {
                                return NativeAttempt::Fallback;
                            };
                            let NativeRegionMetadata::Whole {
                                return_ty: ret,
                                param_tys: params,
                                precise_resume_safe,
                            } = analyzed.metadata()
                            else {
                                unreachable!("whole-function lowering changed region kind")
                            };
                            let ret = *ret;
                            let params = params.clone();
                            let precise_resume_safe = *precise_resume_safe;
                            let string_literals = analyzed.string_literals().to_vec();
                            let jit_fn = analyzed.jit_fn();
                            if compile_controls != vm_jit::RegionCompileControls::default()
                                && !precise_resume_safe
                            {
                                return NativeAttempt::Fallback;
                            }
                            if native.collect_stats {
                                native.stats.translated += 1;
                            }
                            let scalar_leaf_callable = vm_jit::is_native_callable_leaf(jit_fn);
                            // Step 1 cost model (eligibility already proven by `translate`):
                            // in `enforce` mode, decline an unprofitable region and keep the
                            // function on the interpreter (cached below as not-native). `off`
                            // and `report` modes never change execution here.
                            let has_backedge = jit_function_has_loop(&func.code);
                            if consult_profitability(
                                native,
                                jit_fn,
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
                                // Static call-count promotion cannot observe the
                                // dynamic work of an internal backedge. Compile
                                // such bodies directly into the optimized module.
                                // backedge heat.
                                let initial_tier =
                                    if has_backedge && native.optimized_module.is_some() {
                                        NativeCodeTier::Optimized
                                    } else {
                                        NativeCodeTier::Baseline
                                    };
                                let Some(admission) = begin_native_compile(native, 1, initial_tier)
                                else {
                                    native.cache.insert(version_key.clone(), None);
                                    return NativeAttempt::Fallback;
                                };
                                let compiled = match initial_tier {
                                    NativeCodeTier::Baseline => {
                                        if native.force_all_safepoints {
                                            native.baseline_module.compile_forcing_all_bails(jit_fn)
                                        } else {
                                            match native.forced_safepoint {
                                                Some(site) => native
                                                    .baseline_module
                                                    .compile_forcing_bail(jit_fn, site),
                                                None if compile_controls
                                                    != vm_jit::RegionCompileControls::default() =>
                                                {
                                                    analyzed
                                                        .validate(&native.baseline_module)
                                                        .and_then(|validated| {
                                                            validated
                                                                .publish(
                                                                    &mut native.baseline_module,
                                                                    compile_controls,
                                                                )
                                                                .map(|published| published.id)
                                                        })
                                                }
                                                None => analyzed
                                                    .validate(&native.baseline_module)
                                                    .and_then(|validated| {
                                                        validated
                                                            .publish(
                                                                &mut native.baseline_module,
                                                                compile_controls,
                                                            )
                                                            .map(|published| published.id)
                                                    }),
                                            }
                                        }
                                    }
                                    NativeCodeTier::Optimized => {
                                        let module = native
                                            .optimized_module
                                            .as_mut()
                                            .expect("optimized initial tier requires module");
                                        if native.force_all_safepoints {
                                            module.compile_forcing_all_bails(jit_fn)
                                        } else {
                                            match native.forced_safepoint {
                                                Some(site) => {
                                                    module.compile_forcing_bail(jit_fn, site)
                                                }
                                                None if compile_controls
                                                    != vm_jit::RegionCompileControls::default() =>
                                                {
                                                    analyzed.validate(module).and_then(
                                                        |validated| {
                                                            validated
                                                                .publish(module, compile_controls)
                                                                .map(|published| published.id)
                                                        },
                                                    )
                                                }
                                                None => analyzed.validate(module).and_then(
                                                    |validated| {
                                                        validated
                                                            .publish(module, compile_controls)
                                                            .map(|published| published.id)
                                                    },
                                                ),
                                            }
                                        }
                                    }
                                };
                                match compiled {
                                    Ok(id) => {
                                        if !finish_native_compile(
                                            native,
                                            admission,
                                            &[id],
                                            initial_tier,
                                        ) {
                                            native.cache.insert(version_key.clone(), None);
                                            return NativeAttempt::Fallback;
                                        }
                                        let verify_native =
                                            cfg!(debug_assertions) || jit_native_verify_is_strict();
                                        let verification =
                                            verify_native.then(|| match initial_tier {
                                                NativeCodeTier::Baseline => {
                                                    jit_verify_compiled_native(
                                                        &native.baseline_module,
                                                        id,
                                                        jit_fn,
                                                        native.forced_safepoint,
                                                    )
                                                }
                                                NativeCodeTier::Optimized => {
                                                    jit_verify_compiled_native(
                                                        native.optimized_module.as_ref().expect(
                                                            "optimized initial tier module",
                                                        ),
                                                        id,
                                                        jit_fn,
                                                        native.forced_safepoint,
                                                    )
                                                }
                                            });
                                        if let Some(Err(err)) = verification {
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
                                            jit_fn,
                                            initial_tier,
                                        );
                                        if matches!(initial_tier, NativeCodeTier::Baseline)
                                            && native.optimized_module.is_some()
                                            && native_region_is_promotion_eligible(jit_fn)
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
                    let first_static_instance = version_key.instance.type_arguments.is_known()
                        && !native.has_whole_instance(&version_key.instance);
                    let initially_optimized =
                        entry
                            .as_ref()
                            .is_some_and(|(_, _, _, has_backedge, _, _, _)| {
                                *has_backedge && native.optimized_module.is_some()
                            });
                    if initially_optimized {
                        native.cache.insert(version_key.clone(), None);
                        native
                            .optimized_cache
                            .insert(version_key.clone(), entry.clone().expect("compiled entry"));
                    } else {
                        native.cache.insert(version_key.clone(), entry.clone());
                    }
                    if entry.is_some() {
                        native
                            .whole_controllers
                            .entry(version_key.clone())
                            .or_default()
                            .compiled(initially_optimized);
                    }
                    if entry.is_some() && native.collect_stats {
                        native.stats.shape_versions += 1;
                        if first_static_instance {
                            native.stats.static_type_instances += 1;
                        }
                    }
                    entry
                }
            };
            let mut selected_tier = NativeCodeTier::Baseline;
            let mut selected_entry = entry;
            if native.optimized_module.is_some() {
                let work = u64::from(interpreted_region_work(&func.code));
                let promote = native
                    .whole_controllers
                    .entry(version_key.clone())
                    .or_default()
                    .observe_native_work(work, native.optimize_work_threshold);
                if promote
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
                            None if compile_controls
                                != vm_jit::RegionCompileControls::default() =>
                            {
                                native
                                    .optimized_module
                                    .as_mut()
                                    .expect("optimized module")
                                    .compile_with_controls(&jit_fn, compile_controls)
                            }
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
                                        native
                                            .whole_controllers
                                            .entry(version_key.clone())
                                            .or_default()
                                            .compiled(true);
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
        let mut heap_tx = JitNativeCallFrame::begin(self.limits.deadline);
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
                        flat_args.push(vm_jit::IndexedFlatBufferArg::new(
                            index,
                            vm_jit::FlatBufferArg::Int(slice),
                        ));
                    }
                    NativeTy::FlatIntMut => {
                        let slice = flat_mut_iter
                            .next()
                            .and_then(|v| v.as_ints_mut_slice())
                            .expect("Ints mut pinned");
                        scratch.args[index] = slice.as_mut_ptr() as i64;
                        scratch.lens[index] = slice.len() as i64;
                        flat_args.push(vm_jit::IndexedFlatBufferArg::new(
                            index,
                            vm_jit::FlatBufferArg::IntMut(slice),
                        ));
                    }
                    NativeTy::FlatFloat => {
                        let slice = flat_iter
                            .next()
                            .and_then(|v| v.as_floats_slice())
                            .expect("Floats pinned");
                        scratch.args[index] = slice.as_ptr() as i64;
                        scratch.lens[index] = slice.len() as i64;
                        flat_args.push(vm_jit::IndexedFlatBufferArg::new(
                            index,
                            vm_jit::FlatBufferArg::Float(slice),
                        ));
                    }
                    NativeTy::FlatFloatMut => {
                        let slice = flat_mut_iter
                            .next()
                            .and_then(|v| v.as_floats_mut_slice())
                            .expect("Floats mut pinned");
                        scratch.args[index] = slice.as_mut_ptr() as i64;
                        scratch.lens[index] = slice.len() as i64;
                        flat_args.push(vm_jit::IndexedFlatBufferArg::new(
                            index,
                            vm_jit::FlatBufferArg::FloatMut(slice),
                        ));
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
        let (result, elapsed, native_steps) = {
            let Some(native_ref) = self.native.as_mut() else {
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
            let armed = compile_controls != vm_jit::RegionCompileControls::default();
            let initial_steps = i64::try_from(self.steps).unwrap_or(i64::MAX);
            let step_budget = self
                .limits
                .step_budget
                .and_then(|budget| i64::try_from(budget).ok());
            let (result, native_steps) = if armed {
                module.call_with_indexed_flat_args_and_controls_in_session_at_depth(
                    &mut native_ref.call_session,
                    id,
                    &scratch.args,
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
                        &scratch.args,
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
            (result, elapsed, native_steps)
        };
        if compile_controls.step {
            self.steps = native_steps.max(0) as u64;
        }
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
                    native
                        .whole_controllers
                        .entry(version_key.clone())
                        .or_default()
                        .successful_entry();
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
                            native
                                .whole_controllers
                                .entry(version_key.clone())
                                .or_default()
                                .successful_entry();
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
                            if self.try_resume_native_child_deopt_chain(NativeChildDeoptResume {
                                unit: &unit,
                                function: func,
                                base,
                                resume_ip: resume_ip as usize,
                                live: &live,
                                child,
                                host_ctx: heap_tx.host_ctx(),
                            }) {
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
}
