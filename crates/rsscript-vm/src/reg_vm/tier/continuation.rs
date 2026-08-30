use super::*;

impl RegVm {
    pub(in crate::reg_vm) fn resolve_osr_candidates(
        &mut self,
        function: usize,
        func: &RegFunction,
    ) -> OsrCandidates {
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
    pub(in crate::reg_vm) fn resolve_osr_candidate(&mut self, func: &RegFunction) -> Option<usize> {
        let function = func.ordinal;
        self.resolve_osr_candidates(function, func).first_header()
    }

    /// Run one conservative scalar continuation region beginning at the current
    /// interpreter IP. The region commits only register-local scalar work and
    /// yields before the VM-owned barrier (`CallKnown` or `Return`).
    #[cfg(feature = "native-jit")]
    pub(in crate::reg_vm) fn try_continuation_region(
        &mut self,
        function: usize,
        func: &RegFunction,
        base: usize,
        entry_ip: usize,
    ) -> bool {
        if JitCallCtx::is_active() {
            return false;
        }
        // The scalar region has a one-to-one source instruction map, so generated
        // step/cancel accounting is exact. It cannot allocate or invoke an
        // intrinsic/Provider, therefore those meters remain entirely VM-owned at
        // the surrounding barriers. Deadlines are admitted only for finite
        // regions below; the next VM-owned barrier polls the clock.
        let cancel_armed = self.limits.cancel.is_some();
        if let Some(native) = self.native.as_mut()
            && native.collect_stats
        {
            native.stats.continuation_full_probes =
                native.stats.continuation_full_probes.saturating_add(1);
        }
        let Some(verified_facts) = self
            .native
            .as_ref()
            .and_then(|native| native.verified_facts.as_ref())
            .cloned()
        else {
            return false;
        };
        let Some(function_facts) = verified_facts.function(function) else {
            return false;
        };
        if let Some(native) = self.native.as_mut()
            && native.collect_stats
        {
            native.stats.continuation_instance_key_builds = native
                .stats
                .continuation_instance_key_builds
                .saturating_add(1);
        }
        let Some(instance) = JitInstanceKey::from_facts(function, function_facts) else {
            if let Some(native) = self.native.as_mut()
                && native.collect_stats
            {
                native.stats.static_instance_limit_fallbacks += 1;
            }
            return false;
        };
        debug_assert!(self.native.as_ref().is_some_and(|native| {
            native
                .continuation_entry_sets
                .get(&function)
                .is_some_and(|entries| entries.contains(entry_ip))
        }));
        let region = {
            let Some(native) = self.native.as_mut() else {
                return false;
            };
            native
                .continuation_plans
                .entry((function, entry_ip))
                .or_insert_with(|| {
                    detect_scalar_continuation_region(&func.code, func.regs, entry_ip).map(Rc::new)
                })
                .clone()
        };
        let Some(region) = region else {
            return false;
        };
        let decision = self.native.as_ref().map(|native| {
            continuation_decision(
                &region,
                native.cost_model,
                self.limits.step_budget.is_some(),
                self.limits.deadline.is_some(),
            )
        });
        if !matches!(decision, Some(ContinuationDecision::Compile)) {
            return false;
        }
        // Runtime controls can reject this particular entry even when the
        // structural plan is generally dispatchable. Check them before shape
        // construction and codegen so an already-cancelled, expired, or
        // under-budget run cannot populate the executable arena with code it can
        // never enter.
        if self
            .limits
            .deadline
            .is_some_and(rsscript_operation::MonotonicDeadline::is_expired)
        {
            return false;
        }
        if let Some(budget) = self.limits.step_budget
            && self.steps.saturating_add(region.source_instructions as u64) > budget
        {
            return false;
        }
        if self
            .limits
            .cancel
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
        {
            return false;
        }

        // Continuations admit scalars plus opaque read-only heap handles. Flat
        // borrows and unsupported runtime values remain outside this boundary.
        let mut live_in_shapes = Vec::with_capacity(region.live_in_regs.len());
        for &reg in region.live_in_regs.iter() {
            let slot = base + reg;
            if !self.written.get(slot).copied().unwrap_or(false) {
                return false;
            }
            let shape = native_param_shape_with_fact(
                &self.stack[slot],
                function_facts
                    .reg_types
                    .get(reg)
                    .copied()
                    .unwrap_or_default(),
            );
            if matches!(shape, NativeParamShape::Unsupported) {
                return false;
            }
            live_in_shapes.push(shape);
        }
        let shape = ShapeKey::from_shapes(live_in_shapes);
        let version_key = ContinuationVersionKey {
            instance: instance.clone(),
            entry: entry_ip,
            shape,
            cancel_armed,
        };
        // Closed-loop continuations amortize compilation within one activation.
        // Acyclic regions must first demonstrate repeated interpreted work; this
        // prevents a one-shot 512-instruction suffix from paying Cranelift startup
        // merely because it cleared the transition-profitability threshold.
        let compile_ready = {
            let Some(native) = self.native.as_mut() else {
                return false;
            };
            if native.continuation_cache.contains_key(&version_key) {
                true
            } else {
                let threshold = if region.has_backedge
                    || !matches!(native.cost_model, NativeCostModel::Enforce)
                {
                    0
                } else {
                    (region.source_instructions as u64)
                        .saturating_mul(2)
                        .max(1_024)
                };
                native
                    .continuation_controllers
                    .entry(version_key.clone())
                    .or_default()
                    .observe_interpreted_work(region.source_instructions as u64, threshold)
            }
        };
        if !compile_ready {
            return false;
        }
        let param_native_types: Vec<Option<NativeTy>> = (0..func.params)
            .map(|reg| {
                function_facts
                    .reg_types
                    .get(reg)
                    .and_then(|fact| fact.native_ty())
                    .or_else(|| match self.reg(base + reg) {
                        VmValue::Int(_) => Some(NativeTy::Int),
                        VmValue::Bool(_) => Some(NativeTy::Bool),
                        VmValue::Float(_) => Some(NativeTy::Float),
                        value if native_param_shape(value) == NativeParamShape::Handle => {
                            Some(NativeTy::Handle)
                        }
                        _ => None,
                    })
            })
            .collect();

        let entry = {
            let Some(native) = self.native.as_mut() else {
                return false;
            };
            if !native.continuation_cache.contains_key(&version_key) {
                if native.continuation_instance_count(function, entry_ip)
                    >= MAX_JIT_INSTANCES_PER_FUNCTION
                    && !native.has_continuation_instance(&instance, entry_ip)
                {
                    if native.collect_stats {
                        native.stats.static_instance_limit_fallbacks += 1;
                    }
                    return false;
                }
                let versions = native
                    .continuation_cache
                    .keys()
                    .filter(|key| key.instance == instance && key.entry == entry_ip)
                    .count();
                if versions >= MAX_NATIVE_SHAPE_VERSIONS {
                    if native.collect_stats {
                        native.stats.shape_limit_fallbacks += 1;
                    }
                    return false;
                }
                let translation = native.measure_translation(|| {
                    translate_scalar_continuation_region(
                        func,
                        function_facts,
                        &region,
                        &param_native_types,
                    )
                });
                let compiled = translation.and_then(|translation| {
                    let analyzed =
                        NativeRegion::continuation(u32::try_from(region.entry).ok()?, translation)
                            .analyze()?;
                    let admission = begin_native_compile(native, 1, NativeCodeTier::Baseline)?;
                    let controls = vm_jit::RegionCompileControls {
                        step: true,
                        cancel: cancel_armed,
                        deadline: self.limits.deadline.is_some(),
                    };
                    let published =
                        analyzed
                            .validate(&native.baseline_module)
                            .and_then(|validated| {
                                validated.publish(&mut native.baseline_module, controls)
                            });
                    match published {
                        Ok(published) => {
                            let id = published.id;
                            debug_assert!(matches!(
                                published.entry,
                                NativeRegionEntry::Continuation { .. }
                            ));
                            let source_work = analyzed.source_work();
                            let (jit_fn, metadata, _) = analyzed.into_parts();
                            let NativeRegionMetadata::Continuation {
                                slots,
                                live_in_count: n_live_in,
                                exits,
                                typed_summary,
                                virtual_summary,
                            } = metadata
                            else {
                                unreachable!("continuation lowering changed region kind")
                            };
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
                                if version_key.instance.type_arguments.is_known()
                                    && !native.has_continuation_instance(
                                        &version_key.instance,
                                        version_key.entry,
                                    )
                                {
                                    native.stats.static_type_instances += 1;
                                }
                                native.stats.continuation_compiled_source_instructions = native
                                    .stats
                                    .continuation_compiled_source_instructions
                                    .saturating_add(source_work);
                                native.stats.typed_region_compiles =
                                    native.stats.typed_region_compiles.saturating_add(1);
                                native.stats.typed_region_blocks = native
                                    .stats
                                    .typed_region_blocks
                                    .saturating_add(typed_summary.blocks as u64);
                                native.stats.typed_region_values = native
                                    .stats
                                    .typed_region_values
                                    .saturating_add(typed_summary.values as u64);
                                native.stats.typed_region_work_units = native
                                    .stats
                                    .typed_region_work_units
                                    .saturating_add(typed_summary.work_units as u64);
                                native.stats.virtual_objects_observed =
                                    native.stats.virtual_objects_observed.saturating_add(
                                        virtual_summary
                                            .options
                                            .saturating_add(virtual_summary.results)
                                            .saturating_add(virtual_summary.variants)
                                            .saturating_add(virtual_summary.structs)
                                            .saturating_add(virtual_summary.closures)
                                            as u64,
                                    );
                                native.stats.virtual_objects_no_escape = native
                                    .stats
                                    .virtual_objects_no_escape
                                    .saturating_add(virtual_summary.no_escape as u64);
                                native.stats.virtual_objects_exit_only = native
                                    .stats
                                    .virtual_objects_exit_only
                                    .saturating_add(virtual_summary.exit_only as u64);
                                native.stats.virtual_objects_declined = native
                                    .stats
                                    .virtual_objects_declined
                                    .saturating_add(virtual_summary.declined as u64);
                            }
                            native
                                .continuation_controllers
                                .entry(version_key.clone())
                                .or_default()
                                .compiled(false);
                            // No controlled canonical baseline
                            // clears the optimized-continuation retention
                            // gate. Keep the common controller state at
                            // baseline instead of inventing an unmeasured
                            // promotion path.
                            debug_assert_eq!(
                                continuation_tier_decision(None),
                                ContinuationTierDecision::BaselineOnly
                            );
                            Some(Rc::new(ContinuationEntry {
                                id,
                                entry: region.entry,
                                exits,
                                n_jit_regs: jit_fn.n_regs as usize,
                                n_live_in,
                                slots,
                            }))
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
        // Continuations use the same evaluation-local grow-only marshalling
        // buffers as OSR. A mixed function may cross this boundary thousands of
        // times; allocating two register-width vectors per transition otherwise
        // overwhelms the scalar work the region is meant to accelerate.
        let mut scratch = match self.native.as_mut() {
            Some(native) => take_osr_native_call_scratch(native, entry.n_jit_regs),
            None => return false,
        };
        let mut native_frame = JitNativeCallFrame::begin(self.limits.deadline);
        macro_rules! decline_continuation {
            () => {{
                native_frame.abort();
                scratch.restore(self.native.as_mut());
                return false;
            }};
        }
        for (native_reg, region_slot) in entry.slots.iter().take(entry.n_live_in).enumerate() {
            debug_assert!(region_slot.class.is_live_in());
            let slot = base + region_slot.vm_reg;
            if !self.written.get(slot).copied().unwrap_or(false) {
                continue;
            }
            scratch.window[native_reg] = match (region_slot.ty, self.stack.get(slot)) {
                (NativeTy::Int, Some(VmValue::Int(value))) => *value,
                (NativeTy::Bool, Some(VmValue::Bool(value))) => i64::from(*value),
                (NativeTy::Float, Some(VmValue::Float(value))) => value.to_bits() as i64,
                (NativeTy::Handle, Some(value)) => {
                    let Ok(handle) = i64::try_from(native_frame.push_heap_arg(value.clone()))
                    else {
                        decline_continuation!();
                    };
                    handle
                }
                _ => decline_continuation!(),
            };
        }

        let started = self
            .native
            .as_ref()
            .is_some_and(|native| native.collect_stats)
            .then(std::time::Instant::now);
        let steps_before = self.steps;
        let initial_steps = {
            let Ok(steps) = i64::try_from(self.steps) else {
                decline_continuation!();
            };
            steps
        };
        let native_step_budget = match self.limits.step_budget {
            Some(budget) => {
                let Ok(budget) = i64::try_from(budget) else {
                    decline_continuation!();
                };
                Some(budget)
            }
            None => None,
        };
        let physical_depth = self.frames.len();
        let prior_tail_calls = self.frames.last().map_or(0, |frame| frame.tail_calls);
        let initial_depth = osr_initial_logical_depth(physical_depth, prior_tail_calls);
        let native_result = self.native.as_mut().map(|native| {
            native
                .baseline_module
                .call_with_host_ctx_step_cancel_in_session(
                    &mut native.call_session,
                    entry.id,
                    &mut scratch.window,
                    &scratch.lens,
                    vm_jit::RegionCallControls {
                        host_ctx: native_frame.host_ctx(),
                        logical_depth: vm_jit::LogicalCallDepth {
                            current: initial_depth,
                            limit: self.limits.max_depth,
                        },
                        initial_steps,
                        step_budget: native_step_budget,
                        cancel: self.limits.cancel.as_ref().map(|token| token.as_atomic()),
                    },
                )
        });
        let result = native_result.map(|(outcome, steps)| {
            self.steps = steps.max(0) as u64;
            outcome
        });
        if let Some(native) = self.native.as_mut()
            && let Some(started) = started
        {
            native.stats.run_nanos = native
                .stats
                .run_nanos
                .saturating_add(started.elapsed().as_nanos());
        }
        let Some(vm_jit::NativeOutcome::Yield { exit_id }) = result else {
            native_frame.abort();
            scratch.restore(self.native.as_mut());
            let cancelled = self
                .limits
                .cancel
                .as_ref()
                .is_some_and(|token| token.is_cancelled());
            let over_budget = self
                .limits
                .step_budget
                .is_some_and(|budget| self.steps > budget);
            if !cancelled && !over_budget {
                self.steps = steps_before;
                if let Some(native) = self.native.as_mut() {
                    let disabled = native
                        .continuation_controllers
                        .entry(version_key.clone())
                        .or_default()
                        .dynamic_bail(NATIVE_BAIL_GIVEUP_THRESHOLD);
                    if native.collect_stats {
                        native.stats.shape_bails += 1;
                    }
                    if disabled {
                        native.continuation_cache.insert(version_key.clone(), None);
                    }
                }
            }
            return false;
        };
        let exit = exit_id as usize;
        let Some(exit_meta) = entry.exits.get(&exit) else {
            native_frame.abort();
            scratch.restore(self.native.as_mut());
            if let Some(native) = self.native.as_mut() {
                native
                    .continuation_controllers
                    .entry(version_key.clone())
                    .or_default()
                    .disable();
                native.continuation_cache.insert(version_key.clone(), None);
            }
            return false;
        };
        if native_frame.commit_scalar_with_writebacks(&[]).is_none() {
            scratch.restore(self.native.as_mut());
            return false;
        }
        for &native_reg in exit_meta.live_slots.iter() {
            let native_reg = native_reg as usize;
            let Some(region_slot) = entry.slots.get(native_reg).copied() else {
                scratch.restore(self.native.as_mut());
                native_frame.abort();
                return false;
            };
            if !region_slot.class.is_live_out() {
                continue;
            }
            let value = match region_slot.ty {
                NativeTy::Int => VmValue::Int(scratch.window[native_reg]),
                NativeTy::Bool => VmValue::Bool(scratch.window[native_reg] != 0),
                NativeTy::Float => {
                    VmValue::Float(f64::from_bits(scratch.window[native_reg] as u64))
                }
                _ => {
                    scratch.restore(self.native.as_mut());
                    native_frame.abort();
                    return false;
                }
            };
            self.set_reg(base + region_slot.vm_reg, value);
        }
        scratch.restore(self.native.as_mut());
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
                .entry(exit_meta.reason.as_str().to_string())
                .or_default() += 1;
        }
        if let Some(native) = self.native.as_mut() {
            native
                .continuation_controllers
                .entry(version_key)
                .or_default()
                .successful_entry();
        }
        true
    }

    #[cfg(feature = "native-jit")]
    pub(in crate::reg_vm) fn has_continuation_region(
        &mut self,
        function: usize,
        func: &RegFunction,
        entries: &ContinuationEntrySet,
    ) -> bool {
        let Some(native) = self.native.as_mut() else {
            return false;
        };
        if let Some(&has_region) = native.continuation_functions.get(&function) {
            return has_region;
        }
        let step_armed = self.limits.step_budget.is_some();
        let deadline_armed = self.limits.deadline.is_some();
        let cost_model = native.cost_model;
        // This gate stops tier-0 from consuming a function with useful mixed-mode
        // work. Bound the static search: scanning all post-barrier suffixes is
        // quadratic in generated call chains, while the first few barriers cover
        // the normal prelude/aggregate setup shapes. Later entries remain lazy.
        const MAX_EAGER_CONTINUATION_PROBES: usize = 8;
        let has_region = entries
            .iter()
            .take(MAX_EAGER_CONTINUATION_PROBES)
            .any(|entry| {
                let region = native
                    .continuation_plans
                    .entry((function, entry))
                    .or_insert_with(|| {
                        detect_scalar_continuation_region(&func.code, func.regs, entry).map(Rc::new)
                    });
                region.as_ref().is_some_and(|region| {
                    matches!(
                        continuation_decision(region, cost_model, step_armed, deadline_armed),
                        ContinuationDecision::Compile
                    )
                })
            });
        native.continuation_functions.insert(function, has_region);
        has_region
    }
}
