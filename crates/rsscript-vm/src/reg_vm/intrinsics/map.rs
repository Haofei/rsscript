use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
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
                let (key, work) = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                self.charge_work(work)?;
                Ok(VmValue::Bool(map.borrow().contains_key(&key)))
            }
            RegIntrinsic::MapFilter => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let keys = map.borrow().keys().cloned().collect::<Vec<_>>();
                let mut filtered = ValueMap::default();
                for key in keys {
                    let value = match map.borrow().get(&key) {
                        Some(value) => value.clone(),
                        None => continue,
                    };
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
                self.fresh_map(filtered)
            }
            RegIntrinsic::MapFold => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let keys = map.borrow().keys().cloned().collect::<Vec<_>>();
                for key in keys {
                    let value = match map.borrow().get(&key) {
                        Some(value) => value.clone(),
                        None => continue,
                    };
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
                let keys = map.borrow().keys().cloned().collect::<Vec<_>>();
                for key in keys {
                    let value = match map.borrow().get(&key) {
                        Some(value) => value.clone(),
                        None => continue,
                    };
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
                let (key, work) = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                self.charge_work(work)?;
                let default = intrinsic_arg(&self.stack, base, args, 2)?.clone();
                Ok(map.borrow().get(&key).cloned().unwrap_or(default))
            }
            RegIntrinsic::MapIsEmpty => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(map.borrow().is_empty()))
            }
            RegIntrinsic::MapKeys => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let keys = core_map_keys(&map.borrow())
                    .into_iter()
                    .map(|key| vm_value_from_map_key(&key))
                    .collect::<Vec<_>>();
                self.fresh_list(TypedVec::from_values(keys))
            }
            RegIntrinsic::MapLen => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(map.borrow().len() as i64))
            }
            RegIntrinsic::MapMapValues => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let keys = map.borrow().keys().cloned().collect::<Vec<_>>();
                let mut mapped = ValueMap::default();
                for key in keys {
                    let value = match map.borrow().get(&key) {
                        Some(value) => value.clone(),
                        None => continue,
                    };
                    mapped.insert(key, self.call_closure_one(unit, &mapper, value, next_base)?);
                }
                self.fresh_map(mapped)
            }
            RegIntrinsic::MapMerge => {
                let left = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_map_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let resolver = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let mut merged = left.borrow().clone();
                let right_keys = right.borrow().keys().cloned().collect::<Vec<_>>();
                for key in right_keys {
                    let right_value = match right.borrow().get(&key) {
                        Some(value) => value.clone(),
                        None => continue,
                    };
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
                self.fresh_map(merged)
            }
            RegIntrinsic::MapNew => self.fresh_map(ValueMap::default()),
            RegIntrinsic::MapTryFold => {
                let map = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mut state = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                let folder = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let keys = map.borrow().keys().cloned().collect::<Vec<_>>();
                for key in keys {
                    let value = match map.borrow().get(&key) {
                        Some(value) => value.clone(),
                        None => continue,
                    };
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
                let values = core_map_values(&map.borrow());
                self.fresh_list(TypedVec::from_values(values))
            }
            other => unreachable!("exec_map_intrinsics called with non-map intrinsic: {other:?}"),
        }
    }
}
