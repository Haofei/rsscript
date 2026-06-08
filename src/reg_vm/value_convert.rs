//! Free helpers that *construct* and *convert* `VmValue`s: typed value
//! constructors (regex/csv/row/stream/image error values), JSON/native-value
//! conversions, field read/write, and deep copy. Split out of `reg_vm/mod.rs`.

use std::rc::Rc;

use crate::eval_types::{EvalError, NativeValue};
use crate::vm_value::{FieldMap, ValueMap, VmMapKey, VmStruct, VmValue};

use super::*;

pub(super) fn regex_value(pattern: impl Into<String>) -> VmValue {
    let mut fields = FieldMap::default();
    fields.insert("pattern".to_string(), VmValue::string(pattern.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Regex"),
        fields,
    }))
}

pub(super) fn regex_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = FieldMap::default();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("RegexError"),
        fields,
    }))
}

pub(super) fn csv_error_value(message: impl Into<String>) -> VmValue {
    let mut fields = FieldMap::default();
    fields.insert("message".to_string(), VmValue::string(message.into()));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("CsvError"),
        fields,
    }))
}

pub(super) fn row_buffer_value(bytes: Vec<u8>) -> VmValue {
    let mut fields = FieldMap::default();
    fields.insert("bytes".to_string(), VmValue::Bytes(Rc::new(bytes)));
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("RowBuffer"),
        fields,
    }))
}

pub(super) fn row_value(fields: Vec<String>) -> VmValue {
    let mut row_fields = FieldMap::default();
    row_fields.insert(
        "fields".to_string(),
        VmValue::List(Rc::new(RefCell::new(
            fields.into_iter().map(VmValue::string).collect(),
        ))),
    );
    VmValue::Struct(Rc::new(VmStruct {
        name: Rc::from("Row"),
        fields: row_fields,
    }))
}

pub(super) fn csv_parse_row_value(bytes: &[u8]) -> Result<VmValue, VmValue> {
    let text = std::str::from_utf8(bytes).map_err(|error| csv_error_value(error.to_string()))?;
    let Some(line) = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .nth(1)
        .or_else(|| text.lines().map(str::trim).find(|line| !line.is_empty()))
    else {
        return Err(csv_error_value("CSV buffer is empty"));
    };
    Ok(row_value(
        line.split(',')
            .map(|field| field.trim().to_string())
            .collect(),
    ))
}

pub(super) fn csv_rows_stream_value(path: &str) -> Result<VmValue, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("CSV row stream open failed: {error}"))?;
    let mut skipped_header = false;
    let mut rows = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if !skipped_header {
            skipped_header = true;
            continue;
        }
        rows.push(row_value(
            line.split(',')
                .map(|field| field.trim().to_string())
                .collect(),
        ));
    }
    Ok(stream_value(rows))
}

pub(super) fn row_field_string_value(fields: Vec<String>, index: i64) -> Result<VmValue, VmValue> {
    let index = usize::try_from(index).map_err(|_| csv_error_value("negative CSV field index"))?;
    fields
        .get(index)
        .cloned()
        .map(VmValue::string)
        .ok_or_else(|| csv_error_value(format!("CSV field index `{index}` is out of bounds")))
}

pub(super) fn yaml_parse_json_value(text: &str) -> Result<VmValue, VmValue> {
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|error| json_error_value(error.to_string()))?;
    serde_json::to_value(value)
        .map(|value| VmValue::Json(Rc::new(value)))
        .map_err(|error| json_error_value(error.to_string()))
}

pub(super) fn toml_parse_file_value(path: &str) -> Result<VmValue, VmValue> {
    std::fs::read_to_string(path)
        .map_err(|error| json_error_value(error.to_string()))
        .and_then(|text| {
            text.parse::<toml::Value>()
                .map_err(|error| json_error_value(error.to_string()))
        })
        .and_then(|value| {
            serde_json::to_value(value)
                .map(|value| VmValue::Json(Rc::new(value)))
                .map_err(|error| json_error_value(error.to_string()))
        })
}

pub(super) fn split_text_lines(value: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }
    let trimmed = value.strip_suffix('\n').unwrap_or(value);
    trimmed.split('\n').map(ToString::to_string).collect()
}

pub(super) fn diff_unified_string(old: &str, new: &str) -> String {
    if old == new {
        return String::new();
    }
    let old_lines = split_text_lines(old);
    let new_lines = split_text_lines(new);
    let mut out = Vec::new();
    out.push("--- old".to_string());
    out.push("+++ new".to_string());
    out.push(format!(
        "@@ -1,{} +1,{} @@",
        old_lines.len(),
        new_lines.len()
    ));
    for line in &old_lines {
        out.push(format!("-{line}"));
    }
    for line in &new_lines {
        out.push(format!("+{line}"));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

pub(super) fn parse_patch_hunk_old_start(line: &str) -> Result<usize, String> {
    let mut parts = line.split_whitespace();
    let Some("@@") = parts.next() else {
        return Err("malformed hunk header".to_string());
    };
    let Some(old_part) = parts.next() else {
        return Err("hunk header missing old range".to_string());
    };
    if !old_part.starts_with('-') {
        return Err("hunk header old range must start with `-`".to_string());
    }
    old_part[1..]
        .split(',')
        .next()
        .unwrap_or("1")
        .parse::<usize>()
        .map_err(|_| "hunk header old range start must be an integer".to_string())
}

pub(super) fn patch_apply_text_string(original: &str, patch: &str) -> Result<String, String> {
    let original_had_trailing_newline = original.ends_with('\n');
    let original_lines = split_text_lines(original);
    let patch_lines = split_text_lines(patch);
    if patch_lines.is_empty() {
        return Ok(original.to_string());
    }

    let mut output = Vec::new();
    let mut original_index = 0usize;
    let mut patch_index = 0usize;

    while patch_index < patch_lines.len() {
        let line = &patch_lines[patch_index];
        if line.starts_with("--- ") || line.starts_with("+++ ") {
            patch_index += 1;
            continue;
        }
        if !line.starts_with("@@ ") {
            return Err(format!("expected unified diff hunk header, got `{line}`"));
        }
        let old_start = parse_patch_hunk_old_start(line)?;
        let target_index = old_start.saturating_sub(1);
        if target_index < original_index || target_index > original_lines.len() {
            return Err("patch hunk applies outside the original text".to_string());
        }
        while original_index < target_index {
            output.push(original_lines[original_index].clone());
            original_index += 1;
        }
        patch_index += 1;
        while patch_index < patch_lines.len() {
            let hunk_line = &patch_lines[patch_index];
            if hunk_line.starts_with("@@ ")
                || hunk_line.starts_with("--- ")
                || hunk_line.starts_with("+++ ")
            {
                break;
            }
            let Some(prefix) = hunk_line.chars().next() else {
                return Err("empty patch hunk line".to_string());
            };
            let value = hunk_line[1..].to_string();
            match prefix {
                ' ' => {
                    let Some(original_line) = original_lines.get(original_index) else {
                        return Err("patch context extends past original text".to_string());
                    };
                    if original_line != &value {
                        return Err(format!(
                            "patch context mismatch: expected `{}`, got `{value}`",
                            original_line
                        ));
                    }
                    output.push(original_line.clone());
                    original_index += 1;
                }
                '-' => {
                    let Some(original_line) = original_lines.get(original_index) else {
                        return Err("patch removal extends past original text".to_string());
                    };
                    if original_line != &value {
                        return Err(format!(
                            "patch removal mismatch: expected `{}`, got `{value}`",
                            original_line
                        ));
                    }
                    original_index += 1;
                }
                '+' => output.push(value),
                '\\' => {}
                _ => return Err(format!("unsupported patch hunk prefix `{prefix}`")),
            }
            patch_index += 1;
        }
    }

    while original_index < original_lines.len() {
        output.push(original_lines[original_index].clone());
        original_index += 1;
    }

    let mut text = output.join("\n");
    if original_had_trailing_newline || patch.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

pub(super) fn json_result(value: Result<VmValue, VmValue>) -> VmValue {
    match value {
        Ok(value) => value_ok(value),
        Err(error) => value_err(error),
    }
}

pub(super) fn vm_value_to_json_literal(value: &VmValue) -> Result<serde_json::Value, EvalError> {
    match value {
        VmValue::Unit => Ok(serde_json::Value::Null),
        VmValue::Int(value) => Ok(serde_json::Value::Number((*value).into())),
        VmValue::Bool(value) => Ok(serde_json::Value::Bool(*value)),
        VmValue::Char(value) => Ok(serde_json::Value::String(value.to_string())),
        VmValue::Bytes(value) => Ok(serde_json::Value::Array(
            value
                .iter()
                .map(|value| serde_json::Value::Number((*value).into()))
                .collect(),
        )),
        VmValue::String(value) => Ok(serde_json::Value::String(value.to_string())),
        VmValue::Json(value) => Ok(value.as_ref().clone()),
        VmValue::List(items) => items
            .borrow()
            .iter()
            .map(vm_value_to_json_literal)
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        VmValue::Map(entries) => {
            let mut object = serde_json::Map::new();
            for (key, value) in entries.borrow().iter() {
                let VmMapKey::String(key) = key else {
                    return Err(EvalError::Runtime(format!(
                        "cannot convert map key `{}` to a JSON literal.",
                        key.display()
                    )));
                };
                object.insert(key.to_string(), vm_value_to_json_literal(value)?);
            }
            Ok(serde_json::Value::Object(object))
        }
        other => Err(EvalError::Runtime(format!(
            "cannot convert `{}` to a JSON literal.",
            other.display()
        ))),
    }
}

pub(super) fn read_field_ref(value: &VmValue, field: &str) -> Result<VmValue, EvalError> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            data.fields.get(field).cloned().ok_or_else(|| {
                EvalError::Runtime(format!("reg VM struct value is missing field `{field}`."))
            })
        }
        VmValue::Managed(value) => read_field_ref(&value.borrow(), field),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Struct for field `{field}`, got `{}`.",
            other.display()
        ))),
    }
}

/// Return a copy of the struct/variant `value` with `field` set to `new_value`.
/// Structs are value types, so this rebuilds the struct. A `Managed` wrapper is
/// updated in place (its interior is shared and mutable by design).
pub(super) fn write_field_value(
    value: &VmValue,
    field: &str,
    new_value: VmValue,
) -> Result<VmValue, EvalError> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            if !data.fields.contains_key(field) {
                return Err(EvalError::Runtime(format!(
                    "reg VM struct value is missing field `{field}`."
                )));
            }
            let mut fields = data.fields.clone();
            fields.insert(field.to_string(), new_value);
            let updated = Rc::new(VmStruct {
                name: Rc::clone(&data.name),
                fields,
            });
            Ok(match value {
                VmValue::Variant(_) => VmValue::Variant(updated),
                _ => VmValue::Struct(updated),
            })
        }
        VmValue::Managed(inner) => {
            let updated = write_field_value(&inner.borrow(), field, new_value)?;
            *inner.borrow_mut() = updated;
            Ok(VmValue::Managed(Rc::clone(inner)))
        }
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Struct for field `{field}`, got `{}`.",
            other.display()
        ))),
    }
}

pub(super) fn unmanage_vm_value(value: VmValue) -> VmValue {
    match value {
        VmValue::Managed(value) => unmanage_vm_value(value.borrow().clone()),
        other => other,
    }
}

/// Recursively copy a value so the result shares no mutable interior with the
/// original: every `List`/`Map` gets a fresh `Rc<RefCell>` and structs/variants
/// are rebuilt with copied fields. `Managed` is the language's explicit shared
/// reference type, so it keeps its handle (mirroring the backend, which shares
/// `Managed` and treats plain collections as value types). Immutable handles
/// (`String`/`Bytes`/`Json`) and opaque values are cloned shallowly.
pub(super) fn deep_copy_value(value: &VmValue) -> VmValue {
    match value {
        VmValue::List(items) => {
            let copied = items
                .borrow()
                .iter()
                .map(deep_copy_value)
                .collect::<Vec<_>>();
            VmValue::List(Rc::new(RefCell::new(copied)))
        }
        VmValue::Map(entries) => {
            let copied = entries
                .borrow()
                .iter()
                .map(|(key, value)| (key.clone(), deep_copy_value(value)))
                .collect::<ValueMap>();
            VmValue::Map(Rc::new(RefCell::new(copied)))
        }
        VmValue::Struct(data) => VmValue::Struct(deep_copy_struct(data)),
        VmValue::Variant(data) => VmValue::Variant(deep_copy_struct(data)),
        VmValue::OptionSome(inner) => VmValue::OptionSome(Box::new(deep_copy_value(inner))),
        other => other.clone(),
    }
}

pub(super) fn deep_copy_struct(data: &Rc<VmStruct>) -> Rc<VmStruct> {
    let fields = data
        .fields
        .iter()
        .map(|(name, value)| (name.clone(), deep_copy_value(value)))
        .collect::<FieldMap>();
    Rc::new(VmStruct {
        name: Rc::clone(&data.name),
        fields,
    })
}

pub(super) fn native_value_from_vm_value(value: VmValue) -> Result<NativeValue, EvalError> {
    match unmanage_vm_value(value) {
        VmValue::Unit => Ok(NativeValue::Unit),
        VmValue::Int(value) => Ok(NativeValue::Int(value)),
        VmValue::Float(value) => Ok(NativeValue::Float(value)),
        VmValue::Bool(value) => Ok(NativeValue::Bool(value)),
        VmValue::Char(value) => Ok(NativeValue::Char(value)),
        VmValue::Bytes(value) => Ok(NativeValue::Bytes(value.as_ref().clone())),
        VmValue::String(value) => Ok(NativeValue::String(value.to_string())),
        VmValue::Json(value) => Ok(NativeValue::Json(value.as_ref().clone())),
        VmValue::List(items) => items
            .borrow()
            .iter()
            .cloned()
            .map(native_value_from_vm_value)
            .collect::<Result<Vec<_>, _>>()
            .map(NativeValue::List),
        VmValue::Map(entries) => entries
            .borrow()
            .iter()
            .map(|(key, value)| {
                Ok((
                    key.native_value(),
                    native_value_from_vm_value(value.clone())?,
                ))
            })
            .collect::<Result<Vec<_>, EvalError>>()
            .map(NativeValue::Map),
        VmValue::Struct(data) => data
            .fields
            .iter()
            .map(|(field, value)| Ok((field.clone(), native_value_from_vm_value(value.clone())?)))
            .collect::<Result<BTreeMap<_, _>, EvalError>>()
            .map(|fields| NativeValue::Struct {
                name: data.name.to_string(),
                fields,
            }),
        VmValue::Variant(data) => data
            .fields
            .iter()
            .map(|(field, value)| Ok((field.clone(), native_value_from_vm_value(value.clone())?)))
            .collect::<Result<BTreeMap<_, _>, EvalError>>()
            .map(|fields| NativeValue::Variant {
                name: data.name.to_string(),
                fields,
            }),
        VmValue::Native(data) => Ok(NativeValue::Native {
            type_name: data.type_name.to_string(),
            id: data.id,
        }),
        VmValue::Managed(_) => Err(EvalError::Runtime(
            "reg VM native argument stayed managed after unwrapping.".to_string(),
        )),
        // Mirror the return direction: bridge `Option` as a `Some`/`None`
        // variant so native bindings can accept it.
        VmValue::OptionSome(value) => {
            let mut fields = BTreeMap::new();
            fields.insert("value".to_string(), native_value_from_vm_value(*value)?);
            Ok(NativeValue::Variant {
                name: "Some".to_string(),
                fields,
            })
        }
        VmValue::OptionNone => Ok(NativeValue::Variant {
            name: "None".to_string(),
            fields: BTreeMap::new(),
        }),
        VmValue::Closure(_) => Err(EvalError::Runtime(
            "reg VM cannot pass Closure to native host binding.".to_string(),
        )),
    }
}

pub(super) fn vm_value_from_native_value(value: NativeValue) -> VmValue {
    match value {
        NativeValue::Unit => VmValue::Unit,
        NativeValue::Int(value) => VmValue::Int(value),
        NativeValue::Float(value) => VmValue::Float(value),
        NativeValue::Bool(value) => VmValue::Bool(value),
        NativeValue::String(value) => VmValue::string(value),
        NativeValue::Char(value) => VmValue::Char(value),
        NativeValue::Bytes(value) => VmValue::Bytes(Rc::new(value)),
        NativeValue::List(items) => VmValue::List(Rc::new(RefCell::new(
            items.into_iter().map(vm_value_from_native_value).collect(),
        ))),
        NativeValue::Map(entries) => VmValue::Map(Rc::new(RefCell::new(
            entries
                .into_iter()
                .map(|(key, value)| {
                    (
                        vm_map_key_from_native_value(key),
                        vm_value_from_native_value(value),
                    )
                })
                .collect(),
        ))),
        NativeValue::Json(value) => VmValue::Json(Rc::new(value)),
        NativeValue::Struct { name, fields } => VmValue::Struct(Rc::new(VmStruct {
            name: Rc::from(name.as_str()),
            fields: fields
                .into_iter()
                .map(|(field, value)| (field, vm_value_from_native_value(value)))
                .collect(),
        })),
        // `Option` is a dedicated VM value, not a generic variant, so a native
        // binding returning `Some(_)`/`None` must round-trip to `OptionSome`/
        // `OptionNone` for `match`/`?` to recognize it.
        NativeValue::Variant { name, mut fields } if name == "Some" => {
            let value = fields
                .remove("value")
                .map(vm_value_from_native_value)
                .unwrap_or(VmValue::Unit);
            VmValue::OptionSome(Box::new(value))
        }
        NativeValue::Variant { name, .. } if name == "None" => VmValue::OptionNone,
        NativeValue::Variant { name, fields } => VmValue::Variant(Rc::new(VmStruct {
            name: Rc::from(name.as_str()),
            fields: fields
                .into_iter()
                .map(|(field, value)| (field, vm_value_from_native_value(value)))
                .collect(),
        })),
        NativeValue::Native { type_name, id } => VmValue::Native(Rc::new(VmNative {
            type_name: Rc::from(type_name.as_str()),
            id,
        })),
    }
}

pub(super) fn vm_map_key_from_native_value(value: NativeValue) -> VmMapKey {
    match value {
        NativeValue::Bool(value) => VmMapKey::Bool(value),
        NativeValue::Int(value) => VmMapKey::Int(value),
        NativeValue::String(value) => VmMapKey::String(Rc::new(value)),
        other => VmMapKey::String(Rc::new(format!("{other:?}"))),
    }
}
