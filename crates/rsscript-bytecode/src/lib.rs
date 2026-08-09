#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

pub use rsscript_abi_model::LANGUAGE_SEMANTICS_VERSION;
use rsscript_abi_model::{ExternalImport, RUNTIME_ABI_VERSION};
use rsscript_operation::{CancellationToken, MonotonicDeadline};
use semver::{Version, VersionReq};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const BYTECODE_SCHEMA: &str = "rsscript.bytecode.v1";
/// Version of the binary Artifact container envelope, independent of language
/// semantics and instruction-set compatibility.
pub const BYTECODE_CONTAINER_FORMAT_VERSION: u16 = 1;
/// Accepted language-semantics range for the v1 verifier.
pub const SUPPORTED_LANGUAGE_SEMANTICS: &str = ">=0.1.0, <0.2.0";
/// Version of the executable instruction-set encoding inside the v1 envelope.
pub const BYTECODE_ISA_VERSION: u32 = 1;
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
    /// Instruction-set contract for the executable payload. This is separate
    /// from source-language semantics and the runtime/provider ABI.
    pub bytecode_isa_version: u32,
    pub compiler_version: String,
    pub interface_catalog_digest: String,
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
        compiler_version: impl Into<String>,
        interface_catalog_digest: impl Into<String>,
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
                bytecode_isa_version: BYTECODE_ISA_VERSION,
                compiler_version: compiler_version.into(),
                interface_catalog_digest: interface_catalog_digest.into(),
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
    compatibility: BytecodeCompatibility,
}

#[derive(Debug, Clone)]
pub struct BytecodeCompatibility {
    pub language: VersionReq,
    pub bytecode_isa_version: u32,
    pub runtime_abi_version: u32,
}

impl Default for BytecodeCompatibility {
    fn default() -> Self {
        Self {
            language: VersionReq::parse(SUPPORTED_LANGUAGE_SEMANTICS)
                .expect("declared language compatibility requirement"),
            bytecode_isa_version: BYTECODE_ISA_VERSION,
            runtime_abi_version: RUNTIME_ABI_VERSION,
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VerificationContext<'a> {
    pub cancellation: Option<&'a CancellationToken>,
    pub deadline: Option<MonotonicDeadline>,
}

impl VerificationContext<'_> {
    pub fn check(self) -> Result<(), BytecodeError> {
        if self
            .cancellation
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(BytecodeError::Cancelled);
        }
        if self.deadline.is_some_and(MonotonicDeadline::is_expired) {
            return Err(BytecodeError::DeadlineExceeded);
        }
        Ok(())
    }
}

impl BytecodeVerifier {
    pub fn new(limits: BytecodeLimits) -> Self {
        Self {
            limits,
            compatibility: BytecodeCompatibility::default(),
        }
    }

    pub fn with_compatibility(
        limits: BytecodeLimits,
        compatibility: BytecodeCompatibility,
    ) -> Self {
        Self {
            limits,
            compatibility,
        }
    }

    pub fn verify(&self, bytes: &[u8]) -> Result<VerifiedBytecode, BytecodeError> {
        self.verify_with_context(bytes, VerificationContext::default())
    }

    pub fn verify_with_context(
        &self,
        bytes: &[u8],
        context: VerificationContext<'_>,
    ) -> Result<VerifiedBytecode, BytecodeError> {
        context.check()?;
        if bytes.len() > self.limits.max_artifact_bytes {
            return Err(BytecodeError::LimitExceeded("artifact bytes"));
        }
        let artifact = BytecodeArtifact::from_bytes(bytes)?;
        context.check()?;
        if artifact.header.schema != BYTECODE_SCHEMA {
            return Err(BytecodeError::UnsupportedSchema(artifact.header.schema));
        }
        Version::parse(&artifact.header.compiler_version)
            .map_err(|_| BytecodeError::InvalidProvenance("compiler version"))?;
        for (name, digest) in [
            (
                "source content hash",
                artifact.header.source_content_hash.as_str(),
            ),
            (
                "interface catalog digest",
                artifact.header.interface_catalog_digest.as_str(),
            ),
        ] {
            if !is_sha256_digest(digest) {
                return Err(BytecodeError::InvalidProvenance(name));
            }
        }
        if artifact
            .header
            .snapshot_digest
            .as_deref()
            .is_some_and(|digest| !is_sha256_digest(digest))
        {
            return Err(BytecodeError::InvalidProvenance("snapshot digest"));
        }
        let language = Version::parse(&artifact.header.language_version).map_err(|_| {
            BytecodeError::UnsupportedLanguageVersion(artifact.header.language_version.clone())
        })?;
        if !self.compatibility.language.matches(&language) {
            return Err(BytecodeError::UnsupportedLanguageVersion(
                artifact.header.language_version.clone(),
            ));
        }
        if artifact.header.bytecode_isa_version != self.compatibility.bytecode_isa_version {
            return Err(BytecodeError::UnsupportedBytecodeIsa {
                artifact: artifact.header.bytecode_isa_version,
                verifier: self.compatibility.bytecode_isa_version,
            });
        }
        if artifact.header.runtime_abi_version != self.compatibility.runtime_abi_version {
            return Err(BytecodeError::UnsupportedRuntimeAbi {
                artifact: artifact.header.runtime_abi_version,
                runtime: self.compatibility.runtime_abi_version,
            });
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
        verify_executable_payload(&artifact.payload, &artifact.imports, self.limits, context)?;
        context.check()?;
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
    context: VerificationContext<'_>,
) -> Result<(), BytecodeError> {
    context.check()?;
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
    let resource_inputs = resource_drop_inputs(unit, functions)?;

    let mut total_instructions = 0usize;
    let mut called_imports = BTreeSet::new();
    let mut function_names = BTreeSet::new();
    for (function_id, value) in functions.iter().enumerate() {
        context.check()?;
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
        if name.is_empty() || !function_names.insert(name) {
            return Err(invalid_payload(format!(
                "function {function_id} has an empty or duplicate name `{name}`"
            )));
        }
        let registers = json_usize(&function["regs"], "register count")?;
        let params = json_usize(&function["params"], "parameter count")?;
        let captures = json_usize(&function["captures"], "capture count")?;
        if registers > limits.max_registers_per_function {
            return Err(BytecodeError::LimitExceeded("register count"));
        }
        let initialized_inputs = params.checked_add(captures).ok_or_else(|| {
            invalid_payload(format!("function {function_id} input count overflow"))
        })?;
        if initialized_inputs > registers {
            return Err(invalid_payload(format!(
                "function {function_id} has more parameters and captures than registers"
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
            if ip & 0xff == 0 {
                context.check()?;
            }
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
        let mut initialized_registers = (0..initialized_inputs).collect::<BTreeSet<_>>();
        if let Some(resources) = resource_inputs.get(&function_id) {
            initialized_registers.extend(resources);
        }
        verify_register_initialization(function_id, initialized_registers, code)?;
        verify_call_shapes(function_id, code, functions, imports)?;
        let _ = name;
    }

    verify_function_map(unit, "function_ids", functions, true)?;
    context.check()?;
    verify_function_map(unit, "resource_drop_functions", functions, false)?;
    verify_type_metadata(unit, limits)?;
    verify_native_signatures(unit, functions, limits)?;
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

fn resource_drop_inputs(
    unit: &serde_json::Map<String, serde_json::Value>,
    functions: &[serde_json::Value],
) -> Result<BTreeMap<usize, BTreeSet<usize>>, BytecodeError> {
    let drops = unit["resource_drop_functions"]
        .as_object()
        .ok_or_else(|| invalid_payload("resource_drop_functions is not an object"))?;
    let types = unit["types"]
        .as_object()
        .ok_or_else(|| invalid_payload("types is not an object"))?;
    let mut inputs = BTreeMap::new();
    for (type_name, function_id) in drops {
        let function_id = json_usize(function_id, "resource drop function")?;
        let function = functions
            .get(function_id)
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| {
                invalid_payload(format!(
                    "resource `{type_name}` references missing drop function {function_id}"
                ))
            })?;
        let ty = types
            .get(type_name)
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| invalid_payload(format!("resource type `{type_name}` is missing")))?;
        let fields = ty
            .get("fields")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid_payload(format!("resource type `{type_name}` has no fields")))?;
        let locals = function
            .get("local_regs")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| {
                invalid_payload(format!(
                    "resource drop `{type_name}` has no local register map"
                ))
            })?;
        let mut registers = BTreeSet::new();
        for field in fields {
            let name = field
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    invalid_payload(format!("resource type `{type_name}` has invalid field"))
                })?;
            let register = locals.get(name).ok_or_else(|| {
                invalid_payload(format!(
                    "resource drop `{type_name}` is missing field register `{name}`"
                ))
            })?;
            registers.insert(json_usize(register, "resource field register")?);
        }
        if inputs.insert(function_id, registers).is_some() {
            return Err(invalid_payload(format!(
                "drop function {function_id} is shared by multiple resource types"
            )));
        }
    }
    Ok(inputs)
}

fn verify_type_metadata(
    unit: &serde_json::Map<String, serde_json::Value>,
    limits: BytecodeLimits,
) -> Result<(), BytecodeError> {
    let types = unit["types"]
        .as_object()
        .ok_or_else(|| invalid_payload("types is not an object"))?;
    if types.len() > limits.max_functions {
        return Err(BytecodeError::LimitExceeded("type count"));
    }
    for (key, value) in types {
        let ty = value
            .as_object()
            .ok_or_else(|| invalid_payload(format!("type `{key}` is not an object")))?;
        require_object_fields(ty, &["name", "fields"], &format!("type `{key}`"))?;
        let name = ty["name"]
            .as_str()
            .ok_or_else(|| invalid_payload(format!("type `{key}` name is not a string")))?;
        if name != key {
            return Err(invalid_payload(format!(
                "type table key `{key}` does not match metadata name `{name}`"
            )));
        }
        let fields = ty["fields"]
            .as_array()
            .ok_or_else(|| invalid_payload(format!("type `{key}` fields is not an array")))?;
        if fields.len() > limits.max_registers_per_function {
            return Err(BytecodeError::LimitExceeded("fields per type"));
        }
        let mut field_names = BTreeSet::new();
        for field in fields {
            let field = field
                .as_object()
                .ok_or_else(|| invalid_payload(format!("type `{key}` field is not an object")))?;
            require_object_fields(
                field,
                &["name", "type_name"],
                &format!("type `{key}` field"),
            )?;
            let field_name = field["name"].as_str().ok_or_else(|| {
                invalid_payload(format!("type `{key}` field name is not a string"))
            })?;
            let type_name = field["type_name"].as_str().ok_or_else(|| {
                invalid_payload(format!("type `{key}` field type is not a string"))
            })?;
            if field_name.is_empty() || type_name.is_empty() || !field_names.insert(field_name) {
                return Err(invalid_payload(format!(
                    "type `{key}` has an empty or duplicate field `{field_name}`"
                )));
            }
        }
    }
    Ok(())
}

fn verify_native_signatures(
    unit: &serde_json::Map<String, serde_json::Value>,
    functions: &[serde_json::Value],
    limits: BytecodeLimits,
) -> Result<(), BytecodeError> {
    let signatures = unit["native_signatures"]
        .as_object()
        .ok_or_else(|| invalid_payload("native_signatures is not an object"))?;
    if signatures.len() > limits.max_functions {
        return Err(BytecodeError::LimitExceeded("native signature count"));
    }
    let function_ids = unit["function_ids"]
        .as_object()
        .ok_or_else(|| invalid_payload("function_ids is not an object"))?;
    if signatures.keys().collect::<BTreeSet<_>>() != function_ids.keys().collect::<BTreeSet<_>>() {
        return Err(invalid_payload(
            "native signature names differ from the public function map",
        ));
    }
    for (name, value) in signatures {
        let signature = value.as_object().ok_or_else(|| {
            invalid_payload(format!("native signature `{name}` is not an object"))
        })?;
        require_object_fields(
            signature,
            &["params", "return_type"],
            &format!("native signature `{name}`"),
        )?;
        let params = signature["params"].as_array().ok_or_else(|| {
            invalid_payload(format!("native signature `{name}` params is not an array"))
        })?;
        if params
            .iter()
            .any(|parameter| parameter.as_str().is_none_or(str::is_empty))
        {
            return Err(invalid_payload(format!(
                "native signature `{name}` has an invalid parameter type"
            )));
        }
        if !signature["return_type"].is_null()
            && signature["return_type"].as_str().is_none_or(str::is_empty)
        {
            return Err(invalid_payload(format!(
                "native signature `{name}` has an invalid return type"
            )));
        }
        let function_id = json_usize(&function_ids[name], "function id")?;
        let expected = functions[function_id]
            .get("params")
            .ok_or_else(|| invalid_payload(format!("function `{name}` is missing params")))?;
        require_arity(
            function_id,
            0,
            "native signature",
            json_usize(expected, "params")?,
            params.len(),
        )?;
    }
    Ok(())
}

fn verify_call_shapes(
    function_id: usize,
    code: &[serde_json::Value],
    functions: &[serde_json::Value],
    imports: &[ExternalImport],
) -> Result<(), BytecodeError> {
    let imports = imports
        .iter()
        .map(|import| (import.symbol.as_str(), import))
        .collect::<BTreeMap<_, _>>();
    for (ip, instruction) in code.iter().enumerate() {
        let (opcode, fields) = instruction_parts(function_id, ip, instruction)?;
        let args = fields.get("args").and_then(serde_json::Value::as_array);
        if let Some(args) = args {
            verify_mut_argument_indexes(function_id, ip, fields, args.len())?;
        }
        match opcode {
            "MakeClosure" => {
                let target = required_index(fields, "function")?;
                let (_, captures) = function_inputs(functions, target)?;
                let actual = fields
                    .get("captures")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| invalid_payload("MakeClosure captures is not an array"))?
                    .len();
                require_arity(function_id, ip, opcode, captures, actual)?;
            }
            "CallKnown" | "SpawnTask" => {
                let target = required_index(fields, "function")?;
                let (parameters, captures) = function_inputs(functions, target)?;
                if captures != 0 {
                    return Err(invalid_payload(format!(
                        "function {function_id} instruction {ip} `{opcode}` directly calls function {target} with {captures} capture(s)"
                    )));
                }
                let actual = args
                    .ok_or_else(|| invalid_payload(format!("{opcode} args is not an array")))?
                    .len();
                require_arity(function_id, ip, opcode, parameters, actual)?;
            }
            "CallDynamic" => {
                let actual = args
                    .ok_or_else(|| invalid_payload("CallDynamic args is not an array"))?
                    .len();
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
                    let (parameters, captures) = function_inputs(functions, target)?;
                    if captures != 0 {
                        return Err(invalid_payload(format!(
                            "function {function_id} instruction {ip} dynamic target {target} has captures"
                        )));
                    }
                    require_arity(function_id, ip, opcode, parameters, actual)?;
                }
            }
            "CallExternal" => {
                let symbol = fields
                    .get("key")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| invalid_payload("CallExternal key is not a string"))?;
                let import = imports.get(symbol).ok_or_else(|| {
                    invalid_payload(format!("CallExternal `{symbol}` has no import"))
                })?;
                let args =
                    args.ok_or_else(|| invalid_payload("CallExternal args is not an array"))?;
                require_arity(
                    function_id,
                    ip,
                    opcode,
                    import.signature.parameters.len(),
                    args.len(),
                )?;
                let actual_mut = mut_argument_indexes(fields, args.len())?;
                let expected_mut = import
                    .signature
                    .parameters
                    .iter()
                    .enumerate()
                    .filter_map(|(index, parameter)| {
                        (parameter.effect == rsscript_abi_model::DataEffect::Mut).then_some(index)
                    })
                    .collect::<BTreeSet<_>>();
                if actual_mut != expected_mut {
                    return Err(invalid_payload(format!(
                        "function {function_id} instruction {ip} external `{symbol}` mut_args differ: actual={actual_mut:?}, expected={expected_mut:?}"
                    )));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn function_inputs(
    functions: &[serde_json::Value],
    target: usize,
) -> Result<(usize, usize), BytecodeError> {
    let function = functions
        .get(target)
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| invalid_payload(format!("missing function {target}")))?;
    Ok((
        required_index(function, "params")?,
        required_index(function, "captures")?,
    ))
}

fn verify_mut_argument_indexes(
    function_id: usize,
    ip: usize,
    fields: &serde_json::Map<String, serde_json::Value>,
    argument_count: usize,
) -> Result<(), BytecodeError> {
    mut_argument_indexes(fields, argument_count)
        .map(|_| ())
        .map_err(|error| {
            invalid_payload(format!(
                "function {function_id} instruction {ip} has invalid mut_args: {error}"
            ))
        })
}

fn mut_argument_indexes(
    fields: &serde_json::Map<String, serde_json::Value>,
    argument_count: usize,
) -> Result<BTreeSet<usize>, BytecodeError> {
    let Some(values) = fields.get("mut_args") else {
        return Ok(BTreeSet::new());
    };
    let values = values
        .as_array()
        .ok_or_else(|| invalid_payload("mut_args is not an array"))?;
    let mut indexes = BTreeSet::new();
    for value in values {
        let index = json_usize(value, "mut_args")?;
        if index >= argument_count {
            return Err(invalid_payload(format!(
                "index {index} exceeds argument count {argument_count}"
            )));
        }
        if !indexes.insert(index) {
            return Err(invalid_payload(format!("duplicate index {index}")));
        }
    }
    Ok(indexes)
}

fn require_arity(
    function_id: usize,
    ip: usize,
    opcode: &str,
    expected: usize,
    actual: usize,
) -> Result<(), BytecodeError> {
    if actual == expected {
        Ok(())
    } else {
        Err(invalid_payload(format!(
            "function {function_id} instruction {ip} `{opcode}` has {actual} argument(s), expected {expected}"
        )))
    }
}

/// Prove definite register initialization over every reachable control-flow
/// path. Incoming states are intersected at joins, so a value initialized on
/// only one branch cannot be consumed after the merge.
fn verify_register_initialization(
    function_id: usize,
    initialized_inputs: BTreeSet<usize>,
    code: &[serde_json::Value],
) -> Result<(), BytecodeError> {
    if code.is_empty() {
        return Ok(());
    }
    let mut incoming = vec![None::<BTreeSet<usize>>; code.len()];
    incoming[0] = Some(initialized_inputs);
    let mut work = VecDeque::from([0usize]);

    while let Some(ip) = work.pop_front() {
        let state = incoming[ip]
            .clone()
            .expect("queued instructions always have incoming state");
        let (opcode, fields) = instruction_parts(function_id, ip, &code[ip])?;
        let (reads, writes) = register_accesses(opcode, fields)?;
        if let Some(register) = reads.iter().find(|register| !state.contains(register)) {
            return Err(invalid_payload(format!(
                "function {function_id} instruction {ip} `{opcode}` reads uninitialized register {register}"
            )));
        }
        let mut after = state;
        after.extend(writes);

        match opcode {
            "Return" | "RuntimeError" => {}
            "Jump" => enqueue_state(
                &mut incoming,
                &mut work,
                required_index(fields, "target")?,
                after,
            ),
            "JumpIfBool" | "JumpIfIntCompare" => {
                enqueue_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "target")?,
                    after.clone(),
                );
                enqueue_fallthrough(&mut incoming, &mut work, ip, after);
            }
            "MatchOption" => enqueue_two_targets(
                &mut incoming,
                &mut work,
                fields,
                "some_ip",
                "none_ip",
                after,
            )?,
            "MatchResult" => {
                enqueue_two_targets(&mut incoming, &mut work, fields, "ok_ip", "err_ip", after)?
            }
            "MatchVariant" => enqueue_two_targets(
                &mut incoming,
                &mut work,
                fields,
                "match_ip",
                "else_ip",
                after,
            )?,
            "MatchMapGet" | "MatchSortedMapGet" => {
                let mut some = after.clone();
                some.insert(required_index(fields, "value_dst")?);
                enqueue_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "some_ip")?,
                    some,
                );
                enqueue_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "none_ip")?,
                    after,
                );
            }
            _ => enqueue_fallthrough(&mut incoming, &mut work, ip, after),
        }
    }
    Ok(())
}

fn required_index(
    fields: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<usize, BytecodeError> {
    json_usize(
        fields
            .get(field)
            .ok_or_else(|| invalid_payload(format!("missing `{field}` field")))?,
        field,
    )
}

fn instruction_parts(
    function_id: usize,
    ip: usize,
    instruction: &serde_json::Value,
) -> Result<(&str, &serde_json::Map<String, serde_json::Value>), BytecodeError> {
    if let Some(opcode) = instruction.as_str()
        && opcode == "TailCallGuard"
    {
        static EMPTY: std::sync::LazyLock<serde_json::Map<String, serde_json::Value>> =
            std::sync::LazyLock::new(serde_json::Map::new);
        return Ok(("TailCallGuard", &EMPTY));
    }
    let (opcode, fields) = instruction
        .as_object()
        .filter(|outer| outer.len() == 1)
        .and_then(|outer| outer.iter().next())
        .and_then(|(opcode, fields)| fields.as_object().map(|fields| (opcode, fields)))
        .ok_or_else(|| {
            invalid_payload(format!(
                "function {function_id} instruction {ip} has invalid encoding"
            ))
        })?;
    Ok((opcode, fields))
}

fn register_accesses(
    opcode: &str,
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Result<(BTreeSet<usize>, BTreeSet<usize>), BytecodeError> {
    let mut reads = BTreeSet::new();
    let mut writes = BTreeSet::new();
    for (field, value) in fields {
        if field == "dst" {
            writes.insert(json_usize(value, field)?);
        } else if field == "value_dst" {
            // MatchMapGet initializes this register only on its `some` edge.
        } else if opcode == "SelectWait" && matches!(field.as_str(), "winner" | "value") {
            writes.insert(json_usize(value, field)?);
        } else if scalar_register_field(opcode, field) {
            reads.insert(json_usize(value, field)?);
        } else if matches!(
            field.as_str(),
            "args" | "captures" | "cleanup" | "handles" | "items"
        ) {
            for register in value
                .as_array()
                .ok_or_else(|| invalid_payload(format!("{field} is not an array")))?
            {
                reads.insert(json_usize(register, field)?);
            }
        } else if field == "fields" {
            collect_tuple_registers(value, &[1], &mut reads)?;
        } else if field == "entries" {
            collect_tuple_registers(value, &[0, 1], &mut reads)?;
        }
    }
    if matches!(opcode, "DeepCopy" | "DeepCopyElided") {
        writes.extend(reads.iter().copied());
    }
    Ok((reads, writes))
}

fn scalar_register_field(opcode: &str, field: &str) -> bool {
    matches!(
        field,
        "src"
            | "reg"
            | "base"
            | "lhs"
            | "rhs"
            | "cond"
            | "resource"
            | "map"
            | "list"
            | "closure"
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
        || (field == "value" && !opcode.starts_with("Load") && opcode != "SelectWait")
        || (field == "index" && matches!(opcode, "ListGet" | "ListRemoveAt" | "ListSet"))
}

fn collect_tuple_registers(
    value: &serde_json::Value,
    positions: &[usize],
    output: &mut BTreeSet<usize>,
) -> Result<(), BytecodeError> {
    for entry in value
        .as_array()
        .ok_or_else(|| invalid_payload("register tuple list is not an array"))?
    {
        let tuple = entry
            .as_array()
            .ok_or_else(|| invalid_payload("register tuple entry is not an array"))?;
        for position in positions {
            output.insert(json_usize(
                tuple
                    .get(*position)
                    .ok_or_else(|| invalid_payload("register tuple is incomplete"))?,
                "tuple",
            )?);
        }
    }
    Ok(())
}

fn enqueue_two_targets(
    incoming: &mut [Option<BTreeSet<usize>>],
    work: &mut VecDeque<usize>,
    fields: &serde_json::Map<String, serde_json::Value>,
    first: &str,
    second: &str,
    state: BTreeSet<usize>,
) -> Result<(), BytecodeError> {
    enqueue_state(
        incoming,
        work,
        required_index(fields, first)?,
        state.clone(),
    );
    enqueue_state(incoming, work, required_index(fields, second)?, state);
    Ok(())
}

fn enqueue_fallthrough(
    incoming: &mut [Option<BTreeSet<usize>>],
    work: &mut VecDeque<usize>,
    ip: usize,
    state: BTreeSet<usize>,
) {
    if ip + 1 < incoming.len() {
        enqueue_state(incoming, work, ip + 1, state);
    }
}

fn enqueue_state(
    incoming: &mut [Option<BTreeSet<usize>>],
    work: &mut VecDeque<usize>,
    target: usize,
    state: BTreeSet<usize>,
) {
    match &mut incoming[target] {
        None => {
            incoming[target] = Some(state);
            work.push_back(target);
        }
        Some(existing) => {
            let intersection = existing.intersection(&state).copied().collect();
            if *existing != intersection {
                *existing = intersection;
                work.push_back(target);
            }
        }
    }
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
    require_object_fields(
        fields,
        instruction_fields(opcode),
        &format!("function {function_id} instruction {ip} `{opcode}`"),
    )?;
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

fn instruction_fields(opcode: &str) -> &'static [&'static str] {
    match opcode {
        "LoadUnit" | "LoadNone" => &["dst"],
        "LoadInt" | "LoadFloat" | "LoadBool" | "LoadString" | "LoadChar" => &["dst", "value"],
        "Move" | "Manage" | "UnwrapSome" | "AwaitJoin" => &["dst", "src"],
        "DeepCopy" | "DeepCopyElided" => &["reg"],
        "GetField" => &["dst", "base", "name"],
        "GetFieldSlot" => &["dst", "base", "slot"],
        "SetFieldSlot" => &["dst", "base", "slot", "value"],
        "SetField" => &["dst", "base", "name", "value"],
        "MakeStruct" | "MakeVariant" => &["dst", "layout", "fields"],
        "ResourceDrop" => &["resource"],
        "MakeList" => &["dst", "items"],
        "MakeObject" => &["dst", "fields"],
        "MakeMap" => &["dst", "entries"],
        "AddInt" | "SubInt" | "MulInt" | "DivInt" | "ModInt" | "BitAndInt" | "BitOrInt"
        | "BitXorInt" | "ShiftLeftInt" | "ShiftRightInt" | "LessInt" | "LessEqualInt"
        | "GreaterInt" | "GreaterEqualInt" | "Equal" | "NotEqual" => &["dst", "lhs", "rhs"],
        "Jump" => &["target"],
        "JumpIfBool" => &["cond", "expected", "target"],
        "JumpIfIntCompare" => &["lhs", "rhs", "op", "expected", "target"],
        "MatchOption" => &["src", "some_ip", "none_ip"],
        "MatchResult" => &["src", "ok_ip", "err_ip"],
        "MatchVariant" => &["src", "expected", "match_ip", "else_ip"],
        "RuntimeError" => &["message"],
        "MatchMapGet" | "MatchSortedMapGet" => &["map", "key", "value_dst", "some_ip", "none_ip"],
        "UnwrapVariantValue" => &["dst", "src", "expected"],
        "MakeClosure" => &["dst", "function", "captures"],
        "MakeSome" => &["dst", "value"],
        "CallKnown" => &["dst", "function", "args", "mut_args"],
        "CallDynamic" => &["dst", "dispatch", "args", "mut_args"],
        "SpawnTask" => &["dst", "function", "args"],
        "SelectWait" => &["handles", "winner", "value"],
        "CallExternal" => &["dst", "key", "args", "mut_args"],
        "CallClosure" => &["dst", "closure", "args", "mut_args"],
        "NativeGuardClosureId" => &["closure", "expected"],
        "NativeClosureId" => &["dst", "closure"],
        "NativeClosureCapture" => &["dst", "closure", "index"],
        "NativeFieldClosureId" => &["dst", "base", "slot"],
        "NativeFieldClosureCapture" => &["dst", "base", "slot", "index"],
        "ListFilter" => &["dst", "list", "predicate"],
        "ListFold" => &["dst", "list", "state", "folder"],
        "ListGet" | "ListRemoveAt" => &["dst", "list", "index"],
        "ListLen" | "ListClear" | "ListPop" | "ListSort" => &["dst", "list"],
        "ListMap" => &["dst", "list", "mapper"],
        "ListAppend" => &["dst", "list", "values"],
        "ListPush" => &["dst", "list", "value"],
        "ListSet" => &["dst", "list", "index", "value"],
        "ListSortBy" => &["dst", "list", "key", "compare"],
        "ListSortWith" => &["dst", "list", "compare"],
        "DequeClear" | "DequePopBack" | "DequePopFront" => &["dst", "deque"],
        "DequePushBack" | "DequePushFront" => &["dst", "deque", "value"],
        "SetClear" | "SortedSetClear" => &["dst", "set"],
        "SetForEach" => &["dst", "set", "callback"],
        "SetInsert" | "SetRemove" | "SortedSetInsert" | "SortedSetRemove" => {
            &["dst", "set", "value"]
        }
        "SortedMapClear" | "MapClear" => &["dst", "map"],
        "SortedMapInsert" | "MapInsertOld" | "MapInsert" => &["dst", "map", "key", "value"],
        "SortedMapRemove" | "MapGet" | "MapRemove" => &["dst", "map", "key"],
        "BufferClear" => &["dst", "buffer"],
        "StringBuilderPush" => &["dst", "builder", "value"],
        "StringBuilderFinish" => &["dst", "builder"],
        "StringConcat" => &["dst", "left", "right"],
        "CallIntrinsic" => &["dst", "intrinsic", "args"],
        "CallTypedIntrinsic" => &["dst", "intrinsic", "type_arg", "args"],
        "TryResult" => &["dst", "src", "cleanup"],
        "Return" => &["src"],
        _ => &[],
    }
}

fn verify_register_field(
    function_id: usize,
    ip: usize,
    register_count: usize,
    opcode: &str,
    field: &str,
    value: &serde_json::Value,
) -> Result<(), BytecodeError> {
    let scalar_register = field == "dst"
        || field == "value_dst"
        || (opcode == "SelectWait" && matches!(field, "winner" | "value"))
        || scalar_register_field(opcode, field);
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

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
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
    Cancelled,
    DeadlineExceeded,
    InvalidMagic,
    UnsupportedSchema(String),
    UnsupportedLanguageVersion(String),
    UnsupportedBytecodeIsa { artifact: u32, verifier: u32 },
    UnsupportedRuntimeAbi { artifact: u32, runtime: u32 },
    InvalidProvenance(&'static str),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BytecodeErrorCode {
    Cancelled,
    DeadlineExceeded,
    InvalidMagic,
    UnsupportedSchema,
    UnsupportedLanguageVersion,
    UnsupportedBytecodeIsa,
    UnsupportedRuntimeAbi,
    InvalidProvenance,
    LimitExceeded,
    ExecutableHashMismatch,
    ChecksumMismatch,
    ImportsNotCanonical,
    ImportAbiMismatch,
    ImportSignatureHashMismatch,
    InvalidPayload,
    MalformedSectionTable,
    SectionsNotCanonical,
    MissingSection,
    UnknownRequiredSection,
    KnownSectionNotRequired,
    InvalidSectionFlags,
    SectionHashMismatch,
    MalformedChecksum,
    TrailingBytes,
    Encode,
    Cbor,
}

impl BytecodeError {
    pub fn code(&self) -> BytecodeErrorCode {
        match self {
            Self::Cancelled => BytecodeErrorCode::Cancelled,
            Self::DeadlineExceeded => BytecodeErrorCode::DeadlineExceeded,
            Self::InvalidMagic => BytecodeErrorCode::InvalidMagic,
            Self::UnsupportedSchema(_) => BytecodeErrorCode::UnsupportedSchema,
            Self::UnsupportedLanguageVersion(_) => BytecodeErrorCode::UnsupportedLanguageVersion,
            Self::UnsupportedBytecodeIsa { .. } => BytecodeErrorCode::UnsupportedBytecodeIsa,
            Self::UnsupportedRuntimeAbi { .. } => BytecodeErrorCode::UnsupportedRuntimeAbi,
            Self::InvalidProvenance(_) => BytecodeErrorCode::InvalidProvenance,
            Self::LimitExceeded(_) => BytecodeErrorCode::LimitExceeded,
            Self::ExecutableHashMismatch => BytecodeErrorCode::ExecutableHashMismatch,
            Self::ChecksumMismatch => BytecodeErrorCode::ChecksumMismatch,
            Self::ImportsNotCanonical => BytecodeErrorCode::ImportsNotCanonical,
            Self::ImportAbiMismatch => BytecodeErrorCode::ImportAbiMismatch,
            Self::ImportSignatureHashMismatch => BytecodeErrorCode::ImportSignatureHashMismatch,
            Self::InvalidPayload(_) => BytecodeErrorCode::InvalidPayload,
            Self::MalformedSectionTable => BytecodeErrorCode::MalformedSectionTable,
            Self::SectionsNotCanonical => BytecodeErrorCode::SectionsNotCanonical,
            Self::MissingSection(_) => BytecodeErrorCode::MissingSection,
            Self::UnknownRequiredSection(_) => BytecodeErrorCode::UnknownRequiredSection,
            Self::KnownSectionNotRequired(_) => BytecodeErrorCode::KnownSectionNotRequired,
            Self::InvalidSectionFlags { .. } => BytecodeErrorCode::InvalidSectionFlags,
            Self::SectionHashMismatch(_) => BytecodeErrorCode::SectionHashMismatch,
            Self::MalformedChecksum => BytecodeErrorCode::MalformedChecksum,
            Self::TrailingBytes => BytecodeErrorCode::TrailingBytes,
            Self::Encode(_) => BytecodeErrorCode::Encode,
            Self::Cbor(_) => BytecodeErrorCode::Cbor,
        }
    }
}

impl fmt::Display for BytecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => formatter.write_str("bytecode verification cancelled"),
            Self::DeadlineExceeded => {
                formatter.write_str("bytecode verification deadline exceeded")
            }
            Self::InvalidMagic => formatter.write_str("invalid RSScript bytecode magic"),
            Self::UnsupportedSchema(schema) => {
                write!(formatter, "unsupported bytecode schema `{schema}`")
            }
            Self::UnsupportedLanguageVersion(version) => {
                write!(
                    formatter,
                    "unsupported RSScript language version `{version}`"
                )
            }
            Self::UnsupportedBytecodeIsa { artifact, verifier } => write!(
                formatter,
                "bytecode ISA {artifact} is incompatible with verifier ISA {verifier}"
            ),
            Self::UnsupportedRuntimeAbi { artifact, runtime } => write!(
                formatter,
                "bytecode runtime ABI {artifact} is incompatible with runtime ABI {runtime}"
            ),
            Self::InvalidProvenance(field) => {
                write!(formatter, "bytecode {field} is malformed")
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

    const TEST_CATALOG_DIGEST: &str =
        "sha256:0000000000000000000000000000000000000000000000000000000000000000";
    const TEST_SOURCE_DIGEST: &str =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111";

    #[test]
    fn round_trip_requires_intact_artifact() {
        let payload = minimal_payload();
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload.clone(),
        )
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
    fn language_compatibility_is_independent_from_compiler_provenance() {
        let compatibility = BytecodeCompatibility::default();
        assert!(
            compatibility.language.matches(
                &Version::parse(LANGUAGE_SEMANTICS_VERSION)
                    .expect("declared language semantics version")
            )
        );
        assert!(
            !compatibility
                .language
                .matches(&Version::parse("0.2.0").expect("test version"))
        );
        assert_eq!(BYTECODE_SCHEMA, "rsscript.bytecode.v1");
        assert_eq!(BYTECODE_CONTAINER_FORMAT_VERSION, 1);
        assert_eq!(
            u16::from_le_bytes([BYTECODE_MAGIC[6], BYTECODE_MAGIC[7]]),
            BYTECODE_CONTAINER_FORMAT_VERSION
        );
        assert_eq!(BYTECODE_ISA_VERSION, 1);
    }

    #[test]
    fn verification_observes_cancellation_and_deadline_before_decoding() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            BytecodeVerifier::default().verify_with_context(
                b"not bytecode",
                VerificationContext {
                    cancellation: Some(&cancellation),
                    deadline: None,
                },
            ),
            Err(BytecodeError::Cancelled)
        ));
        assert!(matches!(
            BytecodeVerifier::default().verify_with_context(
                b"not bytecode",
                VerificationContext {
                    cancellation: None,
                    deadline: Some(MonotonicDeadline::at(
                        std::time::Instant::now() - std::time::Duration::from_millis(1),
                    )),
                },
            ),
            Err(BytecodeError::DeadlineExceeded)
        ));
    }

    #[test]
    fn verifier_errors_expose_stable_machine_codes() {
        assert_eq!(
            BytecodeError::InvalidMagic.code(),
            BytecodeErrorCode::InvalidMagic
        );
        assert_eq!(
            serde_json::to_string(&BytecodeError::InvalidMagic.code()).unwrap(),
            "\"invalid_magic\""
        );
        assert_eq!(
            BytecodeError::UnsupportedBytecodeIsa {
                artifact: 9,
                verifier: 1,
            }
            .code(),
            BytecodeErrorCode::UnsupportedBytecodeIsa
        );
        assert_eq!(
            BytecodeError::UnsupportedRuntimeAbi {
                artifact: 9,
                runtime: 1,
            }
            .code(),
            BytecodeErrorCode::UnsupportedRuntimeAbi
        );
    }

    #[test]
    fn verifier_rejects_incompatible_language_and_runtime_versions() {
        let future_language = BytecodeArtifact::new(
            "9.0.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            minimal_payload(),
        )
        .unwrap();
        assert!(matches!(
            BytecodeVerifier::default().verify(&future_language.to_bytes().unwrap()),
            Err(BytecodeError::UnsupportedLanguageVersion(version)) if version == "9.0.0"
        ));

        let future_abi = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION + 1,
            TEST_SOURCE_DIGEST,
            vec![],
            minimal_payload(),
        )
        .unwrap();
        assert!(matches!(
            BytecodeVerifier::default().verify(&future_abi.to_bytes().unwrap()),
            Err(BytecodeError::UnsupportedRuntimeAbi { artifact, runtime })
                if artifact == RUNTIME_ABI_VERSION + 1 && runtime == RUNTIME_ABI_VERSION
        ));

        let mut future_isa = future_abi.clone();
        future_isa.header.bytecode_isa_version = BYTECODE_ISA_VERSION + 1;
        future_isa.checksum = future_isa.compute_checksum().unwrap();
        assert!(matches!(
            BytecodeVerifier::default().verify(&future_isa.to_bytes().unwrap()),
            Err(BytecodeError::UnsupportedBytecodeIsa { artifact, verifier })
                if artifact == BYTECODE_ISA_VERSION + 1 && verifier == BYTECODE_ISA_VERSION
        ));
    }

    #[test]
    fn artifact_sections_and_instruction_payload_use_binary_cbor() {
        let payload = minimal_payload();
        assert_ne!(payload.first(), Some(&b'{'));
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload,
        )
        .expect("artifact");
        let bytes = artifact.to_bytes().expect("bytes");
        let first_section_data = BYTECODE_MAGIC.len() + 2 + SECTION_HEADER_BYTES;
        assert_ne!(bytes.get(first_section_data), Some(&b'{'));
        BytecodeVerifier::default().verify(&bytes).unwrap();
    }

    #[test]
    fn unknown_optional_sections_are_forward_compatible() {
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            minimal_payload(),
        )
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
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            minimal_payload(),
        )
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
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            minimal_payload(),
        )
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
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload,
        )
        .expect("artifact");

        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().expect("bytes")),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("unknown opcode")
        ));
    }

    #[test]
    fn verifier_rejects_missing_or_unknown_instruction_fields() {
        for instruction in [
            serde_json::json!({"LoadUnit": {}}),
            serde_json::json!({"LoadUnit": {"dst": 0, "future": true}}),
        ] {
            let payload = encode_executable_payload(&serde_json::json!({
                "functions": [{
                    "name": "main", "params": 0, "captures": 0, "regs": 1,
                    "local_regs": {}, "code": [instruction.clone()]
                }],
                "function_ids": {"main": 0}, "resource_drop_functions": {},
                "types": {}, "native_signatures": {}, "closure_identity_observable": false
            }))
            .unwrap();
            let artifact = BytecodeArtifact::new(
                "0.1.0",
                "0.1.0",
                TEST_CATALOG_DIGEST,
                RUNTIME_ABI_VERSION,
                TEST_SOURCE_DIGEST,
                vec![],
                payload,
            )
            .unwrap();
            assert!(matches!(
                BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
                Err(BytecodeError::InvalidPayload(message)) if message.contains("fields differ")
            ));
        }
    }

    #[test]
    fn every_known_opcode_has_an_exact_field_contract() {
        for opcode in KNOWN_OPCODES {
            assert!(
                !instruction_fields(opcode).is_empty(),
                "missing verifier field contract for {opcode}"
            );
        }
    }

    #[test]
    fn verifier_rejects_inconsistent_type_and_function_metadata() {
        let artifact_for = |types: serde_json::Value, signatures: serde_json::Value| {
            BytecodeArtifact::new(
                "0.1.0",
                "0.1.0",
                TEST_CATALOG_DIGEST,
                RUNTIME_ABI_VERSION,
                TEST_SOURCE_DIGEST,
                vec![],
                encode_executable_payload(&serde_json::json!({
                    "functions": [{
                        "name": "main", "params": 0, "captures": 0, "regs": 1,
                        "local_regs": {},
                        "code": [{"LoadUnit": {"dst": 0}}, {"Return": {"src": 0}}]
                    }],
                    "function_ids": {"main": 0}, "resource_drop_functions": {},
                    "types": types, "native_signatures": signatures,
                    "closure_identity_observable": false
                }))
                .unwrap(),
            )
            .unwrap()
        };

        let bad_type = artifact_for(
            serde_json::json!({"Expected": {"name": "Different", "fields": []}}),
            serde_json::json!({"main": {"params": [], "return_type": "Unit"}}),
        );
        assert!(matches!(
            BytecodeVerifier::default().verify(&bad_type.to_bytes().unwrap()),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("does not match metadata name")
        ));

        let bad_signature = artifact_for(
            serde_json::json!({}),
            serde_json::json!({"main": {"params": ["Int"], "return_type": "Unit"}}),
        );
        assert!(matches!(
            BytecodeVerifier::default().verify(&bad_signature.to_bytes().unwrap()),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("expected 0")
        ));
    }

    #[test]
    fn resource_drop_fields_are_verified_implicit_inputs() {
        let artifact_for = |locals: serde_json::Value| {
            BytecodeArtifact::new(
                "0.1.0",
                "0.1.0",
                TEST_CATALOG_DIGEST,
                RUNTIME_ABI_VERSION,
                TEST_SOURCE_DIGEST,
                vec![],
                encode_executable_payload(&serde_json::json!({
                    "functions": [{
                        "name": "<drop:File>", "params": 0, "captures": 0, "regs": 1,
                        "local_regs": locals, "code": [{"Return": {"src": 0}}]
                    }],
                    "function_ids": {}, "resource_drop_functions": {"File": 0},
                    "types": {"File": {"name": "File", "fields": [
                        {"name": "path", "type_name": "String"}
                    ]}},
                    "native_signatures": {}, "closure_identity_observable": false
                }))
                .unwrap(),
            )
            .unwrap()
        };

        BytecodeVerifier::default()
            .verify(
                &artifact_for(serde_json::json!({"path": 0}))
                    .to_bytes()
                    .unwrap(),
            )
            .expect("resource fields initialize drop registers");
        assert!(matches!(
            BytecodeVerifier::default().verify(
                &artifact_for(serde_json::json!({})).to_bytes().unwrap()
            ),
            Err(BytecodeError::InvalidPayload(message))
                if message.contains("missing field register `path`")
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
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload,
        )
        .expect("artifact");

        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().expect("bytes")),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("register 1")
        ));
    }

    #[test]
    fn verifier_rejects_uninitialized_register_reads() {
        let payload = encode_executable_payload(&serde_json::json!({
            "functions": [{
                "name": "main", "params": 0, "captures": 0, "regs": 1,
                "local_regs": {}, "code": [{"Return": {"src": 0}}]
            }],
            "function_ids": {"main": 0}, "resource_drop_functions": {},
            "types": {}, "native_signatures": {}, "closure_identity_observable": false
        }))
        .unwrap();
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload,
        )
        .unwrap();

        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
            Err(BytecodeError::InvalidPayload(message))
                if message.contains("reads uninitialized register 0")
        ));
    }

    #[test]
    fn verifier_intersects_register_state_at_control_flow_joins() {
        let payload = encode_executable_payload(&serde_json::json!({
            "functions": [{
                "name": "main", "params": 0, "captures": 0, "regs": 2,
                "local_regs": {},
                "code": [
                    {"LoadBool": {"dst": 0, "value": true}},
                    {"JumpIfBool": {"cond": 0, "expected": true, "target": 3}},
                    {"LoadInt": {"dst": 1, "value": 7}},
                    {"Return": {"src": 1}}
                ]
            }],
            "function_ids": {"main": 0}, "resource_drop_functions": {},
            "types": {}, "native_signatures": {}, "closure_identity_observable": false
        }))
        .unwrap();
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload,
        )
        .unwrap();

        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
            Err(BytecodeError::InvalidPayload(message))
                if message.contains("reads uninitialized register 1")
        ));
    }

    #[test]
    fn verifier_counts_captures_and_parameters_in_the_input_window() {
        let payload = encode_executable_payload(&serde_json::json!({
            "functions": [{
                "name": "closure", "params": 1, "captures": 1, "regs": 1,
                "local_regs": {}, "code": []
            }],
            "function_ids": {"closure": 0}, "resource_drop_functions": {},
            "types": {}, "native_signatures": {}, "closure_identity_observable": false
        }))
        .unwrap();
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload,
        )
        .unwrap();

        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
            Err(BytecodeError::InvalidPayload(message))
                if message.contains("parameters and captures")
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
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![ExternalImport {
                symbol: ExternalSymbol::new("host.log.emit").unwrap(),
                signature,
                signature_hash: wrong_hash,
                abi_version: RUNTIME_ABI_VERSION,
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

    #[test]
    fn verifier_rejects_static_call_arity_mismatch() {
        let payload = encode_executable_payload(&serde_json::json!({
            "functions": [
                {
                    "name": "main", "params": 0, "captures": 0, "regs": 1,
                    "local_regs": {},
                    "code": [{"CallKnown": {"dst": 0, "function": 1, "args": [], "mut_args": []}}]
                },
                {
                    "name": "callee", "params": 1, "captures": 0, "regs": 1,
                    "local_regs": {}, "code": [{"Return": {"src": 0}}]
                }
            ],
            "function_ids": {"main": 0, "callee": 1}, "resource_drop_functions": {},
            "types": {}, "native_signatures": {}, "closure_identity_observable": false
        }))
        .unwrap();
        let artifact = BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            TEST_CATALOG_DIGEST,
            RUNTIME_ABI_VERSION,
            TEST_SOURCE_DIGEST,
            vec![],
            payload,
        )
        .unwrap();

        assert!(matches!(
            BytecodeVerifier::default().verify(&artifact.to_bytes().unwrap()),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("expected 1")
        ));
    }

    #[test]
    fn verifier_rejects_external_argument_and_effect_mismatch() {
        let signature = FunctionSignature {
            parameters: vec![ParameterSignature {
                name: "value".into(),
                effect: DataEffect::Mut,
                ty: "Int".into(),
                retained: false,
            }],
            result: "Unit".into(),
            asynchronous: false,
        };
        let symbol = ExternalSymbol::new("host.test.mutate").unwrap();
        let artifact_for = |args: serde_json::Value, mut_args: serde_json::Value| {
            BytecodeArtifact::new(
                "0.1.0",
                "0.1.0",
                TEST_CATALOG_DIGEST,
                RUNTIME_ABI_VERSION,
                TEST_SOURCE_DIGEST,
                vec![ExternalImport {
                    symbol: symbol.clone(),
                    signature: signature.clone(),
                    signature_hash: signature.hash(),
                    abi_version: RUNTIME_ABI_VERSION,
                }],
                encode_executable_payload(&serde_json::json!({
                    "functions": [{
                        "name": "main", "params": 1, "captures": 0, "regs": 2,
                        "local_regs": {},
                        "code": [{"CallExternal": {
                            "dst": 1, "key": "host.test.mutate", "args": args, "mut_args": mut_args
                        }}]
                    }],
                    "function_ids": {"main": 0}, "resource_drop_functions": {},
                    "types": {}, "native_signatures": {}, "closure_identity_observable": false
                }))
                .unwrap(),
            )
            .unwrap()
        };

        let missing_argument = artifact_for(serde_json::json!([]), serde_json::json!([]));
        assert!(matches!(
            BytecodeVerifier::default().verify(&missing_argument.to_bytes().unwrap()),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("expected 1")
        ));

        let missing_mut = artifact_for(serde_json::json!([0]), serde_json::json!([]));
        assert!(matches!(
            BytecodeVerifier::default().verify(&missing_mut.to_bytes().unwrap()),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("mut_args differ")
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
