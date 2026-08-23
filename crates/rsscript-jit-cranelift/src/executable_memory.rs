use crate::{JitError, JitErrorKind};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

const MAX_JIT_PAGE_BYTES: u64 = 64 * 1024;

pub(super) fn arena_allocation_charge(bytes: u64) -> Result<u64, JitError> {
    bytes
        .checked_add(MAX_JIT_PAGE_BYTES - 1)
        .map(|rounded| rounded / MAX_JIT_PAGE_BYTES * MAX_JIT_PAGE_BYTES)
        .ok_or_else(|| {
            JitError::new(
                JitErrorKind::AdmissionRejected,
                "JIT arena allocation charge overflow",
            )
        })
}

/// Shared hard limit for executable-memory allocations made by one or more
/// [`crate::NativeModule`]s.
///
/// Budgeted modules reserve a fixed Cranelift arena before codegen. The arena is
/// the allocation boundary for code and JIT-owned data and is unmapped when its
/// module is dropped. Reserving the whole arena makes the configured limit an
/// address-space hard bound rather than a post-codegen admission counter.
#[derive(Clone, Debug)]
pub struct ExecutableMemoryBudget {
    inner: Arc<ExecutableMemoryBudgetInner>,
}

#[derive(Debug)]
struct ExecutableMemoryBudgetInner {
    limit: u64,
    allocated: AtomicU64,
}

impl ExecutableMemoryBudget {
    pub fn new(limit: u64) -> Self {
        Self {
            inner: Arc::new(ExecutableMemoryBudgetInner {
                limit,
                allocated: AtomicU64::new(0),
            }),
        }
    }

    pub fn limit(&self) -> u64 {
        self.inner.limit
    }

    pub fn allocated(&self) -> u64 {
        self.inner.allocated.load(Ordering::Acquire)
    }

    pub(super) fn reserve(&self, bytes: u64) -> Result<ExecutableMemoryReservation, JitError> {
        self.inner
            .allocated
            .try_update(Ordering::AcqRel, Ordering::Acquire, |allocated| {
                allocated
                    .checked_add(bytes)
                    .filter(|total| *total <= self.inner.limit)
            })
            .map(|_| ExecutableMemoryReservation {
                budget: self.clone(),
                bytes,
            })
            .map_err(|_| {
                JitError::new(JitErrorKind::AdmissionRejected, format!(
                    "JIT executable-memory budget exceeded: requested {bytes} bytes with {} of {} bytes allocated",
                    self.allocated(),
                    self.limit()
                ))
            })
    }

    fn release(&self, bytes: u64) {
        let previous = self.inner.allocated.fetch_sub(bytes, Ordering::AcqRel);
        debug_assert!(previous >= bytes, "JIT memory accounting underflow");
    }
}

pub(super) struct ExecutableMemoryReservation {
    budget: ExecutableMemoryBudget,
    bytes: u64,
}

impl Drop for ExecutableMemoryReservation {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
}
