use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_ops::*;

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
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    flattened,
                )))))
            }
            RegIntrinsic::ListFlatten => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut flattened = Vec::new();
                for value in list.borrow().iter() {
                    let nested = expect_list_ref(&value)?;
                    flattened.extend(nested.borrow().iter());
                }
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    flattened,
                )))))
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
                Ok(VmValue::Map(Rc::new(RefCell::new(groups))))
            }
            RegIntrinsic::ListIsEmpty => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(list.borrow().is_empty()))
            }
            RegIntrinsic::ListJoin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let separator =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                Ok(VmValue::string(join_string_values(
                    &list.borrow(),
                    &separator,
                )?))
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
                let mut values = Vec::new();
                for value in list.borrow().iter() {
                    if !values.contains(&value) {
                        values.push(value);
                    }
                }
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    values,
                )))))
            }
            RegIntrinsic::ListEnumerate => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut values = Vec::new();
                for (index, value) in list.borrow().iter().enumerate() {
                    values.push(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                        vec![
                            VmValue::Int(index as i64),
                            VmValue::Int(expect_int_ref(&value)?),
                        ],
                    )))));
                }
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    values,
                )))))
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
            RegIntrinsic::ListNew => Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::new())))),
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
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    vec![
                        VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(matched)))),
                        VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(unmatched)))),
                    ],
                )))))
            }
            RegIntrinsic::ListReverse => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut values = list.borrow().clone();
                values.reverse();
                Ok(VmValue::List(Rc::new(RefCell::new(values))))
            }
            RegIntrinsic::ListSkip => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let values = list.borrow().iter().skip(count).collect();
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    values,
                )))))
            }
            RegIntrinsic::ListSlice => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = nonnegative_count(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let borrowed = list.borrow();
                if start >= borrowed.len() {
                    return Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::new()))));
                }
                let end = start.saturating_add(len).min(borrowed.len());
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    borrowed.slice_to_vec(start, end),
                )))))
            }
            RegIntrinsic::ListSum => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let total = list
                    .borrow()
                    .iter()
                    .map(|v| expect_int_ref(&v))
                    .try_fold(0_i64, |total, value| value.map(|value| total + value))?;
                Ok(VmValue::Int(total))
            }
            RegIntrinsic::ListZip => {
                let left = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let left = left.borrow();
                let right = right.borrow();
                let values = left
                    .iter()
                    .zip(right.iter())
                    .map(|(left, right)| {
                        VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(vec![
                            left.clone(),
                            right.clone(),
                        ]))))
                    })
                    .collect();
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    values,
                )))))
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
                let values = list.borrow().iter().take(count).collect();
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                    values,
                )))))
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
