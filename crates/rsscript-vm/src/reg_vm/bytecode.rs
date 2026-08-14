use std::collections::BTreeMap;
#[cfg(feature = "legacy-exec-ir")]
use std::collections::BTreeSet;
use std::rc::Rc;

#[cfg(feature = "legacy-exec-ir")]
use rsscript_abi_model::{ExternalImport, RUNTIME_ABI_VERSION};
#[cfg(feature = "legacy-exec-ir")]
use rsscript_bytecode::BytecodeVerifier;
use rsscript_bytecode::{BytecodeArtifact, BytecodeError, VerifiedBytecode};
use serde::{Deserialize, Serialize};

use super::*;

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

#[cfg(feature = "legacy-exec-ir")]
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

#[cfg(feature = "legacy-exec-ir")]
pub(super) fn encode_and_verify(
    unit: &RegUnit,
    source_content_hash: &str,
    interface_catalog_digest: &str,
    executable: &rsscript_exec_ir::ExecutableIr,
) -> Result<VerifiedRegBytecode, EvalError> {
    encode_and_verify_with_imports(
        unit,
        source_content_hash,
        interface_catalog_digest,
        external_imports(unit, executable),
    )
}

#[cfg(feature = "legacy-exec-ir")]
pub(super) fn encode_and_verify_with_imports(
    unit: &RegUnit,
    source_content_hash: &str,
    interface_catalog_digest: &str,
    imports: Vec<ExternalImport>,
) -> Result<VerifiedRegBytecode, EvalError> {
    let payload = rsscript_bytecode::encode_executable_payload(&WireUnit::from(unit))
        .map_err(|error| EvalError::Runtime(format!("cannot encode VM bytecode: {error}")))?;
    let artifact = BytecodeArtifact::new(
        rsscript_bytecode::LANGUAGE_SEMANTICS_VERSION,
        env!("CARGO_PKG_VERSION"),
        interface_catalog_digest,
        RUNTIME_ABI_VERSION,
        source_content_hash,
        imports,
        payload,
    )
    .map_err(bytecode_error)?;
    verify_bytes(&artifact.to_bytes().map_err(bytecode_error)?)
}

#[cfg(feature = "legacy-exec-ir")]
pub(super) fn verify_bytes(bytes: &[u8]) -> Result<VerifiedRegBytecode, EvalError> {
    verify_bytes_with_context(bytes, rsscript_bytecode::VerificationContext::default())
}

#[cfg(feature = "legacy-exec-ir")]
pub(super) fn verify_bytes_with_context(
    bytes: &[u8],
    context: rsscript_bytecode::VerificationContext<'_>,
) -> Result<VerifiedRegBytecode, EvalError> {
    let verified = BytecodeVerifier::default()
        .verify_with_context(bytes, context)
        .map_err(bytecode_error)?;
    decode_verified_bytecode(verified, context)
}

pub(super) fn decode_verified_bytecode(
    verified: VerifiedBytecode,
    context: rsscript_bytecode::VerificationContext<'_>,
) -> Result<VerifiedRegBytecode, EvalError> {
    let artifact = verified.into_artifact();
    context.check().map_err(bytecode_error)?;
    // `VerifiedBytecode` is constructed only by `BytecodeVerifier`, which
    // owns payload, register, control-flow, and import validation. The VM is
    // intentionally decoder/executor-only here.
    let executable: WireUnit = rsscript_bytecode::decode_executable_payload(&artifact.payload)
        .map_err(|error| bytecode_error(BytecodeError::InvalidPayload(error.to_string())))?;
    Ok(VerifiedRegBytecode {
        artifact,
        executable: executable.into_reg_unit(),
    })
}

#[cfg(feature = "legacy-exec-ir")]
fn external_imports(
    unit: &RegUnit,
    executable: &rsscript_exec_ir::ExecutableIr,
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
            signature: import.signature.clone(),
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

pub(super) fn bytecode_error(error: BytecodeError) -> EvalError {
    EvalError::Runtime(error.to_string())
}
