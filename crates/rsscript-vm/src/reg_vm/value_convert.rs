//! Free helpers that *construct* and *convert* `VmValue`s: typed value
//! constructors (regex/csv/row/stream/image error values), JSON/native-value
//! conversions, field read/write, and deep copy. Split out of `reg_vm/mod.rs`.

use std::collections::HashSet;
use std::rc::Rc;

use crate::eval_types::{EvalError, NativeValue};
use crate::vm_value::{TypedVec, ValueMap, VmMapKey, VmStruct, VmValue, vm_value_node_id};

use super::*;

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
    let value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(text).map_err(|error| json_error_value(error.to_string()))?;
    serde_json::to_value(value)
        .map(|value| VmValue::Json(Rc::new(value)))
        .map_err(|error| json_error_value(error.to_string()))
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

pub(super) fn native_value_from_vm_value(value: VmValue) -> Result<NativeValue, EvalError> {
    native_value_from_vm_value_inner(&value, &mut HashSet::new())
}

fn native_value_from_vm_value_inner(
    value: &VmValue,
    active: &mut HashSet<usize>,
) -> Result<NativeValue, EvalError> {
    let node = vm_value_node_id(value);
    if let Some(node) = node {
        if !active.insert(node) {
            return Err(EvalError::Runtime(
                "cyclic value cannot cross a native binding boundary".to_string(),
            ));
        }
    }

    let converted = match value {
        VmValue::Unit => Ok(NativeValue::Unit),
        VmValue::Int(value) => Ok(NativeValue::Int(*value)),
        VmValue::Float(value) => Ok(NativeValue::Float(*value)),
        VmValue::Bool(value) => Ok(NativeValue::Bool(*value)),
        VmValue::Char(value) => Ok(NativeValue::Char(*value)),
        VmValue::Bytes(value) => Ok(NativeValue::Bytes(value.as_ref().clone())),
        VmValue::String(value) => Ok(NativeValue::String(value.to_string())),
        VmValue::Json(value) => Ok(NativeValue::Json(value.as_ref().clone())),
        VmValue::List(items) => items
            .borrow()
            .iter()
            .map(|value| native_value_from_vm_value_inner(&value, active))
            .collect::<Result<Vec<_>, _>>()
            .map(NativeValue::List),
        VmValue::Deque(items) => items
            .borrow()
            .iter()
            .map(|value| native_value_from_vm_value_inner(value, active))
            .collect::<Result<Vec<_>, _>>()
            .map(NativeValue::List),
        VmValue::Map(entries) => entries
            .borrow()
            .iter()
            .map(|(key, value)| {
                Ok((
                    native_value_from_vm_value_inner(key.value(), active)?,
                    native_value_from_vm_value_inner(value, active)?,
                ))
            })
            .collect::<Result<Vec<_>, EvalError>>()
            .map(NativeValue::Map),
        VmValue::Struct(data) => data
            .iter()
            .map(|(field, value)| {
                Ok((
                    field.to_string(),
                    native_value_from_vm_value_inner(value, active)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, EvalError>>()
            .map(|fields| NativeValue::Struct {
                name: data.name().to_string(),
                fields,
            }),
        VmValue::Variant(data) => data
            .iter()
            .map(|(field, value)| {
                Ok((
                    field.to_string(),
                    native_value_from_vm_value_inner(value, active)?,
                ))
            })
            .collect::<Result<BTreeMap<_, _>, EvalError>>()
            .map(|fields| NativeValue::Variant {
                name: data.name().to_string(),
                fields,
            }),
        VmValue::Native(data) => Ok(NativeValue::Native {
            type_name: data.type_name.to_string(),
            id: data.id,
        }),
        VmValue::Managed(value) => native_value_from_vm_value_inner(&value.borrow(), active),
        // Mirror the return direction: bridge `Option` as a `Some`/`None`
        // variant so native bindings can accept it.
        VmValue::OptionSomeHeap(value) => {
            let mut fields = BTreeMap::new();
            fields.insert(
                "value".to_string(),
                native_value_from_vm_value_inner(value, active)?,
            );
            Ok(NativeValue::Variant {
                name: "Some".to_string(),
                fields,
            })
        }
        VmValue::OptionSomeScalar(scalar) => {
            let mut fields = BTreeMap::new();
            fields.insert(
                "value".to_string(),
                native_value_from_vm_value_inner(&scalar.to_value(), active)?,
            );
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
    };

    if let Some(node) = node {
        active.remove(&node);
    }
    converted
}

pub(super) fn vm_value_from_native_value(value: NativeValue) -> Result<VmValue, EvalError> {
    Ok(match value {
        NativeValue::Unit => VmValue::Unit,
        NativeValue::Int(value) => VmValue::Int(value),
        NativeValue::Float(value) => VmValue::Float(value),
        NativeValue::Bool(value) => VmValue::Bool(value),
        NativeValue::String(value) => VmValue::string(value),
        NativeValue::Char(value) => VmValue::Char(value),
        NativeValue::Bytes(value) => VmValue::Bytes(Rc::new(value)),
        NativeValue::List(items) => VmValue::List(Rc::new(RefCell::new(
            items
                .into_iter()
                .map(vm_value_from_native_value)
                .collect::<Result<_, _>>()?,
        ))),
        NativeValue::Map(entries) => VmValue::Map(Rc::new(RefCell::new(
            entries
                .into_iter()
                .map(|(key, value)| {
                    Ok((
                        vm_map_key_from_native_value(key)?,
                        vm_value_from_native_value(value)?,
                    ))
                })
                .collect::<Result<_, EvalError>>()?,
        ))),
        NativeValue::Json(value) => VmValue::Json(Rc::new(value)),
        NativeValue::Struct { name, fields } => VmValue::Struct(Rc::new(VmStruct::from_named(
            name.as_str(),
            fields
                .into_iter()
                .map(|(field, value)| Ok((field, vm_value_from_native_value(value)?)))
                .collect::<Result<Vec<_>, EvalError>>()?,
        ))),
        // `Option` is a dedicated VM value, not a generic variant, so a native
        // binding returning `Some(_)`/`None` must round-trip to `OptionSome`/
        // `OptionNone` for `match`/`?` to recognize it.
        NativeValue::Variant { name, mut fields } if name == "Some" => {
            let value = fields
                .remove("value")
                .map(vm_value_from_native_value)
                .transpose()?
                .unwrap_or(VmValue::Unit);
            VmValue::some(value)
        }
        NativeValue::Variant { name, .. } if name == "None" => VmValue::OptionNone,
        NativeValue::Variant { name, fields } => VmValue::Variant(Rc::new(VmStruct::from_named(
            name.as_str(),
            fields
                .into_iter()
                .map(|(field, value)| Ok((field, vm_value_from_native_value(value)?)))
                .collect::<Result<Vec<_>, EvalError>>()?,
        ))),
        NativeValue::Native { type_name, id } => VmValue::Native(Rc::new(VmNative {
            type_name: Rc::from(type_name.as_str()),
            id,
        })),
    })
}

pub(super) fn vm_map_key_from_native_value(value: NativeValue) -> Result<VmMapKey, EvalError> {
    let value = vm_value_from_native_value(value)?;
    map_key_from_value(&value)
        .map(|(key, _work)| key)
        .map_err(|error| match error {
            EvalError::Runtime(message) => {
                EvalError::Runtime(format!("invalid native Map key: {message}"))
            }
            other => other,
        })
}

#[cfg(test)]
mod tests {
    //! VM <-> native value marshalling: every variant in both directions, the
    //! `Option` bridge, managed-unwrap, the closure rejection, and strict map-key
    //! validation. This is the host boundary, so a missed arm here is a silently
    //! wrong value at a native call.
    use super::*;

    #[test]
    fn native_conversion_rejects_cyclic_values() {
        let cell = Rc::new(RefCell::new(VmValue::Unit));
        let managed = VmValue::Managed(Rc::clone(&cell));
        *cell.borrow_mut() = VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(vec![
            managed.clone(),
        ]))));

        let error = native_value_from_vm_value(managed).expect_err("cycle must be rejected");
        assert!(matches!(error, EvalError::Runtime(message) if message.contains("cyclic value")));
    }
    use crate::vm_value::{VmClosure, VmNative};
    use std::collections::{BTreeMap, VecDeque};

    fn to_native(value: VmValue) -> NativeValue {
        native_value_from_vm_value(value).expect("conversion should succeed")
    }

    #[test]
    fn vm_to_native_scalars() {
        assert_eq!(to_native(VmValue::Unit), NativeValue::Unit);
        assert_eq!(to_native(VmValue::Int(7)), NativeValue::Int(7));
        assert_eq!(to_native(VmValue::Float(1.5)), NativeValue::Float(1.5));
        assert_eq!(to_native(VmValue::Bool(true)), NativeValue::Bool(true));
        assert_eq!(to_native(VmValue::Char('x')), NativeValue::Char('x'));
        assert_eq!(
            to_native(VmValue::string("hi")),
            NativeValue::String("hi".to_string())
        );
        assert_eq!(
            to_native(VmValue::Bytes(Rc::new(vec![1, 2, 3]))),
            NativeValue::Bytes(vec![1, 2, 3])
        );
        assert_eq!(
            to_native(VmValue::Json(Rc::new(serde_json::json!({"a": 1})))),
            NativeValue::Json(serde_json::json!({"a": 1}))
        );
    }

    #[test]
    // `VmMapKey` wraps `VmValue`, whose collection variants use interior
    // mutability; that is the key type by design, so the lint does not apply.
    #[allow(clippy::mutable_key_type)]
    fn vm_to_native_collections() {
        let list = VmValue::List(Rc::new(RefCell::new(TypedVec::from_values(vec![
            VmValue::Int(1),
            VmValue::Int(2),
        ]))));
        assert_eq!(
            to_native(list),
            NativeValue::List(vec![NativeValue::Int(1), NativeValue::Int(2)])
        );

        // `Deque` also marshals to a native list.
        let deque = VmValue::Deque(Rc::new(RefCell::new(VecDeque::from(vec![VmValue::Int(9)]))));
        assert_eq!(
            to_native(deque),
            NativeValue::List(vec![NativeValue::Int(9)])
        );

        let mut map: ValueMap = ValueMap::default();
        map.insert(VmMapKey::new(VmValue::Int(1)), VmValue::string("one"));
        assert_eq!(
            to_native(VmValue::Map(Rc::new(RefCell::new(map)))),
            NativeValue::Map(vec![(
                NativeValue::Int(1),
                NativeValue::String("one".to_string())
            )])
        );
    }

    #[test]
    fn vm_to_native_struct_variant_native() {
        let s = VmValue::Struct(Rc::new(VmStruct::from_named(
            "Point",
            [("x", VmValue::Int(1)), ("y", VmValue::Int(2))],
        )));
        match to_native(s) {
            NativeValue::Struct { name, fields } => {
                assert_eq!(name, "Point");
                assert_eq!(fields["x"], NativeValue::Int(1));
                assert_eq!(fields["y"], NativeValue::Int(2));
            }
            other => panic!("expected struct, got {other:?}"),
        }

        let v = VmValue::Variant(Rc::new(VmStruct::from_named(
            "Tag",
            [("v", VmValue::Bool(true))],
        )));
        match to_native(v) {
            NativeValue::Variant { name, fields } => {
                assert_eq!(name, "Tag");
                assert_eq!(fields["v"], NativeValue::Bool(true));
            }
            other => panic!("expected variant, got {other:?}"),
        }

        let native = VmValue::Native(Rc::new(VmNative {
            type_name: Rc::from("Handle"),
            id: 42,
        }));
        assert_eq!(
            to_native(native),
            NativeValue::Native {
                type_name: "Handle".to_string(),
                id: 42
            }
        );
    }

    #[test]
    fn vm_to_native_bridges_option() {
        match to_native(VmValue::some(VmValue::Int(5))) {
            NativeValue::Variant { name, fields } => {
                assert_eq!(name, "Some");
                assert_eq!(fields["value"], NativeValue::Int(5));
            }
            other => panic!("expected Some variant, got {other:?}"),
        }
        match to_native(VmValue::OptionNone) {
            NativeValue::Variant { name, fields } => {
                assert_eq!(name, "None");
                assert!(fields.is_empty());
            }
            other => panic!("expected None variant, got {other:?}"),
        }
    }

    #[test]
    fn vm_to_native_unwraps_managed() {
        let managed = VmValue::Managed(Rc::new(RefCell::new(VmValue::Int(11))));
        assert_eq!(to_native(managed), NativeValue::Int(11));
    }

    #[test]
    fn vm_to_native_rejects_closure() {
        let closure = VmValue::Closure(Rc::new(VmClosure {
            function: 0,
            captures: Vec::new(),
        }));
        match native_value_from_vm_value(closure) {
            Err(EvalError::Runtime(message)) => assert!(message.contains("Closure"), "{message}"),
            other => panic!("expected closure rejection, got {other:?}"),
        }
    }

    #[test]
    fn native_to_vm_round_trips_every_variant() {
        // A native list carrying one of each variant; round-tripping the whole
        // thing exercises every arm of `vm_value_from_native_value` and the
        // matching arm of `native_value_from_vm_value`. Single-entry map keeps
        // ordering stable across the HashMap hop.
        let mut struct_fields = BTreeMap::new();
        struct_fields.insert("a".to_string(), NativeValue::Int(1));
        let mut variant_fields = BTreeMap::new();
        variant_fields.insert("b".to_string(), NativeValue::Bool(true));

        let native = NativeValue::List(vec![
            NativeValue::Unit,
            NativeValue::Int(1),
            NativeValue::Float(2.5),
            NativeValue::Bool(false),
            NativeValue::Char('q'),
            NativeValue::String("s".to_string()),
            NativeValue::Bytes(vec![7, 8]),
            NativeValue::Json(serde_json::json!([1, 2])),
            NativeValue::Map(vec![(
                NativeValue::Int(3),
                NativeValue::String("v".to_string()),
            )]),
            NativeValue::Struct {
                name: "S".to_string(),
                fields: struct_fields,
            },
            NativeValue::Variant {
                name: "Custom".to_string(),
                fields: variant_fields,
            },
            NativeValue::Native {
                type_name: "H".to_string(),
                id: 5,
            },
        ]);

        let back = native_value_from_vm_value(
            vm_value_from_native_value(native.clone()).expect("native value should be valid"),
        )
        .expect("round trip should succeed");
        assert_eq!(native, back);
    }

    #[test]
    fn native_to_vm_bridges_option_variants() {
        let mut some_fields = BTreeMap::new();
        some_fields.insert("value".to_string(), NativeValue::Int(8));
        assert!(matches!(
            vm_value_from_native_value(NativeValue::Variant {
                name: "Some".to_string(),
                fields: some_fields,
            }),
            Ok(VmValue::OptionSomeScalar(_))
        ));
        assert!(matches!(
            vm_value_from_native_value(NativeValue::Variant {
                name: "None".to_string(),
                fields: BTreeMap::new(),
            }),
            Ok(VmValue::OptionNone)
        ));
    }

    #[test]
    fn map_key_from_native_preserves_hashable_types_and_rejects_float() {
        assert_eq!(
            vm_map_key_from_native_value(NativeValue::Bool(true)).unwrap(),
            VmMapKey::new(VmValue::Bool(true))
        );
        assert_eq!(
            vm_map_key_from_native_value(NativeValue::Int(2)).unwrap(),
            VmMapKey::new(VmValue::Int(2))
        );
        assert_eq!(
            vm_map_key_from_native_value(NativeValue::String("k".to_string())).unwrap(),
            VmMapKey::from_string("k")
        );
        assert_eq!(
            vm_map_key_from_native_value(NativeValue::Char('k')).unwrap(),
            VmMapKey::new(VmValue::Char('k'))
        );
        let mut fields = BTreeMap::new();
        fields.insert("id".to_string(), NativeValue::Int(7));
        assert!(matches!(
            vm_map_key_from_native_value(NativeValue::Struct {
                name: "Key".to_string(),
                fields,
            })
            .unwrap()
            .value(),
            VmValue::Struct(data) if data.name().as_ref() == "Key"
        ));
        let mut variant_fields = BTreeMap::new();
        variant_fields.insert("id".to_string(), NativeValue::Int(8));
        assert!(matches!(
            vm_map_key_from_native_value(NativeValue::Variant {
                name: "KeyTag".to_string(),
                fields: variant_fields,
            })
            .unwrap()
            .value(),
            VmValue::Variant(data) if data.name().as_ref() == "KeyTag"
        ));
        assert!(vm_map_key_from_native_value(NativeValue::Float(1.0)).is_err());
    }
}
