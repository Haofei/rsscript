//! Dependency-free vocabulary shared by every RSScript tier.
//!
//! Only types and total functions that both the frontend (syntax, semantics,
//! lowering) and the runtime (VM, providers) need may live here. The crate has
//! no internal dependencies by construction, so it cannot smuggle a frontend
//! type into the VM: anything added here is visible to both sides, which is the
//! point of the review gate on this manifest.
#![forbid(unsafe_code)]

/// Cancellation tokens and monotonic deadlines shared by every long-running
/// RSScript operation (frontend analysis, VM execution, provider calls).
mod operation;
/// Stable source coordinates and revisions. Owned here rather than by budget
/// accounting or diagnostics so every tier names one `Span`/`FileId`.
mod source_model;

pub use operation::*;
pub use source_model::*;
