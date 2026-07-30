use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use crate::{RssCancellationToken, cancellation_token_is_cancelled};

pub const RUNTIME_PROCESS_CONCURRENCY_CEILING: usize = 32;
pub const RUNTIME_PROCESS_TIMEOUT_CEILING_MS: u64 = 24 * 60 * 60 * 1_000;
pub const DEFAULT_RUNTIME_PROCESS_TIMEOUT_MS: u64 = 30_000;

pub(super) fn process_timeout(timeout_ms: i64) -> Result<Duration, String> {
    if timeout_ms <= 0 {
        return Ok(Duration::from_millis(DEFAULT_RUNTIME_PROCESS_TIMEOUT_MS));
    }
    let timeout_ms = u64::try_from(timeout_ms)
        .map_err(|_| "process timeout must be a positive integer".to_string())?;
    if timeout_ms > RUNTIME_PROCESS_TIMEOUT_CEILING_MS {
        return Err(format!(
            "process timeout exceeds the {}ms runtime ceiling",
            RUNTIME_PROCESS_TIMEOUT_CEILING_MS
        ));
    }
    Ok(Duration::from_millis(timeout_ms))
}

struct ProcessConcurrency {
    active: Mutex<usize>,
    ready: Condvar,
}

pub(super) struct ProcessPermit {
    concurrency: &'static ProcessConcurrency,
}

impl Drop for ProcessPermit {
    fn drop(&mut self) {
        let mut active = self
            .concurrency
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *active = active.saturating_sub(1);
        self.concurrency.ready.notify_one();
    }
}

pub(super) fn process_concurrency_limit() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(1, RUNTIME_PROCESS_CONCURRENCY_CEILING)
}

pub(super) fn acquire_process_permit(
    cancellation: Option<&RssCancellationToken>,
) -> Result<ProcessPermit, String> {
    static CONCURRENCY: OnceLock<ProcessConcurrency> = OnceLock::new();
    let concurrency = CONCURRENCY.get_or_init(|| ProcessConcurrency {
        active: Mutex::new(0),
        ready: Condvar::new(),
    });
    let mut active = concurrency
        .active
        .lock()
        .map_err(|_| "process concurrency lock poisoned".to_string())?;
    while *active >= process_concurrency_limit() {
        if cancellation.is_some_and(cancellation_token_is_cancelled) {
            return Err("process cancelled while waiting for a concurrency slot".to_string());
        }
        let (next, _) = concurrency
            .ready
            .wait_timeout(active, Duration::from_millis(25))
            .map_err(|_| "process concurrency lock poisoned".to_string())?;
        active = next;
    }
    *active += 1;
    Ok(ProcessPermit { concurrency })
}

pub(super) fn process_worker_count(jobs: i64) -> usize {
    if jobs > 0 {
        return usize::try_from(jobs)
            .unwrap_or(RUNTIME_PROCESS_CONCURRENCY_CEILING)
            .min(process_concurrency_limit());
    }
    process_concurrency_limit()
}
