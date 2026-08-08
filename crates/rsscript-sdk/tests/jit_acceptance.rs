//! Fast JIT acceptance matrix for local VM/JIT work.
//!
//! These tests intentionally stay in the `runtime` target and use only the
//! in-process backend set: interpreter, tier-0 JIT, and, with `native-jit`,
//! native plus deopt/OSR stress backends. The slower generated-Rust backend
//! remains covered by the `differential` target.

mod common;

include!("jit_acceptance/core.rs");
include!("jit_acceptance/optimization.rs");
include!("jit_acceptance/limits.rs");
