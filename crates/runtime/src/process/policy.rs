use std::time::Duration;

use crate::RssCancellationToken;

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

pub(super) fn process_concurrency_limit() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(4)
        .clamp(1, RUNTIME_PROCESS_CONCURRENCY_CEILING)
}

pub(super) fn acquire_process_permit(
    cancellation: Option<&RssCancellationToken>,
) -> Result<crate::async_runtime::ProcessPermit, String> {
    crate::async_runtime::default_runtime_services().acquire_process_permit(cancellation)
}

pub(super) fn process_worker_count(jobs: i64) -> usize {
    if jobs > 0 {
        return usize::try_from(jobs)
            .unwrap_or(RUNTIME_PROCESS_CONCURRENCY_CEILING)
            .min(process_concurrency_limit());
    }
    process_concurrency_limit()
}
