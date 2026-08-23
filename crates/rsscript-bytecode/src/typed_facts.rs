//! Optional, verifier-owned static facts for the v1 executable payload.
//!
//! The facts are deliberately not part of the persisted instruction-set ABI.
//! They are an optional section bound to the executable digest. Engines may
//! consume them only through [`BoundTypedExecutableFactsV1`]. The wrapper
//! proves canonical structure, bounds, binding to one verified executable, and
//! every contract that v1 executable metadata can independently reconstruct:
//! static function signatures, Provider contracts, mutable argument positions,
//! declared layouts, and opcode-visible register types. It deliberately does
//! not claim that erased language facts (notably `read` versus `take`, closure
//! results, and ordinary generic substitutions) were reconstructed. Engines
//! must still intersect those facts with their executable-local proof before
//! optimization. An artifact without this section remains a valid v1 artifact
//! and falls back to the conservative executable verifier/runtime analysis path.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

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
    pub max_total_call_arguments: usize,
    pub max_total_type_nodes: usize,
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
            max_total_call_arguments: value.max_instructions,
            // The encoded byte limit is the primary allocation bound. This
            // independent structural budget prevents a compact adversarial
            // tree from monopolizing verifier CPU through repeated recursive
            // type walks.
            max_total_type_nodes: value.max_typed_facts_bytes / 2,
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
    /// Facts represented by the v1 executable are independently reconstructed
    /// during admission. Erased ownership/effect/type facts are not by
    /// themselves authorization for code generation: a backend must intersect
    /// them with its executable-local proof and decline on `Unknown` or
    /// disagreement.
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
        let unit = executable
            .as_object()
            .ok_or_else(|| invalid("executable unit is malformed"))?;
        let functions = executable
            .get("functions")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid("executable functions are unavailable"))?;
        if facts.functions.len() != functions.len()
            || facts.functions.len() > self.limits.max_functions
        {
            return Err(invalid("typed function count does not match executable"));
        }
        let signatures = executable_function_signatures(unit, functions)?;
        let layouts = verify_layout_contract(
            &facts.layouts,
            unit,
            self.limits.max_layouts,
            self.limits.max_layout_fields,
            context,
        )?;
        let mut type_work = TypeWorkBudget::new(self.limits.max_total_type_nodes, context);
        let mut total_call_arguments = 0usize;

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
            let expected_call_instructions = code
                .iter()
                .enumerate()
                .filter_map(|(instruction, value)| {
                    value
                        .as_object()
                        .and_then(|value| value.keys().next())
                        .filter(|opcode| opcode.starts_with("Call") || *opcode == "SpawnTask")
                        .map(|_| instruction)
                })
                .collect::<Vec<_>>();
            if function_facts.call_sites.len() != expected_call_instructions.len() {
                return Err(invalid(
                    "typed call sites do not completely cover executable calls",
                ));
            }
            let mut previous = None;
            for (call, expected_instruction) in function_facts
                .call_sites
                .iter()
                .zip(expected_call_instructions)
            {
                context.check()?;
                let instruction = call.instruction as usize;
                if instruction != expected_instruction {
                    return Err(invalid(
                        "typed call sites do not follow executable call order",
                    ));
                }
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
                total_call_arguments = total_call_arguments
                    .checked_add(call.parameters.len())
                    .ok_or(BytecodeError::LimitExceeded(
                        "typed facts total call arguments",
                    ))?;
                if total_call_arguments > self.limits.max_total_call_arguments {
                    return Err(BytecodeError::LimitExceeded(
                        "typed facts total call arguments",
                    ));
                }
                verify_external_call_contract(call, artifact)?;
                verify_executable_call_contract(
                    call,
                    &code[instruction],
                    &signatures,
                    &function_facts.registers,
                )?;
                for ty in call.parameters.iter().chain(std::iter::once(&call.result)) {
                    verify_fact_type(ty, self.limits.max_type_depth, &mut type_work)?;
                }
                for ty in &call.type_arguments {
                    verify_wire_type(ty, self.limits.max_type_depth, &mut type_work)?;
                }
            }
            for register in &function_facts.registers {
                context.check()?;
                verify_fact_type(&register.ty, self.limits.max_type_depth, &mut type_work)?;
            }
            for ty in &function_facts.generic_substitutions {
                verify_wire_type(ty, self.limits.max_type_depth, &mut type_work)?;
            }
            verify_executable_register_contract(
                ordinal,
                function,
                function_facts,
                &signatures,
                &layouts,
                context,
            )?;
        }

        for layout in &facts.layouts {
            context.check()?;
            for field in &layout.fields {
                verify_fact_type(&field.ty, self.limits.max_type_depth, &mut type_work)?;
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

#[derive(Debug, Clone)]
struct ExecutableFunctionSignature {
    parameters: Vec<WireType>,
    result: WireType,
}

#[derive(Debug, Clone)]
struct ExecutableLayout {
    kind: TypedLayoutKindV1,
    fields: Vec<(Option<String>, String, WireType)>,
}

fn executable_function_signatures(
    unit: &serde_json::Map<String, serde_json::Value>,
    functions: &[serde_json::Value],
) -> Result<Vec<ExecutableFunctionSignature>, BytecodeError> {
    let signatures = unit
        .get("native_signatures")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| invalid("executable native signatures are malformed"))?;
    functions
        .iter()
        .enumerate()
        .map(|(ordinal, function)| {
            let name = function
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("executable function name is malformed"))?;
            let signature = signatures
                .get(name)
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| invalid(format!("function {ordinal} has no native signature")))?;
            let parameters = signature
                .get("params")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| invalid("native signature parameters are malformed"))?
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .map(WireType::parse)
                        .ok_or_else(|| invalid("native signature parameter is malformed"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let result = match signature.get("return_type") {
                Some(serde_json::Value::String(value)) => WireType::parse(value),
                Some(serde_json::Value::Null) | None => WireType::Unit,
                _ => return Err(invalid("native signature result is malformed")),
            };
            Ok(ExecutableFunctionSignature { parameters, result })
        })
        .collect()
}

fn verify_layout_contract(
    facts: &[TypedLayoutV1],
    unit: &serde_json::Map<String, serde_json::Value>,
    max_layouts: usize,
    max_layout_fields: usize,
    context: VerificationContext<'_>,
) -> Result<BTreeMap<String, ExecutableLayout>, BytecodeError> {
    if facts.len() > max_layouts {
        return Err(BytecodeError::LimitExceeded("typed facts layouts"));
    }
    let records = unit
        .get("types")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| invalid("executable record layouts are malformed"))?;
    let variants = unit
        .get("variant_layouts")
        .and_then(serde_json::Value::as_object);
    let expected_count = records
        .len()
        .checked_add(variants.map_or(0, serde_json::Map::len))
        .ok_or(BytecodeError::LimitExceeded("typed facts layouts"))?;
    if facts.len() != expected_count {
        return Err(invalid(
            "typed layout count does not match executable layout metadata",
        ));
    }

    let mut expected = BTreeMap::new();
    for (name, record) in records {
        context.check()?;
        let fields = executable_layout_fields(record, None)?;
        expected.insert(
            name.clone(),
            ExecutableLayout {
                kind: TypedLayoutKindV1::Record,
                fields,
            },
        );
    }
    if let Some(variants) = variants {
        for (name, variant) in variants {
            context.check()?;
            let cases = variant
                .get("variants")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| invalid("executable variant cases are malformed"))?;
            let mut fields = Vec::new();
            for case in cases {
                let case_name = case
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| invalid("executable variant case name is malformed"))?;
                fields.extend(executable_layout_fields(case, Some(case_name))?);
            }
            if expected
                .insert(
                    name.clone(),
                    ExecutableLayout {
                        kind: TypedLayoutKindV1::Variant,
                        fields,
                    },
                )
                .is_some()
            {
                return Err(invalid("record and variant layout names overlap"));
            }
        }
    }

    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut total_fields = 0usize;
    let mut previous_layout_key: Option<(&str, u8)> = None;
    for (ordinal, layout) in facts.iter().enumerate() {
        context.check()?;
        let layout_key = (
            layout.name.as_str(),
            match layout.kind {
                TypedLayoutKindV1::Record => 0,
                TypedLayoutKindV1::Variant => 1,
            },
        );
        if layout.layout_id as usize != ordinal
            || layout.name.is_empty()
            || !ids.insert(layout.layout_id)
            || !names.insert(layout.name.as_str())
            || previous_layout_key.is_some_and(|previous| previous >= layout_key)
        {
            return Err(invalid("typed layouts are not canonical"));
        }
        previous_layout_key = Some(layout_key);
        total_fields = total_fields
            .checked_add(layout.fields.len())
            .ok_or(BytecodeError::LimitExceeded("typed facts layout fields"))?;
        if total_fields > max_layout_fields {
            return Err(BytecodeError::LimitExceeded("typed facts layout fields"));
        }
        let executable = expected
            .get(&layout.name)
            .ok_or_else(|| invalid("typed layout does not reference an executable layout"))?;
        if layout.kind != executable.kind || layout.fields.len() != executable.fields.len() {
            return Err(invalid(
                "typed layout shape differs from executable metadata",
            ));
        }
        let mut case_fields = BTreeSet::new();
        for (actual, (case, name, ty)) in layout.fields.iter().zip(&executable.fields) {
            if actual.name.is_empty()
                || actual.case.as_ref() != case.as_ref()
                || &actual.name != name
                || actual.ty != TypedFactTypeV1::Known(ty.clone())
                || !case_fields.insert((actual.case.as_deref(), actual.name.as_str()))
            {
                return Err(invalid(
                    "typed layout case, field name, or type differs from executable metadata",
                ));
            }
            match layout.kind {
                TypedLayoutKindV1::Record if actual.case.is_some() => {
                    return Err(invalid("record layout field names a variant case"));
                }
                TypedLayoutKindV1::Variant if actual.case.is_none() => {
                    return Err(invalid("variant layout field has no case"));
                }
                _ => {}
            }
        }
    }
    Ok(expected)
}

fn executable_layout_fields(
    value: &serde_json::Value,
    case: Option<&str>,
) -> Result<Vec<(Option<String>, String, WireType)>, BytecodeError> {
    value
        .get("fields")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("executable layout fields are malformed"))?
        .iter()
        .map(|field| {
            let name = field
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("executable layout field name is malformed"))?;
            let ty = field
                .get("type_name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("executable layout field type is malformed"))?;
            Ok((
                case.map(str::to_owned),
                name.to_owned(),
                WireType::parse(ty),
            ))
        })
        .collect()
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

fn verify_executable_call_contract(
    call: &TypedCallSiteV1,
    instruction: &serde_json::Value,
    signatures: &[ExecutableFunctionSignature],
    registers: &[TypedRegisterFactV1],
) -> Result<(), BytecodeError> {
    let fields = call_instruction_fields(instruction)?;
    let opcode = instruction
        .as_object()
        .and_then(|value| value.keys().next())
        .ok_or_else(|| invalid("call opcode is malformed"))?;
    let args = fields
        .get("args")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("call instruction args are malformed"))?;
    let mutable = fields
        .get("mut_args")
        .map(|value| {
            let values = value
                .as_array()
                .ok_or_else(|| invalid("call mutable arguments are malformed"))?;
            values
                .iter()
                .map(|value| {
                    value
                        .as_u64()
                        .and_then(|value| usize::try_from(value).ok())
                        .ok_or_else(|| invalid("call mutable argument index is malformed"))
                })
                .collect::<Result<BTreeSet<_>, _>>()
        })
        .transpose()?;
    if let Some(mutable) = mutable {
        for (index, effect) in call.parameter_effects.iter().enumerate() {
            if mutable.contains(&index) != (*effect == TypedDataEffectV1::Mutate) {
                return Err(invalid(
                    "typed call mutation effects differ from executable mut_args",
                ));
            }
        }
    }

    let proven_signature = match &call.target {
        TypedCallTargetV1::KnownFunction(target) => signatures.get(*target as usize),
        TypedCallTargetV1::Dynamic => dynamic_call_signature(fields, signatures)?,
        // Provider calls are checked against the canonical import signature by
        // `verify_external_call_contract`. Builtins and closures do not carry a
        // complete v1 executable signature, so their facts remain intersection
        // evidence rather than an independently reconstructed proof.
        TypedCallTargetV1::Provider(_)
        | TypedCallTargetV1::Builtin(_)
        | TypedCallTargetV1::Closure => None,
    };
    if let Some(signature) = proven_signature {
        let parameters = signature
            .parameters
            .iter()
            .cloned()
            .map(TypedFactTypeV1::Known)
            .collect::<Vec<_>>();
        if call.parameters != parameters
            || call.result != TypedFactTypeV1::Known(signature.result.clone())
        {
            return Err(invalid(
                "typed call parameter or result types differ from executable signature",
            ));
        }
    }

    for (argument, parameter) in args.iter().zip(&call.parameters) {
        let register = argument
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .and_then(|index| registers.get(index))
            .ok_or_else(|| invalid("call argument register is malformed"))?;
        if !fact_types_compatible(&register.ty, parameter) {
            return Err(invalid(
                "typed call parameter disagrees with its argument register",
            ));
        }
    }
    if opcode != "SpawnTask" {
        let destination = fields
            .get("dst")
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .and_then(|index| registers.get(index))
            .ok_or_else(|| invalid("call destination register is malformed"))?;
        if !fact_types_compatible(&destination.ty, &call.result) {
            return Err(invalid(
                "typed call result disagrees with its destination register",
            ));
        }
    }
    Ok(())
}

fn dynamic_call_signature<'a>(
    fields: &serde_json::Map<String, serde_json::Value>,
    signatures: &'a [ExecutableFunctionSignature],
) -> Result<Option<&'a ExecutableFunctionSignature>, BytecodeError> {
    let dispatch = fields
        .get("dispatch")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("dynamic call dispatch is malformed"))?;
    let mut proven = None;
    for entry in dispatch {
        let target = entry
            .as_array()
            .filter(|entry| entry.len() == 2)
            .and_then(|entry| entry[1].as_u64())
            .and_then(|value| usize::try_from(value).ok())
            .and_then(|index| signatures.get(index))
            .ok_or_else(|| invalid("dynamic call target signature is malformed"))?;
        match proven {
            None => proven = Some(target),
            Some(previous)
                if previous.parameters == target.parameters && previous.result == target.result => {
            }
            Some(_) => return Ok(None),
        }
    }
    Ok(proven)
}

fn fact_types_compatible(left: &TypedFactTypeV1, right: &TypedFactTypeV1) -> bool {
    match (left, right) {
        (TypedFactTypeV1::Unknown, _) | (_, TypedFactTypeV1::Unknown) => true,
        (TypedFactTypeV1::Known(left), TypedFactTypeV1::Known(right)) => {
            strip_qualifiers(left) == strip_qualifiers(right)
        }
    }
}

fn strip_qualifiers(mut ty: &WireType) -> &WireType {
    while let WireType::Qualified { value, .. } = ty {
        ty = value;
    }
    ty
}

fn verify_executable_register_contract(
    ordinal: usize,
    function: &serde_json::Value,
    facts: &TypedFunctionFactsV1,
    signatures: &[ExecutableFunctionSignature],
    layouts: &BTreeMap<String, ExecutableLayout>,
    context: VerificationContext<'_>,
) -> Result<(), BytecodeError> {
    let signature = signatures
        .get(ordinal)
        .ok_or_else(|| invalid("function signature ordinal is out of range"))?;
    let captures = function
        .get("captures")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| invalid("function capture count is malformed"))?;
    for (index, parameter) in signature.parameters.iter().enumerate() {
        require_register_fact(
            &facts.registers,
            captures + index,
            &TypedFactTypeV1::Known(parameter.clone()),
            "function parameter",
        )?;
    }
    let code = function
        .get("code")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| invalid("function code is malformed"))?;
    let reachable = reachable_instructions(code)?;
    for (ip, instruction) in code.iter().enumerate() {
        context.check()?;
        let instruction = instruction
            .as_object()
            .filter(|value| value.len() == 1)
            .ok_or_else(|| invalid("instruction is malformed"))?;
        let (opcode, value) = instruction
            .iter()
            .next()
            .ok_or_else(|| invalid("instruction opcode is missing"))?;
        let fields = value
            .as_object()
            .ok_or_else(|| invalid("instruction fields are malformed"))?;
        let expected = match opcode.as_str() {
            "LoadUnit" => Some(TypedFactTypeV1::Known(WireType::Unit)),
            "LoadInt" => Some(TypedFactTypeV1::Known(WireType::Int {
                bits: 64,
                signed: true,
            })),
            "LoadFloat" => Some(TypedFactTypeV1::Known(WireType::Float { bits: 64 })),
            "LoadBool" => Some(TypedFactTypeV1::Known(WireType::Bool)),
            "LoadString" | "StringConcat" | "StringBuilderFinish" => {
                Some(TypedFactTypeV1::Known(WireType::String))
            }
            "LoadChar" => Some(TypedFactTypeV1::Known(WireType::Char)),
            "AddInt"
            | "SubInt"
            | "MulInt"
            | "DivInt"
            | "ModInt"
            | "BitAndInt"
            | "BitOrInt"
            | "BitXorInt"
            | "ShiftLeftInt"
            | "ShiftRightInt"
            | "ListLen"
            | "NativeClosureId"
            | "NativeFieldClosureId" => Some(TypedFactTypeV1::Known(WireType::Int {
                bits: 64,
                signed: true,
            })),
            "LessInt" | "LessEqualInt" | "GreaterInt" | "GreaterEqualInt" | "Equal"
            | "NotEqual" => Some(TypedFactTypeV1::Known(WireType::Bool)),
            "Move" | "Manage" | "DeepCopyElided" => register_field_fact(
                fields,
                if opcode == "DeepCopyElided" {
                    "reg"
                } else {
                    "src"
                },
                &facts.registers,
            ),
            "MakeList" => {
                common_register_type(fields.get("items"), &facts.registers).map(|element| {
                    TypedFactTypeV1::Known(WireType::List {
                        element: Box::new(element),
                    })
                })
            }
            "GetField" | "GetFieldSlot" => {
                executable_field_result(fields, &facts.registers, layouts)?
            }
            _ => None,
        };
        if let (Some(expected), Some(destination)) =
            (expected, destination_register(opcode, fields))
        {
            require_register_fact(
                &facts.registers,
                destination,
                &expected,
                "instruction result",
            )?;
        }
        if opcode == "MakeStruct" {
            let name = fields
                .get("layout")
                .and_then(serde_json::Value::as_object)
                .and_then(|layout| layout.get("name"))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid("MakeStruct layout name is malformed"))?;
            let destination = destination_register(opcode, fields)
                .ok_or_else(|| invalid("MakeStruct destination is malformed"))?;
            require_named_register_fact(&facts.registers, destination, name)?;
        }
        if opcode == "Return" && reachable.contains(&ip) {
            let source = required_register(fields, "src")?;
            let expected = TypedFactTypeV1::Known(signature.result.clone());
            let actual = facts
                .registers
                .get(source)
                .ok_or_else(|| invalid("typed return register index is out of range"))?;
            if !fact_types_compatible(&actual.ty, &expected) {
                return Err(invalid(format!(
                    "typed function {ordinal} instruction {ip} return register {source} has {:?}, expected {:?}",
                    actual.ty, expected
                )));
            }
        }
        // A call result has already been matched to both its instruction and
        // destination in `verify_executable_call_contract`; this lookup also
        // ensures a malicious fact cannot bind the right call at the wrong IP.
        if opcode.starts_with("Call") || opcode == "SpawnTask" {
            let call = facts
                .call_sites
                .iter()
                .find(|call| call.instruction as usize == ip)
                .ok_or_else(|| invalid("call instruction has no typed call fact"))?;
            if opcode != "SpawnTask"
                && let Some(destination) = destination_register(opcode, fields)
            {
                require_register_fact(&facts.registers, destination, &call.result, "call result")?;
            }
        }
    }
    Ok(())
}

fn reachable_instructions(code: &[serde_json::Value]) -> Result<BTreeSet<usize>, BytecodeError> {
    let mut reachable = BTreeSet::new();
    let mut pending = VecDeque::from([0usize]);
    while let Some(ip) = pending.pop_front() {
        if ip >= code.len() || !reachable.insert(ip) {
            continue;
        }
        let instruction = code[ip]
            .as_object()
            .and_then(|value| value.iter().next())
            .ok_or_else(|| invalid("instruction is malformed during reachability"))?;
        let (opcode, fields) = instruction;
        let fields = fields
            .as_object()
            .ok_or_else(|| invalid("instruction fields are malformed during reachability"))?;
        let mut push_target = |name: &str| -> Result<(), BytecodeError> {
            let target = required_register(fields, name)?;
            pending.push_back(target);
            Ok(())
        };
        match opcode.as_str() {
            "Return" | "RuntimeError" => {}
            "Jump" => push_target("target")?,
            "JumpIfBool" | "JumpIfIntCompare" => {
                push_target("target")?;
                pending.push_back(ip + 1);
            }
            "MatchOption" => {
                push_target("some_ip")?;
                push_target("none_ip")?;
            }
            "MatchResult" => {
                push_target("ok_ip")?;
                push_target("err_ip")?;
            }
            "MatchVariant" => {
                push_target("match_ip")?;
                push_target("else_ip")?;
            }
            "MatchMapGet" | "MatchSortedMapGet" => {
                push_target("some_ip")?;
                push_target("none_ip")?;
            }
            _ => pending.push_back(ip + 1),
        }
    }
    Ok(reachable)
}

fn destination_register(
    opcode: &str,
    fields: &serde_json::Map<String, serde_json::Value>,
) -> Option<usize> {
    let field = if matches!(opcode, "DeepCopy" | "DeepCopyElided") {
        "reg"
    } else {
        "dst"
    };
    fields
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn required_register(
    fields: &serde_json::Map<String, serde_json::Value>,
    name: &str,
) -> Result<usize, BytecodeError> {
    fields
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| invalid(format!("instruction register `{name}` is malformed")))
}

fn register_field_fact(
    fields: &serde_json::Map<String, serde_json::Value>,
    name: &str,
    registers: &[TypedRegisterFactV1],
) -> Option<TypedFactTypeV1> {
    fields
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .and_then(|index| registers.get(index))
        .map(|fact| fact.ty.clone())
}

fn common_register_type(
    registers: Option<&serde_json::Value>,
    facts: &[TypedRegisterFactV1],
) -> Option<WireType> {
    let registers = registers?.as_array()?;
    let mut common = None;
    for register in registers {
        let ty = register
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .and_then(|index| facts.get(index))
            .and_then(|fact| match &fact.ty {
                TypedFactTypeV1::Known(ty) => Some(strip_qualifiers(ty).clone()),
                TypedFactTypeV1::Unknown => None,
            })?;
        if common.as_ref().is_some_and(|common| common != &ty) {
            return None;
        }
        common = Some(ty);
    }
    common
}

fn executable_field_result(
    fields: &serde_json::Map<String, serde_json::Value>,
    facts: &[TypedRegisterFactV1],
    layouts: &BTreeMap<String, ExecutableLayout>,
) -> Result<Option<TypedFactTypeV1>, BytecodeError> {
    let base = required_register(fields, "base")?;
    let Some(TypedRegisterFactV1 {
        ty: TypedFactTypeV1::Known(ty),
        ..
    }) = facts.get(base)
    else {
        return Ok(None);
    };
    let WireType::Named { name, .. } = strip_qualifiers(ty) else {
        return Ok(None);
    };
    let Some(layout) = layouts.get(name) else {
        return Ok(None);
    };
    let field = if let Some(slot) = fields
        .get("slot")
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
    {
        layout.fields.get(slot)
    } else if let Some(name) = fields.get("name").and_then(serde_json::Value::as_str) {
        layout.fields.iter().find(|(_, field, _)| field == name)
    } else {
        None
    };
    Ok(field.map(|(_, _, ty)| TypedFactTypeV1::Known(ty.clone())))
}

fn require_named_register_fact(
    registers: &[TypedRegisterFactV1],
    register: usize,
    expected_name: &str,
) -> Result<(), BytecodeError> {
    let Some(fact) = registers.get(register) else {
        return Err(invalid("typed register index is out of range"));
    };
    match &fact.ty {
        TypedFactTypeV1::Unknown => Ok(()),
        TypedFactTypeV1::Known(ty) if matches!(strip_qualifiers(ty), WireType::Named { name, .. } if name == expected_name) => {
            Ok(())
        }
        TypedFactTypeV1::Known(_) => Err(invalid(
            "typed aggregate register differs from executable layout",
        )),
    }
}

fn require_register_fact(
    registers: &[TypedRegisterFactV1],
    register: usize,
    expected: &TypedFactTypeV1,
    source: &str,
) -> Result<(), BytecodeError> {
    let actual = registers
        .get(register)
        .ok_or_else(|| invalid("typed register index is out of range"))?;
    if !fact_types_compatible(&actual.ty, expected) {
        return Err(invalid(format!(
            "typed register disagrees with independently derived {source} type"
        )));
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

struct TypeWorkBudget<'a> {
    remaining: usize,
    context: VerificationContext<'a>,
}

impl<'a> TypeWorkBudget<'a> {
    fn new(remaining: usize, context: VerificationContext<'a>) -> Self {
        Self { remaining, context }
    }

    fn consume(&mut self) -> Result<(), BytecodeError> {
        self.context.check()?;
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or(BytecodeError::LimitExceeded("typed facts type nodes"))?;
        Ok(())
    }
}

fn verify_fact_type(
    ty: &TypedFactTypeV1,
    max_depth: usize,
    work: &mut TypeWorkBudget<'_>,
) -> Result<(), BytecodeError> {
    if let TypedFactTypeV1::Known(ty) = ty {
        verify_wire_type(ty, max_depth, work)?;
    }
    Ok(())
}

fn verify_wire_type(
    ty: &WireType,
    depth: usize,
    work: &mut TypeWorkBudget<'_>,
) -> Result<(), BytecodeError> {
    if depth == 0 {
        return Err(BytecodeError::LimitExceeded("typed facts type depth"));
    }
    work.consume()?;
    use WireType::{List, Map, Named, Option, Qualified, Result, Tuple};
    match ty {
        List { element } | Option { value: element } | Qualified { value: element, .. } => {
            verify_wire_type(element, depth - 1, work)?
        }
        Map { key, value }
        | Result {
            ok: key,
            error: value,
        } => {
            verify_wire_type(key, depth - 1, work)?;
            verify_wire_type(value, depth - 1, work)?;
        }
        Tuple { elements } => {
            for element in elements {
                verify_wire_type(element, depth - 1, work)?;
            }
        }
        Named {
            package,
            name,
            arguments,
        } => {
            if name.is_empty() || package.as_ref().is_some_and(String::is_empty) {
                return Err(invalid("typed facts contains an empty named type identity"));
            }
            for argument in arguments {
                verify_wire_type(argument, depth - 1, work)?;
            }
        }
        WireType::Resource { name } | WireType::Handle { name } if name.is_empty() => {
            return Err(invalid("typed facts contains an empty resource identity"));
        }
        WireType::Int { bits, signed } if *bits != 64 || !*signed => {
            return Err(invalid("v1 executable facts require signed 64-bit Int"));
        }
        WireType::Float { bits } if *bits != 64 => {
            return Err(invalid("v1 executable facts require 64-bit Float"));
        }
        _ => {}
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> BytecodeError {
    BytecodeError::InvalidTypedExecutableFacts(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_operation::CancellationToken;

    #[test]
    fn nested_type_walk_has_an_independent_work_budget() {
        let ty = WireType::List {
            element: Box::new(WireType::Option {
                value: Box::new(WireType::String),
            }),
        };
        let mut work = TypeWorkBudget::new(2, VerificationContext::default());
        assert!(matches!(
            verify_wire_type(&ty, 64, &mut work),
            Err(BytecodeError::LimitExceeded("typed facts type nodes"))
        ));
    }

    #[test]
    fn nested_type_walk_observes_cancellation_at_each_node() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut work = TypeWorkBudget::new(
            64,
            VerificationContext {
                cancellation: Some(&cancellation),
                deadline: None,
            },
        );
        assert!(matches!(
            verify_wire_type(&WireType::String, 64, &mut work),
            Err(BytecodeError::Cancelled)
        ));
    }

    #[test]
    fn v1_storage_types_reject_noncanonical_scalar_widths() {
        for ty in [
            WireType::Int {
                bits: 32,
                signed: true,
            },
            WireType::Int {
                bits: 64,
                signed: false,
            },
            WireType::Float { bits: 32 },
        ] {
            let mut work = TypeWorkBudget::new(64, VerificationContext::default());
            assert!(matches!(
                verify_wire_type(&ty, 64, &mut work),
                Err(BytecodeError::InvalidTypedExecutableFacts(_))
            ));
        }
    }
}
