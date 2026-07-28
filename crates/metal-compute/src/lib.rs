//! Native Metal GPU compute FFI.
//!
//! Metal-specific code is behind `#[cfg(target_os = "macos")]`. Validation and
//! resource accounting stay platform-neutral so overflow behavior is identical
//! and testable on every target.

use std::collections::BTreeSet;
use std::fmt;

use sha2::{Digest, Sha256};

#[cfg(any(test, target_os = "macos"))]
use std::collections::VecDeque;

const FLOAT_BYTES: usize = std::mem::size_of::<f32>();
#[cfg(target_os = "macos")]
const FLOAT_BYTES_U64: u64 = 4;
const MAX_MSL_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_FUNCTION_NAME_BYTES: usize = 256;
const MAX_INPUT_BUFFERS: usize = 30;
const MAX_RAW_BUFFER_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RAW_TOTAL_INPUT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_RAW_DISPATCH_THREADS: u64 = 4_294_967_295;
#[cfg(target_os = "macos")]
const PIPELINE_CACHE_CAPACITY: usize = 32;

/// A validation, resource, compilation, or dispatch failure from the Metal path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetalError {
    DimensionOverflow {
        expression: &'static str,
        lhs: usize,
        rhs: usize,
    },
    DimensionTooLarge {
        dimension: &'static str,
        value: usize,
        max: u32,
    },
    ByteSizeOverflow {
        elements: usize,
        element_size: usize,
    },
    TotalByteSizeOverflow,
    BufferTooLarge {
        buffer: String,
        bytes: u64,
        max_bytes: u64,
    },
    InvalidBufferLength {
        buffer: &'static str,
        actual: usize,
        expected: usize,
    },
    DispatchTooLarge {
        threads: usize,
    },
    ResourceLimit {
        resource: &'static str,
        actual: usize,
        limit: usize,
    },
    InvalidFunctionName,
    DeviceUnavailable,
    Compilation(String),
    FunctionNotFound {
        function: String,
        message: String,
    },
    PipelineCreation(String),
    InvalidThreadgroup {
        width: u64,
        height: u64,
        max_threads: u64,
        max_width: u64,
        max_height: u64,
    },
    CommandExecution,
    UnsupportedPlatform,
    UntrustedShader {
        digest: String,
    },
}

impl fmt::Display for MetalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionOverflow {
                expression,
                lhs,
                rhs,
            } => write!(
                f,
                "metal: dimension calculation {expression} overflowed ({lhs} * {rhs})"
            ),
            Self::DimensionTooLarge {
                dimension,
                value,
                max,
            } => write!(
                f,
                "metal: dimension {dimension}={value} exceeds kernel uint limit {max}"
            ),
            Self::ByteSizeOverflow {
                elements,
                element_size,
            } => write!(
                f,
                "metal: byte size overflow for {elements} elements of {element_size} bytes"
            ),
            Self::TotalByteSizeOverflow => {
                write!(f, "metal: total input buffer byte size overflow")
            }
            Self::BufferTooLarge {
                buffer,
                bytes,
                max_bytes,
            } => write!(
                f,
                "metal: {buffer} buffer requires {bytes} bytes, limit is {max_bytes}"
            ),
            Self::InvalidBufferLength {
                buffer,
                actual,
                expected,
            } => write!(
                f,
                "metal matmul: {buffer} len {actual} != expected {expected}"
            ),
            Self::DispatchTooLarge { threads } => {
                write!(f, "metal: dispatch width {threads} does not fit in u64")
            }
            Self::ResourceLimit {
                resource,
                actual,
                limit,
            } => write!(
                f,
                "metal: {resource} size/count {actual} exceeds limit {limit}"
            ),
            Self::InvalidFunctionName => write!(f, "metal: kernel function name must not be empty"),
            Self::DeviceUnavailable => write!(f, "metal: no system default device"),
            Self::Compilation(message) => write!(f, "metal: MSL compile failed: {message}"),
            Self::FunctionNotFound { function, message } => {
                write!(f, "metal: kernel '{function}' not found: {message}")
            }
            Self::PipelineCreation(message) => {
                write!(f, "metal: pipeline creation failed: {message}")
            }
            Self::InvalidThreadgroup {
                width,
                height,
                max_threads,
                max_width,
                max_height,
            } => write!(
                f,
                "metal: invalid threadgroup {width}x{height}; limits are \
                 {max_width}x{max_height} and {max_threads} total threads"
            ),
            Self::CommandExecution => {
                write!(f, "metal: command buffer finished in Error status")
            }
            Self::UnsupportedPlatform => {
                write!(f, "metal: GPU compute is only available on macOS")
            }
            Self::UntrustedShader { digest } => write!(
                f,
                "metal: shader {digest} is not present in the trusted SHA-256 allowlist"
            ),
        }
    }
}

impl std::error::Error for MetalError {}

/// Fail-closed policy for caller-provided MSL source.
///
/// This controls which kernels may execute but does not make Metal execution
/// preemptible. Untrusted workloads still require an external worker process.
#[derive(Debug, Clone, Default)]
pub struct ShaderPolicy {
    allowed_sha256: BTreeSet<String>,
}

impl ShaderPolicy {
    pub fn deny_all() -> Self {
        Self::default()
    }

    pub fn from_allowed_sha256(
        digests: impl IntoIterator<Item = String>,
    ) -> Result<Self, MetalError> {
        let mut allowed_sha256 = BTreeSet::new();
        for digest in digests {
            if digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            {
                return Err(MetalError::UntrustedShader { digest });
            }
            allowed_sha256.insert(digest);
        }
        Ok(Self { allowed_sha256 })
    }

    fn authorize(&self, source: &str) -> Result<(), MetalError> {
        let digest = shader_sha256(source);
        if self.allowed_sha256.contains(&digest) {
            Ok(())
        } else {
            Err(MetalError::UntrustedShader { digest })
        }
    }
}

pub fn shader_sha256(source: &str) -> String {
    format!("{:x}", Sha256::digest(source.as_bytes()))
}

// Keeps existing callers that accept an `Into<String>` error source-compatible.
impl From<MetalError> for String {
    fn from(error: MetalError) -> Self {
        error.to_string()
    }
}

#[derive(Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct MatmulRequest {
    a_bytes: u64,
    b_bytes: u64,
    c_elements: usize,
    c_bytes: u64,
    dimensions: [u32; 3],
    grid: [u64; 2],
}

#[derive(Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
struct Run1dRequest {
    input_bytes: Vec<u64>,
    output_bytes: u64,
    grid_width: u64,
}

fn checked_elements(lhs: usize, rhs: usize, expression: &'static str) -> Result<usize, MetalError> {
    lhs.checked_mul(rhs).ok_or(MetalError::DimensionOverflow {
        expression,
        lhs,
        rhs,
    })
}

fn checked_bytes(elements: usize) -> Result<u64, MetalError> {
    let bytes = elements
        .checked_mul(FLOAT_BYTES)
        .ok_or(MetalError::ByteSizeOverflow {
            elements,
            element_size: FLOAT_BYTES,
        })?;
    u64::try_from(bytes).map_err(|_| MetalError::ByteSizeOverflow {
        elements,
        element_size: FLOAT_BYTES,
    })
}

fn checked_dimension(dimension: &'static str, value: usize) -> Result<u32, MetalError> {
    u32::try_from(value).map_err(|_| MetalError::DimensionTooLarge {
        dimension,
        value,
        max: u32::MAX,
    })
}

fn checked_dispatch_width(threads: usize) -> Result<u64, MetalError> {
    u64::try_from(threads).map_err(|_| MetalError::DispatchTooLarge { threads })
}

fn validate_matmul(
    a_len: usize,
    b_len: usize,
    m: usize,
    k: usize,
    n: usize,
) -> Result<MatmulRequest, MetalError> {
    let a_elements = checked_elements(m, k, "m*k")?;
    let b_elements = checked_elements(k, n, "k*n")?;
    let c_elements = checked_elements(m, n, "m*n")?;

    if a_len != a_elements {
        return Err(MetalError::InvalidBufferLength {
            buffer: "lhs",
            actual: a_len,
            expected: a_elements,
        });
    }
    if b_len != b_elements {
        return Err(MetalError::InvalidBufferLength {
            buffer: "rhs",
            actual: b_len,
            expected: b_elements,
        });
    }

    let dimensions = [
        checked_dimension("m", m)?,
        checked_dimension("k", k)?,
        checked_dimension("n", n)?,
    ];
    Ok(MatmulRequest {
        a_bytes: checked_bytes(a_elements)?,
        b_bytes: checked_bytes(b_elements)?,
        c_elements,
        c_bytes: checked_bytes(c_elements)?,
        dimensions,
        grid: [checked_dispatch_width(n)?, checked_dispatch_width(m)?],
    })
}

fn validate_run_1d(
    source: &str,
    fn_name: &str,
    inputs: &[&[f32]],
    out_len: usize,
    threads: usize,
) -> Result<Run1dRequest, MetalError> {
    if source.len() > MAX_MSL_SOURCE_BYTES {
        return Err(MetalError::ResourceLimit {
            resource: "MSL source",
            actual: source.len(),
            limit: MAX_MSL_SOURCE_BYTES,
        });
    }
    if fn_name.is_empty() {
        return Err(MetalError::InvalidFunctionName);
    }
    if fn_name.len() > MAX_FUNCTION_NAME_BYTES {
        return Err(MetalError::ResourceLimit {
            resource: "function name",
            actual: fn_name.len(),
            limit: MAX_FUNCTION_NAME_BYTES,
        });
    }
    if inputs.len() > MAX_INPUT_BUFFERS {
        return Err(MetalError::ResourceLimit {
            resource: "input buffer count",
            actual: inputs.len(),
            limit: MAX_INPUT_BUFFERS,
        });
    }

    let input_bytes = inputs
        .iter()
        .map(|input| checked_bytes(input.len()))
        .collect::<Result<Vec<_>, _>>()?;
    let mut total_input_bytes = 0_u64;
    for (index, &bytes) in input_bytes.iter().enumerate() {
        validate_buffer_limit(format!("input {index}"), bytes, MAX_RAW_BUFFER_BYTES)?;
        total_input_bytes = total_input_bytes
            .checked_add(bytes)
            .ok_or(MetalError::TotalByteSizeOverflow)?;
    }
    validate_buffer_limit(
        "total input payload".into(),
        total_input_bytes,
        MAX_RAW_TOTAL_INPUT_BYTES,
    )?;
    let output_bytes = checked_bytes(out_len)?;
    validate_buffer_limit("output".into(), output_bytes, MAX_RAW_BUFFER_BYTES)?;
    let grid_width = checked_dispatch_width(threads)?;
    if grid_width > MAX_RAW_DISPATCH_THREADS {
        return Err(MetalError::ResourceLimit {
            resource: "dispatch thread count",
            actual: threads,
            limit: usize::try_from(MAX_RAW_DISPATCH_THREADS).unwrap_or(usize::MAX),
        });
    }
    Ok(Run1dRequest {
        input_bytes,
        output_bytes,
        grid_width,
    })
}

fn validate_buffer_limit(buffer: String, bytes: u64, max_bytes: u64) -> Result<(), MetalError> {
    if bytes > max_bytes {
        Err(MetalError::BufferTooLarge {
            buffer,
            bytes,
            max_bytes,
        })
    } else {
        Ok(())
    }
}

fn validate_threadgroup(
    width: u64,
    height: u64,
    max_threads: u64,
    max_width: u64,
    max_height: u64,
) -> Result<(), MetalError> {
    let total = width.checked_mul(height);
    if width == 0
        || height == 0
        || width > max_width
        || height > max_height
        || total.is_none_or(|total| total > max_threads)
    {
        Err(MetalError::InvalidThreadgroup {
            width,
            height,
            max_threads,
            max_width,
            max_height,
        })
    } else {
        Ok(())
    }
}

#[derive(Debug)]
#[cfg(any(test, target_os = "macos"))]
struct BoundedLru<K, V> {
    capacity: usize,
    entries: VecDeque<(K, V)>,
}

#[cfg(any(test, target_os = "macos"))]
impl<K: Eq, V: Clone> BoundedLru<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
        }
    }

    fn get(&mut self, key: &K) -> Option<V> {
        let index = self.entries.iter().position(|(entry, _)| entry == key)?;
        let entry = self.entries.remove(index)?;
        let value = entry.1.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    fn insert(&mut self, key: K, value: V) {
        if self.capacity == 0 {
            return;
        }
        if let Some(index) = self.entries.iter().position(|(entry, _)| entry == &key) {
            self.entries.remove(index);
        } else if self.entries.len() == self.capacity {
            self.entries.pop_front();
        }
        self.entries.push_back((key, value));
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Whether a Metal device is available in this process (always false off macOS).
pub fn metal_available() -> bool {
    imp::metal_available()
}

/// The system default Metal device's name, or an empty string when unavailable.
pub fn metal_device_name() -> String {
    imp::metal_device_name()
}

/// GPU matrix multiply `(m, k) x (k, n) -> (m, n)`, row-major f32.
pub fn gpu_matmul(
    a: &[f32],
    b: &[f32],
    m: usize,
    k: usize,
    n: usize,
) -> Result<Vec<f32>, MetalError> {
    let request = validate_matmul(a.len(), b.len(), m, k, n)?;
    imp::gpu_matmul(a, b, request)
}

/// Compile trusted `source` and dispatch `fn_name` over a one-dimensional grid.
///
/// This compatibility API accepts arbitrary MSL and must only be used with
/// trusted source. Policy-enforced callers should use
/// [`gpu_run_1d_with_policy`].
pub fn gpu_run_1d(
    source: &str,
    fn_name: &str,
    inputs: &[&[f32]],
    out_len: usize,
    threads: usize,
) -> Result<Vec<f32>, MetalError> {
    let request = validate_run_1d(source, fn_name, inputs, out_len, threads)?;
    imp::gpu_run_1d(source, fn_name, inputs, out_len, request)
}

/// Dispatch caller-provided MSL only when its digest is explicitly allowed.
pub fn gpu_run_1d_with_policy(
    policy: &ShaderPolicy,
    source: &str,
    fn_name: &str,
    inputs: &[&[f32]],
    out_len: usize,
    threads: usize,
) -> Result<Vec<f32>, MetalError> {
    policy.authorize(source)?;
    gpu_run_1d(source, fn_name, inputs, out_len, threads)
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use metal::{Device, MTLCommandBufferStatus, MTLResourceOptions, MTLSize};
    use objc::rc::autoreleasepool;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::{Mutex, OnceLock};

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct PipelineOptions {
        fast_math: bool,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct PipelineKey {
        device_registry_id: u64,
        source_hash: u64,
        source: Box<str>,
        function_name: Box<str>,
        options: PipelineOptions,
    }

    impl PipelineKey {
        fn new(
            device_registry_id: u64,
            source: &str,
            function_name: &str,
            options: PipelineOptions,
        ) -> Self {
            let mut hasher = DefaultHasher::new();
            source.hash(&mut hasher);
            Self {
                device_registry_id,
                source_hash: hasher.finish(),
                source: source.into(),
                function_name: function_name.into(),
                options,
            }
        }
    }

    struct MetalState {
        device: Device,
        queue: metal::CommandQueue,
        pipelines: Mutex<BoundedLru<PipelineKey, metal::ComputePipelineState>>,
    }

    static STATE: OnceLock<Option<MetalState>> = OnceLock::new();

    fn state() -> Result<&'static MetalState, MetalError> {
        STATE
            .get_or_init(|| {
                Device::system_default().map(|device| MetalState {
                    queue: device.new_command_queue(),
                    device,
                    pipelines: Mutex::new(BoundedLru::new(PIPELINE_CACHE_CAPACITY)),
                })
            })
            .as_ref()
            .ok_or(MetalError::DeviceUnavailable)
    }

    pub fn metal_available() -> bool {
        state().is_ok()
    }

    pub fn metal_device_name() -> String {
        state()
            .map(|state| state.device.name().to_string())
            .unwrap_or_default()
    }

    fn allocated_bytes(bytes: u64) -> u64 {
        bytes.max(FLOAT_BYTES_U64)
    }

    fn validate_device_buffer(
        state: &MetalState,
        buffer: impl Into<String>,
        bytes: u64,
    ) -> Result<(), MetalError> {
        validate_buffer_limit(
            buffer.into(),
            allocated_bytes(bytes),
            state.device.max_buffer_length(),
        )
    }

    fn upload(
        state: &MetalState,
        name: impl Into<String>,
        data: &[f32],
        byte_len: u64,
    ) -> Result<metal::Buffer, MetalError> {
        validate_device_buffer(state, name, byte_len)?;
        let buffer = state.device.new_buffer(
            allocated_bytes(byte_len),
            MTLResourceOptions::StorageModeShared,
        );
        if !data.is_empty() {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    buffer.contents() as *mut f32,
                    data.len(),
                );
            }
        }
        Ok(buffer)
    }

    fn new_output_buffer(state: &MetalState, byte_len: u64) -> Result<metal::Buffer, MetalError> {
        validate_device_buffer(state, "output", byte_len)?;
        Ok(state.device.new_buffer(
            allocated_bytes(byte_len),
            MTLResourceOptions::StorageModeShared,
        ))
    }

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

    fn make_pipeline(
        state: &MetalState,
        source: &str,
        fn_name: &str,
    ) -> Result<metal::ComputePipelineState, MetalError> {
        let pipeline_options = PipelineOptions::default();
        let key = PipelineKey::new(
            state.device.registry_id(),
            source,
            fn_name,
            pipeline_options,
        );
        if let Some(pipeline) = state
            .pipelines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            return Ok(pipeline);
        }

        // Compile outside the cache lock; concurrent misses may duplicate work but
        // do not serialize unrelated shader compilation.
        let options = metal::CompileOptions::new();
        options.set_fast_math_enabled(pipeline_options.fast_math);
        let library = state
            .device
            .new_library_with_source(source, &options)
            .map_err(MetalError::Compilation)?;
        let function = library.get_function(fn_name, None).map_err(|message| {
            MetalError::FunctionNotFound {
                function: fn_name.to_string(),
                message,
            }
        })?;
        let pipeline = state
            .device
            .new_compute_pipeline_state_with_function(&function)
            .map_err(MetalError::PipelineCreation)?;
        state
            .pipelines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key, pipeline.clone());
        Ok(pipeline)
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
        request: MatmulRequest,
    ) -> Result<Vec<f32>, MetalError> {
        if request.c_elements == 0 {
            return Ok(Vec::new());
        }
        autoreleasepool(|| {
            let state = state()?;
            let pipeline = make_pipeline(state, MATMUL_SRC, "rss_matmul")?;
            let a_buf = upload(state, "lhs", a, request.a_bytes)?;
            let b_buf = upload(state, "rhs", b, request.b_bytes)?;
            let c_buf = new_output_buffer(state, request.c_bytes)?;

            let command_buffer = state.queue.new_command_buffer();
            let encoder = command_buffer.new_compute_command_encoder();
            encoder.set_compute_pipeline_state(&pipeline);
            encoder.set_buffer(0, Some(&a_buf), 0);
            encoder.set_buffer(1, Some(&b_buf), 0);
            encoder.set_buffer(2, Some(&c_buf), 0);
            encoder.set_bytes(
                3,
                FLOAT_BYTES_U64,
                (&request.dimensions[0] as *const u32).cast(),
            );
            encoder.set_bytes(
                4,
                FLOAT_BYTES_U64,
                (&request.dimensions[1] as *const u32).cast(),
            );
            encoder.set_bytes(
                5,
                FLOAT_BYTES_U64,
                (&request.dimensions[2] as *const u32).cast(),
            );

            let grid = MTLSize::new(request.grid[0], request.grid[1], 1);
            let tew = pipeline.thread_execution_width();
            let max_threads = pipeline.max_total_threads_per_threadgroup();
            let tg_h = max_threads.checked_div(tew).unwrap_or(0);
            let device_max = state.device.max_threads_per_threadgroup();
            validate_threadgroup(tew, tg_h, max_threads, device_max.width, device_max.height)?;
            encoder.dispatch_threads(grid, MTLSize::new(tew, tg_h, 1));
            encoder.end_encoding();
            command_buffer.commit();
            command_buffer.wait_until_completed();
            command_buffer_result(command_buffer)?;
            Ok(download(&c_buf, request.c_elements))
        })
    }

    pub fn gpu_run_1d(
        source: &str,
        fn_name: &str,
        inputs: &[&[f32]],
        out_len: usize,
        request: Run1dRequest,
    ) -> Result<Vec<f32>, MetalError> {
        if request.grid_width == 0 {
            return Ok(vec![0.0; out_len]);
        }
        autoreleasepool(|| {
            let state = state()?;
            let pipeline = make_pipeline(state, source, fn_name)?;
            let in_bufs = inputs
                .iter()
                .zip(&request.input_bytes)
                .enumerate()
                .map(|(index, (data, &bytes))| upload(state, format!("input {index}"), data, bytes))
                .collect::<Result<Vec<_>, _>>()?;
            let out_buf = new_output_buffer(state, request.output_bytes)?;

            let command_buffer = state.queue.new_command_buffer();
            let encoder = command_buffer.new_compute_command_encoder();
            encoder.set_compute_pipeline_state(&pipeline);
            for (index, buffer) in in_bufs.iter().enumerate() {
                let binding = u64::try_from(index).map_err(|_| MetalError::ResourceLimit {
                    resource: "buffer binding index",
                    actual: index,
                    limit: MAX_INPUT_BUFFERS,
                })?;
                encoder.set_buffer(binding, Some(buffer), 0);
            }
            let output_binding =
                u64::try_from(in_bufs.len()).map_err(|_| MetalError::ResourceLimit {
                    resource: "buffer binding index",
                    actual: in_bufs.len(),
                    limit: MAX_INPUT_BUFFERS,
                })?;
            encoder.set_buffer(output_binding, Some(&out_buf), 0);

            let tew = pipeline.thread_execution_width();
            let max_threads = pipeline.max_total_threads_per_threadgroup();
            let device_max = state.device.max_threads_per_threadgroup();
            validate_threadgroup(tew, 1, max_threads, device_max.width, device_max.height)?;
            encoder.dispatch_threads(
                MTLSize::new(request.grid_width, 1, 1),
                MTLSize::new(tew, 1, 1),
            );
            encoder.end_encoding();
            command_buffer.commit();
            command_buffer.wait_until_completed();
            command_buffer_result(command_buffer)?;
            Ok(download(&out_buf, out_len))
        })
    }

    fn command_buffer_result(cb: &metal::CommandBufferRef) -> Result<(), MetalError> {
        if cb.status() == MTLCommandBufferStatus::Error {
            Err(MetalError::CommandExecution)
        } else {
            Ok(())
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn metal_available() -> bool {
        false
    }

    pub fn metal_device_name() -> String {
        String::new()
    }

    pub fn gpu_matmul(
        _a: &[f32],
        _b: &[f32],
        _request: MatmulRequest,
    ) -> Result<Vec<f32>, MetalError> {
        Err(MetalError::UnsupportedPlatform)
    }

    pub fn gpu_run_1d(
        _source: &str,
        _fn_name: &str,
        _inputs: &[&[f32]],
        _out_len: usize,
        _request: Run1dRequest,
    ) -> Result<Vec<f32>, MetalError> {
        Err(MetalError::UnsupportedPlatform)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matmul_rejects_dimension_product_overflow() {
        let error = validate_matmul(0, 0, usize::MAX, 2, 1).unwrap_err();
        assert!(matches!(
            error,
            MetalError::DimensionOverflow {
                expression: "m*k",
                ..
            }
        ));
    }

    #[test]
    fn matmul_rejects_dimensions_beyond_kernel_uint() {
        if usize::BITS <= 32 {
            return;
        }
        let too_large = (u32::MAX as usize) + 1;
        let error = validate_matmul(0, 0, too_large, 0, 0).unwrap_err();
        assert_eq!(
            error,
            MetalError::DimensionTooLarge {
                dimension: "m",
                value: too_large,
                max: u32::MAX,
            }
        );
    }

    #[test]
    fn matmul_accepts_empty_dimensions_without_overflow() {
        let request = validate_matmul(0, 0, 0, usize::try_from(u32::MAX).unwrap(), 0).unwrap();
        assert_eq!(request.c_elements, 0);
        assert_eq!(request.c_bytes, 0);
        assert_eq!(request.grid, [0, 0]);
    }

    #[test]
    fn byte_size_checks_usize_boundary() {
        let largest = usize::MAX / FLOAT_BYTES;
        if usize::BITS <= 64 {
            assert_eq!(
                checked_bytes(largest).unwrap(),
                largest as u64 * FLOAT_BYTES as u64
            );
        }
        assert!(matches!(
            checked_bytes(largest + 1),
            Err(MetalError::ByteSizeOverflow { .. })
        ));
    }

    #[test]
    fn buffer_limit_accepts_boundary_and_rejects_next_byte() {
        assert_eq!(validate_buffer_limit("test".into(), 4096, 4096), Ok(()));
        assert_eq!(
            validate_buffer_limit("test".into(), 4097, 4096),
            Err(MetalError::BufferTooLarge {
                buffer: "test".into(),
                bytes: 4097,
                max_bytes: 4096,
            })
        );
    }

    #[test]
    fn threadgroup_limits_accept_boundaries_and_reject_excess() {
        assert_eq!(validate_threadgroup(32, 8, 256, 32, 8), Ok(()));
        assert!(matches!(
            validate_threadgroup(33, 1, 256, 32, 8),
            Err(MetalError::InvalidThreadgroup { .. })
        ));
        assert!(matches!(
            validate_threadgroup(32, 9, 256, 32, 9),
            Err(MetalError::InvalidThreadgroup { .. })
        ));
        assert!(matches!(
            validate_threadgroup(u64::MAX, 2, u64::MAX, u64::MAX, 2),
            Err(MetalError::InvalidThreadgroup { .. })
        ));
    }

    #[test]
    fn raw_dispatch_enforces_source_and_buffer_quotas() {
        let oversized_source = "x".repeat(MAX_MSL_SOURCE_BYTES + 1);
        assert!(matches!(
            validate_run_1d(&oversized_source, "kernel", &[], 0, 0),
            Err(MetalError::ResourceLimit {
                resource: "MSL source",
                ..
            })
        ));

        let empty: &[f32] = &[];
        let inputs = vec![empty; MAX_INPUT_BUFFERS + 1];
        assert!(matches!(
            validate_run_1d("", "kernel", &inputs, 0, 0),
            Err(MetalError::ResourceLimit {
                resource: "input buffer count",
                ..
            })
        ));

        let oversized_output = (MAX_RAW_BUFFER_BYTES as usize / FLOAT_BYTES) + 1;
        assert!(matches!(
            validate_run_1d("", "kernel", &[], oversized_output, 0),
            Err(MetalError::BufferTooLarge { buffer, .. }) if buffer == "output"
        ));

        if usize::BITS > 32 {
            let too_many_threads = (u32::MAX as usize) + 1;
            assert!(matches!(
                validate_run_1d("", "kernel", &[], 0, too_many_threads),
                Err(MetalError::ResourceLimit {
                    resource: "dispatch thread count",
                    ..
                })
            ));
        }
    }

    #[test]
    fn shader_policy_is_fail_closed_and_accepts_exact_digest() {
        let source = "kernel void add() {}";
        let denied = ShaderPolicy::deny_all()
            .authorize(source)
            .expect_err("deny-all policy must reject arbitrary source");
        assert!(matches!(denied, MetalError::UntrustedShader { .. }));

        let policy = ShaderPolicy::from_allowed_sha256([shader_sha256(source)])
            .expect("valid lowercase SHA-256 digest");
        assert_eq!(policy.authorize(source), Ok(()));
        assert!(policy.authorize("kernel void other() {}").is_err());
    }

    #[test]
    fn shader_policy_rejects_noncanonical_digest() {
        assert!(ShaderPolicy::from_allowed_sha256(["ABC".to_string()]).is_err());
    }

    #[test]
    fn bounded_lru_promotes_hits_and_evicts_oldest() {
        let mut cache = BoundedLru::new(2);
        cache.insert("a", 1);
        cache.insert("b", 2);
        assert_eq!(cache.get(&"a"), Some(1));
        cache.insert("c", 3);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"a"), Some(1));
        assert_eq!(cache.get(&"c"), Some(3));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn device_is_present_on_mac() {
        if metal_available() {
            assert!(!metal_device_name().is_empty());
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn gpu_matmul_matches_cpu_and_reuses_pipeline() {
        if !metal_available() {
            return;
        }
        let a = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
        let b = [7.0, 8.0, 9.0, 10.0, 11.0, 12.0];
        for _ in 0..2 {
            let out = gpu_matmul(&a, &b, 2, 3, 2).unwrap();
            let expected = [58.0, 64.0, 139.0, 154.0];
            for (got, expected) in out.iter().zip(expected.iter()) {
                assert!((got - expected).abs() < 1e-3);
            }
        }
    }

    #[cfg(target_os = "macos")]
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
