//! Safe re-export of the native Metal GPU compute FFI.
//!
//! The actual `unsafe` Metal bindings (MTLDevice / MSL compile / shared buffers /
//! pipeline / dispatch / readback) live in the `rss-metal-compute` crate, because
//! this runtime crate is `#![forbid(unsafe_code)]`. Off macOS that crate is a stub
//! reporting the GPU as unavailable, so these functions are total on every target.

pub use rss_metal_compute::{gpu_matmul, gpu_run_1d, metal_available, metal_device_name};
