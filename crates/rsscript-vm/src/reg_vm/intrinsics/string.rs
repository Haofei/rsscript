use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    #[allow(clippy::mutable_key_type)]
    pub(super) fn exec_string_intrinsics(
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
            RegIntrinsic::StringAfter => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let right = value
                    .split_once(delimiter)
                    .map(|(_, right)| right.to_string());
                match right {
                    Some(right) => Ok(VmValue::some(self.fresh_string(right)?)),
                    None => Ok(VmValue::OptionNone),
                }
            }
            RegIntrinsic::StringBefore => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let left = value
                    .find(delimiter)
                    .map(|index| value[..index].to_string());
                match left {
                    Some(left) => Ok(VmValue::some(self.fresh_string(left)?)),
                    None => Ok(VmValue::OptionNone),
                }
            }
            RegIntrinsic::StringBuilderNew => {
                Ok(VmValue::Managed(Rc::new(RefCell::new(VmValue::string("")))))
            }
            RegIntrinsic::StringCharAt => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let index = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(usize::try_from(index)
                    .ok()
                    .and_then(|index| value.chars().nth(index))
                    .map(|value| VmValue::some(VmValue::Char(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringChars => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let chars = value.chars().map(VmValue::Char).collect();
                self.fresh_list(chars)
            }
            RegIntrinsic::StringContains => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let needle = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.contains(needle)))
            }
            RegIntrinsic::StringCount => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let needle = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(value.matches(needle).count() as i64))
            }
            RegIntrinsic::StringCopy => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = value.to_string();
                self.fresh_string(value)
            }
            RegIntrinsic::StringEndsWith => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let suffix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.ends_with(suffix)))
            }
            RegIntrinsic::StringFormat => {
                let template = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let args = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let formatted = string_format(template, &args);
                self.fresh_string(formatted)
            }
            RegIntrinsic::StringFromBool => {
                let value = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(value.to_string())
            }
            RegIntrinsic::StringFromFloat => self.fresh_string(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            ),
            RegIntrinsic::StringFromInt => self.fresh_string(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            ),
            RegIntrinsic::StringIndexOf => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let needle = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .find(needle)
                    .map(|index| VmValue::some(VmValue::Int(index as i64)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringIsEmpty => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(value.is_empty()))
            }
            RegIntrinsic::StringJoin => {
                let list = expect_list_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let separator =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?.to_string();
                let joined = join_string_values(&list.borrow(), &separator)?;
                self.fresh_string(joined)
            }
            RegIntrinsic::StringLines => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = value.lines().count();
                let payload_bytes = value.lines().map(str::len).sum();
                self.ensure_string_list_available(count, payload_bytes)?;
                let mut lines = Vec::with_capacity(count);
                lines.extend(value.lines().map(|line| VmValue::string(line.to_owned())));
                self.account_bytes(payload_bytes)?;
                self.fresh_list(TypedVec::from_values(lines))
            }
            RegIntrinsic::StringLen => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value.len() as i64))
            }
            RegIntrinsic::StringPadLeft => {
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let value =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_owned();
                let fill =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_owned();
                // Charge the (size-parameterized) result against `allocation_budget` BEFORE
                // allocating, so `pad_*` cannot allocate an arbitrarily large string
                // in one step and bypass the memory ceiling.
                let result_len = crate::text_util::string_pad_len(&value, width, &fill)
                    .and_then(|len| usize::try_from(len).ok())
                    .ok_or_else(|| {
                        EvalError::Runtime("String.pad_left result is too large".into())
                    })?;
                self.account_bytes(result_len)?;
                Ok(VmValue::string(string_pad(&value, width, &fill, true)))
            }
            RegIntrinsic::StringPadRight => {
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let value =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_owned();
                let fill =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?.to_owned();
                let result_len = crate::text_util::string_pad_len(&value, width, &fill)
                    .and_then(|len| usize::try_from(len).ok())
                    .ok_or_else(|| {
                        EvalError::Runtime("String.pad_right result is too large".into())
                    })?;
                self.account_bytes(result_len)?;
                Ok(VmValue::string(string_pad(&value, width, &fill, false)))
            }
            RegIntrinsic::StringParseFloat => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match value.parse::<f64>() {
                    Ok(value) => VmValue::some(VmValue::Float(value)),
                    Err(_) => VmValue::OptionNone,
                })
            }
            RegIntrinsic::StringParseInt => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(match value.parse::<i64>() {
                    Ok(value) => VmValue::some(VmValue::Int(value)),
                    Err(_) => VmValue::OptionNone,
                })
            }
            RegIntrinsic::StringRepeat => {
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let value =
                    expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_owned();
                // Charge the projected result against `allocation_budget` BEFORE allocating,
                // so `"x".repeat(2_000_000_000)` trips the memory ceiling instead of
                // eagerly allocating ~2 GB in a single intrinsic step.
                self.account_bytes(value.len().saturating_mul(count))?;
                Ok(VmValue::string(value.repeat(count)))
            }
            RegIntrinsic::StringReplace => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let replaced = value.replace(from, to);
                self.fresh_string(replaced)
            }
            RegIntrinsic::StringReplaceFirst => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let replaced = value.replacen(from, to, 1);
                self.fresh_string(replaced)
            }
            RegIntrinsic::StringReverse => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let reversed = value.chars().rev().collect::<String>();
                self.fresh_string(reversed)
            }
            RegIntrinsic::StringSlice => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                let sliced = string_slice_range(value, start, len).to_string();
                self.fresh_string(sliced)
            }
            RegIntrinsic::StringSplit => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let count = value.split(delimiter).count();
                let payload_bytes = value.split(delimiter).map(str::len).sum();
                self.ensure_string_list_available(count, payload_bytes)?;
                let mut parts = Vec::with_capacity(count);
                parts.extend(
                    value
                        .split(delimiter)
                        .map(|part| VmValue::string(part.to_owned())),
                );
                self.account_bytes(payload_bytes)?;
                self.fresh_list(TypedVec::from_values(parts))
            }
            RegIntrinsic::StringStartsWith => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.starts_with(prefix)))
            }
            RegIntrinsic::StringStripPrefix => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let rest = value.strip_prefix(prefix).map(str::to_string);
                match rest {
                    Some(rest) => Ok(VmValue::some(self.fresh_string(rest)?)),
                    None => Ok(VmValue::OptionNone),
                }
            }
            RegIntrinsic::StringToLowercase => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = value.to_lowercase();
                self.fresh_string(value)
            }
            RegIntrinsic::StringToUppercase => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = value.to_uppercase();
                self.fresh_string(value)
            }
            RegIntrinsic::StringTrim => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = value.trim().to_string();
                self.fresh_string(value)
            }
            RegIntrinsic::StringTrimEnd => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = value.trim_end().to_string();
                self.fresh_string(value)
            }
            RegIntrinsic::StringTrimStart => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let value = value.trim_start().to_string();
                self.fresh_string(value)
            }
            other => {
                unreachable!("exec_string_intrinsics called with non-string intrinsic: {other:?}")
            }
        }
    }
}
