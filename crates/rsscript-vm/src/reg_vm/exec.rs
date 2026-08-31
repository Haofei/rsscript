use super::*;
use crate::serde_json;

mod storage_accounting;

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn accumulate_osr_work(current: u32, iteration_work: u32) -> u32 {
    current.saturating_add(iteration_work)
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn advance_auto_osr_work(current: u32, iteration_work: u32, threshold: u32) -> (u32, bool) {
    let next = accumulate_osr_work(current, iteration_work);
    (next, next >= threshold)
}

impl RegVm {
    pub(super) fn usage(&self) -> crate::ExecutionUsage {
        let resources = self.provider_resources.snapshot().unwrap_or_default();
        crate::ExecutionUsage {
            steps_consumed: self.steps,
            allocation_bytes_consumed: self.allocated_bytes,
            live_memory_bytes_at_return: self.live_memory_bytes,
            peak_live_memory_bytes: self.peak_live_memory_bytes,
            output_bytes: self.stdout.len().saturating_add(self.stderr.len()),
            intrinsic_calls: self.intrinsic_calls,
            provider_calls: self.provider_calls,
            resources_created: resources.created,
            resources_cleaned: resources.cleaned,
            resource_cleanup_failures: resources.cleanup_failures,
            resources_peak_live: resources.peak_live,
            resources_live_at_return: resources.live,
            tasks_created: self.tasks_created,
            tasks_completed: self.tasks_completed,
            tasks_cancelled: self.tasks_cancelled,
            tasks_peak_live: self.tasks_peak_live,
            tasks_live_at_return: self.tasks_live,
        }
    }

    pub(super) fn cleanup_provider_resources(&mut self) -> Result<(), EvalError> {
        let mut errors = self
            .provider_resources
            .cleanup_all()
            .map_err(EvalError::Provider)?;
        if errors.is_empty() {
            Ok(())
        } else {
            Err(EvalError::Provider(errors.remove(0)))
        }
    }

    pub(super) fn new(
        unit: Rc<RegUnit>,
        entry_args: Vec<String>,
        external_bindings: HashMap<String, ExternalFunction>,
    ) -> Self {
        Self {
            jit_state: JitState::for_verified_program(&unit),
            unit,
            entry_args,
            external_bindings,
            stdout: String::new(),
            stream_stdout: false,
            stream_flushed: 0,
            stderr: String::new(),
            stack: Vec::new(),
            written: Vec::new(),
            frames: Vec::new(),
            suspension: None,
            tasks: HashMap::new(),
            ready_queue: VecDeque::new(),
            next_task_id: 0,
            current_task: 0,
            next_cancellation_id: 1,
            cancellation_flags: HashMap::new(),
            next_channel_id: 1,
            channels: HashMap::new(),
            jit_enabled: false,
            jit_force_all: false,
            limits: VmLimits::default(),
            steps: 0,
            allocated_bytes: 0,
            live_memory_bytes: 0,
            peak_live_memory_bytes: 0,
            live_memory_dirty: true,
            intrinsic_calls: 0,
            provider_calls: 0,
            tasks_created: 0,
            tasks_completed: 0,
            tasks_cancelled: 0,
            tasks_live: 0,
            tasks_peak_live: 0,
            resource_scopes: HashMap::new(),
            provider_trace: std::sync::Arc::new(
                crate::eval_types::ProviderTraceCollector::default(),
            ),
            provider_resources: ProviderResourceRegistry::new(VmLimits::default().resource_limit),
            #[cfg(feature = "native-jit")]
            native: None,
            noncapturing_closure_cache: Vec::new(),
            pure_closure_plan_cache: HashMap::new(),
        }
    }

    /// Return the canonical non-capturing `Rc<VmClosure>` for `function`,
    /// allocating it once on first use and cloning the `Rc` (refcount bump)
    /// thereafter. Only called when `unit.closure_identity_observable` is `false`,
    /// so the resulting pointer-sharing is unobservable (see the field doc on
    /// `noncapturing_closure_cache`).
    pub(super) fn cached_noncapturing_closure(&mut self, function: usize) -> Rc<VmClosure> {
        if function >= self.noncapturing_closure_cache.len() {
            self.noncapturing_closure_cache.resize(function + 1, None);
        }
        if let Some(existing) = &self.noncapturing_closure_cache[function] {
            return Rc::clone(existing);
        }
        let closure = Rc::new(VmClosure {
            function,
            captures: Vec::new(),
        });
        self.noncapturing_closure_cache[function] = Some(Rc::clone(&closure));
        self.live_memory_dirty = true;
        closure
    }

    /// Apply resource limits to this VM before it runs, replacing the bounded
    pub(super) fn set_limits(&mut self, limits: VmLimits) {
        self.provider_resources
            .set_limit(limits.resource_limit)
            .expect("fresh Provider resource registry must not be poisoned");
        self.limits = limits;
        self.live_memory_dirty = true;
    }

    /// Push a call frame, enforcing the recursion-depth cap first. `frames.len()`
    /// is the current depth; a successful push would make it `len + 1`, so we
    /// reject when that would exceed `limits.max_depth`. Centralizes the check so
    /// every frame-push site (sync `run_frame`, `CallKnown`, `CallDynamic`) is
    /// covered identically. Returns the depth error as a value, never panics.
    pub(super) fn push_frame(&mut self, frame: Frame) -> Result<(), EvalError> {
        if self.frames.len() + 1 > self.limits.max_depth {
            let max_depth = self.limits.max_depth;
            return Err(EvalError::Runtime(format!(
                "recursion depth limit exceeded ({max_depth} frames)"
            )));
        }
        self.frames.push(frame);
        Ok(())
    }

    /// Whether it is sound to use legacy native call edges that do not carry the
    /// generated region control cell. Whole-function, OSR, and continuation entry
    /// use [`Self::native_preemption_controls_supported`] instead; recursive/direct
    /// research paths remain fail-closed until their nested ABI propagates every
    /// execution meter.
    pub(super) fn native_limits_unarmed(&self) -> bool {
        self.limits.step_budget.is_none()
            && self.limits.cancel.is_none()
            && self.limits.deadline.is_none()
            && self.limits.allocation_budget.is_none()
            && self.limits.live_memory_limit.is_none()
            // Native code runs whole-function without routing intrinsic dispatch
            // through `charge_intrinsic_call`, so an armed intrinsic budget must also
            // force the interpreter/tier-0 path — otherwise the host-call cap is
            // silently unenforced once a function tiers up to native.
            && self.limits.intrinsic_call_budget.is_none()
            && self.limits.provider_call_budget.is_none()
    }

    /// Native regions enforce preemption; intrinsic/provider budgets remain on
    /// interpreter or host dispatch boundaries.
    #[cfg(feature = "native-jit")]
    pub(super) fn native_preemption_controls_supported(&self) -> bool {
        self.limits.intrinsic_call_budget.is_none() && self.limits.provider_call_budget.is_none()
    }

    /// Charge one instruction against the step budget. Always increments the
    /// fuel gauge (the single unconditional add is the whole cost when the budget
    /// is off), and — only when `limits.step_budget` is `Some` — trips once the
    /// count exceeds the limit. This is what stops an infinite loop (`while true
    /// {}`) from hanging the host: it returns a clean error instead.
    ///
    /// It is also the host-level *preemption* hook. When `limits.cancel` is
    /// `Some`, the first instruction and then every `CANCEL_POLL_INTERVAL`
    /// steps load the ambient `AtomicBool` (`Relaxed` — we only need eventual
    /// visibility, not ordering) and, if set, abort the eval with
    /// `EvalError::Runtime("evaluation cancelled")`. Checking the first
    /// instruction is required for a pre-cancelled host request: a small
    /// Artifact must not complete successfully merely because it executes fewer
    /// than one polling interval. The throttle keeps both the off path (no
    /// atomic touched at all) and the steady on path (one relaxed load per 1024
    /// instructions) cheap, so a tight loop stays fast while still being
    /// interruptible by a watchdog.
    ///
    /// Limitation: this stops the *entire* evaluation. Preemptively cancelling a
    /// single *sibling* task stuck in a tight loop — so a `select`/`task_group`
    /// can reach its winner while one branch spins — would require the scheduler
    /// to yield mid-instruction-stream (snapshot a `SavedTask` at an arbitrary
    /// `ip` and reschedule), a deeper redesign that is out of scope here. The RSS
    /// `CancellationToken` remains the cooperative, per-task mechanism (it only
    /// preempts at await points); this ambient flag is the blunt host-level kill.
    #[inline]
    pub(super) fn tick(&mut self) -> Result<(), EvalError> {
        self.refresh_live_memory_usage()?;
        self.steps += 1;
        if let Some(limit) = self.limits.step_budget
            && self.steps > limit
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::StepBudgetExceeded,
                format!("step budget exceeded ({limit} instructions)"),
            ));
        }
        if (self.steps == 1 || self.steps.is_multiple_of(CANCEL_POLL_INTERVAL))
            && let Some(flag) = self.limits.cancel.as_ref()
            && flag.is_cancelled()
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::Cancelled,
                "evaluation cancelled",
            ));
        }
        if self
            .limits
            .deadline
            .is_some_and(rsscript_operation::MonotonicDeadline::is_expired)
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::DeadlineExceeded,
                "execution deadline exceeded",
            ));
        }
        Ok(())
    }

    /// Charge host work hidden behind one bytecode operation (currently structural
    /// map/set key validation and hashing). The unarmed path is a single branch;
    /// when fuel is armed, large keys consume proportional budget instead of one
    /// nominal VM instruction.
    #[inline]
    pub(super) fn charge_work(&mut self, units: usize) -> Result<(), EvalError> {
        if self.limits.step_budget.is_none()
            && self.limits.cancel.is_none()
            && self.limits.deadline.is_none()
        {
            return Ok(());
        }
        self.steps = self
            .steps
            .saturating_add(u64::try_from(units).unwrap_or(u64::MAX));
        if let Some(limit) = self.limits.step_budget
            && self.steps > limit
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::StepBudgetExceeded,
                format!("step budget exceeded ({limit} steps)"),
            ));
        }
        if let Some(cancel) = &self.limits.cancel
            && cancel.is_cancelled()
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::Cancelled,
                "evaluation cancelled",
            ));
        }
        if self
            .limits
            .deadline
            .is_some_and(rsscript_operation::MonotonicDeadline::is_expired)
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::DeadlineExceeded,
                "execution deadline exceeded",
            ));
        }
        Ok(())
    }

    /// Charge one stdlib/runtime intrinsic dispatch against `intrinsic_call_budget`.
    /// Always increments the counter (the single unconditional add is the whole
    /// cost when the budget is off), and — only when the configured budget is
    /// `Some` — trips once the count exceeds the limit. Called once at the entry of
    /// both intrinsic dispatch functions, so it caps the number of host-library
    /// calls (file/process/net/clock/log effects all enter here) independently of
    /// raw instruction count.
    #[inline]
    pub(super) fn charge_intrinsic_call(&mut self) -> Result<(), EvalError> {
        self.intrinsic_calls += 1;
        if let Some(limit) = self.limits.intrinsic_call_budget
            && self.intrinsic_calls > limit
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::IntrinsicBudgetExceeded,
                format!("intrinsic call budget exceeded ({limit} calls)"),
            ));
        }
        Ok(())
    }

    /// Charge one call through an explicitly linked Provider symbol.
    #[inline]
    pub(super) fn charge_provider_call(&mut self) -> Result<(), EvalError> {
        self.provider_calls += 1;
        if let Some(limit) = self.limits.provider_call_budget
            && self.provider_calls > limit
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::ProviderBudgetExceeded,
                format!("provider call budget exceeded ({limit} calls)"),
            ));
        }
        Ok(())
    }

    pub(in crate::reg_vm) fn reserve_output(&self, additional: usize) -> Result<(), EvalError> {
        if let Some(limit) = self.limits.stdout_budget
            && self
                .stdout
                .len()
                .saturating_add(self.stderr.len())
                .saturating_add(additional)
                > limit
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::OutputLimitExceeded,
                format!("output budget exceeded ({limit} bytes)"),
            ));
        }
        Ok(())
    }

    pub(super) fn push_stdout(&mut self, text: &str) -> Result<(), EvalError> {
        self.reserve_output(text.len())?;
        self.stdout.push_str(text);
        if self.stream_stdout {
            self.flush_stdout_stream()?;
        }
        Ok(())
    }

    pub(super) fn push_stderr(&mut self, text: &str) -> Result<(), EvalError> {
        self.reserve_output(text.len())?;
        self.stderr.push_str(text);
        Ok(())
    }

    pub(in crate::reg_vm) fn flush_stdout_stream(&mut self) -> Result<(), EvalError> {
        if let Some(offset) = self.stdout[self.stream_flushed..].rfind('\n') {
            let end = self.stream_flushed + offset + 1;
            let chunk = &self.stdout[self.stream_flushed..end];
            let mut out = std::io::stdout();
            out.write_all(chunk.as_bytes())
                .and_then(|()| out.flush())
                .map_err(|error| EvalError::Runtime(format!("failed to stream stdout: {error}")))?;
            self.stream_flushed = end;
        }
        Ok(())
    }

    /// Whether `func` should run on the tier-0 JIT. Reads evaluation-local JIT
    /// state rather than mutating the verified program. A function is JIT'd only
    /// if (a) it is eligible — non-suspending and
    /// non-recursive — and (b) it contains a back-edge (a loop): straight-line
    /// functions gain nothing from the specializing executor, so JIT-ing them in a
    /// hot call would only add overhead. This keeps the JIT at-least-parity with
    /// the interpreter.
    pub(super) fn is_jit_eligible(&self, function_ordinal: usize, func: &RegFunction) -> bool {
        let (eligible, has_loop) = self.jit_state.tier0_analysis(function_ordinal, func);
        // Production: only JIT functions with a loop (where the specializing
        // executor pays off). `jit_force_all` (tests) JITs every eligible function
        // so the differential verifies the whole covered subset.
        eligible && (self.jit_force_all || has_loop)
    }

    /// Execute one *pure* instruction (no frame push, no suspend, no call). This
    /// is the single source of truth for the tier-0 subset's semantics, shared by
    /// the interpreter (`drive`) and the JIT executor (`run_jit`), so the two can
    /// never silently diverge. Jumps update `*ip`; `Return` is handed back to the
    /// caller (which owns frame unwinding); everything else is [`PureStep::NotPure`].
    // `VmMapKey` is interior-mutable (List/struct keys hold `Rc<RefCell<…>>`),
    // but `Map.insert`'s `retains(key)` effect makes mutating a live key
    // unreachable in well-typed RSScript, so the lint's hazard cannot occur.
    // perf-plan §1.1 (interpreter dispatch — inline the match, minimal form):
    // force this single-instruction executor to expand into BOTH callers
    // (`drive`'s hot loop and `run_jit`), so VM state (ip/base/register ptr)
    // stays in registers across the hot arms without a manual match rewrite.
    // This is an empirical probe; §1.1 may not pay off until the §1.3 hot/cold
    // split, so the win (if any) is judged against run-to-run spread per §0.4.
    #[inline(always)]
    pub(super) fn try_exec_pure(
        &mut self,
        instr: &RegInstr,
        base: usize,
        ip: &mut usize,
        branch_feedback: Option<(&RegFunction, usize)>,
    ) -> Result<PureStep, EvalError> {
        let _ = branch_feedback;
        match instr {
            RegInstr::LoadUnit { dst } => self.set_reg(base + *dst, VmValue::Unit),
            RegInstr::LoadInt { dst, value } => self.set_reg(base + *dst, VmValue::Int(*value)),
            RegInstr::LoadFloat { dst, value } => self.set_reg(base + *dst, VmValue::Float(*value)),
            RegInstr::LoadBool { dst, value } => self.set_reg(base + *dst, VmValue::Bool(*value)),
            RegInstr::Move { dst, src } => {
                let value = self.reg(base + *src).clone();
                self.set_reg(base + *dst, value);
            }
            RegInstr::DeepCopy { reg } => {
                let copied = deep_copy_value(self.reg(base + *reg));
                self.set_reg(base + *reg, copied);
            }
            RegInstr::DeepCopyElided { .. } => {
                // Elided by the `RSS_VM_ELIDE_DEEPCOPY` pass: the copy is provably redundant, so
                // share the caller's `Rc` in place (skip the deep copy). This is the win.
            }
            RegInstr::LoadString { dst, value } => {
                self.set_reg(base + *dst, VmValue::String(Rc::clone(value)));
            }
            RegInstr::LoadChar { dst, value } => {
                self.set_reg(base + *dst, VmValue::Char(*value));
            }
            RegInstr::Manage { dst, src } => {
                let value = self.reg(base + *src).clone();
                // `manage` wraps a value in a shared mutable cell so it can be
                // retained (stored in a collection/field) and mutated in place.
                // Immutable scalars cannot be mutated in place and have value (not
                // reference) semantics, so wrapping them is a no-op that only leaks
                // an opaque `Managed` into reads — borrow-returning accessors
                // (`String`/`Bytes`/`Json`) can't peel it. Store them directly.
                let managed = if value.is_immutable_scalar() {
                    value
                } else {
                    VmValue::Managed(Rc::new(RefCell::new(value)))
                };
                self.set_reg(base + *dst, managed);
            }
            RegInstr::GetField {
                dst,
                base: obj,
                name,
            } => {
                let value = read_field_ref(self.reg(base + *obj), name)?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::SetField {
                dst,
                base: obj,
                name,
                value,
            } => {
                let obj_reg = base + *obj;
                let new_value = self.reg(base + *value).clone();
                // Take the struct out so its `Rc` count reflects only other live
                // holders; `write_field_value_owned` then mutates in place when
                // uniquely owned, or copy-on-writes when shared.
                let current = self.take_reg(obj_reg);
                let updated = write_field_value_owned(current, name, new_value)?;
                self.set_reg(obj_reg, updated);
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::GetFieldSlot {
                dst,
                base: obj,
                slot,
            } => {
                let value = read_field_slot(self.reg(base + *obj), *slot)?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::SetFieldSlot {
                dst,
                base: obj,
                slot,
                value,
            } => {
                let obj_reg = base + *obj;
                let new_value = self.reg(base + *value).clone();
                let current = self.take_reg(obj_reg);
                let updated = write_field_slot_owned(current, *slot, new_value)?;
                self.set_reg(obj_reg, updated);
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::MakeStruct {
                dst,
                layout,
                fields,
            } => {
                // Hot path: the layout is interned once at lowering time, so this is
                // a refcount bump + a field-value gather — no per-construction
                // `(name, field_names)` hashing. Field values are in canonical slot
                // order (the lowerer canonicalized them to match the layout).
                let mut values: Vec<VmValue> = Vec::with_capacity(fields.len());
                for (_field, reg) in fields {
                    values.push(self.reg(base + *reg).clone());
                }
                let roots = self.limits.allocation_budget.map(|_| {
                    self.storage_roots_from_regs(
                        &fields.iter().map(|(_, r)| *r).collect::<Vec<_>>(),
                        base,
                    )
                });
                let value =
                    VmValue::Struct(Rc::new(VmStruct::with_layout(Rc::clone(layout), values)));
                if let Some(roots) = &roots {
                    self.account_result_storage_delta(&value, roots, self.allocated_bytes)?;
                }
                self.set_reg(base + *dst, value);
            }
            RegInstr::MakeVariant {
                dst,
                layout,
                fields,
            } => {
                let mut values: Vec<VmValue> = Vec::with_capacity(fields.len());
                for (_field, reg) in fields {
                    values.push(self.reg(base + *reg).clone());
                }
                let roots = self.limits.allocation_budget.map(|_| {
                    self.storage_roots_from_regs(
                        &fields.iter().map(|(_, r)| *r).collect::<Vec<_>>(),
                        base,
                    )
                });
                let value =
                    VmValue::Variant(Rc::new(VmStruct::with_layout(Rc::clone(layout), values)));
                if let Some(roots) = &roots {
                    self.account_result_storage_delta(&value, roots, self.allocated_bytes)?;
                }
                self.set_reg(base + *dst, value);
            }
            RegInstr::MakeList { dst, items } => {
                let roots = self
                    .limits
                    .allocation_budget
                    .map(|_| self.storage_roots_from_regs(items, base));
                let allocated_bytes_before = self.allocated_bytes;
                let mut list = Vec::with_capacity(items.len());
                for reg in items {
                    list.push(self.reg(base + *reg).clone());
                }
                let typed = TypedVec::from_values(list);
                let value = VmValue::List(Rc::new(RefCell::new(typed)));
                if let Some(roots) = &roots {
                    self.account_result_storage_delta(&value, roots, allocated_bytes_before)?;
                }
                self.set_reg(base + *dst, value);
            }
            RegInstr::MakeObject { dst, fields } => {
                let roots = self.limits.allocation_budget.map(|_| {
                    self.storage_roots_from_regs(
                        &fields.iter().map(|(_, r)| *r).collect::<Vec<_>>(),
                        base,
                    )
                });
                let allocated_bytes_before = self.allocated_bytes;
                let mut object = serde_json::Map::new();
                for (field, reg) in fields {
                    let value = vm_value_to_json_literal(self.reg(base + *reg))?;
                    object.insert(field.clone(), value);
                }
                let value = VmValue::Json(Rc::new(serde_json::Value::Object(object)));
                if let Some(roots) = &roots {
                    self.account_result_storage_delta(&value, roots, allocated_bytes_before)?;
                }
                self.set_reg(base + *dst, value);
            }
            RegInstr::MakeMap { dst, entries } => {
                let entry_regs = entries
                    .iter()
                    .flat_map(|(key, value)| [*key, *value])
                    .collect::<Vec<_>>();
                let roots = self
                    .limits
                    .allocation_budget
                    .map(|_| self.storage_roots_from_regs(&entry_regs, base));
                let allocated_bytes_before = self.allocated_bytes;
                let projected_entries = if entries.is_empty() {
                    0
                } else {
                    entries.len().saturating_mul(2).saturating_add(3)
                };
                self.ensure_memory_available(projected_entries.saturating_mul(MAP_ENTRY_BYTES))?;
                let mut map = ValueMap::with_capacity_and_hasher(entries.len(), Default::default());
                self.account_bytes(map.capacity() * MAP_ENTRY_BYTES)?;
                for (key, value) in entries {
                    let (key, work) = map_key_from_value(self.reg(base + *key))?;
                    self.charge_work(work)?;
                    map.insert(key, self.reg(base + *value).clone());
                }
                let value = VmValue::Map(Rc::new(RefCell::new(map)));
                if let Some(roots) = &roots {
                    self.account_result_storage_delta(&value, roots, allocated_bytes_before)?;
                }
                self.set_reg(base + *dst, value);
            }
            RegInstr::AddInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Add,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::SubInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Subtract,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::MulInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Multiply,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::DivInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Divide,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::ModInt { dst, lhs, rhs } => {
                let value = eval_numeric_binary(
                    BinaryOp::Modulo,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, value);
            }
            RegInstr::BitAndInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                self.set_reg(base + *dst, VmValue::Int(l & r));
            }
            RegInstr::BitOrInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                self.set_reg(base + *dst, VmValue::Int(l | r));
            }
            RegInstr::BitXorInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                self.set_reg(base + *dst, VmValue::Int(l ^ r));
            }
            RegInstr::ShiftLeftInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                let bits = checked_shift_count(r)?;
                self.set_reg(base + *dst, VmValue::Int(l << bits));
            }
            RegInstr::ShiftRightInt { dst, lhs, rhs } => {
                let l = expect_int_ref(self.reg(base + *lhs))?;
                let r = expect_int_ref(self.reg(base + *rhs))?;
                let bits = checked_shift_count(r)?;
                self.set_reg(base + *dst, VmValue::Int(l >> bits));
            }
            RegInstr::LessInt { dst, lhs, rhs } => {
                let value = eval_numeric_compare(
                    RegIntCompare::Less,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, VmValue::Bool(value));
            }
            RegInstr::LessEqualInt { dst, lhs, rhs } => {
                let value = eval_numeric_compare(
                    RegIntCompare::LessEqual,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, VmValue::Bool(value));
            }
            RegInstr::GreaterInt { dst, lhs, rhs } => {
                let value = eval_numeric_compare(
                    RegIntCompare::Greater,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, VmValue::Bool(value));
            }
            RegInstr::GreaterEqualInt { dst, lhs, rhs } => {
                let value = eval_numeric_compare(
                    RegIntCompare::GreaterEqual,
                    self.reg(base + *lhs),
                    self.reg(base + *rhs),
                )?;
                self.set_reg(base + *dst, VmValue::Bool(value));
            }
            RegInstr::Equal { dst, lhs, rhs } => {
                let eq = self.reg(base + *lhs) == self.reg(base + *rhs);
                self.set_reg(base + *dst, VmValue::Bool(eq));
            }
            RegInstr::NotEqual { dst, lhs, rhs } => {
                let ne = self.reg(base + *lhs) != self.reg(base + *rhs);
                self.set_reg(base + *dst, VmValue::Bool(ne));
            }
            RegInstr::Jump { target } => *ip = *target,
            RegInstr::JumpIfBool {
                cond,
                expected,
                target,
            } => {
                let taken = expect_bool_ref(self.reg(base + *cond))? == *expected;
                if taken {
                    *ip = *target;
                }
            }
            RegInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            } => {
                let l = self.reg(base + *lhs);
                let r = self.reg(base + *rhs);
                let taken = eval_numeric_compare(*op, l, r)? == *expected;
                if taken {
                    *ip = *target;
                }
            }
            RegInstr::MakeSome { dst, value } => {
                let value = self.reg(base + *value).clone();
                self.set_reg(base + *dst, VmValue::some(value));
            }
            RegInstr::LoadNone { dst } => {
                self.set_reg(base + *dst, VmValue::OptionNone);
            }
            RegInstr::MakeClosure {
                dst,
                function: callee,
                captures,
            } => {
                // Non-capturing closures of the same function are value-identical
                // every time, so when closure identity is provably unobservable
                // (see `noncapturing_closure_cache`) we reuse one cached `Rc`
                // instead of heap-allocating a fresh one each iteration.
                let closure = if captures.is_empty() && !self.unit.closure_identity_observable {
                    self.cached_noncapturing_closure(*callee)
                } else {
                    let mut captured = Vec::with_capacity(captures.len());
                    for reg in captures {
                        captured.push(self.reg(base + *reg).clone());
                    }
                    Rc::new(VmClosure {
                        function: *callee,
                        captures: captured,
                    })
                };
                self.set_reg(base + *dst, VmValue::Closure(closure));
            }
            RegInstr::MatchOption {
                src,
                some_ip,
                none_ip,
            } => match self.reg(base + *src) {
                VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => *ip = *some_ip,
                VmValue::OptionNone => *ip = *none_ip,
                other => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM Option match expected Option, got `{}`.",
                        other.display()
                    )));
                }
            },
            RegInstr::MatchResult { src, ok_ip, err_ip } => match self.reg(base + *src) {
                VmValue::Variant(data) if data.name().as_ref() == "Ok" => *ip = *ok_ip,
                VmValue::Variant(data) if data.name().as_ref() == "Err" => *ip = *err_ip,
                other => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM Result match expected Result, got `{}`.",
                        other.display()
                    )));
                }
            },
            RegInstr::MatchVariant {
                src,
                expected,
                match_ip,
                else_ip,
            } => match self.reg(base + *src) {
                VmValue::Variant(data) if data.name().as_ref() == expected.as_str() => {
                    *ip = *match_ip
                }
                VmValue::Variant(_) => *ip = *else_ip,
                other => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM variant match expected `{expected}`, got `{}`.",
                        other.display()
                    )));
                }
            },
            RegInstr::MatchMapGet {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let (key, work) = map_key_from_value(self.reg(base + *key))?;
                self.charge_work(work)?;
                if let Some(value) = map.borrow().get(&key).cloned() {
                    self.set_reg(base + *value_dst, value);
                    *ip = *some_ip;
                } else {
                    *ip = *none_ip;
                }
            }
            RegInstr::MatchSortedMapGet {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                let map = expect_list_ref(self.reg(base + *map))?;
                let key = self.reg(base + *key).clone();
                if let Some(value) = sorted_map_get_in_place(&map.borrow(), &key)? {
                    self.set_reg(base + *value_dst, value);
                    *ip = *some_ip;
                } else {
                    *ip = *none_ip;
                }
            }
            RegInstr::UnwrapSome { dst, src } => {
                let value = match self.reg(base + *src) {
                    some @ (VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_)) => {
                        some.unwrap_some().expect("Some arm yields a payload")
                    }
                    other => {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Some binding expected Some, got `{}`.",
                            other.display()
                        )));
                    }
                };
                self.set_reg(base + *dst, value);
            }
            RegInstr::UnwrapVariantValue { dst, src, expected } => {
                let value = match self.reg(base + *src) {
                    VmValue::Variant(data) if data.name().as_ref() == expected.as_str() => data
                        .get("value")
                        .cloned()
                        .or_else(|| {
                            (data.fields.len() == 1)
                                .then(|| data.fields.first().cloned())
                                .flatten()
                        })
                        .ok_or_else(|| {
                            EvalError::Runtime(format!(
                                "reg VM `{expected}` variant is missing value."
                            ))
                        })?,
                    other => {
                        return Err(EvalError::Runtime(format!(
                            "reg VM expected `{expected}` variant, got `{}`.",
                            other.display()
                        )));
                    }
                };
                self.set_reg(base + *dst, value);
            }
            RegInstr::RuntimeError { message } => {
                return Err(EvalError::Runtime(message.clone()));
            }
            // Collection get/set/index ops: pure (no frame push, no closure
            // call), so they belong to the tier-0 subset. Closure-driven
            // collection ops (map/filter/fold/sort-by) stay on the interpreter.
            // TV2.2 hot/cold split (perf-plan §1.3): `try_exec_pure` is
            // `#[inline(always)]` and inlines into `drive()`'s hot dispatch loop.
            // TV1+TV2 enlarged these list arms (TypedVec dispatch + accounting),
            // which bloated the inlined loop and taxed I-cache/regalloc on EVERY
            // kernel — including non-list scalar ones. Each heavy list arm is now a
            // thin call into an `#[inline(never)]` cold helper so the inlined hot
            // loop shrinks back to its scalar core (arith/compare/branch/move/load).
            RegInstr::ListGet { dst, list, index } => {
                self.exec_list_get(base, *dst, *list, *index)?
            }
            RegInstr::ListLen { dst, list } => self.exec_list_len(base, *dst, *list)?,
            RegInstr::ListPush { dst, list, value } => {
                self.exec_list_push(base, *dst, *list, *value)?
            }
            RegInstr::ListAppend { dst, list, values } => {
                self.exec_list_append(base, *dst, *list, *values)?
            }
            RegInstr::ListClear { dst, list } => self.exec_list_clear(base, *dst, *list)?,
            RegInstr::ListPop { dst, list } => self.exec_list_pop(base, *dst, *list)?,
            RegInstr::ListRemoveAt { dst, list, index } => {
                self.exec_list_remove_at(base, *dst, *list, *index)?
            }
            RegInstr::ListSet {
                dst,
                list,
                index,
                value,
            } => self.exec_list_set(base, *dst, *list, *index, *value)?,
            RegInstr::MapGet { dst, map, key } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let (key, work) = map_key_from_value(self.reg(base + *key))?;
                self.charge_work(work)?;
                let value = map
                    .borrow()
                    .get(&key)
                    .cloned()
                    .map(VmValue::some)
                    .unwrap_or(VmValue::OptionNone);
                self.set_reg(base + *dst, value);
            }
            RegInstr::MapClear { dst, map } => {
                expect_map_ref(self.reg(base + *map))?.borrow_mut().clear();
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::MapInsert {
                dst,
                map,
                key,
                value,
            } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let (key, work) = map_key_from_value(self.reg(base + *key))?;
                self.charge_work(work)?;
                let value = self.reg(base + *value).clone();
                let mut map = map.borrow_mut();
                if !map.contains_key(&key) {
                    self.reserve_map_entry_accounted(&mut map)?;
                }
                map.insert(key, value);
                self.set_reg(base + *dst, VmValue::Unit);
            }
            RegInstr::MapInsertOld {
                dst,
                map,
                key,
                value,
            } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let (key, work) = map_key_from_value(self.reg(base + *key))?;
                self.charge_work(work)?;
                let value = self.reg(base + *value).clone();
                let mut map = map.borrow_mut();
                if !map.contains_key(&key) {
                    self.reserve_map_entry_accounted(&mut map)?;
                }
                let old = map.insert(key, value);
                self.set_reg(
                    base + *dst,
                    old.map(VmValue::some).unwrap_or(VmValue::OptionNone),
                );
            }
            RegInstr::MapRemove { dst, map, key } => {
                let map = expect_map_ref(self.reg(base + *map))?;
                let (key, work) = map_key_from_value(self.reg(base + *key))?;
                self.charge_work(work)?;
                let old = map.borrow_mut().remove(&key);
                self.set_reg(
                    base + *dst,
                    old.map(VmValue::some).unwrap_or(VmValue::OptionNone),
                );
            }
            RegInstr::Return { src } => {
                // Clone rather than move the return value out of its register. A
                // `mut` parameter register can be BOTH the return source and a
                // `mut_writeback` target (`fn f(i: mut Int) -> Int { return i }`):
                // moving would clear the register, so the writeback that runs after
                // the frame pops would read an uninitialized slot (panic) or, worse,
                // write `Unit` back to the caller. Cloning leaves the register live
                // for `apply_mut_writeback`; the leftover copy is dropped when the
                // frame window is reused (see `prepare_frame`). The extra clone is a
                // scalar/`Rc` bump on the interpreter-tier return path only.
                return Ok(PureStep::Return(self.reg(base + *src).clone()));
            }
            // Anything else is outside the pure subset; the caller handles it.
            _ => return Ok(PureStep::NotPure),
        }
        Ok(PureStep::Next)
    }

    // ── TV2.2 cold list-opcode bodies (perf-plan §1.3 hot/cold split) ──
    // These are the heavy/cold bodies the TV1+TV2 TypedVec work added to the
    // `try_exec_pure` list arms. `try_exec_pure` is `#[inline(always)]` and
    // inlines into `drive()`'s hot dispatch loop; keeping these bodies inline
    // bloated the loop and taxed I-cache/regalloc on every kernel (including
    // non-list scalar ones). Marked `#[inline(never)]` so each arm is a thin
    // call and the inlined hot loop stays at its scalar core. Semantics are
    // byte-for-byte identical to the previous inline arms.

    #[inline(never)]
    pub(super) fn exec_list_get(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        index: usize,
    ) -> Result<(), EvalError> {
        let list = expect_list_ref(self.reg(base + list))?;
        let index = expect_usize_ref(self.reg(base + index))?;
        let value = list.borrow().get(index).ok_or_else(|| {
            EvalError::Runtime(format!("reg VM List.get index {index} out of bounds."))
        })?;
        self.set_reg(base + dst, value);
        Ok(())
    }

    #[inline(never)]
    pub(super) fn exec_list_len(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
    ) -> Result<(), EvalError> {
        let len = expect_list_ref(self.reg(base + list))?.borrow().len();
        self.set_reg(base + dst, VmValue::Int(len as i64));
        Ok(())
    }

    #[inline(never)]
    pub(super) fn exec_list_push(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        value: usize,
    ) -> Result<(), EvalError> {
        // TV2.1 hot path: a single mutable borrow that fuses the typed direct
        // push with amortized capacity-growth accounting. The push operates
        // straight on the flat `i64`/`f64` buffer with no intermediate `VmValue`
        // materialization, and `account_bytes` is only reached on the (geometric,
        // amortized O(1)) reallocations — not once per element. The adversarial
        // `while true { list.push(x) }` still trips the ceiling: capacity (>= len)
        // growth is charged, so the bound is conservative.
        let list = expect_list_ref(self.reg(base + list))?;
        let value = self.reg(base + value).clone();
        if self.limits.allocation_budget.is_some() {
            let before = list.borrow().allocated_bytes();
            let mut replacement = list.borrow().clone_preserving_capacity();
            replacement.checked_push_accounted(value).map_err(|v| {
                EvalError::Runtime(format!(
                    "reg VM List.push element kind mismatch (got `{}`).",
                    v.display()
                ))
            })?;
            self.account_bytes(replacement.allocated_bytes().saturating_sub(before))?;
            *list.borrow_mut() = replacement;
        } else {
            list.borrow_mut()
                .checked_push_accounted(value)
                .map_err(|v| {
                    EvalError::Runtime(format!(
                        "reg VM List.push element kind mismatch (got `{}`).",
                        v.display()
                    ))
                })?;
        }
        self.set_reg(base + dst, VmValue::Unit);
        Ok(())
    }

    #[inline(never)]
    pub(super) fn exec_list_append(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        values: usize,
    ) -> Result<(), EvalError> {
        // In-place to mirror `List.append(mut list, ...)`: clone the source first
        // (handles append-to-self), then extend the receiver's existing buffer so
        // a `mut` param propagates.
        let append_values = expect_list_ref(self.reg(base + values))?.borrow().clone();
        // Account against the *destination's* real layout, not the source's: a flat
        // `Ints`/`Floats` source extended into a `Boxed` receiver stores 16 B
        // `VmValue` slots, which `extend_accounted` bills correctly (the old
        // source-`elem_bytes` charge under-counted that mixed-layout case).
        let list = expect_list_ref(self.reg(base + list))?;
        if self.limits.allocation_budget.is_some() {
            let before = list.borrow().allocated_bytes();
            let mut replacement = list.borrow().clone_preserving_capacity();
            replacement.extend_accounted(append_values);
            self.account_bytes(replacement.allocated_bytes().saturating_sub(before))?;
            *list.borrow_mut() = replacement;
        } else {
            list.borrow_mut().extend_accounted(append_values);
        }
        self.set_reg(base + dst, VmValue::Unit);
        Ok(())
    }

    #[inline(never)]
    pub(super) fn exec_list_clear(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
    ) -> Result<(), EvalError> {
        expect_list_ref(self.reg(base + list))?.borrow_mut().clear();
        self.set_reg(base + dst, VmValue::Unit);
        Ok(())
    }

    #[inline(never)]
    pub(super) fn exec_list_pop(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
    ) -> Result<(), EvalError> {
        let value = expect_list_ref(self.reg(base + list))?
            .borrow_mut()
            .pop()
            .map(VmValue::some)
            .unwrap_or(VmValue::OptionNone);
        self.set_reg(base + dst, value);
        Ok(())
    }

    #[inline(never)]
    pub(super) fn exec_list_remove_at(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        index: usize,
    ) -> Result<(), EvalError> {
        let index = expect_int_ref(self.reg(base + index))?;
        let list = expect_list_ref(self.reg(base + list))?.clone();
        let mut borrowed = list.borrow_mut();
        let value = if index < 0 || index as usize >= borrowed.len() {
            VmValue::OptionNone
        } else {
            VmValue::some(borrowed.remove(index as usize))
        };
        drop(borrowed);
        self.set_reg(base + dst, value);
        Ok(())
    }

    #[inline(never)]
    pub(super) fn exec_list_set(
        &mut self,
        base: usize,
        dst: usize,
        list: usize,
        index: usize,
        value: usize,
    ) -> Result<(), EvalError> {
        let index = expect_int_ref(self.reg(base + index))?;
        let new_value = self.reg(base + value).clone();
        let list = expect_list_ref(self.reg(base + list))?.clone();
        let mut borrowed = list.borrow_mut();
        if index < 0 || index as usize >= borrowed.len() {
            return Err(EvalError::Runtime(format!(
                "reg VM List.set index {index} out of bounds for length {}.",
                borrowed.len()
            )));
        }
        borrowed
            .checked_set(index as usize, new_value)
            .map_err(|v| {
                EvalError::Runtime(format!(
                    "reg VM List.set element kind mismatch (got `{}`).",
                    v.display()
                ))
            })?;
        drop(borrowed);
        self.set_reg(base + dst, VmValue::Unit);
        Ok(())
    }

    /// Grow the shared register stack so that `stack[..upto]` is addressable.
    /// The stack only ever grows; frames are reused in place.
    pub(super) fn ensure_regs(&mut self, upto: usize) -> Result<(), EvalError> {
        if self.stack.len() < upto {
            let grew = upto - self.stack.len();
            self.stack.resize(upto, VmValue::Unit);
            self.written.resize(upto, false);
            // Account the shared register stack's growth: one `VmValue` slot plus
            // the `written` bool per new register. Deep recursion that slips under
            // the depth cap (huge per-frame register windows) is still bounded.
            self.account_bytes(grew * (std::mem::size_of::<VmValue>() + 1))?;
        }
        Ok(())
    }

    pub(super) fn prepare_frame(&mut self, base: usize, regs: usize) -> Result<(), EvalError> {
        self.ensure_regs(base + regs)?;
        // Clear the new window's written bits AND release any stale value left in a
        // reused slot. The register stack is append-only and reuses windows in
        // place (execution spec §4 rule 4), so a slot may still physically hold the
        // previous frame's `VmValue` — including an `Rc` to a heap list/map/string/
        // closure. The written bit alone only blocks stale *reads*; dropping the
        // value here is what makes an unwritten slot non-retaining for ownership and
        // memory accounting (execution spec §4.1). Args are written immediately
        // after `prepare_frame` via `set_reg`, so this never clobbers live inputs.
        for index in base..base + regs {
            self.written[index] = false;
            // Assigning `Unit` drops whatever the reused slot held; `Unit` owns
            // nothing, so heap refcounts fall here rather than at next overwrite.
            self.stack[index] = VmValue::Unit;
        }
        Ok(())
    }

    #[inline(always)]
    pub(super) fn reg(&self, index: usize) -> &VmValue {
        // Reading an unwritten register is a lowering/codegen invariant violation,
        // never a user-level runtime error. Assert in release too so we fail loudly
        // instead of silently observing a stale value left in the reused frame
        // window (the stack only grows and frames are reused in place).
        assert!(
            self.written.get(index).copied().unwrap_or(false),
            "reg VM internal error: read uninitialized register {index}"
        );
        &self.stack[index]
    }

    #[inline(always)]
    pub(super) fn set_reg(&mut self, index: usize, value: VmValue) {
        self.stack[index] = value;
        self.written[index] = true;
    }

    /// Propagate a completing frame's `mut` parameters back to the caller: each
    /// `(caller_reg, callee_reg)` copies the parameter's final value out. A no-op
    /// for the common call with no `mut` args (empty `mut_writeback`).
    pub(super) fn apply_mut_writeback(&mut self, frame: &Frame) {
        for &(caller_reg, callee_reg) in &frame.mut_writeback {
            let value = self.reg(callee_reg).clone();
            self.set_reg(caller_reg, value);
        }
    }

    #[inline(always)]
    pub(super) fn take_reg(&mut self, index: usize) -> VmValue {
        assert!(
            self.written.get(index).copied().unwrap_or(false),
            "reg VM internal error: take uninitialized register {index}"
        );
        self.written[index] = false;
        std::mem::replace(&mut self.stack[index], VmValue::Unit)
    }

    // Shared register stack with frame windows. Each frame owns
    // `stack[base .. base + function.regs]`; a callee is placed immediately
    // above the caller at `base + function.regs`. The stack only grows
    // (`ensure_regs`) so recursion is bounded only by memory. Debug builds keep
    // a written-register bitmap so stale slots cannot mask lowering bugs.
    /// Synchronous call entry used by contexts that cannot suspend (closure
    /// callbacks, resource drops, the program root before the scheduler owns it).
    /// Pushes `function`'s frame and drives to completion; a suspension here is a
    /// lowering/runtime invariant violation (only `async` code awaits, and that
    /// always runs under the task scheduler).
    pub(super) fn run_frame(
        &mut self,
        unit: &RegUnit,
        function: Rc<RegFunction>,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        self.ensure_regs(base + function.regs)?;
        let floor = self.frames.len();
        #[cfg(feature = "native-jit")]
        let function_ordinal = function.ordinal;
        self.push_frame(Frame {
            func: function,
            #[cfg(feature = "native-jit")]
            function_ordinal,
            ip: 0,
            base,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })?;
        match self.drive(unit, floor)? {
            Outcome::Completed(value) => Ok(value),
            Outcome::Suspended => Err(EvalError::Runtime(
                "reg VM cannot suspend (await/blocking op) inside a synchronous context."
                    .to_string(),
            )),
        }
    }

}

#[cfg(all(test, feature = "native-jit"))]
mod tests {
    use super::{accumulate_osr_work, advance_auto_osr_work};

    #[test]
    fn osr_work_accounting_saturates() {
        assert_eq!(accumulate_osr_work(u32::MAX - 2, 10), u32::MAX);
        assert_eq!(accumulate_osr_work(u32::MAX, 1), u32::MAX);
    }

    #[test]
    fn automatic_osr_waits_below_and_fires_at_threshold() {
        assert_eq!(advance_auto_osr_work(0, 20, 100), (20, false));
        assert_eq!(advance_auto_osr_work(80, 20, 100), (100, true));
        assert_eq!(advance_auto_osr_work(99, 5, 100), (104, true));
    }
}
