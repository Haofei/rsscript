//! Legacy process-wide runtime discovery for generated ABI entrypoints.
//!
//! Canonical host integrations pass an explicit `RuntimeServices` through
//! `OperationContext`. The registry intentionally stores only a weak reference:
//! operations and pending values own the service lifetime.

use std::sync::{Arc, Mutex, OnceLock, Weak};

#[cfg(feature = "legacy-host")]
use crate::async_runtime::ProcessPermit;
use crate::{OperationContext, ResourceBudget, RssCancellationToken, RssDeadline, RuntimeServices};

/// The only process-wide runtime factory. It exists solely for generated-ABI
/// compatibility entrypoints; canonical operations receive an explicit owner.
fn generated_abi_runtime_services() -> Arc<RuntimeServices> {
    static SERVICES: OnceLock<Mutex<Weak<RuntimeServices>>> = OnceLock::new();
    let registry = SERVICES.get_or_init(|| Mutex::new(Weak::new()));
    let mut registered = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(services) = registered
        .upgrade()
        .filter(|services| !services.is_shutdown())
    {
        return services;
    }
    let services =
        Arc::new(RuntimeServices::new().expect("compatibility runtime services should start"));
    *registered = Arc::downgrade(&services);
    services
}

pub(crate) fn generated_abi_operation_context(
    deadline: RssDeadline,
    cancellation: RssCancellationToken,
    byte_budget: ResourceBudget,
) -> OperationContext {
    OperationContext::with_services(
        deadline,
        cancellation,
        byte_budget,
        generated_abi_runtime_services(),
    )
}

#[cfg(feature = "legacy-host")]
pub(crate) fn generated_abi_process_permit(
    cancellation: Option<&RssCancellationToken>,
) -> Result<ProcessPermit, String> {
    generated_abi_runtime_services().acquire_process_permit(cancellation)
}

pub(crate) fn generated_abi_services_for_pending() -> Arc<RuntimeServices> {
    generated_abi_runtime_services()
}
