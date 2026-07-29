//! Safe re-export of the native Metal GPU compute FFI.
//!
//! The actual `unsafe` Metal bindings (MTLDevice / MSL compile / shared buffers /
//! pipeline / dispatch / readback) live in the `rss-metal-compute` crate, because
//! this runtime crate is `#![forbid(unsafe_code)]`. Off macOS that crate is a stub
//! reporting the GPU as unavailable, so these functions are total on every target.

#[cfg(feature = "gpu")]
pub use rss_metal_compute::{gpu_matmul, metal_available, metal_device_name};

/// MSL source that the embedding host has explicitly chosen to trust.
///
/// This is an API capability marker, not a sandbox. Tenant-provided shaders
/// still belong in a killable worker process.
#[derive(Debug, Clone, Copy)]
pub struct TrustedShader<'a> {
    _source: &'a str,
}

impl<'a> TrustedShader<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { _source: source }
    }
}

#[cfg(feature = "gpu")]
pub fn gpu_run_1d_trusted(
    shader: TrustedShader<'_>,
    fn_name: &str,
    inputs: &[&[f32]],
    out_len: usize,
    threads: usize,
) -> Result<Vec<f32>, String> {
    rss_metal_compute::gpu_run_1d_trusted(
        rss_metal_compute::TrustedShader::new(shader._source),
        fn_name,
        inputs,
        out_len,
        threads,
    )
    .map_err(String::from)
}

#[cfg(not(feature = "gpu"))]
pub fn metal_available() -> bool {
    false
}

#[cfg(not(feature = "gpu"))]
pub fn metal_device_name() -> String {
    String::new()
}

#[cfg(not(feature = "gpu"))]
pub fn gpu_matmul(
    _a: &[f32],
    _b: &[f32],
    _m: usize,
    _k: usize,
    _n: usize,
) -> Result<Vec<f32>, String> {
    Err("metal: runtime built without the `gpu` feature".to_string())
}

#[cfg(not(feature = "gpu"))]
pub fn gpu_run_1d_trusted(
    _shader: TrustedShader<'_>,
    _fn_name: &str,
    _inputs: &[&[f32]],
    _out_len: usize,
    _threads: usize,
) -> Result<Vec<f32>, String> {
    Err("metal: runtime built without the `gpu` feature".to_string())
}
