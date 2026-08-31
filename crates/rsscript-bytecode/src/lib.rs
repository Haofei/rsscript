#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

pub use rsscript_abi_model::LANGUAGE_SEMANTICS_VERSION;
use rsscript_abi_model::{CORE_LIBRARY_ABI_VERSION, ExternalImport, RUNTIME_ABI_VERSION};
use rsscript_operation::{CancellationToken, MonotonicDeadline};
use semver::{Version, VersionReq};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod canonical_cbor;
mod typed_facts;
mod verification_metadata;

pub use typed_facts::*;

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
const SECTION_TYPED_EXECUTABLE_FACTS: u8 = 5;
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
    /// Deterministic Core-library contract used by builtin bytecode operations.
    /// This is separate from both the instruction set and Provider ABI.
    pub core_library_abi_version: u32,
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
    /// Optional, verifier-owned optimization facts. This section is not part
    /// of the executable checksum so pre-facts v1 readers can continue to
    /// consume the executable. The facts carry and verify their own binding to
    /// `header.executable_hash` before any engine may consume them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub typed_executable_facts: Option<Vec<u8>>,
}

impl BytecodeArtifact {
    pub fn new(
        language_version: impl Into<String>,
        compiler_version: impl Into<String>,
        interface_catalog_digest: impl Into<String>,
        runtime_abi_version: u32,
        source_content_hash: impl Into<String>,
        imports: Vec<ExternalImport>,
        payload: Vec<u8>,
    ) -> Result<Self, BytecodeError> {
        let mut imports = imports;
        imports.sort_by(|left, right| left.symbol.cmp(&right.symbol));
        let executable_hash = digest(&payload);
        let mut artifact = Self {
            header: BytecodeHeader {
                schema: BYTECODE_SCHEMA.to_string(),
                language_version: language_version.into(),
                bytecode_isa_version: BYTECODE_ISA_VERSION,
                core_library_abi_version: CORE_LIBRARY_ABI_VERSION,
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
            typed_executable_facts: None,
        };
        artifact.checksum = artifact.compute_checksum()?;
        Ok(artifact)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, BytecodeError> {
        let header = encode_executable_payload(&self.header)?;
        let imports = encode_executable_payload(&self.imports)?;
        let mut sections = vec![
            (SECTION_HEADER, SECTION_REQUIRED, header.as_slice()),
            (SECTION_IMPORTS, SECTION_REQUIRED, imports.as_slice()),
            (SECTION_CODE, SECTION_REQUIRED, self.payload.as_slice()),
            (SECTION_CHECKSUM, SECTION_REQUIRED, self.checksum.as_bytes()),
        ];
        if let Some(facts) = self.typed_executable_facts.as_deref() {
            sections.push((SECTION_TYPED_EXECUTABLE_FACTS, 0, facts));
        }
        let mut bytes = Vec::with_capacity(
            BYTECODE_MAGIC.len()
                + 2
                + sections
                    .iter()
                    .map(|(_, _, data)| SECTION_HEADER_BYTES + data.len())
                    .sum::<usize>(),
        );
        bytes.extend_from_slice(BYTECODE_MAGIC);
        bytes.extend_from_slice(&(sections.len() as u16).to_be_bytes());
        for (kind, flags, data) in sections {
            bytes.push(kind);
            bytes.push(flags);
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

    /// Attach canonical typed executable facts after proving that they are
    /// bound to this exact executable payload.
    pub fn attach_typed_executable_facts(
        &mut self,
        facts: &TypedExecutableFactsV1,
    ) -> Result<(), BytecodeError> {
        if facts.executable_hash != self.header.executable_hash
            || facts.bytecode_isa_version != self.header.bytecode_isa_version
            || facts.runtime_abi_version != self.header.runtime_abi_version
            || facts.interface_catalog_digest != self.header.interface_catalog_digest
            || facts.imports_hash != typed_facts_imports_hash(self)?
        {
            return Err(BytecodeError::TypedFactsBindingMismatch("artifact binding"));
        }
        self.typed_executable_facts = Some(encode_typed_executable_facts(facts)?);
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
        let mut typed_executable_facts = None;
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
                SECTION_TYPED_EXECUTABLE_FACTS => {
                    if flags & SECTION_REQUIRED != 0 {
                        return Err(BytecodeError::KnownSectionMustBeOptional(kind));
                    }
                    typed_executable_facts = Some(data.to_vec());
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
            typed_executable_facts,
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
    pub max_typed_facts_bytes: usize,
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
            max_typed_facts_bytes: 16 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VerifiedBytecode {
    artifact: BytecodeArtifact,
    typed_executable_facts: Option<BoundTypedExecutableFactsV1>,
}

impl VerifiedBytecode {
    pub fn artifact(&self) -> &BytecodeArtifact {
        &self.artifact
    }

    pub fn into_artifact(self) -> BytecodeArtifact {
        self.artifact
    }

    /// Return typed facts admitted by the independent facts verifier. Callers
    /// must never deserialize `artifact.typed_executable_facts` themselves.
    pub fn typed_executable_facts(&self) -> Option<&BoundTypedExecutableFactsV1> {
        self.typed_executable_facts.as_ref()
    }

    /// Preserve both the executable and its verifier-owned optimization facts
    /// across a VM load boundary.
    pub fn into_parts(self) -> (BytecodeArtifact, Option<BoundTypedExecutableFactsV1>) {
        (self.artifact, self.typed_executable_facts)
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
    pub core_library_abi_version: u32,
    pub runtime_abi_version: u32,
}

impl Default for BytecodeCompatibility {
    fn default() -> Self {
        Self {
            language: VersionReq::parse(SUPPORTED_LANGUAGE_SEMANTICS)
                .expect("declared language compatibility requirement"),
            bytecode_isa_version: BYTECODE_ISA_VERSION,
            core_library_abi_version: CORE_LIBRARY_ABI_VERSION,
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
        verify_artifact_contract(
            &artifact,
            ArtifactContract {
                limits: self.limits,
                language_compatibility: &self.compatibility.language,
                schema: BYTECODE_SCHEMA,
                bytecode_isa_version: self.compatibility.bytecode_isa_version,
                core_library_abi_version: self.compatibility.core_library_abi_version,
                runtime_abi_version: self.compatibility.runtime_abi_version,
                context,
            },
        )?;
        verify_executable_payload(&artifact.payload, &artifact.imports, self.limits, context)?;
        let typed_executable_facts = artifact
            .typed_executable_facts
            .as_deref()
            .map(|facts| {
                TypedExecutableFactsVerifierV1::new(self.limits.into())
                    .verify_with_context(facts, &artifact, context)
            })
            .transpose()?;
        context.check()?;
        Ok(VerifiedBytecode {
            artifact,
            typed_executable_facts,
        })
    }
}

pub(crate) struct ArtifactContract<'a> {
    limits: BytecodeLimits,
    language_compatibility: &'a VersionReq,
    schema: &'a str,
    bytecode_isa_version: u32,
    core_library_abi_version: u32,
    runtime_abi_version: u32,
    context: VerificationContext<'a>,
}

pub(crate) fn verify_artifact_contract(
    artifact: &BytecodeArtifact,
    contract: ArtifactContract<'_>,
) -> Result<(), BytecodeError> {
    let ArtifactContract {
        limits,
        language_compatibility,
        schema,
        bytecode_isa_version,
        core_library_abi_version,
        runtime_abi_version,
        context,
    } = contract;
    context.check()?;
    if artifact.header.schema != schema {
        return Err(BytecodeError::UnsupportedSchema(
            artifact.header.schema.clone(),
        ));
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
    if !language_compatibility.matches(&language) {
        return Err(BytecodeError::UnsupportedLanguageVersion(
            artifact.header.language_version.clone(),
        ));
    }
    if artifact.header.bytecode_isa_version != bytecode_isa_version {
        return Err(BytecodeError::UnsupportedBytecodeIsa {
            artifact: artifact.header.bytecode_isa_version,
            verifier: bytecode_isa_version,
        });
    }
    if artifact.header.core_library_abi_version != core_library_abi_version {
        return Err(BytecodeError::UnsupportedCoreLibraryAbi {
            artifact: artifact.header.core_library_abi_version,
            runtime: core_library_abi_version,
        });
    }
    if artifact.header.runtime_abi_version != runtime_abi_version {
        return Err(BytecodeError::UnsupportedRuntimeAbi {
            artifact: artifact.header.runtime_abi_version,
            runtime: runtime_abi_version,
        });
    }
    if artifact.payload.len() > limits.max_payload_bytes {
        return Err(BytecodeError::LimitExceeded("payload bytes"));
    }
    if artifact.imports.len() > limits.max_imports {
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
    Ok(())
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
    canonical_cbor::encode(&value).map_err(|error| BytecodeError::Cbor(error.to_string()))
}

/// Decode the executable section owned by this crate.
pub fn decode_executable_payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, BytecodeError> {
    let value =
        canonical_cbor::decode(payload).map_err(|error| BytecodeError::Cbor(error.to_string()))?;
    serde_json::from_value(value).map_err(|error| BytecodeError::Cbor(error.to_string()))
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
    require_object_fields_with_optional(
        unit,
        &[
            "functions",
            "function_ids",
            "resource_drop_functions",
            "types",
            "native_signatures",
            "closure_identity_observable",
        ],
        &["source_map", "variant_layouts"],
        "unit",
    )?;
    let functions = unit["functions"]
        .as_array()
        .ok_or_else(|| invalid_payload("functions is not an array"))?;
    if functions.len() > limits.max_functions {
        return Err(BytecodeError::LimitExceeded("function count"));
    }
    let resource_inputs = verification_metadata::resource_drop_inputs(unit, functions)?;

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
                WireInstructionContext {
                    function_id,
                    ip,
                    register_count: registers,
                    code_len: code.len(),
                    function_count: functions.len(),
                },
                instruction,
                &mut called_imports,
            )?;
        }
        let mut initialized_registers = (0..initialized_inputs).collect::<BTreeSet<_>>();
        if let Some(resources) = resource_inputs.get(&function_id) {
            initialized_registers.extend(resources);
        }
        verify_register_initialization(function_id, initialized_registers, code)?;
        verify_resource_scope_lifetimes(function_id, code)?;
        verify_call_shapes(function_id, code, functions, imports)?;
        let _ = name;
    }

    verify_function_map(unit, "function_ids", functions, true)?;
    context.check()?;
    verify_function_map(unit, "resource_drop_functions", functions, false)?;
    verification_metadata::verify_type_metadata(unit, limits)?;
    let variant_cases = verification_metadata::verify_variant_layout_metadata(unit, limits)?;
    verification_metadata::verify_variant_instruction_layouts(functions, &variant_cases)?;
    verification_metadata::verify_native_signatures(unit, functions, limits)?;
    verification_metadata::verify_source_map(unit, functions, total_instructions)?;
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

/// Prove lexical resource-scope balance for bytecode produced by the explicit
/// MIR resource path. Older v1 Artifacts have no `ResourceAcquire` marker and
/// retain their compatibility execution path; once a function opts in, every
/// normal exit and language short-circuit is checked against one exact LIFO
/// scope stack. Provider failure, host cancellation, and budget termination
/// use the VM's run-owned finalizer for this same tracked stack.
fn verify_resource_scope_lifetimes(
    function_id: usize,
    code: &[serde_json::Value],
) -> Result<(), BytecodeError> {
    if code.is_empty() {
        return Ok(());
    }
    let has_markers = code.iter().any(|instruction| {
        instruction
            .get("ResourceAcquire")
            .is_some_and(serde_json::Value::is_object)
    });
    if !has_markers {
        return Ok(());
    }

    let mut incoming = vec![None::<Vec<usize>>; code.len()];
    incoming[0] = Some(Vec::new());
    let mut work = VecDeque::from([0usize]);
    while let Some(ip) = work.pop_front() {
        let state = incoming[ip]
            .clone()
            .expect("queued instructions always have resource state");
        let (opcode, fields) = instruction_parts(function_id, ip, &code[ip])?;
        let mut after = state;
        match opcode {
            "ResourceAcquire" => after.push(required_index(fields, "resource")?),
            "ResourceDrop" => {
                let resource = required_index(fields, "resource")?;
                if after.pop() != Some(resource) {
                    return Err(invalid_payload(format!(
                        "function {function_id} instruction {ip} releases a resource outside lexical LIFO order"
                    )));
                }
            }
            "TryResult" => {
                let cleanup = fields["cleanup"]
                    .as_array()
                    .ok_or_else(|| invalid_payload("TryResult cleanup is not an array"))?
                    .iter()
                    .map(|value| json_usize(value, "cleanup"))
                    .collect::<Result<Vec<_>, _>>()?;
                let expected = after.iter().rev().copied().collect::<Vec<_>>();
                if cleanup != expected {
                    return Err(invalid_payload(format!(
                        "function {function_id} instruction {ip} TryResult cleanup does not cover its live resource scopes"
                    )));
                }
            }
            "Return" => {
                if !after.is_empty() {
                    return Err(invalid_payload(format!(
                        "function {function_id} instruction {ip} returns with live resource scopes"
                    )));
                }
                continue;
            }
            // An explicit RuntimeError has no language-level cleanup operand;
            // execution terminates through the run-owned VM finalizer.
            "RuntimeError" => continue,
            _ => {}
        }

        match opcode {
            "Jump" => enqueue_resource_scope_state(
                &mut incoming,
                &mut work,
                required_index(fields, "target")?,
                after,
                function_id,
            )?,
            "JumpIfBool" | "JumpIfIntCompare" => {
                enqueue_resource_scope_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "target")?,
                    after.clone(),
                    function_id,
                )?;
                enqueue_resource_scope_fallthrough(
                    &mut incoming,
                    &mut work,
                    ip,
                    after,
                    function_id,
                )?;
            }
            "MatchOption" => {
                enqueue_resource_scope_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "some_ip")?,
                    after.clone(),
                    function_id,
                )?;
                enqueue_resource_scope_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "none_ip")?,
                    after,
                    function_id,
                )?;
            }
            "MatchResult" => {
                enqueue_resource_scope_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "ok_ip")?,
                    after.clone(),
                    function_id,
                )?;
                enqueue_resource_scope_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "err_ip")?,
                    after,
                    function_id,
                )?;
            }
            "MatchVariant" => {
                enqueue_resource_scope_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "match_ip")?,
                    after.clone(),
                    function_id,
                )?;
                enqueue_resource_scope_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "else_ip")?,
                    after,
                    function_id,
                )?;
            }
            "MatchMapGet" | "MatchSortedMapGet" => {
                enqueue_resource_scope_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "some_ip")?,
                    after.clone(),
                    function_id,
                )?;
                enqueue_resource_scope_state(
                    &mut incoming,
                    &mut work,
                    required_index(fields, "none_ip")?,
                    after,
                    function_id,
                )?;
            }
            _ => enqueue_resource_scope_fallthrough(
                &mut incoming,
                &mut work,
                ip,
                after,
                function_id,
            )?,
        }
    }
    Ok(())
}

fn enqueue_resource_scope_fallthrough(
    incoming: &mut [Option<Vec<usize>>],
    work: &mut VecDeque<usize>,
    ip: usize,
    state: Vec<usize>,
    function_id: usize,
) -> Result<(), BytecodeError> {
    if ip + 1 < incoming.len() {
        enqueue_resource_scope_state(incoming, work, ip + 1, state, function_id)?;
    } else if !state.is_empty() {
        return Err(invalid_payload(format!(
            "function {function_id} falls off its body with live resource scopes"
        )));
    }
    Ok(())
}

fn enqueue_resource_scope_state(
    incoming: &mut [Option<Vec<usize>>],
    work: &mut VecDeque<usize>,
    target: usize,
    state: Vec<usize>,
    function_id: usize,
) -> Result<(), BytecodeError> {
    let Some(entry) = incoming.get_mut(target) else {
        return Err(invalid_payload(format!(
            "function {function_id} resource scope branch target {target} is outside its body"
        )));
    };
    match entry {
        None => {
            *entry = Some(state);
            work.push_back(target);
        }
        Some(existing) if *existing == state => {}
        Some(_) => {
            return Err(invalid_payload(format!(
                "function {function_id} merges incompatible lexical resource scopes"
            )));
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

#[derive(Clone, Copy)]
struct WireInstructionContext {
    function_id: usize,
    ip: usize,
    register_count: usize,
    code_len: usize,
    function_count: usize,
}

fn verify_wire_instruction(
    context: WireInstructionContext,
    instruction: &serde_json::Value,
    called_imports: &mut BTreeSet<String>,
) -> Result<(), BytecodeError> {
    let WireInstructionContext {
        function_id,
        ip,
        register_count,
        code_len,
        function_count,
    } = context;
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
        "CancelTask" => &["src"],
        "DeepCopy" | "DeepCopyElided" => &["reg"],
        "GetField" => &["dst", "base", "name"],
        "GetFieldSlot" => &["dst", "base", "slot"],
        "SetFieldSlot" => &["dst", "base", "slot", "value"],
        "SetField" => &["dst", "base", "name", "value"],
        "MakeStruct" | "MakeVariant" => &["dst", "layout", "fields"],
        "ResourceDrop" | "ResourceAcquire" => &["resource"],
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
        "JoinTasks" => &["handles"],
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

fn require_object_fields_with_optional(
    object: &serde_json::Map<String, serde_json::Value>,
    required: &[&str],
    optional: &[&str],
    context: &str,
) -> Result<(), BytecodeError> {
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let required = required.iter().copied().collect::<BTreeSet<_>>();
    let allowed = required
        .iter()
        .copied()
        .chain(optional.iter().copied())
        .collect::<BTreeSet<_>>();
    if !required.is_subset(&actual) || !actual.is_subset(&allowed) {
        return Err(invalid_payload(format!(
            "{context} fields differ: actual={actual:?}, required={required:?}, allowed={allowed:?}"
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
    "ResourceAcquire",
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
    "CancelTask",
    "JoinTasks",
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
    UnsupportedCoreLibraryAbi { artifact: u32, runtime: u32 },
    UnsupportedRuntimeAbi { artifact: u32, runtime: u32 },
    InvalidProvenance(&'static str),
    LimitExceeded(&'static str),
    ExecutableHashMismatch,
    ChecksumMismatch,
    ImportsNotCanonical,
    ImportAbiMismatch,
    ImportSignatureHashMismatch,
    TypedFactsBindingMismatch(&'static str),
    InvalidTypedExecutableFacts(String),
    InvalidPayload(String),
    MalformedSectionTable,
    SectionsNotCanonical,
    MissingSection(u8),
    UnknownRequiredSection(u8),
    KnownSectionNotRequired(u8),
    KnownSectionMustBeOptional(u8),
    InvalidSectionFlags { kind: u8, flags: u8 },
    SectionHashMismatch(u8),
    MalformedChecksum,
    TrailingBytes,
    Encode(serde_json::Error),
    Cbor(String),
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
    UnsupportedCoreLibraryAbi,
    UnsupportedRuntimeAbi,
    InvalidProvenance,
    LimitExceeded,
    ExecutableHashMismatch,
    ChecksumMismatch,
    ImportsNotCanonical,
    ImportAbiMismatch,
    ImportSignatureHashMismatch,
    TypedFactsBindingMismatch,
    InvalidTypedExecutableFacts,
    InvalidPayload,
    MalformedSectionTable,
    SectionsNotCanonical,
    MissingSection,
    UnknownRequiredSection,
    KnownSectionNotRequired,
    KnownSectionMustBeOptional,
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
            Self::UnsupportedCoreLibraryAbi { .. } => BytecodeErrorCode::UnsupportedCoreLibraryAbi,
            Self::UnsupportedRuntimeAbi { .. } => BytecodeErrorCode::UnsupportedRuntimeAbi,
            Self::InvalidProvenance(_) => BytecodeErrorCode::InvalidProvenance,
            Self::LimitExceeded(_) => BytecodeErrorCode::LimitExceeded,
            Self::ExecutableHashMismatch => BytecodeErrorCode::ExecutableHashMismatch,
            Self::ChecksumMismatch => BytecodeErrorCode::ChecksumMismatch,
            Self::ImportsNotCanonical => BytecodeErrorCode::ImportsNotCanonical,
            Self::ImportAbiMismatch => BytecodeErrorCode::ImportAbiMismatch,
            Self::ImportSignatureHashMismatch => BytecodeErrorCode::ImportSignatureHashMismatch,
            Self::TypedFactsBindingMismatch(_) => BytecodeErrorCode::TypedFactsBindingMismatch,
            Self::InvalidTypedExecutableFacts(_) => BytecodeErrorCode::InvalidTypedExecutableFacts,
            Self::InvalidPayload(_) => BytecodeErrorCode::InvalidPayload,
            Self::MalformedSectionTable => BytecodeErrorCode::MalformedSectionTable,
            Self::SectionsNotCanonical => BytecodeErrorCode::SectionsNotCanonical,
            Self::MissingSection(_) => BytecodeErrorCode::MissingSection,
            Self::UnknownRequiredSection(_) => BytecodeErrorCode::UnknownRequiredSection,
            Self::KnownSectionNotRequired(_) => BytecodeErrorCode::KnownSectionNotRequired,
            Self::KnownSectionMustBeOptional(_) => BytecodeErrorCode::KnownSectionMustBeOptional,
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
            Self::UnsupportedCoreLibraryAbi { artifact, runtime } => write!(
                formatter,
                "bytecode Core library ABI {artifact} is incompatible with runtime ABI {runtime}"
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
            Self::TypedFactsBindingMismatch(field) => {
                write!(formatter, "typed executable facts {field} mismatch")
            }
            Self::InvalidTypedExecutableFacts(message) => {
                write!(formatter, "invalid typed executable facts: {message}")
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
            Self::KnownSectionMustBeOptional(section) => {
                write!(formatter, "bytecode section {section} must be optional")
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
mod tests;
