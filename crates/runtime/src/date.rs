use chrono::{DateTime, Datelike, NaiveDate, SecondsFormat, TimeZone, Timelike, Utc};

const MS_PER_DAY: i64 = 86_400_000;

pub fn date_year(unix_ms: i64) -> i64 {
    utc_datetime(unix_ms).year() as i64
}

pub fn date_month(unix_ms: i64) -> i64 {
    utc_datetime(unix_ms).month() as i64
}

pub fn date_day(unix_ms: i64) -> i64 {
    utc_datetime(unix_ms).day() as i64
}

pub fn date_hour(unix_ms: i64) -> i64 {
    utc_datetime(unix_ms).hour() as i64
}

pub fn date_minute(unix_ms: i64) -> i64 {
    utc_datetime(unix_ms).minute() as i64
}

pub fn date_second(unix_ms: i64) -> i64 {
    utc_datetime(unix_ms).second() as i64
}

pub fn date_add_ms(unix_ms: i64, ms: i64) -> i64 {
    unix_ms.saturating_add(ms)
}

pub fn date_add_days(unix_ms: i64, days: i64) -> i64 {
    unix_ms.saturating_add(days.saturating_mul(MS_PER_DAY))
}

pub fn date_days_between(start_unix_ms: i64, end_unix_ms: i64) -> i64 {
    end_unix_ms.saturating_sub(start_unix_ms) / MS_PER_DAY
}

pub fn date_start_of_day(unix_ms: i64) -> i64 {
    let datetime = utc_datetime(unix_ms);
    let start = datetime
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("midnight is valid");
    Utc.from_utc_datetime(&start).timestamp_millis()
}

pub fn date_weekday(unix_ms: i64) -> i64 {
    utc_datetime(unix_ms).weekday().number_from_monday() as i64
}

pub fn date_is_leap_year(year: i64) -> bool {
    let Ok(year) = i32::try_from(year) else {
        return false;
    };
    NaiveDate::from_ymd_opt(year, 2, 29).is_some()
}

pub fn date_days_in_month(year: i64, month: i64) -> i64 {
    let Ok(year) = i32::try_from(year) else {
        return 0;
    };
    let Ok(month) = u32::try_from(month) else {
        return 0;
    };
    days_in_month(year, month).map(i64::from).unwrap_or(0)
}

pub fn date_format_ymd(unix_ms: i64) -> String {
    utc_datetime(unix_ms).format("%Y-%m-%d").to_string()
}

pub fn date_parse_ymd(value: &str) -> Option<i64> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let datetime = date.and_hms_opt(0, 0, 0)?;
    Some(Utc.from_utc_datetime(&datetime).timestamp_millis())
}

pub fn date_format_iso(unix_ms: i64) -> String {
    utc_datetime(unix_ms).to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub fn date_parse_iso(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc).timestamp_millis())
}

fn utc_datetime(unix_ms: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_millis_opt(unix_ms)
        .single()
        .unwrap_or_else(|| {
            Utc.timestamp_millis_opt(0)
                .single()
                .expect("epoch is valid")
        })
}

fn days_in_month(year: i32, month: u32) -> Option<u32> {
    let next_month = if month == 12 {
        NaiveDate::from_ymd_opt(year.checked_add(1)?, 1, 1)?
    } else {
        NaiveDate::from_ymd_opt(year, month.checked_add(1)?, 1)?
    };
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    Some((next_month - first).num_days() as u32)
}
