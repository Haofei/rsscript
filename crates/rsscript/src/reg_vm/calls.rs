use super::*;

impl RegVm {
    pub(super) fn call_native_key(
        &mut self,
        key: &str,
        args: &[Reg],
        mut_args: &[usize],
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let Some(function) = self.native_bindings.get(key).copied() else {
            return Err(EvalError::Runtime(format!(
                "reg VM native function `{key}` has no host binding."
            )));
        };
        let arg_values = args
            .iter()
            .map(|reg| native_value_from_vm_value(self.reg(base + *reg).clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let raw = function(arg_values)
            .map_err(|error| EvalError::Runtime(format!("native host binding failed: {error}")))?;

        // No `mut` params: the binding returns its result directly.
        if mut_args.is_empty() {
            return Ok(vm_value_from_native_value(raw));
        }

        // With `mut` params the shim returns an envelope `List[result, mutated...]`
        // where the mutated values are in `mut_args` order. Write each mutated
        // value back to its arg register so the caller observes the mutation.
        let NativeValue::List(mut envelope) = raw else {
            return Err(EvalError::Runtime(format!(
                "native binding `{key}` was expected to return a mutation envelope."
            )));
        };
        if envelope.len() != mut_args.len() + 1 {
            return Err(EvalError::Runtime(format!(
                "native binding `{key}` returned {} envelope entries, expected {}.",
                envelope.len(),
                mut_args.len() + 1
            )));
        }
        let mutated: Vec<NativeValue> = envelope.split_off(1);
        let result = vm_value_from_native_value(envelope.pop().unwrap_or(NativeValue::Unit));
        for (position, value) in mut_args.iter().zip(mutated) {
            let reg = base + args[*position];
            self.set_reg(reg, vm_value_from_native_value(value));
        }
        Ok(result)
    }

    pub(super) fn call_closure_from_regs(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        arg_regs: &[Reg],
        mut_args: &[usize],
        caller_base: usize,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs)?;
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        let offset = closure.captures.len();
        for (index, reg) in arg_regs.iter().enumerate() {
            let value = self.reg(caller_base + *reg).clone();
            self.set_reg(base + offset + index, value);
        }
        let result = self.run_frame(unit, callee, base)?;
        // `mut` closure arguments are an exclusive borrow for the call: after the
        // closure body runs, write each `mut` parameter's final value back to the
        // caller's argument register, so a `mut Ctx` parameter's field mutations
        // propagate to the caller — identical to `CallKnown`'s `mut_writeback` and
        // to AOT's `&mut` argument semantics. The closure body runs synchronously
        // via `run_frame`, so the parameter registers still hold their final
        // values at `base + offset + pos` here.
        for &pos in mut_args {
            let value = self.reg(base + offset + pos).clone();
            self.set_reg(caller_base + arg_regs[pos], value);
        }
        Ok(result)
    }

    pub(super) fn call_closure_one(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        arg: VmValue,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs)?;
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        self.set_reg(base + closure.captures.len(), arg);
        self.run_frame(unit, callee, base)
    }

    pub(super) fn call_closure_two(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        first: VmValue,
        second: VmValue,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs)?;
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        let offset = closure.captures.len();
        self.set_reg(base + offset, first);
        self.set_reg(base + offset + 1, second);
        self.run_frame(unit, callee, base)
    }

    pub(super) fn call_closure_three(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        first: VmValue,
        second: VmValue,
        third: VmValue,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs)?;
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        let offset = closure.captures.len();
        self.set_reg(base + offset, first);
        self.set_reg(base + offset + 1, second);
        self.set_reg(base + offset + 2, third);
        self.run_frame(unit, callee, base)
    }

    pub(super) fn call_closure_zero(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let callee = Rc::clone(&unit.functions[closure.function]);
        self.prepare_frame(base, callee.regs)?;
        for (index, capture) in closure.captures.iter().enumerate() {
            self.set_reg(base + index, capture.clone());
        }
        self.run_frame(unit, callee, base)
    }

    pub(super) fn channel_state_mut(&mut self, id: i64) -> Result<&mut VmChannel, EvalError> {
        self.channels
            .get_mut(&id)
            .ok_or_else(|| EvalError::Runtime(format!("unknown channel id `{id}`.")))
    }

    /// Store a native tensor handle and return the opaque `VmValue::Native`
    /// carried through the program (mirrors `task_handle_value`).
    pub(super) fn store_tensor(&mut self, tensor: rsscript_runtime::RssTensor) -> VmValue {
        let id = self.next_tensor_id;
        self.next_tensor_id = self.next_tensor_id.saturating_add(1);
        self.tensors.insert(id, tensor);
        VmValue::Native(Rc::new(VmNative {
            type_name: Rc::from("Tensor"),
            id,
        }))
    }

    /// Resolve a `Tensor` handle to the stored `RssTensor` (cloned — the buffer is
    /// `Rc`-shared, so this is a cheap pointer bump, not a data copy).
    pub(super) fn expect_tensor_ref(&self, value: &VmValue) -> Result<rsscript_runtime::RssTensor, EvalError> {
        let id = match value {
            VmValue::Native(native) if native.type_name.as_ref() == "Tensor" => native.id,
            VmValue::Managed(inner) => return self.expect_tensor_ref(&inner.borrow()),
            other => {
                return Err(EvalError::Runtime(format!(
                    "reg VM expected Tensor, got `{}`.",
                    other.display()
                )));
            }
        };
        self.tensors
            .get(&id)
            .cloned()
            .ok_or_else(|| EvalError::Runtime(format!("unknown tensor id `{id}`.")))
    }

    pub(super) fn channel_send(&mut self, sender: VmSender, value: VmValue) -> Result<VmValue, VmValue> {
        if sender.closed {
            return Err(channel_error_value("channel sender closed"));
        }
        let state = self.channels.get_mut(&sender.channel_id).ok_or_else(|| {
            channel_error_value(format!("unknown channel id `{}`", sender.channel_id))
        })?;
        if state.receiver_closed {
            return Err(channel_error_value("channel closed"));
        }
        if state.queue.len() >= state.capacity {
            return Err(channel_error_value(
                "channel send would block on a full channel in the VM",
            ));
        }
        state.queue.push_back(value);
        Ok(VmValue::Unit)
    }

    pub(super) fn channel_recv(&mut self, channel_id: i64) -> Result<VmValue, VmValue> {
        let state = self
            .channels
            .get_mut(&channel_id)
            .ok_or_else(|| channel_error_value(format!("unknown channel id `{channel_id}`")))?;
        if let Some(value) = state.queue.pop_front() {
            return Ok(VmValue::some(value));
        }
        if state.senders == 0 {
            return Ok(VmValue::OptionNone);
        }
        Err(channel_error_value(
            "channel recv would block on an open empty channel in the VM",
        ))
    }

    pub(super) fn filter_list(
        &mut self,
        unit: &RegUnit,
        list: Rc<RefCell<TypedVec>>,
        predicate: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let len = list.borrow().len();
        let mut filtered = Vec::with_capacity(len);
        for index in 0..len {
            let item = list_item_at(&list, index, "List.filter")?;
            let keep = self.call_closure_one(unit, predicate, item.clone(), base)?;
            if expect_bool_ref(&keep)? {
                filtered.push(item);
            }
        }
        Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(filtered)))))
    }

    pub(super) fn fold_list(
        &mut self,
        unit: &RegUnit,
        list: Rc<RefCell<TypedVec>>,
        mut state: VmValue,
        folder: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        // Fast path: a fold whose folder is a recognized simple numeric binary
        // closure (`|acc, x| acc <op> x`) over a list of scalar `Int`/`Float`
        // values is the hot shape for sum/product-style reductions. Running it as
        // a tight loop over the element values — calling the *same*
        // `eval_numeric_binary` the interpreter uses, in the *same* operand
        // order, on the *same* values — avoids a full frame setup + bytecode
        // dispatch per element while producing bit-identical results (identical
        // f64 ops, order, NaN/inf, and error behavior). Any case that does not
        // exactly match (wrong shape, non-scalar element, captures present)
        // falls through to the generic interpreter path below.
        if let Some(form) = recognize_numeric_binary_closure(unit, folder) {
            if matches!(state, VmValue::Int(_) | VmValue::Float(_)) {
                let list = list.borrow();
                if list
                    .iter()
                    .all(|item| matches!(item, VmValue::Int(_) | VmValue::Float(_)))
                {
                    for item in list.iter() {
                        // Preserve the closure's operand order exactly: `state`
                        // and `item` are placed at the two param registers, so
                        // whichever param the lhs/rhs reads determines the order.
                        let (lhs, rhs) = if form.lhs_is_state {
                            (&state, &item)
                        } else {
                            (&item, &state)
                        };
                        state = eval_numeric_binary(form.op, lhs, rhs)?;
                    }
                    return Ok(state);
                }
            }
        }
        let len = list.borrow().len();
        for index in 0..len {
            let item = list_item_at(&list, index, "List.fold")?;
            state = self.call_closure_two(unit, folder, state, item, base)?;
        }
        Ok(state)
    }

    pub(super) fn map_list(
        &mut self,
        unit: &RegUnit,
        list: Rc<RefCell<TypedVec>>,
        mapper: &VmClosure,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let len = list.borrow().len();
        let mut mapped = Vec::with_capacity(len);
        for index in 0..len {
            let item = list_item_at(&list, index, "List.map")?;
            mapped.push(self.call_closure_one(unit, mapper, item, base)?);
        }
        Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(mapped)))))
    }

    pub(super) fn sort_list_with_closure(
        &mut self,
        unit: &RegUnit,
        list: &mut [VmValue],
        compare: &VmClosure,
        base: usize,
    ) -> Result<(), EvalError> {
        for right_index in 1..list.len() {
            let mut index = right_index;
            while index > 0 {
                let ordering = self.call_closure_two(
                    unit,
                    compare,
                    list[index - 1].clone(),
                    list[index].clone(),
                    base,
                )?;
                if expect_int_ref(&ordering)? <= 0 {
                    break;
                }
                list.swap(index - 1, index);
                index -= 1;
            }
        }
        Ok(())
    }

    pub(super) fn sort_list_by_closure(
        &mut self,
        unit: &RegUnit,
        mut list: Vec<VmValue>,
        key: &VmClosure,
        compare: &VmClosure,
        base: usize,
    ) -> Result<Vec<VmValue>, EvalError> {
        for right_index in 1..list.len() {
            let mut index = right_index;
            while index > 0 {
                let left_key = self.call_closure_one(unit, key, list[index - 1].clone(), base)?;
                let right_key = self.call_closure_one(unit, key, list[index].clone(), base)?;
                let ordering = self.call_closure_two(unit, compare, left_key, right_key, base)?;
                if expect_int_ref(&ordering)? <= 0 {
                    break;
                }
                list.swap(index - 1, index);
                index -= 1;
            }
        }
        Ok(list)
    }

    pub(super) fn call_typed_intrinsic(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        type_arg: &str,
        args: &[Reg],
        base: usize,
    ) -> Result<VmValue, EvalError> {
        self.charge_host_call()?;
        match intrinsic {
            RegIntrinsic::JsonDecode => {
                let value = expect_json_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(json_decode_struct_value(unit, type_arg, value)))
            }
            RegIntrinsic::JsonDecodeText => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(json_result(parse_json_text(text).and_then(|value| {
                    json_decode_struct_value(unit, type_arg, &value)
                })))
            }
            RegIntrinsic::ListNew => {
                // TV1: an empty list starts in its static element kind so a
                // homogeneous scalar list never pays the boxed buffer.
                let kind = crate::vm_value::ElemKind::from_type_name(type_arg);
                Ok(VmValue::List(Rc::new(RefCell::new(kind.empty()))))
            }
            other => Err(EvalError::Runtime(format!(
                "reg VM typed intrinsic `{other:?}` is not implemented."
            ))),
        }
    }
}
