use super::*;

impl RegVm {
    fn native_deopt_resume_ip(
        &self,
        function: vm_jit::CompiledId,
        safepoint_id: vm_jit::SafepointId,
    ) -> Option<usize> {
        self.native
            .as_ref()
            .and_then(|native| native.baseline_module.deopt_map(function))
            .and_then(|map| map.sites.get(safepoint_id.0 as usize - 1))
            .map(|site| site.resume_ip as usize)
    }

    #[cfg(feature = "native-jit")]
    fn native_compiled_id_for_func(&self, func: &RegFunction) -> Option<vm_jit::CompiledId> {
        let key = func as *const RegFunction as usize;
        self.native
            .as_ref()
            .and_then(|native| {
                native
                    .cache
                    .iter()
                    .find(|(version, entry)| version.function == key && entry.as_ref().is_some())
                    .and_then(|(_, entry)| entry.as_ref())
            })
            .map(|entry| entry.0)
    }

    #[cfg(feature = "native-jit")]
    pub(super) fn restore_native_deopt_live_regs(
        &mut self,
        base: usize,
        n_regs: usize,
        live: &[vm_jit::DeoptReg],
        host_ctx: vm_jit::HostCtx,
    ) -> bool {
        let mut materialized = Vec::with_capacity(live.len());
        for vm_jit::DeoptReg { reg, value } in live {
            // Flat pointers are excluded by vm-jit. Scalars restore directly;
            // Handle words are table indices and must be materialized while the
            // top-level call context (and its heap-argument table) is still alive.
            if (*reg as usize) >= n_regs {
                continue;
            }
            let vm_value = match value {
                vm_jit::DeoptValue::Int(i) => VmValue::Int(*i),
                vm_jit::DeoptValue::Bool(b) => VmValue::Bool(*b),
                vm_jit::DeoptValue::Float(f) => VmValue::Float(*f),
                // Resolve the heap-table index and clone the actual VmValue so moved
                // Handle locals and child-frame assignments are restored precisely.
                vm_jit::DeoptValue::Handle(handle) => {
                    let Some(value) = JitHostCallCtx::from_token(host_ctx)
                        .and_then(|ctx| ctx.heap_read_handle(*handle, |value| Some(value.clone())))
                    else {
                        return false;
                    };
                    value
                }
            };
            materialized.push((base + *reg as usize, vm_value));
        }
        for (slot, value) in materialized {
            self.set_reg(slot, value);
        }
        true
    }

    #[cfg(feature = "native-jit")]
    fn push_native_child_deopt_frame(
        &mut self,
        unit: &RegUnit,
        caller: &RegFunction,
        caller_base: usize,
        call_ip: usize,
        child: &vm_jit::DeoptFrame,
        host_ctx: vm_jit::HostCtx,
    ) -> bool {
        let Some(RegInstr::CallKnown {
            dst,
            function,
            args,
            mut_args,
        }) = caller.code.get(call_ip)
        else {
            return false;
        };
        if !mut_args.is_empty() {
            return false;
        }
        let Some(callee) = unit.functions.get(*function).cloned() else {
            return false;
        };
        if self.native_compiled_id_for_func(&callee) != Some(child.function) {
            return false;
        }
        let Some(child_resume_ip) = self.native_deopt_resume_ip(child.function, child.safepoint_id)
        else {
            return false;
        };
        let child_base = caller_base + caller.regs;
        if self.prepare_frame(child_base, callee.regs).is_err() {
            return false;
        }
        for (index, reg) in args.iter().enumerate() {
            let value = self.reg(caller_base + *reg).clone();
            self.set_reg(child_base + index, value);
        }
        if !self.restore_native_deopt_live_regs(child_base, callee.regs, &child.live, host_ctx) {
            return false;
        }

        let Some(caller_frame) = self.frames.last_mut() else {
            return false;
        };
        caller_frame.ip = call_ip + 1;
        self.push_frame(Frame {
            func: Rc::clone(&callee),
            ip: child_resume_ip,
            base: child_base,
            ret_dst: caller_base + *dst,
            mut_writeback: Vec::new(),
            tail_calls: 0,
        })
        .is_ok()
            && child.child.as_deref().is_none_or(|grandchild| {
                self.push_native_child_deopt_frame(
                    unit,
                    &callee,
                    child_base,
                    child_resume_ip,
                    grandchild,
                    host_ctx,
                )
            })
    }

    #[cfg(feature = "native-jit")]
    pub(super) fn try_resume_native_child_deopt_chain(
        &mut self,
        unit: &RegUnit,
        func: &RegFunction,
        base: usize,
        resume_ip: usize,
        live: &[vm_jit::DeoptReg],
        child: &vm_jit::DeoptFrame,
        host_ctx: vm_jit::HostCtx,
    ) -> bool {
        let original_len = self.frames.len();
        let original_ip = self.frames.last().map(|frame| frame.ip).unwrap_or_default();
        if !self.restore_native_deopt_live_regs(base, func.regs, live, host_ctx) {
            return false;
        }
        let resumed =
            self.push_native_child_deopt_frame(unit, func, base, resume_ip, child, host_ctx);
        if !resumed {
            self.frames.truncate(original_len);
            if let Some(frame) = self.frames.last_mut() {
                frame.ip = original_ip;
            }
        }
        resumed
    }
}
