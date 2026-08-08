use super::super::*;
use crate::reg_vm::value_access::*;

impl RegVm {
    pub(super) fn exec_result_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        match intrinsic {
            RegIntrinsic::ResultErr => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match result {
                    Ok(_) => VmValue::OptionNone,
                    Err(error) => VmValue::some(error),
                })
            }
            RegIntrinsic::ResultErrMessage => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match result {
                    Ok(_) => VmValue::OptionNone,
                    Err(error) => VmValue::some(VmValue::string(error.display())),
                })
            }
            RegIntrinsic::ResultIsErr => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(result.is_err()))
            }
            RegIntrinsic::ResultIsOk => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(result.is_ok()))
            }
            RegIntrinsic::ResultOk => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match result {
                    Ok(value) => VmValue::some(value),
                    Err(_) => VmValue::OptionNone,
                })
            }
            RegIntrinsic::ResultAndThen => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => {
                        let mapped = self.call_closure_one(unit, &mapper, value, next_base)?;
                        let _ = result_variant_payload(&mapped)?;
                        Ok(mapped)
                    }
                    Err(error) => Ok(value_err(error)),
                }
            }
            RegIntrinsic::ResultMap => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => Ok(value_ok(
                        self.call_closure_one(unit, &mapper, value, next_base)?,
                    )),
                    Err(error) => Ok(value_err(error)),
                }
            }
            RegIntrinsic::ResultMapError => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => Ok(value_ok(value)),
                    Err(error) => Ok(value_err(
                        self.call_closure_one(unit, &mapper, error, next_base)?,
                    )),
                }
            }
            RegIntrinsic::ResultUnwrapOr => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let default = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                Ok(match result {
                    Ok(value) => value,
                    Err(_) => default,
                })
            }
            RegIntrinsic::ResultUnwrapOrElse => {
                let result = result_variant_payload(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let fallback = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match result {
                    Ok(value) => Ok(value),
                    Err(error) => self.call_closure_one(unit, &fallback, error, next_base),
                }
            }
            other => {
                unreachable!("exec_result_intrinsics called with non-result intrinsic: {other:?}")
            }
        }
    }
}
