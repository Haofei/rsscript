//! Analysis scheduling, cancellation, and bounded blocking execution.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Semaphore;
use tower_lsp::lsp_types::Url;

#[derive(Default)]
pub(crate) struct AnalysisCancellation {
    pub(crate) cancelled: AtomicBool,
}

impl AnalysisCancellation {
    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

pub(crate) struct PendingAnalysis {
    pub(crate) task: tokio::task::AbortHandle,
    pub(crate) cancellation: Arc<AnalysisCancellation>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum AnalysisKey {
    Package(PathBuf),
    Workspace,
    Uri(Url),
}

pub(crate) const MAX_BLOCKING_ANALYSES: usize = 2;
pub(crate) const MAX_DOCUMENT_BYTES: usize = 16 * 1024 * 1024;
pub(crate) const MAX_PENDING_DIAGNOSTIC_PUBLICATIONS: usize = 4_096;

pub(crate) fn replace_pending_analysis(
    pending: &mut HashMap<AnalysisKey, PendingAnalysis>,
    analysis_key: AnalysisKey,
    task: PendingAnalysis,
) {
    if let Some(previous) = pending.insert(analysis_key, task) {
        previous.cancellation.cancel();
        previous.task.abort();
    }
}

pub(crate) async fn run_bounded_blocking<T, F>(
    permits: Arc<Semaphore>,
    work: F,
) -> std::result::Result<T, tokio::task::JoinError>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let permit = permits
        .acquire_owned()
        .await
        .expect("blocking analysis semaphore closed");
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
}
