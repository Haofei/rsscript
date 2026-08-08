use super::*;

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
        self.run_scheduler(&unit, root)
    }

    /// Register a new ready task running `func` with `args` placed in its first
    /// registers (its own private register stack, base 0).
    pub(super) fn create_task(&mut self, func: Rc<RegFunction>, args: Vec<VmValue>) -> TaskId {
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
            let Some(tid) = self.ready_queue.pop_front() else {
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
                    Some(Wait::Select { handles, .. }) => handles
                        .iter()
                        .any(|h| self.tasks.get(h).is_some_and(|s| s.done.is_some())),
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

    /// Cancel every losing `select` arm task once a winner is chosen. A resolved
    /// `select` keeps only the winner; the backend drops the losing arms' futures
    /// so they stop immediately, and the VM must do the same — otherwise a loser
    /// would keep being scheduled at later suspension points, run its remaining
    /// side effects, and could even abort the whole program with a late error.
    /// Removing the task slot makes any stale ready-queue entry a no-op and stops
    /// the scheduler (and sleeper wakeups) from ever resuming it.
    pub(super) fn cancel_select_losers(&mut self, handles: &[TaskId], winner: TaskId) {
        for handle in handles {
            if *handle != winner {
                if let Some(task) = self.tasks.remove(handle)
                    && task.done.is_none()
                {
                    self.tasks_cancelled = self.tasks_cancelled.saturating_add(1);
                    self.tasks_live = self.tasks_live.saturating_sub(1);
                }
            }
        }
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
                self.complete_wait(tid, result);
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
                self.cancel_select_losers(&handles, task);
                self.write_saved_reg(tid, winner_dst, VmValue::Int(index as i64));
                self.complete_wait_at(tid, value_dst, value);
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
