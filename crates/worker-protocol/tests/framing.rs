use std::io::Cursor;

use rss_worker_protocol::{
    EvalBackend, EvalRequest, EvalResult, FRAME_HEADER_BYTES, FrameKind, MAGIC, MAX_REQUEST_BYTES,
    NativeValue, PROTOCOL_VERSION, ProgramBundle, ProgramSource, ProtocolError, Request,
    RequestOperation, Response, ResponseOutcome, ResponseValue, decode_request, decode_response,
    encode_request, encode_response, read_request, read_response,
};
use serde_json::{Value, json};

fn eval_request() -> Request {
    Request {
        request_id: 7,
        operation: RequestOperation::Eval(EvalRequest {
            program: ProgramBundle {
                entry: "main.rss".into(),
                sources: vec![ProgramSource {
                    path: "main.rss".into(),
                    source: "fn main() -> Int { 42 }".into(),
                }],
                interfaces: Vec::new(),
                native_bindings: Vec::new(),
            },
            backend: EvalBackend::ReferenceVm,
            args: vec!["one".into(), "two".into()],
            prebuilt: None,
        }),
    }
}

fn response() -> Response {
    Response {
        request_id: 7,
        outcome: ResponseOutcome::Ok(ResponseValue::Eval(EvalResult {
            value: "42".into(),
            display_value: "42".into(),
            native_value: Some(NativeValue::Int(42)),
            stdout: String::new(),
            stderr: String::new(),
        })),
    }
}

fn json_frame(kind: FrameKind, payload: Value) -> Vec<u8> {
    let payload = serde_json::to_vec(&payload).unwrap();
    let mut bytes = Vec::with_capacity(FRAME_HEADER_BYTES + payload.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    bytes.extend_from_slice(&(kind as u16).to_be_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&payload);
    bytes
}

#[test]
fn request_and_response_round_trip() {
    let request = eval_request();
    let request_bytes = encode_request(&request).unwrap();
    assert_eq!(&request_bytes[..4], b"RSSW");
    assert_eq!(decode_request(&request_bytes).unwrap(), request);

    let response = response();
    let response_bytes = encode_response(&response).unwrap();
    assert_eq!(decode_response(&response_bytes).unwrap(), response);
}

#[test]
fn stream_readers_consume_exactly_one_frame() {
    let first = encode_request(&eval_request()).unwrap();
    let mut second_request = eval_request();
    second_request.request_id = 8;
    let second = encode_request(&second_request).unwrap();
    let mut stream = Cursor::new([first, second].concat());

    assert_eq!(read_request(&mut stream).unwrap().request_id, 7);
    assert_eq!(read_request(&mut stream).unwrap().request_id, 8);
}

#[test]
fn rejects_bad_magic_version_and_kind() {
    let bytes = encode_request(&eval_request()).unwrap();

    let mut bad_magic = bytes.clone();
    bad_magic[0] = b'X';
    assert!(matches!(
        decode_request(&bad_magic),
        Err(ProtocolError::BadMagic { .. })
    ));

    let mut bad_version = bytes.clone();
    bad_version[4..6].copy_from_slice(&(PROTOCOL_VERSION + 1).to_be_bytes());
    assert!(matches!(
        decode_request(&bad_version),
        Err(ProtocolError::UnsupportedVersion { .. })
    ));

    let mut unknown_kind = bytes.clone();
    unknown_kind[6..8].copy_from_slice(&99_u16.to_be_bytes());
    assert!(matches!(
        decode_request(&unknown_kind),
        Err(ProtocolError::UnexpectedKind { actual: 99, .. })
    ));

    assert!(matches!(
        read_response(&mut Cursor::new(bytes)),
        Err(ProtocolError::UnexpectedKind {
            expected: FrameKind::Response,
            ..
        })
    ));
}

#[test]
fn rejects_oversized_length_before_reading_payload() {
    let mut header = Vec::from(MAGIC);
    header.extend_from_slice(&PROTOCOL_VERSION.to_be_bytes());
    header.extend_from_slice(&(FrameKind::Request as u16).to_be_bytes());
    header.extend_from_slice(&((MAX_REQUEST_BYTES as u32) + 1).to_be_bytes());

    assert!(matches!(
        decode_request(&header),
        Err(ProtocolError::PayloadTooLarge {
            kind: FrameKind::Request,
            ..
        })
    ));

    header[6..8].copy_from_slice(&(FrameKind::Response as u16).to_be_bytes());
    assert!(matches!(
        decode_response(&header),
        Err(ProtocolError::PayloadTooLarge {
            kind: FrameKind::Response,
            ..
        })
    ));
}

#[test]
fn rejects_truncated_header_and_payload() {
    assert!(matches!(
        decode_request(b"RSSW"),
        Err(ProtocolError::Truncated { section: "header" })
    ));

    let mut bytes = encode_request(&eval_request()).unwrap();
    bytes.pop();
    assert!(matches!(
        decode_request(&bytes),
        Err(ProtocolError::Truncated {
            section: "request payload"
        })
    ));
}

#[test]
fn exact_decoders_reject_trailing_frame_data() {
    let mut bytes = encode_request(&eval_request()).unwrap();
    bytes.extend_from_slice(b"trailing");
    assert!(matches!(
        decode_request(&bytes),
        Err(ProtocolError::TrailingData { bytes: 8 })
    ));
}

#[test]
fn json_decoder_rejects_trailing_payload_data() {
    let mut bytes = encode_request(&eval_request()).unwrap();
    let declared = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
    bytes.extend_from_slice(b"x");
    bytes[8..12].copy_from_slice(&(declared + 1).to_be_bytes());
    assert!(matches!(
        decode_request(&bytes),
        Err(ProtocolError::Deserialize(_))
    ));
}

#[test]
fn rejects_unknown_request_and_nested_operation_fields() {
    let mut top_level = serde_json::to_value(eval_request()).unwrap();
    top_level
        .as_object_mut()
        .unwrap()
        .insert("future".into(), json!(true));
    assert!(matches!(
        decode_request(&json_frame(FrameKind::Request, top_level)),
        Err(ProtocolError::Deserialize(_))
    ));

    let mut nested = serde_json::to_value(eval_request()).unwrap();
    nested["operation"]["payload"]["future"] = json!(true);
    assert!(matches!(
        decode_request(&json_frame(FrameKind::Request, nested)),
        Err(ProtocolError::Deserialize(_))
    ));
}

#[test]
fn rejects_unknown_response_fields() {
    let mut value = serde_json::to_value(response()).unwrap();
    value["outcome"]["payload"]["future"] = json!(true);
    assert!(matches!(
        decode_response(&json_frame(FrameKind::Response, value)),
        Err(ProtocolError::Deserialize(_))
    ));
}

#[test]
fn bounded_serializer_rejects_expansion_past_frame_limit() {
    let source = "\n".repeat(4 * 1024 * 1024);
    let request = Request {
        request_id: 1,
        operation: RequestOperation::Eval(EvalRequest {
            program: ProgramBundle {
                entry: "a.rss".into(),
                sources: vec![
                    ProgramSource {
                        path: "a.rss".into(),
                        source: source.clone(),
                    },
                    ProgramSource {
                        path: "b.rss".into(),
                        source: source.clone(),
                    },
                    ProgramSource {
                        path: "c.rss".into(),
                        source,
                    },
                ],
                interfaces: Vec::new(),
                native_bindings: Vec::new(),
            },
            backend: EvalBackend::ReferenceVm,
            args: Vec::new(),
            prebuilt: None,
        }),
    };

    assert!(matches!(
        encode_request(&request),
        Err(ProtocolError::PayloadTooLarge {
            kind: FrameKind::Request,
            ..
        })
    ));
}
