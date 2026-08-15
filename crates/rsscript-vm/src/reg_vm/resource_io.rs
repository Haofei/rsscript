use super::*;

impl RegVm {
    /// Record a successfully acquired lexical resource in the currently
    /// executing task. The scope owns a clone rather than a register borrow:
    /// a parked task moves its register window out of the active VM stack, and
    /// cancellation must still be able to execute its cleanup contract.
    pub(super) fn acquire_resource_scope(&mut self, register: usize) {
        let value = self.reg(register).clone();
        self.resource_scopes
            .entry(self.current_task)
            .or_default()
            .push(TrackedResource { register, value });
    }

    /// Execute a normal lexical release. New bytecode must release in LIFO
    /// order; an untracked `ResourceDrop` remains supported for pre-scope v1
    /// Artifacts and legacy resource-drop bodies.
    pub(super) fn release_resource_scope(
        &mut self,
        unit: &RegUnit,
        register: usize,
    ) -> Result<(), EvalError> {
        let tracked = self
            .resource_scopes
            .get_mut(&self.current_task)
            .and_then(|scopes| scopes.pop());
        if let Some(tracked) = tracked {
            if tracked.register != register {
                return Err(EvalError::Runtime(
                    "resource scopes must be released in lexical LIFO order.".to_owned(),
                ));
            }
            self.run_resource_drop(unit, tracked.value, self.stack.len())
        } else {
            let value = self.reg(register).clone();
            self.run_resource_drop(unit, value, self.stack.len())
        }
    }

    /// Finalize every live lexical resource for a task, most-recent scope
    /// first. This is used for cancellation and terminal VM errors; normal
    /// verified execution drains the same scopes via `ResourceDrop` instead.
    pub(super) fn cleanup_task_resource_scopes(
        &mut self,
        unit: &RegUnit,
        task: TaskId,
    ) -> Result<(), EvalError> {
        let mut first_error = None;
        while let Some(tracked) = self
            .resource_scopes
            .get_mut(&task)
            .and_then(|scopes| scopes.pop())
        {
            if let Err(error) = self.run_resource_drop(unit, tracked.value, self.stack.len())
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        self.resource_scopes.remove(&task);
        first_error.map_or(Ok(()), Err)
    }

    pub(super) fn cleanup_all_resource_scopes(&mut self, unit: &RegUnit) -> Result<(), EvalError> {
        let tasks = self.resource_scopes.keys().copied().collect::<Vec<_>>();
        let mut first_error = None;
        for task in tasks {
            if let Err(error) = self.cleanup_task_resource_scopes(unit, task)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(super) fn run_resource_drop(
        &mut self,
        unit: &RegUnit,
        value: VmValue,
        base: usize,
    ) -> Result<(), EvalError> {
        let VmValue::Struct(data) = value else {
            return Ok(());
        };
        let Some(function_id) = unit
            .resource_drop_functions
            .get(data.name().as_ref())
            .copied()
        else {
            return Ok(());
        };
        let callee = Rc::clone(&unit.functions[function_id]);
        self.prepare_frame(base, callee.regs)?;
        for (field, value) in data.iter() {
            if let Some(reg) = callee.local_regs.get(field.as_ref()) {
                self.set_reg(base + *reg, value.clone());
            }
        }
        let result = self.run_frame(unit, callee, base)?;
        if matches!(result, VmValue::Unit) {
            Ok(())
        } else {
            Err(EvalError::Runtime(format!(
                "resource drop for `{}` returned unsupported value `{}`.",
                data.name(),
                result.display()
            )))
        }
    }
}
