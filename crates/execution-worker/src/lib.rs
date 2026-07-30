mod conversion;
mod eval;
mod metal;
mod native;

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use rss_worker_protocol::{
    Request, RequestOperation, Response, ResponseOutcome, ResponseValue, WorkerError,
    WorkerErrorCode,
};

const MAX_FAILURE_MESSAGE_BYTES: usize = 4 * 1024;

#[derive(Debug)]
pub(crate) struct DispatchError {
    code: WorkerErrorCode,
    message: String,
}

impl DispatchError {
    pub(crate) fn new(code: WorkerErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: bounded_message(message.into()),
        }
    }
}

pub fn dispatch(request: Request) -> Response {
    let request_id = request.request_id;
    let outcome = match catch_unwind(AssertUnwindSafe(|| dispatch_inner(request))) {
        Ok(Ok(value)) => ResponseOutcome::Ok(value),
        Ok(Err(error)) => ResponseOutcome::Error(WorkerError {
            code: error.code,
            message: error.message,
        }),
        Err(payload) => ResponseOutcome::Error(WorkerError {
            code: WorkerErrorCode::Internal,
            message: bounded_message(format!(
                "worker dispatch panicked: {}",
                panic_message(payload.as_ref())
            )),
        }),
    };
    Response {
        request_id,
        outcome,
    }
}

fn dispatch_inner(request: Request) -> Result<ResponseValue, DispatchError> {
    request.validate().map_err(|error| {
        DispatchError::new(
            WorkerErrorCode::InvalidRequest,
            format!("invalid request: {error}"),
        )
    })?;

    match request.operation {
        RequestOperation::Eval(request) => eval::execute(request).map(ResponseValue::Eval),
        RequestOperation::NativeCall(request) => {
            native::execute(request).map(ResponseValue::NativeCall)
        }
        RequestOperation::MetalMatmul(request) => {
            metal::matmul(request).map(ResponseValue::MetalMatmul)
        }
        RequestOperation::MetalRun1d(request) => {
            metal::run_1d(request).map(ResponseValue::MetalRun1d)
        }
    }
}

fn bounded_message(mut message: String) -> String {
    if message.is_empty() {
        return "worker operation failed".to_string();
    }
    if message.len() <= MAX_FAILURE_MESSAGE_BYTES {
        return message;
    }
    let mut end = MAX_FAILURE_MESSAGE_BYTES;
    while !message.is_char_boundary(end) {
        end -= 1;
    }
    message.truncate(end);
    message
}

fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

#[cfg(test)]
mod tests {
    use rss_worker_protocol::{
        EvalBackend, EvalRequest, MetalMatmulRequest, NativeArtifact, NativeBinding, ProgramBundle,
        ProgramSource, RequestOperation, ResponseOutcome, WorkerErrorCode,
    };

    use super::*;

    fn eval_request(backend: EvalBackend) -> Request {
        Request {
            request_id: 7,
            operation: RequestOperation::Eval(EvalRequest {
                program: ProgramBundle {
                    entry: "main.rss".to_string(),
                    sources: vec![ProgramSource {
                        path: "main.rss".to_string(),
                        source: "fn main() -> Int { return 6 * 7 }\n".to_string(),
                    }],
                    interfaces: Vec::new(),
                    native_bindings: Vec::new(),
                },
                backend,
                args: Vec::new(),
                prebuilt: None,
            }),
        }
    }

    #[test]
    fn reference_and_native_jit_results_match() {
        let reference = dispatch(eval_request(EvalBackend::ReferenceVm));
        let native = dispatch(eval_request(EvalBackend::NativeJit));
        assert_eq!(reference.outcome, native.outcome);
        assert_eq!(
            reference.outcome,
            ResponseOutcome::Ok(ResponseValue::Eval(rss_worker_protocol::EvalResult {
                value: "42".to_string(),
                display_value: "42".to_string(),
                native_value: Some(rss_worker_protocol::NativeValue::Int(42)),
                stdout: String::new(),
                stderr: String::new(),
            }))
        );
    }

    #[test]
    fn eval_preserves_language_output() {
        let mut request = eval_request(EvalBackend::ReferenceVm);
        let RequestOperation::Eval(eval) = &mut request.operation else {
            unreachable!()
        };
        eval.program.sources[0].source =
            "fn main() -> Int { Log.write(message: \"worker\"); return 7 }\n".to_string();
        let response = dispatch(request);
        assert_eq!(
            response.outcome,
            ResponseOutcome::Ok(ResponseValue::Eval(rss_worker_protocol::EvalResult {
                value: "7".to_string(),
                display_value: "7".to_string(),
                native_value: Some(rss_worker_protocol::NativeValue::Int(7)),
                stdout: "worker\n".to_string(),
                stderr: String::new(),
            }))
        );
    }

    #[test]
    fn eval_rejects_native_bindings() {
        let mut request = eval_request(EvalBackend::ReferenceVm);
        let RequestOperation::Eval(eval) = &mut request.operation else {
            unreachable!()
        };
        eval.program.native_bindings.push(NativeBinding {
            binding: "pkg.call".to_string(),
            artifact: NativeArtifact {
                relative_path: "plugin.dylib".to_string(),
                sha256: "0".repeat(64),
            },
        });
        let response = dispatch(request);
        assert!(matches!(
            response.outcome,
            ResponseOutcome::Error(WorkerError {
                code: WorkerErrorCode::PolicyDenied,
                ..
            })
        ));
    }

    #[test]
    fn eval_rejects_prebuilt_artifacts() {
        let mut request = eval_request(EvalBackend::NativeJit);
        let RequestOperation::Eval(eval) = &mut request.operation else {
            unreachable!()
        };
        eval.prebuilt = Some(NativeArtifact {
            relative_path: "program.bin".to_string(),
            sha256: "0".repeat(64),
        });
        let response = dispatch(request);
        assert!(matches!(
            response.outcome,
            ResponseOutcome::Error(WorkerError {
                code: WorkerErrorCode::PolicyDenied,
                ..
            })
        ));
    }

    #[test]
    fn metal_shape_is_rejected_before_backend_dispatch() {
        let response = dispatch(Request {
            request_id: 8,
            operation: RequestOperation::MetalMatmul(MetalMatmulRequest {
                lhs: vec![1.0],
                rhs: Vec::new(),
                m: 1,
                k: 1,
                n: 1,
            }),
        });
        assert!(matches!(
            response.outcome,
            ResponseOutcome::Error(WorkerError {
                code: WorkerErrorCode::InvalidRequest,
                ..
            })
        ));
    }
}
