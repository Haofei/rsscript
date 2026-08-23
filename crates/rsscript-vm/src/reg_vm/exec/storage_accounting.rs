use super::super::*;
use crate::serde_json;

impl RegVm {
    /// Recompute the exact reachable value-storage set defined by the VM model.
    /// Shared heap nodes are counted once; register/channel slot capacity is
    /// counted while it remains owned by a live task or channel. VM metadata,
    /// allocator headers, generated code, and Provider-owned memory are outside
    /// this deliberately deterministic metric.
    pub(in crate::reg_vm) fn refresh_live_memory_usage(&mut self) -> Result<(), EvalError> {
        if !self.live_memory_dirty {
            return Ok(());
        }
        self.refresh_live_memory_usage_with(None)
    }

    pub(in crate::reg_vm) fn refresh_live_memory_usage_with(
        &mut self,
        extra_root: Option<&VmValue>,
    ) -> Result<(), EvalError> {
        if self.limits.live_memory_limit.is_none() && self.peak_live_memory_bytes == 0 {
            return Ok(());
        }

        fn value_bytes(value: &VmValue, visited: &mut HashSet<usize>) -> usize {
            RegVm::retained_storage_bytes_inner(value, &HashSet::new(), visited)
        }

        let mut visited = HashSet::new();
        let mut bytes = self
            .stack
            .capacity()
            .saturating_mul(std::mem::size_of::<VmValue>());
        for (index, value) in self.stack.iter().enumerate() {
            if self.written.get(index).copied().unwrap_or(false) {
                bytes = bytes.saturating_add(value_bytes(value, &mut visited));
            }
        }

        for slot in self.tasks.values() {
            if let Some(saved) = &slot.saved {
                bytes = bytes.saturating_add(
                    saved
                        .stack
                        .capacity()
                        .saturating_mul(std::mem::size_of::<VmValue>()),
                );
                for (index, value) in saved.stack.iter().enumerate() {
                    if saved.written.get(index).copied().unwrap_or(false) {
                        bytes = bytes.saturating_add(value_bytes(value, &mut visited));
                    }
                }
            }
            if let Some(value) = &slot.done {
                bytes = bytes.saturating_add(value_bytes(value, &mut visited));
            }
            if let Some(Wait::Send { value, .. }) = &slot.wait {
                bytes = bytes.saturating_add(value_bytes(value, &mut visited));
            }
        }

        if let Some(Suspension {
            wait: Wait::Send { value, .. },
            ..
        }) = &self.suspension
        {
            bytes = bytes.saturating_add(value_bytes(value, &mut visited));
        }
        for channel in self.channels.values() {
            bytes = bytes.saturating_add(
                channel
                    .queue
                    .capacity()
                    .saturating_mul(std::mem::size_of::<VmValue>()),
            );
            for value in &channel.queue {
                bytes = bytes.saturating_add(value_bytes(value, &mut visited));
            }
        }
        for closure in self.noncapturing_closure_cache.iter().flatten() {
            bytes = bytes.saturating_add(value_bytes(
                &VmValue::Closure(Rc::clone(closure)),
                &mut visited,
            ));
        }
        if let Some(value) = extra_root {
            bytes = bytes.saturating_add(value_bytes(value, &mut visited));
        }

        self.live_memory_bytes = bytes;
        self.peak_live_memory_bytes = self.peak_live_memory_bytes.max(bytes);
        self.live_memory_dirty = false;
        if let Some(limit) = self.limits.live_memory_limit
            && bytes > limit
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::LiveMemoryLimitExceeded,
                format!("live memory limit exceeded ({limit} bytes)"),
            ));
        }
        Ok(())
    }

    /// Account `bytes` of container growth against the memory ceiling. A no-op
    /// (no add, no check) when `limits.allocation_budget` is `None`, so the off path is
    /// near-free. When a budget is set and the cumulative estimate exceeds it,
    /// returns the memory-limit error. Best-effort: see [`RegVm::allocated_bytes`].
    #[inline]
    pub(in crate::reg_vm) fn account_bytes(&mut self, bytes: usize) -> Result<(), EvalError> {
        if self.limits.live_memory_limit.is_some() && bytes != 0 {
            self.live_memory_dirty = true;
        }
        if self.limits.allocation_budget.is_some() {
            self.ensure_memory_available(bytes)?;
            self.allocated_bytes = self.allocated_bytes.saturating_add(bytes);
        }
        Ok(())
    }

    pub(in crate::reg_vm) fn ensure_memory_available(&self, bytes: usize) -> Result<(), EvalError> {
        if let Some(limit) = self.limits.allocation_budget
            && self.allocated_bytes.saturating_add(bytes) > limit
        {
            return Err(EvalError::execution(
                crate::ExecutionFailureKind::AllocationBudgetExceeded,
                format!("allocation budget exceeded ({limit} bytes)"),
            ));
        }
        Ok(())
    }

    pub(in crate::reg_vm) fn account_list_storage(
        &mut self,
        values: &TypedVec,
    ) -> Result<(), EvalError> {
        self.account_bytes(values.capacity().saturating_mul(values.elem_bytes()))
    }

    pub(in crate::reg_vm) fn fresh_list(&mut self, values: TypedVec) -> Result<VmValue, EvalError> {
        self.account_list_storage(&values)?;
        Ok(VmValue::List(Rc::new(RefCell::new(values))))
    }

    #[allow(clippy::mutable_key_type)]
    pub(in crate::reg_vm) fn fresh_map(&mut self, values: ValueMap) -> Result<VmValue, EvalError> {
        self.account_bytes(values.capacity().saturating_mul(MAP_ENTRY_BYTES))?;
        Ok(VmValue::Map(Rc::new(RefCell::new(values))))
    }

    #[allow(clippy::mutable_key_type)]
    pub(in crate::reg_vm) fn reserve_map_entry_accounted(
        &mut self,
        map: &mut ValueMap,
    ) -> Result<(), EvalError> {
        if map.len() < map.capacity() {
            return Ok(());
        }

        // HashMap growth is implementation-defined. Pre-charge a conservative
        // doubling before reserve so the host allocation cannot cross allocation_budget.
        let old_capacity = map.capacity();
        let projected_capacity = old_capacity.saturating_mul(2).saturating_add(3).max(3);
        let projected_bytes = projected_capacity
            .saturating_sub(old_capacity)
            .saturating_mul(MAP_ENTRY_BYTES);
        self.account_bytes(projected_bytes)?;

        if let Err(error) = map.try_reserve(1) {
            self.allocated_bytes = self.allocated_bytes.saturating_sub(projected_bytes);
            return Err(EvalError::Runtime(format!(
                "Map/Set insertion allocation failed: {error}"
            )));
        }

        let actual_bytes = map
            .capacity()
            .saturating_sub(old_capacity)
            .saturating_mul(MAP_ENTRY_BYTES);
        debug_assert!(actual_bytes <= projected_bytes);
        self.allocated_bytes = self
            .allocated_bytes
            .saturating_sub(projected_bytes.saturating_sub(actual_bytes));
        Ok(())
    }

    pub(in crate::reg_vm) fn reserve_deque_entry_accounted(
        &mut self,
        deque: &mut VecDeque<VmValue>,
    ) -> Result<(), EvalError> {
        if deque.len() < deque.capacity() {
            return Ok(());
        }
        let old_capacity = deque.capacity();
        if self.limits.allocation_budget.is_some() {
            let mut replacement = VecDeque::with_capacity(old_capacity);
            replacement.extend(deque.iter().cloned());
            replacement.try_reserve(1).map_err(|error| {
                EvalError::Runtime(format!("Deque insertion allocation failed: {error}"))
            })?;
            let actual_bytes = replacement
                .capacity()
                .saturating_sub(old_capacity)
                .saturating_mul(std::mem::size_of::<VmValue>());
            self.account_bytes(actual_bytes)?;
            *deque = replacement;
            return Ok(());
        }
        let projected_capacity = old_capacity.saturating_mul(2).saturating_add(1).max(1);
        let projected_bytes = projected_capacity
            .saturating_sub(old_capacity)
            .saturating_mul(std::mem::size_of::<VmValue>());
        deque.try_reserve(1).map_err(|error| {
            EvalError::Runtime(format!("Deque insertion allocation failed: {error}"))
        })?;
        let actual_bytes = deque
            .capacity()
            .saturating_sub(old_capacity)
            .saturating_mul(std::mem::size_of::<VmValue>());
        debug_assert!(actual_bytes <= projected_bytes || self.limits.allocation_budget.is_none());
        Ok(())
    }

    pub(in crate::reg_vm) fn fresh_string(&mut self, value: String) -> Result<VmValue, EvalError> {
        self.account_bytes(value.capacity())?;
        Ok(VmValue::string(value))
    }

    pub(in crate::reg_vm) fn fresh_bytes(&mut self, value: Vec<u8>) -> Result<VmValue, EvalError> {
        self.account_bytes(value.capacity())?;
        Ok(VmValue::Bytes(Rc::new(value)))
    }

    pub(in crate::reg_vm) fn account_fresh_value_storage(
        &mut self,
        value: &VmValue,
    ) -> Result<(), EvalError> {
        match value {
            VmValue::List(values) => self.account_list_storage(&values.borrow()),
            VmValue::Map(values) => {
                self.account_bytes(values.borrow().capacity().saturating_mul(MAP_ENTRY_BYTES))
            }
            VmValue::String(value) => self.account_bytes(value.capacity()),
            VmValue::Bytes(value) => self.account_bytes(value.capacity()),
            VmValue::OptionSomeHeap(value) => self.account_fresh_value_storage(value),
            VmValue::OptionSomeScalar(_) | VmValue::OptionNone => Ok(()),
            VmValue::Variant(value) => {
                for (_, field) in value.iter() {
                    self.account_fresh_value_storage(field)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(in crate::reg_vm) fn storage_node_id(value: &VmValue) -> Option<usize> {
        match value {
            VmValue::Bytes(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::String(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::Json(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::List(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::Deque(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::Map(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::Struct(value) | VmValue::Variant(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::Native(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::Managed(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::Closure(value) => Some(Rc::as_ptr(value) as usize),
            VmValue::Unit
            | VmValue::Int(_)
            | VmValue::Float(_)
            | VmValue::Bool(_)
            | VmValue::Char(_)
            | VmValue::OptionSomeHeap(_)
            | VmValue::OptionNone
            | VmValue::OptionSomeScalar(_) => None,
        }
    }

    pub(in crate::reg_vm) fn collect_storage_nodes(value: &VmValue, nodes: &mut HashSet<usize>) {
        if let Some(node) = Self::storage_node_id(value)
            && !nodes.insert(node)
        {
            return;
        }
        match value {
            VmValue::List(values) => {
                for value in values.borrow().iter() {
                    Self::collect_storage_nodes(&value, nodes);
                }
            }
            VmValue::Deque(values) => {
                for value in values.borrow().iter() {
                    Self::collect_storage_nodes(value, nodes);
                }
            }
            VmValue::Map(values) => {
                for (key, value) in values.borrow().iter() {
                    Self::collect_storage_nodes(key.value(), nodes);
                    Self::collect_storage_nodes(value, nodes);
                }
            }
            VmValue::OptionSomeHeap(value) => Self::collect_storage_nodes(value, nodes),
            VmValue::Struct(value) | VmValue::Variant(value) => {
                for (_, field) in value.iter() {
                    Self::collect_storage_nodes(field, nodes);
                }
            }
            VmValue::Managed(value) => Self::collect_storage_nodes(&value.borrow(), nodes),
            VmValue::Closure(value) => {
                for capture in &value.captures {
                    Self::collect_storage_nodes(capture, nodes);
                }
            }
            VmValue::Unit
            | VmValue::Int(_)
            | VmValue::Float(_)
            | VmValue::Bool(_)
            | VmValue::Char(_)
            | VmValue::Bytes(_)
            | VmValue::String(_)
            | VmValue::Json(_)
            | VmValue::OptionNone
            | VmValue::Native(_)
            | VmValue::OptionSomeScalar(_) => {}
        }
    }

    pub(in crate::reg_vm) fn json_retained_bytes(value: &serde_json::Value) -> usize {
        match value {
            serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
                0
            }
            serde_json::Value::String(value) => value.capacity(),
            serde_json::Value::Array(values) => values
                .capacity()
                .saturating_mul(std::mem::size_of::<serde_json::Value>())
                .saturating_add(
                    values
                        .iter()
                        .map(Self::json_retained_bytes)
                        .fold(0usize, usize::saturating_add),
                ),
            serde_json::Value::Object(values) => {
                values.len().saturating_mul(MAP_ENTRY_BYTES).saturating_add(
                    values
                        .iter()
                        .map(|(key, value)| {
                            key.capacity()
                                .saturating_add(Self::json_retained_bytes(value))
                        })
                        .fold(0usize, usize::saturating_add),
                )
            }
        }
    }

    pub(in crate::reg_vm) fn retained_storage_bytes_inner(
        value: &VmValue,
        excluded: &HashSet<usize>,
        visited: &mut HashSet<usize>,
    ) -> usize {
        if let Some(node) = Self::storage_node_id(value)
            && (excluded.contains(&node) || !visited.insert(node))
        {
            return 0;
        }
        match value {
            VmValue::Unit
            | VmValue::Int(_)
            | VmValue::Float(_)
            | VmValue::Bool(_)
            | VmValue::Char(_)
            | VmValue::OptionNone
            | VmValue::OptionSomeScalar(_) => 0,
            VmValue::Bytes(value) => value.capacity(),
            VmValue::String(value) => value.capacity(),
            VmValue::Json(value) => Self::json_retained_bytes(value),
            VmValue::List(values) => {
                let values = values.borrow();
                values.allocated_bytes().saturating_add(
                    values
                        .iter()
                        .map(|value| Self::retained_storage_bytes_inner(&value, excluded, visited))
                        .fold(0usize, usize::saturating_add),
                )
            }
            VmValue::Deque(values) => {
                let values = values.borrow();
                values
                    .capacity()
                    .saturating_mul(std::mem::size_of::<VmValue>())
                    .saturating_add(
                        values
                            .iter()
                            .map(|value| {
                                Self::retained_storage_bytes_inner(value, excluded, visited)
                            })
                            .fold(0usize, usize::saturating_add),
                    )
            }
            VmValue::Map(values) => {
                let values = values.borrow();
                let nested = values
                    .iter()
                    .map(|(key, value)| {
                        Self::retained_storage_bytes_inner(key.value(), excluded, visited)
                            .saturating_add(Self::retained_storage_bytes_inner(
                                value, excluded, visited,
                            ))
                    })
                    .fold(0usize, usize::saturating_add);
                values
                    .capacity()
                    .saturating_mul(MAP_ENTRY_BYTES)
                    .saturating_add(nested)
            }
            VmValue::OptionSomeHeap(value) => std::mem::size_of::<VmValue>()
                .saturating_add(Self::retained_storage_bytes_inner(value, excluded, visited)),
            VmValue::Struct(value) | VmValue::Variant(value) => value
                .fields
                .capacity()
                .saturating_mul(std::mem::size_of::<VmValue>())
                .saturating_add(
                    value
                        .fields
                        .iter()
                        .map(|field| Self::retained_storage_bytes_inner(field, excluded, visited))
                        .fold(0usize, usize::saturating_add),
                ),
            VmValue::Native(value) => value.type_name.len(),
            VmValue::Managed(value) => std::mem::size_of::<VmValue>().saturating_add(
                Self::retained_storage_bytes_inner(&value.borrow(), excluded, visited),
            ),
            VmValue::Closure(value) => value
                .captures
                .capacity()
                .saturating_mul(std::mem::size_of::<VmValue>())
                .saturating_add(
                    value
                        .captures
                        .iter()
                        .map(|capture| {
                            Self::retained_storage_bytes_inner(capture, excluded, visited)
                        })
                        .fold(0usize, usize::saturating_add),
                ),
        }
    }

    pub(in crate::reg_vm) fn storage_roots_from_regs(
        &self,
        regs: &[Reg],
        base: usize,
    ) -> HashSet<usize> {
        let mut roots = HashSet::new();
        for reg in regs {
            Self::collect_storage_nodes(self.reg(base + *reg), &mut roots);
        }
        roots
    }

    pub(in crate::reg_vm) fn account_result_storage_delta(
        &mut self,
        value: &VmValue,
        excluded: &HashSet<usize>,
        allocated_bytes_before: usize,
    ) -> Result<(), EvalError> {
        if self.limits.allocation_budget.is_none() {
            return Ok(());
        }
        let retained = Self::retained_storage_bytes_inner(value, excluded, &mut HashSet::new());
        let already_charged = self.allocated_bytes.saturating_sub(allocated_bytes_before);
        self.account_bytes(retained.saturating_sub(already_charged))
    }
}
