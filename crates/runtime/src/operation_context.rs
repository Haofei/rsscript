use std::sync::Arc;

use crate::{ResourceBudget, RssCancellationToken, RssDeadline, RuntimeServices};

/// Controls shared by one runtime operation and any work it starts.
///
/// Clones retain the same cancellation signal and cumulative byte budget.
#[derive(Clone)]
pub struct OperationContext {
    deadline: RssDeadline,
    cancellation: RssCancellationToken,
    byte_budget: ResourceBudget,
    services: Arc<RuntimeServices>,
}

impl OperationContext {
    pub fn new(
        deadline: RssDeadline,
        cancellation: RssCancellationToken,
        byte_budget: ResourceBudget,
    ) -> Self {
        crate::compatibility::generated_abi_operation_context(deadline, cancellation, byte_budget)
    }

    pub fn with_services(
        deadline: RssDeadline,
        cancellation: RssCancellationToken,
        byte_budget: ResourceBudget,
        services: Arc<RuntimeServices>,
    ) -> Self {
        Self {
            deadline,
            cancellation,
            byte_budget,
            services,
        }
    }

    /// Adapts APIs that historically accepted resource controls in this order.
    pub fn from_resources(
        byte_budget: ResourceBudget,
        cancellation: RssCancellationToken,
        deadline: RssDeadline,
    ) -> Self {
        Self::new(deadline, cancellation, byte_budget)
    }

    pub fn deadline(&self) -> &RssDeadline {
        &self.deadline
    }

    pub fn cancellation(&self) -> &RssCancellationToken {
        &self.cancellation
    }

    pub fn byte_budget(&self) -> &ResourceBudget {
        &self.byte_budget
    }

    pub fn services(&self) -> &Arc<RuntimeServices> {
        &self.services
    }
}

impl std::fmt::Debug for OperationContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("OperationContext")
            .field("deadline", &self.deadline)
            .field("cancellation", &self.cancellation)
            .field("byte_budget", &self.byte_budget)
            .field("services_shutdown", &self.services.is_shutdown())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        cancellation_never, cancellation_source_cancel, cancellation_source_new,
        cancellation_source_token, cancellation_token_is_cancelled, deadline_after_ms,
        deadline_is_expired,
    };

    #[test]
    fn clones_share_cancellation_and_byte_consumption() {
        let mut source = cancellation_source_new();
        let context = OperationContext::new(
            deadline_after_ms(10_000),
            cancellation_source_token(&source),
            ResourceBudget::new(8),
        );
        let clone = context.clone();

        clone
            .byte_budget()
            .try_consume(5)
            .expect("consumption should fit");
        cancellation_source_cancel(&mut source);

        assert_eq!(context.byte_budget().bytes_used(), 5);
        assert!(cancellation_token_is_cancelled(context.cancellation()));
        assert!(cancellation_token_is_cancelled(clone.cancellation()));
    }

    #[test]
    fn resource_adapter_preserves_all_controls() {
        let budget = ResourceBudget::new(4);
        let context = OperationContext::from_resources(
            budget.clone(),
            cancellation_never(),
            deadline_after_ms(0),
        );

        assert!(deadline_is_expired(context.deadline()));
        context
            .byte_budget()
            .try_consume(3)
            .expect("consumption should fit");
        assert_eq!(budget.bytes_used(), 3);
        assert!(!cancellation_token_is_cancelled(context.cancellation()));
    }

    #[test]
    fn context_explicitly_retains_its_runtime_services_owner() {
        let services = Arc::new(RuntimeServices::new().expect("runtime services"));
        let context = OperationContext::with_services(
            deadline_after_ms(10_000),
            cancellation_never(),
            ResourceBudget::new(8),
            services.clone(),
        );
        let clone = context.clone();

        assert!(Arc::ptr_eq(context.services(), &services));
        assert!(Arc::ptr_eq(clone.services(), &services));
        services.shutdown(std::time::Duration::from_secs(1));
        assert!(context.services().is_shutdown());
    }
}
