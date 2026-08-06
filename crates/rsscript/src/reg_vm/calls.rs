use super::*;

const NATIVE_VALUE_MAX_DEPTH: usize = 128;
const NATIVE_VALUE_MAX_NODES: usize = 1_000_000;

fn native_json_storage_estimate(
    value: &serde_json::Value,
    depth: usize,
    nodes: &mut usize,
) -> Result<usize, EvalError> {
    if depth > NATIVE_VALUE_MAX_DEPTH {
        return Err(EvalError::Runtime(
            "native binding result exceeds the maximum value depth".into(),
        ));
    }
    *nodes = nodes.saturating_add(1);
    if *nodes > NATIVE_VALUE_MAX_NODES {
        return Err(EvalError::Runtime(
            "native binding result exceeds the maximum value node count".into(),
        ));
    }
    Ok(match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => 0,
        serde_json::Value::String(value) => value.capacity(),
        serde_json::Value::Array(values) => {
            let mut bytes = values
                .capacity()
                .saturating_mul(std::mem::size_of::<serde_json::Value>());
            for value in values {
                bytes =
                    bytes.saturating_add(native_json_storage_estimate(value, depth + 1, nodes)?);
            }
            bytes
        }
        serde_json::Value::Object(values) => {
            let mut bytes = values.len().saturating_mul(MAP_ENTRY_BYTES);
            for (key, value) in values {
                bytes = bytes
                    .saturating_add(key.capacity())
                    .saturating_add(native_json_storage_estimate(value, depth + 1, nodes)?);
            }
            bytes
        }
    })
}

fn native_value_storage_estimate_inner(
    value: &NativeValue,
    depth: usize,
    nodes: &mut usize,
) -> Result<usize, EvalError> {
    if depth > NATIVE_VALUE_MAX_DEPTH {
        return Err(EvalError::Runtime(
            "native binding result exceeds the maximum value depth".into(),
        ));
    }
    *nodes = nodes.saturating_add(1);
    if *nodes > NATIVE_VALUE_MAX_NODES {
        return Err(EvalError::Runtime(
            "native binding result exceeds the maximum value node count".into(),
        ));
    }
    Ok(match value {
        NativeValue::Unit
        | NativeValue::Int(_)
        | NativeValue::Float(_)
        | NativeValue::Bool(_)
        | NativeValue::Char(_) => 0,
        NativeValue::String(value) => value.capacity(),
        NativeValue::Bytes(value) => value.capacity(),
        NativeValue::List(values) => {
            let mut bytes = values
                .capacity()
                .saturating_mul(std::mem::size_of::<NativeValue>());
            for value in values {
                bytes = bytes.saturating_add(native_value_storage_estimate_inner(
                    value,
                    depth + 1,
                    nodes,
                )?);
            }
            bytes
        }
        NativeValue::Map(entries) => {
            let mut bytes = entries
                .capacity()
                .saturating_mul(std::mem::size_of::<(NativeValue, NativeValue)>());
            for (key, value) in entries {
                bytes = bytes
                    .saturating_add(native_value_storage_estimate_inner(key, depth + 1, nodes)?)
                    .saturating_add(native_value_storage_estimate_inner(
                        value,
                        depth + 1,
                        nodes,
                    )?);
            }
            bytes
        }
        NativeValue::Json(value) => native_json_storage_estimate(value, depth + 1, nodes)?,
        NativeValue::Struct { name, fields } | NativeValue::Variant { name, fields } => {
            let mut bytes = name
                .capacity()
                .saturating_add(fields.len().saturating_mul(std::mem::size_of::<VmValue>()));
            for (field, value) in fields {
                bytes = bytes.saturating_add(field.capacity()).saturating_add(
                    native_value_storage_estimate_inner(value, depth + 1, nodes)?,
                );
            }
            bytes
        }
        NativeValue::Native { type_name, .. } => type_name.capacity(),
    })
}

fn native_value_storage_estimate(value: &NativeValue) -> Result<usize, EvalError> {
    let mut nodes = 0;
    native_value_storage_estimate_inner(value, 0, &mut nodes)
}

fn compact_native_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::String(value) => value.shrink_to_fit(),
        serde_json::Value::Array(values) => {
            for value in values.iter_mut() {
                compact_native_json(value);
            }
            values.shrink_to_fit();
        }
        serde_json::Value::Object(values) => {
            // serde_json does not expose Map capacity. Rebuilding prevents an
            // otherwise empty native object from retaining an arbitrary amount.
            let old = std::mem::take(values);
            let mut compact = serde_json::Map::with_capacity(old.len());
            for (mut key, mut value) in old {
                key.shrink_to_fit();
                compact_native_json(&mut value);
                compact.insert(key, value);
            }
            *values = compact;
        }
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {}
    }
}

fn compact_native_json_values(value: &mut NativeValue) {
    match value {
        NativeValue::Json(value) => compact_native_json(value),
        NativeValue::List(values) => {
            for value in values {
                compact_native_json_values(value);
            }
        }
        NativeValue::Map(entries) => {
            for (key, value) in entries {
                compact_native_json_values(key);
                compact_native_json_values(value);
            }
        }
        NativeValue::Struct { fields, .. } | NativeValue::Variant { fields, .. } => {
            for value in fields.values_mut() {
                compact_native_json_values(value);
            }
        }
        NativeValue::Unit
        | NativeValue::Int(_)
        | NativeValue::Float(_)
        | NativeValue::Bool(_)
        | NativeValue::Char(_)
        | NativeValue::String(_)
        | NativeValue::Bytes(_)
        | NativeValue::Native { .. } => {}
    }
}

#[derive(Clone)]
pub(super) struct PureClosurePlan {
    regs: usize,
    code: Vec<PureClosureInstr>,
}

#[derive(Clone)]
enum PureClosureInstr {
    LoadUnit {
        dst: Reg,
    },
    LoadInt {
        dst: Reg,
        value: i64,
    },
    LoadFloat {
        dst: Reg,
        value: f64,
    },
    LoadBool {
        dst: Reg,
        value: bool,
    },
    Move {
        dst: Reg,
        src: Reg,
    },
    Numeric {
        op: BinaryOp,
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    IntCompare {
        op: RegIntCompare,
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Equal {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
        negated: bool,
    },
    GetFieldSlot {
        dst: Reg,
        base: Reg,
        slot: usize,
    },
    MakeStruct {
        dst: Reg,
        layout: Rc<TypeLayout>,
        fields: Vec<Reg>,
    },
    Return {
        src: Reg,
    },
    RuntimeError {
        message: String,
    },
}

#[derive(Clone, Copy)]
enum PureIntScalar {
    Int(i64),
    Bool(bool),
}

struct IntStructFoldPlan {
    layout: Rc<TypeLayout>,
    fields: Vec<PureIntExpr>,
}

enum IntPipelineStage {
    Map(PureIntExpr),
    Filter(PureBoolExpr),
}

type IntPipelinePrefix = (Reg, Vec<IntPipelineStage>, Reg, usize, Vec<Reg>);

#[derive(Clone)]
enum PureIntExpr {
    StateField(usize),
    Item,
    Const(i64),
    Binary {
        op: BinaryOp,
        lhs: Box<PureIntExpr>,
        rhs: Box<PureIntExpr>,
    },
}

#[derive(Clone)]
enum PureBoolExpr {
    Const(bool),
    Compare {
        op: RegIntCompare,
        lhs: Box<PureIntExpr>,
        rhs: Box<PureIntExpr>,
    },
    EqualInt {
        lhs: Box<PureIntExpr>,
        rhs: Box<PureIntExpr>,
        negated: bool,
    },
}

#[derive(Clone)]
enum PureExprSymbol {
    Int(PureIntExpr),
    Bool(PureBoolExpr),
}

#[derive(Clone)]
enum PureFoldSymbol {
    State,
    Int(PureIntExpr),
    IntStruct {
        layout: Rc<TypeLayout>,
        fields: Vec<PureIntExpr>,
    },
}

impl RegVm {
    pub(super) fn call_external_symbol(
        &mut self,
        key: &str,
        args: &[Reg],
        mut_args: &[usize],
        base: usize,
    ) -> Result<VmValue, EvalError> {
        self.charge_provider_call()?;
        let Some(function) = self.external_bindings.get(key).cloned() else {
            return Err(EvalError::Runtime(format!(
                "reg VM native function `{key}` has no host binding."
            )));
        };
        let arg_values = args
            .iter()
            .map(|reg| native_value_from_vm_value(self.reg(base + *reg).clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let cancellation = self.limits.cancel.clone();
        let mut context = ProviderCallContext {
            cancellation: cancellation.as_ref(),
            deadline: self.limits.deadline,
            remaining_byte_budget: self
                .limits
                .allocation_budget
                .map(|limit| limit.saturating_sub(self.allocated_bytes)),
            remaining_output_budget: self
                .limits
                .stdout_budget
                .map(|limit| limit.saturating_sub(self.stdout.len())),
            call_id: rsscript_operation::OperationId(self.provider_calls),
            resources: Some(&mut self.provider_resources),
        };
        let mut raw = function
            .call_with_context(&mut context, arg_values)
            .map_err(EvalError::Provider)?;

        // No `mut` params: the binding returns its result directly.
        if mut_args.is_empty() {
            let source_bytes = native_value_storage_estimate(&raw)?;
            self.ensure_memory_available(source_bytes)?;
            compact_native_json_values(&mut raw);
            let value = vm_value_from_native_value(raw)?;
            let retained_bytes =
                Self::retained_storage_bytes_inner(&value, &HashSet::new(), &mut HashSet::new());
            let bytes = source_bytes.max(retained_bytes);
            self.ensure_memory_available(bytes)?;
            self.account_bytes(bytes)?;
            return Ok(value);
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
        let mut mutated: Vec<NativeValue> = envelope.split_off(1);
        let mut result_raw = envelope.pop().unwrap_or(NativeValue::Unit);
        let mut source_bytes = native_value_storage_estimate(&result_raw)?;
        for value in &mutated {
            source_bytes = source_bytes.saturating_add(native_value_storage_estimate(value)?);
        }
        self.ensure_memory_available(source_bytes)?;
        compact_native_json_values(&mut result_raw);
        for value in &mut mutated {
            compact_native_json_values(value);
        }
        let result = vm_value_from_native_value(result_raw)?;
        let mutated = mutated
            .into_iter()
            .map(vm_value_from_native_value)
            .collect::<Result<Vec<_>, _>>()?;
        let mut retained_bytes =
            Self::retained_storage_bytes_inner(&result, &HashSet::new(), &mut HashSet::new());
        for value in &mutated {
            retained_bytes = retained_bytes.saturating_add(Self::retained_storage_bytes_inner(
                value,
                &HashSet::new(),
                &mut HashSet::new(),
            ));
        }
        let bytes = source_bytes.max(retained_bytes);
        self.ensure_memory_available(bytes)?;
        self.account_bytes(bytes)?;
        for (position, value) in mut_args.iter().zip(mutated) {
            let reg = base + args[*position];
            self.set_reg(reg, value);
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

    pub(super) fn channel_send(
        &mut self,
        sender: VmSender,
        value: VmValue,
    ) -> Result<VmValue, VmValue> {
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
        let pure_predicate = self.captureless_pure_closure_plan(unit, predicate, 1);
        if let Some(ref plan) = pure_predicate {
            if let Some(result) = Self::try_filter_int_list_with_plan(&list, plan) {
                return result;
            }
        }
        let mut pure_regs = Vec::new();
        for index in 0..len {
            let item = list_item_at(&list, index, "List.filter")?;
            let keep = match pure_predicate {
                Some(ref plan) => Self::eval_captureless_pure_closure(
                    plan,
                    std::slice::from_ref(&item),
                    &mut pure_regs,
                )?,
                None => self.call_closure_one(unit, predicate, item.clone(), base)?,
            };
            if expect_bool_ref(&keep)? {
                filtered.push(item);
            }
        }
        Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
            filtered,
        )))))
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
        let pure_folder = self.captureless_pure_closure_plan(unit, folder, 2);
        if let Some(ref plan) = pure_folder {
            if let Some(result) = Self::try_fold_int_list_with_struct_plan(&list, &state, plan) {
                return result;
            }
        }
        let mut pure_regs = Vec::new();
        for index in 0..len {
            let item = list_item_at(&list, index, "List.fold")?;
            state = match pure_folder {
                Some(ref plan) => Self::eval_captureless_pure_closure(
                    plan,
                    &[state.clone(), item.clone()],
                    &mut pure_regs,
                )?,
                None => self.call_closure_two(unit, folder, state, item, base)?,
            };
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
        let pure_mapper = self.captureless_pure_closure_plan(unit, mapper, 1);
        if let Some(ref plan) = pure_mapper {
            if let Some(result) = Self::try_map_int_list_with_plan(&list, plan) {
                return result;
            }
        }
        let mut pure_regs = Vec::new();
        for index in 0..len {
            let item = list_item_at(&list, index, "List.map")?;
            let mapped_value = match pure_mapper {
                Some(ref plan) => Self::eval_captureless_pure_closure(
                    plan,
                    std::slice::from_ref(&item),
                    &mut pure_regs,
                )?,
                None => self.call_closure_one(unit, mapper, item, base)?,
            };
            mapped.push(mapped_value);
        }
        Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
            mapped,
        )))))
    }

    fn try_filter_int_list_with_plan(
        list: &Rc<RefCell<TypedVec>>,
        plan: &PureClosurePlan,
    ) -> Option<Result<VmValue, EvalError>> {
        let borrowed = list.borrow();
        let TypedVec::Ints(values) = &*borrowed else {
            return None;
        };
        let mut filtered = Vec::with_capacity(values.len());
        let mut regs = Vec::new();
        for &value in values {
            match Self::eval_captureless_pure_int_closure(plan, value, &mut regs) {
                Some(Ok(PureIntScalar::Bool(true))) => filtered.push(value),
                Some(Ok(PureIntScalar::Bool(false))) => {}
                Some(Ok(PureIntScalar::Int(_))) => return None,
                Some(Err(error)) => return Some(Err(error)),
                None => return None,
            }
        }
        Some(Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::Ints(
            filtered,
        ))))))
    }

    fn try_map_int_list_with_plan(
        list: &Rc<RefCell<TypedVec>>,
        plan: &PureClosurePlan,
    ) -> Option<Result<VmValue, EvalError>> {
        let borrowed = list.borrow();
        let TypedVec::Ints(values) = &*borrowed else {
            return None;
        };
        let mut mapped = Vec::with_capacity(values.len());
        let mut regs = Vec::new();
        for &value in values {
            match Self::eval_captureless_pure_int_closure(plan, value, &mut regs) {
                Some(Ok(PureIntScalar::Int(value))) => mapped.push(value),
                Some(Ok(PureIntScalar::Bool(_))) => return None,
                Some(Err(error)) => return Some(Err(error)),
                None => return None,
            }
        }
        Some(Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::Ints(
            mapped,
        ))))))
    }

    pub(super) fn try_fuse_int_list_pipeline_at(
        &mut self,
        unit: &RegUnit,
        code: &[RegInstr],
        ip: usize,
        base: usize,
    ) -> Result<Option<usize>, EvalError> {
        let Some(instr) = code.get(ip) else {
            return Ok(None);
        };

        let Some((source, stages, current_reg, after_stages, mut temps)) =
            self.fusable_int_pipeline_prefix(unit, code, instr, ip, base)?
        else {
            return Ok(None);
        };
        let Some((fold_ip, dst, state, folder, collect_temp)) =
            Self::fusable_fold_sink(code, after_stages, current_reg)
        else {
            return Ok(None);
        };
        if let Some(temp) = collect_temp {
            temps.push(temp);
        }
        if !Self::pipeline_temps_dead_after(code, fold_ip + 1, &temps) {
            return Ok(None);
        }
        let list = expect_list_ref(self.reg(base + source))?;
        let state = self.reg(base + state).clone();
        let folder = expect_closure_rc(self.reg(base + folder))?;
        let Some(folder_plan) = self.captureless_pure_closure_plan(unit, &folder, 2) else {
            return Ok(None);
        };
        let Some(result) =
            Self::try_fold_int_pipeline_with_struct_plan(&list, &stages, &state, &folder_plan)
        else {
            return Ok(None);
        };
        self.set_reg(base + dst, result?);
        Ok(Some(fold_ip + 1))
    }

    fn fusable_int_pipeline_prefix(
        &mut self,
        unit: &RegUnit,
        code: &[RegInstr],
        instr: &RegInstr,
        ip: usize,
        base: usize,
    ) -> Result<Option<IntPipelinePrefix>, EvalError> {
        match instr {
            RegInstr::ListMap { dst, list, mapper } => {
                let Some(RegInstr::ListFilter {
                    dst: filter_dst,
                    list: filter_list,
                    predicate,
                }) = code.get(ip + 1)
                else {
                    return Ok(None);
                };
                if *filter_list != *dst {
                    return Ok(None);
                }
                let Some(mapper) = self.compile_pure_closure_from_reg(unit, base, *mapper, 1)?
                else {
                    return Ok(None);
                };
                let Some(predicate) =
                    self.compile_pure_closure_from_reg(unit, base, *predicate, 1)?
                else {
                    return Ok(None);
                };
                let Some(mapper) = Self::compile_int_expr_closure(&mapper) else {
                    return Ok(None);
                };
                let Some(predicate) = Self::compile_bool_expr_closure(&predicate) else {
                    return Ok(None);
                };
                Ok(Some((
                    *list,
                    vec![
                        IntPipelineStage::Map(mapper),
                        IntPipelineStage::Filter(predicate),
                    ],
                    *filter_dst,
                    ip + 2,
                    vec![*dst, *filter_dst],
                )))
            }
            RegInstr::ListFilter {
                dst,
                list,
                predicate,
            } => {
                let Some(RegInstr::ListMap {
                    dst: map_dst,
                    list: map_list,
                    mapper,
                }) = code.get(ip + 1)
                else {
                    return Ok(None);
                };
                if *map_list != *dst {
                    return Ok(None);
                }
                let Some(predicate) =
                    self.compile_pure_closure_from_reg(unit, base, *predicate, 1)?
                else {
                    return Ok(None);
                };
                let Some(mapper) = self.compile_pure_closure_from_reg(unit, base, *mapper, 1)?
                else {
                    return Ok(None);
                };
                let Some(predicate) = Self::compile_bool_expr_closure(&predicate) else {
                    return Ok(None);
                };
                let Some(mapper) = Self::compile_int_expr_closure(&mapper) else {
                    return Ok(None);
                };
                Ok(Some((
                    *list,
                    vec![
                        IntPipelineStage::Filter(predicate),
                        IntPipelineStage::Map(mapper),
                    ],
                    *map_dst,
                    ip + 2,
                    vec![*dst, *map_dst],
                )))
            }
            _ => Ok(None),
        }
    }

    fn compile_pure_closure_from_reg(
        &mut self,
        unit: &RegUnit,
        base: usize,
        reg: Reg,
        args_len: usize,
    ) -> Result<Option<PureClosurePlan>, EvalError> {
        let closure = expect_closure_rc(self.reg(base + reg))?;
        Ok(self.captureless_pure_closure_plan(unit, &closure, args_len))
    }

    fn fusable_fold_sink(
        code: &[RegInstr],
        after_stages: usize,
        current_reg: Reg,
    ) -> Option<(usize, Reg, Reg, Reg, Option<Reg>)> {
        match code.get(after_stages)? {
            RegInstr::ListFold {
                dst,
                list,
                state,
                folder,
            } if *list == current_reg => Some((after_stages, *dst, *state, *folder, None)),
            RegInstr::CallIntrinsic {
                dst: collect_dst,
                intrinsic: RegIntrinsic::PipelineCollect,
                args,
            } if args.as_slice() == [current_reg] => match code.get(after_stages + 1)? {
                RegInstr::ListFold {
                    dst,
                    list,
                    state,
                    folder,
                } if *list == *collect_dst => {
                    Some((after_stages + 1, *dst, *state, *folder, Some(*collect_dst)))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn pipeline_temps_dead_after(code: &[RegInstr], start: usize, temps: &[Reg]) -> bool {
        !code
            .iter()
            .skip(start)
            .any(|instr| temps.iter().any(|reg| instr_reads_register(instr, *reg)))
    }

    fn try_fold_int_pipeline_with_struct_plan(
        list: &Rc<RefCell<TypedVec>>,
        stages: &[IntPipelineStage],
        state: &VmValue,
        folder: &PureClosurePlan,
    ) -> Option<Result<VmValue, EvalError>> {
        let borrowed = list.borrow();
        let TypedVec::Ints(values) = &*borrowed else {
            return None;
        };
        let fold = Self::compile_int_struct_fold_plan(folder)?;
        let (_state_layout, mut state_fields) = Self::pure_int_struct_parts_from_value(state)?;
        let mut next_fields = vec![0; fold.fields.len()];

        'items: for &source_value in values {
            let mut value = source_value;
            for stage in stages {
                match stage {
                    IntPipelineStage::Map(expr) => {
                        value = match Self::eval_pure_int_expr(expr, &[], value) {
                            Some(Ok(value)) => value,
                            Some(Err(error)) => return Some(Err(error)),
                            None => return None,
                        }
                    }
                    IntPipelineStage::Filter(expr) => {
                        let keep = match Self::eval_pure_bool_expr(expr, &[], value) {
                            Some(Ok(value)) => value,
                            Some(Err(error)) => return Some(Err(error)),
                            None => return None,
                        };
                        if !keep {
                            continue 'items;
                        }
                    }
                }
            }

            for (index, expr) in fold.fields.iter().enumerate() {
                let evaluated = match Self::eval_pure_int_expr(expr, &state_fields, value) {
                    Some(Ok(value)) => value,
                    Some(Err(error)) => return Some(Err(error)),
                    None => return None,
                };
                next_fields[index] = evaluated;
            }
            std::mem::swap(&mut state_fields, &mut next_fields);
        }

        Some(Ok(VmValue::Struct(Rc::new(VmStruct::with_layout(
            fold.layout,
            state_fields.into_iter().map(VmValue::Int).collect(),
        )))))
    }

    fn try_fold_int_list_with_struct_plan(
        list: &Rc<RefCell<TypedVec>>,
        state: &VmValue,
        plan: &PureClosurePlan,
    ) -> Option<Result<VmValue, EvalError>> {
        let borrowed = list.borrow();
        let TypedVec::Ints(values) = &*borrowed else {
            return None;
        };
        let fold = Self::compile_int_struct_fold_plan(plan)?;
        let (_state_layout, mut state_fields) = Self::pure_int_struct_parts_from_value(state)?;
        let mut next_fields = vec![0; fold.fields.len()];
        for &value in values {
            for (index, expr) in fold.fields.iter().enumerate() {
                let evaluated = match Self::eval_pure_int_expr(expr, &state_fields, value) {
                    Some(Ok(value)) => value,
                    Some(Err(error)) => return Some(Err(error)),
                    None => return None,
                };
                next_fields[index] = evaluated;
            }
            std::mem::swap(&mut state_fields, &mut next_fields);
        }
        Some(Ok(VmValue::Struct(Rc::new(VmStruct::with_layout(
            fold.layout,
            state_fields.into_iter().map(VmValue::Int).collect(),
        )))))
    }

    fn pure_int_struct_parts_from_value(value: &VmValue) -> Option<(Rc<TypeLayout>, Vec<i64>)> {
        match value {
            VmValue::Struct(data) => {
                let mut fields = Vec::with_capacity(data.fields.len());
                for field in &data.fields {
                    let VmValue::Int(value) = field else {
                        return None;
                    };
                    fields.push(*value);
                }
                Some((Rc::clone(&data.layout), fields))
            }
            VmValue::Managed(inner) => Self::pure_int_struct_parts_from_value(&inner.borrow()),
            _ => None,
        }
    }

    fn compile_int_struct_fold_plan(plan: &PureClosurePlan) -> Option<IntStructFoldPlan> {
        let mut regs = vec![None; plan.regs];
        regs[0] = Some(PureFoldSymbol::State);
        regs[1] = Some(PureFoldSymbol::Int(PureIntExpr::Item));

        let read = |regs: &[Option<PureFoldSymbol>], reg: usize| -> Option<PureFoldSymbol> {
            regs.get(reg).and_then(Option::as_ref).cloned()
        };
        let write = |regs: &mut [Option<PureFoldSymbol>],
                     reg: usize,
                     value: PureFoldSymbol|
         -> Option<()> {
            *regs.get_mut(reg)? = Some(value);
            Some(())
        };
        let read_int = |regs: &[Option<PureFoldSymbol>], reg: usize| -> Option<PureIntExpr> {
            match read(regs, reg)? {
                PureFoldSymbol::Int(expr) => Some(expr),
                PureFoldSymbol::State | PureFoldSymbol::IntStruct { .. } => None,
            }
        };

        for instr in &plan.code {
            match instr {
                PureClosureInstr::LoadInt { dst, value } => write(
                    regs.as_mut_slice(),
                    *dst,
                    PureFoldSymbol::Int(PureIntExpr::Const(*value)),
                )?,
                PureClosureInstr::Move { dst, src } => {
                    let value = read(regs.as_slice(), *src)?;
                    write(regs.as_mut_slice(), *dst, value)?;
                }
                PureClosureInstr::Numeric { op, dst, lhs, rhs } => {
                    let lhs = read_int(regs.as_slice(), *lhs)?;
                    let rhs = read_int(regs.as_slice(), *rhs)?;
                    write(
                        regs.as_mut_slice(),
                        *dst,
                        PureFoldSymbol::Int(PureIntExpr::Binary {
                            op: *op,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        }),
                    )?;
                }
                PureClosureInstr::GetFieldSlot { dst, base, slot } => {
                    let expr = match read(regs.as_slice(), *base)? {
                        PureFoldSymbol::State => PureIntExpr::StateField(*slot),
                        PureFoldSymbol::IntStruct { fields, .. } => fields.get(*slot)?.clone(),
                        PureFoldSymbol::Int(_) => return None,
                    };
                    write(regs.as_mut_slice(), *dst, PureFoldSymbol::Int(expr))?;
                }
                PureClosureInstr::MakeStruct {
                    dst,
                    layout,
                    fields,
                } => {
                    let mut exprs = Vec::with_capacity(fields.len());
                    for field in fields {
                        exprs.push(read_int(regs.as_slice(), *field)?);
                    }
                    write(
                        regs.as_mut_slice(),
                        *dst,
                        PureFoldSymbol::IntStruct {
                            layout: Rc::clone(layout),
                            fields: exprs,
                        },
                    )?;
                }
                PureClosureInstr::Return { src } => {
                    let PureFoldSymbol::IntStruct { layout, fields } = read(regs.as_slice(), *src)?
                    else {
                        return None;
                    };
                    return Some(IntStructFoldPlan { layout, fields });
                }
                PureClosureInstr::RuntimeError { .. }
                | PureClosureInstr::LoadUnit { .. }
                | PureClosureInstr::LoadFloat { .. }
                | PureClosureInstr::LoadBool { .. }
                | PureClosureInstr::IntCompare { .. }
                | PureClosureInstr::Equal { .. } => return None,
            }
        }
        None
    }

    fn eval_pure_int_expr(
        expr: &PureIntExpr,
        state_fields: &[i64],
        item: i64,
    ) -> Option<Result<i64, EvalError>> {
        Some(match expr {
            PureIntExpr::StateField(slot) => Ok(*state_fields.get(*slot)?),
            PureIntExpr::Item => Ok(item),
            PureIntExpr::Const(value) => Ok(*value),
            PureIntExpr::Binary { op, lhs, rhs } => {
                let lhs = match Self::eval_pure_int_expr(lhs, state_fields, item)? {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let rhs = match Self::eval_pure_int_expr(rhs, state_fields, item)? {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                Self::eval_pure_int_binary(*op, lhs, rhs)
            }
        })
    }

    fn compile_int_expr_closure(plan: &PureClosurePlan) -> Option<PureIntExpr> {
        match Self::compile_unary_int_item_expr_closure(plan)? {
            PureExprSymbol::Int(expr) => Some(expr),
            PureExprSymbol::Bool(_) => None,
        }
    }

    fn compile_bool_expr_closure(plan: &PureClosurePlan) -> Option<PureBoolExpr> {
        match Self::compile_unary_int_item_expr_closure(plan)? {
            PureExprSymbol::Bool(expr) => Some(expr),
            PureExprSymbol::Int(_) => None,
        }
    }

    fn compile_unary_int_item_expr_closure(plan: &PureClosurePlan) -> Option<PureExprSymbol> {
        let mut regs = vec![None; plan.regs];
        regs[0] = Some(PureExprSymbol::Int(PureIntExpr::Item));

        let read = |regs: &[Option<PureExprSymbol>], reg: usize| -> Option<PureExprSymbol> {
            regs.get(reg).and_then(Option::as_ref).cloned()
        };
        let write = |regs: &mut [Option<PureExprSymbol>],
                     reg: usize,
                     value: PureExprSymbol|
         -> Option<()> {
            *regs.get_mut(reg)? = Some(value);
            Some(())
        };
        let read_int = |regs: &[Option<PureExprSymbol>], reg: usize| -> Option<PureIntExpr> {
            match read(regs, reg)? {
                PureExprSymbol::Int(expr) => Some(expr),
                PureExprSymbol::Bool(_) => None,
            }
        };

        for instr in &plan.code {
            match instr {
                PureClosureInstr::LoadInt { dst, value } => write(
                    regs.as_mut_slice(),
                    *dst,
                    PureExprSymbol::Int(PureIntExpr::Const(*value)),
                )?,
                PureClosureInstr::LoadBool { dst, value } => write(
                    regs.as_mut_slice(),
                    *dst,
                    PureExprSymbol::Bool(PureBoolExpr::Const(*value)),
                )?,
                PureClosureInstr::Move { dst, src } => {
                    let value = read(regs.as_slice(), *src)?;
                    write(regs.as_mut_slice(), *dst, value)?;
                }
                PureClosureInstr::Numeric { op, dst, lhs, rhs } => {
                    let lhs = read_int(regs.as_slice(), *lhs)?;
                    let rhs = read_int(regs.as_slice(), *rhs)?;
                    write(
                        regs.as_mut_slice(),
                        *dst,
                        PureExprSymbol::Int(PureIntExpr::Binary {
                            op: *op,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        }),
                    )?;
                }
                PureClosureInstr::IntCompare { op, dst, lhs, rhs } => {
                    let lhs = read_int(regs.as_slice(), *lhs)?;
                    let rhs = read_int(regs.as_slice(), *rhs)?;
                    write(
                        regs.as_mut_slice(),
                        *dst,
                        PureExprSymbol::Bool(PureBoolExpr::Compare {
                            op: *op,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                        }),
                    )?;
                }
                PureClosureInstr::Equal {
                    dst,
                    lhs,
                    rhs,
                    negated,
                } => {
                    let lhs = read_int(regs.as_slice(), *lhs)?;
                    let rhs = read_int(regs.as_slice(), *rhs)?;
                    write(
                        regs.as_mut_slice(),
                        *dst,
                        PureExprSymbol::Bool(PureBoolExpr::EqualInt {
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                            negated: *negated,
                        }),
                    )?;
                }
                PureClosureInstr::Return { src } => return read(regs.as_slice(), *src),
                PureClosureInstr::RuntimeError { .. }
                | PureClosureInstr::LoadUnit { .. }
                | PureClosureInstr::LoadFloat { .. }
                | PureClosureInstr::GetFieldSlot { .. }
                | PureClosureInstr::MakeStruct { .. } => return None,
            }
        }
        None
    }

    fn eval_pure_bool_expr(
        expr: &PureBoolExpr,
        state_fields: &[i64],
        item: i64,
    ) -> Option<Result<bool, EvalError>> {
        Some(match expr {
            PureBoolExpr::Const(value) => Ok(*value),
            PureBoolExpr::Compare { op, lhs, rhs } => {
                let lhs = match Self::eval_pure_int_expr(lhs, state_fields, item)? {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let rhs = match Self::eval_pure_int_expr(rhs, state_fields, item)? {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                Ok(eval_int_compare(*op, lhs, rhs))
            }
            PureBoolExpr::EqualInt { lhs, rhs, negated } => {
                let lhs = match Self::eval_pure_int_expr(lhs, state_fields, item)? {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                let rhs = match Self::eval_pure_int_expr(rhs, state_fields, item)? {
                    Ok(value) => value,
                    Err(error) => return Some(Err(error)),
                };
                Ok((lhs == rhs) ^ *negated)
            }
        })
    }

    fn captureless_pure_closure_plan(
        &mut self,
        unit: &RegUnit,
        closure: &VmClosure,
        args_len: usize,
    ) -> Option<PureClosurePlan> {
        if !closure.captures.is_empty() {
            return None;
        }
        let key = (closure.function, args_len);
        if let Some(cached) = self.pure_closure_plan_cache.get(&key) {
            return cached.clone();
        }
        let plan = Self::compile_captureless_pure_closure(unit, closure, args_len);
        self.pure_closure_plan_cache.insert(key, plan.clone());
        plan
    }

    fn compile_captureless_pure_closure(
        unit: &RegUnit,
        closure: &VmClosure,
        args_len: usize,
    ) -> Option<PureClosurePlan> {
        if !closure.captures.is_empty() {
            return None;
        }
        let function = unit.functions.get(closure.function)?;
        if function.params != args_len || function.captures != 0 || function.params > function.regs
        {
            return None;
        }

        let mut code = Vec::with_capacity(function.code.len());
        for instr in &function.code {
            let planned = match instr {
                RegInstr::LoadUnit { dst } => PureClosureInstr::LoadUnit { dst: *dst },
                RegInstr::LoadInt { dst, value } => PureClosureInstr::LoadInt {
                    dst: *dst,
                    value: *value,
                },
                RegInstr::LoadFloat { dst, value } => PureClosureInstr::LoadFloat {
                    dst: *dst,
                    value: *value,
                },
                RegInstr::LoadBool { dst, value } => PureClosureInstr::LoadBool {
                    dst: *dst,
                    value: *value,
                },
                RegInstr::Move { dst, src } => PureClosureInstr::Move {
                    dst: *dst,
                    src: *src,
                },
                RegInstr::AddInt { dst, lhs, rhs }
                | RegInstr::SubInt { dst, lhs, rhs }
                | RegInstr::MulInt { dst, lhs, rhs }
                | RegInstr::DivInt { dst, lhs, rhs }
                | RegInstr::ModInt { dst, lhs, rhs } => {
                    let (op, _, _, _) = arithmetic_binop_parts(instr)?;
                    PureClosureInstr::Numeric {
                        op,
                        dst: *dst,
                        lhs: *lhs,
                        rhs: *rhs,
                    }
                }
                RegInstr::LessInt { dst, lhs, rhs } => PureClosureInstr::IntCompare {
                    op: RegIntCompare::Less,
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                },
                RegInstr::LessEqualInt { dst, lhs, rhs } => PureClosureInstr::IntCompare {
                    op: RegIntCompare::LessEqual,
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                },
                RegInstr::GreaterInt { dst, lhs, rhs } => PureClosureInstr::IntCompare {
                    op: RegIntCompare::Greater,
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                },
                RegInstr::GreaterEqualInt { dst, lhs, rhs } => PureClosureInstr::IntCompare {
                    op: RegIntCompare::GreaterEqual,
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                },
                RegInstr::Equal { dst, lhs, rhs } => PureClosureInstr::Equal {
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                    negated: false,
                },
                RegInstr::NotEqual { dst, lhs, rhs } => PureClosureInstr::Equal {
                    dst: *dst,
                    lhs: *lhs,
                    rhs: *rhs,
                    negated: true,
                },
                RegInstr::GetFieldSlot { dst, base, slot } => PureClosureInstr::GetFieldSlot {
                    dst: *dst,
                    base: *base,
                    slot: *slot,
                },
                RegInstr::MakeStruct {
                    dst,
                    layout,
                    fields,
                } => PureClosureInstr::MakeStruct {
                    dst: *dst,
                    layout: Rc::clone(layout),
                    fields: fields.iter().map(|(_, reg)| *reg).collect(),
                },
                RegInstr::Return { src } => PureClosureInstr::Return { src: *src },
                RegInstr::RuntimeError { message } => PureClosureInstr::RuntimeError {
                    message: message.clone(),
                },
                _ => return None,
            };
            code.push(planned);
        }

        Some(PureClosurePlan {
            regs: function.regs,
            code,
        })
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn try_call_captureless_pure_closure(
        &self,
        unit: &RegUnit,
        closure: &VmClosure,
        args: &[VmValue],
    ) -> Option<Result<VmValue, EvalError>> {
        let plan = Self::compile_captureless_pure_closure(unit, closure, args.len())?;
        let mut regs = Vec::new();
        Some(Self::eval_captureless_pure_closure(&plan, args, &mut regs))
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(super) fn try_fold_int_list_with_struct_plan_for_test(
        unit: &RegUnit,
        list: &Rc<RefCell<TypedVec>>,
        state: &VmValue,
        folder: &VmClosure,
    ) -> Option<Result<VmValue, EvalError>> {
        let plan = Self::compile_captureless_pure_closure(unit, folder, 2)?;
        Self::try_fold_int_list_with_struct_plan(list, state, &plan)
    }

    fn eval_captureless_pure_int_closure(
        plan: &PureClosurePlan,
        arg: i64,
        regs: &mut Vec<Option<PureIntScalar>>,
    ) -> Option<Result<PureIntScalar, EvalError>> {
        regs.clear();
        regs.resize_with(plan.regs, || None);
        regs[0] = Some(PureIntScalar::Int(arg));

        let read_reg = |regs: &[Option<PureIntScalar>], reg: usize| -> Option<PureIntScalar> {
            regs.get(reg).and_then(|slot| *slot)
        };
        let write_reg =
            |regs: &mut [Option<PureIntScalar>], reg: usize, value: PureIntScalar| -> bool {
                let Some(slot) = regs.get_mut(reg) else {
                    return false;
                };
                *slot = Some(value);
                true
            };
        let read_int = |regs: &[Option<PureIntScalar>], reg: usize| -> Option<i64> {
            match read_reg(regs, reg)? {
                PureIntScalar::Int(value) => Some(value),
                PureIntScalar::Bool(_) => None,
            }
        };

        for instr in &plan.code {
            let step = match instr {
                PureClosureInstr::LoadInt { dst, value } => {
                    write_reg(regs.as_mut_slice(), *dst, PureIntScalar::Int(*value))
                        .then_some(Ok(()))?
                }
                PureClosureInstr::LoadBool { dst, value } => {
                    write_reg(regs.as_mut_slice(), *dst, PureIntScalar::Bool(*value))
                        .then_some(Ok(()))?
                }
                PureClosureInstr::Move { dst, src } => {
                    let value = read_reg(regs.as_slice(), *src)?;
                    write_reg(regs.as_mut_slice(), *dst, value).then_some(Ok(()))?
                }
                PureClosureInstr::Numeric { op, dst, lhs, rhs } => {
                    let lhs = read_int(regs.as_slice(), *lhs)?;
                    let rhs = read_int(regs.as_slice(), *rhs)?;
                    Self::eval_pure_int_binary(*op, lhs, rhs)
                        .map(|value| {
                            write_reg(regs.as_mut_slice(), *dst, PureIntScalar::Int(value))
                                .then_some(())
                        })
                        .and_then(|written| match written {
                            Some(()) => Ok(()),
                            None => Err(EvalError::Runtime(
                                "reg VM closure write register out of range.".to_string(),
                            )),
                        })
                }
                PureClosureInstr::IntCompare { op, dst, lhs, rhs } => {
                    let lhs = read_int(regs.as_slice(), *lhs)?;
                    let rhs = read_int(regs.as_slice(), *rhs)?;
                    write_reg(
                        regs.as_mut_slice(),
                        *dst,
                        PureIntScalar::Bool(eval_int_compare(*op, lhs, rhs)),
                    )
                    .then_some(Ok(()))?
                }
                PureClosureInstr::Equal {
                    dst,
                    lhs,
                    rhs,
                    negated,
                } => {
                    let lhs = read_reg(regs.as_slice(), *lhs)?;
                    let rhs = read_reg(regs.as_slice(), *rhs)?;
                    let equal = match (lhs, rhs) {
                        (PureIntScalar::Int(lhs), PureIntScalar::Int(rhs)) => lhs == rhs,
                        (PureIntScalar::Bool(lhs), PureIntScalar::Bool(rhs)) => lhs == rhs,
                        _ => return None,
                    };
                    write_reg(
                        regs.as_mut_slice(),
                        *dst,
                        PureIntScalar::Bool(equal ^ *negated),
                    )
                    .then_some(Ok(()))?
                }
                PureClosureInstr::Return { src } => {
                    return Some(Ok(read_reg(regs.as_slice(), *src)?));
                }
                PureClosureInstr::RuntimeError { message } => {
                    return Some(Err(EvalError::Runtime(message.clone())));
                }
                PureClosureInstr::LoadUnit { .. }
                | PureClosureInstr::LoadFloat { .. }
                | PureClosureInstr::GetFieldSlot { .. }
                | PureClosureInstr::MakeStruct { .. } => return None,
            };
            if let Err(error) = step {
                return Some(Err(error));
            }
        }
        None
    }

    fn eval_pure_int_binary(op: BinaryOp, lhs: i64, rhs: i64) -> Result<i64, EvalError> {
        match op {
            BinaryOp::Add => lhs
                .checked_add(rhs)
                .ok_or_else(|| int_overflow_error("addition", lhs, rhs)),
            BinaryOp::Subtract => lhs
                .checked_sub(rhs)
                .ok_or_else(|| int_overflow_error("subtraction", lhs, rhs)),
            BinaryOp::Multiply => lhs
                .checked_mul(rhs)
                .ok_or_else(|| int_overflow_error("multiplication", lhs, rhs)),
            BinaryOp::Divide => {
                if rhs == 0 {
                    return Err(EvalError::Runtime("integer division by zero".to_string()));
                }
                lhs.checked_div(rhs)
                    .ok_or_else(|| int_overflow_error("division", lhs, rhs))
            }
            BinaryOp::Modulo => {
                if rhs == 0 {
                    return Err(EvalError::Runtime("integer modulo by zero".to_string()));
                }
                lhs.checked_rem(rhs)
                    .ok_or_else(|| int_overflow_error("modulo", lhs, rhs))
            }
            _ => Err(EvalError::Runtime(
                "reg VM closure scalar integer evaluator called with non-integer op.".to_string(),
            )),
        }
    }

    fn eval_captureless_pure_closure(
        plan: &PureClosurePlan,
        args: &[VmValue],
        regs: &mut Vec<Option<VmValue>>,
    ) -> Result<VmValue, EvalError> {
        regs.clear();
        regs.resize_with(plan.regs, || None);
        for (index, value) in args.iter().enumerate() {
            regs[index] = Some(value.clone());
        }

        let read_reg = |regs: &[Option<VmValue>], reg: usize| -> Result<VmValue, EvalError> {
            regs.get(reg)
                .and_then(Option::as_ref)
                .cloned()
                .ok_or_else(|| {
                    EvalError::Runtime(format!("reg VM closure read uninitialized register {reg}."))
                })
        };
        let write_reg =
            |regs: &mut [Option<VmValue>], reg: usize, value: VmValue| -> Result<(), EvalError> {
                let Some(slot) = regs.get_mut(reg) else {
                    return Err(EvalError::Runtime(format!(
                        "reg VM closure write register {reg} out of range."
                    )));
                };
                *slot = Some(value);
                Ok(())
            };

        for instr in &plan.code {
            let step = match instr {
                PureClosureInstr::LoadUnit { dst } => {
                    write_reg(regs.as_mut_slice(), *dst, VmValue::Unit)
                }
                PureClosureInstr::LoadInt { dst, value } => {
                    write_reg(regs.as_mut_slice(), *dst, VmValue::Int(*value))
                }
                PureClosureInstr::LoadFloat { dst, value } => {
                    write_reg(regs.as_mut_slice(), *dst, VmValue::Float(*value))
                }
                PureClosureInstr::LoadBool { dst, value } => {
                    write_reg(regs.as_mut_slice(), *dst, VmValue::Bool(*value))
                }
                PureClosureInstr::Move { dst, src } => {
                    let value = read_reg(regs.as_slice(), *src);
                    value.and_then(|value| write_reg(regs.as_mut_slice(), *dst, value))
                }
                PureClosureInstr::Numeric { op, dst, lhs, rhs } => {
                    let lhs = read_reg(regs.as_slice(), *lhs);
                    let rhs = read_reg(regs.as_slice(), *rhs);
                    lhs.and_then(|lhs| rhs.and_then(|rhs| eval_numeric_binary(*op, &lhs, &rhs)))
                        .and_then(|value| write_reg(regs.as_mut_slice(), *dst, value))
                }
                PureClosureInstr::IntCompare { op, dst, lhs, rhs } => {
                    let lhs =
                        read_reg(regs.as_slice(), *lhs).and_then(|value| expect_int_ref(&value));
                    let rhs =
                        read_reg(regs.as_slice(), *rhs).and_then(|value| expect_int_ref(&value));
                    lhs.and_then(|lhs| rhs.map(|rhs| eval_int_compare(*op, lhs, rhs)))
                        .and_then(|value| {
                            write_reg(regs.as_mut_slice(), *dst, VmValue::Bool(value))
                        })
                }
                PureClosureInstr::Equal {
                    dst,
                    lhs,
                    rhs,
                    negated,
                } => {
                    let lhs = read_reg(regs.as_slice(), *lhs);
                    let rhs = read_reg(regs.as_slice(), *rhs);
                    lhs.and_then(|lhs| rhs.map(|rhs| (lhs == rhs) ^ *negated))
                        .and_then(|value| {
                            write_reg(regs.as_mut_slice(), *dst, VmValue::Bool(value))
                        })
                }
                PureClosureInstr::GetFieldSlot { dst, base, slot } => {
                    let value = read_reg(regs.as_slice(), *base)
                        .and_then(|value| read_field_slot(&value, *slot));
                    value.and_then(|value| write_reg(regs.as_mut_slice(), *dst, value))
                }
                PureClosureInstr::MakeStruct {
                    dst,
                    layout,
                    fields,
                } => {
                    let mut values = Vec::with_capacity(fields.len());
                    for reg in fields {
                        match read_reg(regs.as_slice(), *reg) {
                            Ok(value) => values.push(value),
                            Err(error) => return Err(error),
                        }
                    }
                    write_reg(
                        regs.as_mut_slice(),
                        *dst,
                        VmValue::Struct(Rc::new(VmStruct::with_layout(Rc::clone(layout), values))),
                    )
                }
                PureClosureInstr::Return { src } => return read_reg(regs.as_slice(), *src),
                PureClosureInstr::RuntimeError { message } => {
                    return Err(EvalError::Runtime(message.clone()));
                }
            };
            if let Err(error) = step {
                return Err(error);
            }
        }
        Ok(VmValue::Unit)
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
        self.charge_intrinsic_call()?;
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
