use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ValidationError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub request_id: u64,
    pub operation: RequestOperation,
}

impl Request {
    pub fn validate(&self) -> Result<(), ValidationError> {
        crate::validation::validate_request(self)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RequestOperation {
    Eval(EvalRequest),
    NativeCall(NativeCallRequest),
    MetalMatmul(MetalMatmulRequest),
    MetalRun1d(MetalRun1dRequest),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalRequest {
    pub program: ProgramBundle,
    pub backend: EvalBackend,
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prebuilt: Option<NativeArtifact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvalResult {
    pub value: String,
    pub display_value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_value: Option<NativeValue>,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalBackend {
    ReferenceVm,
    NativeJit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramBundle {
    pub entry: String,
    pub sources: Vec<ProgramSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub interfaces: Vec<ProgramSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_bindings: Vec<NativeBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProgramSource {
    pub path: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeArtifact {
    pub relative_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeBinding {
    pub binding: String,
    pub artifact: NativeArtifact,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCallRequest {
    pub library: NativeArtifact,
    pub binding: String,
    pub args: Vec<NativeValue>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum NativeValue {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    Bytes(Vec<u8>),
    List(Vec<NativeValue>),
    Map(Vec<(NativeValue, NativeValue)>),
    Json(serde_json::Value),
    Struct {
        name: String,
        fields: BTreeMap<String, NativeValue>,
    },
    Variant {
        name: String,
        fields: BTreeMap<String, NativeValue>,
    },
    Native {
        type_name: String,
        id: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetalMatmulRequest {
    pub lhs: Vec<f32>,
    pub rhs: Vec<f32>,
    pub m: u32,
    pub k: u32,
    pub n: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetalRun1dRequest {
    pub source: String,
    pub function: String,
    pub inputs: Vec<Vec<f32>>,
    pub output_len: u32,
    pub threads: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub request_id: u64,
    pub outcome: ResponseOutcome,
}

impl Response {
    pub fn validate(&self) -> Result<(), ValidationError> {
        crate::validation::validate_response(self)
    }

    pub fn validate_for_request(&self, request: &Request) -> Result<(), ValidationError> {
        self.validate()?;
        if self.request_id != request.request_id {
            return Err(ValidationError::new(
                "response.request_id",
                format!(
                    "does not match request ID {} (got {})",
                    request.request_id, self.request_id
                ),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ResponseOutcome {
    Ok(ResponseValue),
    Error(WorkerError),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ResponseValue {
    Eval(EvalResult),
    NativeCall(NativeValue),
    MetalMatmul(Vec<f32>),
    MetalRun1d(Vec<f32>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerError {
    pub code: WorkerErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerErrorCode {
    InvalidRequest,
    PolicyDenied,
    ResourceLimit,
    Evaluation,
    Native,
    Gpu,
    Internal,
}
