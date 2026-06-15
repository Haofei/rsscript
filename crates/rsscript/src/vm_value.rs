use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::eval_types::NativeValue;

/// FNV-1a (64-bit) hasher. The standard library's `HashMap` uses a *randomly
/// seeded* SipHash, which is DoS-resistant but (a) slow for the short keys the VM
/// hashes constantly (struct field names, small map keys) and (b) gives a
/// run-to-run *random* iteration order. FNV is far faster and, being fixed-seed,
/// deterministic — and `Map.keys()`/`Map.values()` expose iteration order directly,
/// so a stable order is a correctness requirement, not just a nicety (the backend
/// differential and reproducible review output both depend on it).
///
/// The trade-off is that a fixed-seed hash is, by construction, vulnerable to
/// worst-case collision flooding: an adversary who controls map *keys* can force
/// O(n) lookups. That is accepted because RSScript's VM is a local execution and
/// review tool — the program author controls the workload, not a remote attacker —
/// and DoS-resistance is mutually exclusive with the deterministic iteration order
/// we require here. If a VM `Map` ever backs an adversary-facing surface (e.g. a
/// long-lived server keying a map on untrusted request data), that surface needs
/// its own bounded/ordered structure rather than relying on this hasher.
#[derive(Clone, Copy)]
pub(crate) struct FnvHasher(u64);

impl Default for FnvHasher {
    fn default() -> Self {
        FnvHasher(0xcbf2_9ce4_8422_2325)
    }
}

impl std::hash::Hasher for FnvHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut hash = self.0;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = hash;
    }
}

pub(crate) type FnvBuildHasher = std::hash::BuildHasherDefault<FnvHasher>;
/// VM `Map` value (key → value), FNV-hashed.
pub(crate) type ValueMap = HashMap<VmMapKey, VmValue, FnvBuildHasher>;

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
    /// `Deque<T>` — a double-ended queue with O(1) front/back push/pop, unlike a
    /// `Vec`-backed list whose front ops are O(n).
    Deque(Rc<RefCell<std::collections::VecDeque<VmValue>>>),
    Map(Rc<RefCell<ValueMap>>),
    OptionSome(Box<VmValue>),
    OptionNone,
    Struct(Rc<VmStruct>),
    Variant(Rc<VmStruct>),
    Native(Rc<VmNative>),
    Managed(Rc<RefCell<VmValue>>),
    Closure(Rc<VmClosure>),
}

/// The ordered field names of a struct/variant type, shared (via `Rc`) across all
/// instances so the names aren't duplicated per instance and field access can be
/// an offset index rather than a string hash.
#[derive(Debug)]
pub(crate) struct StructLayout {
    pub(crate) field_names: Vec<Rc<str>>,
}

impl StructLayout {
    pub(crate) fn new(field_names: Vec<Rc<str>>) -> Self {
        StructLayout { field_names }
    }

    /// The slot of `field`, or `None` if absent. A linear scan — struct field
    /// counts are tiny, so this beats hashing, and the hot path uses precomputed
    /// slots (`GetFieldSlot`) from the lowerer anyway.
    pub(crate) fn slot(&self, field: &str) -> Option<usize> {
        self.field_names.iter().position(|name| &**name == field)
    }
}

#[derive(Debug, Clone)]
pub(crate) struct VmStruct {
    pub(crate) name: Rc<str>,
    pub(crate) layout: Rc<StructLayout>,
    pub(crate) fields: Vec<VmValue>,
}

impl VmStruct {
    /// Build a struct/variant from named field/value pairs (the field order
    /// becomes the slot order). Allocates a fresh layout; callers that share a
    /// type's layout reuse it via [`VmStruct::with_layout`].
    pub(crate) fn from_named<K: Into<Rc<str>>>(
        name: impl Into<Rc<str>>,
        fields: impl IntoIterator<Item = (K, VmValue)>,
    ) -> Self {
        let (field_names, values): (Vec<Rc<str>>, Vec<VmValue>) = fields
            .into_iter()
            .map(|(key, value)| (key.into(), value))
            .unzip();
        VmStruct {
            name: name.into(),
            layout: Rc::new(StructLayout::new(field_names)),
            fields: values,
        }
    }

    pub(crate) fn with_layout(
        name: Rc<str>,
        layout: Rc<StructLayout>,
        fields: Vec<VmValue>,
    ) -> Self {
        VmStruct {
            name,
            layout,
            fields,
        }
    }

    pub(crate) fn slot(&self, field: &str) -> Option<usize> {
        self.layout.slot(field)
    }

    pub(crate) fn get(&self, field: &str) -> Option<&VmValue> {
        self.layout
            .slot(field)
            .and_then(|slot| self.fields.get(slot))
    }

    pub(crate) fn contains(&self, field: &str) -> bool {
        self.layout.slot(field).is_some()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Iterate `(field_name, value)` pairs in slot order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Rc<str>, &VmValue)> {
        self.layout.field_names.iter().zip(self.fields.iter())
    }
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

/// A `Map`/`Set` key: any hashable `VmValue`. Keys are not restricted to scalars
/// — RSScript's `Hashable` bound also admits `derives(Eq, Hash)` structs/sums and
/// the structural `List`/`Option` containers over hashable types, all of which
/// the compiled backend keys on directly. Equality reuses `VmValue`'s structural
/// `PartialEq`; `Hash` is a matching recursive projection (see [`hash_vm_value`]),
/// so equal keys hash equal and the VM stays in lockstep with the derived
/// `Hash`/`Eq` of the lowered Rust.
#[derive(Debug, Clone)]
pub(crate) struct VmMapKey(VmValue);

impl VmMapKey {
    pub(crate) fn new(value: VmValue) -> Self {
        VmMapKey(value)
    }

    pub(crate) fn from_string(value: impl Into<String>) -> Self {
        VmMapKey(VmValue::string(value))
    }

    /// The underlying value, returned as-is by `Map.keys()` / `Set.to_list()`.
    pub(crate) fn value(&self) -> &VmValue {
        &self.0
    }

    /// The key's string contents, when it is a `String` key (e.g. JSON object
    /// keys must be strings).
    pub(crate) fn as_str(&self) -> Option<&str> {
        match &self.0 {
            VmValue::String(value) => Some(value),
            _ => None,
        }
    }

    pub(crate) fn display(&self) -> String {
        self.0.display()
    }

    pub(crate) fn native_value(&self) -> NativeValue {
        // Every hashable key has a native form; the string fallback only covers
        // shapes the host ABI lacks a slot for (e.g. an `Option` key), keeping
        // this total rather than panicking on the boundary.
        self.0
            .native_value()
            .unwrap_or_else(|| NativeValue::String(self.0.display()))
    }
}

impl PartialEq for VmMapKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for VmMapKey {}

impl std::hash::Hash for VmMapKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        hash_vm_value(&self.0, state);
    }
}

/// Recursively hash a `VmValue` consistently with [`VmValue`]'s `PartialEq`:
/// equal values must hash equal. `Managed` is transparent in equality, so it is
/// unwrapped here *before* the discriminant is mixed in (a `Managed(Int(1))`
/// must hash like `Int(1)`). `Float` keys cannot occur (not `Hashable`), but the
/// ±0.0 case is normalized anyway so the function is a correct `Hash` for any
/// `VmValue`.
fn hash_vm_value<H: std::hash::Hasher>(value: &VmValue, state: &mut H) {
    use std::hash::Hash;

    if let VmValue::Managed(inner) = value {
        hash_vm_value(&inner.borrow(), state);
        return;
    }

    std::mem::discriminant(value).hash(state);
    match value {
        VmValue::Unit | VmValue::OptionNone => {}
        VmValue::Bool(value) => value.hash(state),
        VmValue::Int(value) => value.hash(state),
        VmValue::Char(value) => value.hash(state),
        VmValue::String(value) => value.hash(state),
        VmValue::Bytes(value) => value.hash(state),
        VmValue::Float(value) => {
            let bits = if *value == 0.0 { 0 } else { value.to_bits() };
            bits.hash(state);
        }
        VmValue::Json(value) => value.to_string().hash(state),
        VmValue::List(items) => {
            let items = items.borrow();
            items.len().hash(state);
            for item in items.iter() {
                hash_vm_value(item, state);
            }
        }
        VmValue::Deque(items) => {
            let items = items.borrow();
            items.len().hash(state);
            for item in items.iter() {
                hash_vm_value(item, state);
            }
        }
        VmValue::OptionSome(inner) => hash_vm_value(inner, state),
        VmValue::Struct(data) | VmValue::Variant(data) => {
            data.name.hash(state);
            for field in &data.fields {
                hash_vm_value(field, state);
            }
        }
        VmValue::Native(data) => {
            data.type_name.hash(state);
            data.id.hash(state);
        }
        // Not a hashable key shape (the checker rejects `Map`/closure keys), but
        // stay total: an order-independent fold so equal maps hash equally.
        VmValue::Map(entries) => {
            let mut acc: u64 = 0;
            for (key, value) in entries.borrow().iter() {
                let mut hasher = FnvHasher::default();
                key.hash(&mut hasher);
                hash_vm_value(value, &mut hasher);
                acc = acc.wrapping_add(std::hash::Hasher::finish(&hasher));
            }
            acc.hash(state);
        }
        VmValue::Closure(closure) => (Rc::as_ptr(closure) as usize).hash(state),
        VmValue::Managed(_) => unreachable!("Managed handled above"),
    }
}

impl VmValue {
    pub(crate) fn string(value: impl Into<String>) -> Self {
        Self::String(Rc::new(value.into()))
    }

    /// Whether this value has value (not reference) semantics and no in-place
    /// mutation — so wrapping it in a `Managed` shared cell would be a semantic
    /// no-op. Used by the `manage` op to avoid leaking an opaque `Managed`
    /// around immutable scalars (`String`/`Bytes`/`Json` are `Rc`-shared and
    /// immutable; the rest are `Copy`).
    pub(crate) fn is_immutable_scalar(&self) -> bool {
        matches!(
            self,
            Self::Unit
                | Self::Int(_)
                | Self::Float(_)
                | Self::Bool(_)
                | Self::Char(_)
                | Self::Bytes(_)
                | Self::String(_)
                | Self::Json(_)
        )
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
            Self::Deque(values) => {
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
                if data.is_empty() {
                    return data.name.to_string();
                }
                let mut values = data
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
            // No `Deque` in the native ABI — a deque crosses the host boundary as
            // a list (the same shape the compiled backend produces).
            Self::Deque(values) => values
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
            Self::Struct(data) => native_fields(data).map(|fields| NativeValue::Struct {
                name: data.name.to_string(),
                fields,
            }),
            Self::Variant(data) => native_fields(data).map(|fields| NativeValue::Variant {
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

fn native_fields(data: &VmStruct) -> Option<BTreeMap<String, NativeValue>> {
    data.iter()
        .map(|(field, value)| Some((field.to_string(), value.native_value()?)))
        .collect()
}

impl PartialEq for VmValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Unit, Self::Unit) => true,
            (Self::Int(left), Self::Int(right)) => left == right,
            // Use IEEE `==` (not bitwise) so the interpreter matches the AOT
            // backend: `NaN == NaN` is false and `0.0 == -0.0` is true. Floats
            // are never used as map keys (see `VmMapKey`), so this does not affect
            // hashing.
            (Self::Float(left), Self::Float(right)) => left == right,
            (Self::Bool(left), Self::Bool(right)) => left == right,
            (Self::Char(left), Self::Char(right)) => left == right,
            (Self::Bytes(left), Self::Bytes(right)) => left == right,
            (Self::String(left), Self::String(right)) => left == right,
            (Self::Json(left), Self::Json(right)) => left == right,
            (Self::OptionSome(left), Self::OptionSome(right)) => left == right,
            (Self::OptionNone, Self::OptionNone) => true,
            (Self::List(left), Self::List(right)) => *left.borrow() == *right.borrow(),
            (Self::Deque(left), Self::Deque(right)) => *left.borrow() == *right.borrow(),
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
