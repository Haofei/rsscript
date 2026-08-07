use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct RssDuration {
    pub ms: i64,
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

/// A monotonic deadline: a point on the steady clock. Cooperative code checks
/// `is_expired`/`remaining_ms` in its own loop instead of racing futures, which
/// keeps timeout handling structured and clock-jump-resistant.
#[derive(Debug, Clone)]
pub struct RssDeadline {
    deadline: Instant,
}

pub fn deadline_after_ms(ms: i64) -> RssDeadline {
    RssDeadline {
        deadline: Instant::now() + Duration::from_millis(ms.max(0) as u64),
    }
}

pub fn deadline_after(duration: &RssDuration) -> RssDeadline {
    deadline_after_ms(duration.ms)
}

pub fn deadline_is_expired(deadline: &RssDeadline) -> bool {
    Instant::now() >= deadline.deadline
}

pub fn deadline_remaining_ms(deadline: &RssDeadline) -> i64 {
    let now = Instant::now();
    if now >= deadline.deadline {
        0
    } else {
        (deadline.deadline - now).as_millis() as i64
    }
}

pub(crate) fn deadline_remaining_duration(deadline: &RssDeadline) -> Duration {
    deadline.deadline.saturating_duration_since(Instant::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_zero_is_expired_with_no_remaining() {
        let deadline = deadline_after_ms(0);
        assert!(deadline_is_expired(&deadline));
        assert_eq!(deadline_remaining_ms(&deadline), 0);
    }

    #[test]
    fn deadline_in_future_is_not_expired() {
        let deadline = deadline_after_ms(10_000);
        assert!(!deadline_is_expired(&deadline));
        let remaining = deadline_remaining_ms(&deadline);
        assert!(
            remaining > 0 && remaining <= 10_000,
            "remaining = {remaining}"
        );
    }

    #[test]
    fn deadline_after_duration_matches_after_ms() {
        let deadline = deadline_after(&RssDuration { ms: 5_000 });
        let remaining = deadline_remaining_ms(&deadline);
        assert!(
            remaining > 0 && remaining <= 5_000,
            "remaining = {remaining}"
        );
    }

    #[test]
    fn deadline_clamps_negative_to_now() {
        let deadline = deadline_after_ms(-100);
        assert!(deadline_is_expired(&deadline));
    }
}
