use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub const RUNTIME_ALLOCATION_CEILING_BYTES: usize = 64 * 1024 * 1024;

pub(crate) fn bounded_allocation_size(size: i64, operation: &str) -> usize {
    let size = usize::try_from(size.max(0))
        .unwrap_or_else(|_| panic!("{operation} size does not fit this platform"));
    assert!(
        size <= RUNTIME_ALLOCATION_CEILING_BYTES,
        "{operation} size {size} exceeds runtime allocation ceiling of \
         {RUNTIME_ALLOCATION_CEILING_BYTES} bytes"
    );
    size
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceBudgetError {
    requested: u64,
    remaining: u64,
}

impl ResourceBudgetError {
    pub fn requested(&self) -> u64 {
        self.requested
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }
}

impl std::fmt::Display for ResourceBudgetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "resource byte budget exhausted: requested {} bytes with {} remaining",
            self.requested, self.remaining
        )
    }
}

impl std::error::Error for ResourceBudgetError {}

#[derive(Debug, Clone)]
pub struct ResourceBudget {
    limit: u64,
    used: Arc<AtomicU64>,
}

impl ResourceBudget {
    pub fn new(byte_limit: u64) -> Self {
        Self {
            limit: byte_limit,
            used: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn byte_limit(&self) -> u64 {
        self.limit
    }

    pub fn bytes_used(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    pub fn bytes_remaining(&self) -> u64 {
        self.limit.saturating_sub(self.bytes_used())
    }

    pub fn try_consume(&self, bytes: usize) -> Result<(), ResourceBudgetError> {
        self.try_reserve(bytes).map(ResourceReservation::commit_all)
    }

    pub(crate) fn try_reserve(
        &self,
        bytes: usize,
    ) -> Result<ResourceReservation, ResourceBudgetError> {
        let requested = u64::try_from(bytes).unwrap_or(u64::MAX);
        let result = self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(requested)
                    .filter(|next| *next <= self.limit)
            });
        match result {
            Ok(_) => Ok(ResourceReservation {
                budget: self.clone(),
                reserved: requested,
                committed: false,
            }),
            Err(used) => Err(ResourceBudgetError {
                requested,
                remaining: self.limit.saturating_sub(used),
            }),
        }
    }

    fn refund(&self, bytes: u64) {
        if bytes != 0 {
            self.used.fetch_sub(bytes, Ordering::AcqRel);
        }
    }
}

pub(crate) struct ResourceReservation {
    budget: ResourceBudget,
    reserved: u64,
    committed: bool,
}

impl ResourceReservation {
    #[cfg(any(feature = "host-compat", test))]
    pub(crate) fn commit(mut self, actual_bytes: usize) {
        let actual = u64::try_from(actual_bytes)
            .unwrap_or(u64::MAX)
            .min(self.reserved);
        self.budget.refund(self.reserved - actual);
        self.committed = true;
    }

    fn commit_all(mut self) {
        self.committed = true;
    }
}

impl Drop for ResourceReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.budget.refund(self.reserved);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloned_budgets_share_consumption() {
        let budget = ResourceBudget::new(8);
        budget.try_consume(5).expect("first charge should fit");
        let error = budget
            .clone()
            .try_consume(4)
            .expect_err("clones must share the same limit");
        assert_eq!(error.requested(), 4);
        assert_eq!(error.remaining(), 3);
        assert_eq!(budget.bytes_used(), 5);
    }

    #[test]
    fn reservation_refunds_unused_capacity() {
        let budget = ResourceBudget::new(10);
        let reservation = budget.try_reserve(8).expect("reservation should fit");
        reservation.commit(3);
        assert_eq!(budget.bytes_used(), 3);
        assert_eq!(budget.bytes_remaining(), 7);
    }

    #[test]
    fn dropped_reservation_is_fully_refunded() {
        let budget = ResourceBudget::new(4);
        drop(budget.try_reserve(4).expect("reservation should fit"));
        assert_eq!(budget.bytes_used(), 0);
    }
}
