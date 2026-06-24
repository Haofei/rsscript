//! Free helpers that coerce / extract typed runtime state out of a [`VmValue`]
//! (channels, senders, files, HTTP requests, config handles, …). Split out of
//! `reg_vm/mod.rs`; the VM core calls these via `use value_access::*`.

use std::path::PathBuf;
use std::rc::Rc;

use crate::eval_types::EvalError;
use crate::vm_value::VmValue;

use super::*;

pub(super) fn expect_environment_state(value: &VmValue) -> Result<(bool, bool), EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Environment" => {
            let has_parent = data.get("has_parent").ok_or_else(|| {
                EvalError::Runtime("Environment value is missing has_parent.".to_string())
            })?;
            let has_function = data.get("has_function").ok_or_else(|| {
                EvalError::Runtime("Environment value is missing has_function.".to_string())
            })?;
            Ok((expect_bool_ref(has_parent)?, expect_bool_ref(has_function)?))
        }
        VmValue::Managed(value) => expect_environment_state(&value.borrow()),
        other => Err(EvalError::Runtime(format!(
            "expected Environment, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_function_has_closure(value: &VmValue) -> Result<bool, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "FunctionObject" => {
            let value = data.get("has_closure").ok_or_else(|| {
                EvalError::Runtime("FunctionObject value is missing has_closure.".to_string())
            })?;
            expect_bool_ref(value)
        }
        VmValue::Managed(value) => expect_function_has_closure(&value.borrow()),
        other => Err(EvalError::Runtime(format!(
            "expected FunctionObject, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_counter_value(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Counter" => {
            let value = data
                .get("value")
                .ok_or_else(|| EvalError::Runtime("Counter value is missing.".to_string()))?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Counter, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_instant_unix_ms(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Instant" => {
            let value = data.get("unix_ms").ok_or_else(|| {
                EvalError::Runtime("Instant value is missing unix_ms.".to_string())
            })?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Instant, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_deadline_unix_ms(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Deadline" => {
            let value = data.get("unix_ms").ok_or_else(|| {
                EvalError::Runtime("Deadline value is missing unix_ms.".to_string())
            })?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Deadline, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_db_connection_ref(value: &VmValue) -> Result<VmDbConnection, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "DbConnection" => {
            let url = data.get("url").ok_or_else(|| {
                EvalError::Runtime("DbConnection value is missing url.".to_string())
            })?;
            let queries = data.get("queries").ok_or_else(|| {
                EvalError::Runtime("DbConnection value is missing queries.".to_string())
            })?;
            Ok(VmDbConnection {
                url: expect_string_ref(url)?.to_string(),
                queries: expect_string_list_ref(queries)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected DbConnection, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_cancellation_id_ref(
    value: &VmValue,
    expected_name: &str,
) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == expected_name => {
            let value = data.get("id").ok_or_else(|| {
                EvalError::Runtime(format!("{expected_name} value is missing id."))
            })?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected {expected_name}, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_channel_ref(value: &VmValue) -> Result<VmChannelState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Channel" => {
            let int_field = |name: &str| {
                data.get(name)
                    .ok_or_else(|| EvalError::Runtime(format!("Channel value is missing {name}.")))
                    .and_then(expect_int_ref)
            };
            let receiver_taken = data
                .get("receiver_taken")
                .ok_or_else(|| {
                    EvalError::Runtime("Channel value is missing receiver_taken.".to_string())
                })
                .and_then(expect_bool_ref)?;
            Ok(VmChannelState {
                id: int_field("id")?,
                capacity: int_field("capacity")?,
                receiver_taken,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Channel, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_sender_ref(value: &VmValue) -> Result<VmSender, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Sender" => {
            let channel_id = data.get("channel_id").ok_or_else(|| {
                EvalError::Runtime("Sender value is missing channel_id.".to_string())
            })?;
            let closed = data
                .get("closed")
                .ok_or_else(|| EvalError::Runtime("Sender value is missing closed.".to_string()))?;
            Ok(VmSender {
                channel_id: expect_int_ref(channel_id)?,
                closed: expect_bool_ref(closed)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Sender, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_receiver_ref(value: &VmValue) -> Result<VmReceiver, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Receiver" => {
            let channel_id = data.get("channel_id").ok_or_else(|| {
                EvalError::Runtime("Receiver value is missing channel_id.".to_string())
            })?;
            let closed = data.get("closed").ok_or_else(|| {
                EvalError::Runtime("Receiver value is missing closed.".to_string())
            })?;
            Ok(VmReceiver {
                channel_id: expect_int_ref(channel_id)?,
                closed: expect_bool_ref(closed)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Receiver, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_resource_pool_ref(value: &VmValue) -> Result<VmResourcePoolState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "ResourcePool" => {
            let id = data.get("id").ok_or_else(|| {
                EvalError::Runtime("ResourcePool value is missing id.".to_string())
            })?;
            Ok(VmResourcePoolState {
                id: expect_int_ref(id)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected ResourcePool, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_stream_ref(value: &VmValue) -> Result<VmStreamState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Stream" => {
            // Share the struct's underlying items list (an `Rc<RefCell<TypedVec>>`)
            // rather than copying it: `Stream.next` removes the head element, and a
            // `read stream` clone of the struct shares this same `Rc`, so the cursor
            // advance must write back through it to be visible to a later
            // `collect_list`/`next` on the same stream.
            let items = data
                .get("items")
                .ok_or_else(|| EvalError::Runtime("Stream value is missing items.".to_string()))
                .and_then(expect_list_ref)?;
            let collect_error = data
                .get("collect_error")
                .map(option_payload_value)
                .transpose()?
                .flatten()
                .map(|value| expect_string_ref(&value).map(str::to_string))
                .transpose()?;
            let channel_id = data
                .get("channel_id")
                .map(option_payload_value)
                .transpose()?
                .flatten()
                .map(|value| expect_int_ref(&value))
                .transpose()?;
            Ok(VmStreamState {
                items,
                collect_error,
                channel_id,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Stream, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_tcp_stream_id_ref(value: &VmValue) -> Result<i64, EvalError> {
    expect_id_struct_ref(value, "TcpStream")
}

pub(super) fn expect_websocket_id_ref(value: &VmValue) -> Result<i64, EvalError> {
    expect_id_struct_ref(value, "WebSocket")
}

pub(super) fn expect_id_struct_ref(value: &VmValue, expected_name: &str) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == expected_name => {
            let id = data.get("id").ok_or_else(|| {
                EvalError::Runtime(format!("{expected_name} value is missing id."))
            })?;
            expect_int_ref(id)
        }
        other => Err(EvalError::Runtime(format!(
            "expected {expected_name}, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn option_payload_value(value: &VmValue) -> Result<Option<VmValue>, EvalError> {
    match value {
        // Inline and heap `Some` are unified into one owned payload; the inline
        // arm has no `&VmValue` to borrow, so this accessor returns owned (cheap:
        // a scalar copy or the same clone the heap arm did before).
        VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => Ok(value.unwrap_some()),
        VmValue::OptionNone => Ok(None),
        VmValue::Variant(data) if data.name().as_ref() == "Some" => data
            .get("value")
            .cloned()
            .map(Some)
            .ok_or_else(|| EvalError::Runtime("Some value is missing.".to_string())),
        VmValue::Variant(data) if data.name().as_ref() == "None" => Ok(None),
        other => Err(EvalError::Runtime(format!(
            "expected Option, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_process_request_ref(value: &VmValue) -> Result<VmProcessRequest, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "ProcessRequest" => {
            let command = data.get("command").ok_or_else(|| {
                EvalError::Runtime("ProcessRequest command is missing.".to_string())
            })?;
            let args = data.get("args").ok_or_else(|| {
                EvalError::Runtime("ProcessRequest args are missing.".to_string())
            })?;
            let cwd = data
                .get("cwd")
                .map(option_payload_value)
                .transpose()?
                .flatten()
                .map(|value| expect_string_ref(&value).map(PathBuf::from))
                .transpose()?;
            let stdin = data
                .get("stdin")
                .map(option_payload_value)
                .transpose()?
                .flatten()
                .map(|value| expect_string_ref(&value).map(str::to_string))
                .transpose()?;
            let env = data
                .get("env")
                .map(expect_process_env_list_ref)
                .transpose()?
                .unwrap_or_default();
            let timeout_ms = data
                .get("timeout_ms")
                .map(expect_int_ref)
                .transpose()?
                .unwrap_or(0);
            let merge_stderr = data
                .get("merge_stderr")
                .map(expect_bool_ref)
                .transpose()?
                .unwrap_or(false);
            let output_cap_bytes = data
                .get("output_cap_bytes")
                .map(expect_int_ref)
                .transpose()?
                .unwrap_or(0);
            Ok(VmProcessRequest {
                command: expect_string_ref(command)?.to_string(),
                args: expect_string_list_ref(args)?,
                cwd,
                stdin,
                env,
                timeout_ms,
                merge_stderr,
                output_cap_bytes,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected ProcessRequest, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_process_env_list_ref(
    value: &VmValue,
) -> Result<Vec<(String, String)>, EvalError> {
    let list = expect_list_ref(value)?;
    list.borrow()
        .iter()
        .map(|value| match value {
            VmValue::Struct(data) if data.name().as_ref() == "ProcessEnv" => {
                let name = data
                    .get("name")
                    .ok_or_else(|| EvalError::Runtime("ProcessEnv name is missing.".to_string()))?;
                let value = data.get("value").ok_or_else(|| {
                    EvalError::Runtime("ProcessEnv value is missing.".to_string())
                })?;
                Ok((
                    expect_string_ref(name)?.to_string(),
                    expect_string_ref(value)?.to_string(),
                ))
            }
            other => Err(EvalError::Runtime(format!(
                "expected ProcessEnv, got `{}`.",
                other.display()
            ))),
        })
        .collect()
}

pub(super) fn expect_row_buffer_bytes_ref(value: &VmValue) -> Result<Vec<u8>, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "RowBuffer" => data
            .get("bytes")
            .ok_or_else(|| EvalError::Runtime("RowBuffer value is missing bytes.".to_string()))
            .and_then(expect_bytes_ref)
            .map(|bytes| bytes.to_vec()),
        other => Err(EvalError::Runtime(format!(
            "expected RowBuffer, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_row_fields_ref(value: &VmValue) -> Result<Vec<String>, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Row" => {
            let fields = data
                .get("fields")
                .ok_or_else(|| EvalError::Runtime("Row value is missing fields.".to_string()))?;
            expect_string_list_ref(fields)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Row, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_file_ref(value: &VmValue) -> Result<VmFileState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "File" => {
            let string_field = |name: &str| {
                data.get(name)
                    .ok_or_else(|| EvalError::Runtime(format!("File {name} is missing.")))
                    .and_then(expect_string_ref)
                    .map(str::to_string)
            };
            let cursor = data
                .get("cursor")
                .ok_or_else(|| EvalError::Runtime("File cursor is missing.".to_string()))
                .and_then(expect_int_ref)?;
            Ok(VmFileState {
                path: string_field("path")?,
                mode: string_field("mode")?,
                cursor: cursor.max(0) as u64,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected File, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_tempdir_path_ref(value: &VmValue) -> Result<String, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "TempDir" => data
            .get("path")
            .ok_or_else(|| EvalError::Runtime("TempDir path is missing.".to_string()))
            .and_then(expect_string_ref)
            .map(str::to_string),
        other => Err(EvalError::Runtime(format!(
            "expected TempDir, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_config_value_name(value: &VmValue) -> Result<String, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "ConfigValue" => {
            let value = data
                .get("name")
                .ok_or_else(|| EvalError::Runtime("ConfigValue name is missing.".to_string()))?;
            Ok(expect_string_ref(value)?.to_string())
        }
        other => Err(EvalError::Runtime(format!(
            "expected ConfigValue, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_config_store_name(value: &VmValue) -> Result<String, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "ConfigStore" => {
            let value = data
                .get("name")
                .ok_or_else(|| EvalError::Runtime("ConfigStore name is missing.".to_string()))?;
            Ok(expect_string_ref(value)?.to_string())
        }
        other => Err(EvalError::Runtime(format!(
            "expected ConfigStore, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_config_rule_count(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Config" => {
            let value = data
                .get("rule_count")
                .ok_or_else(|| EvalError::Runtime("Config rule_count is missing.".to_string()))?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected Config, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_global_config_rule_count(value: &VmValue) -> Result<i64, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "GlobalConfig" => {
            let value = data.get("rule_count").ok_or_else(|| {
                EvalError::Runtime("GlobalConfig rule_count is missing.".to_string())
            })?;
            expect_int_ref(value)
        }
        other => Err(EvalError::Runtime(format!(
            "expected GlobalConfig, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_image_state(value: &VmValue) -> Result<ImageState, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Image" => {
            let bytes = data
                .get("bytes")
                .ok_or_else(|| EvalError::Runtime("Image value is missing bytes.".to_string()))?;
            let width = data
                .get("width")
                .ok_or_else(|| EvalError::Runtime("Image value is missing width.".to_string()))?;
            let height = data
                .get("height")
                .ok_or_else(|| EvalError::Runtime("Image value is missing height.".to_string()))?;
            let operations = data.get("operations").ok_or_else(|| {
                EvalError::Runtime("Image value is missing operations.".to_string())
            })?;
            Ok(ImageState {
                bytes: expect_bytes_ref(bytes)?.to_vec(),
                width: option_int_payload(width)?,
                height: option_int_payload(height)?,
                operations: expect_string_list_ref(operations)?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected Image, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn option_int_payload(value: &VmValue) -> Result<Option<i64>, EvalError> {
    match value {
        VmValue::Variant(data) if data.name().as_ref() == "Some" => data
            .get("value")
            .ok_or_else(|| EvalError::Runtime("Some value is missing.".to_string()))
            .and_then(expect_int_ref)
            .map(Some),
        VmValue::Variant(data) if data.name().as_ref() == "None" => Ok(None),
        VmValue::OptionSomeScalar(_) | VmValue::OptionSomeHeap(_) => value
            .unwrap_some()
            .map(|inner| expect_int_ref(&inner))
            .transpose(),
        VmValue::OptionNone => Ok(None),
        other => Err(EvalError::Runtime(format!(
            "expected Option<Int>, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_http_request_ref(value: &VmValue) -> Result<VmHttpRequest, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "HttpRequest" => {
            let string_field = |name: &str| {
                data.get(name)
                    .ok_or_else(|| EvalError::Runtime(format!("HttpRequest {name} is missing.")))
                    .and_then(expect_string_ref)
                    .map(str::to_string)
            };
            let int_field = |name: &str| {
                data.get(name)
                    .ok_or_else(|| EvalError::Runtime(format!("HttpRequest {name} is missing.")))
                    .and_then(expect_int_ref)
            };
            Ok(VmHttpRequest {
                method: string_field("method")?,
                url: string_field("url")?,
                body: string_field("body")?,
                timeout_ms: int_field("timeout_ms")?,
                attempts: int_field("attempts")?,
                backoff_ms: int_field("backoff_ms")?,
                header_count: int_field("header_count")?,
            })
        }
        other => Err(EvalError::Runtime(format!(
            "expected HttpRequest, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn expect_regex_ref(value: &VmValue) -> Result<regex::Regex, EvalError> {
    match value {
        VmValue::Struct(data) if data.name().as_ref() == "Regex" => {
            let pattern = data
                .get("pattern")
                .ok_or_else(|| EvalError::Runtime("Regex value is missing pattern.".to_string()))?;
            regex::Regex::new(expect_string_ref(pattern)?)
                .map_err(|error| EvalError::Runtime(error.to_string()))
        }
        other => Err(EvalError::Runtime(format!(
            "expected Regex, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn result_variant_payload(
    value: &VmValue,
) -> Result<Result<VmValue, VmValue>, EvalError> {
    match value {
        VmValue::Variant(data) if data.name().as_ref() == "Ok" => data
            .get("value")
            .cloned()
            .map(Ok)
            .ok_or_else(|| EvalError::Runtime("Ok value is missing.".to_string())),
        VmValue::Variant(data) if data.name().as_ref() == "Err" => data
            .get("value")
            .cloned()
            .map(Err)
            .ok_or_else(|| EvalError::Runtime("Err value is missing.".to_string())),
        other => Err(EvalError::Runtime(format!(
            "? expects a Result value, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn value_some(value: VmValue) -> VmValue {
    VmValue::some(value)
}

pub(super) fn value_none() -> VmValue {
    VmValue::OptionNone
}

pub(super) fn value_err(value: VmValue) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("value".to_string(), value)];
    VmValue::Variant(Rc::new(VmStruct::from_named(Rc::from("Err"), fields)))
}

pub(super) fn value_ok(value: VmValue) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("value".to_string(), value)];
    VmValue::Variant(Rc::new(VmStruct::from_named(Rc::from("Ok"), fields)))
}

pub(super) fn expect_string_ref(value: &VmValue) -> Result<&str, EvalError> {
    match value {
        VmValue::String(value) => Ok(value.as_str()),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected String, got `{}`.",
            other.display()
        ))),
    }
}
