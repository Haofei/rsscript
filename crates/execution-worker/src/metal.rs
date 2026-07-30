use rss_worker_protocol::{MetalMatmulRequest, MetalRun1dRequest, WorkerErrorCode};

use crate::DispatchError;

pub(crate) fn matmul(request: MetalMatmulRequest) -> Result<Vec<f32>, DispatchError> {
    rss_metal_compute::gpu_matmul(
        &request.lhs,
        &request.rhs,
        request.m as usize,
        request.k as usize,
        request.n as usize,
    )
    .map_err(gpu_error)
}

pub(crate) fn run_1d(request: MetalRun1dRequest) -> Result<Vec<f32>, DispatchError> {
    // This binary is a killable sandbox boundary. Scope dynamic-source authority
    // to the exact source digest for this one request.
    let digest = rss_metal_compute::shader_sha256(&request.source);
    let policy =
        rss_metal_compute::ShaderPolicy::from_allowed_sha256([digest]).map_err(gpu_error)?;
    let inputs = request.inputs.iter().map(Vec::as_slice).collect::<Vec<_>>();
    rss_metal_compute::gpu_run_1d_with_policy(
        &policy,
        &request.source,
        &request.function,
        &inputs,
        request.output_len as usize,
        request.threads as usize,
    )
    .map_err(gpu_error)
}

fn gpu_error(error: rss_metal_compute::MetalError) -> DispatchError {
    let code = match error {
        rss_metal_compute::MetalError::ResourceLimit { .. }
        | rss_metal_compute::MetalError::BufferTooLarge { .. }
        | rss_metal_compute::MetalError::DispatchTooLarge { .. }
        | rss_metal_compute::MetalError::DimensionTooLarge { .. }
        | rss_metal_compute::MetalError::DimensionOverflow { .. }
        | rss_metal_compute::MetalError::ByteSizeOverflow { .. }
        | rss_metal_compute::MetalError::TotalByteSizeOverflow => WorkerErrorCode::ResourceLimit,
        _ => WorkerErrorCode::Gpu,
    };
    DispatchError::new(code, error.to_string())
}
