//! Bounded request/response protocol for isolated RSScript workers.
//!
//! The wire types intentionally do not depend on `rsscript`, `rss-native-abi`,
//! or a backend crate. Hosts and workers convert at their respective trust
//! boundaries, avoiding dependency cycles and accidental sharing of in-process
//! representations.

mod error;
mod framing;
mod types;
mod validation;

pub use error::{ProtocolError, ValidationError};
pub use framing::{
    FRAME_HEADER_BYTES, FrameKind, MAGIC, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, PROTOCOL_VERSION,
    decode_request, decode_response, encode_request, encode_response, read_request, read_response,
    write_request, write_response,
};
pub use types::{
    EvalBackend, EvalRequest, EvalResult, MetalMatmulRequest, MetalRun1dRequest, NativeArtifact,
    NativeBinding, NativeCallRequest, NativeValue, ProgramBundle, ProgramSource, Request,
    RequestOperation, Response, ResponseOutcome, ResponseValue, WorkerError, WorkerErrorCode,
};
pub use validation::{
    MAX_ARGUMENT_BYTES, MAX_ARGUMENT_COUNT, MAX_BINDING_BYTES, MAX_EVAL_TEXT_BYTES,
    MAX_GPU_BUFFER_BYTES, MAX_GPU_INPUT_BUFFERS, MAX_GPU_OUTPUT_BYTES, MAX_GPU_SHADER_BYTES,
    MAX_GPU_TOTAL_INPUT_BYTES, MAX_NATIVE_VALUE_DEPTH, MAX_NATIVE_VALUE_NODES, MAX_PATH_BYTES,
    MAX_SOURCE_BYTES, MAX_SOURCE_FILE_BYTES, MAX_SOURCE_FILES,
};
