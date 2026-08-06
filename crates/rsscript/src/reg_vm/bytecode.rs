use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use rsscript_abi_model::{ExternalImport, RUNTIME_ABI_VERSION};
use rsscript_bytecode::{BytecodeArtifact, BytecodeError, BytecodeVerifier};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::*;

const MAX_FUNCTIONS: usize = 65_536;
const MAX_REGISTERS_PER_FUNCTION: usize = 1_048_576;
const MAX_INSTRUCTIONS: usize = 10_000_000;

#[derive(Serialize, Deserialize)]
struct WireUnit {
    functions: Vec<WireFunction>,
    function_ids: BTreeMap<String, usize>,
    resource_drop_functions: BTreeMap<String, usize>,
    types: BTreeMap<String, RegTypeInfo>,
    native_signatures: BTreeMap<String, RegNativeSignature>,
    closure_identity_observable: bool,
}

#[derive(Serialize, Deserialize)]
struct WireFunction {
    name: String,
    params: usize,
    captures: usize,
    regs: usize,
    local_regs: BTreeMap<String, Reg>,
    code: Vec<RegInstr>,
}

pub(super) struct VerifiedRegBytecode {
    artifact: BytecodeArtifact,
    executable: RegUnit,
}

impl VerifiedRegBytecode {
    pub(super) fn into_parts(self) -> (BytecodeArtifact, RegUnit) {
        (self.artifact, self.executable)
    }
}

impl From<&RegUnit> for WireUnit {
    fn from(unit: &RegUnit) -> Self {
        Self {
            functions: unit
                .functions
                .iter()
                .map(|function| WireFunction {
                    name: function.name.clone(),
                    params: function.params,
                    captures: function.captures,
                    regs: function.regs,
                    local_regs: function
                        .local_regs
                        .iter()
                        .map(|(name, reg)| (name.clone(), *reg))
                        .collect(),
                    code: function.code.clone(),
                })
                .collect(),
            function_ids: unit
                .function_ids
                .iter()
                .map(|(name, id)| (name.clone(), *id))
                .collect(),
            resource_drop_functions: unit
                .resource_drop_functions
                .iter()
                .map(|(name, id)| (name.clone(), *id))
                .collect(),
            types: unit
                .types
                .iter()
                .map(|(name, info)| (name.clone(), info.clone()))
                .collect(),
            native_signatures: unit
                .native_signatures
                .iter()
                .map(|(name, signature)| (name.clone(), signature.clone()))
                .collect(),
            closure_identity_observable: unit.closure_identity_observable,
        }
    }
}

impl WireUnit {
    fn into_reg_unit(self) -> RegUnit {
        let functions = self
            .functions
            .into_iter()
            .map(|function| RegFunction {
                name: function.name,
                params: function.params,
                captures: function.captures,
                regs: function.regs,
                local_regs: function.local_regs.into_iter().collect(),
                code: function.code,
                jit_analysis: std::cell::Cell::new(None),
                jit_self_recursion_kind: std::cell::Cell::new(None),
                native_status: std::cell::Cell::new(0),
                call_count: std::cell::Cell::new(0),
                branch_count: std::cell::Cell::new(0),
                profile: std::cell::RefCell::new(None),
                osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
            })
            .collect::<Vec<_>>();
        let eligibility = compute_jit_eligibility(&functions);
        for (function, &eligible) in functions.iter().zip(&eligibility) {
            function
                .jit_analysis
                .set(Some((eligible, jit_function_has_loop(&function.code))));
        }
        RegUnit {
            functions: functions.into_iter().map(Rc::new).collect(),
            function_ids: self.function_ids.into_iter().collect(),
            resource_drop_functions: self.resource_drop_functions.into_iter().collect(),
            types: self.types.into_iter().collect(),
            native_signatures: self.native_signatures.into_iter().collect(),
            closure_identity_observable: self.closure_identity_observable,
        }
    }
}

pub(super) fn encode_and_verify(
    unit: &RegUnit,
    validated: &ValidatedProgram,
    executable: &rsscript_lowering::ExecutableIr,
) -> Result<VerifiedRegBytecode, EvalError> {
    let payload = serde_json::to_vec(&WireUnit::from(unit))
        .map_err(|error| EvalError::Runtime(format!("cannot encode VM bytecode: {error}")))?;
    let imports = external_imports(unit, executable);
    let artifact = BytecodeArtifact::new(
        env!("CARGO_PKG_VERSION"),
        RUNTIME_ABI_VERSION,
        source_hash(validated),
        imports,
        payload,
    )
    .map_err(bytecode_error)?;
    verify_bytes(&artifact.to_bytes().map_err(bytecode_error)?)
}

pub(super) fn verify_bytes(bytes: &[u8]) -> Result<VerifiedRegBytecode, EvalError> {
    let artifact = BytecodeVerifier::default()
        .verify(bytes)
        .map_err(bytecode_error)?
        .into_artifact();
    let executable = verify_payload(&artifact.payload, &artifact.imports)
        .map_err(|message| bytecode_error(BytecodeError::InvalidPayload(message)))?;
    Ok(VerifiedRegBytecode {
        artifact,
        executable,
    })
}

fn verify_payload(payload: &[u8], imports: &[ExternalImport]) -> Result<RegUnit, String> {
    let wire: WireUnit = serde_json::from_slice(payload)
        .map_err(|error| format!("cannot decode VM instruction stream: {error}"))?;
    verify_wire_unit(&wire, imports)?;
    Ok(wire.into_reg_unit())
}

fn verify_wire_unit(unit: &WireUnit, imports: &[ExternalImport]) -> Result<(), String> {
    if unit.functions.len() > MAX_FUNCTIONS {
        return Err("function count exceeds verifier limit".to_string());
    }
    let function_count = unit.functions.len();
    let mut instruction_count = 0usize;
    for (id, function) in unit.functions.iter().enumerate() {
        if function.params > function.regs || function.captures > function.regs {
            return Err(format!(
                "function {id} has more parameters/captures than registers"
            ));
        }
        if function.regs > MAX_REGISTERS_PER_FUNCTION {
            return Err(format!(
                "function {id} register count exceeds verifier limit"
            ));
        }
        instruction_count = instruction_count
            .checked_add(function.code.len())
            .ok_or_else(|| "instruction count overflow".to_string())?;
        if instruction_count > MAX_INSTRUCTIONS {
            return Err("instruction count exceeds verifier limit".to_string());
        }
        for (name, &reg) in &function.local_regs {
            verify_reg(id, function.regs, reg, name)?;
        }
        for (ip, instruction) in function.code.iter().enumerate() {
            verify_instruction(id, ip, function, function_count, instruction)?;
        }
    }
    for (name, &id) in &unit.function_ids {
        let function = unit
            .functions
            .get(id)
            .ok_or_else(|| format!("function map `{name}` references missing function {id}"))?;
        if function.name != *name {
            return Err(format!(
                "function map `{name}` does not match function metadata"
            ));
        }
    }
    for (name, &id) in &unit.resource_drop_functions {
        if id >= function_count {
            return Err(format!(
                "resource `{name}` references missing drop function {id}"
            ));
        }
    }
    let declared = imports
        .iter()
        .map(|import| import.symbol.as_str())
        .collect::<BTreeSet<_>>();
    let called = unit
        .functions
        .iter()
        .flat_map(|function| &function.code)
        .filter_map(|instruction| match instruction {
            RegInstr::CallExternal { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    if called != declared {
        return Err(format!(
            "external call table mismatch: instructions={called:?}, imports={declared:?}"
        ));
    }
    Ok(())
}

fn verify_instruction(
    function_id: usize,
    ip: usize,
    function: &WireFunction,
    function_count: usize,
    instruction: &RegInstr,
) -> Result<(), String> {
    match instruction {
        RegInstr::Jump { target } => verify_target(function_id, ip, function.code.len(), *target)?,
        RegInstr::JumpIfBool { target, .. } => {
            verify_target(function_id, ip, function.code.len(), *target)?
        }
        RegInstr::JumpIfIntCompare { target, .. } => {
            verify_target(function_id, ip, function.code.len(), *target)?
        }
        RegInstr::MatchOption {
            some_ip, none_ip, ..
        } => {
            verify_target(function_id, ip, function.code.len(), *some_ip)?;
            verify_target(function_id, ip, function.code.len(), *none_ip)?;
        }
        RegInstr::MatchResult { ok_ip, err_ip, .. } => {
            verify_target(function_id, ip, function.code.len(), *ok_ip)?;
            verify_target(function_id, ip, function.code.len(), *err_ip)?;
        }
        RegInstr::MatchVariant {
            match_ip, else_ip, ..
        } => {
            verify_target(function_id, ip, function.code.len(), *match_ip)?;
            verify_target(function_id, ip, function.code.len(), *else_ip)?;
        }
        RegInstr::MatchMapGet {
            some_ip, none_ip, ..
        }
        | RegInstr::MatchSortedMapGet {
            some_ip, none_ip, ..
        } => {
            verify_target(function_id, ip, function.code.len(), *some_ip)?;
            verify_target(function_id, ip, function.code.len(), *none_ip)?;
        }
        RegInstr::MakeClosure { function, .. }
        | RegInstr::CallKnown { function, .. }
        | RegInstr::SpawnTask { function, .. } => {
            if *function >= function_count {
                return Err(format!(
                    "function {function_id} instruction {ip} references missing function {function}"
                ));
            }
        }
        RegInstr::CallDynamic { dispatch, .. } => {
            if let Some((_, target)) = dispatch
                .iter()
                .find(|(_, target)| *target >= function_count)
            {
                return Err(format!(
                    "function {function_id} instruction {ip} references missing dispatch function {target}"
                ));
            }
        }
        _ => {}
    }

    let value = serde_json::to_value(instruction)
        .map_err(|error| format!("cannot inspect instruction {ip}: {error}"))?;
    // Serde's externally tagged representation encodes unit variants (currently
    // `TailCallGuard`) as a string and data-carrying variants as a one-entry
    // object. Both are canonical instruction encodings.
    if value.as_str().is_some() {
        return Ok(());
    }
    let (opcode, fields) = value
        .as_object()
        .filter(|outer| outer.len() == 1)
        .and_then(|outer| outer.iter().next())
        .ok_or_else(|| format!("function {function_id} instruction {ip} has invalid encoding"))?;
    if let Some(fields) = fields.as_object() {
        for (field, value) in fields {
            verify_register_field(function_id, ip, function.regs, opcode, field, value)?;
        }
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
) -> Result<(), String> {
    let scalar_reg = matches!(
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
    if scalar_reg {
        let reg = value.as_u64().ok_or_else(|| {
            format!("function {function_id} instruction {ip} field `{field}` is not a register")
        })? as usize;
        return verify_reg(function_id, register_count, reg, field);
    }
    if matches!(field, "args" | "captures" | "cleanup" | "handles" | "items") {
        for item in value.as_array().ok_or_else(|| {
            format!("function {function_id} instruction {ip} field `{field}` is not an array")
        })? {
            let reg = item.as_u64().ok_or_else(|| {
                format!("function {function_id} instruction {ip} field `{field}` contains a non-register")
            })? as usize;
            verify_reg(function_id, register_count, reg, field)?;
        }
    } else if field == "fields" {
        verify_tuple_registers(function_id, ip, register_count, value, &[1])?;
    } else if field == "entries" {
        verify_tuple_registers(function_id, ip, register_count, value, &[0, 1])?;
    }
    Ok(())
}

fn verify_tuple_registers(
    function_id: usize,
    ip: usize,
    register_count: usize,
    value: &serde_json::Value,
    positions: &[usize],
) -> Result<(), String> {
    for tuple in value
        .as_array()
        .ok_or_else(|| "tuple list is not an array".to_string())?
    {
        let tuple = tuple
            .as_array()
            .ok_or_else(|| "tuple entry is not an array".to_string())?;
        for &position in positions {
            let reg = tuple
                .get(position)
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| {
                    format!("function {function_id} instruction {ip} has invalid register tuple")
                })? as usize;
            verify_reg(function_id, register_count, reg, "tuple")?;
        }
    }
    Ok(())
}

fn verify_reg(
    function: usize,
    register_count: usize,
    reg: usize,
    field: &str,
) -> Result<(), String> {
    if reg >= register_count {
        Err(format!(
            "function {function} field `{field}` references register {reg}, limit is {register_count}"
        ))
    } else {
        Ok(())
    }
}

fn verify_target(function: usize, ip: usize, code_len: usize, target: usize) -> Result<(), String> {
    if target >= code_len {
        Err(format!(
            "function {function} instruction {ip} jumps outside its body to {target}"
        ))
    } else {
        Ok(())
    }
}

fn external_imports(
    unit: &RegUnit,
    executable: &rsscript_lowering::ExecutableIr,
) -> Vec<ExternalImport> {
    let called = unit
        .functions
        .iter()
        .flat_map(|function| &function.code)
        .filter_map(|instruction| match instruction {
            RegInstr::CallExternal { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    executable
        .external_imports()
        .iter()
        .map(|import| ExternalImport {
            symbol: import.symbol.clone(),
            signature_hash: import.signature.hash(),
            abi_version: RUNTIME_ABI_VERSION,
        })
        .filter(|import| called.contains(import.symbol.as_str()))
        .fold(BTreeMap::new(), |mut imports, import| {
            imports.entry(import.symbol.clone()).or_insert(import);
            imports
        })
        .into_values()
        .collect()
}

fn source_hash(validated: &ValidatedProgram) -> String {
    let mut input = Vec::new();
    for snapshot in [
        validated.database().sources(),
        validated.database().interfaces(),
    ] {
        for file in snapshot.files() {
            input.extend_from_slice(&(file.path().len() as u64).to_be_bytes());
            input.extend_from_slice(file.path().as_bytes());
            input.extend_from_slice(&(file.text().len() as u64).to_be_bytes());
            input.extend_from_slice(file.text().as_bytes());
        }
    }
    format!("sha256:{:x}", Sha256::digest(input))
}

fn bytecode_error(error: BytecodeError) -> EvalError {
    EvalError::Runtime(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiled_bytecode_round_trips_before_execution() {
        let executable = reg_vm_compile_source(
            "roundtrip.rss",
            "fn main() -> Unit { let x = 20 + 22; return Unit }",
        )
        .expect("compile");
        let bytes = executable.to_bytecode().expect("serialize");
        let loaded = RegVmExecutable::from_bytecode(&bytes).expect("verify and load");
        assert_eq!(
            loaded
                .eval_main_with_args(Vec::<String>::new())
                .expect("run")
                .value,
            "Unit"
        );
    }

    #[test]
    fn verifier_rejects_out_of_range_register_even_with_valid_checksum() {
        let executable = reg_vm_compile_source("bad-reg.rss", "fn main() -> Unit { return Unit }")
            .expect("compile");
        let mut artifact = executable.bytecode_artifact().clone();
        let mut wire: WireUnit = serde_json::from_slice(&artifact.payload).expect("wire payload");
        wire.functions[0].code[0] = RegInstr::LoadUnit {
            dst: wire.functions[0].regs,
        };
        artifact = BytecodeArtifact::new(
            artifact.header.language_version,
            artifact.header.runtime_abi_version,
            artifact.header.source_content_hash,
            artifact.imports,
            serde_json::to_vec(&wire).expect("payload"),
        )
        .expect("checksummed artifact");
        let error = RegVmExecutable::from_bytecode(&artifact.to_bytes().expect("bytes"))
            .expect_err("invalid register must fail before execution");
        assert!(matches!(error, EvalError::Runtime(message) if message.contains("register")));
    }

    #[test]
    fn verifier_accepts_unit_variant_instructions() {
        let function = WireFunction {
            name: "tail_recursive".to_string(),
            params: 0,
            captures: 0,
            regs: 0,
            local_regs: BTreeMap::new(),
            code: vec![RegInstr::TailCallGuard],
        };

        verify_instruction(0, 0, &function, 1, &function.code[0])
            .expect("unit instruction variants are valid bytecode");
    }
}
