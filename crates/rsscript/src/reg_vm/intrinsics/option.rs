use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    pub(super) fn exec_option_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        match intrinsic {
            RegIntrinsic::OptionAndThen => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => {
                        ensure_option_value(self.call_closure_one(
                            unit,
                            &mapper,
                            option.unwrap_some().expect("Some arm yields a payload"),
                            next_base,
                        )?)
                    }
                    VmValue::OptionNone => Ok(VmValue::OptionNone),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.and_then expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionFilter => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let predicate = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => {
                        let value = option.unwrap_some().expect("Some arm yields a payload");
                        let keep =
                            self.call_closure_one(unit, &predicate, value.clone(), next_base)?;
                        if expect_bool_ref(&keep)? {
                            Ok(VmValue::some(value))
                        } else {
                            Ok(VmValue::OptionNone)
                        }
                    }
                    VmValue::OptionNone => Ok(VmValue::OptionNone),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.filter expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionIsNone => Ok(VmValue::Bool(matches!(
                intrinsic_arg(&self.stack, base, args, 0)?,
                VmValue::OptionNone
            ))),
            RegIntrinsic::OptionIsSome => Ok(VmValue::Bool(matches!(
                intrinsic_arg(&self.stack, base, args, 0)?,
                VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_)
            ))),
            RegIntrinsic::OptionMap => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let mapper = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => {
                        Ok(VmValue::some(self.call_closure_one(
                            unit,
                            &mapper,
                            option.unwrap_some().expect("Some arm yields a payload"),
                            next_base,
                        )?))
                    }
                    VmValue::OptionNone => Ok(VmValue::OptionNone),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.map expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionOkOr => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let error = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                match option {
                    VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => Ok(value_ok(
                        option.unwrap_some().expect("Some arm yields a payload"),
                    )),
                    VmValue::OptionNone => Ok(value_err(error)),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.ok_or expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionOr => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let fallback = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                match option {
                    VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => Ok(option.clone()),
                    VmValue::OptionNone => Ok(fallback),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.or expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionUnwrapOr => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let default = intrinsic_arg(&self.stack, base, args, 1)?.clone();
                match option {
                    VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => {
                        Ok(option.unwrap_some().expect("Some arm yields a payload"))
                    }
                    VmValue::OptionNone => Ok(default),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.unwrap_or expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            RegIntrinsic::OptionUnwrapOrElse => {
                let option = intrinsic_arg(&self.stack, base, args, 0)?;
                let fallback = expect_closure_rc(intrinsic_arg(&self.stack, base, args, 1)?)?;
                match option {
                    VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => {
                        Ok(option.unwrap_some().expect("Some arm yields a payload"))
                    }
                    VmValue::OptionNone => self.call_closure_zero(unit, &fallback, next_base),
                    other => Err(EvalError::Runtime(format!(
                        "reg VM Option.unwrap_or_else expected Option, got `{}`.",
                        other.display()
                    ))),
                }
            }
            other => {
                unreachable!("exec_option_intrinsics called with non-option intrinsic: {other:?}")
            }
        }
    }
}
