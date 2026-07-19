use super::super::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    #[allow(clippy::mutable_key_type)]
    pub(super) fn exec_math_intrinsics(
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
            RegIntrinsic::MathAbs => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                value.checked_abs().map(VmValue::Int).ok_or_else(|| {
                    EvalError::Runtime(format!(
                        "Math.abs overflow: abs({value}) exceeds the Int range"
                    ))
                })
            }
            RegIntrinsic::MathAbsFloat => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.abs(),
            )),
            RegIntrinsic::MathCeil => Ok(VmValue::Int(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.ceil() as i64,
            )),
            RegIntrinsic::MathClamp => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let min = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let max = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                if min > max {
                    return Err(EvalError::Runtime(format!(
                        "Math.clamp requires min <= max, got min {min} and max {max}"
                    )));
                }
                Ok(VmValue::Int(value.clamp(min, max)))
            }
            RegIntrinsic::MathClampFloat => {
                let value = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let min = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let max = expect_float_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                if min.is_nan() || max.is_nan() || min > max {
                    return Err(EvalError::Runtime(format!(
                        "Math.clamp_float requires non-NaN bounds with min <= max, got min {min} and max {max}"
                    )));
                }
                Ok(VmValue::Float(value.clamp(min, max)))
            }
            RegIntrinsic::MathCos => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.cos(),
            )),
            RegIntrinsic::MathExp => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.exp(),
            )),
            RegIntrinsic::MathExp2 => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.exp2(),
            )),
            RegIntrinsic::MathFloor => Ok(VmValue::Int(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.floor() as i64,
            )),
            RegIntrinsic::MathLog => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.ln(),
            )),
            RegIntrinsic::MathLog2 => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.log2(),
            )),
            RegIntrinsic::MathMax => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.max(right)))
            }
            RegIntrinsic::MathMaxFloat => {
                let left = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Float(left.max(right)))
            }
            RegIntrinsic::MathMin => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.min(right)))
            }
            RegIntrinsic::MathWrappingAdd => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.wrapping_add(right)))
            }
            RegIntrinsic::MathWrappingSub => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.wrapping_sub(right)))
            }
            RegIntrinsic::MathWrappingMul => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.wrapping_mul(right)))
            }
            RegIntrinsic::MathSaturatingAdd => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.saturating_add(right)))
            }
            RegIntrinsic::MathSaturatingSub => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.saturating_sub(right)))
            }
            RegIntrinsic::MathSaturatingMul => {
                let left = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(left.saturating_mul(right)))
            }
            RegIntrinsic::MathMinFloat => {
                let left = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Float(left.min(right)))
            }
            RegIntrinsic::MathPow => {
                let base_value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let exponent = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let exponent = u32::try_from(exponent).map_err(|_| {
                    EvalError::Runtime(format!(
                        "Math.pow exponent must be between 0 and {}, got {exponent}",
                        u32::MAX
                    ))
                })?;
                base_value
                    .checked_pow(exponent)
                    .map(VmValue::Int)
                    .ok_or_else(|| {
                        EvalError::Runtime(format!(
                            "Math.pow overflow: {base_value} raised to {exponent} exceeds the Int range"
                        ))
                    })
            }
            RegIntrinsic::MathPowFloat => {
                let base_value = expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let exponent = expect_float_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Float(base_value.powf(exponent)))
            }
            RegIntrinsic::MathRound => Ok(VmValue::Int(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.round() as i64,
            )),
            RegIntrinsic::MathSin => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.sin(),
            )),
            RegIntrinsic::MathSqrt => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.sqrt(),
            )),
            RegIntrinsic::MathTanh => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.tanh(),
            )),
            RegIntrinsic::MathTruncFloat => Ok(VmValue::Float(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.trunc(),
            )),
            other => unreachable!("exec_math_intrinsics called with non-math intrinsic: {other:?}"),
        }
    }
}
