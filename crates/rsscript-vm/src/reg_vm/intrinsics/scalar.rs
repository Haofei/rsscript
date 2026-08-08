use super::super::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    pub(super) fn exec_scalar_intrinsics(
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
            RegIntrinsic::IntBitAnd => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left & right))
            }
            RegIntrinsic::IntBitNot => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(!value))
            }
            RegIntrinsic::IntBitOr => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left | right))
            }
            RegIntrinsic::IntBitXor => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left ^ right))
            }
            RegIntrinsic::IntShiftLeft => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bits = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(value << checked_shift_count(bits)?))
            }
            RegIntrinsic::IntShiftRight => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let bits = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(value >> checked_shift_count(bits)?))
            }
            RegIntrinsic::IntToString => self.fresh_string(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            ),
            RegIntrinsic::IntToFloat => {
                Ok(VmValue::Float(
                    expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)? as f64,
                ))
            }
            RegIntrinsic::FloatToString => self.fresh_string(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            ),
            RegIntrinsic::FloatIsFinite => Ok(VmValue::Bool(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.is_finite(),
            )),
            RegIntrinsic::FloatIsInfinite => Ok(VmValue::Bool(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.is_infinite(),
            )),
            RegIntrinsic::FloatIsNan => Ok(VmValue::Bool(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.is_nan(),
            )),
            other => {
                unreachable!("exec_scalar_intrinsics called with non-scalar intrinsic: {other:?}")
            }
        }
    }
}
