use std::collections::BTreeMap;

use rss_native_abi::NativeValue as AbiValue;
use rss_worker_protocol::NativeValue as WireValue;

pub(crate) fn wire_to_abi(value: WireValue) -> AbiValue {
    match value {
        WireValue::Unit => AbiValue::Unit,
        WireValue::Int(value) => AbiValue::Int(value),
        WireValue::Float(value) => AbiValue::Float(value),
        WireValue::Bool(value) => AbiValue::Bool(value),
        WireValue::String(value) => AbiValue::String(value),
        WireValue::Char(value) => AbiValue::Char(value),
        WireValue::Bytes(value) => AbiValue::Bytes(value),
        WireValue::List(values) => AbiValue::List(values.into_iter().map(wire_to_abi).collect()),
        WireValue::Map(entries) => AbiValue::Map(
            entries
                .into_iter()
                .map(|(key, value)| (wire_to_abi(key), wire_to_abi(value)))
                .collect(),
        ),
        WireValue::Json(value) => AbiValue::Json(value),
        WireValue::Struct { name, fields } => AbiValue::Struct {
            name,
            fields: convert_wire_fields(fields),
        },
        WireValue::Variant { name, fields } => AbiValue::Variant {
            name,
            fields: convert_wire_fields(fields),
        },
        WireValue::Native { type_name, id } => AbiValue::Native { type_name, id },
    }
}

pub(crate) fn abi_to_wire(value: AbiValue) -> WireValue {
    match value {
        AbiValue::Unit => WireValue::Unit,
        AbiValue::Int(value) => WireValue::Int(value),
        AbiValue::Float(value) => WireValue::Float(value),
        AbiValue::Bool(value) => WireValue::Bool(value),
        AbiValue::String(value) => WireValue::String(value),
        AbiValue::Char(value) => WireValue::Char(value),
        AbiValue::Bytes(value) => WireValue::Bytes(value),
        AbiValue::List(values) => WireValue::List(values.into_iter().map(abi_to_wire).collect()),
        AbiValue::Map(entries) => WireValue::Map(
            entries
                .into_iter()
                .map(|(key, value)| (abi_to_wire(key), abi_to_wire(value)))
                .collect(),
        ),
        AbiValue::Json(value) => WireValue::Json(value),
        AbiValue::Struct { name, fields } => WireValue::Struct {
            name,
            fields: convert_abi_fields(fields),
        },
        AbiValue::Variant { name, fields } => WireValue::Variant {
            name,
            fields: convert_abi_fields(fields),
        },
        AbiValue::Native { type_name, id } => WireValue::Native { type_name, id },
    }
}

fn convert_wire_fields(fields: BTreeMap<String, WireValue>) -> BTreeMap<String, AbiValue> {
    fields
        .into_iter()
        .map(|(name, value)| (name, wire_to_abi(value)))
        .collect()
}

fn convert_abi_fields(fields: BTreeMap<String, AbiValue>) -> BTreeMap<String, WireValue> {
    fields
        .into_iter()
        .map(|(name, value)| (name, abi_to_wire(value)))
        .collect()
}
