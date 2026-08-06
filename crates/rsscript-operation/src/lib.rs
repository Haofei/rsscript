#![forbid(unsafe_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Atomic storage used by the optional native tier's polling ABI.
    pub fn as_atomic(&self) -> &AtomicBool {
        &self.cancelled
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MonotonicDeadline(Instant);

impl MonotonicDeadline {
    pub fn at(instant: Instant) -> Self {
        Self(instant)
    }

    pub fn after(duration: Duration) -> Self {
        Self(Instant::now() + duration)
    }

    pub fn is_expired(self) -> bool {
        Instant::now() >= self.0
    }

    pub fn instant(self) -> Instant {
        self.0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct OperationId(pub u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_token_observes_cancellation() {
        let token = CancellationToken::new();
        let clone = token.clone();
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[test]
    fn elapsed_deadline_is_expired() {
        assert!(MonotonicDeadline::at(Instant::now() - Duration::from_millis(1)).is_expired());
    }
}
