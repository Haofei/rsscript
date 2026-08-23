//! Optional, verifier-owned static facts for the v1 executable payload.
//!
//! The facts are deliberately not part of the persisted instruction-set ABI.
//! They are an optional section bound to the executable digest. Engines may
//! consume them only through [`BoundTypedExecutableFactsV1`]. The wrapper
//! proves canonical structure, bounds, and binding to one verified executable;
//! it deliberately does not claim that every language type/effect fact was
//! independently re-derived from register bytecode. Engines must intersect
//! these facts with their executable-local proof before optimization. An artifact
//! without this section remains a valid v1 artifact and falls back to the
//! conservative executable verifier/runtime analysis path.

use std::collections::BTreeSet;

use rsscript_abi_model::WireType;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    BytecodeArtifact, BytecodeError, BytecodeLimits, VerificationContext,
    decode_executable_payload, encode_executable_payload,
};

pub const TYPED_EXECUTABLE_FACTS_SCHEMA_V1: &str = "rsscript.typed_executable_facts.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedExecutableFactsV1 {
    pub schema: String,
    /// SHA-256 digest of the canonical executable payload, including the
    /// `sha256:` domain prefix used by the Artifact header.
    pub executable_hash: String,
    pub bytecode_isa_version: u32,
    pub runtime_abi_version: u32,
    pub interface_catalog_digest: String,
    pub imports_hash: String,
    pub functions: Vec<TypedFunctionFactsV1>,
    pub layouts: Vec<TypedLayoutV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedFunctionFactsV1 {
    pub function_ordinal: u32,
    /// One entry for every register in the v1 function register window.
    pub registers: Vec<TypedRegisterFactV1>,
    pub call_sites: Vec<TypedCallSiteV1>,
    /// Reserved for substitutions proven by lowering. The current v1 MIR does
    /// not retain instantiation arguments at ordinary function calls, so the
    /// emitter leaves this empty rather than recovering them from spellings.
    pub generic_substitutions: Vec<WireType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedRegisterFactV1 {
    pub ty: TypedFactTypeV1,
    pub ownership: TypedValueOwnershipV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedFactTypeV1 {
    Unknown,
    Known(WireType),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedValueOwnershipV1 {
    Unknown,
    Copy,
    ReadBorrow,
    UniqueBorrow,
    Owned,
    Shared,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedCallSiteV1 {
    pub instruction: u32,
    pub target: TypedCallTargetV1,
    pub parameters: Vec<TypedFactTypeV1>,
    pub result: TypedFactTypeV1,
    pub parameter_effects: Vec<TypedDataEffectV1>,
    /// Concrete generic arguments proven by lowering. Empty means unavailable,
    /// not an inferred empty generic parameter list.
    pub type_arguments: Vec<WireType>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedCallTargetV1 {
    KnownFunction(u32),
    Closure,
    Provider(u32),
    Builtin(String),
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedDataEffectV1 {
    Read,
    Mutate,
    Take,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedLayoutV1 {
    pub layout_id: u32,
    pub name: String,
    pub kind: TypedLayoutKindV1,
    pub fields: Vec<TypedLayoutFieldV1>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypedLayoutKindV1 {
    Record,
    Variant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypedLayoutFieldV1 {
    pub case: Option<String>,
    pub name: String,
    pub ty: TypedFactTypeV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypedFactsLimits {
    pub max_bytes: usize,
    pub max_functions: usize,
    pub max_registers_per_function: usize,
    pub max_call_sites_per_function: usize,
    pub max_layouts: usize,
    pub max_layout_fields: usize,
    pub max_type_depth: usize,
}

impl From<BytecodeLimits> for TypedFactsLimits {
    fn from(value: BytecodeLimits) -> Self {
        Self {
            max_bytes: value.max_typed_facts_bytes,
            max_functions: value.max_functions,
            max_registers_per_function: value.max_registers_per_function,
            max_call_sites_per_function: value.max_instructions,
            max_layouts: value.max_functions,
            max_layout_fields: value.max_instructions,
            max_type_depth: 64,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BoundTypedExecutableFactsV1 {
    facts: TypedExecutableFactsV1,
}

impl BoundTypedExecutableFactsV1 {
    /// Return canonical, executable-bound optimization evidence.
    ///
    /// The contained language types, ownership, and effects are not by
    /// themselves authorization for code generation. A backend must intersect
    /// them with facts independently proved from the executable and decline on
    /// `Unknown` or disagreement.
    pub fn facts(&self) -> &TypedExecutableFactsV1 {
        &self.facts
    }

    pub fn into_facts(self) -> TypedExecutableFactsV1 {
        self.facts
    }
}

pub fn encode_typed_executable_facts(
    facts: &TypedExecutableFactsV1,
) -> Result<Vec<u8>, BytecodeError> {
    encode_executable_payload(facts)
}

pub struct TypedExecutableFactsVerifierV1 {
    limits: TypedFactsLimits,
}

impl TypedExecutableFactsVerifierV1 {
    pub fn new(limits: TypedFactsLimits) -> Self {
        Self { limits }
    }

    pub fn verify(
        &self,
        bytes: &[u8],
        artifact: &BytecodeArtifact,
    ) -> Result<BoundTypedExecutableFactsV1, BytecodeError> {
        self.verify_with_context(bytes, artifact, VerificationContext::default())
    }

    pub fn verify_with_context(
        &self,
        bytes: &[u8],
        artifact: &BytecodeArtifact,
        context: VerificationContext<'_>,
    ) -> Result<BoundTypedExecutableFactsV1, BytecodeError> {
        context.check()?;
        if bytes.len() > self.limits.max_bytes {
            return Err(BytecodeError::LimitExceeded("typed facts bytes"));
        }
        let facts: TypedExecutableFactsV1 =
            decode_executable_payload(bytes).map_err(|error| invalid(error.to_string()))?;
        if encode_typed_executable_facts(&facts)? != bytes {
            return Err(invalid("typed facts CBOR is not canonical"));
        }
        if facts.schema != TYPED_EXECUTABLE_FACTS_SCHEMA_V1 {
            return Err(invalid("unsupported typed facts schema"));
        }
        let executable_hash = format!("sha256:{:x}", Sha256::digest(&artifact.payload));
        if facts.executable_hash != executable_hash {
            return Err(BytecodeError::TypedFactsBindingMismatch("executable hash"));
        }
        if facts.bytecode_isa_version != artifact.header.bytecode_isa_version {
            return Err(BytecodeError::TypedFactsBindingMismatch("bytecode ISA"));
        }
        if facts.runtime_abi_version != artifact.header.runtime_abi_version {
            return Err(BytecodeError::TypedFactsBindingMismatch("runtime ABI"));
        }
        if facts.interface_catalog_digest != artifact.header.interface_catalog_digest {
            return Err(BytecodeError::TypedFactsBindingMismatch(
                "interface catalog digest",
            ));
        }
        if facts.imports_hash != typed_facts_imports_hash(artifact)? {
            return Err(BytecodeError::TypedFactsBindingMismatch("imports hash"));
        }
        self.verify_structure(&facts, artifact, context)?;
        Ok(BoundTypedExecutableFactsV1 { facts })
    }

    fn verify_structure(
        &self,
        facts: &TypedExecutableFactsV1,
        artifact: &BytecodeArtifact,
        context: VerificationContext<'_>,
    ) -> Result<(), BytecodeError> {
        let executable: serde_json::Value = decode_executable_payload(&artifact.payload)
            .map_err(|error| invalid(format!("cannot inspect executable: {error}")))?;
        let functions = executable
            .get("functions")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid("executable functions are unavailable"))?;
        if facts.functions.len() != functions.len()
            || facts.functions.len() > self.limits.max_functions
        {
            return Err(invalid("typed function count does not match executable"));
        }

        for (ordinal, (function_facts, function)) in
            facts.functions.iter().zip(functions).enumerate()
        {
            context.check()?;
            if function_facts.function_ordinal as usize != ordinal {
                return Err(invalid(
                    "typed functions are not in canonical ordinal order",
                ));
            }
            let regs = function
                .get("regs")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .ok_or_else(|| invalid("executable register count is malformed"))?;
            if function_facts.registers.len() != regs
                || regs > self.limits.max_registers_per_function
            {
                return Err(invalid("typed register count does not match executable"));
            }
            if !function_facts.generic_substitutions.is_empty() {
                return Err(invalid(
                    "v1 executable does not prove function generic substitutions",
                ));
            }
            let code = function
                .get("code")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| invalid("executable code is malformed"))?;
            if function_facts.call_sites.len() > self.limits.max_call_sites_per_function {
                return Err(BytecodeError::LimitExceeded("typed facts call sites"));
            }
            let mut previous = None;
            for call in &function_facts.call_sites {
                let instruction = call.instruction as usize;
                if previous.is_some_and(|value| instruction <= value) {
                    return Err(invalid("call sites are not in canonical instruction order"));
                }
                previous = Some(instruction);
                let opcode = code
                    .get(instruction)
                    .and_then(serde_json::Value::as_object)
                    .and_then(|value| value.keys().next())
                    .ok_or_else(|| invalid("call site instruction is malformed"))?;
                if !opcode.starts_with("Call") && opcode != "SpawnTask" {
                    return Err(invalid("typed call site does not name a call instruction"));
                }
                verify_call_target(
                    &call.target,
                    opcode,
                    &code[instruction],
                    artifact,
                    functions.len(),
                )?;
                verify_call_type_arguments(call, opcode, &code[instruction])?;
                if call.parameters.len() != call.parameter_effects.len() {
                    return Err(invalid("call parameter and effect counts differ"));
                }
                let executable_arg_count = call_instruction_fields(&code[instruction])?
                    .get("args")
                    .and_then(serde_json::Value::as_array)
                    .ok_or_else(|| invalid("call instruction args are malformed"))?
                    .len();
                if call.parameters.len() != executable_arg_count {
                    return Err(invalid(
                        "typed call parameter count does not match executable",
                    ));
                }
                verify_external_call_contract(call, artifact)?;
                for ty in call.parameters.iter().chain(std::iter::once(&call.result)) {
                    verify_fact_type(ty, self.limits.max_type_depth)?;
                }
                for ty in &call.type_arguments {
                    verify_wire_type(ty, self.limits.max_type_depth)?;
                }
            }
            for register in &function_facts.registers {
                verify_fact_type(&register.ty, self.limits.max_type_depth)?;
            }
            for ty in &function_facts.generic_substitutions {
                verify_wire_type(ty, self.limits.max_type_depth)?;
            }
        }

        if facts.layouts.len() > self.limits.max_layouts {
            return Err(BytecodeError::LimitExceeded("typed facts layouts"));
        }
        let mut ids = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut field_count = 0usize;
        for (ordinal, layout) in facts.layouts.iter().enumerate() {
            if layout.layout_id as usize != ordinal
                || layout.name.is_empty()
                || !ids.insert(layout.layout_id)
                || !names.insert(layout.name.as_str())
            {
                return Err(invalid("typed layouts are not canonical"));
            }
            field_count = field_count
                .checked_add(layout.fields.len())
                .ok_or(BytecodeError::LimitExceeded("typed facts layout fields"))?;
            if field_count > self.limits.max_layout_fields {
                return Err(BytecodeError::LimitExceeded("typed facts layout fields"));
            }
            for field in &layout.fields {
                if field.name.is_empty() {
                    return Err(invalid("typed layout field name is empty"));
                }
                verify_fact_type(&field.ty, self.limits.max_type_depth)?;
            }
        }
        Ok(())
    }
}

fn call_instruction_fields(
    instruction: &serde_json::Value,
) -> Result<&serde_json::Map<String, serde_json::Value>, BytecodeError> {
    instruction
        .as_object()
        .and_then(|instruction| instruction.values().next())
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| invalid("call instruction fields are malformed"))
}

fn verify_external_call_contract(
    call: &TypedCallSiteV1,
    artifact: &BytecodeArtifact,
) -> Result<(), BytecodeError> {
    let TypedCallTargetV1::Provider(ordinal) = &call.target else {
        return Ok(());
    };
    let import = artifact
        .imports
        .get(*ordinal as usize)
        .ok_or_else(|| invalid("provider call target is out of range"))?;
    if call.parameters.len() != import.signature.parameters.len() {
        return Err(invalid(
            "typed provider parameter count does not match import signature",
        ));
    }
    for ((actual_type, actual_effect), expected) in call
        .parameters
        .iter()
        .zip(&call.parameter_effects)
        .zip(&import.signature.parameters)
    {
        if actual_type != &TypedFactTypeV1::Known(expected.ty.clone())
            || *actual_effect != typed_effect(expected.effect)
        {
            return Err(invalid(
                "typed provider parameter contract does not match import signature",
            ));
        }
    }
    if call.result != TypedFactTypeV1::Known(import.signature.result.clone()) {
        return Err(invalid(
            "typed provider result does not match import signature",
        ));
    }
    Ok(())
}

fn typed_effect(effect: rsscript_abi_model::DataEffect) -> TypedDataEffectV1 {
    match effect {
        rsscript_abi_model::DataEffect::Read => TypedDataEffectV1::Read,
        rsscript_abi_model::DataEffect::Mut => TypedDataEffectV1::Mutate,
        rsscript_abi_model::DataEffect::Take => TypedDataEffectV1::Take,
    }
}

pub fn typed_facts_imports_hash(artifact: &BytecodeArtifact) -> Result<String, BytecodeError> {
    let imports = encode_executable_payload(&artifact.imports)?;
    Ok(format!("sha256:{:x}", Sha256::digest(imports)))
}

fn verify_call_target(
    target: &TypedCallTargetV1,
    opcode: &str,
    instruction: &serde_json::Value,
    artifact: &BytecodeArtifact,
    function_count: usize,
) -> Result<(), BytecodeError> {
    let fields = instruction
        .as_object()
        .and_then(|instruction| instruction.values().next())
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| invalid("call instruction fields are malformed"))?;
    match target {
        TypedCallTargetV1::KnownFunction(ordinal) => {
            if !matches!(opcode, "CallKnown" | "SpawnTask")
                || *ordinal as usize >= function_count
                || fields.get("function").and_then(serde_json::Value::as_u64)
                    != Some(u64::from(*ordinal))
            {
                return Err(invalid("known call target does not match executable"));
            }
        }
        TypedCallTargetV1::Closure if opcode == "CallClosure" => {}
        TypedCallTargetV1::Dynamic if opcode == "CallDynamic" => {}
        TypedCallTargetV1::Builtin(name)
            if matches!(opcode, "CallIntrinsic" | "CallTypedIntrinsic")
                && fields.get("intrinsic").and_then(serde_json::Value::as_str)
                    == Some(name.as_str()) => {}
        TypedCallTargetV1::Provider(ordinal) => {
            let import = artifact
                .imports
                .get(*ordinal as usize)
                .ok_or_else(|| invalid("provider call target is out of range"))?;
            if opcode != "CallExternal"
                || fields.get("key").and_then(serde_json::Value::as_str)
                    != Some(import.symbol.as_str())
            {
                return Err(invalid("provider call target does not match executable"));
            }
        }
        _ => return Err(invalid("typed call target kind does not match executable")),
    }
    Ok(())
}

fn verify_call_type_arguments(
    call: &TypedCallSiteV1,
    opcode: &str,
    instruction: &serde_json::Value,
) -> Result<(), BytecodeError> {
    if opcode != "CallTypedIntrinsic" {
        if !call.type_arguments.is_empty() {
            return Err(invalid(
                "v1 call instruction does not prove generic substitutions",
            ));
        }
        return Ok(());
    }
    let fields = instruction
        .as_object()
        .and_then(|instruction| instruction.values().next())
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| invalid("typed intrinsic fields are malformed"))?;
    let expected = fields
        .get("type_arg")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| invalid("typed intrinsic type argument is missing"))?;
    if call.type_arguments.len() != 1
        || wire_runtime_type_name(&call.type_arguments[0]).as_deref() != Some(expected)
    {
        return Err(invalid(
            "typed intrinsic argument does not match executable",
        ));
    }
    Ok(())
}

fn wire_runtime_type_name(ty: &WireType) -> Option<String> {
    match ty {
        WireType::Unit => Some("Unit".to_owned()),
        WireType::Bool => Some("Bool".to_owned()),
        WireType::Int { .. } => Some("Int".to_owned()),
        WireType::Float { .. } => Some("Float".to_owned()),
        WireType::String => Some("String".to_owned()),
        WireType::Char => Some("Char".to_owned()),
        WireType::Bytes => Some("Bytes".to_owned()),
        WireType::Named { name, .. } | WireType::Resource { name } | WireType::Handle { name } => {
            Some(name.clone())
        }
        WireType::Qualified { value, .. } => wire_runtime_type_name(value),
        _ => None,
    }
}

fn verify_fact_type(ty: &TypedFactTypeV1, max_depth: usize) -> Result<(), BytecodeError> {
    if let TypedFactTypeV1::Known(ty) = ty {
        verify_wire_type(ty, max_depth)?;
    }
    Ok(())
}

fn verify_wire_type(ty: &WireType, depth: usize) -> Result<(), BytecodeError> {
    if depth == 0 {
        return Err(BytecodeError::LimitExceeded("typed facts type depth"));
    }
    use WireType::{List, Map, Named, Option, Qualified, Result, Tuple};
    match ty {
        List { element } | Option { value: element } | Qualified { value: element, .. } => {
            verify_wire_type(element, depth - 1)?
        }
        Map { key, value }
        | Result {
            ok: key,
            error: value,
        } => {
            verify_wire_type(key, depth - 1)?;
            verify_wire_type(value, depth - 1)?;
        }
        Tuple { elements } => {
            for element in elements {
                verify_wire_type(element, depth - 1)?;
            }
        }
        Named { arguments, .. } => {
            for argument in arguments {
                verify_wire_type(argument, depth - 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> BytecodeError {
    BytecodeError::InvalidTypedExecutableFacts(message.into())
}
