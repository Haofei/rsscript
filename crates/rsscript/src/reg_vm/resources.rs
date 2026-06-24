//! Resource-value constructor free functions and their supporting data structs,
//! split out of `reg_vm/mod.rs` (channels/senders/receivers/pools/instants/
//! configs/requests/responses/http/ws/db/process/file/directory/image/tempdir/
//! stream builders). The VM core calls these via `use resources::*`.

use super::*;

pub(super) fn channel_value(id: i64, capacity: i64, receiver_taken: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("id".to_string(), VmValue::Int(id)),
        ("capacity".to_string(), VmValue::Int(capacity)),
        ("receiver_taken".to_string(), VmValue::Bool(receiver_taken)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Channel"), fields)))
}

#[derive(Debug, Clone)]
pub(super) struct VmSender {
    pub(super) channel_id: i64,
    pub(super) closed: bool,
}

/// A spawned-task handle (`Task<T>`), carried as a `Native` so it flows through
/// bindings/structs like any value; `await` recognises it via [`as_task_handle`].
pub(super) fn task_handle_value(task: TaskId) -> VmValue {
    VmValue::Native(Rc::new(VmNative {
        type_name: Rc::from("Task"),
        id: task as i64,
    }))
}

pub(super) fn sender_value(channel_id: i64, closed: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("channel_id".to_string(), VmValue::Int(channel_id)),
        ("closed".to_string(), VmValue::Bool(closed)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Sender"), fields)))
}

#[derive(Debug, Clone)]
pub(super) struct VmReceiver {
    pub(super) channel_id: i64,
    pub(super) closed: bool,
}

pub(super) fn receiver_value(channel_id: i64, closed: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("channel_id".to_string(), VmValue::Int(channel_id)),
        ("closed".to_string(), VmValue::Bool(closed)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Receiver"), fields)))
}

#[derive(Debug, Clone)]
pub(super) struct VmResourcePoolState {
    pub(super) id: i64,
}

pub(super) fn resource_pool_value(id: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("id".to_string(), VmValue::Int(id))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ResourcePool"),
        fields,
    )))
}

pub(super) fn pool_stats_value(capacity: i64, created: i64, available: i64, in_use: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("capacity".to_string(), VmValue::Int(capacity)),
        ("created".to_string(), VmValue::Int(created)),
        ("available".to_string(), VmValue::Int(available)),
        ("in_use".to_string(), VmValue::Int(in_use)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("PoolStats"), fields)))
}

pub(super) fn pool_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("PoolError"), fields)))
}

pub(super) fn instant_value(unix_ms: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("unix_ms".to_string(), VmValue::Int(unix_ms))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Instant"), fields)))
}

pub(super) fn deadline_value(unix_ms: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("unix_ms".to_string(), VmValue::Int(unix_ms))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Deadline"), fields)))
}

pub(super) fn counter_value(value: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("value".to_string(), VmValue::Int(value))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Counter"), fields)))
}

pub(super) fn config_value(name: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("name".to_string(), VmValue::string(name.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ConfigValue"),
        fields,
    )))
}

pub(super) fn config_rules_value(name: impl Into<String>, rule_count: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("name".to_string(), VmValue::string(name.into())),
        ("rule_count".to_string(), VmValue::Int(rule_count)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Config"), fields)))
}

pub(super) fn rule_value(name: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("name".to_string(), VmValue::string(name.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Rule"), fields)))
}

pub(super) fn environment_value(has_parent: bool, has_function: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("has_parent".to_string(), VmValue::Bool(has_parent)),
        ("has_function".to_string(), VmValue::Bool(has_function)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("Environment"),
        fields,
    )))
}

pub(super) fn function_object_value(has_closure: bool) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("has_closure".to_string(), VmValue::Bool(has_closure))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("FunctionObject"),
        fields,
    )))
}

pub(super) fn config_store_value(name: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("name".to_string(), VmValue::string(name.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ConfigStore"),
        fields,
    )))
}

pub(super) fn global_config_value(rule_count: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("rule_count".to_string(), VmValue::Int(rule_count))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("GlobalConfig"),
        fields,
    )))
}

pub(super) fn request_value(path: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("path".to_string(), VmValue::string(path.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Request"), fields)))
}

pub(super) fn response_value(status: i64, body: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("status".to_string(), VmValue::Int(status)),
        ("body".to_string(), VmValue::string(body.into())),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Response"), fields)))
}

pub(super) fn http_request_value(
    method: impl Into<String>,
    url: impl Into<String>,
    body: impl Into<String>,
    timeout_ms: i64,
    attempts: i64,
    backoff_ms: i64,
    header_count: i64,
) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("method".to_string(), VmValue::string(method.into())),
        ("url".to_string(), VmValue::string(url.into())),
        ("body".to_string(), VmValue::string(body.into())),
        ("timeout_ms".to_string(), VmValue::Int(timeout_ms)),
        ("attempts".to_string(), VmValue::Int(attempts)),
        ("backoff_ms".to_string(), VmValue::Int(backoff_ms)),
        ("header_count".to_string(), VmValue::Int(header_count)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("HttpRequest"),
        fields,
    )))
}

pub(super) struct WebSocketFrame {
    pub(super) opcode: u8,
    pub(super) payload: Vec<u8>,
}

pub(super) fn db_connection_value(url: impl Into<String>, queries: Vec<String>) -> VmValue {
    let mut fields: Vec<(String, VmValue)> = vec![("url".to_string(), VmValue::string(url.into()))];
    fields.push((
        "queries".to_string(),
        VmValue::List(Rc::new(RefCell::new(
            queries.into_iter().map(VmValue::string).collect(),
        ))),
    ));
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("DbConnection"),
        fields,
    )))
}

pub(super) fn db_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("DbError"), fields)))
}

#[derive(Debug, Clone)]
pub(super) struct VmProcessOutput {
    pub(super) status: i64,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) merged: String,
    pub(super) truncated: bool,
}

#[derive(Debug, Clone)]
pub(super) struct VmProcessRequest {
    pub(super) command: String,
    pub(super) args: Vec<String>,
    pub(super) cwd: Option<PathBuf>,
    pub(super) stdin: Option<String>,
    pub(super) env: Vec<(String, String)>,
    /// Process deadline in ms; enforced by `process_run_request` (kills the child
    /// past the deadline), matching the compiled runtime.
    pub(super) timeout_ms: i64,
    pub(super) merge_stderr: bool,
    pub(super) output_cap_bytes: i64,
}

pub(super) fn process_output_value(output: VmProcessOutput) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("status".to_string(), VmValue::Int(output.status)),
        ("stdout".to_string(), VmValue::string(output.stdout)),
        ("stderr".to_string(), VmValue::string(output.stderr)),
        ("merged".to_string(), VmValue::string(output.merged)),
        ("truncated".to_string(), VmValue::Bool(output.truncated)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ProcessOutput"),
        fields,
    )))
}

pub(super) fn process_event_value(kind: &str, data: &str, status: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("kind".to_string(), VmValue::string(kind)),
        ("data".to_string(), VmValue::string(data)),
        ("status".to_string(), VmValue::Int(status)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ProcessEvent"),
        fields,
    )))
}

pub(super) fn file_value(path: impl Into<String>, mode: impl Into<String>, cursor: u64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("path".to_string(), VmValue::string(path.into())),
        ("mode".to_string(), VmValue::string(mode.into())),
        ("cursor".to_string(), VmValue::Int(cursor as i64)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("File"), fields)))
}

pub(super) fn file_bytes_stream_value(path: &str, chunk_size: i64) -> Result<VmValue, String> {
    let bytes =
        std::fs::read(path).map_err(|error| format!("file byte stream open failed: {error}"))?;
    let chunk_size = chunk_size.max(1) as usize;
    Ok(stream_value(
        bytes
            .chunks(chunk_size)
            .map(|chunk| VmValue::Bytes(Rc::new(chunk.to_vec())))
            .collect(),
    ))
}

pub(super) fn file_metadata_value(metadata: std::fs::Metadata) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("is_file".to_string(), VmValue::Bool(metadata.is_file())),
        ("is_dir".to_string(), VmValue::Bool(metadata.is_dir())),
        ("len".to_string(), VmValue::Int(metadata.len() as i64)),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("FileMetadata"),
        fields,
    )))
}

fn tempdir_value(path: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("path".to_string(), VmValue::string(path.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("TempDir"), fields)))
}

pub(super) fn tempdir_new_value(parent: PathBuf) -> Result<VmValue, VmValue> {
    let seed = clock_system_unix_ms();
    for attempt in 0..100 {
        let path = parent.join(format!("rsscript-{}-{seed}-{attempt}", std::process::id()));
        match std::fs::create_dir_all(&path) {
            Ok(()) => return Ok(tempdir_value(path.to_string_lossy())),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(file_error_value(error.to_string())),
        }
    }
    Err(file_error_value(
        "could not allocate unique TempDir path".to_string(),
    ))
}

pub(super) fn image_value(
    bytes: Vec<u8>,
    width: Option<i64>,
    height: Option<i64>,
    operations: Vec<String>,
) -> VmValue {
    let mut fields: Vec<(String, VmValue)> =
        vec![("bytes".to_string(), VmValue::Bytes(Rc::new(bytes)))];
    fields.push((
        "width".to_string(),
        width
            .map(|value| value_some(VmValue::Int(value)))
            .unwrap_or_else(value_none),
    ));
    fields.push((
        "height".to_string(),
        height
            .map(|value| value_some(VmValue::Int(value)))
            .unwrap_or_else(value_none),
    ));
    fields.push((
        "operations".to_string(),
        VmValue::List(Rc::new(RefCell::new(
            operations.into_iter().map(VmValue::string).collect(),
        ))),
    ));
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Image"), fields)))
}

pub(super) fn image_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ImageError"),
        fields,
    )))
}

pub(super) fn cancellation_source_value(id: i64) -> VmValue {
    cancellation_handle_value("CancellationSource", id)
}

pub(super) fn cancellation_token_value(id: i64) -> VmValue {
    cancellation_handle_value("CancellationToken", id)
}

fn cancellation_handle_value(name: &'static str, id: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("id".to_string(), VmValue::Int(id))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from(name), fields)))
}

pub(super) fn stream_value(items: Vec<VmValue>) -> VmValue {
    let mut fields: Vec<(String, VmValue)> = vec![(
        "items".to_string(),
        VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(items)))),
    )];
    fields.push(("collect_error".to_string(), VmValue::OptionNone));
    fields.push(("channel_id".to_string(), VmValue::OptionNone));
    fields.push(("stream_id".to_string(), VmValue::OptionNone));
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Stream"), fields)))
}

pub(super) fn stream_channel_value(channel_id: i64) -> VmValue {
    let mut fields: Vec<(String, VmValue)> = vec![(
        "items".to_string(),
        VmValue::List(Rc::new(RefCell::new(TypedVec::new()))),
    )];
    fields.push(("collect_error".to_string(), VmValue::OptionNone));
    fields.push((
        "channel_id".to_string(),
        VmValue::some(VmValue::Int(channel_id)),
    ));
    fields.push(("stream_id".to_string(), VmValue::OptionNone));
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Stream"), fields)))
}

pub(super) fn stream_collect_error_value(message: impl Into<String>) -> VmValue {
    let mut fields: Vec<(String, VmValue)> = vec![(
        "items".to_string(),
        VmValue::List(Rc::new(RefCell::new(TypedVec::new()))),
    )];
    fields.push((
        "collect_error".to_string(),
        VmValue::some(VmValue::string(message.into())),
    ));
    fields.push(("channel_id".to_string(), VmValue::OptionNone));
    fields.push(("stream_id".to_string(), VmValue::OptionNone));
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Stream"), fields)))
}

#[derive(Debug, Clone)]
pub(super) struct VmStreamState {
    pub(super) items: Rc<RefCell<TypedVec>>,
    pub(super) collect_error: Option<String>,
    pub(super) channel_id: Option<i64>,
}
