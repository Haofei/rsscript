//! `RegVm` dispatch loop (`drive`) and following execution methods, split from
//! `exec.rs` for module-size partitioning (a second `impl RegVm` block).

use super::*;
#[cfg(feature = "native-jit")]
use super::exec::advance_auto_osr_work;

impl RegVm {
    /// Drive the explicit call stack until the frame at depth `floor` returns
    /// (`Completed`) or a blocking operation parks the current task
    /// (`Suspended`, with the wait recorded in `self.suspension`). `floor` is
    /// the stack depth below the frame we are running.
    pub(super) fn drive(&mut self, unit: &RegUnit, floor: usize) -> Result<Outcome, EvalError> {
        'frames: loop {
            // Hoist the current (top) frame into fast locals. The instruction
            // body below only references `base`/`next_base`/`ip`/`unit`, so it is
            // byte-for-byte the recursive interpreter; only `CallKnown`/`Return`
            // (and falling off the end) manipulate the frame stack.
            let func = {
                let frame = self.frames.last().expect("active frame");
                Rc::clone(&frame.func)
            };
            let base = self.frames.last().expect("active frame").base;
            let next_base = base + func.regs;
            let mut ip = self.frames.last().expect("active frame").ip;
            #[cfg(feature = "native-jit")]
            let function_ordinal = self.frames.last().expect("active frame").function_ordinal;

            // Native JIT tier: a fresh frame whose function compiles to machine
            // code runs there first (the integer/control core). Completes exactly
            // like the `Return` arm. Falls through if not native-eligible or the
            // native code bailed on an edge.
            #[cfg(feature = "native-jit")]
            if ip == 0
                && self.native.is_some()
                // Whole-function completion returns only the function result. Heap
                // and flat parameters are synchronized through the native call
                // transaction, but reassigned scalar `mut` parameters have no
                // result channel for caller writeback yet.
                && self
                    .frames
                    .last()
                    .is_some_and(|frame| {
                        frame.mut_writeback.iter().all(|&(_, callee_reg)| {
                            !matches!(
                                self.reg(callee_reg),
                                VmValue::Unit
                                    | VmValue::Int(_)
                                    | VmValue::Float(_)
                                    | VmValue::Bool(_)
                                    | VmValue::Char(_)
                                    | VmValue::OptionNone
                                    | VmValue::OptionSomeScalar(_)
                            )
                        })
                    })
                // Inline negative check: skip the `try_native` call entirely for
                // functions already known not native-eligible (just a `Cell` read).
                && self.jit_state.native_status(&func) != NATIVE_STATUS_NOT_ELIGIBLE
            {
                match self.attempt_native(&func, base) {
                    NativeAttempt::Completed(value) => {
                        let frame = self.frames.pop().expect("active frame");
                        self.apply_mut_writeback(&frame);
                        if self.frames.len() == floor {
                            return Ok(Outcome::Completed(value));
                        }
                        self.set_reg(frame.ret_dst, value);
                        continue 'frames;
                    }
                    // precise resume: `try_native` already restored the live
                    // register window and set this frame's `ip` to the safepoint
                    // `resume_ip`. Re-enter the interpreter loop; because `ip != 0`
                    // the re-entry skips the native/tier-0 dispatch and resumes
                    // interpretation mid-function.
                    NativeAttempt::Resumed => continue 'frames,
                    // Fall through to tier-0 + the interpreter loop. The frame's
                    // `ip` is still `0`, so the function re-runs from the top.
                    NativeAttempt::Fallback => {}
                }
            }

            // If native OSR has a candidate loop, let the interpreter reach that
            // header instead of consuming the whole frame in tier-0. Whole-function
            // native has already had first refusal above; this only changes the
            // native-active, function-ineligible-but-loop-eligible shape.
            #[cfg(feature = "native-jit")]
            let osr_active = self
                .native
                .as_ref()
                .is_some_and(|native| native.auto_osr_enabled || native.eager_osr)
                && super::tier::osr_controls_unarmed_for_dispatch(&self.limits);
            #[cfg(feature = "native-jit")]
            let osr_pre_candidates = if ip == 0 && osr_active {
                self.resolve_osr_candidates(function_ordinal, &func)
            } else {
                OsrCandidates::default()
            };
            #[cfg(not(feature = "native-jit"))]
            let osr_pre_candidate: Option<usize> = None;
            #[cfg(feature = "native-jit")]
            let continuation_entries = self.native.as_mut().map(|native| {
                native
                    .continuation_entry_sets
                    .entry(function_ordinal)
                    .or_insert_with(|| Rc::new(ContinuationEntrySet::from_code(&func.code)))
                    .clone()
            });
            #[cfg(feature = "native-jit")]
            let has_continuation_region = continuation_entries.as_ref().is_some_and(|entries| {
                self.has_continuation_region(function_ordinal, &func, entries)
            });

            // Tier-0 JIT: a fresh JIT-eligible frame runs via the specializing
            // executor (which reuses the interpreter's semantics), then completes
            // exactly like the `Return` arm. Eligible functions never suspend, so
            // they are always entered at `ip == 0`.
            if self.jit_enabled
                && ip == 0
                && {
                    #[cfg(feature = "native-jit")]
                    {
                        !has_continuation_region
                    }
                    #[cfg(not(feature = "native-jit"))]
                    {
                        true
                    }
                }
                && {
                    #[cfg(feature = "native-jit")]
                    {
                        osr_pre_candidates.is_empty()
                    }
                    #[cfg(not(feature = "native-jit"))]
                    {
                        osr_pre_candidate.is_none()
                    }
                }
                && self.is_jit_eligible(func.ordinal, &func)
            {
                let value = self.run_jit(unit, &func, base)?;
                let frame = self.frames.pop().expect("active frame");
                self.apply_mut_writeback(&frame);
                if self.frames.len() == floor {
                    return Ok(Outcome::Completed(value));
                }
                self.set_reg(frame.ret_dst, value);
                continue 'frames;
            }

            // OSR auto-trigger: resolve a bounded candidate set once per function and
            // evaluation, then hoist its fixed-size value into the frame. Empty sets
            // retain the non-candidate never-taken branch. Candidate counters and
            // stable declines are keyed independently by `(function, header)`.
            //
            // `osr_eager` (set by the host-owned execution plan) keeps the forced
            // path: threshold 0, so the FIRST header hit triggers `try_osr` — this
            // preserves the differential OSR backend and the deterministic
            // forced-OSR tests. NOTE: candidacy is NOT gated on `native_status`: OSR
            // targets functions that are native-INELIGIBLE as a whole (the loop is
            // wrapped by non-native I/O); the verdict is cached in `native.osr_cache`.
            #[cfg(feature = "native-jit")]
            let (osr_candidates, osr_eager) = if osr_active {
                (
                    if osr_pre_candidates.is_empty() {
                        self.resolve_osr_candidates(function_ordinal, &func)
                    } else {
                        osr_pre_candidates
                    },
                    self.native.as_ref().is_some_and(|n| n.eager_osr),
                )
            } else {
                (OsrCandidates::default(), false)
            };
            #[cfg(not(feature = "native-jit"))]
            let _osr_candidate: Option<usize> = None;

            while let Some(instr) = func.code.get(ip) {
                #[cfg(feature = "native-jit")]
                let osr_candidate = osr_candidates
                    .iter()
                    .find(|candidate| candidate.header_ip == ip);
                // Mixed-mode continuation tier. Structural plans (including
                // negative results) are cached by function/IP, so this probe stays
                // cheap while allowing the VM to re-enter native code immediately
                // after it executes an aggregate/call/async-style barrier.
                #[cfg(feature = "native-jit")]
                if osr_candidate.is_none()
                    && let Some(entries) = continuation_entries.as_ref()
                {
                    if let Some(native) = self.native.as_mut()
                        && native.collect_stats
                    {
                        native.stats.continuation_candidate_checks =
                            native.stats.continuation_candidate_checks.saturating_add(1);
                    }
                    if !entries.contains(ip) {
                        // The overwhelmingly common interpreter instruction is
                        // not a mixed-mode entry. Keep frame state resident in
                        // locals and avoid all native preparation in this case.
                    } else {
                        // A continuation that envelops a still-counting OSR loop
                        // would jump over the loop header before its hotness state
                        // can advance. Give that loop first refusal; after a stable
                        // OSR decline (`GaveUp`) the continuation becomes eligible
                        // again, so this never strands useful native work.
                        let protects_pending_osr = if osr_candidates.is_empty() {
                            false
                        } else {
                            let plan = self.native.as_mut().and_then(|native| {
                                native
                                    .continuation_plans
                                    .entry((function_ordinal, ip))
                                    .or_insert_with(|| {
                                        detect_scalar_continuation_region(&func.code, func.regs, ip)
                                            .map(Rc::new)
                                    })
                                    .clone()
                            });
                            plan.is_some_and(|region| {
                                let direct_only_loop = region.has_backedge
                                    && func.code.iter().enumerate().all(|(region_ip, instr)| {
                                        !region.included.get(region_ip).copied().unwrap_or(false)
                                            || matches!(
                                                native_lowering_class(instr),
                                                NativeLoweringClass::Direct
                                            )
                                    });
                                direct_only_loop
                                    && osr_candidates.iter().any(|candidate| {
                                        region
                                            .included
                                            .get(candidate.header_ip)
                                            .copied()
                                            .unwrap_or(false)
                                            && !matches!(
                                                self.native.as_ref().and_then(|native| {
                                                    native.osr_triggers.get(&RegionKey {
                                                        function: function_ordinal,
                                                        header: candidate.header_ip,
                                                    })
                                                }),
                                                Some(OsrTrigger::GaveUp)
                                            )
                                    })
                            })
                        };
                        if !protects_pending_osr {
                            self.frames.last_mut().expect("active frame").ip = ip;
                            if self.try_continuation_region(function_ordinal, &func, base, ip) {
                                continue 'frames;
                            }
                        }
                    }
                }

                // At most four comparisons are performed for candidate functions.
                // Each matching header charges and probes only its own RegionKey.
                #[cfg(feature = "native-jit")]
                if let Some(candidate) = osr_candidate {
                    let region_key = RegionKey {
                        function: function_ordinal,
                        header: ip,
                    };
                    let trigger = self
                        .native
                        .as_ref()
                        .and_then(|native| native.osr_triggers.get(&region_key))
                        .copied();
                    if !matches!(trigger, Some(OsrTrigger::GaveUp)) || osr_eager {
                        let fire = if osr_eager {
                            true
                        } else {
                            match trigger {
                                Some(OsrTrigger::Counting { count, probe_cc }) => {
                                    let threshold = self
                                        .native
                                        .as_ref()
                                        .map_or(OSR_BACKEDGE_THRESHOLD, |native| {
                                            native.osr_work_threshold
                                        });
                                    let (next, reached) = advance_auto_osr_work(
                                        count,
                                        candidate.iteration_work,
                                        threshold,
                                    );
                                    if reached {
                                        true
                                    } else {
                                        if let Some(native) = self.native.as_mut() {
                                            native.osr_triggers.insert(
                                                region_key,
                                                OsrTrigger::Counting {
                                                    count: next,
                                                    probe_cc,
                                                },
                                            );
                                        }
                                        false
                                    }
                                }
                                _ => false,
                            }
                        };
                        if fire {
                            let prev_probe_cc = match trigger {
                                Some(OsrTrigger::Counting { probe_cc, .. }) => probe_cc,
                                _ => self.jit_state.call_count(&func),
                            };
                            self.frames.last_mut().expect("active frame").ip = ip;
                            if self.try_osr(function_ordinal, &func, base, ip) {
                                continue 'frames;
                            }
                            let dynamic_osr_bail = self
                                .native
                                .as_ref()
                                .is_some_and(|native| native.osr_dynamic_bail);
                            // Declined. In COUNTING (auto) mode we must NOT give up
                            // forever if the decline is only because a profile-guided
                            // closure-inline site is still PENDING — `try_osr` leaves
                            // that verdict uncached (re-probable) so a warmer retry can
                            // succeed. But we must also not re-probe forever: a
                            // structurally-present but **dynamically dead** (never-taken)
                            // `CallClosure` stays `pending` indefinitely (no profile
                            // entry), and its `call_count` never advances toward the
                            // `PROFILE_RECORD_LIMIT` freeze. So re-probe ONLY when the
                            // profile has made PROGRESS — `call_count` (the dynamic-call
                            // count) increased since the previous probe. That is
                            // intrinsically bounded: `call_count` is capped at
                            // `PROFILE_RECORD_LIMIT`, so there can be at most that many
                            // progress-resets before the profile freezes (⇒ not pending
                            // ⇒ stable) or stalls (⇒ no progress ⇒ GaveUp here).
                            //   - PENDING **and** progressed ⇒ reset (record the new
                            //     progress point as `probe_cc`).
                            //   - STABLE decline, OR pending-but-stalled/dead ⇒ `GaveUp`.
                            // EAGER mode keeps firing every header hit (the cached `None`
                            // makes a stable retry cheap; a pending profile is re-probed).
                            if !osr_eager {
                                let cc = self.jit_state.call_count(&func);
                                let profile_progressed = cc > prev_probe_cc
                                    && native_translation_pending_on_profile(
                                        &self.unit,
                                        &func,
                                        self.jit_state.profile(&func),
                                        self.jit_state.call_count(&func),
                                    );
                                if dynamic_osr_bail || profile_progressed {
                                    if let Some(native) = self.native.as_mut() {
                                        native.osr_triggers.insert(
                                            region_key,
                                            OsrTrigger::Counting {
                                                count: 0,
                                                probe_cc: cc,
                                            },
                                        );
                                    }
                                } else {
                                    if let Some(native) = self.native.as_mut() {
                                        native.osr_triggers.insert(region_key, OsrTrigger::GaveUp);
                                    }
                                }
                            }
                        }
                    }
                }
                self.tick()?;
                #[cfg(feature = "native-jit")]
                if let Some(native) = self.native.as_mut()
                    && native.collect_stats
                    && matches!(native.cost_model, NativeCostModel::Report)
                {
                    match native_lowering_class(instr) {
                        NativeLoweringClass::Direct => {
                            native.stats.interpreted_native_work =
                                native.stats.interpreted_native_work.saturating_add(1);
                        }
                        NativeLoweringClass::Helper { estimated_cost } => {
                            native.stats.interpreted_native_work = native
                                .stats
                                .interpreted_native_work
                                .saturating_add(u64::from(estimated_cost));
                        }
                        NativeLoweringClass::Yield { reason } => {
                            *native
                                .stats
                                .native_barrier_counts
                                .entry(reason.as_str().to_string())
                                .or_default() += 1;
                        }
                        NativeLoweringClass::Reject => {}
                    }
                }
                ip += 1;
                // Pure instructions (loads, arithmetic, jumps, matches, heap
                // construction, …) run through the shared `try_exec_pure`, the one
                // copy of their semantics that the JIT executor also uses — so the
                // two can never diverge. Only frame/suspension/call-shaped
                // instructions need the interpreter-specific handling below.
                match self.try_exec_pure(
                    instr,
                    base,
                    &mut ip,
                    None,
                )? {
                    PureStep::Next => {}
                    PureStep::Return(value) => {
                        let frame = self.frames.pop().expect("active frame");
                        self.apply_mut_writeback(&frame);
                        if self.frames.len() == floor {
                            return Ok(Outcome::Completed(value));
                        }
                        self.set_reg(frame.ret_dst, value);
                        continue 'frames;
                    }
                    PureStep::NotPure => match instr {
                        RegInstr::TailCallGuard => {
                            let physical_depth = self.frames.len();
                            let frame = self.frames.last_mut().expect("active frame");
                            if physical_depth
                                .saturating_add(frame.tail_calls)
                                .saturating_add(1)
                                > self.limits.max_depth
                            {
                                let max_depth = self.limits.max_depth;
                                return Err(EvalError::Runtime(format!(
                                    "recursion depth limit exceeded ({max_depth} frames)"
                                )));
                            }
                            frame.tail_calls += 1;
                        }
                        RegInstr::ResourceAcquire { resource } => {
                            self.acquire_resource_scope(base + *resource);
                        }
                        RegInstr::ResourceDrop { resource } => {
                            self.release_resource_scope(unit, base + *resource)?;
                        }
                        RegInstr::CallKnown {
                            dst,
                            function: callee_id,
                            args,
                            mut_args,
                        } => {
                            // Native recursive fast paths do not push a VM `Frame`,
                            // but the call is still one logical language frame. Check
                            // the boundary before dispatch and seed native execution
                            // with this callee depth so a base-case native call cannot
                            // slip past the same limit enforced by `push_frame`.
                            let callee_depth = self.frames.len().saturating_add(1);
                            if callee_depth > self.limits.max_depth {
                                let max_depth = self.limits.max_depth;
                                return Err(EvalError::Runtime(format!(
                                    "recursion depth limit exceeded ({max_depth} frames)"
                                )));
                            }
                            // Recursive native fast paths run Cranelift code that polls
                            // neither `step_budget` nor `cancel` (and allocates off the
                            // `allocation_budget` meter), so they are gated on all three limits
                            // being unarmed — matching the native-tier gate in
                            // `try_native`. With any limit armed, recursion runs on the
                            // interpreter / tier-0 executor, which `tick()`s every step.
                            if self.jit_enabled
                                && mut_args.is_empty()
                                && self.native_limits_unarmed()
                                && let Some(value) =
                                    self.run_jit_self_recursive_int(unit, *callee_id, base, args)?
                            {
                                self.set_reg(base + *dst, value);
                                continue;
                            }
                            // Native mutual recursion (native-call-ABI slice 4): a
                            // member of a mutually-recursive scalar-Int cycle runs via
                            // the co-compiled native group; deep recursion falls back.
                            #[cfg(feature = "native-jit")]
                            if self.jit_enabled
                                && mut_args.is_empty()
                                && self.native_limits_unarmed()
                                && let Some(value) = self
                                    .try_native_mutual_recursive_int(unit, *callee_id, base, args)
                            {
                                self.set_reg(base + *dst, value);
                                continue;
                            }
                            let callee = Rc::clone(&unit.functions[*callee_id]);
                            self.prepare_frame(next_base, callee.regs)?;
                            for (index, reg) in args.iter().enumerate() {
                                let value = self.reg(base + *reg).clone();
                                self.set_reg(next_base + index, value);
                            }
                            // `mut` args: when this frame completes, write each
                            // parameter's final value back to the caller's register
                            // so mutations propagate (caller_abs_reg, callee_abs_reg).
                            let mut_writeback = mut_args
                                .iter()
                                .map(|&pos| (base + args[pos], next_base + pos))
                                .collect();
                            // Stackless call: save our resume point, push the callee, and
                            // re-enter the driver loop instead of recursing on the host
                            // stack — so an `await` deep in this chain can later suspend it.
                            self.frames.last_mut().expect("active frame").ip = ip;
                            self.push_frame(Frame {
                                func: callee,
                                #[cfg(feature = "native-jit")]
                                function_ordinal: *callee_id,
                                ip: 0,
                                base: next_base,
                                ret_dst: base + *dst,
                                mut_writeback,
                                tail_calls: 0,
                            })?;
                            continue 'frames;
                        }
                        RegInstr::CallDynamic {
                            dst,
                            dispatch,
                            args,
                            mut_args,
                        } => {
                            // Select the concrete impl by the runtime struct type of
                            // the receiver (args[0]), then call it like `CallKnown`.
                            let receiver = self.reg(base + args[0]).clone();
                            let type_name = match &receiver {
                                VmValue::Struct(data) => Some(data.name().clone()),
                                _ => None,
                            };
                            let callee_id = type_name.as_ref().and_then(|name| {
                                dispatch
                                    .iter()
                                    .find(|(struct_name, _)| struct_name.as_str() == &**name)
                                    .map(|(_, id)| *id)
                            });
                            let Some(callee_id) = callee_id else {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM dynamic protocol dispatch found no impl for receiver `{}`.",
                                    type_name.as_deref().unwrap_or("<non-struct value>")
                                )));
                            };
                            // bounded profile collection type feedback (warm-gated + bounded inside the
                            // helper): record the resolved callee identity at this
                            // site. The dispatch DECISION above (`callee_id`) is
                            // unchanged — we only observe it. `ip` was already
                            // advanced past this instruction, so its index is
                            // `ip - 1`.
                            let callee = Rc::clone(&unit.functions[callee_id]);
                            self.prepare_frame(next_base, callee.regs)?;
                            for (index, reg) in args.iter().enumerate() {
                                let value = self.reg(base + *reg).clone();
                                self.set_reg(next_base + index, value);
                            }
                            let mut_writeback = mut_args
                                .iter()
                                .map(|&pos| (base + args[pos], next_base + pos))
                                .collect();
                            self.frames.last_mut().expect("active frame").ip = ip;
                            self.push_frame(Frame {
                                func: callee,
                                #[cfg(feature = "native-jit")]
                                function_ordinal: callee_id,
                                ip: 0,
                                base: next_base,
                                ret_dst: base + *dst,
                                mut_writeback,
                                tail_calls: 0,
                            })?;
                            continue 'frames;
                        }
                        RegInstr::SpawnTask {
                            dst,
                            function: callee_id,
                            args,
                        } => {
                            let callee = Rc::clone(&unit.functions[*callee_id]);
                            let arg_values = args
                                .iter()
                                .map(|reg| self.reg(base + *reg).clone())
                                .collect::<Vec<_>>();
                            let tid = self.create_task(callee, arg_values);
                            self.set_reg(base + *dst, task_handle_value(tid));
                        }
                        RegInstr::AwaitJoin { dst, src } => {
                            let value = self.reg(base + *src).clone();
                            match as_task_handle(&value) {
                                Some(task) => {
                                    match self.tasks.get(&task).and_then(|s| s.done.clone()) {
                                        // Already finished: take its value, no park.
                                        // Reap the slot — a handle is awaited at most
                                        // once (RS0030), so its value is now consumed
                                        // and the slot must not linger (else the task
                                        // table grows unboundedly across loop rounds,
                                        // turning the scheduler's per-step scans O(n²)).
                                        Some(result) => {
                                            self.tasks.remove(&task);
                                            self.cleanup_task_resource_scopes(unit, task)?;
                                            self.set_reg(base + *dst, result);
                                        }
                                        // Park until the joined task completes.
                                        None => {
                                            self.suspension = Some(Suspension {
                                                wait: Wait::Join { task },
                                                resume_dst: base + *dst,
                                            });
                                        }
                                    }
                                }
                                // Not a handle: `await` of an already-evaluated value.
                                None => self.set_reg(base + *dst, value),
                            }
                        }
                        RegInstr::CancelTask { src } => {
                            let value = self.reg(base + *src).clone();
                            let task = as_task_handle(&value).ok_or_else(|| {
                                EvalError::Runtime(
                                    "reg VM cancel operand did not produce a task.".to_string(),
                                )
                            })?;
                            self.cancel_task(task)?;
                        }
                        RegInstr::JoinTasks { handles } => {
                            let mut tasks = handles
                                .iter()
                                .filter_map(|reg| as_task_handle(self.reg(base + *reg)))
                                .collect::<Vec<_>>();
                            tasks.sort_unstable();
                            tasks.dedup();
                            if tasks.iter().all(|task| {
                                self.tasks.get(task).is_none_or(|slot| slot.done.is_some())
                            }) {
                                for task in tasks {
                                    self.tasks.remove(&task);
                                    self.cleanup_task_resource_scopes(unit, task)?;
                                }
                            } else {
                                self.suspension = Some(Suspension {
                                    wait: Wait::JoinAll { tasks },
                                    // Group join has no result, but scheduler
                                    // completion still requires a valid slot.
                                    resume_dst: base,
                                });
                            }
                        }
                        RegInstr::SelectWait {
                            handles,
                            winner,
                            value,
                        } => {
                            let tids = handles
                                .iter()
                                .map(|reg| {
                                    as_task_handle(self.reg(base + *reg)).ok_or_else(|| {
                                        EvalError::Runtime(
                                            "reg VM select arm did not produce a task.".to_string(),
                                        )
                                    })
                                })
                                .collect::<Result<Vec<_>, _>>()?;
                            // If an arm already finished, resolve immediately; else park.
                            let ready = tids
                                .iter()
                                .enumerate()
                                .find(|(_, tid)| {
                                    self.tasks.get(tid).is_some_and(|s| s.done.is_some())
                                })
                                .map(|(index, tid)| (index, *tid));
                            match ready {
                                Some((index, won_tid)) => {
                                    let won = self
                                        .tasks
                                        .get(&won_tid)
                                        .and_then(|s| s.done.clone())
                                        .expect("done");
                                    self.cancel_select_losers(unit, &tids, won_tid)?;
                                    self.set_reg(base + *winner, VmValue::Int(index as i64));
                                    self.set_reg(base + *value, won);
                                }
                                None => {
                                    self.suspension = Some(Suspension {
                                        wait: Wait::Select {
                                            handles: tids,
                                            winner_dst: base + *winner,
                                            value_dst: base + *value,
                                        },
                                        resume_dst: usize::MAX,
                                    });
                                }
                            }
                        }
                        RegInstr::CallExternal {
                            dst,
                            key,
                            args,
                            mut_args,
                        } => {
                            let async_call =
                                self.external_bindings.get(key).is_some_and(|function| {
                                    function.call_mode() == ProviderCallMode::Async
                                });
                            if async_call {
                                self.suspension = Some(Suspension {
                                    wait: self
                                        .start_async_external_symbol(key, args, mut_args, base)?,
                                    resume_dst: base + *dst,
                                });
                            } else {
                                let result =
                                    self.call_external_symbol(key, args, mut_args, base)?;
                                self.set_reg(base + *dst, result);
                            }
                        }
                        RegInstr::CallClosure {
                            dst,
                            closure,
                            args,
                            mut_args,
                        } => {
                            let closure = match self.reg(base + *closure) {
                                VmValue::Closure(closure) => Rc::clone(closure),
                                other => {
                                    return Err(EvalError::Runtime(format!(
                                        "reg VM expected Closure, got `{}`.",
                                        other.display()
                                    )));
                                }
                            };
                            // bounded profile collection type feedback (warm-gated + bounded inside the
                            // helper): the closure's underlying function id is its
                            // stable identity (one callee ⇒ monomorphic). Recording
                            // does not change which closure runs; `ip` already
                            // points past this instruction, so its index is
                            // `ip - 1`.
                            let result = self.call_closure_from_regs(
                                unit, &closure, args, mut_args, base, next_base,
                            )?;
                            self.set_reg(base + *dst, result);
                        }
                        RegInstr::ListFilter {
                            dst,
                            list,
                            predicate,
                        } => {
                            if let Some(next_ip) =
                                self.try_fuse_int_list_pipeline_at(unit, &func.code, ip - 1, base)?
                            {
                                ip = next_ip;
                                continue;
                            }
                            let list = expect_list_ref(self.reg(base + *list))?;
                            let predicate = expect_closure_rc(self.reg(base + *predicate))?;
                            let result = self.filter_list(unit, list, &predicate, next_base)?;
                            self.account_fresh_value_storage(&result)?;
                            self.set_reg(base + *dst, result);
                        }
                        RegInstr::ListFold {
                            dst,
                            list,
                            state,
                            folder,
                        } => {
                            let list = expect_list_ref(self.reg(base + *list))?;
                            let state = self.reg(base + *state).clone();
                            let folder = expect_closure_rc(self.reg(base + *folder))?;
                            let result = self.fold_list(unit, list, state, &folder, next_base)?;
                            self.set_reg(base + *dst, result);
                        }
                        RegInstr::ListMap { dst, list, mapper } => {
                            if let Some(next_ip) =
                                self.try_fuse_int_list_pipeline_at(unit, &func.code, ip - 1, base)?
                            {
                                ip = next_ip;
                                continue;
                            }
                            let list = expect_list_ref(self.reg(base + *list))?;
                            let mapper = expect_closure_rc(self.reg(base + *mapper))?;
                            let result = self.map_list(unit, list, &mapper, next_base)?;
                            self.account_fresh_value_storage(&result)?;
                            self.set_reg(base + *dst, result);
                        }
                        RegInstr::ListSort { dst, list } => {
                            let list = expect_list_ref(self.reg(base + *list))?.clone();
                            let mut borrowed = list.borrow_mut();
                            // Sort a flat `Ints` list in place (kind-preserving); any
                            // other kind promotes to the boxed view and sorts via the
                            // shared `VmValue` comparator (parity-identical).
                            if let TypedVec::Ints(values) = &mut *borrowed {
                                values.sort_unstable();
                            } else {
                                sort_vm_values(borrowed.as_boxed_mut())?;
                            }
                            drop(borrowed);
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::ListSortBy {
                            dst,
                            list,
                            key,
                            compare,
                        } => {
                            let values = expect_list_ref(self.reg(base + *list))?.borrow().to_vec();
                            let key = expect_closure_rc(self.reg(base + *key))?;
                            let compare = expect_closure_rc(self.reg(base + *compare))?;
                            let sorted =
                                self.sort_list_by_closure(unit, values, &key, &compare, next_base)?;
                            let sorted =
                                VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(sorted))));
                            self.account_fresh_value_storage(&sorted)?;
                            self.set_reg(base + *dst, sorted);
                        }
                        RegInstr::ListSortWith { dst, list, compare } => {
                            // Sort a detached copy first so the comparator closure can read
                            // the list without a RefCell double-borrow, then overwrite the
                            // receiver's buffer in place so `mut list` propagates.
                            let list = expect_list_ref(self.reg(base + *list))?.clone();
                            let scratch_bytes = list
                                .borrow()
                                .len()
                                .saturating_mul(std::mem::size_of::<VmValue>());
                            self.ensure_memory_available(scratch_bytes.saturating_mul(2))?;
                            self.account_bytes(scratch_bytes)?;
                            let mut values = list.borrow().to_vec();
                            let compare = expect_closure_rc(self.reg(base + *compare))?;
                            self.sort_list_with_closure(unit, &mut values, &compare, next_base)?;
                            let replacement = TypedVec::from_values(values);
                            self.account_list_storage(&replacement)?;
                            *list.borrow_mut() = replacement;
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::DequeClear { dst, deque } => {
                            expect_deque_ref(self.reg(base + *deque))?
                                .borrow_mut()
                                .clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::DequePopBack { dst, deque } => {
                            let value = expect_deque_ref(self.reg(base + *deque))?
                                .borrow_mut()
                                .pop_back()
                                .map(VmValue::some)
                                .unwrap_or(VmValue::OptionNone);
                            self.set_reg(base + *dst, value);
                        }
                        RegInstr::DequePopFront { dst, deque } => {
                            let value = expect_deque_ref(self.reg(base + *deque))?
                                .borrow_mut()
                                .pop_front() // O(1), unlike the old `Vec::remove(0)`
                                .map(VmValue::some)
                                .unwrap_or(VmValue::OptionNone);
                            self.set_reg(base + *dst, value);
                        }
                        RegInstr::DequePushBack { dst, deque, value } => {
                            let value = self.reg(base + *value).clone();
                            let deque = expect_deque_ref(self.reg(base + *deque))?.clone();
                            let mut deque = deque.borrow_mut();
                            self.reserve_deque_entry_accounted(&mut deque)?;
                            deque.push_back(value);
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::DequePushFront { dst, deque, value } => {
                            let value = self.reg(base + *value).clone();
                            let deque = expect_deque_ref(self.reg(base + *deque))?.clone();
                            let mut deque = deque.borrow_mut();
                            self.reserve_deque_entry_accounted(&mut deque)?;
                            deque.push_front(value); // O(1), unlike the old `Vec::insert(0, _)`
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SetClear { dst, set } => {
                            expect_map_ref(self.reg(base + *set))?.borrow_mut().clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SetForEach { dst, set, callback } => {
                            let set = expect_map_ref(self.reg(base + *set))?;
                            let callback = expect_closure_rc(self.reg(base + *callback))?;
                            let values = set
                                .borrow()
                                .keys()
                                .map(vm_value_from_map_key)
                                .collect::<Vec<_>>();
                            for value in values {
                                let _ = self.call_closure_one(unit, &callback, value, next_base)?;
                            }
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SetInsert { dst, set, value } => {
                            let (key, work) = map_key_from_value(self.reg(base + *value))?;
                            self.charge_work(work)?;
                            let map = expect_map_ref(self.reg(base + *set))?;
                            let mut map = map.borrow_mut();
                            if !map.contains_key(&key) {
                                self.reserve_map_entry_accounted(&mut map)?;
                            }
                            let inserted = map.insert(key, VmValue::Unit).is_none();
                            self.set_reg(base + *dst, VmValue::Bool(inserted));
                        }
                        RegInstr::SetRemove { dst, set, value } => {
                            let (key, work) = map_key_from_value(self.reg(base + *value))?;
                            self.charge_work(work)?;
                            let map = expect_map_ref(self.reg(base + *set))?;
                            let removed = map.borrow_mut().remove(&key).is_some();
                            self.set_reg(base + *dst, VmValue::Bool(removed));
                        }
                        RegInstr::SortedSetClear { dst, set } => {
                            expect_list_ref(self.reg(base + *set))?.borrow_mut().clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SortedSetInsert { dst, set, value } => {
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *set))?.clone();
                            let inserted = if sorted_contains_vm(&list.borrow(), &value)? {
                                false
                            } else if self.limits.allocation_budget.is_some() {
                                let before = list.borrow().allocated_bytes();
                                let mut replacement = list.borrow().clone_preserving_capacity();
                                let inserted = sorted_insert_vm(replacement.as_boxed_mut(), value)?;
                                let growth = replacement.allocated_bytes().saturating_sub(before);
                                self.account_bytes(growth)?;
                                *list.borrow_mut() = replacement;
                                inserted
                            } else {
                                let before = list.borrow().allocated_bytes();
                                let inserted =
                                    sorted_insert_vm(list.borrow_mut().as_boxed_mut(), value)?;
                                debug_assert!(list.borrow().allocated_bytes() >= before);
                                inserted
                            };
                            self.set_reg(base + *dst, VmValue::Bool(inserted));
                        }
                        RegInstr::SortedSetRemove { dst, set, value } => {
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *set))?;
                            let removed =
                                sorted_remove_vm(list.borrow_mut().as_boxed_mut(), &value)?;
                            self.set_reg(base + *dst, VmValue::Bool(removed));
                        }
                        RegInstr::SortedMapClear { dst, map } => {
                            expect_list_ref(self.reg(base + *map))?.borrow_mut().clear();
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SortedMapInsert {
                            dst,
                            map,
                            key,
                            value,
                        } => {
                            let key = self.reg(base + *key).clone();
                            let value = self.reg(base + *value).clone();
                            let list = expect_list_ref(self.reg(base + *map))?.clone();
                            let updates_existing =
                                sorted_map_get_in_place(&list.borrow(), &key)?.is_some();
                            let before = list.borrow().allocated_bytes();
                            let pair_bytes = 2 * std::mem::size_of::<VmValue>();
                            if self.limits.allocation_budget.is_some() {
                                let mut replacement = list.borrow().clone_preserving_capacity();
                                if updates_existing {
                                    for entry in replacement.as_boxed_mut().iter_mut() {
                                        if let VmValue::List(pair) = entry {
                                            let detached =
                                                pair.borrow().clone_preserving_capacity();
                                            *entry = VmValue::List(Rc::new(RefCell::new(detached)));
                                        }
                                    }
                                }
                                let inserted = sorted_map_insert_in_place(
                                    replacement.as_boxed_mut(),
                                    key,
                                    value,
                                )?;
                                let growth = replacement
                                    .allocated_bytes()
                                    .saturating_sub(before)
                                    .saturating_add(
                                        usize::from(inserted).saturating_mul(pair_bytes),
                                    );
                                self.account_bytes(growth)?;
                                *list.borrow_mut() = replacement;
                            } else {
                                sorted_map_insert_in_place(
                                    list.borrow_mut().as_boxed_mut(),
                                    key,
                                    value,
                                )?;
                            }
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::SortedMapRemove { dst, map, key } => {
                            let key = self.reg(base + *key).clone();
                            let list = expect_list_ref(self.reg(base + *map))?;
                            let removed =
                                sorted_map_remove_in_place(list.borrow_mut().as_boxed_mut(), &key)?;
                            self.set_reg(
                                base + *dst,
                                removed.map(VmValue::some).unwrap_or(VmValue::OptionNone),
                            );
                        }
                        RegInstr::BufferClear { dst, buffer } => {
                            expect_bytes_ref(self.reg(base + *buffer))?;
                            self.set_reg(base + *buffer, VmValue::Bytes(Rc::new(Vec::new())));
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::StringBuilderPush {
                            dst,
                            builder,
                            value,
                        } => {
                            let value = match self.reg(base + *value) {
                                VmValue::String(value) => Rc::clone(value),
                                other => {
                                    return Err(EvalError::Runtime(format!(
                                        "reg VM expected StringBuilder push value to be String, got `{}`.",
                                        other.display()
                                    )));
                                }
                            };
                            let builder = match self.reg(base + *builder) {
                                VmValue::Managed(builder) => Rc::clone(builder),
                                other => {
                                    return Err(EvalError::Runtime(format!(
                                        "reg VM expected StringBuilder, got `{}`.",
                                        other.display()
                                    )));
                                }
                            };
                            let mut builder = builder.borrow_mut();
                            let VmValue::String(text) = &mut *builder else {
                                return Err(EvalError::Runtime(
                                    "reg VM expected StringBuilder storage to be String."
                                        .to_string(),
                                ));
                            };
                            let was_shared = Rc::strong_count(text) > 1;
                            if self.limits.allocation_budget.is_some() {
                                let old_capacity = text.capacity();
                                let mut replacement = String::with_capacity(
                                    old_capacity.max(text.len().saturating_add(value.len())),
                                );
                                replacement.push_str(text);
                                replacement.push_str(value.as_str());
                                let charge = if was_shared {
                                    replacement.capacity()
                                } else {
                                    replacement.capacity().saturating_sub(old_capacity)
                                };
                                self.account_bytes(charge)?;
                                *text = Rc::new(replacement);
                            } else {
                                Rc::make_mut(text).push_str(value.as_str());
                            }
                            self.set_reg(base + *dst, VmValue::Unit);
                        }
                        RegInstr::StringBuilderFinish { dst, builder } => {
                            let builder = match self.reg(base + *builder) {
                                VmValue::Managed(builder) => Rc::clone(builder),
                                other => {
                                    return Err(EvalError::Runtime(format!(
                                        "reg VM expected StringBuilder, got `{}`.",
                                        other.display()
                                    )));
                                }
                            };
                            let builder = builder.borrow();
                            let VmValue::String(text) = &*builder else {
                                return Err(EvalError::Runtime(
                                    "reg VM expected StringBuilder storage to be String."
                                        .to_string(),
                                ));
                            };
                            let value = self.fresh_string(text.as_str().to_string())?;
                            self.set_reg(base + *dst, value);
                        }
                        RegInstr::StringConcat { dst, left, right } => {
                            let total_len = {
                                let left = expect_string_ref(self.reg(base + *left))?;
                                let right = expect_string_ref(self.reg(base + *right))?;
                                left.len().saturating_add(right.len())
                            };
                            self.ensure_memory_available(total_len)?;
                            let value = {
                                let left = expect_string_ref(self.reg(base + *left))?;
                                let right = expect_string_ref(self.reg(base + *right))?;
                                let mut value = String::with_capacity(total_len);
                                value.push_str(left);
                                value.push_str(right);
                                value
                            };
                            let value = self.fresh_string(value)?;
                            self.set_reg(base + *dst, value);
                        }
                        RegInstr::CallIntrinsic {
                            dst,
                            intrinsic,
                            args,
                        } => {
                            let storage_roots = self
                                .limits
                                .allocation_budget
                                .map(|_| self.storage_roots_from_regs(args, base));
                            let allocated_bytes_before = self.allocated_bytes;
                            let value =
                                self.call_intrinsic(unit, *intrinsic, args, base, next_base)?;
                            // A blocking intrinsic (channel/sleep) parked the task and left
                            // `resume_dst` unfilled; record where its result must land. The
                            // end-of-loop check then yields to the scheduler.
                            if let Some(suspension) = self.suspension.as_mut() {
                                suspension.resume_dst = base + *dst;
                            } else {
                                if let Some(storage_roots) = &storage_roots {
                                    self.account_result_storage_delta(
                                        &value,
                                        storage_roots,
                                        allocated_bytes_before,
                                    )?;
                                }
                                self.set_reg(base + *dst, value);
                            }
                        }
                        RegInstr::CallTypedIntrinsic {
                            dst,
                            intrinsic,
                            type_arg,
                            args,
                        } => {
                            let storage_roots = self
                                .limits
                                .allocation_budget
                                .map(|_| self.storage_roots_from_regs(args, base));
                            let allocated_bytes_before = self.allocated_bytes;
                            let value =
                                self.call_typed_intrinsic(unit, *intrinsic, type_arg, args, base)?;
                            if let Some(storage_roots) = &storage_roots {
                                self.account_result_storage_delta(
                                    &value,
                                    storage_roots,
                                    allocated_bytes_before,
                                )?;
                            }
                            self.set_reg(base + *dst, value);
                        }
                        RegInstr::TryResult { dst, src, cleanup } => {
                            let value = self.reg(base + *src).clone();
                            // `?` keeps the success payload (`Ok(x)`/`Some(x)`) and
                            // short-circuits on failure (`Err(e)`/`None`), returning
                            // that failure from the current frame. Option support
                            // mirrors Result so `?` works in `Option`-returning fns.
                            let short_circuit = match &value {
                                VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => {
                                    self.set_reg(
                                        base + *dst,
                                        value.unwrap_some().expect("Some arm yields a payload"),
                                    );
                                    None
                                }
                                VmValue::OptionNone => Some(VmValue::OptionNone),
                                _ => match result_variant_payload(&value)? {
                                    Ok(payload) => {
                                        self.set_reg(base + *dst, payload);
                                        None
                                    }
                                    Err(error) => Some(value_err(error)),
                                },
                            };
                            if let Some(return_value) = short_circuit {
                                for resource in cleanup {
                                    self.release_resource_scope(unit, base + *resource)?;
                                }
                                // Short-circuit: return the failure from the *current*
                                // frame only (pop one frame like `Return`), not out of
                                // the whole stackless driver.
                                let frame = self.frames.pop().expect("active frame");
                                self.apply_mut_writeback(&frame);
                                if self.frames.len() == floor {
                                    return Ok(Outcome::Completed(return_value));
                                }
                                self.set_reg(frame.ret_dst, return_value);
                                continue 'frames;
                            }
                        }
                        // `Return` and the rest of the pure subset are handled above by
                        // `try_exec_pure`; reaching this arm means an instruction is in
                        // neither the pure subset nor the impure arms — a lowering bug.
                        _ => unreachable!(
                            "reg VM instruction handled by neither try_exec_pure nor the interpreter: {instr:?}"
                        ),
                    },
                }
                // A blocking op (channel/sleep/join) parked the task: save the
                // resume point (`ip` already points past the instruction, so on
                // wake the scheduler writes the result into `resume_dst` and we
                // continue here) and hand control back to the scheduler.
                if self.suspension.is_some() {
                    self.frames.last_mut().expect("active frame").ip = ip;
                    return Ok(Outcome::Suspended);
                }
            }
            // Fell off the end of the function body without an explicit `Return`.
            // Lowering always appends one, so this is a defensive `Unit` return.
            let frame = self.frames.pop().expect("active frame");
            self.apply_mut_writeback(&frame);
            if self.frames.len() == floor {
                return Ok(Outcome::Completed(VmValue::Unit));
            }
            self.set_reg(frame.ret_dst, VmValue::Unit);
        }
    }
}
