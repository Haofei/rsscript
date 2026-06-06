use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::eval_types::NativeValue;

#[derive(Debug, Clone)]
pub(crate) enum VmValue {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    Bytes(Rc<Vec<u8>>),
    String(Rc<String>),
    Json(Rc<serde_json::Value>),
    List(Rc<RefCell<Vec<VmValue>>>),
    Map(Rc<RefCell<HashMap<VmMapKey, VmValue>>>),
    OptionSome(Box<VmValue>),
    OptionNone,
    Struct(Rc<VmStruct>),
    Variant(Rc<VmStruct>),
    Native(Rc<VmNative>),
    Managed(Rc<RefCell<VmValue>>),
    Closure(Rc<VmClosure>),
}

#[derive(Debug, Clone)]
pub(crate) struct VmStruct {
    pub(crate) name: Rc<str>,
    pub(crate) fields: HashMap<String, VmValue>,
}

#[derive(Debug, Clone)]
pub(crate) struct VmClosure {
    pub(crate) function: usize,
    pub(crate) captures: Vec<VmValue>,
}

#[derive(Debug, Clone)]
pub(crate) struct VmNative {
    pub(crate) type_name: Rc<str>,
    pub(crate) id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum VmMapKey {
    Bool(bool),
    Int(i64),
    String(Rc<String>),
}

impl VmMapKey {
    pub(crate) fn display(&self) -> String {
        match self {
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::String(value) => value.to_string(),
        }
    }

    pub(crate) fn native_value(&self) -> NativeValue {
        match self {
            Self::Bool(value) => NativeValue::Bool(*value),
            Self::Int(value) => NativeValue::Int(*value),
            Self::String(value) => NativeValue::String(value.to_string()),
        }
    }
}

impl VmValue {
    pub(crate) fn string(value: impl Into<String>) -> Self {
        Self::String(Rc::new(value.into()))
    }

    pub(crate) fn display(&self) -> String {
        match self {
            Self::Unit => "Unit".to_string(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Char(value) => value.to_string(),
            Self::Bytes(value) => format!("{value:?}"),
            Self::String(value) => value.to_string(),
            Self::Json(value) => {
                serde_json::to_string(value.as_ref()).unwrap_or_else(|_| "<json>".to_string())
            }
            Self::List(values) => {
                let values = values
                    .borrow()
                    .iter()
                    .map(VmValue::display)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{values}]")
            }
            Self::Map(entries) => {
                let mut values = entries
                    .borrow()
                    .iter()
                    .map(|(key, value)| format!("{}: {}", key.display(), value.display()))
                    .collect::<Vec<_>>();
                values.sort();
                let values = values.join(", ");
                format!("{{{values}}}")
            }
            Self::OptionSome(value) => format!("Some({})", value.display()),
            Self::OptionNone => "None".to_string(),
            Self::Struct(data) | Self::Variant(data) => {
                if data.fields.is_empty() {
                    return data.name.to_string();
                }
                let mut values = data
                    .fields
                    .iter()
                    .map(|(key, value)| format!("{key}: {}", value.display()))
                    .collect::<Vec<_>>();
                values.sort();
                let values = values.join(", ");
                format!("{} {{ {values} }}", data.name)
            }
            Self::Native(data) => format!("<native {}#{}>", data.type_name, data.id),
            Self::Managed(value) => value.borrow().display(),
            Self::Closure(_) => "<closure>".to_string(),
        }
    }

    pub(crate) fn native_value(&self) -> Option<NativeValue> {
        match self {
            Self::Unit => Some(NativeValue::Unit),
            Self::Int(value) => Some(NativeValue::Int(*value)),
            Self::Float(value) => Some(NativeValue::Float(*value)),
            Self::Bool(value) => Some(NativeValue::Bool(*value)),
            Self::Char(value) => Some(NativeValue::Char(*value)),
            Self::Bytes(value) => Some(NativeValue::Bytes(value.as_ref().clone())),
            Self::String(value) => Some(NativeValue::String(value.to_string())),
            Self::Json(value) => Some(NativeValue::Json(value.as_ref().clone())),
            Self::List(values) => values
                .borrow()
                .iter()
                .map(VmValue::native_value)
                .collect::<Option<Vec<_>>>()
                .map(NativeValue::List),
            Self::Map(entries) => entries
                .borrow()
                .iter()
                .map(|(key, value)| Some((key.native_value(), value.native_value()?)))
                .collect::<Option<Vec<_>>>()
                .map(NativeValue::Map),
            Self::Struct(data) => native_fields(&data.fields).map(|fields| NativeValue::Struct {
                name: data.name.to_string(),
                fields,
            }),
            Self::Variant(data) => native_fields(&data.fields).map(|fields| NativeValue::Variant {
                name: data.name.to_string(),
                fields,
            }),
            Self::Native(data) => Some(NativeValue::Native {
                type_name: data.type_name.to_string(),
                id: data.id,
            }),
            Self::Managed(value) => value.borrow().native_value(),
            Self::OptionSome(_) | Self::OptionNone | Self::Closure(_) => None,
        }
    }
}

fn native_fields(fields: &HashMap<String, VmValue>) -> Option<BTreeMap<String, NativeValue>> {
    fields
        .iter()
        .map(|(field, value)| Some((field.clone(), value.native_value()?)))
        .collect()
}

impl PartialEq for VmValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unit, Self::Unit) => true,
            (Self::Int(left), Self::Int(right)) => left == right,
            (Self::Float(left), Self::Float(right)) => left.to_bits() == right.to_bits(),
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Char(left), Self::Char(right)) => left == right,
            (Self::Bytes(left), Self::Bytes(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Json(left), Self::Json(right)) => left == right,
            (Self::OptionSome(left), Self::OptionSome(right)) => left == right,
            (Self::OptionNone, Self::OptionNone) => true,
            (Self::List(left), Self::List(right)) => *left.borrow() == *right.borrow(),
            (Self::Map(left), Self::Map(right)) => *left.borrow() == *right.borrow(),
            (Self::Struct(left), Self::Struct(right)) => {
                left.name == right.name && left.fields == right.fields
            }
            (Self::Variant(left), Self::Variant(right)) => {
                left.name == right.name && left.fields == right.fields
            }
            (Self::Native(left), Self::Native(right)) => {
                left.type_name == right.type_name && left.id == right.id
            }
            (Self::Managed(left), Self::Managed(right)) => *left.borrow() == *right.borrow(),
            (Self::Managed(left), right) => *left.borrow() == *right,
            (left, Self::Managed(right)) => *left == *right.borrow(),
            (Self::Closure(left), Self::Closure(right)) => Rc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Eq for VmValue {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_value_representation_stays_compact() {
        assert_eq!(std::mem::size_of::<VmValue>(), 16);
    }
}
