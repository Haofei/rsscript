//! Legacy process-wide runtime discovery for generated ABI entrypoints.
//!
//! Canonical host integrations pass an explicit `RuntimeServices` through
//! `OperationContext`. The registry intentionally stores only a weak reference:
//! operations and pending values own the service lifetime.

use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::RuntimeServices;

pub(crate) fn runtime_services() -> Arc<RuntimeServices> {
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
