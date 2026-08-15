use std::collections::BTreeMap;
use std::rc::Rc;

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
    #[serde(default)]
    _source_map: Vec<serde_json::Value>,
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
            })
            .collect::<Vec<_>>();
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

pub(super) fn bytecode_error(error: BytecodeError) -> EvalError {
    EvalError::Runtime(error.to_string())
}
