use super::*;
use crate::serde_json;

mod storage_accounting;

#[cfg(feature = "native-jit")]
fn accumulate_osr_work(current: u32, iteration_work: u32) -> u32 {
    current.saturating_add(iteration_work)
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
        executable_digest: String,
        entry_args: Vec<String>,
        external_bindings: HashMap<String, ExternalFunction>,
    ) -> Self {
        Self {
            jit_state: JitState::for_verified_program(executable_digest, &unit),
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

    /// Whether it is sound to dispatch Cranelift-native code right now: native code
    /// polls neither the step budget nor the cancel flag and runs allocation off the
    /// memory meter, so all three preemption/accounting limits must be unarmed (it
    /// `tick()`s on the interpreter/tier-0 paths instead). The single source of truth
    /// for both the native-tier gate (`try_native`) and the recursive native fast
    /// paths (self-recursive + mutual-recursive); see execution spec §6.2 (Model A).
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

    fn reserve_output(&self, additional: usize) -> Result<(), EvalError> {
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

    fn flush_stdout_stream(&mut self) -> Result<(), EvalError> {
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
    pub(super) fn is_jit_eligible(&self, func: &RegFunction) -> bool {
        let (eligible, has_loop) = self.jit_state.tier0_analysis(func);
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
    #[allow(clippy::mutable_key_type)]
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
    ) -> Result<PureStep, EvalError> {
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
                if expect_bool_ref(self.reg(base + *cond))? == *expected {
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
                if eval_numeric_compare(*op, l, r)? == *expected {
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

    #[cfg(feature = "native-jit")]
    #[inline(always)]
    pub(super) fn record_native_branch_feedback(
        &mut self,
        func: &RegFunction,
        instr: &RegInstr,
        base: usize,
        instr_ip: usize,
    ) -> Result<(), EvalError> {
        if self.native.is_none() {
            return Ok(());
        }
        match instr {
            RegInstr::JumpIfBool { cond, expected, .. } => {
                let taken = expect_bool_ref(self.reg(base + *cond))? == *expected;
                self.jit_state.record_branch_site(func, instr_ip, taken);
            }
            RegInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                ..
            } => {
                let taken =
                    eval_numeric_compare(*op, self.reg(base + *lhs), self.reg(base + *rhs))?
                        == *expected;
                self.jit_state.record_branch_site(func, instr_ip, taken);
            }
            _ => {}
        }
        Ok(())
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
        self.push_frame(Frame {
            func: function,
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
                match self.try_native(&func, base) {
                    NativeAttempt::Completed(value) => {
                        let frame = self.frames.pop().expect("active frame");
                        self.apply_mut_writeback(&frame);
                        if self.frames.len() == floor {
                            return Ok(Outcome::Completed(value));
                        }
                        self.set_reg(frame.ret_dst, value);
                        continue 'frames;
                    }
                    // J0.2 precise resume: `try_native` already restored the live
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
            let osr_pre_candidates = if ip == 0 && self.native.is_some() {
                self.resolve_osr_candidates(&func)
            } else {
                OsrCandidates::default()
            };
            #[cfg(not(feature = "native-jit"))]
            let osr_pre_candidate: Option<usize> = None;

            // Tier-0 JIT: a fresh JIT-eligible frame runs via the specializing
            // executor (which reuses the interpreter's semantics), then completes
            // exactly like the `Return` arm. Eligible functions never suspend, so
            // they are always entered at `ip == 0`.
            if self.jit_enabled
                && ip == 0
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
                && self.is_jit_eligible(&func)
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
            let (osr_candidates, osr_eager) = if self.native.is_some() {
                (
                    if osr_pre_candidates.is_empty() {
                        self.resolve_osr_candidates(&func)
                    } else {
                        osr_pre_candidates
                    },
                    self.native.as_ref().is_some_and(|n| n.osr_enabled),
                )
            } else {
                (OsrCandidates::default(), false)
            };
            #[cfg(not(feature = "native-jit"))]
            let _osr_candidate: Option<usize> = None;

            while let Some(instr) = func.code.get(ip) {
                // At most four comparisons are performed for candidate functions.
                // Each matching header charges and probes only its own RegionKey.
                #[cfg(feature = "native-jit")]
                if let Some(candidate) = osr_candidates
                    .iter()
                    .find(|candidate| candidate.header_ip == ip)
                {
                    let region_key = RegionKey {
                        function: self.jit_state.function_ordinal(&func),
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
                                    let next = accumulate_osr_work(count, candidate.iteration_work);
                                    let threshold = self
                                        .native
                                        .as_ref()
                                        .map_or(OSR_BACKEDGE_THRESHOLD, |native| {
                                            native.osr_work_threshold
                                        });
                                    if next >= threshold {
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
                            if self.try_osr(&func, base, ip) {
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
                self.record_native_branch_feedback(&func, instr, base, ip)?;
                ip += 1;
                // Pure instructions (loads, arithmetic, jumps, matches, heap
                // construction, …) run through the shared `try_exec_pure`, the one
                // copy of their semantics that the JIT executor also uses — so the
                // two can never diverge. Only frame/suspension/call-shaped
                // instructions need the interpreter-specific handling below.
                match self.try_exec_pure(instr, base, &mut ip)? {
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
                            // J1 type feedback (warm-gated + bounded inside the
                            // helper): record the resolved callee identity at this
                            // site. The dispatch DECISION above (`callee_id`) is
                            // unchanged — we only observe it. `ip` was already
                            // advanced past this instruction, so its index is
                            // `ip - 1`.
                            #[cfg(feature = "native-jit")]
                            if self.native.is_some() {
                                self.jit_state.record_call_site(
                                    &func,
                                    ip - 1,
                                    callee_id as u64,
                                    true,
                                );
                            }
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
                            // J1 type feedback (warm-gated + bounded inside the
                            // helper): the closure's underlying function id is its
                            // stable identity (one callee ⇒ monomorphic). Recording
                            // does not change which closure runs; `ip` already
                            // points past this instruction, so its index is
                            // `ip - 1`.
                            #[cfg(feature = "native-jit")]
                            if self.native.is_some() {
                                self.jit_state.record_call_site(
                                    &func,
                                    ip - 1,
                                    closure.function as u64,
                                    closure_captures_all_scalar(&closure),
                                );
                            }
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

#[cfg(all(test, feature = "native-jit"))]
mod tests {
    use super::accumulate_osr_work;

    #[test]
    fn osr_work_accounting_saturates() {
        assert_eq!(accumulate_osr_work(u32::MAX - 2, 10), u32::MAX);
        assert_eq!(accumulate_osr_work(u32::MAX, 1), u32::MAX);
    }
}
