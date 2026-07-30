use rss_worker_protocol::{EvalBackend, EvalRequest, EvalResult, WorkerErrorCode};
use rsscript::{EvalError, VmLimits};

use crate::DispatchError;
use crate::conversion::abi_to_wire;

const VM_STDOUT_BUDGET: usize = 1024 * 1024;

pub(crate) fn execute(request: EvalRequest) -> Result<EvalResult, DispatchError> {
    if !request.program.native_bindings.is_empty() {
        return Err(DispatchError::new(
            WorkerErrorCode::PolicyDenied,
            "eval native bindings are not supported by this worker",
        ));
    }
    if request.prebuilt.is_some() {
        return Err(DispatchError::new(
            WorkerErrorCode::PolicyDenied,
            "prebuilt eval artifacts are not supported by this worker",
        ));
    }

    let source_refs = request
        .program
        .sources
        .iter()
        .map(|source| (source.path.as_str(), source.source.as_str()))
        .collect::<Vec<_>>();
    let interface_refs = request
        .program
        .interfaces
        .iter()
        .map(|source| (source.path.as_str(), source.source.as_str()))
        .collect::<Vec<_>>();
    let validated = rsscript::validate_sources_with_interfaces(&source_refs, &interface_refs)
        .map_err(|diagnostics| {
            evaluation_error(rsscript::format_diagnostics_human(&diagnostics))
        })?;
    let executable = rsscript::reg_vm_compile_validated(&validated).map_err(map_eval_error)?;
    let limits = fixed_limits();
    let output = match request.backend {
        EvalBackend::ReferenceVm => executable.eval_main_with_limits(request.args, limits),
        EvalBackend::NativeJit => executable
            .eval_main_with_args_native_with_limits(request.args, limits)
            .map(|(output, _stats)| output),
    }
    .map_err(map_eval_error)?;

    Ok(EvalResult {
        value: output.value,
        display_value: output.display_value,
        native_value: output.native_value.map(abi_to_wire),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

fn fixed_limits() -> VmLimits {
    VmLimits {
        stdout_budget: Some(VM_STDOUT_BUDGET),
        // Arming step/memory/cancel/host-call budgets forces the VM to bypass
        // Cranelift. CPU and memory termination belong to the process guard.
        ..VmLimits::default()
    }
}

fn map_eval_error(error: EvalError) -> DispatchError {
    match error {
        EvalError::Diagnostics(diagnostics) => {
            evaluation_error(rsscript::format_diagnostics_human(&diagnostics))
        }
        EvalError::Runtime(message) => evaluation_error(message),
    }
}

fn evaluation_error(message: impl Into<String>) -> DispatchError {
    DispatchError::new(WorkerErrorCode::Evaluation, message)
}
