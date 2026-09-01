//! `RegVm::build_osr_plan` — whole/OSR-tier plan builder, extracted from
//! try_osr in tier.rs for module-size partitioning. function_facts is taken by
//! value (cloned once per OSR attempt) to avoid a self split-borrow conflict.

use super::*;

/// Inputs to [`RegVm::build_osr_plan`], bundled so the plan builder is a
/// single-argument call. All values are computed in try_osr's preamble;
/// `function_facts` is owned (cloned once per OSR attempt) to avoid a `self`
/// split-borrow against the `&mut self` cache mutation.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct OsrPlanInputs<'a> {
    pub(in crate::reg_vm) func: &'a RegFunction,
    pub(in crate::reg_vm) native_key: usize,
    pub(in crate::reg_vm) header_ip: usize,
    pub(in crate::reg_vm) function_facts: VerifiedFunctionFacts,
    pub(in crate::reg_vm) instance: JitInstanceKey,
    pub(in crate::reg_vm) call_count: u32,
    pub(in crate::reg_vm) profile: Option<&'a FunctionProfile>,
    pub(in crate::reg_vm) emit_step: bool,
    pub(in crate::reg_vm) emit_cancel: bool,
    pub(in crate::reg_vm) emit_deadline: bool,
    pub(in crate::reg_vm) memory_armed: bool,
    pub(in crate::reg_vm) immutable_leaf_params: &'a [bool],
    pub(in crate::reg_vm) param_native_types: &'a [Option<NativeTy>],
    pub(in crate::reg_vm) osr_version_key: &'a OsrVersionKey,
    pub(in crate::reg_vm) region_key: RegionKey,
}

impl RegVm {
    #[cfg(feature = "native-jit")]
    pub(in crate::reg_vm) fn build_osr_plan(
        &mut self,
        inputs: OsrPlanInputs<'_>,
    ) -> Option<(
        vm_jit::CompiledId, usize, usize, usize,
        Vec<NativeTy>, Vec<OsrDerivedLiveIn>, Vec<OsrScalarField>, Vec<usize>,
        Vec<NativeTy>, Vec<bool>, Vec<Rc<String>>, Vec<OsrMaterializeRecipe>,
        NativeCodeTier,
    )> {
        let OsrPlanInputs {
            func,
            native_key,
            header_ip,
            function_facts,
            instance,
            call_count,
            profile,
            emit_step,
            emit_cancel,
            emit_deadline,
            memory_armed,
            immutable_leaf_params,
            param_native_types,
            osr_version_key,
            region_key,
        } = inputs;
        Some({
            // Fast path: cached and NOT at the header ⇒ nothing to do (no clone).
            if let Some(native) = self.native.as_ref()
                && let Some(entry) = native.osr_cache.get(&osr_version_key)
            {
                match entry {
                    Some(e) if e.orig_header == header_ip => {}
                    _ => return None,
                }
            }
            // Clone the unit handle before borrowing `self.native` mutably: the OSR
            // pre-pass inlines leaf `CallKnown`s, which needs the callee bodies.
            let unit = Rc::clone(&self.unit);
            let Some(native) = self.native.as_mut() else {
                return None;
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
                if native.osr_instance_count(region_key) >= MAX_JIT_INSTANCES_PER_FUNCTION
                    && !native.has_osr_instance(region_key, &instance.type_arguments)
                {
                    if native.collect_stats {
                        native.stats.static_instance_limit_fallbacks += 1;
                    }
                    return None;
                }
                if native.osr_shape_count(region_key, &instance.type_arguments)
                    >= MAX_NATIVE_SHAPE_VERSIONS
                {
                    if native.collect_stats {
                        native.stats.shape_limit_fallbacks += 1;
                    }
                    return None;
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
                        let translation_started =
                            native.collect_stats.then(std::time::Instant::now);
                        let translation = translate_osr_loop_profiled(OsrTranslationRequest {
                            function: func,
                            facts: &function_facts,
                            profile,
                            code: &func.code,
                            register_count: func.regs,
                            parameter_count: func.params,
                            capture_count: func.captures,
                            region: lp,
                            ip_map: &identity_ip_map,
                            parameter_types: &param_native_types,
                            immutable_leaf_params: &immutable_leaf_params,
                        });
                        if let Some(started) = translation_started {
                            native.stats.translation_nanos = native
                                .stats
                                .translation_nanos
                                .saturating_add(started.elapsed().as_nanos());
                        }
                        translation.and_then(|translation| {
                            let analyzed =
                                NativeRegion::osr(lp.header as u32, translation).analyze()?;
                            let NativeRegionMetadata::Osr {
                                param_tys: params,
                                derived_live_ins: derived_liveins,
                                scalar_fields,
                                reg_tys: reg_types,
                                written_regs,
                            } = analyzed.metadata()
                            else {
                                unreachable!("OSR lowering changed region kind")
                            };
                            let params = params.clone();
                            let derived_liveins = derived_liveins.clone();
                            let scalar_fields = scalar_fields.clone();
                            let reg_types = reg_types.clone();
                            let written_regs = written_regs.clone();
                            let string_literals = analyzed.string_literals().to_vec();
                            let jit_fn = analyzed.jit_fn();
                            if !osr_memory_controls_supported(jit_fn, memory_armed) {
                                return None;
                            }
                            let n_jit_regs = jit_fn.n_regs as usize;
                            // native limit accounting mem: a `ListPush*` flat-capacity growth now charges
                            // `allocation_budget` in its host helper (the only native-subset op
                            // the interpreter bills), so an allocating loop runs natively
                            // under an armed budget and bails to the interpreter at the
                            // exact over-budget push — no blanket decline needed.
                            // Step 1 cost model: an OSR loop is always a back-edge region;
                            // in `enforce` mode decline an unprofitable loop and resume on
                            // the interpreter (correctness-safe).
                            if consult_profitability(native, jit_fn, true, "osr", &func.name) {
                                return None;
                            }
                            let heap_input_regs = osr_heap_input_regs(jit_fn);
                            let admission =
                                begin_native_compile(native, 1, NativeCodeTier::Baseline)?;
                            let controls = vm_jit::RegionCompileControls {
                                step: emit_step,
                                cancel: emit_cancel,
                                deadline: emit_deadline,
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
                                            jit_fn,
                                            jit_fn
                                                .instruction_origins
                                                .get(lp.exit)
                                                .map_or(lp.exit, |origin| {
                                                    origin.resume_ip as usize
                                                }),
                                        )
                                    {
                                        debug_assert!(false, "native OSR verifier failed: {err}");
                                        if jit_native_verify_is_strict() {
                                            return None;
                                        }
                                    }
                                    record_native_compile_stats(
                                        native,
                                        id,
                                        jit_fn,
                                        NativeCodeTier::Baseline,
                                    );
                                    if native.optimized_module.is_some()
                                        && native_region_is_promotion_eligible(jit_fn)
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
                        })
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
                            ordinal: func.ordinal,
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
                            let translation_started =
                                native.collect_stats.then(std::time::Instant::now);
                            let translation = translate_osr_loop_profiled(OsrTranslationRequest {
                                function: func,
                                facts: &function_facts,
                                profile,
                                code: &code,
                                register_count: n_regs,
                                parameter_count: eff_func.params,
                                capture_count: eff_func.captures,
                                region: lp,
                                ip_map: &real_ip_map,
                                parameter_types: &param_native_types,
                                immutable_leaf_params: &immutable_leaf_params,
                            });
                            if let Some(started) = translation_started {
                                native.stats.translation_nanos = native
                                    .stats
                                    .translation_nanos
                                    .saturating_add(started.elapsed().as_nanos());
                            }
                            translation.and_then(|translation| {
                                    let analyzed = NativeRegion::osr(
                                        lp.header as u32,
                                        translation,
                                    )
                                    .analyze()?;
                                    let NativeRegionMetadata::Osr {
                                        param_tys: params,
                                        derived_live_ins: derived_liveins,
                                        scalar_fields,
                                        reg_tys: reg_types,
                                        written_regs,
                                    } = analyzed.metadata()
                                    else {
                                        unreachable!("OSR lowering changed region kind")
                                    };
                                    let params = params.clone();
                                    let derived_liveins = derived_liveins.clone();
                                    let scalar_fields = scalar_fields.clone();
                                    let reg_types = reg_types.clone();
                                    let written_regs = written_regs.clone();
                                    let string_literals = analyzed.string_literals().to_vec();
                                    let jit_fn = analyzed.jit_fn();
                                    if !osr_memory_controls_supported(jit_fn, memory_armed) {
                                        return None;
                                    }
                                    let n_jit_regs = jit_fn.n_regs as usize;
                                    // native limit accounting mem: `ListPush*` now charges `allocation_budget` in its
                                    // helper (the only native-subset billed op), so an
                                    // allocating loop runs natively and bails at the exact
                                    // over-budget push — no blanket decline needed.
                                    // Step 1 cost model: an OSR loop is always a back-edge
                                    // region; in `enforce` mode decline an unprofitable loop
                                    // and resume on the interpreter (correctness-safe).
                                    if consult_profitability(native, jit_fn, true, "osr", &func.name) {
                                        return None;
                                    }
                                    let heap_input_regs = osr_heap_input_regs(jit_fn);
                                    let admission = begin_native_compile(
                                        native,
                                        1,
                                        NativeCodeTier::Baseline,
                                    )?;
                                    let controls = vm_jit::RegionCompileControls {
                                            step: emit_step,
                                            cancel: emit_cancel,
                                            deadline: emit_deadline,
                                        };
                                    let published = analyzed
                                        .validate(&native.baseline_module)
                                        .and_then(|validated| {
                                            validated.publish(
                                                &mut native.baseline_module,
                                                controls,
                                            )
                                        });
                                    match published {
                                        Ok(published) => {
                                            let id = published.id;
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
                                                    jit_fn,
                                                    orig_exit,
                                                ) {
                                                    debug_assert!(false, "native OSR verifier failed: {err}");
                                                    if jit_native_verify_is_strict() {
                                                        return None;
                                                    }
                                                }
                                            record_native_compile_stats(
                                                native,
                                                id,
                                                jit_fn,
                                                NativeCodeTier::Baseline,
                                            );
                                            if native.optimized_module.is_some()
                                                && native_region_is_promotion_eligible(jit_fn)
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
                let profile_is_stable = {
                    {
                        true
                    }
                };
                if entry.is_some() || profile_is_stable {
                    if entry.is_some() && native.collect_stats {
                        native.stats.shape_versions += 1;
                        if osr_version_key.type_arguments.is_known()
                            && !native.has_osr_instance(
                                osr_version_key.region,
                                &osr_version_key.type_arguments,
                            )
                        {
                            native.stats.static_type_instances += 1;
                        }
                    }
                    native.osr_cache.insert(osr_version_key.clone(), entry);
                    if native
                        .osr_cache
                        .get(&osr_version_key)
                        .is_some_and(Option::is_some)
                    {
                        native
                            .osr_controllers
                            .entry(osr_version_key.clone())
                            .or_default()
                            .compiled(false);
                    }
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
                let controller = native
                    .osr_controllers
                    .entry(osr_version_key.clone())
                    .or_default();
                let first_promotion_observation = matches!(
                    controller.state,
                    RegionTierState::Baseline { native_work: 0, .. }
                );
                let promote_work = if first_promotion_observation {
                    trigger_work.max(u64::from(iteration_work))
                } else {
                    u64::from(iteration_work)
                };
                let promote =
                    controller.observe_native_work(promote_work, native.optimize_work_threshold);
                if promote
                    && !native.optimized_osr_cache.contains_key(&osr_version_key)
                    && let Some(source) = native.osr_optimization_sources.remove(&osr_version_key)
                    && let Some(admission) =
                        begin_native_compile(native, 1, NativeCodeTier::Optimized)
                {
                    let compiled = native
                        .optimized_module
                        .as_mut()
                        .expect("optimized module")
                        .compile_osr_with_controls(
                            &source.jit_fn,
                            source.header,
                            vm_jit::RegionCompileControls {
                                step: emit_step,
                                cancel: emit_cancel,
                                deadline: emit_deadline,
                            },
                        );
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
                                        source
                                            .jit_fn
                                            .instruction_origins
                                            .get(source.exit)
                                            .map_or(source.exit, |origin| {
                                                origin.resume_ip as usize
                                            }),
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
                                        native
                                            .osr_controllers
                                            .entry(osr_version_key.clone())
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
                    return None;
                }
            }
        })
    }
}
