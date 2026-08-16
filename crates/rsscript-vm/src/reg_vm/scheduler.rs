use super::*;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::time::Duration;

struct SchedulerWake(std::thread::Thread);

impl Wake for SchedulerWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

impl RegVm {
    /// Run `name` (the program entry, usually `main`) as the root task under the
    /// cooperative scheduler and return its value. Other tasks created via
    /// `spawn`/`async let` are interleaved at suspension points.
    pub(super) fn run_program(&mut self, name: &str) -> Result<VmValue, EvalError> {
        let function_id = self.unit.function_ids.get(name).copied().ok_or_else(|| {
            EvalError::Runtime(format!("reg VM cannot resolve function `{name}`."))
        })?;
        let unit = Rc::clone(&self.unit);
        let func = Rc::clone(&unit.functions[function_id]);
        let args = match func.params {
            0 => Vec::new(),
            1 => {
                let signature = unit.native_signatures.get(name).ok_or_else(|| {
                    EvalError::Runtime(format!(
                        "reg VM cannot validate the `{name}` entry-point signature."
                    ))
                })?;
                if signature.params.as_slice() != ["List<String>"] {
                    return Err(EvalError::Runtime(format!(
                        "reg VM entry point `{name}` must accept either no parameters or one `List<String>` parameter."
                    )));
                }
                vec![VmValue::List(Rc::new(RefCell::new(
                    self.entry_args
                        .iter()
                        .cloned()
                        .map(VmValue::string)
                        .collect(),
                )))]
            }
            _ => {
                return Err(EvalError::Runtime(format!(
                    "reg VM entry point `{name}` must accept either no parameters or one `List<String>` parameter."
                )));
            }
        };
        let root = self.create_task(func, args);
        let result = self.run_scheduler(&unit, root);
        let cleanup = self.cleanup_all_resource_scopes(&unit);
        match (result, cleanup) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(value), Ok(())) => Ok(value),
        }
    }

    /// Register a new ready task running `func` with `args` placed in its first
    /// registers (its own private register stack, base 0).
    pub(super) fn create_task(&mut self, func: Rc<RegFunction>, args: Vec<VmValue>) -> TaskId {
        self.live_memory_dirty = true;
        let tid = self.next_task_id;
        self.next_task_id += 1;
        let regs = func.regs.max(args.len());
        let mut stack = vec![VmValue::Unit; regs];
        let mut written = vec![false; regs];
        for (index, arg) in args.into_iter().enumerate() {
            stack[index] = arg;
            written[index] = true;
        }
        let frames = vec![Frame {
            func,
            ip: 0,
            base: 0,
            ret_dst: usize::MAX,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        }];
        self.tasks.insert(
            tid,
            TaskSlot {
                saved: Some(SavedTask {
                    frames,
                    stack,
                    written,
                }),
                done: None,
                wait: None,
                resume_dst: usize::MAX,
            },
        );
        self.tasks_created = self.tasks_created.saturating_add(1);
        self.tasks_live = self.tasks_live.saturating_add(1);
        self.tasks_peak_live = self.tasks_peak_live.max(self.tasks_live);
        self.ready_queue.push_back(tid);
        tid
    }

    /// Make `tid` the running task: move its parked register state into `self`.
    pub(super) fn swap_in(&mut self, tid: TaskId) {
        let saved = self
            .tasks
            .get_mut(&tid)
            .expect("task slot")
            .saved
            .take()
            .expect("parked task state");
        self.frames = saved.frames;
        self.stack = saved.stack;
        self.written = saved.written;
        self.current_task = tid;
    }

    /// Park the running task: move its register state back into its slot.
    pub(super) fn swap_out(&mut self, tid: TaskId) {
        let saved = SavedTask {
            frames: std::mem::take(&mut self.frames),
            stack: std::mem::take(&mut self.stack),
            written: std::mem::take(&mut self.written),
        };
        self.tasks.get_mut(&tid).expect("task slot").saved = Some(saved);
    }

    pub(super) fn run_scheduler(
        &mut self,
        unit: &RegUnit,
        root: TaskId,
    ) -> Result<VmValue, EvalError> {
        loop {
            if self.ready_queue.is_empty() {
                self.satisfy_waiters()?;
            }
            let Some(tid) = self.ready_queue.pop_front() else {
                if self.has_pending_provider_future() {
                    // Futures arrange an unpark through their waker. The short
                    // timeout also lets cancellation/deadline state be observed
                    // when a broken Provider fails to wake the scheduler.
                    self.charge_work(0)?;
                    std::thread::park_timeout(Duration::from_millis(1));
                    continue;
                }
                return Err(EvalError::Runtime(
                    "reg VM async scheduler stalled: every task is blocked (deadlock).".to_string(),
                ));
            };
            // Skip stale queue entries (finished, still parked, or running).
            match self.tasks.get(&tid) {
                Some(slot)
                    if slot.done.is_none() && slot.wait.is_none() && slot.saved.is_some() => {}
                _ => continue,
            }
            self.swap_in(tid);
            match self.drive(unit, 0) {
                Ok(Outcome::Completed(value)) => {
                    self.tasks_completed = self.tasks_completed.saturating_add(1);
                    self.tasks_live = self.tasks_live.saturating_sub(1);
                    // Drop the finished task's register state.
                    self.frames = Vec::new();
                    self.stack = Vec::new();
                    self.written = Vec::new();
                    if tid == root {
                        self.refresh_live_memory_usage_with(Some(&value))?;
                        return Ok(value);
                    }
                    self.tasks.get_mut(&tid).expect("task slot").done = Some(value);
                }
                Ok(Outcome::Suspended) => {
                    let suspension = self.suspension.take().expect("suspension recorded");
                    self.swap_out(tid);
                    let slot = self.tasks.get_mut(&tid).expect("task slot");
                    slot.resume_dst = suspension.resume_dst;
                    slot.wait = Some(suspension.wait);
                }
                // A hard VM error in any task aborts the whole program.
                Err(error) => return Err(error),
            }
            // A send/recv/finish may have unblocked parked tasks.
            self.satisfy_waiters()?;
        }
    }

    /// Repeatedly wake any parked task whose wait is now satisfiable, until no
    /// further progress (a fixpoint), so a single send can cascade-wake a chain.
    pub(super) fn satisfy_waiters(&mut self) -> Result<(), EvalError> {
        loop {
            self.poll_provider_futures();
            let ready: Vec<TaskId> = self
                .tasks
                .iter()
                .filter(|(_, slot)| slot.done.is_none())
                .filter(|(_, slot)| match &slot.wait {
                    Some(Wait::Recv { channel }) => self.channel_ready(*channel),
                    Some(Wait::Send { sender, .. }) => self.channel_has_space(sender.channel_id),
                    Some(Wait::Join { task }) => {
                        self.tasks.get(task).is_some_and(|s| s.done.is_some())
                    }
                    Some(Wait::JoinAll { tasks }) => tasks
                        .iter()
                        .all(|task| self.tasks.get(task).is_none_or(|slot| slot.done.is_some())),
                    Some(Wait::Select { handles, .. }) => handles
                        .iter()
                        .any(|h| self.tasks.get(h).is_some_and(|s| s.done.is_some())),
                    Some(Wait::WireProvider { result, .. }) => result.is_some(),
                    Some(Wait::WireMutationProvider { result, .. }) => result.is_some(),
                    None => false,
                })
                .map(|(id, _)| *id)
                .collect();
            if ready.is_empty() {
                return Ok(());
            }
            for tid in ready {
                self.resolve_wait(tid)?;
            }
        }
    }

    fn poll_provider_futures(&mut self) {
        let waker = Waker::from(Arc::new(SchedulerWake(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        for slot in self.tasks.values_mut() {
            if let Some(Wait::WireProvider { future, result, .. }) = slot.wait.as_mut()
                && result.is_none()
                && let Poll::Ready(value) = future.as_mut().poll(&mut context)
            {
                *result = Some(value);
            }
            if let Some(Wait::WireMutationProvider { future, result, .. }) = slot.wait.as_mut()
                && result.is_none()
                && let Poll::Ready(value) = future.as_mut().poll(&mut context)
            {
                *result = Some(value);
            }
        }
    }

    fn has_pending_provider_future(&self) -> bool {
        self.tasks.values().any(|slot| {
            matches!(
                slot.wait,
                Some(Wait::WireProvider { result: None, .. })
                    | Some(Wait::WireMutationProvider { result: None, .. })
            )
        })
    }

    /// Cancel every losing `select` arm task once a winner is chosen. A resolved
    /// `select` keeps only the winner; the backend drops the losing arms' futures
    /// so they stop immediately, and the VM must do the same — otherwise a loser
    /// would keep being scheduled at later suspension points, run its remaining
    /// side effects, and could even abort the whole program with a late error.
    /// Removing the task slot makes any stale ready-queue entry a no-op and stops
    /// the scheduler (and sleeper wakeups) from ever resuming it.
    pub(super) fn cancel_select_losers(
        &mut self,
        unit: &RegUnit,
        handles: &[TaskId],
        winner: TaskId,
    ) -> Result<(), EvalError> {
        for handle in handles {
            if *handle != winner {
                if let Some(task) = self.tasks.remove(handle)
                    && task.done.is_none()
                {
                    self.tasks_cancelled = self.tasks_cancelled.saturating_add(1);
                    self.tasks_live = self.tasks_live.saturating_sub(1);
                    self.cleanup_task_resource_scopes(unit, *handle)?;
                }
            }
        }
        Ok(())
    }

    /// Close a child task's lifetime without treating cancellation as a
    /// successful completion. A task is only externally cancellable while it
    /// is parked or queued; the current task cannot remove itself while its
    /// frames are swapped into the VM. Run-owned Provider resources are not
    /// stored in a task slot, so they retain their existing exact-once
    /// finalization at the enclosing execution boundary.
    pub(super) fn cancel_task(&mut self, task: TaskId) -> Result<(), EvalError> {
        if task == self.current_task {
            return Err(EvalError::Runtime(
                "reg VM task cannot cancel itself while it is running.".to_string(),
            ));
        }
        let Some(slot) = self.tasks.remove(&task) else {
            return Err(EvalError::Runtime(
                "reg VM cannot cancel an unknown or already-reaped task.".to_string(),
            ));
        };
        if slot.done.is_none() {
            self.tasks_cancelled = self.tasks_cancelled.saturating_add(1);
            self.tasks_live = self.tasks_live.saturating_sub(1);
        }
        self.cleanup_task_resource_scopes(&Rc::clone(&self.unit), task)
    }

    /// Produce the result of `tid`'s satisfied wait and re-queue it.
    pub(super) fn resolve_wait(&mut self, tid: TaskId) -> Result<(), EvalError> {
        let wait = self
            .tasks
            .get_mut(&tid)
            .expect("task slot")
            .wait
            .take()
            .expect("parked wait");
        match wait {
            Wait::Recv { channel } => {
                let result = json_result(self.channel_recv(channel));
                self.complete_wait(tid, result);
            }
            Wait::Send { sender, value } => {
                let result = json_result(self.channel_send(sender, value));
                self.complete_wait(tid, result);
            }
            Wait::Join { task } => {
                let result = self
                    .tasks
                    .get(&task)
                    .and_then(|slot| slot.done.clone())
                    .expect("joined task finished");
                // Reap the joined slot: its value has been delivered and a handle
                // is awaited at most once (RS0030), so nothing references it again.
                // Without this the task table grows by one slot per `async let`
                // forever, making the scheduler's per-step scans O(n²) (see the
                // `AwaitJoin` immediate-path note).
                self.tasks.remove(&task);
                let unit = Rc::clone(&self.unit);
                self.cleanup_task_resource_scopes(&unit, task)?;
                self.complete_wait(tid, result);
            }
            Wait::JoinAll { tasks } => {
                for task in tasks {
                    self.tasks.remove(&task);
                    let unit = Rc::clone(&self.unit);
                    self.cleanup_task_resource_scopes(&unit, task)?;
                }
                self.complete_wait(tid, VmValue::Unit);
            }
            Wait::Select {
                handles,
                winner_dst,
                value_dst,
            } => {
                // First finished arm wins; its value goes to `value_dst`, its arm
                // index to `winner_dst`. The losing arms are cancelled (see
                // `cancel_select_losers`) so they cannot keep running.
                let (index, task) = handles
                    .iter()
                    .enumerate()
                    .find(|(_, h)| self.tasks.get(h).is_some_and(|s| s.done.is_some()))
                    .map(|(i, h)| (i, *h))
                    .expect("a select arm finished");
                let value = self
                    .tasks
                    .get(&task)
                    .and_then(|slot| slot.done.clone())
                    .expect("winning arm value");
                let unit = Rc::clone(&self.unit);
                self.cancel_select_losers(&unit, &handles, task)?;
                self.write_saved_reg(tid, winner_dst, VmValue::Int(index as i64));
                self.complete_wait_at(tid, value_dst, value);
            }
            Wait::WireProvider {
                result,
                key,
                mutation_targets,
                ..
            } => {
                let wire = result
                    .expect("wire Provider future was ready")
                    .map_err(EvalError::Provider)?;
                let function = self.external_bindings.get(&key).cloned().ok_or_else(|| {
                    EvalError::Runtime(format!(
                        "reg VM wire provider function `{key}` disappeared while suspended"
                    ))
                })?;
                let contract = function.contract().ok_or_else(|| {
                    EvalError::Runtime(format!(
                        "reg VM wire provider function `{key}` has no linked contract"
                    ))
                })?;
                let types = function.wire_types().map_err(EvalError::Provider)?;
                let value =
                    vm_value_from_wire_value(wire, &contract.descriptor.signature.result, &types)?;
                if !mutation_targets.is_empty() {
                    return Err(EvalError::Runtime(
                        "non-mutation Provider result cannot write back mut parameters".into(),
                    ));
                }
                self.complete_wait(tid, value);
            }
            Wait::WireMutationProvider {
                result,
                key,
                mutation_targets,
                ..
            } => {
                let wire = result
                    .expect("wire mutation Provider future was ready")
                    .map_err(EvalError::Provider)?;
                let function = self.external_bindings.get(&key).cloned().ok_or_else(|| {
                    EvalError::Runtime(format!(
                        "reg VM wire mutation provider function `{key}` disappeared while suspended"
                    ))
                })?;
                let contract = function.contract().ok_or_else(|| {
                    EvalError::Runtime(format!(
                        "reg VM wire mutation provider function `{key}` has no linked contract"
                    ))
                })?;
                let types = function.wire_types().map_err(EvalError::Provider)?;
                let value = vm_value_from_wire_value(
                    wire.result,
                    &contract.descriptor.signature.result,
                    &types,
                )?;
                let mutation_types = contract
                    .descriptor
                    .signature
                    .parameters
                    .iter()
                    .filter(|parameter| parameter.effect == rsscript_abi_model::DataEffect::Mut)
                    .map(|parameter| &parameter.ty)
                    .collect::<Vec<_>>();
                if wire.mutated.len() != mutation_targets.len()
                    || mutation_types.len() != mutation_targets.len()
                {
                    return Err(EvalError::Runtime(
                        "wire mutation Provider result does not contain every mut write-back"
                            .into(),
                    ));
                }
                let mutated = wire
                    .mutated
                    .into_iter()
                    .zip(mutation_types)
                    .map(|(value, ty)| vm_value_from_wire_value(value, ty, &types))
                    .collect::<Result<Vec<_>, _>>()?;
                for (register, mutated_value) in mutation_targets.into_iter().zip(mutated) {
                    self.write_saved_reg(tid, register, mutated_value);
                }
                self.complete_wait(tid, value);
            }
        }
        Ok(())
    }

    /// Write `value` into a parked task's saved register window (no re-queue).
    pub(super) fn write_saved_reg(&mut self, tid: TaskId, dst: usize, value: VmValue) {
        let saved = self
            .tasks
            .get_mut(&tid)
            .expect("task slot")
            .saved
            .as_mut()
            .expect("parked task state");
        if dst >= saved.stack.len() {
            saved.stack.resize(dst + 1, VmValue::Unit);
            saved.written.resize(dst + 1, false);
        }
        saved.stack[dst] = value;
        saved.written[dst] = true;
    }

    /// Write a woken task's result into its recorded `resume_dst` and re-queue it.
    pub(super) fn complete_wait(&mut self, tid: TaskId, result: VmValue) {
        let dst = self.tasks.get(&tid).expect("task slot").resume_dst;
        self.complete_wait_at(tid, dst, result);
    }

    /// Write `result` into a parked task's register `dst` and re-queue it.
    pub(super) fn complete_wait_at(&mut self, tid: TaskId, dst: usize, result: VmValue) {
        self.write_saved_reg(tid, dst, result);
        self.ready_queue.push_back(tid);
    }

    pub(super) fn channel_ready(&self, channel: i64) -> bool {
        self.channels.get(&channel).is_none_or(|state| {
            !state.queue.is_empty() || state.senders == 0 || state.receiver_closed
        })
    }

    pub(super) fn channel_has_space(&self, channel: i64) -> bool {
        self.channels
            .get(&channel)
            .is_none_or(|state| state.receiver_closed || state.queue.len() < state.capacity)
    }

    /// True when `Sender.send` on an open channel would block (buffer full).
    pub(super) fn channel_send_would_block(&self, sender: &VmSender) -> bool {
        if sender.closed {
            return false;
        }
        self.channels
            .get(&sender.channel_id)
            .is_some_and(|state| !state.receiver_closed && state.queue.len() >= state.capacity)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cancellation_unit() -> Rc<RegUnit> {
        let main = RegFunction {
            name: "main".to_string(),
            params: 0,
            captures: 0,
            regs: 2,
            local_regs: HashMap::new(),
            code: vec![
                RegInstr::SpawnTask {
                    dst: 0,
                    function: 1,
                    args: vec![],
                },
                RegInstr::CancelTask { src: 0 },
                RegInstr::LoadUnit { dst: 1 },
                RegInstr::Return { src: 1 },
            ],
        };
        let worker = RegFunction {
            name: "worker".to_string(),
            params: 0,
            captures: 0,
            regs: 1,
            local_regs: HashMap::new(),
            code: vec![
                RegInstr::LoadInt { dst: 0, value: 7 },
                RegInstr::Return { src: 0 },
            ],
        };
        Rc::new(RegUnit {
            functions: vec![Rc::new(main), Rc::new(worker)],
            function_ids: HashMap::from([("main".to_string(), 0)]),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            variant_layouts: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        })
    }

    #[test]
    fn explicit_cancel_reaps_a_child_without_marking_it_completed() {
        let mut vm = RegVm::new(
            cancellation_unit(),
            "sha256:test-cancel".to_string(),
            vec![],
            HashMap::new(),
        );

        assert!(matches!(vm.run_program("main"), Ok(VmValue::Unit)));
        let usage = vm.usage();
        assert_eq!(usage.tasks_created, 2);
        assert_eq!(usage.tasks_completed, 1, "only main completed");
        assert_eq!(usage.tasks_cancelled, 1);
        assert_eq!(usage.tasks_live_at_return, 0);
    }

    #[test]
    fn explicit_cancel_rejects_an_unknown_handle() {
        let mut vm = RegVm::new(
            cancellation_unit(),
            "sha256:test-cancel".to_string(),
            vec![],
            HashMap::new(),
        );
        assert!(matches!(
            vm.cancel_task(99),
            Err(EvalError::Runtime(message)) if message.contains("unknown or already-reaped")
        ));
    }

    #[test]
    fn cancelling_a_parked_task_drains_its_tracked_resource_scopes() {
        let mut vm = RegVm::new(
            cancellation_unit(),
            "sha256:test-resource-cancel".to_string(),
            vec![],
            HashMap::new(),
        );
        let root = Rc::clone(&vm.unit.functions[0]);
        vm.create_task(root, Vec::new());
        let worker = Rc::clone(&vm.unit.functions[1]);
        let task = vm.create_task(worker, Vec::new());
        vm.current_task = task;
        vm.stack = vec![VmValue::Unit];
        vm.written = vec![true];
        vm.acquire_resource_scope(0);
        vm.current_task = 0;

        vm.cancel_task(task)
            .expect("cancelling a parked tracked task");
        assert!(
            !vm.resource_scopes.contains_key(&task),
            "cancel must drain lexical resource scopes even after task registers are parked"
        );
    }
}
