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

/// Numeric instruction opcode. Its layout is owned by the one
/// [`INSTRUCTION_SCHEMA_V2`] table; future codecs encode the tag as this `u8`
/// rather than a source-language opcode name.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperandKindV2 {
    Register,
    Constant,
    Function,
    Import,
    InstructionTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstructionSchemaV2 {
    pub opcode: WireOpcodeV2,
    pub name: &'static str,
    pub operands: &'static [OperandKindV2],
}

const REGISTER_CONSTANT: &[OperandKindV2] = &[OperandKindV2::Register, OperandKindV2::Constant];
const REGISTER_REGISTER: &[OperandKindV2] = &[OperandKindV2::Register, OperandKindV2::Register];
const THREE_REGISTERS: &[OperandKindV2] = &[
    OperandKindV2::Register,
    OperandKindV2::Register,
    OperandKindV2::Register,
];
const REGISTER_FUNCTION_CONSTANT: &[OperandKindV2] = &[
    OperandKindV2::Register,
    OperandKindV2::Function,
    OperandKindV2::Constant,
];
const REGISTER_IMPORT_CONSTANT: &[OperandKindV2] = &[
    OperandKindV2::Register,
    OperandKindV2::Import,
    OperandKindV2::Constant,
];
const REGISTER_TARGET: &[OperandKindV2] =
    &[OperandKindV2::Register, OperandKindV2::InstructionTarget];
const TARGET: &[OperandKindV2] = &[OperandKindV2::InstructionTarget];
const REGISTER: &[OperandKindV2] = &[OperandKindV2::Register];

/// Single source of truth for v2 opcode tags, operand arity, operand identity
/// classes, and generated reference documentation.
pub const INSTRUCTION_SCHEMA_V2: &[InstructionSchemaV2] = &[
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::LoadConstant,
        name: "load_constant",
        operands: REGISTER_CONSTANT,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::Move,
        name: "move",
        operands: REGISTER_REGISTER,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::AddInt,
        name: "add_int",
        operands: THREE_REGISTERS,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::Call,
        name: "call",
        operands: REGISTER_FUNCTION_CONSTANT,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::CallExternal,
        name: "call_external",
        operands: REGISTER_IMPORT_CONSTANT,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::Jump,
        name: "jump",
        operands: TARGET,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::JumpIfTrue,
        name: "jump_if_true",
        operands: REGISTER_TARGET,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::Return,
        name: "return",
        operands: REGISTER,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::ResourceDrop,
        name: "resource_drop",
        operands: REGISTER,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::Spawn,
        name: "spawn",
        operands: REGISTER_FUNCTION_CONSTANT,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::Await,
        name: "await",
        operands: REGISTER_REGISTER,
    },
    InstructionSchemaV2 {
        opcode: WireOpcodeV2::Cancel,
        name: "cancel",
        operands: REGISTER,
    },
];

/// Render the checked-in opcode schema for Artifact/ISA documentation. Keeping
/// this derived from the same table used by codec and validation prevents prose
/// from becoming an independent opcode contract.
pub fn instruction_schema_markdown() -> String {
    let mut output = String::from("| Opcode | Tag | Operands |\n| --- | ---: | --- |\n");
    for schema in INSTRUCTION_SCHEMA_V2 {
        let operands = schema
            .operands
            .iter()
            .map(|kind| match kind {
                OperandKindV2::Register => "register",
                OperandKindV2::Constant => "constant",
                OperandKindV2::Function => "function",
                OperandKindV2::Import => "import",
                OperandKindV2::InstructionTarget => "instruction_target",
            })
            .collect::<Vec<_>>()
            .join(", ");
        output.push_str(&format!(
            "| `{}` | {} | {} |\n",
            schema.name, schema.opcode as u8, operands
        ));
    }
    output
}

impl WireOpcodeV2 {
    pub const fn operand_count(self) -> usize {
        self.schema().operands.len()
    }

    pub const fn schema(self) -> &'static InstructionSchemaV2 {
        &INSTRUCTION_SCHEMA_V2[(self as u8 - 1) as usize]
    }

    fn from_raw(value: u8) -> Option<Self> {
        INSTRUCTION_SCHEMA_V2
            .iter()
            .find(|schema| schema.opcode as u8 == value)
            .map(|schema| schema.opcode)
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

/// Numeric link into the Artifact-level import table. The external symbol and
/// signature remain in that separately verified table; executable code only
/// carries this stable index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireImportV2 {
    artifact_import: u32,
}

impl WireImportV2 {
    pub const fn new(artifact_import: u32) -> Self {
        Self { artifact_import }
    }

    pub const fn artifact_import(self) -> u32 {
        self.artifact_import
    }
}

/// Named exports deliberately use a numeric function identity in executable
/// code. A future Artifact section supplies the stable export-name string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireExportV2 {
    function: WireFunctionId,
}

impl WireExportV2 {
    pub const fn new(function: WireFunctionId) -> Self {
        Self { function }
    }

    pub const fn function(self) -> WireFunctionId {
        self.function
    }
}

/// Optional source/debug side-table record. It cannot participate in
/// executable control flow, but its numeric location is verified against the
/// code table so a malformed debug section cannot reference arbitrary code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WireDebugLocationV2 {
    function: WireFunctionId,
    instruction: WireInstructionOffset,
    source_start: u32,
    source_end: u32,
}

impl WireDebugLocationV2 {
    pub const fn new(
        function: WireFunctionId,
        instruction: WireInstructionOffset,
        source_start: u32,
        source_end: u32,
    ) -> Self {
        Self {
            function,
            instruction,
            source_start,
            source_end,
        }
    }
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
    imports: Vec<WireImportV2>,
    exports: Vec<WireExportV2>,
    functions: Vec<WireFunctionV2>,
    debug: Vec<WireDebugLocationV2>,
}

impl WireProgramV2 {
    pub fn new(
        types: Vec<WireType>,
        constants: Vec<Vec<u8>>,
        import_count: u32,
        functions: Vec<WireFunctionV2>,
    ) -> Self {
        let imports = (0..import_count).map(WireImportV2::new).collect();
        Self::with_tables(types, constants, imports, Vec::new(), functions, Vec::new())
    }

    pub fn with_tables(
        types: Vec<WireType>,
        constants: Vec<Vec<u8>>,
        imports: Vec<WireImportV2>,
        exports: Vec<WireExportV2>,
        functions: Vec<WireFunctionV2>,
        debug: Vec<WireDebugLocationV2>,
    ) -> Self {
        Self {
            types,
            constants,
            imports,
            exports,
            functions,
            debug,
        }
    }

    pub fn types(&self) -> &[WireType] {
        &self.types
    }

    pub fn constants(&self) -> &[Vec<u8>] {
        &self.constants
    }

    pub const fn import_count(&self) -> u32 {
        self.imports.len() as u32
    }

    pub fn imports(&self) -> &[WireImportV2] {
        &self.imports
    }

    pub fn exports(&self) -> &[WireExportV2] {
        &self.exports
    }

    pub fn functions(&self) -> &[WireFunctionV2] {
        &self.functions
    }

    pub fn debug_locations(&self) -> &[WireDebugLocationV2] {
        &self.debug
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
                    self.imports.len(),
                    function.instructions.len(),
                )?;
            }
        }
        for (index, export) in self.exports.iter().enumerate() {
            if export.function.index() >= self.functions.len() {
                return Err(invalid(format!(
                    "v2 export {index} references invalid function {}",
                    export.function.index()
                )));
            }
        }
        for (index, location) in self.debug.iter().enumerate() {
            let Some(function) = self.functions.get(location.function.index()) else {
                return Err(invalid(format!(
                    "v2 debug location {index} references invalid function {}",
                    location.function.index()
                )));
            };
            if location.instruction.index() >= function.instructions.len() {
                return Err(invalid(format!(
                    "v2 debug location {index} references invalid instruction {}",
                    location.instruction.index()
                )));
            }
            if location.source_start > location.source_end {
                return Err(invalid(format!(
                    "v2 debug location {index} has inverted source range"
                )));
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
    imports: Vec<u32>,
    exports: Vec<u32>,
    functions: Vec<RawFunctionV2>,
    debug: Vec<RawDebugLocationV2>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RawDebugLocationV2(u32, u32, u32, u32);

impl From<&WireProgramV2> for RawProgramV2 {
    fn from(program: &WireProgramV2) -> Self {
        Self {
            types: program.types.clone(),
            constants: program.constants.clone(),
            imports: program
                .imports
                .iter()
                .map(|import| import.artifact_import)
                .collect(),
            exports: program
                .exports
                .iter()
                .map(|export| export.function.0)
                .collect(),
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
            debug: program
                .debug
                .iter()
                .map(|location| {
                    RawDebugLocationV2(
                        location.function.0,
                        location.instruction.0,
                        location.source_start,
                        location.source_end,
                    )
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
            imports: raw.imports.into_iter().map(WireImportV2::new).collect(),
            exports: raw
                .exports
                .into_iter()
                .map(|function| WireExportV2::new(WireFunctionId::new(function)))
                .collect(),
            functions,
            debug: raw
                .debug
                .into_iter()
                .map(|RawDebugLocationV2(function, instruction, start, end)| {
                    WireDebugLocationV2::new(
                        WireFunctionId::new(function),
                        WireInstructionOffset::new(instruction),
                        start,
                        end,
                    )
                })
                .collect(),
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
    for (operand, kind) in instruction.opcode.schema().operands.iter().enumerate() {
        let (limit, name) = match kind {
            OperandKindV2::Register => (registers, "register"),
            OperandKindV2::Constant => (constants, "constant"),
            OperandKindV2::Function => (functions, "function"),
            OperandKindV2::Import => (imports, "import"),
            OperandKindV2::InstructionTarget => (instructions, "instruction target"),
        };
        check_index(instruction.operands[operand], limit, name, function, offset)?;
    }
    Ok(())
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
    use proptest::prelude::*;

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
            imports: vec![],
            exports: vec![],
            functions: vec![RawFunctionV2 {
                parameter_count: 0,
                register_count: 1,
                instructions: vec![RawInstructionV2(255, vec![])],
            }],
            debug: vec![],
        };
        let unknown = crate::encode_executable_payload(&raw).expect("malformed test payload");
        assert!(matches!(
            decode_program(&unknown, BytecodeLimits::default()),
            Err(BytecodeError::InvalidPayload(message)) if message.contains("unknown opcode")
        ));
    }

    #[test]
    fn instruction_schema_is_unique_and_drives_generated_reference() {
        let mut tags = INSTRUCTION_SCHEMA_V2
            .iter()
            .map(|schema| schema.opcode as u8)
            .collect::<Vec<_>>();
        tags.sort_unstable();
        tags.dedup();
        assert_eq!(tags.len(), INSTRUCTION_SCHEMA_V2.len());
        let reference = instruction_schema_markdown();
        assert!(reference.contains("`call_external` | 5 | register, import, constant"));
        assert!(reference.contains("`resource_drop` | 9 | register"));
    }

    #[test]
    fn v2_verifies_export_and_optional_debug_tables() {
        let valid = WireProgramV2::with_tables(
            vec![WireType::Unit],
            vec![vec![0]],
            vec![WireImportV2::new(4)],
            vec![WireExportV2::new(WireFunctionId::new(0))],
            vec![WireFunctionV2::new(
                0,
                1,
                vec![WireInstructionV2::new(WireOpcodeV2::Return, vec![0])],
            )],
            vec![WireDebugLocationV2::new(
                WireFunctionId::new(0),
                WireInstructionOffset::new(0),
                3,
                7,
            )],
        );
        let bytes = encode_program(&valid).expect("v2 tables encode");
        let decoded = BytecodeV2Verifier::default()
            .verify_payload(&bytes)
            .expect("v2 tables verify");
        assert_eq!(decoded.program(), &valid);

        let invalid_debug = WireProgramV2::with_tables(
            vec![WireType::Unit],
            vec![],
            vec![],
            vec![WireExportV2::new(WireFunctionId::new(1))],
            vec![WireFunctionV2::new(0, 1, vec![])],
            vec![WireDebugLocationV2::new(
                WireFunctionId::new(0),
                WireInstructionOffset::new(1),
                5,
                4,
            )],
        );
        assert!(matches!(
            invalid_debug.verify(BytecodeLimits::default()),
            Err(BytecodeError::InvalidPayload(_))
        ));
    }

    proptest! {
        #[test]
        fn arbitrary_bounded_v2_payload_never_panics(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
            let result = std::panic::catch_unwind(|| {
                BytecodeV2Verifier::default().verify_payload(&bytes)
            });
            prop_assert!(result.is_ok());
        }
    }
}
