//! Typed, numeric executable wire model for the next bytecode ISA.
//!
//! This is intentionally separate from the v1 JSON-shaped payload decoder.
//! It provides a finite instruction vocabulary, numeric table identities, and
//! fixed operand layouts before a v2 writer is enabled. The v1 reader remains
//! the only deployed artifact reader/writer during this migration.

use rsscript_abi_model::WireType;
use serde::{Deserialize, Serialize};

use crate::{BytecodeError, BytecodeLimits};

macro_rules! wire_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u32);

        impl $name {
            pub const fn new(value: u32) -> Self {
                Self(value)
            }

            pub const fn index(self) -> usize {
                self.0 as usize
            }
        }
    };
}

wire_id!(WireFunctionId);
wire_id!(WireTypeId);
wire_id!(WireConstantId);
wire_id!(WireImportId);
wire_id!(WireRegister);
wire_id!(WireInstructionOffset);

/// Numeric instruction opcode. Each variant has one fixed operand layout
/// declared by [`WireOpcodeV2::operand_count`]; future codecs encode the tag as
/// this `u8` rather than a source-language opcode name.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireOpcodeV2 {
    LoadConstant = 1,
    Move = 2,
    AddInt = 3,
    Call = 4,
    CallExternal = 5,
    Jump = 6,
    JumpIfTrue = 7,
    Return = 8,
    ResourceDrop = 9,
    Spawn = 10,
    Await = 11,
    Cancel = 12,
}

impl WireOpcodeV2 {
    pub const fn operand_count(self) -> usize {
        match self {
            Self::LoadConstant => 2, // dst, constant
            Self::Move => 2,         // dst, source
            Self::AddInt => 3,       // dst, left, right
            Self::Call => 3,         // dst, function, argument-list constant
            Self::CallExternal => 3, // dst, import, argument-list constant
            Self::Jump => 1,         // instruction target
            Self::JumpIfTrue => 2,   // condition, instruction target
            Self::Return => 1,       // source
            Self::ResourceDrop => 1, // resource register
            Self::Spawn => 3,        // dst, function, argument-list constant
            Self::Await => 2,        // dst, task register
            Self::Cancel => 1,       // task register
        }
    }

    fn from_raw(value: u8) -> Option<Self> {
        Some(match value {
            1 => Self::LoadConstant,
            2 => Self::Move,
            3 => Self::AddInt,
            4 => Self::Call,
            5 => Self::CallExternal,
            6 => Self::Jump,
            7 => Self::JumpIfTrue,
            8 => Self::Return,
            9 => Self::ResourceDrop,
            10 => Self::Spawn,
            11 => Self::Await,
            12 => Self::Cancel,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireInstructionV2 {
    opcode: WireOpcodeV2,
    operands: Vec<u32>,
}

impl WireInstructionV2 {
    pub fn new(opcode: WireOpcodeV2, operands: Vec<u32>) -> Self {
        Self { opcode, operands }
    }

    pub const fn opcode(&self) -> WireOpcodeV2 {
        self.opcode
    }

    pub fn operands(&self) -> &[u32] {
        &self.operands
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireFunctionV2 {
    parameter_count: u32,
    register_count: u32,
    instructions: Vec<WireInstructionV2>,
}

impl WireFunctionV2 {
    pub fn new(
        parameter_count: u32,
        register_count: u32,
        instructions: Vec<WireInstructionV2>,
    ) -> Self {
        Self {
            parameter_count,
            register_count,
            instructions,
        }
    }

    pub const fn parameter_count(&self) -> u32 {
        self.parameter_count
    }

    pub const fn register_count(&self) -> u32 {
        self.register_count
    }

    pub fn instructions(&self) -> &[WireInstructionV2] {
        &self.instructions
    }
}

/// Separate tables ensure executable instructions never use source-level names
/// as identities. Constants remain opaque bytes until the v2 constant codec is
/// introduced; their bounds are already validated here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireProgramV2 {
    types: Vec<WireType>,
    constants: Vec<Vec<u8>>,
    import_count: u32,
    functions: Vec<WireFunctionV2>,
}

impl WireProgramV2 {
    pub fn new(
        types: Vec<WireType>,
        constants: Vec<Vec<u8>>,
        import_count: u32,
        functions: Vec<WireFunctionV2>,
    ) -> Self {
        Self {
            types,
            constants,
            import_count,
            functions,
        }
    }

    pub fn types(&self) -> &[WireType] {
        &self.types
    }

    pub fn constants(&self) -> &[Vec<u8>] {
        &self.constants
    }

    pub const fn import_count(&self) -> u32 {
        self.import_count
    }

    pub fn functions(&self) -> &[WireFunctionV2] {
        &self.functions
    }

    /// Structural v2 verification that is independent of the compiler and VM.
    /// Data-flow/type verification will layer on this model rather than decode a
    /// `serde_json::Value` object tree as v1 currently does.
    pub fn verify(&self, limits: BytecodeLimits) -> Result<(), BytecodeError> {
        if self.functions.len() > limits.max_functions {
            return Err(BytecodeError::LimitExceeded("v2 functions"));
        }
        for (function_index, function) in self.functions.iter().enumerate() {
            let register_count = function.register_count as usize;
            if register_count > limits.max_registers_per_function {
                return Err(BytecodeError::LimitExceeded("v2 registers"));
            }
            if function.parameter_count > function.register_count {
                return Err(invalid(format!(
                    "v2 function {function_index} has more parameters than registers"
                )));
            }
            if function.instructions.len() > limits.max_instructions {
                return Err(BytecodeError::LimitExceeded("v2 instructions"));
            }
            for (offset, instruction) in function.instructions.iter().enumerate() {
                verify_instruction(
                    instruction,
                    function_index,
                    offset,
                    register_count,
                    self.constants.len(),
                    self.functions.len(),
                    self.import_count as usize,
                    function.instructions.len(),
                )?;
            }
        }
        Ok(())
    }
}

/// Encode a v2 executable payload with array-shaped numeric instruction
/// records. No source names or field-map opcode keys participate in the wire
/// representation.
pub fn encode_program(program: &WireProgramV2) -> Result<Vec<u8>, BytecodeError> {
    program.verify(BytecodeLimits::default())?;
    let raw = RawProgramV2::from(program);
    crate::encode_executable_payload(&raw)
}

/// Decode, canonicalize, and structurally verify a v2 executable payload.
/// This is deliberately independent from the v1 `serde_json::Value` verifier;
/// callers receive a typed program only after the canonical-byte check and all
/// numeric ID bounds succeed.
pub fn decode_program(
    payload: &[u8],
    limits: BytecodeLimits,
) -> Result<VerifiedProgramV2, BytecodeError> {
    if payload.len() > limits.max_payload_bytes {
        return Err(BytecodeError::LimitExceeded("v2 payload bytes"));
    }
    let raw: RawProgramV2 = crate::decode_executable_payload(payload)?;
    if crate::encode_executable_payload(&raw)? != payload {
        return Err(invalid("v2 executable CBOR is not canonical".to_owned()));
    }
    let program = WireProgramV2::try_from(raw)?;
    program.verify(limits)?;
    Ok(VerifiedProgramV2 { program })
}

/// Opaque result of bounded v2 payload decoding. Its program fields stay
/// private so execution backends cannot accidentally receive a caller-built
/// decoded instruction vector through this path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedProgramV2 {
    program: WireProgramV2,
}

impl VerifiedProgramV2 {
    pub fn program(&self) -> &WireProgramV2 {
        &self.program
    }

    pub fn functions(&self) -> &[WireFunctionV2] {
        self.program.functions()
    }
}

/// V2 verification owner. A future Artifact v2 verifier will compose this
/// after envelope/version/import validation and pass only `VerifiedProgramV2`
/// to the VM decoder.
pub struct BytecodeV2Verifier {
    limits: BytecodeLimits,
}

impl BytecodeV2Verifier {
    pub const fn new(limits: BytecodeLimits) -> Self {
        Self { limits }
    }

    pub fn verify_payload(&self, payload: &[u8]) -> Result<VerifiedProgramV2, BytecodeError> {
        decode_program(payload, self.limits)
    }
}

impl Default for BytecodeV2Verifier {
    fn default() -> Self {
        Self::new(BytecodeLimits::default())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RawProgramV2 {
    types: Vec<WireType>,
    constants: Vec<Vec<u8>>,
    import_count: u32,
    functions: Vec<RawFunctionV2>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RawFunctionV2 {
    parameter_count: u32,
    register_count: u32,
    instructions: Vec<RawInstructionV2>,
}

/// Tuple serialization fixes the executable layout to `[opcode, operands]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RawInstructionV2(u8, Vec<u32>);

impl From<&WireProgramV2> for RawProgramV2 {
    fn from(program: &WireProgramV2) -> Self {
        Self {
            types: program.types.clone(),
            constants: program.constants.clone(),
            import_count: program.import_count,
            functions: program
                .functions
                .iter()
                .map(|function| RawFunctionV2 {
                    parameter_count: function.parameter_count,
                    register_count: function.register_count,
                    instructions: function
                        .instructions
                        .iter()
                        .map(|instruction| {
                            RawInstructionV2(instruction.opcode as u8, instruction.operands.clone())
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

impl TryFrom<RawProgramV2> for WireProgramV2 {
    type Error = BytecodeError;

    fn try_from(raw: RawProgramV2) -> Result<Self, Self::Error> {
        let mut functions = Vec::with_capacity(raw.functions.len());
        for (function, raw_function) in raw.functions.into_iter().enumerate() {
            let mut instructions = Vec::with_capacity(raw_function.instructions.len());
            for (offset, RawInstructionV2(opcode, operands)) in
                raw_function.instructions.into_iter().enumerate()
            {
                let opcode = WireOpcodeV2::from_raw(opcode).ok_or_else(|| {
                    invalid(format!(
                        "v2 function {function} instruction {offset} has unknown opcode {opcode}"
                    ))
                })?;
                instructions.push(WireInstructionV2 { opcode, operands });
            }
            functions.push(WireFunctionV2 {
                parameter_count: raw_function.parameter_count,
                register_count: raw_function.register_count,
                instructions,
            });
        }
        Ok(Self {
            types: raw.types,
            constants: raw.constants,
            import_count: raw.import_count,
            functions,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_instruction(
    instruction: &WireInstructionV2,
    function: usize,
    offset: usize,
    registers: usize,
    constants: usize,
    functions: usize,
    imports: usize,
    instructions: usize,
) -> Result<(), BytecodeError> {
    if instruction.operands.len() != instruction.opcode.operand_count() {
        return Err(invalid(format!(
            "v2 function {function} instruction {offset} {:?} has {} operands, expected {}",
            instruction.opcode,
            instruction.operands.len(),
            instruction.opcode.operand_count()
        )));
    }
    let register = |operand: usize| {
        check_index(
            instruction.operands[operand],
            registers,
            "register",
            function,
            offset,
        )
    };
    match instruction.opcode {
        WireOpcodeV2::LoadConstant => {
            register(0)?;
            check_index(
                instruction.operands[1],
                constants,
                "constant",
                function,
                offset,
            )
        }
        WireOpcodeV2::Move | WireOpcodeV2::Await => {
            register(0)?;
            register(1)
        }
        WireOpcodeV2::AddInt => {
            register(0)?;
            register(1)?;
            register(2)
        }
        WireOpcodeV2::Call | WireOpcodeV2::Spawn => {
            register(0)?;
            check_index(
                instruction.operands[1],
                functions,
                "function",
                function,
                offset,
            )?;
            check_index(
                instruction.operands[2],
                constants,
                "argument list",
                function,
                offset,
            )
        }
        WireOpcodeV2::CallExternal => {
            register(0)?;
            check_index(instruction.operands[1], imports, "import", function, offset)?;
            check_index(
                instruction.operands[2],
                constants,
                "argument list",
                function,
                offset,
            )
        }
        WireOpcodeV2::Jump => check_index(
            instruction.operands[0],
            instructions,
            "instruction target",
            function,
            offset,
        ),
        WireOpcodeV2::JumpIfTrue => {
            register(0)?;
            check_index(
                instruction.operands[1],
                instructions,
                "instruction target",
                function,
                offset,
            )
        }
        WireOpcodeV2::Return | WireOpcodeV2::ResourceDrop | WireOpcodeV2::Cancel => register(0),
    }
}

fn check_index(
    value: u32,
    limit: usize,
    kind: &str,
    function: usize,
    offset: usize,
) -> Result<(), BytecodeError> {
    if (value as usize) >= limit {
        return Err(invalid(format!(
            "v2 function {function} instruction {offset} references invalid {kind} {value}"
        )));
    }
    Ok(())
}

fn invalid(message: String) -> BytecodeError {
    BytecodeError::InvalidPayload(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn program(instructions: Vec<WireInstructionV2>) -> WireProgramV2 {
        WireProgramV2::new(
            vec![WireType::Unit],
            vec![vec![0]],
            1,
            vec![WireFunctionV2::new(0, 2, instructions)],
        )
    }

    #[test]
    fn numeric_v2_program_verifies_without_source_names() {
        let program = program(vec![
            WireInstructionV2::new(WireOpcodeV2::LoadConstant, vec![0, 0]),
            WireInstructionV2::new(WireOpcodeV2::CallExternal, vec![1, 0, 0]),
            WireInstructionV2::new(WireOpcodeV2::Return, vec![1]),
        ]);
        program
            .verify(BytecodeLimits::default())
            .expect("typed numeric v2 program is structurally valid");
    }

    #[test]
    fn v2_rejects_wrong_operands_and_out_of_range_ids() {
        let malformed = program(vec![WireInstructionV2::new(
            WireOpcodeV2::Return,
            vec![0, 1],
        )]);
        assert!(matches!(
            malformed.verify(BytecodeLimits::default()),
            Err(BytecodeError::InvalidPayload(_))
        ));
        let invalid_import = program(vec![WireInstructionV2::new(
            WireOpcodeV2::CallExternal,
            vec![0, 1, 0],
        )]);
        assert!(matches!(
            invalid_import.verify(BytecodeLimits::default()),
            Err(BytecodeError::InvalidPayload(_))
        ));
    }

    #[test]
    fn v2_codec_round_trips_only_canonical_numeric_instructions() {
        let original = program(vec![
            WireInstructionV2::new(WireOpcodeV2::LoadConstant, vec![0, 0]),
            WireInstructionV2::new(WireOpcodeV2::Return, vec![0]),
        ]);
        let bytes = encode_program(&original).expect("v2 program encodes");
        let decoded = BytecodeV2Verifier::default()
            .verify_payload(&bytes)
            .expect("canonical v2 payload decodes");
        assert_eq!(decoded.program(), &original);

        let raw = RawProgramV2 {
            types: vec![WireType::Unit],
            constants: vec![vec![0]],
            import_count: 0,
            functions: vec![RawFunctionV2 {
                parameter_count: 0,
                register_count: 1,
                instructions: vec![RawInstructionV2(255, vec![])],
            }],
        };
        let unknown = crate::encode_executable_payload(&raw).expect("malformed test payload");
        assert!(matches!(
            decode_program(&unknown, BytecodeLimits::default()),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("unknown opcode")
        ));
    }
}
