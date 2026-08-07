//! Resource-value constructor free functions and their supporting data structs,
//! split out of `reg_vm/mod.rs` (channels/senders/receivers/instants/
//! configs/requests/responses/http/ws/process/file/directory/image/tempdir/
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

pub(super) fn deadline_value(unix_ms: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("unix_ms".to_string(), VmValue::Int(unix_ms))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Deadline"), fields)))
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
