#![forbid(unsafe_code)]

use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Version of the provider/runtime semantic call ABI.
pub const RUNTIME_ABI_VERSION: u32 = 2;
/// Version of the deterministic Core library contract used by bytecode.
///
/// This deliberately changes independently from the Provider/runtime ABI:
/// moving a pure builtin or changing its observable semantics must not be
/// mistaken for a host-call compatibility change.
pub const CORE_LIBRARY_ABI_VERSION: u32 = 1;
/// Language semantics carried by compiled artifacts and neutral analysis.
/// This deliberately does not track any crate/package release version.
pub const LANGUAGE_SEMANTICS_VERSION: &str = "0.1.0";

/// Canonical binary operation identity shared by executable contracts and
/// runtime evaluation. This is semantic identity, not source spelling: syntax
/// parsers and legacy IR projections map their local representations here.
/// Keeping it in the ABI model prevents a VM execution primitive from being
/// owned by the source-shaped compatibility IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    LogicalAnd,
    LogicalOr,
}

macro_rules! wire_id {
    ($name:ident) => {
        /// Opaque, scope-local identity used by a typed ABI value table.
        /// Human-readable names remain in the surrounding descriptor/type
        /// table; dynamic values never carry them as executable identity.
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(u32);

        impl $name {
            pub const fn new(value: u32) -> Self {
                Self(value)
            }

            pub const fn get(self) -> u32 {
                self.0
            }
        }
    };
}

wire_id!(WireTypeId);
wire_id!(WireFieldId);
wire_id!(WireVariantId);
wire_id!(WireResourceTypeId);

/// Numeric identities derived from one linked function signature.
///
/// `WireValue` deliberately transports numeric type and variant identities
/// rather than names.  A full Artifact type table will eventually own those
/// identities for an entire module.  During the compatibility transition,
/// synchronous Provider calls can safely use this smaller table: both the VM
/// adapter and the Provider derive it from the exact same validated function
/// signature.  It is therefore never valid to reuse an ID from one function
/// signature for another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireCallTypeTable {
    types: Vec<WireType>,
    // Resource identities deliberately occupy a table distinct from general
    // value types. A `WireResourceTypeId` must never be mistaken for a
    // `WireTypeId`, even when both happen to have the same numeric value.
    // This table is scoped to one linked function signature just like
    // `types`; the Artifact-wide table is a later boundary.
    resources: Vec<WireType>,
    records: Vec<WireRecordLayout>,
    variants: Vec<WireVariantLayout>,
}

/// Canonical positional layout for one named record in a linked interface
/// scope. The record type itself is present in the enclosing type table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRecordLayout {
    pub ty: WireType,
    pub fields: Vec<WireRecordFieldLayout>,
}

/// One canonical record field. Field names live in the linked descriptor,
/// never in a [`WireValue::Record`]; the positional value payload is decoded
/// against this layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireRecordFieldLayout {
    pub name: String,
    pub ty: WireType,
}

/// Canonical layout for one named sum type in a linked interface scope.
/// Cases retain declaration order, which is the numeric `WireVariantId` used
/// by [`WireValue::Variant`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireVariantLayout {
    pub ty: WireType,
    pub variants: Vec<WireVariantCaseLayout>,
}

/// One sum case. Multiple payload fields are transported as a positional tuple
/// at the wire boundary; a zero-field case has no payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireVariantCaseLayout {
    pub name: String,
    pub fields: Vec<WireRecordFieldLayout>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireTypeTableOverflow;

impl fmt::Display for WireTypeTableOverflow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("wire call type table exceeds u32 identities")
    }
}

impl std::error::Error for WireTypeTableOverflow {}

/// A canonical wire value did not match the descriptor-owned type expected at
/// a Provider boundary. The path is structural and contains no user-supplied
/// executable identity, so callers may safely include it in diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireValueTypeError {
    pub path: String,
    pub message: String,
}

impl fmt::Display for WireValueTypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for WireValueTypeError {}

impl WireCallTypeTable {
    /// Build the deterministic type table for a function call. Parameter
    /// types are visited in declaration order, followed by the result type;
    /// children are assigned before their containing type.
    pub fn for_signature(signature: &FunctionSignature) -> Result<Self, WireTypeTableOverflow> {
        let mut table = Self {
            types: Vec::new(),
            resources: Vec::new(),
            records: Vec::new(),
            variants: Vec::new(),
        };
        for parameter in &signature.parameters {
            table.insert(&parameter.ty)?;
        }
        table.insert(&signature.result)?;
        Ok(table)
    }

    /// Extend one call table with canonical layouts supplied by the linked
    /// interface descriptor. Layout order is canonicalized by record type so
    /// provider and VM callers derive identical numeric identities.
    pub fn with_record_layouts(
        mut self,
        mut records: Vec<WireRecordLayout>,
    ) -> Result<Self, WireTypeTableOverflow> {
        records.sort_by(|left, right| left.ty.cmp(&right.ty));
        for record in &records {
            self.insert(&record.ty)?;
            for field in &record.fields {
                self.insert(&field.ty)?;
            }
        }
        self.records = records;
        Ok(self)
    }

    pub fn record_layout(&self, ty: &WireType) -> Option<&WireRecordLayout> {
        self.records.iter().find(|record| &record.ty == ty)
    }

    /// Extend the table with descriptor-owned public sum layouts. Like record
    /// layouts, canonical type identity is table-owned and never inferred from
    /// a Provider-returned variant name.
    pub fn with_variant_layouts(
        mut self,
        mut variants: Vec<WireVariantLayout>,
    ) -> Result<Self, WireTypeTableOverflow> {
        variants.sort_by(|left, right| left.ty.cmp(&right.ty));
        for layout in &variants {
            self.insert(&layout.ty)?;
            for variant in &layout.variants {
                for field in &variant.fields {
                    self.insert(&field.ty)?;
                }
            }
        }
        self.variants = variants;
        Ok(self)
    }

    pub fn variant_layout(&self, ty: &WireType) -> Option<&WireVariantLayout> {
        self.variants.iter().find(|layout| &layout.ty == ty)
    }

    pub fn variant_case(&self, ty: &WireType, id: WireVariantId) -> Option<&WireVariantCaseLayout> {
        self.variant_layout(ty)?.variants.get(id.get() as usize)
    }

    /// Return the identity for a type present in this signature's table.
    pub fn type_id(&self, ty: &WireType) -> Option<WireTypeId> {
        self.types
            .iter()
            .position(|candidate| candidate == ty)
            .and_then(|index| u32::try_from(index).ok())
            .map(WireTypeId::new)
    }

    /// Return the generation-safe resource-type identity for a resource in
    /// this linked call scope. Qualifiers describe ownership semantics rather
    /// than a distinct resource kind, so they resolve to the wrapped resource
    /// identity.
    pub fn resource_type_id(&self, ty: &WireType) -> Option<WireResourceTypeId> {
        let ty = match ty {
            WireType::Qualified { value, .. } => value.as_ref(),
            other => other,
        };
        self.resources
            .iter()
            .position(|candidate| candidate == ty)
            .and_then(|index| u32::try_from(index).ok())
            .map(WireResourceTypeId::new)
    }

    /// The stable `Some(value)` variant identity for an `Option<T>` in this
    /// table.  The enclosing option type identity is still carried by the
    /// value, so this ordinal cannot be used without it.
    pub const fn option_some_variant() -> WireVariantId {
        WireVariantId::new(0)
    }

    /// The stable `None` variant identity for an `Option<T>` in this table.
    pub const fn option_none_variant() -> WireVariantId {
        WireVariantId::new(1)
    }

    /// The stable `Ok(value)` variant identity for a `Result<T, E>` in this
    /// table. The enclosing result type identity remains mandatory.
    pub const fn result_ok_variant() -> WireVariantId {
        WireVariantId::new(0)
    }

    /// The stable `Err(value)` variant identity for a `Result<T, E>` in this
    /// table. The enclosing result type identity remains mandatory.
    pub const fn result_err_variant() -> WireVariantId {
        WireVariantId::new(1)
    }

    /// Validate one canonical value against the exact linked wire type.
    ///
    /// Numeric record, variant, collection-element, and resource identities
    /// are checked against this call-scoped table. This is deliberately kept
    /// next to the table construction logic so Provider adapters and runtimes
    /// cannot acquire independent notions of wire compatibility.
    pub fn validate_value(
        &self,
        expected: &WireType,
        value: &WireValue,
    ) -> Result<(), WireValueTypeError> {
        self.validate_value_at(expected, value, "value")
    }

    fn validate_value_at(
        &self,
        expected: &WireType,
        value: &WireValue,
        path: &str,
    ) -> Result<(), WireValueTypeError> {
        let mismatch = |message: String| WireValueTypeError {
            path: path.to_string(),
            message,
        };
        match (expected, value) {
            (
                WireType::Qualified {
                    value: expected, ..
                },
                value,
            ) => self.validate_value_at(expected, value, path),
            (WireType::Unit, WireValue::Unit) => Ok(()),
            (WireType::Bool, WireValue::Bool { .. }) => Ok(()),
            (WireType::Int { .. }, WireValue::Int { .. }) => Ok(()),
            (WireType::Float { .. }, WireValue::Float { .. }) => Ok(()),
            (WireType::String, WireValue::String { .. }) => Ok(()),
            (WireType::Char, WireValue::Char { .. }) => Ok(()),
            (WireType::Bytes, WireValue::Bytes { .. }) => Ok(()),
            (
                WireType::List { element },
                WireValue::List {
                    element_type,
                    values,
                },
            ) => {
                let expected_id = self.type_id(element).ok_or_else(|| {
                    mismatch("list element type is absent from the linked type table".into())
                })?;
                if *element_type != expected_id {
                    return Err(mismatch("list element type identity does not match".into()));
                }
                for (index, value) in values.iter().enumerate() {
                    self.validate_value_at(element, value, &format!("{path}[{index}]"))?;
                }
                Ok(())
            }
            (
                WireType::Map {
                    key,
                    value: expected_value,
                },
                WireValue::Map {
                    key_type,
                    value_type,
                    entries,
                },
            ) => {
                if *key_type
                    != self.type_id(key).ok_or_else(|| {
                        mismatch("map key type is absent from the linked type table".into())
                    })?
                    || *value_type
                        != self.type_id(expected_value).ok_or_else(|| {
                            mismatch("map value type is absent from the linked type table".into())
                        })?
                {
                    return Err(mismatch(
                        "map key/value type identity does not match".into(),
                    ));
                }
                for (index, (entry_key, entry_value)) in entries.iter().enumerate() {
                    self.validate_value_at(key, entry_key, &format!("{path}.key[{index}]"))?;
                    self.validate_value_at(
                        expected_value,
                        entry_value,
                        &format!("{path}.value[{index}]"),
                    )?;
                }
                Ok(())
            }
            (WireType::Tuple { elements }, WireValue::Tuple { values }) => {
                if elements.len() != values.len() {
                    return Err(mismatch(format!(
                        "expected {} tuple elements, received {}",
                        elements.len(),
                        values.len()
                    )));
                }
                for (index, (element, value)) in elements.iter().zip(values).enumerate() {
                    self.validate_value_at(element, value, &format!("{path}.{index}"))?;
                }
                Ok(())
            }
            (
                WireType::Option { value: payload },
                WireValue::Variant {
                    type_id,
                    variant_id,
                    payload: actual,
                },
            ) => self.validate_builtin_variant(
                expected,
                *type_id,
                *variant_id,
                actual.as_deref(),
                &[
                    (Self::option_some_variant(), Some(payload.as_ref())),
                    (Self::option_none_variant(), None),
                ],
                path,
            ),
            (
                WireType::Result { ok, error },
                WireValue::Variant {
                    type_id,
                    variant_id,
                    payload,
                },
            ) => self.validate_builtin_variant(
                expected,
                *type_id,
                *variant_id,
                payload.as_deref(),
                &[
                    (Self::result_ok_variant(), Some(ok.as_ref())),
                    (Self::result_err_variant(), Some(error.as_ref())),
                ],
                path,
            ),
            (WireType::Named { .. }, WireValue::Record { type_id, fields })
                if self.record_layout(expected).is_some() =>
            {
                if *type_id
                    != self.type_id(expected).ok_or_else(|| {
                        mismatch("record type is absent from the linked type table".into())
                    })?
                {
                    return Err(mismatch("record type identity does not match".into()));
                }
                let layout = self.record_layout(expected).expect("checked above");
                if layout.fields.len() != fields.len() {
                    return Err(mismatch(format!(
                        "expected {} record fields, received {}",
                        layout.fields.len(),
                        fields.len()
                    )));
                }
                for (index, (field, value)) in layout.fields.iter().zip(fields).enumerate() {
                    self.validate_value_at(
                        &field.ty,
                        value,
                        &format!("{path}.{}[{index}]", field.name),
                    )?;
                }
                Ok(())
            }
            (
                WireType::Named { .. },
                WireValue::Variant {
                    type_id,
                    variant_id,
                    payload,
                },
            ) if self.variant_layout(expected).is_some() => {
                if *type_id
                    != self.type_id(expected).ok_or_else(|| {
                        mismatch("variant type is absent from the linked type table".into())
                    })?
                {
                    return Err(mismatch("variant type identity does not match".into()));
                }
                let case = self.variant_case(expected, *variant_id).ok_or_else(|| {
                    mismatch("variant case identity is outside the linked layout".into())
                })?;
                self.validate_variant_payload(&case.fields, payload.as_deref(), path)
            }
            (WireType::Resource { .. }, WireValue::Resource { handle })
            | (WireType::Handle { .. }, WireValue::Resource { handle }) => {
                let expected_id = self.resource_type_id(expected).ok_or_else(|| {
                    mismatch("resource type is absent from the linked resource table".into())
                })?;
                if handle.resource_type == expected_id {
                    Ok(())
                } else {
                    Err(mismatch("resource type identity does not match".into()))
                }
            }
            _ => Err(mismatch(format!(
                "wire value kind does not match expected type {expected:?}"
            ))),
        }
    }

    fn validate_builtin_variant(
        &self,
        expected: &WireType,
        type_id: WireTypeId,
        variant_id: WireVariantId,
        payload: Option<&WireValue>,
        cases: &[(WireVariantId, Option<&WireType>)],
        path: &str,
    ) -> Result<(), WireValueTypeError> {
        if Some(type_id) != self.type_id(expected) {
            return Err(WireValueTypeError {
                path: path.into(),
                message: "variant type identity does not match".into(),
            });
        }
        let expected_payload = cases
            .iter()
            .find_map(|(id, payload)| (*id == variant_id).then_some(*payload))
            .ok_or_else(|| WireValueTypeError {
                path: path.into(),
                message: "variant case identity is outside the linked layout".into(),
            })?;
        match (expected_payload, payload) {
            (None, None) => Ok(()),
            (Some(expected), Some(value)) => {
                self.validate_value_at(expected, value, &format!("{path}.payload"))
            }
            (None, Some(_)) => Err(WireValueTypeError {
                path: path.into(),
                message: "payload-free variant unexpectedly carries a payload".into(),
            }),
            (Some(_), None) => Err(WireValueTypeError {
                path: path.into(),
                message: "variant payload is missing".into(),
            }),
        }
    }

    fn validate_variant_payload(
        &self,
        fields: &[WireRecordFieldLayout],
        payload: Option<&WireValue>,
        path: &str,
    ) -> Result<(), WireValueTypeError> {
        match (fields, payload) {
            ([], None) => Ok(()),
            ([field], Some(value)) => {
                self.validate_value_at(&field.ty, value, &format!("{path}.{}", field.name))
            }
            (fields, Some(WireValue::Tuple { values })) if fields.len() == values.len() => {
                for (index, (field, value)) in fields.iter().zip(values).enumerate() {
                    self.validate_value_at(
                        &field.ty,
                        value,
                        &format!("{path}.{}[{index}]", field.name),
                    )?;
                }
                Ok(())
            }
            ([], Some(_)) => Err(WireValueTypeError {
                path: path.into(),
                message: "payload-free variant unexpectedly carries a payload".into(),
            }),
            (_, None) => Err(WireValueTypeError {
                path: path.into(),
                message: "variant payload is missing".into(),
            }),
            _ => Err(WireValueTypeError {
                path: path.into(),
                message: "variant payload field count does not match".into(),
            }),
        }
    }

    fn insert(&mut self, ty: &WireType) -> Result<(), WireTypeTableOverflow> {
        match ty {
            WireType::List { element }
            | WireType::Option { value: element }
            | WireType::Qualified { value: element, .. } => self.insert(element)?,
            WireType::Map { key, value } => {
                self.insert(key)?;
                self.insert(value)?;
            }
            WireType::Result { ok, error } => {
                self.insert(ok)?;
                self.insert(error)?;
            }
            WireType::Tuple { elements } => {
                for element in elements {
                    self.insert(element)?;
                }
            }
            WireType::Named { arguments, .. } => {
                for argument in arguments {
                    self.insert(argument)?;
                }
            }
            WireType::Unit
            | WireType::Bool
            | WireType::Int { .. }
            | WireType::Float { .. }
            | WireType::String
            | WireType::Char
            | WireType::Bytes
            | WireType::Resource { .. }
            | WireType::Handle { .. } => {}
        }
        if !self.types.contains(ty) {
            u32::try_from(self.types.len()).map_err(|_| WireTypeTableOverflow)?;
            self.types.push(ty.clone());
        }
        let resource = match ty {
            WireType::Resource { .. } | WireType::Handle { .. } => Some(ty),
            WireType::Qualified { value, .. }
                if matches!(
                    value.as_ref(),
                    WireType::Resource { .. } | WireType::Handle { .. }
                ) =>
            {
                Some(value.as_ref())
            }
            _ => None,
        };
        if let Some(resource) = resource
            && !self.resources.contains(resource)
        {
            u32::try_from(self.resources.len()).map_err(|_| WireTypeTableOverflow)?;
            self.resources.push(resource.clone());
        }
        Ok(())
    }
}

/// A generation-safe resource reference in the canonical Provider wire model.
///
/// The table slot and generation are deliberately numeric: a resource value
/// cannot be forged by spelling a type name or by reusing a stale slot after
/// cleanup. The runtime/provider adapter owns the table that interprets this
/// identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct WireResourceHandle {
    pub resource_type: WireResourceTypeId,
    pub slot: u32,
    pub generation: u32,
}

/// Canonical dynamically transported value for Provider boundaries.
///
/// Unlike the legacy `NativeValue` compatibility representation, records and
/// variants are positional and reference typed table identities; they contain
/// no free-form type or field-name strings. JSON remains a named extension
/// codec at an adapter boundary rather than an implicit escape hatch here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireValue {
    Unit,
    Bool {
        value: bool,
    },
    Int {
        value: i64,
    },
    Float {
        value: f64,
    },
    String {
        value: String,
    },
    Char {
        value: char,
    },
    Bytes {
        value: Vec<u8>,
    },
    List {
        element_type: WireTypeId,
        values: Vec<WireValue>,
    },
    Map {
        key_type: WireTypeId,
        value_type: WireTypeId,
        entries: Vec<(WireValue, WireValue)>,
    },
    Tuple {
        values: Vec<WireValue>,
    },
    Record {
        type_id: WireTypeId,
        fields: Vec<WireValue>,
    },
    Variant {
        type_id: WireTypeId,
        variant_id: WireVariantId,
        payload: Option<Box<WireValue>>,
    },
    Resource {
        handle: WireResourceHandle,
    },
}

impl WireValue {
    /// Deterministic lower-bound accounting for call/request budgets. This is
    /// intentionally independent from a serialization codec so hosts can
    /// enforce limits before choosing an adapter transport.
    pub fn estimated_payload_bytes(&self) -> usize {
        let mut total = 0usize;
        let mut pending = vec![self];
        while let Some(value) = pending.pop() {
            match value {
                Self::Unit => {}
                Self::Bool { .. } => total = total.saturating_add(1),
                Self::Int { .. } | Self::Float { .. } => total = total.saturating_add(8),
                Self::String { value } => total = total.saturating_add(value.len()),
                Self::Char { value } => total = total.saturating_add(value.len_utf8()),
                Self::Bytes { value } => total = total.saturating_add(value.len()),
                Self::List { values, .. } | Self::Tuple { values } => pending.extend(values),
                Self::Map { entries, .. } => {
                    for (key, value) in entries {
                        pending.push(key);
                        pending.push(value);
                    }
                }
                Self::Record { fields, .. } => pending.extend(fields),
                Self::Variant { payload, .. } => pending.extend(payload.as_deref()),
                Self::Resource { .. } => {
                    total = total.saturating_add(std::mem::size_of::<WireResourceHandle>());
                }
            }
        }
        total
    }
}

/// Canonical, serializable type representation used by artifacts and Providers.
/// Semantic arenas may use local IDs internally; those IDs never cross the ABI.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WireType {
    Unit,
    Bool,
    Int {
        bits: u16,
        signed: bool,
    },
    Float {
        bits: u16,
    },
    String,
    Char,
    Bytes,
    List {
        element: Box<WireType>,
    },
    Map {
        key: Box<WireType>,
        value: Box<WireType>,
    },
    Option {
        value: Box<WireType>,
    },
    Result {
        ok: Box<WireType>,
        error: Box<WireType>,
    },
    Tuple {
        elements: Vec<WireType>,
    },
    Named {
        package: Option<String>,
        name: String,
        arguments: Vec<WireType>,
    },
    Resource {
        name: String,
    },
    Handle {
        name: String,
    },
    Qualified {
        qualifier: WireQualifier,
        value: Box<WireType>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireQualifier {
    Fresh,
    Owned,
    NoEscape,
}

impl WireType {
    pub fn parse(source: &str) -> Self {
        let source = source.trim();
        for (prefix, qualifier) in [
            ("fresh ", WireQualifier::Fresh),
            ("owned ", WireQualifier::Owned),
            ("noescape ", WireQualifier::NoEscape),
        ] {
            if let Some(value) = source.strip_prefix(prefix) {
                return Self::Qualified {
                    qualifier,
                    value: Box::new(Self::parse(value)),
                };
            }
        }
        match source {
            "Unit" => return Self::Unit,
            "Bool" => return Self::Bool,
            "Int" => {
                return Self::Int {
                    bits: 64,
                    signed: true,
                };
            }
            "Float" => return Self::Float { bits: 64 },
            "String" => return Self::String,
            "Char" => return Self::Char,
            "Bytes" => return Self::Bytes,
            _ => {}
        }
        if source.starts_with('(') && source.ends_with(')') {
            return Self::Tuple {
                elements: split_type_arguments(&source[1..source.len() - 1])
                    .into_iter()
                    .map(Self::parse)
                    .collect(),
            };
        }
        let (root, arguments) = split_generic(source);
        let arguments = arguments
            .map(split_type_arguments)
            .unwrap_or_default()
            .into_iter()
            .map(Self::parse)
            .collect::<Vec<_>>();
        match (root, arguments.as_slice()) {
            ("List", [element]) => Self::List {
                element: Box::new(element.clone()),
            },
            ("Map", [key, value]) => Self::Map {
                key: Box::new(key.clone()),
                value: Box::new(value.clone()),
            },
            ("Option", [value]) => Self::Option {
                value: Box::new(value.clone()),
            },
            ("Result", [ok, error]) => Self::Result {
                ok: Box::new(ok.clone()),
                error: Box::new(error.clone()),
            },
            ("Resource", []) => Self::Resource {
                name: "Resource".into(),
            },
            ("Handle", []) => Self::Handle {
                name: "Handle".into(),
            },
            _ => {
                let (package, name) = root.rsplit_once('.').map_or_else(
                    || (None, root.to_string()),
                    |(package, name)| (Some(package.to_string()), name.to_string()),
                );
                Self::Named {
                    package,
                    name,
                    arguments,
                }
            }
        }
    }

    fn encode_canonical(&self, output: &mut Vec<u8>) {
        let encoded = serde_json::to_vec(self).expect("WireType serialization cannot fail");
        output.extend_from_slice(&(encoded.len() as u64).to_be_bytes());
        output.extend_from_slice(&encoded);
    }
}

impl From<&str> for WireType {
    fn from(value: &str) -> Self {
        Self::parse(value)
    }
}

impl From<String> for WireType {
    fn from(value: String) -> Self {
        Self::parse(&value)
    }
}

fn split_generic(source: &str) -> (&str, Option<&str>) {
    source
        .find('<')
        .filter(|_| source.ends_with('>'))
        .map_or((source, None), |start| {
            (&source[..start], Some(&source[start + 1..source.len() - 1]))
        })
}

fn split_type_arguments(source: &str) -> Vec<&str> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut arguments = Vec::new();
    for (index, character) in source.char_indices() {
        match character {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                arguments.push(source[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    let tail = source[start..].trim();
    if !tail.is_empty() {
        arguments.push(tail);
    }
    arguments
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalSymbol(String);

impl ExternalSymbol {
    pub fn new(symbol: impl Into<String>) -> Result<Self, InvalidExternalSymbol> {
        let symbol = symbol.into();
        if symbol.is_empty()
            || symbol.starts_with('.')
            || symbol.ends_with('.')
            || symbol.split('.').any(|part| {
                part.is_empty()
                    || !part
                        .chars()
                        .all(|character| character == '_' || character.is_ascii_alphanumeric())
            })
        {
            return Err(InvalidExternalSymbol);
        }
        Ok(Self(symbol))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExternalSymbol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidExternalSymbol;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataEffect {
    Read,
    Mut,
    Take,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParameterSignature {
    pub name: String,
    pub effect: DataEffect,
    pub ty: WireType,
    pub retained: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSignature {
    pub parameters: Vec<ParameterSignature>,
    pub result: WireType,
    pub asynchronous: bool,
}

impl FunctionSignature {
    pub fn hash(&self) -> SignatureHash {
        let mut canonical = Vec::new();
        canonical.extend_from_slice(b"rsscript.semantic_signature.v1\0");
        append_field(
            &mut canonical,
            if self.asynchronous { "async" } else { "sync" },
        );
        self.result.encode_canonical(&mut canonical);
        canonical.extend_from_slice(&(self.parameters.len() as u64).to_be_bytes());
        for parameter in &self.parameters {
            append_field(&mut canonical, &parameter.name);
            append_field(
                &mut canonical,
                match parameter.effect {
                    DataEffect::Read => "read",
                    DataEffect::Mut => "mut",
                    DataEffect::Take => "take",
                },
            );
            parameter.ty.encode_canonical(&mut canonical);
            canonical.push(u8::from(parameter.retained));
        }
        let digest = Sha256::digest(canonical);
        SignatureHash(format!("sha256:{digest:x}"))
    }
}

fn append_field(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SignatureHash(String);

impl SignatureHash {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalImport {
    pub symbol: ExternalSymbol,
    /// Canonical structural ABI retained in the artifact so verification and
    /// inspection do not need compiler-owned type strings or Provider metadata.
    pub signature: FunctionSignature,
    pub signature_hash: SignatureHash,
    pub abi_version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature(effect: DataEffect) -> FunctionSignature {
        FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "message".to_string(),
                effect,
                ty: "String".into(),
                retained: false,
            }],
            result: "Unit".into(),
            asynchronous: false,
        }
    }

    #[test]
    fn signature_hash_is_deterministic_and_semantic() {
        assert_eq!(
            signature(DataEffect::Read).hash(),
            signature(DataEffect::Read).hash()
        );
        assert_ne!(
            signature(DataEffect::Read).hash(),
            signature(DataEffect::Take).hash()
        );
    }

    #[test]
    fn wire_types_parse_nested_structure_without_textual_abi_fields() {
        assert_eq!(
            WireType::parse("Result<List<String>, host.errors.Failure>"),
            WireType::Result {
                ok: Box::new(WireType::List {
                    element: Box::new(WireType::String),
                }),
                error: Box::new(WireType::Named {
                    package: Some("host.errors".into()),
                    name: "Failure".into(),
                    arguments: vec![],
                }),
            }
        );
        assert_eq!(
            WireType::parse("fresh List<Int>"),
            WireType::Qualified {
                qualifier: WireQualifier::Fresh,
                value: Box::new(WireType::List {
                    element: Box::new(WireType::Int {
                        bits: 64,
                        signed: true,
                    }),
                }),
            }
        );
        assert_eq!(
            WireType::parse("Map<String, List<Char>>"),
            WireType::Map {
                key: Box::new(WireType::String),
                value: Box::new(WireType::List {
                    element: Box::new(WireType::Char),
                }),
            }
        );
    }

    #[test]
    fn parameter_names_remain_part_of_named_argument_abi() {
        let mut renamed = signature(DataEffect::Read);
        renamed.parameters[0].name = "text".into();
        assert_ne!(signature(DataEffect::Read).hash(), renamed.hash());
    }

    #[test]
    fn wire_values_use_numeric_type_field_variant_and_resource_identity() {
        let value = WireValue::Record {
            type_id: WireTypeId::new(7),
            fields: vec![WireValue::Variant {
                type_id: WireTypeId::new(8),
                variant_id: WireVariantId::new(2),
                payload: Some(Box::new(WireValue::Resource {
                    handle: WireResourceHandle {
                        resource_type: WireResourceTypeId::new(3),
                        slot: 4,
                        generation: 5,
                    },
                })),
            }],
        };
        let json = serde_json::to_value(&value).expect("wire value serializes");
        let object = json.as_object().expect("record serialization");
        assert_eq!(object["type_id"], 7);
        assert!(object.get("type_name").is_none());
        assert!(object.get("fields").is_some());
        assert_eq!(
            value.estimated_payload_bytes(),
            std::mem::size_of::<WireResourceHandle>()
        );
    }

    #[test]
    fn call_type_table_is_deterministic_and_assigns_children_first() {
        let signature = FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "input".to_string(),
                effect: DataEffect::Read,
                ty: WireType::Option {
                    value: Box::new(WireType::String),
                },
                retained: false,
            }],
            result: WireType::List {
                element: Box::new(WireType::String),
            },
            asynchronous: false,
        };
        let first = WireCallTypeTable::for_signature(&signature).unwrap();
        let second = WireCallTypeTable::for_signature(&signature).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.type_id(&WireType::String), Some(WireTypeId::new(0)));
        assert_eq!(
            first.type_id(&WireType::Option {
                value: Box::new(WireType::String),
            }),
            Some(WireTypeId::new(1))
        );
        assert_eq!(
            first.type_id(&WireType::List {
                element: Box::new(WireType::String),
            }),
            Some(WireTypeId::new(2))
        );
        assert_eq!(
            WireCallTypeTable::option_some_variant(),
            WireVariantId::new(0)
        );
        assert_eq!(
            WireCallTypeTable::option_none_variant(),
            WireVariantId::new(1)
        );
        assert_eq!(
            WireCallTypeTable::result_ok_variant(),
            WireVariantId::new(0)
        );
        assert_eq!(
            WireCallTypeTable::result_err_variant(),
            WireVariantId::new(1)
        );
    }

    #[test]
    fn call_type_table_assigns_resource_identities_independently_from_values() {
        let file = WireType::Resource {
            name: "host.fs.File".into(),
        };
        let socket = WireType::Resource {
            name: "host.net.Socket".into(),
        };
        let signature = FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "file".into(),
                effect: DataEffect::Read,
                ty: WireType::Qualified {
                    qualifier: WireQualifier::Owned,
                    value: Box::new(file.clone()),
                },
                retained: false,
            }],
            result: socket.clone(),
            asynchronous: false,
        };

        let first = WireCallTypeTable::for_signature(&signature).unwrap();
        let second = WireCallTypeTable::for_signature(&signature).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first.resource_type_id(&file),
            Some(WireResourceTypeId::new(0))
        );
        assert_eq!(
            first.resource_type_id(&socket),
            Some(WireResourceTypeId::new(1))
        );
        assert_eq!(
            first.resource_type_id(&WireType::Qualified {
                qualifier: WireQualifier::Owned,
                value: Box::new(file),
            }),
            Some(WireResourceTypeId::new(0)),
            "resource qualifiers must not create a second runtime handle kind"
        );
    }

    #[test]
    fn call_type_table_interns_map_key_and_value_before_the_map() {
        let map = WireType::Map {
            key: Box::new(WireType::String),
            value: Box::new(WireType::Char),
        };
        let table = WireCallTypeTable::for_signature(&FunctionSignature {
            parameters: Vec::new(),
            result: map.clone(),
            asynchronous: false,
        })
        .expect("map signature builds a wire type table");
        assert_eq!(table.type_id(&WireType::String), Some(WireTypeId::new(0)));
        assert_eq!(table.type_id(&WireType::Char), Some(WireTypeId::new(1)));
        assert_eq!(table.type_id(&map), Some(WireTypeId::new(2)));
    }

    #[test]
    fn record_layouts_are_canonicalized_before_assigning_type_ids() {
        let signature = FunctionSignature {
            parameters: Vec::new(),
            result: WireType::Unit,
            asynchronous: false,
        };
        let table = WireCallTypeTable::for_signature(&signature)
            .unwrap()
            .with_record_layouts(vec![
                WireRecordLayout {
                    ty: WireType::from("host.Z"),
                    fields: vec![WireRecordFieldLayout {
                        name: "value".into(),
                        ty: WireType::String,
                    }],
                },
                WireRecordLayout {
                    ty: WireType::from("host.A"),
                    fields: vec![WireRecordFieldLayout {
                        name: "value".into(),
                        ty: WireType::Int {
                            bits: 64,
                            signed: true,
                        },
                    }],
                },
            ])
            .unwrap();
        assert_eq!(
            table
                .record_layout(&WireType::from("host.A"))
                .unwrap()
                .fields
                .len(),
            1
        );
        assert_eq!(
            table.type_id(&WireType::from("host.A")),
            Some(WireTypeId::new(1))
        );
    }

    #[test]
    fn wire_payload_accounting_handles_deeply_nested_values_iteratively() {
        let mut value = WireValue::Bytes {
            value: vec![1, 2, 3],
        };
        for _ in 0..1_024 {
            value = WireValue::List {
                element_type: WireTypeId::new(1),
                values: vec![value],
            };
        }
        assert_eq!(value.estimated_payload_bytes(), 3);
    }
}
