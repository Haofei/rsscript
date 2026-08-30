use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    pub(super) fn exec_date_intrinsics(
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
            RegIntrinsic::DateAddDays => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let days = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(core_date_add_days(unix_ms, days)))
            }
            RegIntrinsic::DateAddMs => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(core_date_add_ms(unix_ms, ms)))
            }
            RegIntrinsic::DateDay => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(core_date_day(unix_ms)))
            }
            RegIntrinsic::DateDaysBetween => {
                let start_unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let end_unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(core_date_days_between(
                    start_unix_ms,
                    end_unix_ms,
                )))
            }
            RegIntrinsic::DateDaysInMonth => {
                let year = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let month = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(core_date_days_in_month(year, month)))
            }
            RegIntrinsic::DateFormatIso => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(core_date_format_iso(unix_ms))
            }
            RegIntrinsic::DateFormatYmd => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(core_date_format_ymd(unix_ms))
            }
            RegIntrinsic::DateHour => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(core_date_hour(unix_ms)))
            }
            RegIntrinsic::DateIsLeapYear => {
                let year = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(core_date_is_leap_year(year)))
            }
            RegIntrinsic::DateMinute => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(core_date_minute(unix_ms)))
            }
            RegIntrinsic::DateMonth => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(core_date_month(unix_ms)))
            }
            RegIntrinsic::DateParseIso => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(core_date_parse_iso(value)
                    .map(|value| VmValue::some(VmValue::Int(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::DateParseYmd => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(core_date_parse_ymd(value)
                    .map(|value| VmValue::some(VmValue::Int(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::DateSecond => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(core_date_second(unix_ms)))
            }
            RegIntrinsic::DateStartOfDay => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(core_date_start_of_day(unix_ms)))
            }
            RegIntrinsic::DateWeekday => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(core_date_weekday(unix_ms)))
            }
            RegIntrinsic::DateYear => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(core_date_year(unix_ms)))
            }
            other => unreachable!("exec_date_intrinsics called with non-date intrinsic: {other:?}"),
        }
    }
}
