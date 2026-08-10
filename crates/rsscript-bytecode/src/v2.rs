//! Typed, numeric executable wire model for the next bytecode ISA.
//!
//! This is intentionally separate from the v1 JSON-shaped payload decoder.
//! It provides a finite instruction vocabulary, numeric table identities, and
//! fixed operand layouts before a v2 writer is enabled. The v1 reader remains
//! the only deployed artifact reader/writer during this migration.

use rsscript_abi_model::WireType;

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
}
