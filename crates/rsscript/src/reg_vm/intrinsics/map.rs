use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    #[allow(clippy::mutable_key_type)]
    pub(super) fn exec_map_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::MapContainsKey => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(map.borrow().contains_key(&key)))
            }
            RegIntrinsic::MapFilter => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                let mut filtered = ValueMap::default();
                for (key, value) in entries {
                    let keep = self.call_closure_two(
                        unit,
                        &predicate,
                        vm_value_from_map_key(&key),
                        value.clone(),
                        next_base,
                    )?;
                    if expect_bool_ref(&keep)? {
                        filtered.insert(key, value);
                    }
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(filtered))))
            }
            RegIntrinsic::MapFold => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, value) in entries {
                    state = self.call_closure_three(
                        unit,
                        &folder,
                        state,
                        vm_value_from_map_key(&key),
                        value,
                        next_base,
                    )?;
                }
                Ok(state)
            }
            RegIntrinsic::MapForEach => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let callback = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, value) in entries {
                    let _ = self.call_closure_two(
                        unit,
                        &callback,
                        vm_value_from_map_key(&key),
                        value,
                        next_base,
                    )?;
                }
                Ok(VmValue::Unit)
            }
            RegIntrinsic::MapGetOrDefault => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let default = intrinsic_arg(&self.stack, base, args, 2)?.clone();
                Ok(map.borrow().get(&key).cloned().unwrap_or(default))
            }
            RegIntrinsic::MapIsEmpty => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(map.borrow().is_empty()))
            }
            RegIntrinsic::MapKeys => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let keys = map
                    .borrow()
                    .keys()
                    .map(vm_value_from_map_key)
                    .collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(keys)))))
            }
            RegIntrinsic::MapLen => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(map.borrow().len() as i64))
            }
            RegIntrinsic::MapMapValues => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                let mut mapped = ValueMap::default();
                for (key, value) in entries {
                    mapped.insert(key, self.call_closure_one(unit, &mapper, value, next_base)?);
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(mapped))))
            }
            RegIntrinsic::MapMerge => {
                let left = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_map_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let resolver = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut merged = left.borrow().clone();
                let right_entries = right
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, right_value) in right_entries {
                    if let Some(left_value) = merged.get(&key).cloned() {
                        let resolved = self.call_closure_two(
                            unit,
                            &resolver,
                            left_value,
                            right_value,
                            next_base,
                        )?;
                        merged.insert(key, resolved);
                    } else {
                        merged.insert(key, right_value);
                    }
                }
                Ok(VmValue::Map(Rc::new(RefCell::new(merged))))
            }
            RegIntrinsic::MapNew => Ok(VmValue::Map(Rc::new(RefCell::new(ValueMap::default())))),
            RegIntrinsic::MapTryFold => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let entries = map
                    .borrow()
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<_>>();
                for (key, value) in entries {
                    let folded = self.call_closure_three(
                        unit,
                        &folder,
                        state,
                        vm_value_from_map_key(&key),
                        value,
                        next_base,
                    )?;
                    match result_variant_payload(&folded)? {
                        Ok(value) => state = value,
                        Err(error) => return Ok(value_err(error)),
                    }
                }
                Ok(value_ok(state))
            }
            RegIntrinsic::MapValues => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = map.borrow().values().cloned().collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(values)))))
            }
            other => unreachable!("exec_map_intrinsics called with non-map intrinsic: {other:?}"),
        }
    }
}
