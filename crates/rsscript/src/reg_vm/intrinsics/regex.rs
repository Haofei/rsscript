use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_convert::*;

impl RegVm {
    pub(super) fn exec_regex_intrinsics(
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
            RegIntrinsic::RegexCaptures => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let captures = regex
                    .captures(value)
                    .map(|captures| {
                        captures
                            .iter()
                            .filter_map(|matched| {
                                matched.map(|matched| VmValue::string(matched.as_str()))
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.account_bytes(
                    captures
                        .iter()
                        .filter_map(|value| match value {
                            VmValue::String(value) => Some(value.len()),
                            _ => None,
                        })
                        .sum(),
                )?;
                self.fresh_list(TypedVec::from_values(captures))
            }
            RegIntrinsic::RegexCompile => {
                let pattern = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match regex::Regex::new(pattern) {
                    Ok(_) => value_ok(regex_value(pattern)),
                    Err(error) => value_err(regex_error_value(error.to_string())),
                })
            }
            RegIntrinsic::RegexErrorMessage => {
                read_field_ref(intrinsic_arg(&self.stack, base, args, 0)?, "message")
            }
            RegIntrinsic::RegexFind => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = regex
                    .find(value)
                    .map(|matched| VmValue::some(VmValue::string(matched.as_str())))
                    .unwrap_or(VmValue::OptionNone);
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::RegexIsMatch => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(regex.is_match(value)))
            }
            RegIntrinsic::RegexReplaceAll => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let replacement = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                self.fresh_string(regex.replace_all(value, replacement).to_string())
            }
            RegIntrinsic::RegexSplit => {
                let regex = expect_regex_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let parts = regex.split(value).map(str::to_string).collect::<Vec<_>>();
                self.account_bytes(parts.iter().map(String::len).sum())?;
                self.fresh_list(TypedVec::from_values(
                    parts.into_iter().map(VmValue::string).collect(),
                ))
            }
            other => {
                unreachable!("exec_regex_intrinsics called with non-regex intrinsic: {other:?}")
            }
        }
    }
}
