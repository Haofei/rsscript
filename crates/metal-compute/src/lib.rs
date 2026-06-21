//! Native Metal GPU compute FFI.
//!
//! This is the real device-dispatch layer the tinygrad port's `METAL` backend was
//! missing: `MTLDevice` acquisition, MSL source compilation, shared buffers,
//! compute-pipeline creation, kernel dispatch, and readback. It lives in its own
//! crate (not `rsscript-runtime`, which is `#![forbid(unsafe_code)]`) because
//! reading a `MTLBuffer`'s `contents()` pointer is inherently `unsafe`.
//!
//! Everything Metal-specific is behind `#[cfg(target_os = "macos")]`; other targets
//! get a stub that reports the GPU as unavailable and errors from any dispatch, so
//! dependents still build on Linux CI. Bit-for-bit equality with CPU kernels is NOT
//! expected — GPU reductions accumulate in a different order — so callers validate
//! within an f32 tolerance, exactly as tinygrad's own METAL vs CPU results differ.

/// Whether a Metal device is available in this process (always false off macOS).
pub fn metal_available() -> bool {
    imp::metal_available()
}

/// The system default Metal device's name (e.g. "Apple M1 Pro"), or an empty
/// string when no device is available.
pub fn metal_device_name() -> String {
    imp::metal_device_name()
}

/// GPU matrix multiply `(m, k) x (k, n) -> (m, n)`, row-major f32. Uploads `a`/`b`
/// to shared buffers, runs an MSL kernel, and reads the result back. Returns an
/// error string if no device is available or compilation/dispatch fails.
pub fn gpu_matmul(a: &[f32], b: &[f32], m: usize, k: usize, n: usize) -> Result<Vec<f32>, String> {
    imp::gpu_matmul(a, b, m, k, n)
}

/// Compile `source` and dispatch the kernel `fn_name` over `threads` linear
/// threads, binding `inputs[i]` to `buffer(i)` and returning `buffer(inputs.len())`
/// sized `out_len`. A general escape hatch (used by the tests) to prove the FFI runs
/// arbitrary MSL on the GPU; tensor ops use the specialized helpers above.
pub fn gpu_run_1d(
    source: &str,
    fn_name: &str,
    inputs: &[&[f32]],
    out_len: usize,
    threads: usize,
) -> Result<Vec<f32>, String> {
    imp::gpu_run_1d(source, fn_name, inputs, out_len, threads)
}

#[cfg(target_os = "macos")]
mod imp {
    use metal::{Device, MTLCommandBufferStatus, MTLResourceOptions, MTLSize};
    use objc::rc::autoreleasepool;

    pub fn metal_available() -> bool {
        Device::system_default().is_some()
    }

    pub fn metal_device_name() -> String {
        Device::system_default()
            .map(|d| d.name().to_string())
            .unwrap_or_default()
    }

    /// Upload an f32 slice into a shared-storage Metal buffer.
    fn upload(device: &Device, data: &[f32]) -> metal::Buffer {
        let byte_len = std::mem::size_of_val(data) as u64;
        // A zero-length buffer is invalid; round up to one element so empty inputs
        // still produce a usable handle.
        let len = byte_len.max(std::mem::size_of::<f32>() as u64);
        let buffer = device.new_buffer(len, MTLResourceOptions::StorageModeShared);
        if !data.is_empty() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    buffer.contents() as *mut f32,
                    data.len(),
                );
            }
        }
        buffer
    }

    /// Read `len` f32s back out of a shared buffer.
    fn download(buffer: &metal::Buffer, len: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; len];
        if len > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    buffer.contents() as *const f32,
                    out.as_mut_ptr(),
                    len,
                );
            }
        }
        out
    }

    /// Compile MSL `source`, build a compute pipeline for `fn_name`, returning it.
    /// Errors surface as the Metal compiler's message.
    fn make_pipeline(
        device: &Device,
        source: &str,
        fn_name: &str,
    ) -> Result<metal::ComputePipelineState, String> {
        let options = metal::CompileOptions::new();
        let library = device
            .new_library_with_source(source, &options)
            .map_err(|e| format!("metal: MSL compile failed: {e}"))?;
        let function = library
            .get_function(fn_name, None)
            .map_err(|e| format!("metal: kernel '{fn_name}' not found: {e}"))?;
        device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(|e| format!("metal: pipeline creation failed: {e}"))
    }

    const MATMUL_SRC: &str = r#"
#include <metal_stdlib>
using namespace metal;
kernel void rss_matmul(
    device const float* a   [[buffer(0)]],
    device const float* b   [[buffer(1)]],
    device float*       c   [[buffer(2)]],
    constant uint& M        [[buffer(3)]],
    constant uint& K        [[buffer(4)]],
    constant uint& N        [[buffer(5)]],
    uint2 gid               [[thread_position_in_grid]])
{
    uint row = gid.y;
    uint col = gid.x;
    if (row >= M || col >= N) { return; }
    float acc = 0.0f;
    for (uint k = 0; k < K; k++) {
        acc += a[row * K + k] * b[k * N + col];
    }
    c[row * N + col] = acc;
}
"#;

    pub fn gpu_matmul(
        a: &[f32],
        b: &[f32],
        m: usize,
        k: usize,
        n: usize,
    ) -> Result<Vec<f32>, String> {
        if a.len() != m * k {
            return Err(format!("metal matmul: lhs len {} != {m}*{k}", a.len()));
        }
        if b.len() != k * n {
            return Err(format!("metal matmul: rhs len {} != {k}*{n}", b.len()));
        }
        autoreleasepool(|| {
            let device = Device::system_default()
                .ok_or_else(|| "metal: no system default device".to_string())?;
            let pipeline = make_pipeline(&device, MATMUL_SRC, "rss_matmul")?;
            let queue = device.new_command_queue();

            let a_buf = upload(&device, a);
            let b_buf = upload(&device, b);
            let c_len = m * n;
            let c_buf = device.new_buffer(
                (c_len.max(1) * std::mem::size_of::<f32>()) as u64,
                MTLResourceOptions::StorageModeShared,
            );
            let dims = [m as u32, k as u32, n as u32];

            let command_buffer = queue.new_command_buffer();
            let encoder = command_buffer.new_compute_command_encoder();
            encoder.set_compute_pipeline_state(&pipeline);
            encoder.set_buffer(0, Some(&a_buf), 0);
            encoder.set_buffer(1, Some(&b_buf), 0);
            encoder.set_buffer(2, Some(&c_buf), 0);
            encoder.set_bytes(3, 4, (&dims[0] as *const u32).cast());
            encoder.set_bytes(4, 4, (&dims[1] as *const u32).cast());
            encoder.set_bytes(5, 4, (&dims[2] as *const u32).cast());

            // One thread per output element; let Metal pick the threadgroup shape.
            let grid = MTLSize::new(n as u64, m as u64, 1);
            let tew = pipeline.thread_execution_width();
            let max_threads = pipeline.max_total_threads_per_threadgroup();
            let tg_h = (max_threads / tew).max(1);
            let threadgroup = MTLSize::new(tew, tg_h, 1);
            encoder.dispatch_threads(grid, threadgroup);
            encoder.end_encoding();
            command_buffer.commit();
            command_buffer.wait_until_completed();

            if let Some(err) = command_buffer_error(command_buffer) {
                return Err(err);
            }
            Ok(download(&c_buf, c_len))
        })
    }

    pub fn gpu_run_1d(
        source: &str,
        fn_name: &str,
        inputs: &[&[f32]],
        out_len: usize,
        threads: usize,
    ) -> Result<Vec<f32>, String> {
        autoreleasepool(|| {
            let device = Device::system_default()
                .ok_or_else(|| "metal: no system default device".to_string())?;
            let pipeline = make_pipeline(&device, source, fn_name)?;
            let queue = device.new_command_queue();

            let in_bufs: Vec<metal::Buffer> = inputs.iter().map(|d| upload(&device, d)).collect();
            let out_buf = device.new_buffer(
                (out_len.max(1) * std::mem::size_of::<f32>()) as u64,
                MTLResourceOptions::StorageModeShared,
            );

            let command_buffer = queue.new_command_buffer();
            let encoder = command_buffer.new_compute_command_encoder();
            encoder.set_compute_pipeline_state(&pipeline);
            for (i, buf) in in_bufs.iter().enumerate() {
                encoder.set_buffer(i as u64, Some(buf), 0);
            }
            encoder.set_buffer(in_bufs.len() as u64, Some(&out_buf), 0);

            let tew = pipeline.thread_execution_width();
            let threadgroup = MTLSize::new(tew, 1, 1);
            let grid = MTLSize::new(threads as u64, 1, 1);
            encoder.dispatch_threads(grid, threadgroup);
            encoder.end_encoding();
            command_buffer.commit();
            command_buffer.wait_until_completed();

            if let Some(err) = command_buffer_error(command_buffer) {
                return Err(err);
            }
            Ok(download(&out_buf, out_len))
        })
    }

    /// Surface a GPU-side execution failure as a message (the command buffer ends in
    /// the `Error` status when the kernel faults).
    fn command_buffer_error(cb: &metal::CommandBufferRef) -> Option<String> {
        if cb.status() == MTLCommandBufferStatus::Error {
            Some("metal: command buffer finished in Error status".to_string())
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn metal_available() -> bool {
        false
    }

    pub fn metal_device_name() -> String {
        String::new()
    }

    pub fn gpu_matmul(
        _a: &[f32],
        _b: &[f32],
        _m: usize,
        _k: usize,
        _n: usize,
    ) -> Result<Vec<f32>, String> {
        Err("metal: GPU compute is only available on macOS".to_string())
    }

    pub fn gpu_run_1d(
        _source: &str,
        _fn_name: &str,
        _inputs: &[&[f32]],
        _out_len: usize,
        _threads: usize,
    ) -> Result<Vec<f32>, String> {
        Err("metal: GPU compute is only available on macOS".to_string())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn device_is_present_on_mac() {
        if metal_available() {
            assert!(!metal_device_name().is_empty());
        }
    }

    #[test]
    fn gpu_matmul_matches_cpu() {
        if !metal_available() {
            return;
        }
        // (2x3) x (3x2) -> (2x2): [[1,2,3],[4,5,6]] x [[7,8],[9,10],[11,12]].
        let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let b = [7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
        let out = gpu_matmul(&a, &b, 2, 3, 2).unwrap();
        // Expected: [[58,64],[139,154]].
        let expected = [58.0, 64.0, 139.0, 154.0];
        for (g, e) in out.iter().zip(expected.iter()) {
            assert!((g - e).abs() < 1e-3, "gpu {g} vs cpu {e}");
        }
    }

    #[test]
    fn gpu_run_1d_elementwise_add() {
        if !metal_available() {
            return;
        }
        let src = r#"
#include <metal_stdlib>
using namespace metal;
kernel void add(device const float* a [[buffer(0)]],
                device const float* b [[buffer(1)]],
                device float* c [[buffer(2)]],
                uint i [[thread_position_in_grid]]) {
    c[i] = a[i] + b[i];
}
"#;
        let a = [1.0, 2.0, 3.0, 4.0];
        let b = [10.0, 20.0, 30.0, 40.0];
        let out = gpu_run_1d(src, "add", &[&a, &b], 4, 4).unwrap();
        assert_eq!(out, vec![11.0, 22.0, 33.0, 44.0]);
    }
}
