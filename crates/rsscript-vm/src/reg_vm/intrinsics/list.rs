use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_ops::*;
use crate::serde_json;

impl RegVm {
    #[allow(clippy::mutable_key_type)]
    pub(super) fn exec_list_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::ListAll => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                for index in 0..len {
                    let value = list_item_at(&list, index, "List.all")?;
                    let keep = self.call_closure_one(unit, &predicate, value, next_base)?;
                    if !expect_bool_ref(&keep)? {
                        return Ok(VmValue::Bool(false));
                    }
                }
                Ok(VmValue::Bool(true))
            }
            RegIntrinsic::ListAny | RegIntrinsic::ListContains => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                for index in 0..len {
                    let value = list_item_at(&list, index, "List.any")?;
                    let matched = self.call_closure_one(unit, &predicate, value, next_base)?;
                    if expect_bool_ref(&matched)? {
                        return Ok(VmValue::Bool(true));
                    }
                }
                Ok(VmValue::Bool(false))
            }
            RegIntrinsic::ListContainsValue => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(
                    list.borrow().iter().any(|item| &item == value),
                ))
            }
            RegIntrinsic::ListCountWhere => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                let mut count = 0;
                for index in 0..len {
                    let value = list_item_at(&list, index, "List.count_where")?;
                    let matched = self.call_closure_one(unit, &predicate, value, next_base)?;
                    if expect_bool_ref(&matched)? {
                        count += 1;
                    }
                }
                Ok(VmValue::Int(count))
            }
            RegIntrinsic::ListConsume => {
                let _ = intrinsic_arg(&self.stack, base, args, 0)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::ListFind => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                for index in 0..len {
                    let value = list_item_at(&list, index, "List.find")?;
                    let matched =
                        self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                    if expect_bool_ref(&matched)? {
                        return Ok(VmValue::some(value));
                    }
                }
                Ok(VmValue::OptionNone)
            }
            RegIntrinsic::ListFirst => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(list
                    .borrow()
                    .first()
                    .map(|value| VmValue::some(value))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListFlatMap => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                let mut flattened = Vec::new();
                for index in 0..len {
                    let value = list_item_at(&list, index, "List.flat_map")?;
                    let mapped = self.call_closure_one(unit, &mapper, value, next_base)?;
                    let mapped = expect_list_ref(&mapped)?;
                    flattened.extend(mapped.borrow().iter());
                }
                self.fresh_list(TypedVec::from_values(flattened))
            }
            RegIntrinsic::ListFlatten => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut flattened = Vec::new();
                for value in list.borrow().iter() {
                    let nested = expect_list_ref(&value)?;
                    flattened.extend(nested.borrow().iter());
                }
                self.fresh_list(TypedVec::from_values(flattened))
            }
            RegIntrinsic::ListGroupBy => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key_fn = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                let mut groups: ValueMap = ValueMap::default();
                for index in 0..len {
                    let value = list_item_at(&list, index, "List.group_by")?;
                    let key_value =
                        self.call_closure_one(unit, &key_fn, value.clone(), next_base)?;
                    let (key, work) = map_key_from_value(&key_value)?;
                    self.charge_work(work)?;
                    match groups.get(&key) {
                        Some(VmValue::List(items)) => {
                            items.borrow_mut().push(value);
                        }
                        Some(other) => {
                            return Err(EvalError::Runtime(format!(
                                "reg VM List.group_by expected List group, got `{}`.",
                                other.display()
                            )));
                        }
                        None => {
                            groups.insert(
                                key,
                                VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(vec![
                                    value,
                                ])))),
                            );
                        }
                    }
                }
                for value in groups.values() {
                    if let VmValue::List(items) = value {
                        self.account_list_storage(&items.borrow())?;
                    }
                }
                self.fresh_map(groups)
            }
            RegIntrinsic::ListIsEmpty => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(list.borrow().is_empty()))
            }
            RegIntrinsic::ListJoin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let separator =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                let joined = join_string_values(&list.borrow(), &separator)?;
                self.fresh_string(joined)
            }
            RegIntrinsic::ListLast => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(list
                    .borrow()
                    .last()
                    .map(|value| VmValue::some(value))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListDedup => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = core_list_dedup(list.borrow().iter());
                self.fresh_list(TypedVec::from_values(values))
            }
            RegIntrinsic::ListEnumerate => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let pairs = core_list_enumerate(list.borrow().iter());
                let mut values = Vec::with_capacity(pairs.len());
                for (index, value) in pairs {
                    values.push(self.fresh_list(TypedVec::from_values(vec![
                        VmValue::Int(index as i64),
                        VmValue::Int(expect_int_ref(&value)?),
                    ]))?);
                }
                self.fresh_list(TypedVec::from_values(values))
            }
            RegIntrinsic::ListMax => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let max = list
                    .borrow()
                    .iter()
                    .map(|v| expect_int_ref(&v))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .max();
                Ok(max
                    .map(|value| VmValue::some(VmValue::Int(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListMin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let min = list
                    .borrow()
                    .iter()
                    .map(|v| expect_int_ref(&v))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .min();
                Ok(min
                    .map(|value| VmValue::some(VmValue::Int(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::ListNew => self.fresh_list(TypedVec::new()),
            RegIntrinsic::ListPartition => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = list.borrow().len();
                let mut matched = Vec::new();
                let mut unmatched = Vec::new();
                for index in 0..len {
                    let value = list_item_at(&list, index, "List.partition")?;
                    let keep = self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                    if expect_bool_ref(&keep)? {
                        matched.push(value);
                    } else {
                        unmatched.push(value);
                    }
                }
                let matched = self.fresh_list(TypedVec::from_values(matched))?;
                let unmatched = self.fresh_list(TypedVec::from_values(unmatched))?;
                self.fresh_list(TypedVec::from_values(vec![matched, unmatched]))
            }
            RegIntrinsic::ListReverse => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = core_list_reverse(list.borrow().iter());
                self.fresh_list(TypedVec::from_values(values))
            }
            RegIntrinsic::ListSkip => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = core_list_skip(list.borrow().iter(), count);
                self.fresh_list(TypedVec::from_values(values))
            }
            RegIntrinsic::ListSlice => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = nonnegative_count(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let values = core_list_slice(list.borrow().iter(), start, len);
                self.fresh_list(TypedVec::from_values(values))
            }
            RegIntrinsic::ListSum => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let total = list.borrow().iter().map(|v| expect_int_ref(&v)).try_fold(
                    0_i64,
                    |total, value| {
                        value.and_then(|value| {
                            total.checked_add(value).ok_or_else(|| {
                                EvalError::Runtime(
                                    "List.sum overflow exceeds the Int range".to_string(),
                                )
                            })
                        })
                    },
                )?;
                Ok(VmValue::Int(total))
            }
            RegIntrinsic::ListZip => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let left = left.borrow();
                let right = right.borrow();
                let values = core_list_zip(left.iter(), right.iter())
                    .into_iter()
                    .map(|(left, right)| TypedVec::from_values(vec![left, right]))
                    .collect::<Vec<_>>();
                drop(left);
                drop(right);
                let mut pairs = Vec::with_capacity(values.len());
                for pair in values {
                    pairs.push(self.fresh_list(pair)?);
                }
                self.fresh_list(TypedVec::from_values(pairs))
            }
            RegIntrinsic::ListTryFold => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let len = list.borrow().len();
                for index in 0..len {
                    let value = list_item_at(&list, index, "List.try_fold")?;
                    let folded = self.call_closure_two(unit, &folder, state, value, next_base)?;
                    match result_variant_payload(&folded)? {
                        Ok(value) => state = value,
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(state))
            }
            RegIntrinsic::ListTake => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = core_list_take(list.borrow().iter(), count);
                self.fresh_list(TypedVec::from_values(values))
            }
            RegIntrinsic::ListToJsonStrings => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = list
                    .borrow()
                    .iter()
                    .map(|value| expect_string_ref(&value).map(|value| value.to_string()))
                    .map(|value| value.map(serde_json::Value::String))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(values))))
            }
            RegIntrinsic::ListToJsonValues => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = list
                    .borrow()
                    .iter()
                    .map(|value| expect_json_ref(&value).cloned())
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Json(Rc::new(serde_json::Value::Array(values))))
            }
            other => unreachable!("exec_list_intrinsics called with non-list intrinsic: {other:?}"),
        }
    }
}
