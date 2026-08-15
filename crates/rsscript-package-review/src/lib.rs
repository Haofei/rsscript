//! Captured package-review compatibility inputs.
//!
//! This crate owns the manifest/source-set representation used by optional
//! package review tooling. It is deliberately separate from the compiler's
//! normal in-memory frontend path: callers capture project input before asking
//! the compiler for semantic facts.

mod source_set;

pub use source_set::*;
