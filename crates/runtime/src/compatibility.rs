//! Legacy process-wide runtime ownership for generated ABI entrypoints.
//!
//! Canonical host integrations pass an explicit `RuntimeServices` through
//! `OperationContext`. This module is the only place allowed to own a global
//! compatibility service.

use std::sync::{Arc, OnceLock};

use crate::RuntimeServices;

pub(crate) fn runtime_services() -> &'static Arc<RuntimeServices> {
    static SERVICES: OnceLock<Arc<RuntimeServices>> = OnceLock::new();
    SERVICES.get_or_init(|| {
        Arc::new(RuntimeServices::new().expect("compatibility runtime services should start"))
    })
}
