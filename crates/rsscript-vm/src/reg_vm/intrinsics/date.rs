use super::super::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    #[allow(clippy::mutable_key_type)]
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
                Ok(VmValue::Int(
                    unix_ms.saturating_add(days.saturating_mul(MS_PER_DAY)),
                ))
            }
            RegIntrinsic::DateAddMs => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(unix_ms.saturating_add(ms)))
            }
            RegIntrinsic::DateDay => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).day() as i64))
            }
            RegIntrinsic::DateDaysBetween => {
                let start_unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let end_unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(
                    end_unix_ms.saturating_sub(start_unix_ms) / MS_PER_DAY,
                ))
            }
            RegIntrinsic::DateDaysInMonth => {
                let year = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let month = expect_int_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Int(date_days_in_month(year, month)))
            }
            RegIntrinsic::DateFormatIso => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(
                    utc_datetime(unix_ms).to_rfc3339_opts(SecondsFormat::Millis, true),
                )
            }
            RegIntrinsic::DateFormatYmd => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(utc_datetime(unix_ms).format("%Y-%m-%d").to_string())
            }
            RegIntrinsic::DateHour => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).hour() as i64))
            }
            RegIntrinsic::DateIsLeapYear => {
                let year = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(date_is_leap_year(year)))
            }
            RegIntrinsic::DateMinute => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).minute() as i64))
            }
            RegIntrinsic::DateMonth => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).month() as i64))
            }
            RegIntrinsic::DateParseIso => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(date_parse_iso(value)
                    .map(|value| VmValue::some(VmValue::Int(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::DateParseYmd => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(date_parse_ymd(value)
                    .map(|value| VmValue::some(VmValue::Int(value)))
                    .unwrap_or(VmValue::OptionNone))
            }
            RegIntrinsic::DateSecond => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).second() as i64))
            }
            RegIntrinsic::DateStartOfDay => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let start = utc_datetime(unix_ms)
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight is valid");
                Ok(VmValue::Int(
                    Utc.from_utc_datetime(&start).timestamp_millis(),
                ))
            }
            RegIntrinsic::DateWeekday => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(
                    utc_datetime(unix_ms).weekday().number_from_monday() as i64,
                ))
            }
            RegIntrinsic::DateYear => {
                let unix_ms = expect_int_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Int(utc_datetime(unix_ms).year() as i64))
            }
            other => unreachable!("exec_date_intrinsics called with non-date intrinsic: {other:?}"),
        }
    }
}
