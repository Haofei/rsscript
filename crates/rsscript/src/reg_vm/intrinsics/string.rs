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
                Ok(value
                    .split_once(delimiter)
                    .map(|(_, right)| VmValue::some(VmValue::string(right)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringBefore => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .find(delimiter)
                    .map(|index| VmValue::some(VmValue::string(&value[..index])))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringBuilderNew => Ok(VmValue::string("")),
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
                Ok(VmValue::List(Rc::new(RefCell::new(
                    value.chars().map(VmValue::Char).collect(),
                ))))
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
                Ok(VmValue::string(value))
            }
            RegIntrinsic::StringEndsWith => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let suffix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.ends_with(suffix)))
            }
            RegIntrinsic::StringFormat => {
                let template = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let args = expect_string_list_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(string_format(template, &args)))
            }
            RegIntrinsic::StringFromBool => {
                let value = expect_bool_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_string()))
            }
            RegIntrinsic::StringFromFloat => Ok(VmValue::string(
                expect_float_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
            RegIntrinsic::StringFromInt => Ok(VmValue::string(
                expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?.to_string(),
            )),
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
                Ok(VmValue::string(join_string_values(
                    &list.borrow(),
                    &separator,
                )?))
            }
            RegIntrinsic::StringLines => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let lines = value.lines().map(VmValue::string).collect::<Vec<VmValue>>();
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(lines)))))
            }
            RegIntrinsic::StringLen => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(value.len() as i64))
            }
            RegIntrinsic::StringPadLeft => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fill = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(string_pad(value, width, fill, true)))
            }
            RegIntrinsic::StringPadRight => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let width = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let fill = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(string_pad(value, width, fill, false)))
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
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let count = nonnegative_count(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::string(value.repeat(count)))
            }
            RegIntrinsic::StringReplace => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(value.replace(from, to)))
            }
            RegIntrinsic::StringReplaceFirst => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let from = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let to = expect_string_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(value.replacen(from, to, 1)))
            }
            RegIntrinsic::StringReverse => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.chars().rev().collect::<String>()))
            }
            RegIntrinsic::StringSlice => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let len = expect_int_ref(intrinsic_arg(&self.stack, base, args, 2)?)?;
                Ok(VmValue::string(string_slice_range(value, start, len)))
            }
            RegIntrinsic::StringSplit => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let delimiter = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let parts = value
                    .split(delimiter)
                    .map(VmValue::string)
                    .collect::<Vec<VmValue>>();
                Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(parts)))))
            }
            RegIntrinsic::StringStartsWith => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(value.starts_with(prefix)))
            }
            RegIntrinsic::StringStripPrefix => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let prefix = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(value
                    .strip_prefix(prefix)
                    .map(|rest| VmValue::some(VmValue::string(rest)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::StringToLowercase => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_lowercase()))
            }
            RegIntrinsic::StringToUppercase => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.to_uppercase()))
            }
            RegIntrinsic::StringTrim => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.trim()))
            }
            RegIntrinsic::StringTrimEnd => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.trim_end()))
            }
            RegIntrinsic::StringTrimStart => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::string(value.trim_start()))
            }
            other => {
                unreachable!("exec_string_intrinsics called with non-string intrinsic: {other:?}")
            }
        }
    }
}
