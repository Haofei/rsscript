#![forbid(unsafe_code)]

//! Checked-HIR to provider-neutral MIR lowering.
//!
//! Checked semantic HIR is lowered directly to verifier-owned typed CFG MIR.

mod mir;

pub use mir::{MirLoweringError, lower_checked_hir_to_mir};
