use super::super::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    pub(super) fn exec_char_intrinsics(
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
            RegIntrinsic::CharCompare => {
                let left = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let right = expect_char_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let value = match left.cmp(&right) {
                    std::cmp::Ordering::Less => -1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 1,
                };
                Ok(VmValue::Int(value))
            }
            RegIntrinsic::CharFromCode => {
                let value = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(u32::try_from(value)
                    .ok()
                    .and_then(char::from_u32)
                    .map(VmValue::Char)
                    .map(VmValue::some)
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::CharIsAlphanumeric => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_ascii_alphanumeric()))
            }
            RegIntrinsic::CharIsAlpha => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_ascii_alphabetic()))
            }
            RegIntrinsic::CharIsDigit => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_ascii_digit()))
            }
            RegIntrinsic::CharIsLower => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_lowercase()))
            }
            RegIntrinsic::CharIsUpper => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_uppercase()))
            }
            RegIntrinsic::CharIsWhitespace => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_whitespace()))
            }
            RegIntrinsic::CharToCode => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value as u32 as i64))
            }
            RegIntrinsic::CharToLower => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Char(value.to_lowercase().next().unwrap_or(value)))
            }
            RegIntrinsic::CharToString => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(value.to_string())
            }
            RegIntrinsic::CharToUpper => {
                let value = expect_char_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Char(value.to_uppercase().next().unwrap_or(value)))
            }
            other => unreachable!("exec_char_intrinsics called with non-char intrinsic: {other:?}"),
        }
    }
}
