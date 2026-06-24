use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    #[allow(clippy::mutable_key_type)]
    pub(super) fn exec_bytes_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        let _ = unit;
        match intrinsic {
            RegIntrinsic::BytesConcat => {
                let left = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut bytes = Vec::with_capacity(left.len() + right.len());
                bytes.extend_from_slice(left);
                bytes.extend_from_slice(right);
                Ok(VmValue::Bytes(Rc::new(bytes)))
            }
            RegIntrinsic::BytesConsume => {
                expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Unit)
            }
            RegIntrinsic::BytesFromString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(value.as_bytes().to_vec())))
            }
            RegIntrinsic::BytesFromUints => {
                let values = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bytes = values
                    .borrow()
                    .iter()
                    .map(|value| expect_int_ref(&value).map(|v| v as u8))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(VmValue::Bytes(Rc::new(bytes)))
            }
            RegIntrinsic::BytesIsEmpty => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_empty()))
            }
            RegIntrinsic::BytesLen => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value.len() as i64))
            }
            RegIntrinsic::BytesSlice => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::Bytes(Rc::new(bytes_slice(value, start, len))))
            }
            RegIntrinsic::BytesToString => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(String::from_utf8_lossy(value)))
            }
            RegIntrinsic::BytesToUints => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::List(Rc::new(RefCell::new(
                    value
                        .iter()
                        .map(|byte| VmValue::Int(i64::from(*byte)))
                        .collect(),
                ))))
            }
            RegIntrinsic::BytesViewStartsWith => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.starts_with(prefix)))
            }
            RegIntrinsic::BytesViewToBytes => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bytes(Rc::new(value.to_vec())))
            }
            other => {
                unreachable!("exec_bytes_intrinsics called with non-bytes intrinsic: {other:?}")
            }
        }
    }
}
