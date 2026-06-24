use super::*;

impl RegVm {

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
        // `tick()` on every instruction. `mem_budget` is *not* in this gate: a
        // native-eligible function is a side-effect-free pure-scalar / read-heap
        // leaf that allocates no VM-managed container, so it cannot grow the
        // accounted live-set in the first place.
        if self.limits.step_budget.is_some() || self.limits.cancel.is_some() {
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
        let (id, ret_type, param_types) = {
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
                    let entry = match translate_to_native_jit(&unit, func) {
                        Some((jit_fn, ret, params)) => {
                            if native.collect_stats {
                                native.stats.translated += 1;
                            }
                            let started = native.collect_stats.then(std::time::Instant::now);
                            let compiled = native.module.compile(&jit_fn);
                            if let Some(started) = started {
                                native.stats.compile_nanos += started.elapsed().as_nanos();
                            }
                            match compiled {
                                Ok(id) => {
                                    if native.collect_stats {
                                        native.stats.compiled += 1;
                                    }
                                    // Static profitability signal: a body with an
                                    // internal back-edge (a loop) does O(n) work per
                                    // dispatch and amortizes FFI cost; a loop-free body
                                    // does O(1) and, if dispatched per loop iteration,
                                    // never does. Drives `NATIVE_NOAMORTIZE_GIVEUP`.
                                    let has_backedge = jit_function_has_loop(&func.code);
                                    Some((id, ret, params, has_backedge))
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
            let (id, ret, params, has_backedge) = match entry {
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
            (id, ret, params)
        };
        // Phase 2: marshal each argument to 64 bits per its inferred parameter
        // type. Scalars unbox directly; a `Handle` (struct/list) is registered in
        // the per-call heap table and passed as its index, for the host helpers to
        // read. (`NativeModule::call` resets its own bail flag.) A drop guard clears
        // the (possibly large) heap table on every exit path so cloned args aren't
        // retained after the call.
        let _heap_guard = JitHeapArgsGuard;
        // Heap-result return ABI (S0): clear the output table on EVERY exit too, so a
        // bailed attempt (where the host never populates it) still leaves it empty for
        // the next call — no stale heap result can be double-materialized (§7.2).
        let _heap_result_guard = JitHeapResultsGuard;
        // `args[i]` and `lens[i]` are parallel per-param words (TV2 ABI). A scalar
        // unboxes into `args[i]` (with `lens[i] = 0`); a `Handle` is a heap-table
        // index; a `FlatInt`/`FlatFloat` puts the raw buffer pointer in `args[i]`
        // and the element count in `lens[i]`.
        let n = param_types.len();
        // Reuse the pooled scratch buffers instead of allocating three `Vec`s on
        // every native call: a tiny leaf/closure dispatched once per loop iteration
        // would otherwise pay that per-call allocation churn (the actual cause of
        // marginal closure/leaf kernels running slower than the interpreter). The
        // buffers are taken from `self.native` and returned to it on every exit
        // path (`restore_scratch`), so they stay warm across calls.
        let (mut args, mut lens, mut flat_owned) = match self.native.as_mut() {
            Some(native) => (
                std::mem::take(&mut native.scratch_args),
                std::mem::take(&mut native.scratch_lens),
                std::mem::take(&mut native.scratch_flat_owned),
            ),
            None => return NativeAttempt::Fallback,
        };
        args.clear();
        args.resize(n, 0i64);
        lens.clear();
        lens.resize(n, 0i64);
        // TV2 borrow protocol: owned `Rc`s of every flat list arg, kept alive for
        // the whole call; we then pin a shared `Ref` borrow of each so no
        // `borrow_mut`/realloc can occur while native code holds the raw pointer.
        flat_owned.clear();
        // Returns the (drained) scratch buffers to the pool so the next call reuses
        // their capacity. `flat_owned` is cleared of its `Rc`s first so no list arg
        // is retained past the call.
        let restore_scratch =
            |this: &mut Self, mut args: Vec<i64>, mut lens: Vec<i64>, mut flat: Vec<Rc<RefCell<TypedVec>>>| {
                if let Some(native) = this.native.as_mut() {
                    args.clear();
                    lens.clear();
                    flat.clear();
                    native.scratch_args = args;
                    native.scratch_lens = lens;
                    native.scratch_flat_owned = flat;
                }
            };
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
                NativeTy::Handle => Some(JIT_HEAP_ARGS.with(|table| {
                    let mut table = table.borrow_mut();
                    table.push(value.clone());
                    (table.len() - 1) as i64
                })),
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
                            flat_owned.push(Rc::clone(list));
                            // Placeholder; ptr+len filled in the pin pass once all
                            // borrows are held simultaneously.
                            Some(0)
                        } else {
                            None
                        }
                    }
                    _ => None,
                },
            };
            match bits {
                Some(bits) => args[index] = bits,
                None => {
                    bail_marshal(self);
                    restore_scratch(self, args, lens, flat_owned);
                    return NativeAttempt::Fallback;
                }
            }
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
            flat_owned.iter().map(|rc| rc.borrow()).collect();
        {
            let mut next = 0usize;
            for index in 0..n {
                match param_types[index] {
                    NativeTy::FlatInt => {
                        let (ptr, len) = flat_guards[next].as_ints_slice().expect("Ints pinned");
                        next += 1;
                        args[index] = ptr as i64;
                        lens[index] = len as i64;
                    }
                    NativeTy::FlatFloat => {
                        let (ptr, len) =
                            flat_guards[next].as_floats_slice().expect("Floats pinned");
                        next += 1;
                        args[index] = ptr as i64;
                        lens[index] = len as i64;
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
                return NativeAttempt::Fallback;
            };
            let collect_stats = native_ref.collect_stats;
            let started = collect_stats.then(std::time::Instant::now);
            let result = native_ref.module.call(id, &args, &lens);
            let elapsed = started.map(|started| started.elapsed().as_nanos());
            (result, elapsed)
        };
        drop(flat_guards);
        // Return the scratch buffers (incl. the now-unborrowed `flat_owned`) to the
        // pool before the result-handling re-borrows `self.native`.
        restore_scratch(self, args, lens, flat_owned);
        let Some(native) = self.native.as_mut() else {
            return NativeAttempt::Fallback;
        };
        if let Some(elapsed) = elapsed {
            native.stats.run_nanos += elapsed;
        }
        match result {
            vm_jit::NativeOutcome::Completed(bits) => {
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
                debug_assert_ne!(
                    ret_type,
                    NativeTy::Handle,
                    "a Handle-returning native function must report CompletedHandle, not Completed",
                );
                NativeAttempt::Completed(match ret_type {
                    NativeTy::Float => VmValue::Float(f64::from_bits(bits as u64)),
                    _ => VmValue::Int(bits),
                })
            }
            // Heap-result return ABI (heap-write S0): the native call completed
            // cleanly (the vm-jit `call` reports this variant ONLY when the bail flag
            // is clear) and its result is a heap value at output-table handle `bits`.
            // S0's only producer is a pass-through: `bits` is the index of a heap
            // PARAMETER in the input table (`JIT_HEAP_ARGS`), which native returned
            // unchanged. We copy that value into the VM-owned OUTPUT table
            // (`JIT_HEAP_RESULTS`) — exercising the output-table substrate end-to-end —
            // and materialize the `VmValue` from it. §7.2: this runs ONLY on clean
            // completion; on any bail vm-jit returns `Deopt` instead and the output
            // table is left empty (and cleared by its guard on exit), so a bailed
            // attempt produces no value here and the interpreter re-run is the sole
            // source of truth — no observable effect precedes a bail.
            vm_jit::NativeOutcome::CompletedHandle(bits) => {
                if native.collect_stats {
                    native.stats.native_calls += 1;
                }
                if native.report {
                    native.report_native_ok.insert(native_key);
                }
                native.bail_counts.insert(native_key, 0);
                // Resolve the input-table handle to its heap value, then publish it
                // into the output table at index 0. A malformed/out-of-range handle
                // cannot occur for the S0 pass-through (codegen returns a real param
                // index), but if it ever did we fall back rather than panic.
                let materialized = JIT_HEAP_ARGS.with(|args| {
                    usize::try_from(bits)
                        .ok()
                        .and_then(|i| args.borrow().get(i).cloned())
                });
                match materialized {
                    Some(value) => {
                        let result = JIT_HEAP_RESULTS.with(|out| {
                            let mut out = out.borrow_mut();
                            out.push(value);
                            // Materialize from the output table (the VM-owned home of
                            // the result), proving the round-trip through it.
                            out[0].clone()
                        });
                        NativeAttempt::Completed(result)
                    }
                    None => {
                        // Treat an unresolvable handle exactly like a bail: re-run on
                        // the interpreter (always correct, no effect leaked).
                        native.record_bail(native_key, func);
                        NativeAttempt::Fallback
                    }
                }
            }
            vm_jit::NativeOutcome::Deopt { safepoint_id, live } => {
                // Bail bookkeeping is identical on both paths (precise or not):
                // a bail is still a bail for the give-up/demotion heuristic.
                if native.collect_stats {
                    native.stats.native_bails += 1;
                }
                native.record_bail(native_key, func);
                let precise_deopt = native.precise_deopt;
                // J0.2 precise resume: take it ONLY when the flag is on AND this is
                // a real, mapped safepoint (id ≥ 1 with a recorded site). Anything
                // else (flag off, anonymous/early bail, or a missing site) falls
                // back to the safe re-run-from-top default.
                if precise_deopt && safepoint_id.0 >= 1 {
                    // Re-borrow `native` immutably to look up the site; clone the
                    // `resume_ip` out so the borrow ends before we touch `self`.
                    let resume_ip = self
                        .native
                        .as_ref()
                        .and_then(|n| n.module.deopt_map(id))
                        .and_then(|m| m.sites.get(safepoint_id.0 as usize - 1))
                        .map(|site| site.resume_ip);
                    if let Some(resume_ip) = resume_ip {
                        // Restore the live register window from the captured values,
                        // SKIPPING parameter registers: their window slots
                        // `base..base+n_params` are already valid and may hold heap
                        // `VmValue`s the scalar deopt payload cannot represent.
                        let n_params = func.params;
                        for vm_jit::DeoptReg { reg, value } in live {
                            if (reg as usize) < n_params {
                                continue;
                            }
                            let vm_value = match value {
                                vm_jit::DeoptValue::Int(i) => VmValue::Int(i),
                                vm_jit::DeoptValue::Float(f) => VmValue::Float(f),
                            };
                            self.set_reg(base + reg as usize, vm_value);
                        }
                        // Resume interpretation AT the bailing instruction.
                        self.frames.last_mut().expect("active frame").ip = resume_ip as usize;
                        return NativeAttempt::Resumed;
                    }
                }
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
                // Preemption parity with `try_osr`: native loops poll neither the
                // step budget nor the cancel flag, so a function can never OSR while
                // either is armed. Resolve to `NotCandidate` WITHOUT caching it
                // permanently — a later run without the budget must re-resolve, so
                // leave the state `Unknown` (return `None` for this frame only).
                if self.limits.step_budget.is_some() || self.limits.cancel.is_some() {
                    return None;
                }
                // Cheap candidacy pre-check: does the ORIGINAL code have a single
                // analyzable natural loop? (No inline/region passes, no compile.)
                // The real native-subset/dissolvable verdict is only known after
                // `try_osr` at threshold; a detected-but-uncompilable loop simply
                // counts to threshold once, fails `try_osr`, and goes `GaveUp` —
                // bounded cost, only for functions that have a natural loop at all.
                let state = match detect_single_natural_loop(&func.code) {
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
        // Preemption parity (as in `try_native`): native loops poll neither the
        // step budget nor the cancel flag, so refuse OSR while either is armed.
        if self.limits.step_budget.is_some() || self.limits.cancel.is_some() {
            return false;
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
        let (id, trans_exit, orig_exit, n_jit_regs, _param_types, reg_types) = {
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
                let expanded = detect_single_natural_loop(&func.code).and_then(|lp_pre| {
                    native_expand_option_result_combinators_in_region(
                        &unit, func, &func.code, func.regs, lp_pre.header, lp_pre.exit,
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
                            native_status: std::cell::Cell::new(0),
                            call_count: std::cell::Cell::new(0),
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
                let entry = detect_single_natural_loop(&eff_func.code).and_then(|lp_orig| {
                native_inline_leaf_calls(&unit, eff_func, true, Some((lp_orig.header, lp_orig.exit))).and_then(
                    |(inlined_code, n_regs0, ip_map0)| {
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
                    detect_single_natural_loop(&inlined_code).and_then(|lp_sl| {
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
                    detect_single_natural_loop(&inlined_code).and_then(|lp_by| {
                    native_bytes_length_fold_in_region(
                        &inlined_code, n_regs0, lp_by.header, lp_by.exit,
                    )
                    .and_then(|(inlined_code, n_regs0, ip_map_by)| {
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
                    detect_single_natural_loop(&inlined_code).and_then(|lp_r| {
                    native_scalar_replace_results_in_region(
                        &inlined_code, n_regs0, lp_r.header, lp_r.exit,
                    )
                    .and_then(|(code_r, n_regs_r, ip_map_r)| {
                        // OSR × J3 for OPTIONS: dissolve any non-escaping scalar Option
                        // living entirely inside the region. After Result-SR the region
                        // carries only Option ops + native subset, so the strict
                        // subset-or-option gate is satisfied. Identity (no Option) ⇒
                        // unchanged.
                        let lp1 = detect_single_natural_loop(&code_r)?;
                        let (code1, n_regs1, ip_map1) = native_scalar_replace_options_in_region(
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
                        let lp_v = detect_single_natural_loop(&code1)?;
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
                        let lp_s = detect_single_natural_loop(&code2)?;
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
                        let lp_lc = detect_single_natural_loop(&code_s)?;
                        let (code, n_regs, ip_map3b) = native_loop_carried_struct_in_region(
                            &code_s, n_regs_s, lp_lc.header, lp_lc.exit,
                        )?;
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
                        detect_single_natural_loop(&code).and_then(|lp| {
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
                            let maps_into_inline = |eff_idx: usize| {
                                eff_func.code.get(eff_idx).is_some_and(|instr| {
                                    matches!(
                                        instr,
                                        RegInstr::CallKnown { .. } | RegInstr::CallClosure { .. }
                                    )
                                })
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
                            if maps_into_inline(eff_header) {
                                return None;
                            }
                            let orig_header = to_real(eff_header)?;
                            let orig_exit = if lp.exit < ip_map.len() {
                                let oe = ip_map[lp.exit];
                                if maps_into_inline(oe) {
                                    return None;
                                }
                                to_real(oe)?
                            } else if lp.exit == code.len() {
                                real_code.len()
                            } else {
                                return None;
                            };
                            translate_osr_loop(&code, n_regs, eff_func.params, eff_func.captures, lp)
                                .and_then(|(jit_fn, params, reg_types)| {
                                    let n_jit_regs = jit_fn.n_regs as usize;
                                    match native.module.compile_osr(&jit_fn, lp.header as u32) {
                                        Ok(id) => Some(OsrEntry {
                                            id,
                                            orig_header,
                                            trans_exit: lp.exit,
                                            orig_exit,
                                            n_jit_regs,
                                            param_types: params,
                                            reg_types,
                                        }),
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
                Some(Some(e)) if e.orig_header == header_ip => {
                    (
                        e.id,
                        e.trans_exit,
                        e.orig_exit,
                        e.n_jit_regs,
                        e.param_types.clone(),
                        e.reg_types.clone(),
                    )
                }
                _ => return false,
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
        let _heap_guard = JitHeapArgsGuard;
        let n_regs = func.regs;
        let mut window = vec![0i64; n_jit_regs];
        let mut lens = vec![0i64; n_jit_regs];
        // TV2 flat live-in lists: each loop-invariant typed list classified
        // `FlatInt`/`FlatFloat` is marshalled as a raw (pointer, length) pair, with a
        // shared `Ref` borrow pinned for the whole `module.call` so the backing buffer
        // cannot reallocate/mutate under native code (see the borrow protocol in
        // `try_native`). `flat_owned` keeps the `Rc`s alive; `flat_slots` records each
        // pinned list's window slot and flat kind so the pin pass can fill ptr+len once
        // all borrows are held. If the runtime value is not the expected flat kind
        // (e.g. a `Boxed`/heap-element list, or not a list at all), we BAIL the whole
        // OSR attempt — the interpreter runs the loop, always correct.
        let mut flat_owned: Vec<Rc<RefCell<TypedVec>>> = Vec::new();
        let mut flat_slots: Vec<(usize, NativeTy)> = Vec::new();
        for reg in 0..n_regs {
            if !self.written.get(base + reg).copied().unwrap_or(false) {
                continue; // not live here; native won't read it
            }
            let value = self.reg(base + reg);
            // Classify by the full per-register native type (not only params): a flat
            // live-in list may be a non-param register on the OSR path.
            let ty = reg_types.get(reg).copied().unwrap_or(NativeTy::Int);
            if matches!(ty, NativeTy::FlatInt | NativeTy::FlatFloat) {
                let want_int = ty == NativeTy::FlatInt;
                match value {
                    VmValue::List(list) => {
                        let ok = {
                            let borrowed = list.borrow();
                            if want_int {
                                borrowed.as_ints_slice().is_some()
                            } else {
                                borrowed.as_floats_slice().is_some()
                            }
                        };
                        if !ok {
                            // Not the canonical flat kind ⇒ bail this OSR attempt.
                            return false;
                        }
                        flat_owned.push(Rc::clone(list));
                        flat_slots.push((reg, ty));
                        // window[reg]/lens[reg] filled in the pin pass below.
                        continue;
                    }
                    // A flat-classified register that isn't a List at runtime ⇒ bail.
                    _ => return false,
                }
            }
            let bits = match (ty, value) {
                (NativeTy::Float, VmValue::Float(f)) => f.to_bits() as i64,
                (NativeTy::Float, VmValue::Int(i)) => (*i as f64).to_bits() as i64,
                (_, VmValue::Int(i)) => *i,
                (_, VmValue::Bool(b)) => i64::from(*b),
                (_, VmValue::Float(f)) => f.to_bits() as i64,
                // A handle (List/struct/etc.): pass its heap-table index.
                (_, other) => JIT_HEAP_ARGS.with(|table| {
                    let mut table = table.borrow_mut();
                    table.push(other.clone());
                    (table.len() - 1) as i64
                }),
            };
            window[reg] = bits;
        }

        // SAFETY (TV2 borrow protocol — the same audited core as `try_native`): pin a
        // shared `Ref` borrow of every flat list's `RefCell<TypedVec>` for the entire
        // `module.call` below. While these shared borrows are held no `borrow_mut` can
        // succeed, so the backing `Vec` cannot reallocate/mutate — the raw `as_ptr()`
        // we hand to native stays valid and immovable for the call. The list register
        // is loop-invariant (never rewritten in-loop) and the native subset has no
        // list-mutating instruction, so the loop never even attempts a write; the
        // pinned borrow is belt-and-suspenders. Every index is bounds-checked against
        // the matching `lens` slot (→ deopt on OOB). The pointer is not retained past
        // the call; `flat_guards`/`flat_owned` drop right after.
        let flat_guards: Vec<std::cell::Ref<'_, TypedVec>> =
            flat_owned.iter().map(|rc| rc.borrow()).collect();
        for (i, &(reg, kind)) in flat_slots.iter().enumerate() {
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
            window[reg] = ptr;
            lens[reg] = len as i64;
        }

        // Phase 3: run the OSR loop body natively.
        let Some(native_ref) = self.native.as_ref() else {
            return false;
        };
        let collect_stats = native_ref.collect_stats;
        let started = collect_stats.then(std::time::Instant::now);
        let result = native_ref.module.call(id, &window, &lens);
        let elapsed = started.map(|started| started.elapsed().as_nanos());
        // The pinned borrows are no longer needed once the native call returns.
        drop(flat_guards);
        if let Some(native) = self.native.as_mut() {
            if let Some(elapsed) = elapsed {
                native.stats.run_nanos += elapsed;
            }
        }

        // Phase 4: OSR-exit. The loop always exits via the `OsrExit` safepoint (a
        // deopt). Resume the interpreter at the post-loop ip with the restored
        // live-out window — the precise-deopt resume, reused verbatim.
        match result {
            vm_jit::NativeOutcome::Deopt { safepoint_id, live } if safepoint_id.0 >= 1 => {
                let resume_ip = self
                    .native
                    .as_ref()
                    .and_then(|n| n.module.deopt_map(id))
                    .and_then(|m| m.sites.get(safepoint_id.0 as usize - 1))
                    .map(|site| site.resume_ip);
                let Some(resume_ip) = resume_ip else {
                    return false;
                };
                // The OSR-exit's resume_ip (a TRANSFORMED-code ip) MUST be the loop's
                // post-loop exit ip; anything else is an OSR construction bug. Fall
                // back rather than misresume.
                if resume_ip as usize != trans_exit {
                    return false;
                }
                // Restore the live-out window, SKIPPING parameter registers (their
                // slots already hold valid — possibly heap — values the scalar deopt
                // payload cannot represent; non-param live-out regs are all scalar by
                // construction of `translate_osr_loop`) AND any J3-added tag/payload
                // register (index >= func.regs): those are strictly loop-internal,
                // dead at the exit, and have no slot in the interpreter's original
                // `func.regs`-wide window. ALSO skip any **Handle**-class register
                // (Pending #1 stored-closure broadening): a loop-internal handle (a
                // stored struct/closure fetched via `FieldHandle`/`ListGetHandle`) is
                // dead at the exit and its captured "value" is only a heap-table
                // index — writing it back as an `Int` would corrupt the interpreter
                // slot. The interpreter re-derives any value it still needs.
                let n_params = func.params;
                let n_orig_regs = func.regs;
                for vm_jit::DeoptReg { reg, value } in live {
                    if (reg as usize) < n_params || (reg as usize) >= n_orig_regs {
                        continue;
                    }
                    // Skip Handle (heap-table index) AND flat-array (raw buffer
                    // pointer bits) registers: neither's deopt payload word is a VM
                    // value; writing it back would corrupt the interpreter slot. A
                    // flat list is loop-invariant, so the original List already sits in
                    // its slot unchanged.
                    if matches!(
                        reg_types.get(reg as usize),
                        Some(&NativeTy::Handle | &NativeTy::FlatInt | &NativeTy::FlatFloat)
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
                        native.report_osr_ok.insert(func as *const RegFunction as usize);
                    }
                }
                true
            }
            // A completion (the OSR body has no `Return`) or an anonymous/early bail
            // is not a normal OSR-exit: leave the frame untouched and let the
            // interpreter run the loop. Safe and behavior-preserving.
            _ => false,
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
            ip += 1;
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
                let result = self.run_frame(unit, callee, next_base)?;
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



}
