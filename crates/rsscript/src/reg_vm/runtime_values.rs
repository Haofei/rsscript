//! JSON path/decoding helpers and protocol (HTTP/TCP/WebSocket/config/file)
//! error+value constructors used by the runtime intrinsics. Split out of
//! `reg_vm/mod.rs`.

use std::rc::Rc;

use crate::vm_value::{ValueMap, VmStruct, VmValue};

use super::value_access::{value_none, value_some};
use super::*;

pub(super) fn parse_json_path(path: &str) -> Result<Vec<JsonPathPart>, VmValue> {
    let path = path.strip_prefix("$.").unwrap_or(path);
    let path = path.strip_prefix('$').unwrap_or(path);
    if path.is_empty() {
        return Ok(Vec::new());
    }

    let chars = path.chars().collect::<Vec<_>>();
    let mut parts = Vec::new();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '.' {
            index += 1;
            continue;
        }

        if chars[index] == '[' {
            index += 1;
            let start = index;
            while index < chars.len() && chars[index] != ']' {
                index += 1;
            }
            if index == chars.len() {
                return Err(json_error_value(format!(
                    "JSON path `{path}` has an unterminated array index"
                )));
            }
            let raw_index = chars[start..index].iter().collect::<String>();
            let item_index = raw_index.parse::<usize>().map_err(|_| {
                json_error_value(format!(
                    "JSON path `{path}` has invalid array index `{raw_index}`"
                ))
            })?;
            parts.push(JsonPathPart::Index(item_index));
            index += 1;
            continue;
        }

        let start = index;
        while index < chars.len() && chars[index] != '.' && chars[index] != '[' {
            index += 1;
        }
        let field = chars[start..index].iter().collect::<String>();
        if field.is_empty() {
            return Err(json_error_value(format!(
                "JSON path `{path}` contains an empty field"
            )));
        }
        parts.push(JsonPathPart::Field(field));
    }

    Ok(parts)
}

pub(super) fn parse_json_text(text: &str) -> Result<serde_json::Value, VmValue> {
    serde_json::from_str::<serde_json::Value>(text)
        .map_err(|error| json_error_value(error.to_string()))
}

pub(super) fn json_value_at(
    value: &serde_json::Value,
    path: &str,
) -> Result<serde_json::Value, VmValue> {
    let mut current = value;
    for part in parse_json_path(path)? {
        match part {
            JsonPathPart::Field(name) => {
                let Some(next) = current.get(&name) else {
                    return Err(json_error_value(format!(
                        "missing JSON field `{name}` at path `{path}`"
                    )));
                };
                current = next;
            }
            JsonPathPart::Index(index) => {
                let Some(items) = current.as_array() else {
                    return Err(json_error_value(format!(
                        "JSON path `{path}` expected an array before index `{index}`"
                    )));
                };
                let Some(next) = items.get(index) else {
                    return Err(json_error_value(format!(
                        "JSON array index `{index}` is out of bounds at path `{path}`"
                    )));
                };
                current = next;
            }
        }
    }
    Ok(current.clone())
}

pub(super) fn json_optional_path_value(value: &serde_json::Value, path: &str) -> VmValue {
    match json_value_at(value, path) {
        Ok(value) if value.is_null() => VmValue::OptionNone,
        Ok(value) => VmValue::OptionSome(Box::new(VmValue::Json(Rc::new(value)))),
        Err(_) => VmValue::OptionNone,
    }
}

pub(super) fn json_optional_typed_path_value(
    value: &serde_json::Value,
    path: &str,
    convert: fn(serde_json::Value) -> Result<VmValue, VmValue>,
) -> Result<VmValue, VmValue> {
    match json_value_at(value, path) {
        Ok(value) if value.is_null() => Ok(VmValue::OptionNone),
        Ok(value) => convert(value).map(|value| VmValue::OptionSome(Box::new(value))),
        Err(_) => Ok(VmValue::OptionNone),
    }
}

pub(super) fn json_array_contains_string_value(
    value: &serde_json::Value,
    needle: &str,
    mode: JsonArrayStringMatch,
) -> Result<VmValue, VmValue> {
    let items = json_array_items(value)?;
    Ok(VmValue::Bool(items.iter().any(|value| {
        value.as_str().is_some_and(|item| match mode {
            JsonArrayStringMatch::Exact => item == needle,
            JsonArrayStringMatch::Substring => item.contains(needle),
            JsonArrayStringMatch::Prefix => item.starts_with(needle),
        })
    })))
}

pub(super) fn json_field_json(
    value: &serde_json::Value,
    name: &str,
) -> Result<serde_json::Value, VmValue> {
    value
        .get(name)
        .cloned()
        .ok_or_else(|| json_error_value(format!("missing JSON field `{name}`")))
}

pub(super) fn json_field_value(value: &serde_json::Value, name: &str) -> Result<VmValue, VmValue> {
    json_field_json(value, name).map(|value| VmValue::Json(Rc::new(value)))
}

pub(super) fn json_typed_field_value(
    value: &serde_json::Value,
    name: &str,
    type_name: &str,
    convert: fn(&serde_json::Value) -> Option<VmValue>,
) -> Result<VmValue, VmValue> {
    let field = json_field_json(value, name)?;
    convert(&field).ok_or_else(|| {
        json_error_value(format!(
            "JSON field `{name}` is not {} {type_name}",
            json_type_article(type_name)
        ))
    })
}

pub(super) fn json_as_bool_value(value: serde_json::Value) -> Result<VmValue, VmValue> {
    value
        .as_bool()
        .map(VmValue::Bool)
        .ok_or_else(|| json_error_value("JSON value is not a boolean"))
}

pub(super) fn json_as_int_value(value: serde_json::Value) -> Result<VmValue, VmValue> {
    value
        .as_i64()
        .map(VmValue::Int)
        .ok_or_else(|| json_error_value("JSON value is not an integer"))
}

pub(super) fn json_as_string_value(value: serde_json::Value) -> Result<VmValue, VmValue> {
    value
        .as_str()
        .map(VmValue::string)
        .ok_or_else(|| json_error_value("JSON value is not a string"))
}

pub(super) fn json_optional_field_value(value: &serde_json::Value, name: &str) -> VmValue {
    match value.get(name) {
        Some(value) if value.is_null() => VmValue::OptionNone,
        Some(value) => VmValue::OptionSome(Box::new(VmValue::Json(Rc::new(value.clone())))),
        None => VmValue::OptionNone,
    }
}

pub(super) fn json_optional_typed_field_value(
    value: &serde_json::Value,
    name: &str,
    type_name: &str,
    convert: fn(&serde_json::Value) -> Option<VmValue>,
) -> Result<VmValue, VmValue> {
    match value.get(name) {
        Some(value) if value.is_null() => Ok(VmValue::OptionNone),
        Some(value) => convert(value)
            .map(|value| VmValue::OptionSome(Box::new(value)))
            .ok_or_else(|| {
                json_error_value(format!(
                    "JSON field `{name}` is not {} {type_name}",
                    json_type_article(type_name)
                ))
            }),
        None => Ok(VmValue::OptionNone),
    }
}

pub(super) fn json_type_article(type_name: &str) -> &'static str {
    if matches!(
        type_name.as_bytes().first(),
        Some(b'a' | b'e' | b'i' | b'o' | b'u')
    ) {
        "an"
    } else {
        "a"
    }
}

pub(super) fn json_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("JsonError"), fields)))
}

pub(super) fn json_decode_struct_value(
    unit: &RegUnit,
    type_name: &str,
    value: &serde_json::Value,
) -> Result<VmValue, VmValue> {
    let info = unit
        .types
        .get(type_root_name(type_name))
        .ok_or_else(|| json_error_value(format!("unknown JSON decode type `{type_name}`")))?;
    let object = value
        .as_object()
        .ok_or_else(|| json_error_value("JSON decode expected an object"))?;
    let mut fields: Vec<(String, VmValue)> = Vec::with_capacity(info.fields_ordered.len());
    for field in &info.fields_ordered {
        let decoded = match object.get(&field.name) {
            Some(value) => json_decode_field_value(unit, &field.type_name, value)?,
            None if type_root_name(&field.type_name) == "Option" => value_none(),
            None => {
                return Err(json_error_value(format!(
                    "missing JSON field `{}`",
                    field.name
                )));
            }
        };
        fields.push((field.name.clone(), decoded));
    }
    Ok(VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from(info.name.as_str()),
        fields,
    ))))
}

pub(super) fn json_decode_field_value(
    unit: &RegUnit,
    type_name: &str,
    value: &serde_json::Value,
) -> Result<VmValue, VmValue> {
    match type_root_name(type_name) {
        "Unit" => {
            if value.is_null() {
                Ok(VmValue::Unit)
            } else {
                Err(json_type_error(type_name, value))
            }
        }
        "Int" => value
            .as_i64()
            .map(VmValue::Int)
            .ok_or_else(|| json_type_error(type_name, value)),
        "Float" => value
            .as_f64()
            .map(VmValue::Float)
            .ok_or_else(|| json_type_error(type_name, value)),
        "Bool" => value
            .as_bool()
            .map(VmValue::Bool)
            .ok_or_else(|| json_type_error(type_name, value)),
        "String" => value
            .as_str()
            .map(VmValue::string)
            .ok_or_else(|| json_type_error(type_name, value)),
        "JsonValue" | "JsonLiteral" => Ok(VmValue::Json(Rc::new(value.clone()))),
        "Option" => {
            if value.is_null() {
                return Ok(value_none());
            }
            let Some(inner) = type_arg_names(type_name).and_then(|args| args.first().copied())
            else {
                return Err(json_error_value(format!(
                    "Option field `{type_name}` is missing a type argument"
                )));
            };
            json_decode_field_value(unit, inner, value).map(value_some)
        }
        "List" => {
            let Some(inner) = type_arg_names(type_name).and_then(|args| args.first().copied())
            else {
                return Err(json_error_value(format!(
                    "List field `{type_name}` is missing a type argument"
                )));
            };
            let items = value
                .as_array()
                .ok_or_else(|| json_type_error(type_name, value))?;
            let decoded = items
                .iter()
                .map(|item| json_decode_field_value(unit, inner, item))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(VmValue::List(Rc::new(RefCell::new(decoded))))
        }
        "Map" => {
            let args = type_arg_names(type_name).unwrap_or_default();
            if args.len() != 2 || type_root_name(args[0]) != "String" {
                return Err(json_error_value(format!(
                    "JSON decode only supports Map<String, T> fields in the VM, got `{type_name}`"
                )));
            }
            let object = value
                .as_object()
                .ok_or_else(|| json_type_error(type_name, value))?;
            let mut decoded = ValueMap::with_capacity_and_hasher(object.len(), Default::default());
            for (key, value) in object {
                decoded.insert(
                    VmMapKey::String(Rc::new(key.clone())),
                    json_decode_field_value(unit, args[1], value)?,
                );
            }
            Ok(VmValue::Map(Rc::new(RefCell::new(decoded))))
        }
        other if unit.types.contains_key(other) => json_decode_struct_value(unit, other, value),
        _ => Err(json_error_value(format!(
            "JSON decode does not support VM field type `{type_name}`"
        ))),
    }
}

pub(super) fn json_type_error(expected: &str, value: &serde_json::Value) -> VmValue {
    json_error_value(format!(
        "JSON decode expected `{}` but found `{}`",
        type_root_name(expected),
        json_kind(value)
    ))
}

pub(super) fn decode_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("DecodeError"),
        fields,
    )))
}

pub(super) fn config_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ConfigError"),
        fields,
    )))
}

pub(super) fn file_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("FileError"), fields)))
}

pub(super) fn channel_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("ChannelError"),
        fields,
    )))
}

pub(super) fn http_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("HttpError"), fields)))
}

pub(super) fn http_response_value(status: i64, body: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![
        ("status".to_string(), VmValue::Int(status)),
        ("body".to_string(), VmValue::string(body.into())),
    ];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("HttpResponse"),
        fields,
    )))
}

pub(super) fn http_get_local(url: &str) -> Result<VmValue, VmValue> {
    let Some(rest) = url.strip_prefix("http://") else {
        return Err(http_error_value(format!(
            "HTTP request failed for {url}: error sending request for url ({url})"
        )));
    };
    let (host_port, path) = match rest.split_once('/') {
        Some((host_port, path)) => (host_port, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    if host_port.is_empty() {
        return Err(http_error_value(format!(
            "HTTP URL is missing a host: `{url}`"
        )));
    }
    let mut stream = TcpStream::connect(host_port).map_err(|error| {
        http_error_value(format!("HTTP connect to `{host_port}` failed: {error}"))
    })?;
    let timeout = Some(std::time::Duration::from_secs(5));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|error| {
        http_error_value(format!("HTTP request write failed for `{url}`: {error}"))
    })?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).map_err(|error| {
        http_error_value(format!("HTTP response read failed for `{url}`: {error}"))
    })?;
    parse_http_response(&response).map_err(http_error_value)
}

pub(super) fn parse_http_response(response: &[u8]) -> Result<VmValue, String> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "HTTP response is missing header terminator".to_string())?;
    let headers = String::from_utf8_lossy(&response[..header_end]);
    let status_line = headers
        .lines()
        .next()
        .ok_or_else(|| "HTTP response is missing status line".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| format!("HTTP response status line is invalid: `{status_line}`"))?
        .parse::<i64>()
        .map_err(|error| format!("HTTP response status is invalid: {error}"))?;
    let body = String::from_utf8_lossy(&response[header_end + 4..]).to_string();
    Ok(http_response_value(status, body))
}

pub(super) fn tcp_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("TcpError"), fields)))
}

pub(super) fn tcp_stream_value(id: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("id".to_string(), VmValue::Int(id))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("TcpStream"), fields)))
}

pub(super) fn websocket_value(id: i64) -> VmValue {
    let fields: Vec<(String, VmValue)> = vec![("id".to_string(), VmValue::Int(id))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("WebSocket"), fields)))
}

pub(super) fn websocket_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("WebSocketError"),
        fields,
    )))
}
