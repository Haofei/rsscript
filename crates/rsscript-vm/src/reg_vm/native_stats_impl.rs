//! NativeStats impl + JIT report helpers — impls/free-fns split from `reg_vm/mod.rs` for module-size partitioning.
//! All type definitions stay in mod.rs.

use super::*;

#[cfg(feature = "native-jit")]
impl NativeStats {
    #[cfg(any(test, feature = "jit-diagnostics"))]
    pub(in crate::reg_vm) fn summary(&self) -> String {
        format!(
            "native-jit: verified_known_reg_types={} verified_unknown_reg_types={} verified_known_call_sites={} verified_instruction_effects={} considered={} translated={} compiled={} baseline_compiles={} optimized_compiles={} baseline_calls={} optimized_calls={} promotions={} ir_instrs={} code_bytes={} admission_admitted={} admission_admitted_bytes={} admission_rejected={} admission_rejected_bytes={} deopt_sites={} direct_list_bounds_check_sites={} memoized_runtime_helper_call_sites={} runtime_helper_call_sites={} fused_map_match_helper_sites={} direct_list_store_load_forwarded_moves={} native_call_edges={} direct_scalar_call_edges={} static_inline_candidates={} native_call_depth_max={} profile_closure_guards={} profile_closure_id_reads={} profile_closure_pic_sites={} profile_closure_pic_arms={} profile_branch_sites={} profile_branch_samples={} profile_branch_taken={} profile_branch_fallthrough={} profile_branch_cold_blocks={} profile_branch_side_exits={} not_eligible={} top_decline={} \
compile_failed={} calls={} bails={} child_bails={} child_resumes={} arg_mismatch={} shape_versions={} static_type_instances={} static_instance_limit_fallbacks={} shape_cache_hits={} shape_limit_fallbacks={} shape_bails={} tier_deferred={} \
compile_ms={:.3} run_ms={:.3} osr_entries={} continuation_entries={} continuation_yields={} unprofitable_declines={} typed_region_compiles={} typed_region_blocks={} typed_region_values={} typed_region_work_units={} virtual_objects_observed={} virtual_objects_no_escape={} virtual_objects_exit_only={} virtual_objects_declined={}",
            self.verified_known_reg_types,
            self.verified_unknown_reg_types,
            self.verified_known_call_sites,
            self.verified_instruction_effects,
            self.considered,
            self.translated,
            self.compiled,
            self.baseline_compiles,
            self.optimized_compiles,
            self.baseline_calls,
            self.optimized_calls,
            self.promotions,
            self.compiled_ir_instrs,
            self.compiled_code_bytes,
            self.admission_admitted,
            self.admission_admitted_bytes,
            self.admission_rejected,
            self.admission_rejected_bytes,
            self.deopt_sites,
            self.direct_list_bounds_check_sites,
            self.memoized_runtime_helper_call_sites,
            self.runtime_helper_call_sites,
            self.fused_map_match_helper_sites,
            self.direct_list_store_load_forwarded_moves,
            self.native_call_edges,
            self.direct_scalar_call_edges,
            self.static_inline_candidates,
            self.native_call_depth_max,
            self.profile_closure_guard_sites,
            self.profile_closure_id_reads,
            self.profile_closure_pic_sites,
            self.profile_closure_pic_arms,
            self.profile_branch_sites,
            self.profile_branch_samples,
            self.profile_branch_taken,
            self.profile_branch_fallthrough,
            self.profile_branch_cold_blocks,
            self.profile_branch_side_exits,
            self.not_eligible,
            self.top_native_decline_reason(),
            self.compile_failed,
            self.native_calls,
            self.native_bails,
            self.native_child_bails,
            self.native_child_resumes,
            self.arg_mismatch,
            self.shape_versions,
            self.static_type_instances,
            self.static_instance_limit_fallbacks,
            self.shape_cache_hits,
            self.shape_limit_fallbacks,
            self.shape_bails,
            self.tier_deferred,
            self.compile_nanos as f64 / 1.0e6,
            self.run_nanos as f64 / 1.0e6,
            self.osr_entries,
            self.continuation_entries,
            self.continuation_yields,
            self.unprofitable_declines,
            self.typed_region_compiles,
            self.typed_region_blocks,
            self.typed_region_values,
            self.typed_region_work_units,
            self.virtual_objects_observed,
            self.virtual_objects_no_escape,
            self.virtual_objects_exit_only,
            self.virtual_objects_declined,
        )
    }

    #[cfg(any(test, feature = "jit-diagnostics"))]
    pub(in crate::reg_vm) fn top_native_decline_reason(&self) -> String {
        self.native_decline_reasons
            .iter()
            .max_by(|(lhs_reason, lhs_count), (rhs_reason, rhs_count)| {
                lhs_count
                    .cmp(rhs_count)
                    .then_with(|| rhs_reason.cmp(lhs_reason))
            })
            .map(|(reason, count)| format!("{count}x {reason}"))
            .unwrap_or_else(|| "none".to_string())
    }

    pub(in crate::reg_vm) fn add_native_decline_reasons(
        &mut self,
        unit: &RegUnit,
        jit_state: &JitState,
        facts: &VerifiedExecutableFacts,
    ) {
        self.native_decline_reasons = native_decline_reason_counts(unit, jit_state, facts);
    }

    pub(in crate::reg_vm) fn add_profile_feedback(&mut self, unit: &RegUnit, jit_state: &JitState) {
        let mut sites = 0u64;
        let mut taken = 0u64;
        let mut fallthrough = 0u64;
        for func in &unit.functions {
            let Some(profile) = jit_state.profile(func) else {
                continue;
            };
            for (_, feedback) in profile.branch_feedback_sites() {
                sites += 1;
                taken += u64::from(feedback.taken);
                fallthrough += u64::from(feedback.fallthrough);
            }
        }
        self.profile_branch_sites = sites;
        self.profile_branch_taken = taken;
        self.profile_branch_fallthrough = fallthrough;
        self.profile_branch_samples = taken.saturating_add(fallthrough);
    }

    pub(in crate::reg_vm) fn add_loop_optimization_evidence(&mut self, evidence: &LoopOptimizationEvidence) {
        self.canonical_loops = evidence.canonical_loops;
        self.canonical_loop_preheaders = evidence.unique_preheaders;
        self.canonical_induction_variables = evidence.induction_variables;
        self.loop_analysis_work_units = evidence.analysis_work_units;
        self.loop_analysis_limit_reached = u64::from(evidence.analysis_limit_reached);
    }

    /// Telemetry as JSON for VM/JIT benchmark and reporting harnesses.
    pub fn to_json(&self) -> crate::serde_json::Value {
        let mut value = crate::serde_json::json!({
            "considered": self.considered,
            "translated": self.translated,
            "compiled": self.compiled,
            "compiled_ir_instrs": self.compiled_ir_instrs,
            "compiled_code_bytes": self.compiled_code_bytes,
            "admission_admitted": self.admission_admitted,
            "admission_admitted_bytes": self.admission_admitted_bytes,
            "admission_rejected": self.admission_rejected,
            "admission_rejected_bytes": self.admission_rejected_bytes,
            "deopt_sites": self.deopt_sites,
            "direct_list_bounds_check_sites": self.direct_list_bounds_check_sites,
            "memoized_runtime_helper_call_sites": self.memoized_runtime_helper_call_sites,
            "runtime_helper_call_sites": self.runtime_helper_call_sites,
            "direct_list_store_load_forwarded_moves": self.direct_list_store_load_forwarded_moves,
            "native_call_edges": self.native_call_edges,
            "native_call_depth_max": self.native_call_depth_max,
            "profile_closure_guard_sites": self.profile_closure_guard_sites,
            "profile_closure_id_reads": self.profile_closure_id_reads,
            "profile_closure_pic_sites": self.profile_closure_pic_sites,
            "profile_closure_pic_arms": self.profile_closure_pic_arms,
            "profile_branch_sites": self.profile_branch_sites,
            "profile_branch_samples": self.profile_branch_samples,
            "profile_branch_taken": self.profile_branch_taken,
            "profile_branch_fallthrough": self.profile_branch_fallthrough,
            "profile_branch_cold_blocks": self.profile_branch_cold_blocks,
            "profile_branch_side_exits": self.profile_branch_side_exits,
            "not_eligible": self.not_eligible,
            "native_decline_reasons": &self.native_decline_reasons,
            "compile_failed": self.compile_failed,
            "native_calls": self.native_calls,
            "bails": self.native_bails,
            "child_bails": self.native_child_bails,
            "child_resumes": self.native_child_resumes,
            "arg_mismatch": self.arg_mismatch,
            "tier_deferred": self.tier_deferred,
            "compile_ms": self.compile_nanos as f64 / 1.0e6,
            "run_ms": self.run_nanos as f64 / 1.0e6,
            "osr_entries": self.osr_entries,
            "unprofitable_declines": self.unprofitable_declines,
            "unprofitable_decline_reasons": &self.unprofitable_decline_reasons,
            "unprofitable_declined_fns": &self.unprofitable_declined_fns,
        });
        let object = value.as_object_mut().expect("stats JSON is an object");
        object.insert(
            "translation_nanos".into(),
            crate::serde_json::json!(self.translation_nanos),
        );
        object.insert(
            "validation_nanos".into(),
            crate::serde_json::json!(self.validation_nanos),
        );
        object.insert(
            "codegen_nanos".into(),
            crate::serde_json::json!(self.codegen_nanos),
        );
        object.insert(
            "finalize_nanos".into(),
            crate::serde_json::json!(self.finalize_nanos),
        );
        object.insert(
            "direct_scalar_call_edges".into(),
            self.direct_scalar_call_edges.into(),
        );
        object.insert(
            "static_inline_candidates".into(),
            self.static_inline_candidates.into(),
        );
        object.insert(
            "direct_list_bounds_checks_elided".into(),
            self.direct_list_bounds_checks_elided.into(),
        );
        object.insert("canonical_loops".into(), self.canonical_loops.into());
        object.insert(
            "canonical_loop_preheaders".into(),
            self.canonical_loop_preheaders.into(),
        );
        object.insert(
            "canonical_induction_variables".into(),
            self.canonical_induction_variables.into(),
        );
        object.insert(
            "loop_analysis_work_units".into(),
            self.loop_analysis_work_units.into(),
        );
        object.insert(
            "loop_analysis_limit_reached".into(),
            self.loop_analysis_limit_reached.into(),
        );
        object.insert(
            "verified_known_reg_types".into(),
            self.verified_known_reg_types.into(),
        );
        object.insert(
            "verified_unknown_reg_types".into(),
            self.verified_unknown_reg_types.into(),
        );
        object.insert(
            "verified_known_call_sites".into(),
            self.verified_known_call_sites.into(),
        );
        object.insert(
            "verified_instruction_effects".into(),
            self.verified_instruction_effects.into(),
        );
        object.insert(
            "typed_region_compiles".into(),
            self.typed_region_compiles.into(),
        );
        object.insert(
            "typed_region_blocks".into(),
            self.typed_region_blocks.into(),
        );
        object.insert(
            "typed_region_values".into(),
            self.typed_region_values.into(),
        );
        object.insert(
            "typed_region_work_units".into(),
            self.typed_region_work_units.into(),
        );
        object.insert(
            "virtual_objects_observed".into(),
            self.virtual_objects_observed.into(),
        );
        object.insert(
            "virtual_objects_no_escape".into(),
            self.virtual_objects_no_escape.into(),
        );
        object.insert(
            "virtual_objects_exit_only".into(),
            self.virtual_objects_exit_only.into(),
        );
        object.insert(
            "virtual_objects_declined".into(),
            self.virtual_objects_declined.into(),
        );
        object.insert(
            "interpreted_native_work".into(),
            self.interpreted_native_work.into(),
        );
        object.insert(
            "native_barrier_counts".into(),
            crate::serde_json::to_value(&self.native_barrier_counts)
                .expect("barrier counts serialize"),
        );
        object.insert(
            "continuation_entries".into(),
            self.continuation_entries.into(),
        );
        object.insert(
            "continuation_candidate_checks".into(),
            self.continuation_candidate_checks.into(),
        );
        object.insert(
            "continuation_full_probes".into(),
            self.continuation_full_probes.into(),
        );
        object.insert(
            "continuation_instance_key_builds".into(),
            self.continuation_instance_key_builds.into(),
        );
        object.insert(
            "continuation_compiled_source_instructions".into(),
            self.continuation_compiled_source_instructions.into(),
        );
        object.insert(
            "continuation_yields".into(),
            self.continuation_yields.into(),
        );
        object.insert("baseline_compiles".into(), self.baseline_compiles.into());
        object.insert("optimized_compiles".into(), self.optimized_compiles.into());
        object.insert("baseline_calls".into(), self.baseline_calls.into());
        object.insert("optimized_calls".into(), self.optimized_calls.into());
        object.insert("promotions".into(), self.promotions.into());
        object.insert(
            "fused_map_match_helper_sites".into(),
            self.fused_map_match_helper_sites.into(),
        );
        object.insert("shape_versions".into(), self.shape_versions.into());
        object.insert(
            "static_type_instances".into(),
            self.static_type_instances.into(),
        );
        object.insert(
            "static_instance_limit_fallbacks".into(),
            self.static_instance_limit_fallbacks.into(),
        );
        object.insert("shape_cache_hits".into(), self.shape_cache_hits.into());
        object.insert(
            "shape_limit_fallbacks".into(),
            self.shape_limit_fallbacks.into(),
        );
        object.insert("shape_bails".into(), self.shape_bails.into());
        value
    }
}

/// Developer-facing structured missed-optimization report.
///
/// Walks every function in `unit` and re-derives — **observationally, read-only** —
/// why each did or didn't go native / OSR / scalar-replace / inline / fold, with the
/// intrinsic-level reasons sourced from the central [`intrinsic_descriptor`] registry
/// (effect + notes). This RE-RUNS the same cheap predicates the real passes use
/// (`translate_to_native_jit`, `detect_single_natural_loop`, `native_subset_instruction`,
/// `native_inline_leaf_calls`) WITHOUT touching the passes themselves, so it cannot
/// change any compile decision — the proof is the byte-identical differential with the
/// report on or off. Positive verdicts (`native: ok`, `osr: entered`) are cross-checked
/// against the actual runtime outcome recorded in `report_native_ok` / `report_osr_ok`,
/// so a line the report prints as "ok"/"entered" really happened, and a "not …" line
/// really did not (the report-correctness tests assert this).
///
/// One block per function (deduped by construction — each function is visited once).
#[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
pub(in crate::reg_vm) fn jit_missed_opt_report(
    unit: &RegUnit,
    jit_state: &JitState,
    native: &NativeState,
) -> Vec<String> {
    let mut out = vec![format!("jit-report: summary\n  {}", native.stats.summary())];
    let native_decline_counts = native_decline_reason_counts(
        unit,
        jit_state,
        native
            .verified_facts
            .as_deref()
            .expect("native execution installs verified facts"),
    );
    for (function, func) in unit.functions.iter().enumerate() {
        let profile_lines = jit_profile_report_lines(unit, jit_state, func);
        // Skip the synthetic/placeholder/trivial bodies: a body that is only the
        // lowerer's defensive `LoadUnit; Return` (≤ 2 instructions, no real work) is
        // not a "hot region" worth a block unless it accumulated profile feedback.
        // Tiny higher-order dispatchers often contain only `CallClosure; Return`,
        // and their profile is exactly the data profile-guided inlining speculation consumes.
        if func.code.len() <= 2 && profile_lines.is_empty() {
            continue;
        }
        let key = func.ordinal;
        let mut block = vec![format!("jit-report: fn `{}`", func.name)];

        // --- Native-tier verdict --------------------------------------------------
        match translate_to_native_jit(
            unit,
            func,
            native
                .verified_facts
                .as_ref()
                .and_then(|facts| facts.function(function))
                .expect("native execution installs facts for every verified function"),
            jit_state.profile(func),
            jit_state.call_count(func),
        ) {
            Some(_) => {
                if native.report_native_ok.contains(&key) {
                    block.push("  native: ok".to_string());
                } else if let Some(reason) = native.stats.unprofitable_declined_fns.get(&func.name)
                {
                    // Runtime attribution (ground truth from this run's cost-model
                    // consult) — the common "why no JIT" case now the model enforces
                    // by default. Reliable even for profile-guided PICs, which a
                    // re-derivation here would miss.
                    block.push(format!("  not native: declined by cost model — {reason}"));
                } else {
                    // Eligible but never observed running natively this run
                    // (tier-deferred, not called hot, or demoted by another gate).
                    block.push("  native: eligible (not run natively this execution)".to_string());
                }
            }
            None => {
                let reason = native_decline_reason(unit, jit_state, func);
                block.push(format!("  not native: {reason}"));
            }
        }

        // --- OSR verdict ----------------------------------------------------------
        // ACCURACY FIRST: if the function actually OSR-entered this run, the verdict is
        // `osr: entered` regardless of any static re-derivation — the recorded runtime
        // outcome is ground truth. (The OSR pipeline applies several region transforms —
        // combinator expansion, leaf inlining, string-length folding, Option/Result/
        // variant/struct scalar replacement — before the subset check, so a body with a
        // *raw* allocating string/Option op can still OSR once those passes dissolve it;
        // re-deriving that whole pipeline here would be fragile, so we trust the outcome
        // for the positive and use the cheap static re-derivation only to EXPLAIN a
        // genuine non-entry.)
        if native.report_osr_ok.contains(&key) {
            block.push("  osr: entered".to_string());
        } else {
            match detect_single_natural_loop(&func.code) {
                None => {
                    if jit_function_has_loop(&func.code) {
                        block.push(
                            "  not osr: loop shape not a single reducible natural loop".to_string(),
                        );
                    } else {
                        block.push("  not osr: no loop".to_string());
                    }
                }
                Some(lp) => {
                    let checked = native_lower_checked_payload_intrinsics_in_region(
                        &func.code, func.regs, lp.header, lp.exit,
                    );
                    let (code, header, exit) = checked
                        .as_ref()
                        .map(|(code, _, _)| (code.as_slice(), lp.header, lp.exit))
                        .unwrap_or((func.code.as_slice(), lp.header, lp.exit));
                    // A candidate loop exists but it did not OSR. Surface the first
                    // disqualifier after cheap native-only checked-payload rewrites
                    // (registry-sourced for intrinsics) as the likely cause; if the
                    // body is already in the native subset, the decline was a
                    // downstream type/marshalling reason.
                    match first_non_subset_reason(&code[header..exit]) {
                        Some(reason) => block.push(format!("  not osr: loop body {reason}")),
                        None if native.report_native_ok.contains(&key) => block.push(
                            "  osr: n/a (whole function ran native; no mid-function OSR needed)"
                                .to_string(),
                        ),
                        None => block.push(
                            "  not osr: loop not lowered (type/marshalling decline)".to_string(),
                        ),
                    }
                }
            }
        }

        block.extend(profile_lines);
        out.push(block.join("\n"));
    }
    out.insert(1, jit_native_decline_summary_block(native_decline_counts));
    out.insert(2, jit_cost_model_decline_summary_block(&native.stats));
    out
}

/// "Why did the cost model keep functions on the interpreter?" — the per-reason
/// counts of profitability declines (each reason carries its score breakdown). Empty
/// (`none`) when the model is off or nothing was declined. Distinct from the native
/// ELIGIBILITY decline summary: these regions ARE valid native code, just judged
/// not worth it (native ≈ interpreter).
#[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
pub(in crate::reg_vm) fn jit_cost_model_decline_summary_block(stats: &NativeStats) -> String {
    let mut lines = vec!["jit-report: cost-model decline summary".to_string()];
    if stats.unprofitable_decline_reasons.is_empty() {
        lines.push("  none".to_string());
        return lines.join("\n");
    }
    let mut counts: Vec<(&String, &u64)> = stats.unprofitable_decline_reasons.iter().collect();
    counts.sort_by(|(lhs_reason, lhs_count), (rhs_reason, rhs_count)| {
        rhs_count
            .cmp(lhs_count)
            .then_with(|| lhs_reason.cmp(rhs_reason))
    });
    for (reason, count) in counts {
        lines.push(format!("  {count}× {reason}"));
    }
    lines.join("\n")
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_decline_reason_counts(
    unit: &RegUnit,
    jit_state: &JitState,
    verified_facts: &VerifiedExecutableFacts,
) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::<String, u64>::new();
    for (function, func) in unit.functions.iter().enumerate() {
        let profile_lines = jit_profile_report_lines(unit, jit_state, func);
        if func.code.len() <= 2 && profile_lines.is_empty() {
            continue;
        }
        if translate_to_native_jit(
            unit,
            func,
            verified_facts
                .function(function)
                .expect("facts cover every verified function"),
            jit_state.profile(func),
            jit_state.call_count(func),
        )
        .is_none()
        {
            let reason = native_decline_reason(unit, jit_state, func);
            *counts.entry(reason).or_default() += 1;
        }
    }
    counts
}

#[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
pub(in crate::reg_vm) fn jit_native_decline_summary_block(counts: BTreeMap<String, u64>) -> String {
    let mut lines = vec!["jit-report: native decline summary".to_string()];
    if counts.is_empty() {
        lines.push("  none".to_string());
        return lines.join("\n");
    }

    let mut counts: Vec<(String, u64)> = counts.into_iter().collect();
    counts.sort_by(|(lhs_reason, lhs_count), (rhs_reason, rhs_count)| {
        rhs_count
            .cmp(lhs_count)
            .then_with(|| lhs_reason.cmp(rhs_reason))
    });
    for (reason, count) in counts {
        lines.push(format!("  {count}x {reason}"));
    }
    lines.join("\n")
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn jit_profile_report_lines(
    unit: &RegUnit,
    jit_state: &JitState,
    func: &RegFunction,
) -> Vec<String> {
    let Some(profile) = jit_state.profile(func) else {
        return Vec::new();
    };
    let function_name = |id: usize| {
        unit.functions
            .get(id)
            .map(|func| func.name.as_str())
            .unwrap_or("<unknown>")
            .to_string()
    };
    let mut lines = Vec::new();
    for (ip, instr) in func.code.iter().enumerate() {
        if !matches!(instr, RegInstr::CallClosure { .. }) {
            continue;
        }
        let Some(feedback) = profile.call_sites.get(&ip) else {
            continue;
        };
        let state = match feedback.state() {
            MonoState::Monomorphic => "monomorphic",
            MonoState::Polymorphic => "polymorphic",
            MonoState::Megamorphic => "megamorphic",
        };
        let observed = feedback
            .observed
            .iter()
            .map(|(key, count)| {
                let name = usize::try_from(*key)
                    .ok()
                    .map(&function_name)
                    .unwrap_or_else(|| "<invalid>".to_string());
                format!("{name}:{count}")
            })
            .collect::<Vec<_>>()
            .join(",");
        let mut line = format!("  profile: closure@{ip} {state} observed=[{observed}]");
        if !feedback.captures_all_scalar {
            line.push_str(" scalar-captures=false");
        }
        if let Some(target) = monomorphic_closure_inline_target(
            unit,
            func,
            Some(profile),
            jit_state.call_count(func),
            ip,
        ) {
            line.push_str(&format!(" guard={}", function_name(target)));
        } else if let Some(targets) = polymorphic_closure_inline_targets(
            unit,
            func,
            Some(profile),
            jit_state.call_count(func),
            ip,
        ) {
            let arm_count = targets.len();
            let order = targets
                .into_iter()
                .map(&function_name)
                .collect::<Vec<_>>()
                .join(",");
            line.push_str(&format!(" pic=hottest-first[{order}] pic_arms={arm_count}"));
        }
        lines.push(line);
    }
    for (ip, instr) in func.code.iter().enumerate() {
        if !matches!(
            instr,
            RegInstr::JumpIfBool { .. } | RegInstr::JumpIfIntCompare { .. }
        ) {
            continue;
        }
        let Some(feedback) = profile.branch_feedback(ip) else {
            continue;
        };
        let mut line = format!(
            "  profile: branch@{ip} taken={} fallthrough={} taken_pct={:.1} bias={}",
            feedback.taken,
            feedback.fallthrough,
            feedback.taken_percent(),
            profile.branch_bias(ip).as_str(),
        );
        if let Some(hot_target) = feedback.hot_edge() {
            let (hot_edge, cold_edge) = if hot_target {
                ("target", "fallthrough")
            } else {
                ("fallthrough", "target")
            };
            line.push_str(&format!(
                " hot_edge={hot_edge} side_exit_candidate={cold_edge}"
            ));
        }
        lines.push(line);
    }
    lines
}

/// Re-derive, observationally, the first reason whole-function native translation
/// declines `func`. Mirrors the early bails in [`translate_to_native_jit`] and then
/// scans the (leaf-inlined) reachable body for the first non-subset instruction —
/// reporting the intrinsic-level cause from the registry. Read-only.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_decline_reason(unit: &RegUnit, jit_state: &JitState, func: &RegFunction) -> String {
    if func.captures != 0 {
        return "function has captures (closure body, not a native leaf)".to_string();
    }
    // Re-run leaf inlining + aggregate scalar-replacement exactly as translation does,
    // so the reason reflects the FINAL body the native subset check sees. If any pass
    // bails, report that — these are the structural reasons the real pass declines on.
    let Some((code, _n_regs, _ip_map)) = native_inline_leaf_calls(
        unit,
        func,
        jit_state.profile(func),
        jit_state.call_count(func),
        false,
        None,
    ) else {
        return "contains a non-inlinable call (callee not native-inlinable)".to_string();
    };
    let region_exit = native_whole_function_region_exit(&code);
    let Some((code, _n_regs, _ip_map, _recipes)) =
        native_scalar_replace_results_in_region(&code, _n_regs, 0, region_exit)
    else {
        return "not scalar-replaced: Result escapes the region".to_string();
    };
    let Some((code, _n_regs, _payload, _ip_map)) = native_scalar_replace_options(&code, _n_regs)
    else {
        return "not scalar-replaced: Option escapes the region".to_string();
    };
    let region_exit = native_whole_function_region_exit(&code);
    let Some((code, _n_regs, _ip_map, _recipes)) =
        native_scalar_replace_variants_in_region(&code, _n_regs, 0, region_exit)
    else {
        return "not scalar-replaced: variant escapes the region".to_string();
    };
    let region_exit = native_whole_function_region_exit(&code);
    let Some((code, _n_regs, _ip_map, _recipes)) =
        native_scalar_replace_structs_in_region(&code, _n_regs, 0, region_exit)
    else {
        return "not scalar-replaced: struct escapes the region".to_string();
    };
    let reachable = native_reachable_instructions(&code);
    for (i, instr) in code.iter().enumerate() {
        if reachable[i]
            && !native_subset_instruction(instr)
            && let Some(reason) = instr_decline_reason(instr)
        {
            return reason;
        }
    }
    // Translation declined for a shape reason the above re-derivation doesn't pinpoint
    // (e.g. type unification conflict, param/reg count). Generic but honest.
    "outside the native subset (shape/type not lowerable)".to_string()
}

/// A report reason for why `body` is outside the native subset. Prefers the most
/// *substantive* cause — a non-pure (allocate/write/suspend/read) `CallIntrinsic`,
/// whose registry effect/notes are the real missed-opt explanation — over an
/// incidental non-subset instruction (e.g. a `LoadString` constant load that the
/// subset also rejects). Falls back to the first non-subset instruction otherwise.
/// `None` ⇒ the whole body is in the native subset.
#[cfg(all(feature = "native-jit", any(test, feature = "jit-diagnostics")))]
pub(in crate::reg_vm) fn first_non_subset_reason(body: &[RegInstr]) -> Option<String> {
    // First: a non-subset effectful intrinsic (the headline reason).
    if let Some(instr) = body.iter().find(|instr| {
        !native_subset_instruction(instr)
            && matches!(
                instr,
                RegInstr::CallIntrinsic { .. } | RegInstr::CallTypedIntrinsic { .. }
            )
            && match instr {
                RegInstr::CallIntrinsic { intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, .. } => {
                    intrinsic_descriptor(*intrinsic).effect != IntrinsicEffect::Pure
                }
                _ => false,
            }
    }) {
        return instr_decline_reason(instr);
    }
    // Otherwise: the first non-subset instruction, whatever it is.
    body.iter()
        .find(|instr| !native_subset_instruction(instr))
        .map(|instr| instr_decline_reason(instr).unwrap_or_else(|| "outside native subset".into()))
}

/// Human-readable reason a single instruction is outside the native subset, with
/// the intrinsic-level effect/notes pulled from the central [`intrinsic_descriptor`]
/// registry for `CallIntrinsic`/`CallTypedIntrinsic`. `None` for a subset instruction.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn instr_decline_reason(instr: &RegInstr) -> Option<String> {
    if native_subset_instruction(instr) {
        return None;
    }
    Some(match instr {
        RegInstr::CallIntrinsic { intrinsic, .. }
        | RegInstr::CallTypedIntrinsic { intrinsic, .. } => {
            let d = intrinsic_descriptor(*intrinsic);
            let effect = match d.effect {
                IntrinsicEffect::Pure => "pure",
                IntrinsicEffect::Read => "read",
                IntrinsicEffect::Allocate => "allocate",
            };
            format!(
                "contains CallIntrinsic {:?} (effect={}; {})",
                intrinsic, effect, d.notes
            )
        }
        RegInstr::CallClosure { .. } => {
            "contains a closure call (megamorphic / not native-inlinable)".to_string()
        }
        RegInstr::CallKnown { .. } | RegInstr::CallDynamic { .. } => {
            "contains a non-inlined call".to_string()
        }
        other => {
            // A non-call, non-subset instruction (heap construct, async, float-only
            // op the subset rejects, …). Name the opcode for the developer.
            let dbg = format!("{other:?}");
            let opcode = dbg.split([' ', '{']).next().unwrap_or("?");
            format!("contains {opcode} (outside native scalar/control subset)")
        }
    })
}

// --- Native-JIT host helpers ------------------------------------------------
//
// Heap values (structs/lists) can't live in the native tier's scalar registers,
// so the compiled code reads them by calling back into these helpers, passing an
// opaque handle (an index into a per-call table the VM fills in `try_native`).
// A read that can't be satisfied (wrong type / out of bounds) sets a bail flag;
// `try_native` checks it and re-runs the function on the interpreter, preserving
// the gap-free model. `rsscript` stays `#![forbid(unsafe_code)]`: defining these
// `extern "C"` functions and taking their addresses needs no `unsafe` — the only
// `unsafe` (the indirect call) lives in `vm-jit`.
