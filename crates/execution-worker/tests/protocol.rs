use std::io::Write;
use std::process::{Command, Stdio};

use rss_worker_protocol::{
    MetalMatmulRequest, Request, RequestOperation, ResponseOutcome, decode_response, encode_request,
};

fn run_worker(input: &[u8]) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rss-execution-worker"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn binary_processes_one_protocol_request() {
    let request = Request {
        request_id: 11,
        operation: RequestOperation::MetalMatmul(MetalMatmulRequest {
            lhs: vec![1.0],
            rhs: vec![2.0],
            m: 1,
            k: 1,
            n: 1,
        }),
    };
    let bytes = encode_request(&request).unwrap();
    let output = run_worker(&bytes);
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let response = decode_response(&output.stdout).unwrap();
    assert_eq!(response.request_id, request.request_id);
    assert!(matches!(
        response.outcome,
        ResponseOutcome::Ok(_) | ResponseOutcome::Error(_)
    ));
}

#[test]
fn malformed_frame_emits_no_non_protocol_stdout() {
    let output = run_worker(b"not a worker frame");
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}

#[test]
fn trailing_request_data_is_rejected() {
    let request = Request {
        request_id: 12,
        operation: RequestOperation::MetalMatmul(MetalMatmulRequest {
            lhs: vec![1.0],
            rhs: vec![2.0],
            m: 1,
            k: 1,
            n: 1,
        }),
    };
    let mut bytes = encode_request(&request).unwrap();
    bytes.extend_from_slice(b"second-request");
    let output = run_worker(&bytes);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
