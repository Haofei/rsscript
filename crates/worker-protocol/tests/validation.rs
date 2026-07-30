use std::collections::BTreeMap;

use rss_worker_protocol::{
    EvalBackend, EvalRequest, EvalResult, MAX_ARGUMENT_BYTES, MAX_GPU_SHADER_BYTES,
    MAX_SOURCE_BYTES, MetalMatmulRequest, MetalRun1dRequest, NativeArtifact, NativeBinding,
    NativeCallRequest, NativeValue, ProgramBundle, ProgramSource, Request, RequestOperation,
    Response, ResponseOutcome, ResponseValue,
};

fn artifact(path: &str) -> NativeArtifact {
    NativeArtifact {
        relative_path: path.into(),
        sha256: "a".repeat(64),
    }
}

fn eval_request() -> Request {
    Request {
        request_id: 1,
        operation: RequestOperation::Eval(EvalRequest {
            program: ProgramBundle {
                entry: "src/main.rss".into(),
                sources: vec![ProgramSource {
                    path: "src/main.rss".into(),
                    source: "fn main() -> Int { 0 }".into(),
                }],
                interfaces: Vec::new(),
                native_bindings: Vec::new(),
            },
            backend: EvalBackend::ReferenceVm,
            args: Vec::new(),
            prebuilt: None,
        }),
    }
}

#[test]
fn validates_nonzero_and_matching_request_ids() {
    let mut request = eval_request();
    request.request_id = 0;
    assert_eq!(
        request.validate().unwrap_err().field(),
        "request.request_id"
    );

    let request = eval_request();
    let response = Response {
        request_id: 2,
        outcome: ResponseOutcome::Ok(ResponseValue::Eval(EvalResult {
            value: "()".into(),
            display_value: "()".into(),
            native_value: Some(NativeValue::Unit),
            stdout: String::new(),
            stderr: String::new(),
        })),
    };
    assert_eq!(
        response.validate_for_request(&request).unwrap_err().field(),
        "response.request_id"
    );
}

#[test]
fn validates_source_inventory_and_aggregate_source_bytes() {
    let mut request = eval_request();
    let RequestOperation::Eval(eval) = &mut request.operation else {
        unreachable!()
    };
    eval.program.sources.push(eval.program.sources[0].clone());
    assert!(
        request
            .validate()
            .unwrap_err()
            .message()
            .contains("duplicates")
    );

    let mut request = eval_request();
    let RequestOperation::Eval(eval) = &mut request.operation else {
        unreachable!()
    };
    eval.program
        .interfaces
        .push(eval.program.sources[0].clone());
    assert!(
        request
            .validate()
            .unwrap_err()
            .message()
            .contains("duplicates")
    );

    let mut request = eval_request();
    let RequestOperation::Eval(eval) = &mut request.operation else {
        unreachable!()
    };
    eval.program.entry = "missing.rss".into();
    assert!(
        request
            .validate()
            .unwrap_err()
            .message()
            .contains("bundled source")
    );

    let mut request = eval_request();
    let RequestOperation::Eval(eval) = &mut request.operation else {
        unreachable!()
    };
    let source = "x".repeat(MAX_SOURCE_BYTES / 4 + 1);
    eval.program.entry = "0.rss".into();
    eval.program.sources = (0..4)
        .map(|index| ProgramSource {
            path: format!("{index}.rss"),
            source: source.clone(),
        })
        .collect();
    assert!(request.validate().is_err());
}

#[test]
fn rejects_duplicate_bindings() {
    let mut request = eval_request();
    let RequestOperation::Eval(eval) = &mut request.operation else {
        unreachable!()
    };
    let binding = NativeBinding {
        binding: "Crypto.hash".into(),
        artifact: artifact("staged/libcrypto.dylib"),
    };
    eval.program.native_bindings = vec![binding.clone(), binding];

    assert!(
        request
            .validate()
            .unwrap_err()
            .message()
            .contains("duplicates")
    );
}

#[test]
fn rejects_unsafe_native_paths_and_bad_digests() {
    for path in [
        "/tmp/plugin.so",
        "../plugin.so",
        "staged/../plugin.so",
        r"C:\plugin.dll",
        r"staged\..\plugin.dll",
        "./plugin.so",
    ] {
        let request = Request {
            request_id: 1,
            operation: RequestOperation::NativeCall(NativeCallRequest {
                library: artifact(path),
                binding: "binding".into(),
                args: Vec::new(),
            }),
        };
        assert!(request.validate().is_err(), "accepted path {path:?}");
    }

    let mut bad_digest = artifact("staged/plugin.so");
    bad_digest.sha256 = "A".repeat(64);
    let request = Request {
        request_id: 1,
        operation: RequestOperation::NativeCall(NativeCallRequest {
            library: bad_digest,
            binding: "binding".into(),
            args: Vec::new(),
        }),
    };
    assert!(request.validate().is_err());
}

#[test]
fn prebuilt_artifact_requires_native_jit() {
    let mut request = eval_request();
    let RequestOperation::Eval(eval) = &mut request.operation else {
        unreachable!()
    };
    eval.prebuilt = Some(artifact("jit/program.bin"));
    assert!(request.validate().is_err());
    let RequestOperation::Eval(eval) = &mut request.operation else {
        unreachable!()
    };
    eval.backend = EvalBackend::NativeJit;
    assert!(request.validate().is_ok());
}

#[test]
fn validates_argument_and_nested_native_value_budgets() {
    let request = Request {
        request_id: 1,
        operation: RequestOperation::NativeCall(NativeCallRequest {
            library: artifact("staged/plugin.so"),
            binding: "binding".into(),
            args: vec![NativeValue::String("x".repeat(MAX_ARGUMENT_BYTES + 1))],
        }),
    };
    assert!(request.validate().is_err());

    let mut nested = NativeValue::Unit;
    for _ in 0..65 {
        nested = NativeValue::List(vec![nested]);
    }
    let request = Request {
        request_id: 1,
        operation: RequestOperation::NativeCall(NativeCallRequest {
            library: artifact("staged/plugin.so"),
            binding: "binding".into(),
            args: vec![nested],
        }),
    };
    assert!(request.validate().is_err());

    let request = Request {
        request_id: 1,
        operation: RequestOperation::NativeCall(NativeCallRequest {
            library: artifact("staged/plugin.so"),
            binding: "binding".into(),
            args: vec![NativeValue::Float(f64::NAN)],
        }),
    };
    assert!(request.validate().is_err());
}

#[test]
fn native_wire_values_cover_native_abi_compatible_shapes() {
    let mut fields = BTreeMap::new();
    fields.insert("bytes".into(), NativeValue::Bytes(vec![1, 2, 3]));
    fields.insert(
        "variant".into(),
        NativeValue::Variant {
            name: "Some".into(),
            fields: BTreeMap::from([("value".into(), NativeValue::Int(4))]),
        },
    );
    let request = Request {
        request_id: 1,
        operation: RequestOperation::NativeCall(NativeCallRequest {
            library: artifact("staged/plugin.so"),
            binding: "binding".into(),
            args: vec![
                NativeValue::Struct {
                    name: "Payload".into(),
                    fields,
                },
                NativeValue::Map(vec![(
                    NativeValue::String("key".into()),
                    NativeValue::Bool(true),
                )]),
                NativeValue::Json(serde_json::json!({"ok": [1, 2, 3]})),
                NativeValue::Native {
                    type_name: "Handle".into(),
                    id: 9,
                },
            ],
        }),
    };
    assert!(request.validate().is_ok());
}

#[test]
fn validates_metal_matmul_shapes_bounds_and_values() {
    let valid = Request {
        request_id: 1,
        operation: RequestOperation::MetalMatmul(MetalMatmulRequest {
            lhs: vec![1.0; 6],
            rhs: vec![1.0; 6],
            m: 2,
            k: 3,
            n: 2,
        }),
    };
    assert!(valid.validate().is_ok());

    let mut wrong_shape = valid.clone();
    let RequestOperation::MetalMatmul(matmul) = &mut wrong_shape.operation else {
        unreachable!()
    };
    matmul.rhs.pop();
    assert!(wrong_shape.validate().is_err());

    let mut non_finite = valid;
    let RequestOperation::MetalMatmul(matmul) = &mut non_finite.operation else {
        unreachable!()
    };
    matmul.lhs[0] = f32::INFINITY;
    assert!(non_finite.validate().is_err());
}

#[test]
fn validates_metal_run_1d_bounds() {
    let valid = Request {
        request_id: 1,
        operation: RequestOperation::MetalRun1d(MetalRun1dRequest {
            source: "kernel void add() {}".into(),
            function: "add".into(),
            inputs: vec![vec![1.0, 2.0]],
            output_len: 2,
            threads: 2,
        }),
    };
    assert!(valid.validate().is_ok());

    let mut no_threads = valid.clone();
    let RequestOperation::MetalRun1d(run) = &mut no_threads.operation else {
        unreachable!()
    };
    run.threads = 0;
    assert!(no_threads.validate().is_err());

    let mut large_source = valid;
    let RequestOperation::MetalRun1d(run) = &mut large_source.operation else {
        unreachable!()
    };
    run.source = "x".repeat(MAX_GPU_SHADER_BYTES + 1);
    assert!(large_source.validate().is_err());

    let mut aggregate_inputs = Request {
        request_id: 1,
        operation: RequestOperation::MetalRun1d(MetalRun1dRequest {
            source: "kernel void add() {}".into(),
            function: "add".into(),
            inputs: vec![vec![0.0; 2 * 1024 * 1024], vec![0.0; 2 * 1024 * 1024]],
            output_len: 1,
            threads: 1,
        }),
    };
    assert!(aggregate_inputs.validate().is_err());
    let RequestOperation::MetalRun1d(run) = &mut aggregate_inputs.operation else {
        unreachable!()
    };
    run.inputs.pop();
    assert!(aggregate_inputs.validate().is_ok());
}

#[test]
fn validates_response_payloads_and_errors() {
    let response = Response {
        request_id: 1,
        outcome: ResponseOutcome::Ok(ResponseValue::MetalRun1d(vec![f32::NAN])),
    };
    assert!(response.validate().is_err());

    let response = Response {
        request_id: 1,
        outcome: ResponseOutcome::Ok(ResponseValue::Eval(EvalResult {
            value: "1".into(),
            display_value: "1".into(),
            native_value: Some(NativeValue::Int(1)),
            stdout: "x".repeat(rss_worker_protocol::MAX_EVAL_TEXT_BYTES + 1),
            stderr: String::new(),
        })),
    };
    assert!(response.validate().is_err());

    let response = Response {
        request_id: 1,
        outcome: ResponseOutcome::Error(rss_worker_protocol::WorkerError {
            code: rss_worker_protocol::WorkerErrorCode::Evaluation,
            message: String::new(),
        }),
    };
    assert!(response.validate().is_err());
}
