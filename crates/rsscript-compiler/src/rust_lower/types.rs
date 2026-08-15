//! Compatibility bridge for the experimental AOT output model.
//!
//! The data contracts live in `experiments/aot-model`; this module preserves
//! internal paths while the remaining lowerer implementation migrates.

pub use rsscript_aot_model::{
    GeneratedRustPackage, LoweredRust, RemappedRustcDiagnostic, RustSourceMapEntry,
};
