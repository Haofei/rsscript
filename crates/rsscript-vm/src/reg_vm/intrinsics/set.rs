use super::super::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    pub(super) fn exec_set_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = unit;
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::SetContains => {
                let set = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let (key, work) = map_key_from_value(intrinsic_arg(&self.stack, base, args, 1)?)?;
                self.charge_work(work)?;
                Ok(VmValue::Bool(set.borrow().contains_key(&key)))
            }
            RegIntrinsic::SetDifference => {
                let left = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_map_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                self.fresh_map(core_map_difference(&left.borrow(), &right.borrow()).into())
            }
            RegIntrinsic::SetIntersection => {
                let left = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_map_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                self.fresh_map(core_map_intersection(&left.borrow(), &right.borrow()).into())
            }
            RegIntrinsic::SetIsEmpty => {
                let set = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(set.borrow().is_empty()))
            }
            RegIntrinsic::SetIsSubset => {
                let left = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_map_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(core_map_is_subset(
                    &left.borrow(),
                    &right.borrow(),
                )))
            }
            RegIntrinsic::SetLen => {
                let set = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(set.borrow().len() as i64))
            }
            RegIntrinsic::SetNew => self.fresh_map(ValueMap::default()),
            RegIntrinsic::SetToList => {
                let set = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = core_map_keys(&set.borrow())
                    .into_iter()
                    .map(|key| vm_value_from_map_key(&key))
                    .collect::<Vec<_>>();
                self.fresh_list(TypedVec::from_values(values))
            }
            RegIntrinsic::SetUnion => {
                let left = expect_map_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_map_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                self.fresh_map(core_map_union(&left.borrow(), &right.borrow()).into())
            }
            RegIntrinsic::SortedSetContains => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(sorted_contains_vm(&set.borrow(), value)?))
            }
            RegIntrinsic::SortedSetIsEmpty => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(set.borrow().is_empty()))
            }
            RegIntrinsic::SortedSetLen => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(set.borrow().len() as i64))
            }
            RegIntrinsic::SortedSetNew => self.fresh_list(TypedVec::new()),
            RegIntrinsic::SortedSetToList => {
                let set = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = set.borrow().clone();
                self.fresh_list(values)
            }
            RegIntrinsic::SortedMapContainsKey => {
                let map = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(VmValue::Bool(
                    sorted_map_get_in_place(&map.borrow(), key)?.is_some(),
                ))
            }
            RegIntrinsic::SortedMapGet => {
                let map = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let key = intrinsic_arg(&self.stack, base, args, 1)?;
                Ok(sorted_map_get_in_place(&map.borrow(), key)?
                    .map(VmValue::some)
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::SortedMapIsEmpty => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(entries.is_empty()))
            }
            RegIntrinsic::SortedMapKeys => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let keys = entries.into_iter().map(|(key, _)| key).collect::<Vec<_>>();
                self.fresh_list(TypedVec::from_values(keys))
            }
            RegIntrinsic::SortedMapLen => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(entries.len() as i64))
            }
            RegIntrinsic::SortedMapNew => Ok(sorted_map_value(Vec::new())),
            RegIntrinsic::SortedMapValues => {
                let entries =
                    expect_sorted_map_entries(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let values = entries
                    .into_iter()
                    .map(|(_, value)| value)
                    .collect::<Vec<_>>();
                self.fresh_list(TypedVec::from_values(values))
            }
            other => unreachable!("exec_set_intrinsics called with non-set intrinsic: {other:?}"),
        }
    }
}
