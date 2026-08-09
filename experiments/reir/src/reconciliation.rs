mod capability_match;
mod engine;
mod index;
mod model;

pub use engine::{
    reconcile_capabilities, reconcile_capabilities_for_gate, reconcile_capabilities_for_target,
    reconcile_capabilities_with_limits,
};
pub use model::{
    Reconciliation, ReconciliationKind, ReconciliationLimits, ReconciliationRisk,
    ReconciliationStatus, RiskClass, RiskSeverity,
};
