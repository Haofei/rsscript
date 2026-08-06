#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use rsscript_abi_model::ExternalImport;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BYTECODE_SCHEMA: &str = "rsscript.bytecode.v1";
pub const BYTECODE_MAGIC: &[u8; 8] = b"RSSBC\0\x01\0";
const SECTION_HEADER: u8 = 1;
const SECTION_IMPORTS: u8 = 2;
const SECTION_CODE: u8 = 3;
const SECTION_CHECKSUM: u8 = 4;
const SECTION_REQUIRED: u8 = 1;
const SECTION_HEADER_BYTES: usize = 1 + 1 + 8 + 32;
const MAX_SECTIONS: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BytecodeHeader {
    pub schema: String,
    pub language_version: String,
    pub runtime_abi_version: u32,
    pub source_content_hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_digest: Option<String>,
    pub executable_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BytecodeArtifact {
    pub header: BytecodeHeader,
    pub imports: Vec<ExternalImport>,
    pub payload: Vec<u8>,
    pub checksum: String,
}

impl BytecodeArtifact {
    pub fn new(
        language_version: impl Into<String>,
        runtime_abi_version: u32,
        source_content_hash: impl Into<String>,
        mut imports: Vec<ExternalImport>,
        payload: Vec<u8>,
    ) -> Result<Self, BytecodeError> {
        imports.sort_by(|left, right| left.symbol.cmp(&right.symbol));
        let executable_hash = digest(&payload);
        let mut artifact = Self {
            header: BytecodeHeader {
                schema: BYTECODE_SCHEMA.to_string(),
                language_version: language_version.into(),
                runtime_abi_version,
                source_content_hash: source_content_hash.into(),
                snapshot_digest: None,
                executable_hash,
            },
            imports,
            payload,
            checksum: String::new(),
        };
        artifact.checksum = artifact.compute_checksum()?;
        Ok(artifact)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, BytecodeError> {
        let header = encode_executable_payload(&self.header)?;
        let imports = encode_executable_payload(&self.imports)?;
        let sections = [
            (SECTION_HEADER, header.as_slice()),
            (SECTION_IMPORTS, imports.as_slice()),
            (SECTION_CODE, self.payload.as_slice()),
            (SECTION_CHECKSUM, self.checksum.as_bytes()),
        ];
        let mut bytes = Vec::with_capacity(
            BYTECODE_MAGIC.len()
                + 2
                + sections
                    .iter()
                    .map(|(_, data)| SECTION_HEADER_BYTES + data.len())
                    .sum::<usize>(),
        );
        bytes.extend_from_slice(BYTECODE_MAGIC);
        bytes.extend_from_slice(&(sections.len() as u16).to_be_bytes());
        for (kind, data) in sections {
            bytes.push(kind);
            bytes.push(SECTION_REQUIRED);
            bytes.extend_from_slice(&(data.len() as u64).to_be_bytes());
            bytes.extend_from_slice(&Sha256::digest(data));
            bytes.extend_from_slice(data);
        }
        Ok(bytes)
    }

    /// Bind the artifact to the immutable workspace snapshot that produced it.
    /// Recomputes the envelope checksum; the executable payload is unchanged.
    pub fn bind_snapshot_digest(&mut self, digest: impl Into<String>) -> Result<(), BytecodeError> {
        self.header.snapshot_digest = Some(digest.into());
        self.checksum = self.compute_checksum()?;
        Ok(())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, BytecodeError> {
        let mut body = bytes
            .strip_prefix(BYTECODE_MAGIC)
            .ok_or(BytecodeError::InvalidMagic)?;
        let section_count = take_array::<2>(&mut body)
            .map(u16::from_be_bytes)
            .map(usize::from)?;
        if section_count == 0 || section_count > MAX_SECTIONS {
            return Err(BytecodeError::MalformedSectionTable);
        }
        let mut header = None;
        let mut imports = None;
        let mut payload = None;
        let mut checksum = None;
        let mut previous_kind = 0u8;
        for _ in 0..section_count {
            let kind = take_array::<1>(&mut body)?[0];
            let flags = take_array::<1>(&mut body)?[0];
            if flags & !SECTION_REQUIRED != 0 {
                return Err(BytecodeError::InvalidSectionFlags { kind, flags });
            }
            let length = usize::try_from(u64::from_be_bytes(take_array::<8>(&mut body)?))
                .map_err(|_| BytecodeError::MalformedSectionTable)?;
            let expected_hash = take_array::<32>(&mut body)?;
            let data = take_bytes(&mut body, length)?;
            if Sha256::digest(data).as_slice() != expected_hash {
                return Err(BytecodeError::SectionHashMismatch(kind));
            }
            if kind <= previous_kind {
                return Err(BytecodeError::SectionsNotCanonical);
            }
            previous_kind = kind;
            match kind {
                SECTION_HEADER => {
                    require_section(kind, flags)?;
                    let decoded: BytecodeHeader = decode_canonical_section(data)?;
                    header = Some(decoded);
                }
                SECTION_IMPORTS => {
                    require_section(kind, flags)?;
                    let decoded: Vec<ExternalImport> = decode_canonical_section(data)?;
                    imports = Some(decoded);
                }
                SECTION_CODE => {
                    require_section(kind, flags)?;
                    payload = Some(data.to_vec());
                }
                SECTION_CHECKSUM => {
                    require_section(kind, flags)?;
                    checksum = Some(
                        std::str::from_utf8(data)
                            .map_err(|_| BytecodeError::MalformedChecksum)?
                            .to_string(),
                    );
                }
                unknown if flags & SECTION_REQUIRED != 0 => {
                    return Err(BytecodeError::UnknownRequiredSection(unknown));
                }
                _ => {}
            }
        }
        if !body.is_empty() {
            return Err(BytecodeError::TrailingBytes);
        }
        Ok(Self {
            header: header.ok_or(BytecodeError::MissingSection(SECTION_HEADER))?,
            imports: imports.ok_or(BytecodeError::MissingSection(SECTION_IMPORTS))?,
            payload: payload.ok_or(BytecodeError::MissingSection(SECTION_CODE))?,
            checksum: checksum.ok_or(BytecodeError::MissingSection(SECTION_CHECKSUM))?,
        })
    }

    fn compute_checksum(&self) -> Result<String, BytecodeError> {
        #[derive(Serialize)]
        struct ChecksumInput<'a> {
            header: &'a BytecodeHeader,
            imports: &'a [ExternalImport],
            payload: &'a [u8],
        }
        let input = serde_json::to_vec(&ChecksumInput {
            header: &self.header,
            imports: &self.imports,
            payload: &self.payload,
        })
        .map_err(BytecodeError::Encode)?;
        Ok(digest(&input))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BytecodeLimits {
    pub max_artifact_bytes: usize,
    pub max_payload_bytes: usize,
    pub max_imports: usize,
    pub max_functions: usize,
    pub max_registers_per_function: usize,
    pub max_instructions: usize,
}

impl Default for BytecodeLimits {
    fn default() -> Self {
        Self {
            max_artifact_bytes: 64 * 1024 * 1024,
            max_payload_bytes: 48 * 1024 * 1024,
            max_imports: 16_384,
            max_functions: 65_536,
            max_registers_per_function: 1_048_576,
            max_instructions: 10_000_000,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedBytecode {
    artifact: BytecodeArtifact,
}

impl VerifiedBytecode {
    pub fn artifact(&self) -> &BytecodeArtifact {
        &self.artifact
    }

    pub fn into_artifact(self) -> BytecodeArtifact {
        self.artifact
    }
}

pub struct BytecodeVerifier {
    limits: BytecodeLimits,
}

impl BytecodeVerifier {
    pub fn new(limits: BytecodeLimits) -> Self {
        Self { limits }
    }

    pub fn verify(&self, bytes: &[u8]) -> Result<VerifiedBytecode, BytecodeError> {
        if bytes.len() > self.limits.max_artifact_bytes {
            return Err(BytecodeError::LimitExceeded("artifact bytes"));
        }
        let artifact = BytecodeArtifact::from_bytes(bytes)?;
        if artifact.header.schema != BYTECODE_SCHEMA {
            return Err(BytecodeError::UnsupportedSchema(artifact.header.schema));
        }
        if artifact.payload.len() > self.limits.max_payload_bytes {
            return Err(BytecodeError::LimitExceeded("payload bytes"));
        }
        if artifact.imports.len() > self.limits.max_imports {
            return Err(BytecodeError::LimitExceeded("imports"));
        }
        if artifact.header.executable_hash != digest(&artifact.payload) {
            return Err(BytecodeError::ExecutableHashMismatch);
        }
        if artifact.checksum != artifact.compute_checksum()? {
            return Err(BytecodeError::ChecksumMismatch);
        }
        if artifact
            .imports
            .windows(2)
            .any(|pair| pair[0].symbol >= pair[1].symbol)
        {
            return Err(BytecodeError::ImportsNotCanonical);
        }
        if artifact
            .imports
            .iter()
            .any(|import| import.abi_version != artifact.header.runtime_abi_version)
        {
            return Err(BytecodeError::ImportAbiMismatch);
        }
        if artifact
            .imports
            .iter()
            .any(|import| import.signature.hash() != import.signature_hash)
        {
            return Err(BytecodeError::ImportSignatureHashMismatch);
        }
        verify_executable_payload(&artifact.payload, &artifact.imports, self.limits)?;
        Ok(VerifiedBytecode { artifact })
    }
}

impl Default for BytecodeVerifier {
    fn default() -> Self {
        Self::new(BytecodeLimits::default())
    }
}

/// Encode the executable section as deterministic binary CBOR. Object keys are
/// sorted before serialization, so equivalent wire values have one byte form.
pub fn encode_executable_payload<T: Serialize>(value: &T) -> Result<Vec<u8>, BytecodeError> {
    let value = serde_json::to_value(value).map_err(BytecodeError::Encode)?;
    serde_cbor::to_vec(&canonical_cbor(value)).map_err(BytecodeError::Cbor)
}

/// Decode the executable section owned by this crate.
pub fn decode_executable_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, BytecodeError> {
    serde_cbor::from_slice(payload).map_err(BytecodeError::Cbor)
}

fn canonical_cbor(value: serde_json::Value) -> serde_cbor::Value {
    match value {
        serde_json::Value::Null => serde_cbor::Value::Null,
        serde_json::Value::Bool(value) => serde_cbor::Value::Bool(value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                serde_cbor::Value::Integer(value.into())
            } else if let Some(value) = value.as_u64() {
                serde_cbor::Value::Integer(value.into())
            } else {
                serde_cbor::Value::Float(value.as_f64().expect("JSON number"))
            }
        }
        serde_json::Value::String(value) => serde_cbor::Value::Text(value),
        serde_json::Value::Array(values) => {
            serde_cbor::Value::Array(values.into_iter().map(canonical_cbor).collect())
        }
        serde_json::Value::Object(values) => serde_cbor::Value::Map(
            values
                .into_iter()
                .map(|(key, value)| (serde_cbor::Value::Text(key), canonical_cbor(value)))
                .collect::<BTreeMap<_, _>>(),
        ),
    }
}

/// Validate the executable instruction payload without linking the compiler or
/// VM implementation. The wire format is intentionally inspected as data so a
/// verifier-only process can reject malformed control-flow, register, function,
/// and import references before any engine-specific deserialization occurs.
fn verify_executable_payload(
    payload: &[u8],
    imports: &[ExternalImport],
    limits: BytecodeLimits,
) -> Result<(), BytecodeError> {
    let value: serde_json::Value = decode_executable_payload(payload)
        .map_err(|error| BytecodeError::InvalidPayload(error.to_string()))?;
    if encode_executable_payload(&value)? != payload {
        return Err(invalid_payload("executable CBOR is not canonical"));
    }
    let unit = value
        .as_object()
        .ok_or_else(|| invalid_payload("unit is not an object"))?;
    require_object_fields(
        unit,
        &[
            "functions",
            "function_ids",
            "resource_drop_functions",
            "types",
            "native_signatures",
            "closure_identity_observable",
        ],
        "unit",
    )?;
    let functions = unit["functions"]
        .as_array()
        .ok_or_else(|| invalid_payload("functions is not an array"))?;
    if functions.len() > limits.max_functions {
        return Err(BytecodeError::LimitExceeded("function count"));
    }

    let mut total_instructions = 0usize;
    let mut called_imports = BTreeSet::new();
    for (function_id, value) in functions.iter().enumerate() {
        let function = value
            .as_object()
            .ok_or_else(|| invalid_payload(format!("function {function_id} is not an object")))?;
        require_object_fields(
            function,
            &["name", "params", "captures", "regs", "local_regs", "code"],
            &format!("function {function_id}"),
        )?;
        let name = function["name"].as_str().ok_or_else(|| {
            invalid_payload(format!("function {function_id} name is not a string"))
        })?;
        let registers = json_usize(&function["regs"], "register count")?;
        let params = json_usize(&function["params"], "parameter count")?;
        let captures = json_usize(&function["captures"], "capture count")?;
        if registers > limits.max_registers_per_function {
            return Err(BytecodeError::LimitExceeded("register count"));
        }
        if params > registers || captures > registers {
            return Err(invalid_payload(format!(
                "function {function_id} has more parameters/captures than registers"
            )));
        }
        let locals = function["local_regs"].as_object().ok_or_else(|| {
            invalid_payload(format!("function {function_id} locals is not an object"))
        })?;
        for (local, register) in locals {
            verify_register(function_id, registers, json_usize(register, local)?, local)?;
        }
        let code = function["code"].as_array().ok_or_else(|| {
            invalid_payload(format!("function {function_id} code is not an array"))
        })?;
        total_instructions = total_instructions
            .checked_add(code.len())
            .ok_or(BytecodeError::LimitExceeded("instruction count"))?;
        if total_instructions > limits.max_instructions {
            return Err(BytecodeError::LimitExceeded("instruction count"));
        }
        for (ip, instruction) in code.iter().enumerate() {
            verify_wire_instruction(
                function_id,
                ip,
                registers,
                code.len(),
                functions.len(),
                instruction,
                &mut called_imports,
            )?;
        }
        let _ = name;
    }

    verify_function_map(unit, "function_ids", functions, true)?;
    verify_function_map(unit, "resource_drop_functions", functions, false)?;
    let declared_imports = imports
        .iter()
        .map(|import| import.symbol.as_str().to_string())
        .collect::<BTreeSet<_>>();
    if called_imports != declared_imports {
        return Err(invalid_payload(format!(
            "external call table mismatch: instructions={called_imports:?}, imports={declared_imports:?}"
        )));
    }
    Ok(())
}

fn verify_function_map(
    unit: &serde_json::Map<String, serde_json::Value>,
    field: &str,
    functions: &[serde_json::Value],
    names_must_match: bool,
) -> Result<(), BytecodeError> {
    let entries = unit[field]
        .as_object()
        .ok_or_else(|| invalid_payload(format!("{field} is not an object")))?;
    for (name, value) in entries {
        let function_id = json_usize(value, field)?;
        let function = functions.get(function_id).ok_or_else(|| {
            invalid_payload(format!(
                "{field} `{name}` references missing function {function_id}"
            ))
        })?;
        if names_must_match
            && function.get("name").and_then(serde_json::Value::as_str) != Some(name)
        {
            return Err(invalid_payload(format!(
                "function map `{name}` does not match function metadata"
            )));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn verify_wire_instruction(
    function_id: usize,
    ip: usize,
    register_count: usize,
    code_len: usize,
    function_count: usize,
    instruction: &serde_json::Value,
    called_imports: &mut BTreeSet<String>,
) -> Result<(), BytecodeError> {
    if let Some(opcode) = instruction.as_str() {
        if opcode == "TailCallGuard" {
            return Ok(());
        }
        return Err(invalid_payload(format!(
            "function {function_id} instruction {ip} has unknown unit opcode `{opcode}`"
        )));
    }
    let (opcode, fields) = instruction
        .as_object()
        .filter(|outer| outer.len() == 1)
        .and_then(|outer| outer.iter().next())
        .ok_or_else(|| {
            invalid_payload(format!(
                "function {function_id} instruction {ip} has invalid encoding"
            ))
        })?;
    if !KNOWN_OPCODES.contains(&opcode.as_str()) {
        return Err(invalid_payload(format!(
            "function {function_id} instruction {ip} has unknown opcode `{opcode}`"
        )));
    }
    let fields = fields.as_object().ok_or_else(|| {
        invalid_payload(format!(
            "function {function_id} instruction {ip} fields are not an object"
        ))
    })?;
    for target_field in [
        "target", "some_ip", "none_ip", "ok_ip", "err_ip", "match_ip", "else_ip",
    ] {
        if let Some(target) = fields.get(target_field) {
            let target = json_usize(target, target_field)?;
            if target >= code_len {
                return Err(invalid_payload(format!(
                    "function {function_id} instruction {ip} jumps outside its body to {target}"
                )));
            }
        }
    }
    if matches!(opcode.as_str(), "MakeClosure" | "CallKnown" | "SpawnTask") {
        let target = fields
            .get("function")
            .ok_or_else(|| invalid_payload(format!("{opcode} is missing function")))?;
        let target = json_usize(target, "function")?;
        if target >= function_count {
            return Err(invalid_payload(format!(
                "function {function_id} instruction {ip} references missing function {target}"
            )));
        }
    }
    if opcode == "CallDynamic" {
        for entry in fields
            .get("dispatch")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid_payload("CallDynamic dispatch is not an array"))?
        {
            let tuple = entry
                .as_array()
                .filter(|tuple| tuple.len() == 2)
                .ok_or_else(|| invalid_payload("CallDynamic dispatch entry is invalid"))?;
            let target = json_usize(&tuple[1], "dispatch target")?;
            if target >= function_count {
                return Err(invalid_payload(format!(
                    "missing dispatch function {target}"
                )));
            }
        }
    }
    if opcode == "CallExternal" {
        let symbol = fields
            .get("key")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| invalid_payload("CallExternal key is not a string"))?;
        called_imports.insert(symbol.to_string());
    }
    for (field, value) in fields {
        verify_register_field(function_id, ip, register_count, opcode, field, value)?;
    }
    Ok(())
}

fn verify_register_field(
    function_id: usize,
    ip: usize,
    register_count: usize,
    opcode: &str,
    field: &str,
    value: &serde_json::Value,
) -> Result<(), BytecodeError> {
    let scalar_register = matches!(
        field,
        "dst"
            | "src"
            | "reg"
            | "base"
            | "lhs"
            | "rhs"
            | "cond"
            | "resource"
            | "map"
            | "list"
            | "closure"
            | "winner"
            | "buffer"
            | "builder"
            | "left"
            | "right"
            | "state"
            | "folder"
            | "predicate"
            | "mapper"
            | "values"
            | "deque"
            | "set"
            | "callback"
            | "compare"
    ) || (field == "key" && opcode != "CallExternal")
        || (field == "value" && !opcode.starts_with("Load"))
        || (field == "index" && matches!(opcode, "ListGet" | "ListRemoveAt" | "ListSet"));
    if scalar_register {
        return verify_register(
            function_id,
            register_count,
            json_usize(value, field)?,
            field,
        );
    }
    if matches!(field, "args" | "captures" | "cleanup" | "handles" | "items") {
        let values = value.as_array().ok_or_else(|| {
            invalid_payload(format!(
                "function {function_id} instruction {ip} field `{field}` is not an array"
            ))
        })?;
        for register in values {
            verify_register(
                function_id,
                register_count,
                json_usize(register, field)?,
                field,
            )?;
        }
    } else if field == "fields" {
        verify_tuple_registers(function_id, register_count, value, &[1])?;
    } else if field == "entries" {
        verify_tuple_registers(function_id, register_count, value, &[0, 1])?;
    }
    Ok(())
}

fn verify_tuple_registers(
    function_id: usize,
    register_count: usize,
    value: &serde_json::Value,
    positions: &[usize],
) -> Result<(), BytecodeError> {
    for entry in value
        .as_array()
        .ok_or_else(|| invalid_payload("register tuple list is not an array"))?
    {
        let tuple = entry
            .as_array()
            .ok_or_else(|| invalid_payload("register tuple entry is not an array"))?;
        for position in positions {
            let register = tuple
                .get(*position)
                .ok_or_else(|| invalid_payload("register tuple is incomplete"))?;
            verify_register(
                function_id,
                register_count,
                json_usize(register, "tuple")?,
                "tuple",
            )?;
        }
    }
    Ok(())
}

fn verify_register(
    function_id: usize,
    register_count: usize,
    register: usize,
    field: &str,
) -> Result<(), BytecodeError> {
    if register >= register_count {
        Err(invalid_payload(format!(
            "function {function_id} field `{field}` references register {register}, limit is {register_count}"
        )))
    } else {
        Ok(())
    }
}

fn json_usize(value: &serde_json::Value, field: &str) -> Result<usize, BytecodeError> {
    value
        .as_u64()
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| invalid_payload(format!("{field} is not a valid index")))
}

fn require_object_fields(
    object: &serde_json::Map<String, serde_json::Value>,
    expected: &[&str],
    context: &str,
) -> Result<(), BytecodeError> {
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(invalid_payload(format!(
            "{context} fields differ: actual={actual:?}, expected={expected:?}"
        )));
    }
    Ok(())
}

fn invalid_payload(message: impl Into<String>) -> BytecodeError {
    BytecodeError::InvalidPayload(message.into())
}

const KNOWN_OPCODES: &[&str] = &[
    "LoadUnit",
    "LoadInt",
    "LoadFloat",
    "LoadBool",
    "LoadString",
    "LoadChar",
    "Move",
    "DeepCopy",
    "DeepCopyElided",
    "Manage",
    "GetField",
    "GetFieldSlot",
    "SetFieldSlot",
    "SetField",
    "MakeStruct",
    "ResourceDrop",
    "MakeVariant",
    "MakeList",
    "MakeObject",
    "MakeMap",
    "AddInt",
    "SubInt",
    "MulInt",
    "DivInt",
    "ModInt",
    "BitAndInt",
    "BitOrInt",
    "BitXorInt",
    "ShiftLeftInt",
    "ShiftRightInt",
    "LessInt",
    "LessEqualInt",
    "GreaterInt",
    "GreaterEqualInt",
    "Equal",
    "NotEqual",
    "Jump",
    "JumpIfBool",
    "JumpIfIntCompare",
    "MatchOption",
    "MatchResult",
    "MatchVariant",
    "RuntimeError",
    "MatchMapGet",
    "MatchSortedMapGet",
    "UnwrapSome",
    "UnwrapVariantValue",
    "MakeClosure",
    "MakeSome",
    "LoadNone",
    "CallKnown",
    "CallDynamic",
    "SpawnTask",
    "AwaitJoin",
    "SelectWait",
    "CallExternal",
    "CallClosure",
    "NativeGuardClosureId",
    "NativeClosureId",
    "NativeClosureCapture",
    "NativeFieldClosureId",
    "NativeFieldClosureCapture",
    "ListFilter",
    "ListFold",
    "ListGet",
    "ListLen",
    "ListMap",
    "ListAppend",
    "ListClear",
    "ListPop",
    "ListPush",
    "ListRemoveAt",
    "ListSet",
    "ListSort",
    "ListSortBy",
    "ListSortWith",
    "DequeClear",
    "DequePopBack",
    "DequePopFront",
    "DequePushBack",
    "DequePushFront",
    "SetClear",
    "SetForEach",
    "SetInsert",
    "SetRemove",
    "SortedSetClear",
    "SortedSetInsert",
    "SortedSetRemove",
    "SortedMapClear",
    "SortedMapInsert",
    "SortedMapRemove",
    "MapGet",
    "MapClear",
    "MapInsertOld",
    "MapRemove",
    "BufferClear",
    "MapInsert",
    "StringBuilderPush",
    "StringBuilderFinish",
    "StringConcat",
    "CallIntrinsic",
    "CallTypedIntrinsic",
    "TryResult",
    "Return",
];

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn take_array<const N: usize>(input: &mut &[u8]) -> Result<[u8; N], BytecodeError> {
    let bytes = take_bytes(input, N)?;
    bytes
        .try_into()
        .map_err(|_| BytecodeError::MalformedSectionTable)
}

fn take_bytes<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8], BytecodeError> {
    if length > input.len() {
        return Err(BytecodeError::MalformedSectionTable);
    }
    let (value, rest) = input.split_at(length);
    *input = rest;
    Ok(value)
}

fn require_section(kind: u8, flags: u8) -> Result<(), BytecodeError> {
    if flags & SECTION_REQUIRED == 0 {
        return Err(BytecodeError::KnownSectionNotRequired(kind));
    }
    Ok(())
}

fn decode_canonical_section<T: Serialize + DeserializeOwned>(
    bytes: &[u8],
) -> Result<T, BytecodeError> {
    let value = decode_executable_payload(bytes)?;
    if encode_executable_payload(&value)? != bytes {
        return Err(BytecodeError::SectionsNotCanonical);
    }
    Ok(value)
}

#[derive(Debug)]
pub enum BytecodeError {
    InvalidMagic,
    UnsupportedSchema(String),
    LimitExceeded(&'static str),
    ExecutableHashMismatch,
    ChecksumMismatch,
    ImportsNotCanonical,
    ImportAbiMismatch,
    ImportSignatureHashMismatch,
    InvalidPayload(String),
    MalformedSectionTable,
    SectionsNotCanonical,
    MissingSection(u8),
    UnknownRequiredSection(u8),
    KnownSectionNotRequired(u8),
    InvalidSectionFlags { kind: u8, flags: u8 },
    SectionHashMismatch(u8),
    MalformedChecksum,
    TrailingBytes,
    Encode(serde_json::Error),
    Cbor(serde_cbor::Error),
}

impl fmt::Display for BytecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMagic => formatter.write_str("invalid RSScript bytecode magic"),
            Self::UnsupportedSchema(schema) => {
                write!(formatter, "unsupported bytecode schema `{schema}`")
            }
            Self::LimitExceeded(limit) => {
                write!(formatter, "bytecode {limit} exceeds verifier limit")
            }
            Self::ExecutableHashMismatch => {
                formatter.write_str("bytecode executable hash mismatch")
            }
            Self::ChecksumMismatch => formatter.write_str("bytecode artifact checksum mismatch"),
            Self::ImportsNotCanonical => {
                formatter.write_str("bytecode imports are duplicated or not sorted")
            }
            Self::ImportAbiMismatch => {
                formatter.write_str("bytecode import ABI does not match its header")
            }
            Self::ImportSignatureHashMismatch => {
                formatter.write_str("bytecode import signature does not match its hash")
            }
            Self::InvalidPayload(message) => {
                write!(formatter, "invalid bytecode payload: {message}")
            }
            Self::MalformedSectionTable => formatter.write_str("malformed bytecode section table"),
            Self::SectionsNotCanonical => {
                formatter.write_str("bytecode sections are duplicated or not canonical")
            }
            Self::MissingSection(section) => {
                write!(formatter, "bytecode is missing required section {section}")
            }
            Self::UnknownRequiredSection(section) => {
                write!(
                    formatter,
                    "bytecode contains unknown required section {section}"
                )
            }
            Self::KnownSectionNotRequired(section) => {
                write!(
                    formatter,
                    "bytecode section {section} is not marked required"
                )
            }
            Self::InvalidSectionFlags { kind, flags } => {
                write!(
                    formatter,
                    "bytecode section {kind} has invalid flags {flags:#04x}"
                )
            }
            Self::SectionHashMismatch(section) => {
                write!(formatter, "bytecode section {section} hash mismatch")
            }
            Self::MalformedChecksum => formatter.write_str("bytecode checksum is not UTF-8"),
            Self::TrailingBytes => formatter.write_str("bytecode has trailing bytes"),
            Self::Encode(error) => write!(formatter, "cannot encode bytecode: {error}"),
            Self::Cbor(error) => write!(formatter, "cannot encode/decode executable CBOR: {error}"),
        }
    }
}

impl Error for BytecodeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rsscript_abi_model::{DataEffect, ExternalSymbol, FunctionSignature, ParameterSignature};

    #[test]
    fn round_trip_requires_intact_artifact() {
        let payload = minimal_payload();
        let artifact = BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], payload.clone())
            .expect("artifact");
        let bytes = artifact.to_bytes().expect("bytes");
        let verified = BytecodeVerifier::default()
            .verify(&bytes)
            .expect("verified");
        assert_eq!(verified.artifact().payload, payload);

        let mut corrupt = bytes;
        *corrupt.last_mut().expect("non-empty") ^= 1;
        assert!(BytecodeVerifier::default().verify(&corrupt).is_err());
    }

    #[test]
    fn artifact_sections_and_instruction_payload_use_binary_cbor() {
        let payload = minimal_payload();
        assert_ne!(payload.first(), Some(&b'{'));
        let artifact =
            BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], payload).expect("artifact");
        let bytes = artifact.to_bytes().expect("bytes");
        let first_section_data = BYTECODE_MAGIC.len() + 2 + SECTION_HEADER_BYTES;
        assert_ne!(bytes.get(first_section_data), Some(&b'{'));
        BytecodeVerifier::default().verify(&bytes).unwrap();
    }

    #[test]
    fn unknown_optional_sections_are_forward_compatible() {
        let artifact = BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], minimal_payload())
            .expect("artifact");
        let mut bytes = artifact.to_bytes().expect("bytes");
        bytes[BYTECODE_MAGIC.len()..BYTECODE_MAGIC.len() + 2].copy_from_slice(&5u16.to_be_bytes());
        append_test_section(&mut bytes, 5, 0, b"future metadata");

        BytecodeVerifier::default()
            .verify(&bytes)
            .expect("optional section should be ignored");
    }

    #[test]
    fn unknown_required_sections_fail_closed() {
        let artifact = BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], minimal_payload())
            .expect("artifact");
        let mut bytes = artifact.to_bytes().expect("bytes");
        bytes[BYTECODE_MAGIC.len()..BYTECODE_MAGIC.len() + 2].copy_from_slice(&5u16.to_be_bytes());
        append_test_section(&mut bytes, 5, SECTION_REQUIRED, b"future semantics");

        assert!(matches!(
            BytecodeVerifier::default().verify(&bytes),
            Err(BytecodeError::UnknownRequiredSection(5))
        ));
    }

    #[test]
    fn malformed_binary_metadata_section_is_rejected() {
        let artifact = BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], minimal_payload())
            .expect("artifact");
        let bytes = artifact.to_bytes().expect("bytes");
        let header_offset = BYTECODE_MAGIC.len() + 2;
        let data_offset = header_offset + SECTION_HEADER_BYTES;
        let data_length = u64::from_be_bytes(
            bytes[header_offset + 2..header_offset + 10]
                .try_into()
                .expect("section length"),
        ) as usize;
        let mut rewritten = Vec::new();
        rewritten.extend_from_slice(&bytes[..header_offset]);
        let mut header = Vec::with_capacity(data_length + 1);
        header.push(b' ');
        header.extend_from_slice(&bytes[data_offset..data_offset + data_length]);
        append_test_section(&mut rewritten, SECTION_HEADER, SECTION_REQUIRED, &header);
        rewritten.extend_from_slice(&bytes[data_offset + data_length..]);

        assert!(BytecodeVerifier::default().verify(&rewritten).is_err());
    }

    #[test]
    fn verifier_rejects_unknown_instruction_with_a_valid_envelope() {
        let payload = encode_executable_payload(&serde_json::json!({
            "functions": [{
                "name": "main",
                "params": 0,
                "captures": 0,
                "regs": 1,
                "local_regs": {},
                "code": [{"FutureOpcode": {"dst": 0}}]
            }],
            "function_ids": {"main": 0},
            "resource_drop_functions": {},
            "types": {},
            "native_signatures": {},
            "closure_identity_observable": false
        }))
        .expect("payload");
        let artifact =
            BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], payload).expect("artifact");

        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().expect("bytes")),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("unknown opcode")
        ));
    }

    #[test]
    fn verifier_rejects_out_of_range_register_with_a_valid_envelope() {
        let payload = encode_executable_payload(&serde_json::json!({
            "functions": [{
                "name": "main",
                "params": 0,
                "captures": 0,
                "regs": 1,
                "local_regs": {},
                "code": [{"LoadUnit": {"dst": 1}}]
            }],
            "function_ids": {"main": 0},
            "resource_drop_functions": {},
            "types": {},
            "native_signatures": {},
            "closure_identity_observable": false
        }))
        .expect("payload");
        let artifact =
            BytecodeArtifact::new("0.1", 1, "sha256:source", vec![], payload).expect("artifact");

        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().expect("bytes")),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("register 1")
        ));
    }

    #[test]
    fn verifier_rejects_import_whose_structural_signature_disagrees_with_hash() {
        let signature = FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "message".into(),
                effect: DataEffect::Read,
                ty: "String".into(),
                retained: false,
            }],
            result: "Unit".into(),
            asynchronous: false,
        };
        let wrong_hash = FunctionSignature {
            parameters: vec![],
            result: "Unit".into(),
            asynchronous: false,
        }
        .hash();
        let artifact = BytecodeArtifact::new(
            "0.1",
            1,
            "sha256:source",
            vec![ExternalImport {
                symbol: ExternalSymbol::new("host.log.emit").unwrap(),
                signature,
                signature_hash: wrong_hash,
                abi_version: 1,
            }],
            encode_executable_payload(&serde_json::json!({
                "functions": [{
                    "name": "main", "params": 0, "captures": 0, "regs": 1,
                    "local_regs": {},
                    "code": [{"CallExternal": {"dst": 0, "key": "host.log.emit", "args": [], "mut_args": []}}]
                }],
                "function_ids": {"main": 0},
                "resource_drop_functions": {}, "types": {}, "native_signatures": {},
                "closure_identity_observable": false
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
            Err(BytecodeError::ImportSignatureHashMismatch)
        ));
    }

    fn append_test_section(bytes: &mut Vec<u8>, kind: u8, flags: u8, data: &[u8]) {
        bytes.push(kind);
        bytes.push(flags);
        bytes.extend_from_slice(&(data.len() as u64).to_be_bytes());
        bytes.extend_from_slice(&Sha256::digest(data));
        bytes.extend_from_slice(data);
    }

    fn minimal_payload() -> Vec<u8> {
        encode_executable_payload(&serde_json::json!({
            "functions": [],
            "function_ids": {},
            "resource_drop_functions": {},
            "types": {},
            "native_signatures": {},
            "closure_identity_observable": false
        }))
        .expect("minimal payload")
    }

    proptest! {
        #[test]
        fn arbitrary_bounded_input_is_rejected_without_panicking(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let verifier = BytecodeVerifier::new(BytecodeLimits {
                max_artifact_bytes: 2048,
                max_payload_bytes: 1024,
                max_imports: 32,
                max_functions: 32,
                max_registers_per_function: 256,
                max_instructions: 1024,
            });
            let _ = verifier.verify(&bytes);
        }
    }
}
