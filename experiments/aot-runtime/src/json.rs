use std::fmt;

#[derive(Debug, Clone)]
pub struct JsonValue {
    inner: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    message: String,
}

impl JsonError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for JsonError {}

impl From<serde_json::Error> for JsonError {
    fn from(error: serde_json::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<std::io::Error> for JsonError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

pub fn json_error_message(error: &JsonError) -> String {
    error.to_string()
}

pub fn json_parse(text: &str) -> Result<JsonValue, JsonError> {
    serde_json::from_str(text)
        .map(|inner| JsonValue { inner })
        .map_err(JsonError::from)
}

pub fn json_value(value: &str) -> JsonValue {
    serde_json::from_str(value)
        .map(|inner| JsonValue { inner })
        .expect("compiler-generated JSON literal should be valid JSON")
}

pub fn json_clone(value: &JsonValue) -> JsonValue {
    value.clone()
}

pub fn json_decode_value<T>(value: &JsonValue) -> Result<T, JsonError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_value(value.inner.clone()).map_err(JsonError::from)
}

pub fn json_decode_text<T>(text: &str) -> Result<T, JsonError>
where
    T: serde::de::DeserializeOwned,
{
    serde_json::from_str(text).map_err(JsonError::from)
}

pub fn json_quote_string(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string to JSON cannot fail")
}

pub fn json_to_string(value: &JsonValue) -> String {
    serde_json::to_string(&value.inner).expect("serializing a JSON value cannot fail")
}

pub fn json_string_field(name: &str, value: &str) -> String {
    format!("{}:{}", json_quote_string(name), json_quote_string(value))
}

pub fn json_int_field(name: &str, value: i64) -> String {
    format!("{}:{value}", json_quote_string(name))
}

pub fn json_bool_field(name: &str, value: bool) -> String {
    format!("{}:{value}", json_quote_string(name))
}

pub fn json_raw_field(name: &str, value: &str) -> String {
    format!("{}:{value}", json_quote_string(name))
}

pub fn json_object(fields: &[String]) -> String {
    format!("{{{}}}", fields.join(","))
}

pub fn json_array(items: &[String]) -> String {
    format!("[{}]", items.join(","))
}

pub fn json_string_array(items: &[String]) -> String {
    let quoted = items
        .iter()
        .map(|item| json_quote_string(item))
        .collect::<Vec<_>>();
    json_array(&quoted)
}

pub fn json_strings(items: &[String]) -> JsonValue {
    JsonValue {
        inner: serde_json::Value::Array(
            items
                .iter()
                .map(|item| serde_json::Value::String(item.clone()))
                .collect(),
        ),
    }
}

pub fn json_values(items: &[JsonValue]) -> JsonValue {
    JsonValue {
        inner: serde_json::Value::Array(items.iter().map(|item| item.inner.clone()).collect()),
    }
}

pub fn yaml_parse(text: &str) -> Result<JsonValue, JsonError> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|error| JsonError::new(error.to_string()))?;
    let inner = serde_json::to_value(value)?;
    Ok(JsonValue { inner })
}

pub fn json_field(value: &JsonValue, name: &str) -> Result<JsonValue, JsonError> {
    let Some(field) = value.inner.get(name) else {
        return Err(JsonError::new(format!("missing JSON field `{name}`")));
    };
    Ok(JsonValue {
        inner: field.clone(),
    })
}

pub fn json_field_string(value: &JsonValue, name: &str) -> Result<String, JsonError> {
    let field = json_field(value, name)?;
    let Some(text) = field.inner.as_str() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not a string"
        )));
    };
    Ok(text.to_string())
}

pub fn json_field_int(value: &JsonValue, name: &str) -> Result<i64, JsonError> {
    let field = json_field(value, name)?;
    let Some(number) = field.inner.as_i64() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not an integer"
        )));
    };
    Ok(number)
}

pub fn json_field_bool(value: &JsonValue, name: &str) -> Result<bool, JsonError> {
    let field = json_field(value, name)?;
    let Some(flag) = field.inner.as_bool() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not a boolean"
        )));
    };
    Ok(flag)
}

pub fn json_field_optional(value: &JsonValue, name: &str) -> Result<Option<JsonValue>, JsonError> {
    match value.inner.get(name) {
        Some(field) if field.is_null() => Ok(None),
        Some(field) => Ok(Some(JsonValue {
            inner: field.clone(),
        })),
        None => Ok(None),
    }
}

pub fn json_field_optional_string(
    value: &JsonValue,
    name: &str,
) -> Result<Option<String>, JsonError> {
    let Some(field) = value.inner.get(name) else {
        return Ok(None);
    };
    if field.is_null() {
        return Ok(None);
    }
    let Some(text) = field.as_str() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not a string"
        )));
    };
    Ok(Some(text.to_string()))
}

pub fn json_field_optional_int(value: &JsonValue, name: &str) -> Result<Option<i64>, JsonError> {
    let Some(field) = value.inner.get(name) else {
        return Ok(None);
    };
    if field.is_null() {
        return Ok(None);
    }
    let Some(number) = field.as_i64() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not an integer"
        )));
    };
    Ok(Some(number))
}

pub fn json_field_optional_bool(value: &JsonValue, name: &str) -> Result<Option<bool>, JsonError> {
    let Some(field) = value.inner.get(name) else {
        return Ok(None);
    };
    if field.is_null() {
        return Ok(None);
    }
    let Some(flag) = field.as_bool() else {
        return Err(JsonError::new(format!(
            "JSON field `{name}` is not a boolean"
        )));
    };
    Ok(Some(flag))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum JsonPathPart {
    Field(String),
    Index(usize),
}

fn parse_json_path(path: &str) -> Result<Vec<JsonPathPart>, JsonError> {
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
                return Err(JsonError::new(format!(
                    "JSON path `{path}` has an unterminated array index"
                )));
            }
            let raw_index = chars[start..index].iter().collect::<String>();
            let item_index = raw_index.parse::<usize>().map_err(|_| {
                JsonError::new(format!(
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
            return Err(JsonError::new(format!(
                "JSON path `{path}` contains an empty field"
            )));
        }
        parts.push(JsonPathPart::Field(field));
    }

    Ok(parts)
}

pub fn json_value_at(value: &JsonValue, path: &str) -> Result<JsonValue, JsonError> {
    let mut current = &value.inner;
    for part in parse_json_path(path)? {
        match part {
            JsonPathPart::Field(name) => {
                let Some(next) = current.get(&name) else {
                    return Err(JsonError::new(format!(
                        "missing JSON field `{name}` at path `{path}`"
                    )));
                };
                current = next;
            }
            JsonPathPart::Index(index) => {
                let Some(items) = current.as_array() else {
                    return Err(JsonError::new(format!(
                        "JSON path `{path}` expected an array before index `{index}`"
                    )));
                };
                let Some(next) = items.get(index) else {
                    return Err(JsonError::new(format!(
                        "JSON array index `{index}` is out of bounds at path `{path}`"
                    )));
                };
                current = next;
            }
        }
    }
    Ok(JsonValue {
        inner: current.clone(),
    })
}

pub fn json_at(value: &JsonValue, path: &str) -> Result<JsonValue, JsonError> {
    json_value_at(value, path)
}

pub fn json_at_or(value: &JsonValue, path: &str, fallback: &JsonValue) -> JsonValue {
    json_value_at(value, path).unwrap_or_else(|_| fallback.clone())
}

pub fn json_at_string(value: &JsonValue, path: &str) -> Result<String, JsonError> {
    let item = json_value_at(value, path)?;
    json_as_string(&item)
}

pub fn json_at_int(value: &JsonValue, path: &str) -> Result<i64, JsonError> {
    let item = json_value_at(value, path)?;
    json_as_int(&item)
}

pub fn json_at_bool(value: &JsonValue, path: &str) -> Result<bool, JsonError> {
    let item = json_value_at(value, path)?;
    json_as_bool(&item)
}

pub fn json_at_optional(value: &JsonValue, path: &str) -> Result<Option<JsonValue>, JsonError> {
    match json_value_at(value, path) {
        Ok(item) if item.inner.is_null() => Ok(None),
        Ok(item) => Ok(Some(item)),
        Err(_) => Ok(None),
    }
}

pub fn json_at_optional_string(value: &JsonValue, path: &str) -> Result<Option<String>, JsonError> {
    match json_value_at(value, path) {
        Ok(item) if item.inner.is_null() => Ok(None),
        Ok(item) => json_as_string(&item).map(Some),
        Err(_) => Ok(None),
    }
}

pub fn json_at_optional_int(value: &JsonValue, path: &str) -> Result<Option<i64>, JsonError> {
    match json_value_at(value, path) {
        Ok(item) if item.inner.is_null() => Ok(None),
        Ok(item) => json_as_int(&item).map(Some),
        Err(_) => Ok(None),
    }
}

pub fn json_at_optional_bool(value: &JsonValue, path: &str) -> Result<Option<bool>, JsonError> {
    match json_value_at(value, path) {
        Ok(item) if item.inner.is_null() => Ok(None),
        Ok(item) => json_as_bool(&item).map(Some),
        Err(_) => Ok(None),
    }
}

pub fn json_at_string_or(value: &JsonValue, path: &str, fallback: &str) -> String {
    json_at_string(value, path).unwrap_or_else(|_| fallback.to_string())
}

pub fn json_at_int_or(value: &JsonValue, path: &str, fallback: i64) -> i64 {
    json_at_int(value, path).unwrap_or(fallback)
}

pub fn json_at_bool_or(value: &JsonValue, path: &str, fallback: bool) -> bool {
    json_at_bool(value, path).unwrap_or(fallback)
}

pub fn json_at_to_string(value: &JsonValue, path: &str) -> Result<String, JsonError> {
    let item = json_value_at(value, path)?;
    Ok(json_to_string(&item))
}

pub fn json_at_to_string_or(value: &JsonValue, path: &str, fallback: &str) -> String {
    json_at_to_string(value, path).unwrap_or_else(|_| fallback.to_string())
}

pub fn json_string_at(text: &str, path: &str) -> Result<String, JsonError> {
    let value = json_parse(text)?;
    let item = json_value_at(&value, path)?;
    json_as_string(&item)
}

pub fn json_int_at(text: &str, path: &str) -> Result<i64, JsonError> {
    let value = json_parse(text)?;
    let item = json_value_at(&value, path)?;
    json_as_int(&item)
}

pub fn json_bool_at(text: &str, path: &str) -> Result<bool, JsonError> {
    let value = json_parse(text)?;
    let item = json_value_at(&value, path)?;
    json_as_bool(&item)
}

pub fn json_to_string_at(text: &str, path: &str) -> Result<String, JsonError> {
    let value = json_parse(text)?;
    let item = json_value_at(&value, path)?;
    Ok(json_to_string(&item))
}

pub fn json_string_at_or(text: &str, path: &str, fallback: &str) -> String {
    json_string_at(text, path).unwrap_or_else(|_| fallback.to_string())
}

pub fn json_int_at_or(text: &str, path: &str, fallback: i64) -> i64 {
    json_int_at(text, path).unwrap_or(fallback)
}

pub fn json_bool_at_or(text: &str, path: &str, fallback: bool) -> bool {
    json_bool_at(text, path).unwrap_or(fallback)
}

pub fn json_to_string_at_or(text: &str, path: &str, fallback: &str) -> String {
    json_to_string_at(text, path).unwrap_or_else(|_| fallback.to_string())
}

pub fn json_as_string(value: &JsonValue) -> Result<String, JsonError> {
    let Some(text) = value.inner.as_str() else {
        return Err(JsonError::new("JSON value is not a string"));
    };
    Ok(text.to_string())
}

pub fn json_as_int(value: &JsonValue) -> Result<i64, JsonError> {
    let Some(number) = value.inner.as_i64() else {
        return Err(JsonError::new("JSON value is not an integer"));
    };
    Ok(number)
}

pub fn json_as_bool(value: &JsonValue) -> Result<bool, JsonError> {
    let Some(flag) = value.inner.as_bool() else {
        return Err(JsonError::new("JSON value is not a boolean"));
    };
    Ok(flag)
}

pub fn json_is_null(value: &JsonValue) -> bool {
    value.inner.is_null()
}

pub fn json_is_array(value: &JsonValue) -> bool {
    value.inner.is_array()
}

pub fn json_is_object(value: &JsonValue) -> bool {
    value.inner.is_object()
}

pub fn json_kind(value: &JsonValue) -> String {
    match &value.inner {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(number) if number.is_i64() || number.is_u64() => "int",
        serde_json::Value::Number(_) => "float",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
    .to_string()
}

pub fn json_object_len(value: &JsonValue) -> Result<i64, JsonError> {
    let Some(fields) = value.inner.as_object() else {
        return Err(JsonError::new("JSON value is not an object"));
    };
    Ok(fields.len() as i64)
}

pub fn json_object_keys(value: &JsonValue) -> Result<Vec<String>, JsonError> {
    let Some(fields) = value.inner.as_object() else {
        return Err(JsonError::new("JSON value is not an object"));
    };
    let mut keys = fields.keys().cloned().collect::<Vec<_>>();
    keys.sort();
    Ok(keys)
}

pub fn json_array_len(value: &JsonValue) -> Result<i64, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    Ok(items.len() as i64)
}

pub fn json_array_get(value: &JsonValue, index: i64) -> Result<JsonValue, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    if index < 0 {
        return Err(JsonError::new(format!(
            "JSON array index `{index}` is negative"
        )));
    }
    let Some(item) = items.get(index as usize) else {
        return Err(JsonError::new(format!(
            "JSON array index `{index}` is out of bounds"
        )));
    };
    Ok(JsonValue {
        inner: item.clone(),
    })
}

pub fn json_array_strings(value: &JsonValue) -> Result<Vec<String>, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut strings = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(text) = item.as_str() else {
            return Err(JsonError::new(format!(
                "JSON array item `{index}` is not a string"
            )));
        };
        strings.push(text.to_string());
    }
    Ok(strings)
}

pub fn json_array_ints(value: &JsonValue) -> Result<Vec<i64>, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut numbers = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(number) = item.as_i64() else {
            return Err(JsonError::new(format!(
                "JSON array item `{index}` is not an integer"
            )));
        };
        numbers.push(number);
    }
    Ok(numbers)
}

pub fn json_array_bools(value: &JsonValue) -> Result<Vec<bool>, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut flags = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let Some(flag) = item.as_bool() else {
            return Err(JsonError::new(format!(
                "JSON array item `{index}` is not a boolean"
            )));
        };
        flags.push(flag);
    }
    Ok(flags)
}

pub fn json_array_contains_string(value: &JsonValue, item: &str) -> Result<bool, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    Ok(items
        .iter()
        .any(|value| value.as_str().is_some_and(|text| text == item)))
}

pub fn json_array_contains_substring(value: &JsonValue, text: &str) -> Result<bool, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    Ok(items
        .iter()
        .any(|value| value.as_str().is_some_and(|item| item.contains(text))))
}

pub fn json_array_contains_prefix(value: &JsonValue, prefix: &str) -> Result<bool, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    Ok(items
        .iter()
        .any(|value| value.as_str().is_some_and(|item| item.starts_with(prefix))))
}

pub fn json_array_count_where(
    value: &JsonValue,
    mut predicate: impl FnMut(JsonValue) -> Result<bool, JsonError>,
) -> Result<i64, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut count = 0_i64;
    for item in items {
        if predicate(JsonValue {
            inner: item.clone(),
        })? {
            count += 1;
        }
    }
    Ok(count)
}

pub fn json_array_fold<T: Clone>(
    value: &JsonValue,
    initial: &T,
    mut folder: impl FnMut(T, JsonValue) -> Result<T, JsonError>,
) -> Result<T, JsonError> {
    let Some(items) = value.inner.as_array() else {
        return Err(JsonError::new("JSON value is not an array"));
    };
    let mut state = initial.clone();
    for item in items {
        state = folder(
            state,
            JsonValue {
                inner: item.clone(),
            },
        )?;
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaml_parse_reuses_json_value_accessors() {
        let value = yaml_parse("name: rss\nports:\n  - 8080\n  - 9090\n")
            .expect("YAML should parse into a JsonValue");

        let name = json_field(&value, "name").expect("name field exists");
        assert_eq!(json_as_string(&name).expect("name is a string"), "rss");

        let ports = json_field(&value, "ports").expect("ports field exists");
        assert_eq!(json_array_len(&ports).expect("ports is an array"), 2);
    }

    #[test]
    fn json_path_helpers_read_nested_fields_and_arrays() {
        let text = r#"{
            "choices": [
                {
                    "message": {
                        "content": "done",
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "function": {
                                    "name": "write_file",
                                    "arguments": "{\"path\":\"hello.txt\"}"
                                }
                            }
                        ]
                    }
                }
            ]
        }"#;
        let value = json_parse(text).expect("JSON should parse");
        let literal = json_value(r#"{"ready":true,"count":2}"#);
        assert!(!json_bool_at_or(
            text,
            "choices[0].message.tool_calls[0].function.name",
            false
        ));
        assert_eq!(json_int_at_or(&json_to_string(&literal), "count", 0), 2);
        assert!(json_bool_at_or(&json_to_string(&literal), "ready", false));
        assert_eq!(
            json_string_array(&["a".to_string(), "b".to_string()]),
            r#"["a","b"]"#
        );

        assert_eq!(
            json_as_string(
                &json_value_at(&value, "choices[0].message.tool_calls[0].function.name")
                    .expect("path should resolve")
            )
            .expect("path value should be string"),
            "write_file"
        );
        assert_eq!(
            json_string_at(text, "$.choices[0].message.content")
                .expect("string path should resolve"),
            "done"
        );
        assert_eq!(
            json_string_at_or(text, "choices[0].message.missing", "fallback"),
            "fallback"
        );
        let message = json_value_at(&value, "choices[0].message").expect("message should resolve");
        assert_eq!(
            json_to_string(&json_at_or(&value, "choices[2].message", &message)),
            json_to_string(&message)
        );
        let serialized = json_to_string(&message);
        assert!(
            serialized.contains(r#""tool_calls""#),
            "serialized message should preserve tool calls"
        );
        assert!(
            json_value_at(&value, "choices[2].message")
                .expect_err("missing index should fail")
                .message
                .contains("out of bounds")
        );
    }

    #[test]
    fn yaml_parse_reports_error_text() {
        let error = yaml_parse("name: [unterminated\n").expect_err("invalid YAML should error");
        assert!(
            !json_error_message(&error).is_empty(),
            "YAML parse error should carry a message"
        );
    }
}
