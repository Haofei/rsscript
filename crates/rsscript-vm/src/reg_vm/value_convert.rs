//! Free helpers that *construct* and *convert* `VmValue`s: typed value
//! constructors (regex/csv/row/stream/image error values), JSON/native-value
//! conversions, field read/write, and deep copy. Split out of `reg_vm/mod.rs`.

use std::collections::HashSet;
use std::rc::Rc;

use crate::eval_types::EvalError;
use crate::serde_json;
use crate::vm_value::{TypedVec, ValueMap, VmMapKey, VmStruct, VmValue, vm_value_node_id};
use rsscript_abi_model::{WireCallTypeTable, WireType, WireValue};
use rsscript_provider_api::ResourceHandle;

use super::*;

/// Convert a register value directly into the descriptor-scoped Provider wire
/// representation.  This is the only VM-to-host value boundary: Provider
/// calls must not detour through the retired dynamic `NativeValue` model.
pub(super) fn wire_value_from_vm_value(
    value: VmValue,
    expected: &WireType,
    types: &WireCallTypeTable,
) -> Result<WireValue, EvalError> {
    wire_value_from_vm_value_inner(&value, expected, types, &mut HashSet::new())
}

fn wire_value_from_vm_value_inner(
    value: &VmValue,
    expected: &WireType,
    types: &WireCallTypeTable,
    active: &mut HashSet<usize>,
) -> Result<WireValue, EvalError> {
    let node = vm_value_node_id(value);
    if let Some(node) = node {
        if !active.insert(node) {
            return Err(EvalError::Runtime(
                "cyclic value cannot cross a Provider wire boundary".into(),
            ));
        }
    }
    let result = match (value, expected) {
        (VmValue::Unit, WireType::Unit) => Ok(WireValue::Unit),
        (VmValue::Bool(value), WireType::Bool) => Ok(WireValue::Bool { value: *value }),
        (VmValue::Int(value), WireType::Int { .. }) => Ok(WireValue::Int { value: *value }),
        (VmValue::Float(value), WireType::Float { .. }) => Ok(WireValue::Float { value: *value }),
        (VmValue::String(value), WireType::String) => Ok(WireValue::String {
            value: value.to_string(),
        }),
        (VmValue::Char(value), WireType::Char) => Ok(WireValue::Char { value: *value }),
        (VmValue::Bytes(value), WireType::Bytes) => Ok(WireValue::Bytes {
            value: value.as_ref().clone(),
        }),
        (VmValue::Managed(value), expected) => {
            wire_value_from_vm_value_inner(&value.borrow(), expected, types, active)
        }
        (VmValue::Native(value), resource @ WireType::Resource { name })
            if value.type_name.as_ref() == name =>
        {
            let resource_type = types.resource_type_id(resource).ok_or_else(|| {
                EvalError::Runtime(
                    "linked Provider signature is missing a resource type identity".into(),
                )
            })?;
            Ok(WireValue::Resource {
                handle: ResourceHandle::from_native_id(value.id).to_wire(resource_type),
            })
        }
        (VmValue::List(values), WireType::List { element }) => {
            let element_type = types.type_id(element).ok_or_else(|| {
                EvalError::Runtime(
                    "linked Provider signature is missing a list element identity".into(),
                )
            })?;
            Ok(WireValue::List {
                element_type,
                values: values
                    .borrow()
                    .iter()
                    .map(|value| wire_value_from_vm_value_inner(&value, element, types, active))
                    .collect::<Result<_, _>>()?,
            })
        }
        (VmValue::Deque(values), WireType::List { element }) => {
            let element_type = types.type_id(element).ok_or_else(|| {
                EvalError::Runtime(
                    "linked Provider signature is missing a list element identity".into(),
                )
            })?;
            Ok(WireValue::List {
                element_type,
                values: values
                    .borrow()
                    .iter()
                    .map(|value| wire_value_from_vm_value_inner(value, element, types, active))
                    .collect::<Result<_, _>>()?,
            })
        }
        (VmValue::List(values), WireType::Tuple { elements }) => {
            let values = values.borrow();
            if values.len() != elements.len() {
                return Err(EvalError::Runtime(
                    "Provider tuple argument length does not match its linked signature".into(),
                ));
            }
            Ok(WireValue::Tuple {
                values: values
                    .iter()
                    .zip(elements)
                    .map(|(value, ty)| wire_value_from_vm_value_inner(&value, ty, types, active))
                    .collect::<Result<_, _>>()?,
            })
        }
        (VmValue::Deque(values), WireType::Tuple { elements }) => {
            let values = values.borrow();
            if values.len() != elements.len() {
                return Err(EvalError::Runtime(
                    "Provider tuple argument length does not match its linked signature".into(),
                ));
            }
            Ok(WireValue::Tuple {
                values: values
                    .iter()
                    .zip(elements)
                    .map(|(value, ty)| wire_value_from_vm_value_inner(value, ty, types, active))
                    .collect::<Result<_, _>>()?,
            })
        }
        (VmValue::Map(entries), WireType::Map { key, value }) => {
            let key_type = types.type_id(key).ok_or_else(|| {
                EvalError::Runtime("linked Provider signature is missing a map key identity".into())
            })?;
            let value_type = types.type_id(value).ok_or_else(|| {
                EvalError::Runtime(
                    "linked Provider signature is missing a map value identity".into(),
                )
            })?;
            let entries = entries
                .borrow()
                .iter()
                .map(|(entry_key, entry_value)| {
                    Ok((
                        wire_value_from_vm_value_inner(entry_key.value(), key, types, active)?,
                        wire_value_from_vm_value_inner(entry_value, value, types, active)?,
                    ))
                })
                .collect::<Result<_, EvalError>>()?;
            Ok(WireValue::Map {
                key_type,
                value_type,
                entries,
            })
        }
        (VmValue::Struct(data), named @ WireType::Named { .. }) => {
            let layout = types.record_layout(named).ok_or_else(|| {
                EvalError::Runtime("linked Provider record layout is unavailable".into())
            })?;
            let values = data.iter().collect::<Vec<_>>();
            if data.name().as_ref() != wire_type_name(named) || values.len() != layout.fields.len()
            {
                return Err(EvalError::Runtime(
                    "Provider record argument does not match its linked layout".into(),
                ));
            }
            let fields = values
                .into_iter()
                .zip(&layout.fields)
                .map(|((field, value), layout)| {
                    if field.as_ref() != layout.name {
                        return Err(EvalError::Runtime(
                            "Provider record field order does not match its linked layout".into(),
                        ));
                    }
                    wire_value_from_vm_value_inner(value, &layout.ty, types, active)
                })
                .collect::<Result<_, _>>()?;
            Ok(WireValue::Record {
                type_id: types.type_id(named).ok_or_else(|| {
                    EvalError::Runtime(
                        "linked Provider signature is missing a record identity".into(),
                    )
                })?,
                fields,
            })
        }
        (VmValue::Variant(data), result_type @ WireType::Result { ok, error }) => {
            let (variant_id, payload_type) = match data.name().as_ref() {
                "Ok" => (WireCallTypeTable::result_ok_variant(), ok.as_ref()),
                "Err" => (WireCallTypeTable::result_err_variant(), error.as_ref()),
                _ => {
                    return Err(EvalError::Runtime(
                        "Result value has an unknown variant".into(),
                    ));
                }
            };
            let payload = data
                .get("value")
                .ok_or_else(|| EvalError::Runtime("Result value is missing its payload".into()))?;
            Ok(WireValue::Variant {
                type_id: types.type_id(result_type).ok_or_else(|| {
                    EvalError::Runtime(
                        "linked Provider signature is missing a result identity".into(),
                    )
                })?,
                variant_id,
                payload: Some(Box::new(wire_value_from_vm_value_inner(
                    payload,
                    payload_type,
                    types,
                    active,
                )?)),
            })
        }
        (VmValue::Variant(data), named @ WireType::Named { .. }) => {
            let layout = types.variant_layout(named).ok_or_else(|| {
                EvalError::Runtime("linked Provider variant layout is unavailable".into())
            })?;
            let (index, case) = layout
                .variants
                .iter()
                .enumerate()
                .find(|(_, case)| case.name == data.name().as_ref())
                .ok_or_else(|| {
                    EvalError::Runtime(
                        "Provider variant is not declared by its linked layout".into(),
                    )
                })?;
            let values = data.iter().collect::<Vec<_>>();
            if values.len() != case.fields.len() {
                return Err(EvalError::Runtime(
                    "Provider variant argument does not match its linked layout".into(),
                ));
            }
            let fields = values
                .into_iter()
                .zip(&case.fields)
                .map(|((field, value), layout)| {
                    if field.as_ref() != layout.name {
                        return Err(EvalError::Runtime(
                            "Provider variant field order does not match its linked layout".into(),
                        ));
                    }
                    wire_value_from_vm_value_inner(value, &layout.ty, types, active)
                })
                .collect::<Result<Vec<_>, _>>()?;
            let payload = match fields.len() {
                0 => None,
                1 => Some(Box::new(fields.into_iter().next().expect("one field"))),
                _ => Some(Box::new(WireValue::Tuple { values: fields })),
            };
            Ok(WireValue::Variant {
                type_id: types.type_id(named).ok_or_else(|| {
                    EvalError::Runtime(
                        "linked Provider signature is missing a variant identity".into(),
                    )
                })?,
                variant_id: rsscript_abi_model::WireVariantId::new(index as u32),
                payload,
            })
        }
        (VmValue::OptionSomeHeap(value), option @ WireType::Option { value: element }) => {
            Ok(WireValue::Variant {
                type_id: types.type_id(option).ok_or_else(|| {
                    EvalError::Runtime(
                        "linked Provider signature is missing an option identity".into(),
                    )
                })?,
                variant_id: WireCallTypeTable::option_some_variant(),
                payload: Some(Box::new(wire_value_from_vm_value_inner(
                    value, element, types, active,
                )?)),
            })
        }
        (VmValue::OptionSomeScalar(value), option @ WireType::Option { value: element }) => {
            Ok(WireValue::Variant {
                type_id: types.type_id(option).ok_or_else(|| {
                    EvalError::Runtime(
                        "linked Provider signature is missing an option identity".into(),
                    )
                })?,
                variant_id: WireCallTypeTable::option_some_variant(),
                payload: Some(Box::new(wire_value_from_vm_value_inner(
                    &value.to_value(),
                    element,
                    types,
                    active,
                )?)),
            })
        }
        (VmValue::OptionNone, option @ WireType::Option { .. }) => Ok(WireValue::Variant {
            type_id: types.type_id(option).ok_or_else(|| {
                EvalError::Runtime("linked Provider signature is missing an option identity".into())
            })?,
            variant_id: WireCallTypeTable::option_none_variant(),
            payload: None,
        }),
        (
            value,
            WireType::Qualified {
                value: expected, ..
            },
        ) => wire_value_from_vm_value_inner(value, expected, types, active),
        _ => Err(EvalError::Runtime(
            "VM value does not match its linked Provider wire type".into(),
        )),
    };
    if let Some(node) = node {
        active.remove(&node);
    }
    result
}

pub(super) fn vm_value_from_wire_value(
    value: WireValue,
    expected: &WireType,
    types: &WireCallTypeTable,
) -> Result<VmValue, EvalError> {
    match (value, expected) {
        (WireValue::Unit, WireType::Unit) => Ok(VmValue::Unit),
        (WireValue::Bool { value }, WireType::Bool) => Ok(VmValue::Bool(value)),
        (WireValue::Int { value }, WireType::Int { .. }) => Ok(VmValue::Int(value)),
        (WireValue::Float { value }, WireType::Float { .. }) => Ok(VmValue::Float(value)),
        (WireValue::String { value }, WireType::String) => Ok(VmValue::string(value)),
        (WireValue::Char { value }, WireType::Char) => Ok(VmValue::Char(value)),
        (WireValue::Bytes { value }, WireType::Bytes) => Ok(VmValue::Bytes(Rc::new(value))),
        (WireValue::Resource { handle }, resource @ WireType::Resource { name }) => {
            let expected_id = types.resource_type_id(resource).ok_or_else(|| {
                EvalError::Runtime(
                    "linked Provider signature is missing a resource type identity".into(),
                )
            })?;
            if handle.resource_type != expected_id {
                return Err(EvalError::Runtime(
                    "Provider resource result has the wrong type identity".into(),
                ));
            }
            Ok(VmValue::Native(Rc::new(VmNative {
                type_name: Rc::from(name.as_str()),
                id: ResourceHandle::from_wire(handle).to_native_id(),
            })))
        }
        (
            WireValue::List {
                element_type,
                values,
            },
            WireType::List { element },
        ) => {
            if element_type
                != types.type_id(element).ok_or_else(|| {
                    EvalError::Runtime(
                        "linked Provider signature is missing a list element identity".into(),
                    )
                })?
            {
                return Err(EvalError::Runtime(
                    "Provider list result has the wrong element identity".into(),
                ));
            }
            let values = values
                .into_iter()
                .map(|value| vm_value_from_wire_value(value, element, types))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                values,
            )))))
        }
        (WireValue::Tuple { values }, WireType::Tuple { elements }) => {
            if values.len() != elements.len() {
                return Err(EvalError::Runtime(
                    "Provider tuple result length does not match its linked signature".into(),
                ));
            }
            let values = values
                .into_iter()
                .zip(elements)
                .map(|(value, ty)| vm_value_from_wire_value(value, ty, types))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(
                values,
            )))))
        }
        (
            WireValue::Map {
                key_type,
                value_type,
                entries,
            },
            WireType::Map { key, value },
        ) => {
            if key_type
                != types.type_id(key).ok_or_else(|| {
                    EvalError::Runtime(
                        "linked Provider signature is missing a map key identity".into(),
                    )
                })?
                || value_type
                    != types.type_id(value).ok_or_else(|| {
                        EvalError::Runtime(
                            "linked Provider signature is missing a map value identity".into(),
                        )
                    })?
            {
                return Err(EvalError::Runtime(
                    "Provider map result has the wrong type identity".into(),
                ));
            }
            let entries = entries
                .into_iter()
                .map(|(key_value, value_value)| {
                    let key_value = vm_value_from_wire_value(key_value, key, types)?;
                    Ok((
                        map_key_from_value(&key_value)?.0,
                        vm_value_from_wire_value(value_value, value, types)?,
                    ))
                })
                .collect::<Result<ValueMap, EvalError>>()?;
            Ok(VmValue::Map(Rc::new(RefCell::new(entries))))
        }
        (
            WireValue::Variant {
                type_id,
                variant_id,
                payload: None,
            },
            option @ WireType::Option { .. },
        ) => {
            if type_id
                != types.type_id(option).ok_or_else(|| {
                    EvalError::Runtime(
                        "linked Provider signature is missing an option identity".into(),
                    )
                })?
                || variant_id != WireCallTypeTable::option_none_variant()
            {
                return Err(EvalError::Runtime(
                    "Provider option result has the wrong variant identity".into(),
                ));
            }
            Ok(VmValue::OptionNone)
        }
        (
            WireValue::Variant {
                type_id,
                variant_id,
                payload: Some(payload),
            },
            option @ WireType::Option { value: element },
        ) => {
            if type_id
                != types.type_id(option).ok_or_else(|| {
                    EvalError::Runtime(
                        "linked Provider signature is missing an option identity".into(),
                    )
                })?
                || variant_id != WireCallTypeTable::option_some_variant()
            {
                return Err(EvalError::Runtime(
                    "Provider option result has the wrong variant identity".into(),
                ));
            }
            Ok(VmValue::some(vm_value_from_wire_value(
                *payload, element, types,
            )?))
        }
        (
            WireValue::Variant {
                type_id,
                variant_id,
                payload: Some(payload),
            },
            result_type @ WireType::Result { ok, error },
        ) => {
            let expected_type_id = types.type_id(result_type).ok_or_else(|| {
                EvalError::Runtime("linked Provider signature is missing a result identity".into())
            })?;
            if type_id != expected_type_id {
                return Err(EvalError::Runtime(
                    "Provider result has the wrong type identity".into(),
                ));
            }
            let (name, payload_type) = if variant_id == WireCallTypeTable::result_ok_variant() {
                ("Ok", ok.as_ref())
            } else if variant_id == WireCallTypeTable::result_err_variant() {
                ("Err", error.as_ref())
            } else {
                return Err(EvalError::Runtime(
                    "Provider result has the wrong variant identity".into(),
                ));
            };
            let value = vm_value_from_wire_value(*payload, payload_type, types)?;
            Ok(VmValue::Variant(Rc::new(VmStruct::from_named(
                Rc::from(name),
                [("value", value)],
            ))))
        }
        (
            value,
            WireType::Qualified {
                value: expected, ..
            },
        ) => vm_value_from_wire_value(value, expected, types),
        _ => Err(EvalError::Runtime(
            "Provider wire result does not match its linked signature".into(),
        )),
    }
}

fn wire_type_name(ty: &WireType) -> String {
    match ty {
        WireType::Named {
            package: Some(package),
            name,
            ..
        } => format!("{package}.{name}"),
        WireType::Named {
            package: None,
            name,
            ..
        } => name.clone(),
        _ => String::new(),
    }
}

pub(super) fn regex_value(pattern: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("pattern".to_string(), VmValue::string(pattern.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Regex"), fields)))
}

pub(super) fn regex_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(
        Rc::from("RegexError"),
        fields,
    )))
}

pub(super) fn csv_error_value(message: impl Into<String>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("message".to_string(), VmValue::string(message.into()))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("CsvError"), fields)))
}

pub(super) fn row_buffer_value(bytes: Vec<u8>) -> VmValue {
    let fields: Vec<(String, VmValue)> =
        vec![("bytes".to_string(), VmValue::Bytes(Rc::new(bytes)))];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("RowBuffer"), fields)))
}

pub(super) fn row_value(fields: Vec<String>) -> VmValue {
    let row_fields: Vec<(String, VmValue)> = vec![(
        "fields".to_string(),
        VmValue::List(Rc::new(RefCell::new(
            fields.into_iter().map(VmValue::string).collect(),
        ))),
    )];
    VmValue::Struct(Rc::new(VmStruct::from_named(Rc::from("Row"), row_fields)))
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

pub(super) fn row_field_string_value(fields: Vec<String>, index: i64) -> Result<VmValue, VmValue> {
    let index = usize::try_from(index).map_err(|_| csv_error_value("negative CSV field index"))?;
    fields
        .get(index)
        .cloned()
        .map(VmValue::string)
        .ok_or_else(|| csv_error_value(format!("CSV field index `{index}` is out of bounds")))
}

pub(super) fn yaml_parse_json_value(text: &str) -> Result<VmValue, VmValue> {
    core_yaml_to_json(text)
        .map(|value| VmValue::Json(Rc::new(value)))
        .map_err(json_error_value)
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
            .map(|v| vm_value_to_json_literal(&v))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        VmValue::Map(entries) => {
            let mut object = serde_json::Map::new();
            for (key, value) in entries.borrow().iter() {
                let Some(key) = key.as_str() else {
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
            data.get(field).cloned().ok_or_else(|| {
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

/// Read a struct/variant field by its precomputed slot (no name lookup).
pub(super) fn read_field_slot(value: &VmValue, slot: usize) -> Result<VmValue, EvalError> {
    match value {
        VmValue::Struct(data) | VmValue::Variant(data) => {
            data.fields.get(slot).cloned().ok_or_else(|| {
                EvalError::Runtime(format!("reg VM struct field slot {slot} out of range."))
            })
        }
        VmValue::Managed(value) => read_field_slot(&value.borrow(), slot),
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Struct for field slot {slot}, got `{}`.",
            other.display()
        ))),
    }
}

/// Slot-indexed copy-on-write field set (see [`write_field_value_owned`]).
pub(super) fn write_field_slot_owned(
    value: VmValue,
    slot: usize,
    new_value: VmValue,
) -> Result<VmValue, EvalError> {
    match value {
        VmValue::Struct(mut data) => {
            write_struct_slot_in_place(&mut data, slot, new_value)?;
            Ok(VmValue::Struct(data))
        }
        VmValue::Variant(mut data) => {
            write_struct_slot_in_place(&mut data, slot, new_value)?;
            Ok(VmValue::Variant(data))
        }
        VmValue::Managed(inner) => {
            let current = inner.borrow().clone();
            let updated = write_field_slot_owned(current, slot, new_value)?;
            *inner.borrow_mut() = updated;
            Ok(VmValue::Managed(inner))
        }
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Struct for field slot {slot}, got `{}`.",
            other.display()
        ))),
    }
}

fn write_struct_slot_in_place(
    data: &mut Rc<VmStruct>,
    slot: usize,
    new_value: VmValue,
) -> Result<(), EvalError> {
    if slot >= data.fields.len() {
        return Err(EvalError::Runtime(format!(
            "reg VM struct field slot {slot} out of range."
        )));
    }
    if let Some(unique) = Rc::get_mut(data) {
        unique.fields[slot] = new_value;
        return Ok(());
    }
    let mut fields = data.fields.clone();
    fields[slot] = new_value;
    *data = Rc::new(VmStruct::with_layout(Rc::clone(&data.layout), fields));
    Ok(())
}

/// Return a copy of the struct/variant `value` with `field` set to `new_value`.
/// Structs are value types, so this rebuilds the struct. A `Managed` wrapper is
/// updated in place (its interior is shared and mutable by design).
/// Set `field`, mutating the struct in place when the value is the sole owner of
/// its `Rc` (copy-on-write). A uniquely-owned struct has no other observer, so
/// in-place mutation is observationally identical to rebuilding — but avoids
/// cloning the entire field map and allocating a new `Rc` on every `obj.field =
/// ...` (the dominant cost in `mut`-binding field-write loops). Falls back to
/// clone + rebuild when the `Rc` is shared.
pub(super) fn write_field_value_owned(
    value: VmValue,
    field: &str,
    new_value: VmValue,
) -> Result<VmValue, EvalError> {
    match value {
        VmValue::Struct(mut data) => {
            write_struct_field_in_place(&mut data, field, new_value)?;
            Ok(VmValue::Struct(data))
        }
        VmValue::Variant(mut data) => {
            write_struct_field_in_place(&mut data, field, new_value)?;
            Ok(VmValue::Variant(data))
        }
        VmValue::Managed(inner) => {
            let current = inner.borrow().clone();
            let updated = write_field_value_owned(current, field, new_value)?;
            *inner.borrow_mut() = updated;
            Ok(VmValue::Managed(inner))
        }
        other => Err(EvalError::Runtime(format!(
            "reg VM expected Struct for field `{field}`, got `{}`.",
            other.display()
        ))),
    }
}

fn write_struct_field_in_place(
    data: &mut Rc<VmStruct>,
    field: &str,
    new_value: VmValue,
) -> Result<(), EvalError> {
    let Some(slot) = data.slot(field) else {
        return Err(EvalError::Runtime(format!(
            "reg VM struct value is missing field `{field}`."
        )));
    };
    if let Some(unique) = Rc::get_mut(data) {
        unique.fields[slot] = new_value;
        return Ok(());
    }
    // Shared `Rc`: copy-on-write to preserve value semantics for the other holders
    // (the layout is immutable and shared with them).
    let mut fields = data.fields.clone();
    fields[slot] = new_value;
    *data = Rc::new(VmStruct::with_layout(Rc::clone(&data.layout), fields));
    Ok(())
}

/// Recursively copy a value so the result shares no mutable interior with the
/// original: every `List`/`Map` gets a fresh `Rc<RefCell>` and structs/variants
/// are rebuilt with copied fields. `Managed` is the language's explicit shared
/// reference type, so it keeps its handle (mirroring the backend, which shares
/// `Managed` and treats plain collections as value types). Immutable handles
/// (`String`/`Bytes`/`Json`) and opaque values are cloned shallowly.
// See the note in `runtime_values::json_decode_field_value`: a `VmMapKey` is
// interior-mutable but the `retains(key)` effect makes mutating a live key
// unreachable in well-typed programs.
#[allow(clippy::mutable_key_type)]
pub(super) fn deep_copy_value(value: &VmValue) -> VmValue {
    match value {
        VmValue::List(items) => {
            let copied: TypedVec = items.borrow().iter().map(|v| deep_copy_value(&v)).collect();
            VmValue::List(Rc::new(RefCell::new(copied)))
        }
        VmValue::Deque(values) => {
            let copied = values
                .borrow()
                .iter()
                .map(deep_copy_value)
                .collect::<std::collections::VecDeque<_>>();
            VmValue::Deque(Rc::new(RefCell::new(copied)))
        }
        VmValue::Map(entries) => {
            // Deep-copy BOTH key and value. Keys are not restricted to scalars
            // (`Set<List<Int>>` / `Map<List<Int>, _>` are legal), so cloning a
            // key only bumps its inner `Rc` — leaving the copy's key aliasing the
            // original's. A caller mutating that shared key through a `read`
            // argument would then mutate the source (and corrupt the set/map hash
            // invariant), diverging from the compiled backend. Rebuilding the key
            // from a deep-copied value severs the alias.
            let copied = entries
                .borrow()
                .iter()
                .map(|(key, value)| {
                    (
                        VmMapKey::new(deep_copy_value(key.value())),
                        deep_copy_value(value),
                    )
                })
                .collect::<ValueMap>();
            VmValue::Map(Rc::new(RefCell::new(copied)))
        }
        VmValue::Struct(data) => VmValue::Struct(deep_copy_struct(data)),
        VmValue::Variant(data) => VmValue::Variant(deep_copy_struct(data)),
        VmValue::OptionSomeHeap(inner) => VmValue::some(deep_copy_value(inner)),
        // Inline scalars are `Copy` and immutable — no deep copy needed, and the
        // representation is already canonical.
        VmValue::OptionSomeScalar(_) => value.clone(),
        other => other.clone(),
    }
}

pub(super) fn deep_copy_struct(data: &Rc<VmStruct>) -> Rc<VmStruct> {
    // Share the immutable layout; deep-copy only the values (in slot order).
    let fields = data.fields.iter().map(deep_copy_value).collect();
    Rc::new(VmStruct::with_layout(Rc::clone(&data.layout), fields))
}
