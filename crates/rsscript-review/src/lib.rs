//! Optional presentation adapters for compatibility package-review evidence.
//!
//! This crate consumes compiler-produced facts. It does not participate in
//! parsing, semantic validation, lowering, or execution.

mod format;

pub use format::*;
