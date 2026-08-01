//! `rss-testgen` — a generative-testing framework for the RSScript implementation.
//!
//! It generates well-typed, terminating, deterministic RSScript *programs* from a
//! byte seed ([`gen::generate`]) and runs them through reusable property drivers
//! ([`properties`]): an N-way backend **differential** (every VM-family backend
//! must agree, or all fail), **robustness** (the front end never panics), and the
//! shared backend abstraction ([`backends`]). The same generator and drivers back
//! both the in-suite proptest tests and the libFuzzer targets in `fuzz/`.
//!
//! This tests the toolchain directly; RSScript does not ship a second
//! language-level property-testing package.

pub mod backends;
pub mod generator;
pub mod mutate;
pub mod properties;
pub mod seed;
pub mod ty;

pub use generator::{GeneratedProgram, generate};
