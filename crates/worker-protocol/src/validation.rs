use std::collections::BTreeSet;

use crate::{
    EvalBackend, EvalRequest, MetalMatmulRequest, MetalRun1dRequest, NativeArtifact, NativeBinding,
    NativeCallRequest, NativeValue, ProgramBundle, Request, RequestOperation, Response,
    ResponseOutcome, ResponseValue, ValidationError,
};

pub const MAX_SOURCE_FILES: usize = 4_096;
pub const MAX_SOURCE_FILE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_SOURCE_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_EVAL_TEXT_BYTES: usize = 1024 * 1024;
pub const MAX_ARGUMENT_COUNT: usize = 4_096;
pub const MAX_ARGUMENT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PATH_BYTES: usize = 4_096;
pub const MAX_BINDING_BYTES: usize = 1_024;
pub const MAX_NATIVE_VALUE_DEPTH: usize = 64;
pub const MAX_NATIVE_VALUE_NODES: usize = 65_536;
pub const MAX_GPU_SHADER_BYTES: usize = 1024 * 1024;
pub const MAX_GPU_INPUT_BUFFERS: usize = 30;
pub const MAX_GPU_BUFFER_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_GPU_TOTAL_INPUT_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_GPU_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

const FLOAT_BYTES: usize = size_of::<f32>();
const MAX_FUNCTION_BYTES: usize = 256;
const MAX_ERROR_MESSAGE_BYTES: usize = 64 * 1024;

pub(crate) fn validate_request(request: &Request) -> Result<(), ValidationError> {
    validate_request_id(request.request_id, "request.request_id")?;
    match &request.operation {
        RequestOperation::Eval(eval) => validate_eval(eval),
        RequestOperation::NativeCall(call) => validate_native_call(call),
        RequestOperation::MetalMatmul(matmul) => validate_matmul(matmul),
        RequestOperation::MetalRun1d(run) => validate_run_1d(run),
    }
}

pub(crate) fn validate_response(response: &Response) -> Result<(), ValidationError> {
    validate_request_id(response.request_id, "response.request_id")?;
    match &response.outcome {
        ResponseOutcome::Ok(value) => validate_response_value(value),
        ResponseOutcome::Error(error) => {
            if error.message.is_empty() {
                return Err(invalid("response.outcome.message", "must not be empty"));
            }
            limit(
                "response.outcome.message",
                error.message.len(),
                MAX_ERROR_MESSAGE_BYTES,
            )
        }
    }
}

fn validate_request_id(id: u64, field: &str) -> Result<(), ValidationError> {
    if id == 0 {
        Err(invalid(field, "must be nonzero"))
    } else {
        Ok(())
    }
}

fn validate_eval(eval: &EvalRequest) -> Result<(), ValidationError> {
    validate_program(&eval.program)?;
    validate_string_args(&eval.args, "operation.args")?;
    if eval.backend == EvalBackend::ReferenceVm && eval.prebuilt.is_some() {
        return Err(invalid(
            "operation.prebuilt",
            "is only valid for the native_jit backend",
        ));
    }
    if let Some(artifact) = &eval.prebuilt {
        validate_artifact(artifact, "operation.prebuilt")?;
    }
    Ok(())
}

fn validate_program(program: &ProgramBundle) -> Result<(), ValidationError> {
    if program.sources.is_empty() {
        return Err(invalid("operation.program.sources", "must not be empty"));
    }
    let source_count = checked_add(
        "operation.program.sources",
        program.sources.len(),
        program.interfaces.len(),
    )?;
    limit("operation.program.sources", source_count, MAX_SOURCE_FILES)?;
    validate_relative_path(&program.entry, "operation.program.entry")?;

    let mut paths = BTreeSet::new();
    let mut source_bytes = 0_usize;
    for (index, source) in program
        .sources
        .iter()
        .chain(program.interfaces.iter())
        .enumerate()
    {
        let path_field = format!("operation.program.sources[{index}].path");
        validate_relative_path(&source.path, &path_field)?;
        if !paths.insert(source.path.as_str()) {
            return Err(invalid(path_field, "duplicates another source path"));
        }
        limit(
            format!("operation.program.sources[{index}].source"),
            source.source.len(),
            MAX_SOURCE_FILE_BYTES,
        )?;
        source_bytes = checked_add(
            "operation.program.sources",
            source_bytes,
            source.source.len(),
        )?;
        limit("operation.program.sources", source_bytes, MAX_SOURCE_BYTES)?;
    }
    if !paths.contains(program.entry.as_str()) {
        return Err(invalid(
            "operation.program.entry",
            "must name one of the bundled source paths",
        ));
    }
    validate_bindings(&program.native_bindings)
}

fn validate_bindings(bindings: &[NativeBinding]) -> Result<(), ValidationError> {
    let mut names = BTreeSet::new();
    for (index, binding) in bindings.iter().enumerate() {
        let field = format!("operation.program.native_bindings[{index}].binding");
        validate_binding(&binding.binding, &field)?;
        if !names.insert(binding.binding.as_str()) {
            return Err(invalid(field, "duplicates another native binding"));
        }
        validate_artifact(
            &binding.artifact,
            &format!("operation.program.native_bindings[{index}].artifact"),
        )?;
    }
    Ok(())
}

fn validate_native_call(call: &NativeCallRequest) -> Result<(), ValidationError> {
    validate_artifact(&call.library, "operation.library")?;
    validate_binding(&call.binding, "operation.binding")?;
    limit("operation.args", call.args.len(), MAX_ARGUMENT_COUNT)?;
    let mut budget = NativeBudget::default();
    for (index, value) in call.args.iter().enumerate() {
        validate_native_value(value, &format!("operation.args[{index}]"), 1, &mut budget)?;
    }
    Ok(())
}

fn validate_artifact(artifact: &NativeArtifact, field: &str) -> Result<(), ValidationError> {
    validate_relative_path(&artifact.relative_path, &format!("{field}.relative_path"))?;
    if artifact.sha256.len() != 64
        || !artifact
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            format!("{field}.sha256"),
            "must be a 64-character lowercase hexadecimal SHA-256 digest",
        ));
    }
    Ok(())
}

fn validate_binding(binding: &str, field: &str) -> Result<(), ValidationError> {
    if binding.is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    limit(field, binding.len(), MAX_BINDING_BYTES)?;
    if binding.contains('\0') || binding.chars().any(char::is_control) {
        return Err(invalid(field, "must not contain control characters"));
    }
    Ok(())
}

fn validate_string_args(args: &[String], field: &str) -> Result<(), ValidationError> {
    limit(field, args.len(), MAX_ARGUMENT_COUNT)?;
    let mut bytes = 0_usize;
    for arg in args {
        bytes = checked_add(field, bytes, arg.len())?;
        limit(field, bytes, MAX_ARGUMENT_BYTES)?;
    }
    Ok(())
}

fn validate_matmul(request: &MetalMatmulRequest) -> Result<(), ValidationError> {
    let lhs_len = checked_product(
        "operation.metal_matmul.m*k",
        request.m as usize,
        request.k as usize,
    )?;
    let rhs_len = checked_product(
        "operation.metal_matmul.k*n",
        request.k as usize,
        request.n as usize,
    )?;
    let output_len = checked_product(
        "operation.metal_matmul.m*n",
        request.m as usize,
        request.n as usize,
    )?;
    if request.lhs.len() != lhs_len {
        return Err(invalid(
            "operation.lhs",
            format!("has {} elements, expected {lhs_len}", request.lhs.len()),
        ));
    }
    if request.rhs.len() != rhs_len {
        return Err(invalid(
            "operation.rhs",
            format!("has {} elements, expected {rhs_len}", request.rhs.len()),
        ));
    }
    validate_float_slice(&request.lhs, "operation.lhs")?;
    validate_float_slice(&request.rhs, "operation.rhs")?;
    validate_gpu_buffer("operation.lhs", lhs_len, MAX_GPU_BUFFER_BYTES)?;
    validate_gpu_buffer("operation.rhs", rhs_len, MAX_GPU_BUFFER_BYTES)?;
    validate_gpu_buffer("operation.output", output_len, MAX_GPU_OUTPUT_BYTES)?;
    let total = checked_add("operation.inputs", lhs_len, rhs_len)?;
    validate_gpu_buffer("operation.inputs", total, MAX_GPU_TOTAL_INPUT_BYTES)
}

fn validate_run_1d(request: &MetalRun1dRequest) -> Result<(), ValidationError> {
    if request.source.is_empty() {
        return Err(invalid("operation.source", "must not be empty"));
    }
    limit(
        "operation.source",
        request.source.len(),
        MAX_GPU_SHADER_BYTES,
    )?;
    if request.function.is_empty() {
        return Err(invalid("operation.function", "must not be empty"));
    }
    limit(
        "operation.function",
        request.function.len(),
        MAX_FUNCTION_BYTES,
    )?;
    if request.function.contains('\0') || request.function.chars().any(char::is_control) {
        return Err(invalid(
            "operation.function",
            "must not contain control characters",
        ));
    }
    limit(
        "operation.inputs",
        request.inputs.len(),
        MAX_GPU_INPUT_BUFFERS,
    )?;
    if request.threads == 0 {
        return Err(invalid("operation.threads", "must be nonzero"));
    }

    let mut total_elements = 0_usize;
    for (index, input) in request.inputs.iter().enumerate() {
        validate_float_slice(input, &format!("operation.inputs[{index}]"))?;
        validate_gpu_buffer(
            &format!("operation.inputs[{index}]"),
            input.len(),
            MAX_GPU_BUFFER_BYTES,
        )?;
        total_elements = checked_add("operation.inputs", total_elements, input.len())?;
    }
    validate_gpu_buffer(
        "operation.inputs",
        total_elements,
        MAX_GPU_TOTAL_INPUT_BYTES,
    )?;
    validate_gpu_buffer(
        "operation.output_len",
        request.output_len as usize,
        MAX_GPU_OUTPUT_BYTES,
    )
}

fn validate_response_value(value: &ResponseValue) -> Result<(), ValidationError> {
    match value {
        ResponseValue::Eval(result) => {
            limit(
                "response.outcome.value.value",
                result.value.len(),
                MAX_EVAL_TEXT_BYTES,
            )?;
            limit(
                "response.outcome.value.display_value",
                result.display_value.len(),
                MAX_EVAL_TEXT_BYTES,
            )?;
            limit(
                "response.outcome.value.stdout",
                result.stdout.len(),
                MAX_EVAL_TEXT_BYTES,
            )?;
            limit(
                "response.outcome.value.stderr",
                result.stderr.len(),
                MAX_EVAL_TEXT_BYTES,
            )?;
            if let Some(value) = &result.native_value {
                validate_native_value(
                    value,
                    "response.outcome.value.native_value",
                    1,
                    &mut NativeBudget::default(),
                )?;
            }
            Ok(())
        }
        ResponseValue::NativeCall(value) => validate_native_value(
            value,
            "response.outcome.value",
            1,
            &mut NativeBudget::default(),
        ),
        ResponseValue::MetalMatmul(values) | ResponseValue::MetalRun1d(values) => {
            validate_float_slice(values, "response.outcome.value")?;
            validate_gpu_buffer("response.outcome.value", values.len(), MAX_GPU_OUTPUT_BYTES)
        }
    }
}

#[derive(Default)]
struct NativeBudget {
    nodes: usize,
    bytes: usize,
}

fn validate_native_value(
    value: &NativeValue,
    field: &str,
    depth: usize,
    budget: &mut NativeBudget,
) -> Result<(), ValidationError> {
    if depth > MAX_NATIVE_VALUE_DEPTH {
        return Err(invalid(
            field,
            format!("nesting exceeds depth limit {MAX_NATIVE_VALUE_DEPTH}"),
        ));
    }
    budget.nodes = checked_add(field, budget.nodes, 1)?;
    limit(field, budget.nodes, MAX_NATIVE_VALUE_NODES)?;

    match value {
        NativeValue::Float(value) if !value.is_finite() => {
            return Err(invalid(field, "float must be finite"));
        }
        NativeValue::String(value) => add_native_bytes(field, value.len(), budget)?,
        NativeValue::Bytes(value) => add_native_bytes(field, value.len(), budget)?,
        NativeValue::List(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_native_value(value, &format!("{field}[{index}]"), depth + 1, budget)?;
            }
        }
        NativeValue::Map(entries) => {
            for (index, (key, value)) in entries.iter().enumerate() {
                validate_native_value(key, &format!("{field}[{index}].key"), depth + 1, budget)?;
                validate_native_value(
                    value,
                    &format!("{field}[{index}].value"),
                    depth + 1,
                    budget,
                )?;
            }
        }
        NativeValue::Json(value) => validate_json_value(value, field, depth, budget)?,
        NativeValue::Struct { name, fields } | NativeValue::Variant { name, fields } => {
            add_native_bytes(field, name.len(), budget)?;
            for (name, value) in fields {
                add_native_bytes(field, name.len(), budget)?;
                validate_native_value(value, &format!("{field}.{name}"), depth + 1, budget)?;
            }
        }
        NativeValue::Native { type_name, .. } => {
            add_native_bytes(field, type_name.len(), budget)?;
        }
        NativeValue::Unit
        | NativeValue::Int(_)
        | NativeValue::Float(_)
        | NativeValue::Bool(_)
        | NativeValue::Char(_) => {}
    }
    Ok(())
}

fn validate_json_value(
    value: &serde_json::Value,
    field: &str,
    depth: usize,
    budget: &mut NativeBudget,
) -> Result<(), ValidationError> {
    if depth > MAX_NATIVE_VALUE_DEPTH {
        return Err(invalid(
            field,
            format!("nesting exceeds depth limit {MAX_NATIVE_VALUE_DEPTH}"),
        ));
    }
    budget.nodes = checked_add(field, budget.nodes, 1)?;
    limit(field, budget.nodes, MAX_NATIVE_VALUE_NODES)?;
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) => {}
        serde_json::Value::Number(number) => {
            add_native_bytes(field, number.to_string().len(), budget)?;
        }
        serde_json::Value::String(string) => add_native_bytes(field, string.len(), budget)?,
        serde_json::Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                validate_json_value(value, &format!("{field}[{index}]"), depth + 1, budget)?;
            }
        }
        serde_json::Value::Object(values) => {
            for (name, value) in values {
                add_native_bytes(field, name.len(), budget)?;
                validate_json_value(value, &format!("{field}.{name}"), depth + 1, budget)?;
            }
        }
    }
    Ok(())
}

fn add_native_bytes(
    field: &str,
    bytes: usize,
    budget: &mut NativeBudget,
) -> Result<(), ValidationError> {
    budget.bytes = checked_add(field, budget.bytes, bytes)?;
    limit(field, budget.bytes, MAX_ARGUMENT_BYTES)
}

fn validate_float_slice(values: &[f32], field: &str) -> Result<(), ValidationError> {
    if values.iter().any(|value| !value.is_finite()) {
        Err(invalid(field, "contains a non-finite float"))
    } else {
        Ok(())
    }
}

fn validate_gpu_buffer(
    field: &str,
    elements: usize,
    max_bytes: usize,
) -> Result<(), ValidationError> {
    let bytes = checked_product(field, elements, FLOAT_BYTES)?;
    limit(field, bytes, max_bytes)
}

fn validate_relative_path(path: &str, field: &str) -> Result<(), ValidationError> {
    if path.is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    limit(field, path.len(), MAX_PATH_BYTES)?;
    if path.contains('\0') || path.starts_with('/') || path.starts_with('\\') {
        return Err(invalid(field, "must be a clean relative path"));
    }
    if path.len() >= 2 && path.as_bytes()[1] == b':' && path.as_bytes()[0].is_ascii_alphabetic() {
        return Err(invalid(field, "must not use a Windows drive prefix"));
    }
    if path
        .split(['/', '\\'])
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(invalid(
            field,
            "must not contain empty, current-directory, or parent-directory components",
        ));
    }
    Ok(())
}

fn checked_product(field: &str, lhs: usize, rhs: usize) -> Result<usize, ValidationError> {
    lhs.checked_mul(rhs)
        .ok_or_else(|| invalid(field, "size calculation overflowed"))
}

fn checked_add(field: &str, lhs: usize, rhs: usize) -> Result<usize, ValidationError> {
    lhs.checked_add(rhs)
        .ok_or_else(|| invalid(field, "size calculation overflowed"))
}

fn limit(field: impl Into<String>, actual: usize, maximum: usize) -> Result<(), ValidationError> {
    if actual > maximum {
        Err(invalid(
            field,
            format!("size/count {actual} exceeds limit {maximum}"),
        ))
    } else {
        Ok(())
    }
}

fn invalid(field: impl Into<String>, message: impl Into<String>) -> ValidationError {
    ValidationError::new(field, message)
}
