use super::super::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    pub(super) fn exec_deque_intrinsics(
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
            RegIntrinsic::DequeIsEmpty => {
                let deque = expect_deque_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(deque.borrow().is_empty()))
            }
            RegIntrinsic::DequeLen => {
                let deque = expect_deque_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(deque.borrow().len() as i64))
            }
            RegIntrinsic::DequeNew => Ok(VmValue::Deque(Rc::new(RefCell::new(
                std::collections::VecDeque::new(),
            )))),
            RegIntrinsic::DequeToList => {
                let deque = expect_deque_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let list = deque.borrow().iter().cloned().collect::<Vec<_>>();
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(list)))))
            }
            other => {
                unreachable!("exec_deque_intrinsics called with non-deque intrinsic: {other:?}")
            }
        }
    }
}
