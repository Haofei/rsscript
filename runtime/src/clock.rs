use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct RssInstant {
    inner: Instant,
}

#[derive(Debug, Clone)]
pub struct RssDuration {
    pub ms: i64,
}

pub fn clock_now() -> RssInstant {
    RssInstant {
        inner: Instant::now(),
    }
}

pub fn clock_system_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as i64
}

pub fn instant_elapsed(start: &RssInstant) -> RssDuration {
    let elapsed = start.inner.elapsed();
    RssDuration {
        ms: elapsed.as_millis() as i64,
    }
}

pub fn duration_ms(value: i64) -> RssDuration {
    RssDuration { ms: value }
}

pub fn duration_seconds(value: i64) -> RssDuration {
    RssDuration { ms: value * 1000 }
}

pub fn duration_as_ms(value: &RssDuration) -> i64 {
    value.ms
}

pub fn duration_as_seconds(value: &RssDuration) -> i64 {
    value.ms / 1000
}

pub fn duration_add(left: &RssDuration, right: &RssDuration) -> RssDuration {
    RssDuration {
        ms: left.ms + right.ms,
    }
}
