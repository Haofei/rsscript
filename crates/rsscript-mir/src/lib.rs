#![forbid(unsafe_code)]

//! Typed, owned, control-flow MIR shared by RSScript executable backends.
//!
//! MIR deliberately has no dependency on syntax, HIR, compiler orchestration,
//! Providers, or a runtime. Human-readable names are retained only in debug
//! tables; instructions use typed local identities.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use rsscript_abi_model::{ExternalSymbol, FunctionSignature, WireType};

macro_rules! mir_id {
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

mir_id!(FunctionId);
mir_id!(TypeId);
mir_id!(BlockId);
mir_id!(ValueId);
mir_id!(PlaceId);
mir_id!(BuiltinId);
mir_id!(ExternalSymbolId);
mir_id!(ResourceTypeId);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirBinaryOp {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    BitAnd,
    BitOr,
    BitXor,
    ShiftLeft,
    ShiftRight,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    LogicalAnd,
    LogicalOr,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MirLiteral {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
}

/// Backend-facing operations. A value is defined exactly once; mutable local
/// state is represented by a [`PlaceId`] rather than reusing a value identity.
#[derive(Debug, Clone, PartialEq)]
pub enum MirInstruction {
    LoadLiteral {
        destination: ValueId,
        value: MirLiteral,
    },
    ReadPlace {
        destination: ValueId,
        place: PlaceId,
    },
    WritePlace {
        place: PlaceId,
        value: ValueId,
    },
    Binary {
        destination: ValueId,
        op: MirBinaryOp,
        left: ValueId,
        right: ValueId,
    },
    Call {
        destination: ValueId,
        target: MirCallTarget,
        arguments: Vec<ValueId>,
    },
    Discard {
        value: ValueId,
    },
}

/// Fully resolved direct call target. Dynamic protocol dispatch deliberately
/// has no representation until its semantic and cancellation contracts can be
/// made explicit in MIR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirCallTarget {
    Function(FunctionId),
    External(ExternalSymbolId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirTerminator {
    Return(Option<ValueId>),
    Jump(BlockId),
    Branch {
        condition: ValueId,
        then_target: BlockId,
        else_target: BlockId,
    },
    Unreachable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BasicBlock {
    id: BlockId,
    instructions: Vec<MirInstruction>,
    terminator: MirTerminator,
}

impl BasicBlock {
    pub fn new(id: BlockId, instructions: Vec<MirInstruction>, terminator: MirTerminator) -> Self {
        Self {
            id,
            instructions,
            terminator,
        }
    }

    pub fn id(&self) -> BlockId {
        self.id
    }

    pub fn instructions(&self) -> &[MirInstruction] {
        &self.instructions
    }

    pub fn terminator(&self) -> &MirTerminator {
        &self.terminator
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirFunctionDebug {
    name: String,
    places: Vec<String>,
}

/// Resolved function ABI for backend consumption. Type identity is local to the
/// MIR module; human-readable type spelling is deliberately absent here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirFunctionSignature {
    parameter_types: Vec<TypeId>,
    result: TypeId,
    asynchronous: bool,
}

impl MirFunctionSignature {
    pub fn new(parameter_types: Vec<TypeId>, result: TypeId, asynchronous: bool) -> Self {
        Self {
            parameter_types,
            result,
            asynchronous,
        }
    }

    pub fn parameter_types(&self) -> &[TypeId] {
        &self.parameter_types
    }

    pub fn result(&self) -> TypeId {
        self.result
    }

    pub fn is_async(&self) -> bool {
        self.asynchronous
    }
}

impl MirFunctionDebug {
    pub fn new(name: impl Into<String>, places: Vec<String>) -> Self {
        Self {
            name: name.into(),
            places,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn places(&self) -> &[String] {
        &self.places
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirFunction {
    id: FunctionId,
    signature: MirFunctionSignature,
    place_count: u32,
    value_count: u32,
    blocks: Vec<BasicBlock>,
}

impl MirFunction {
    pub fn new(
        id: FunctionId,
        signature: MirFunctionSignature,
        place_count: u32,
        value_count: u32,
        blocks: Vec<BasicBlock>,
    ) -> Self {
        Self {
            id,
            signature,
            place_count,
            value_count,
            blocks,
        }
    }

    pub fn id(&self) -> FunctionId {
        self.id
    }

    pub fn signature(&self) -> &MirFunctionSignature {
        &self.signature
    }

    pub fn place_count(&self) -> u32 {
        self.place_count
    }

    pub fn value_count(&self) -> u32 {
        self.value_count
    }

    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirExternalImport {
    id: ExternalSymbolId,
    symbol: ExternalSymbol,
    signature: FunctionSignature,
}

impl MirExternalImport {
    pub fn new(id: ExternalSymbolId, symbol: ExternalSymbol, signature: FunctionSignature) -> Self {
        Self {
            id,
            symbol,
            signature,
        }
    }

    pub fn id(&self) -> ExternalSymbolId {
        self.id
    }

    pub fn symbol(&self) -> &ExternalSymbol {
        &self.symbol
    }

    pub fn signature(&self) -> &FunctionSignature {
        &self.signature
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirModule {
    types: Vec<WireType>,
    functions: Vec<MirFunction>,
    function_debug: Vec<MirFunctionDebug>,
    external_imports: Vec<MirExternalImport>,
}

impl MirModule {
    pub fn new(
        types: Vec<WireType>,
        functions: Vec<MirFunction>,
        function_debug: Vec<MirFunctionDebug>,
        external_imports: Vec<MirExternalImport>,
    ) -> Result<Self, MirValidationError> {
        let module = Self {
            types,
            functions,
            function_debug,
            external_imports,
        };
        module.verify()?;
        Ok(module)
    }

    pub fn functions(&self) -> &[MirFunction] {
        &self.functions
    }

    pub fn types(&self) -> &[WireType] {
        &self.types
    }

    pub fn ty(&self, id: TypeId) -> Option<&WireType> {
        self.types.get(id.index())
    }

    pub fn function(&self, id: FunctionId) -> Option<&MirFunction> {
        self.functions.get(id.index())
    }

    pub fn function_debug(&self, id: FunctionId) -> Option<&MirFunctionDebug> {
        self.function_debug.get(id.index())
    }

    pub fn external_imports(&self) -> &[MirExternalImport] {
        &self.external_imports
    }

    pub fn verify(&self) -> Result<(), MirValidationError> {
        if self.functions.len() != self.function_debug.len() {
            return Err(MirValidationError::FunctionDebugCount {
                functions: self.functions.len(),
                debug: self.function_debug.len(),
            });
        }
        for (index, function) in self.functions.iter().enumerate() {
            if function.id.index() != index {
                return Err(MirValidationError::FunctionIdMismatch {
                    expected: index,
                    actual: function.id.index(),
                });
            }
            verify_function(
                function,
                self.types.len(),
                self.functions.len(),
                self.external_imports.len(),
            )?;
        }
        for (index, import) in self.external_imports.iter().enumerate() {
            if import.id.index() != index {
                return Err(MirValidationError::ExternalImportIdMismatch {
                    expected: index,
                    actual: import.id.index(),
                });
            }
        }
        Ok(())
    }
}

fn verify_function(
    function: &MirFunction,
    type_count: usize,
    function_count: usize,
    external_import_count: usize,
) -> Result<(), MirValidationError> {
    if function.blocks.is_empty() {
        return Err(MirValidationError::EmptyFunction {
            function: function.id,
        });
    }
    for ty in function
        .signature
        .parameter_types()
        .iter()
        .copied()
        .chain(std::iter::once(function.signature.result()))
    {
        if ty.index() >= type_count {
            return Err(MirValidationError::InvalidType {
                function: function.id,
                ty,
            });
        }
    }

    let mut defined = BTreeSet::new();
    let mut used = Vec::new();
    for (index, block) in function.blocks.iter().enumerate() {
        if block.id.index() != index {
            return Err(MirValidationError::BlockIdMismatch {
                function: function.id,
                expected: index,
                actual: block.id.index(),
            });
        }
        for instruction in &block.instructions {
            verify_instruction(
                function,
                instruction,
                &mut defined,
                &mut used,
                function_count,
                external_import_count,
            )?;
        }
        verify_terminator(function, block.terminator(), &mut used)?;
    }
    for value in used {
        if value.index() >= function.value_count as usize || !defined.contains(&value) {
            return Err(MirValidationError::UndefinedValue {
                function: function.id,
                value,
            });
        }
    }
    Ok(())
}

fn verify_instruction(
    function: &MirFunction,
    instruction: &MirInstruction,
    defined: &mut BTreeSet<ValueId>,
    used: &mut Vec<ValueId>,
    function_count: usize,
    external_import_count: usize,
) -> Result<(), MirValidationError> {
    let define = |value: ValueId, defined: &mut BTreeSet<ValueId>| {
        if value.index() >= function.value_count as usize || !defined.insert(value) {
            Err(MirValidationError::InvalidValueDefinition {
                function: function.id,
                value,
            })
        } else {
            Ok(())
        }
    };
    let check_place = |place: PlaceId| {
        if place.index() >= function.place_count as usize {
            Err(MirValidationError::InvalidPlace {
                function: function.id,
                place,
            })
        } else {
            Ok(())
        }
    };
    match instruction {
        MirInstruction::LoadLiteral { destination, .. } => define(*destination, defined),
        MirInstruction::ReadPlace { destination, place } => {
            check_place(*place)?;
            define(*destination, defined)
        }
        MirInstruction::WritePlace { place, value } => {
            check_place(*place)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::Binary {
            destination,
            left,
            right,
            ..
        } => {
            define(*destination, defined)?;
            used.push(*left);
            used.push(*right);
            Ok(())
        }
        MirInstruction::Call {
            destination,
            target,
            arguments,
        } => {
            define(*destination, defined)?;
            match target {
                MirCallTarget::Function(target) if target.index() < function_count => {}
                MirCallTarget::Function(target) => {
                    return Err(MirValidationError::InvalidFunctionTarget {
                        function: function.id,
                        target: *target,
                    });
                }
                MirCallTarget::External(target) if target.index() < external_import_count => {}
                MirCallTarget::External(target) => {
                    return Err(MirValidationError::InvalidExternalTarget {
                        function: function.id,
                        target: *target,
                    });
                }
            }
            used.extend(arguments.iter().copied());
            Ok(())
        }
        MirInstruction::Discard { value } => {
            used.push(*value);
            Ok(())
        }
    }
}

fn verify_terminator(
    function: &MirFunction,
    terminator: &MirTerminator,
    used: &mut Vec<ValueId>,
) -> Result<(), MirValidationError> {
    let check_target = |target: BlockId| {
        if target.index() >= function.blocks.len() {
            Err(MirValidationError::InvalidBlockTarget {
                function: function.id,
                target,
            })
        } else {
            Ok(())
        }
    };
    match terminator {
        MirTerminator::Return(value) => {
            if let Some(value) = value {
                used.push(*value);
            }
        }
        MirTerminator::Jump(target) => check_target(*target)?,
        MirTerminator::Branch {
            condition,
            then_target,
            else_target,
        } => {
            used.push(*condition);
            check_target(*then_target)?;
            check_target(*else_target)?;
        }
        MirTerminator::Unreachable => {}
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirValidationError {
    FunctionDebugCount {
        functions: usize,
        debug: usize,
    },
    FunctionIdMismatch {
        expected: usize,
        actual: usize,
    },
    ExternalImportIdMismatch {
        expected: usize,
        actual: usize,
    },
    EmptyFunction {
        function: FunctionId,
    },
    BlockIdMismatch {
        function: FunctionId,
        expected: usize,
        actual: usize,
    },
    InvalidBlockTarget {
        function: FunctionId,
        target: BlockId,
    },
    InvalidPlace {
        function: FunctionId,
        place: PlaceId,
    },
    InvalidType {
        function: FunctionId,
        ty: TypeId,
    },
    InvalidFunctionTarget {
        function: FunctionId,
        target: FunctionId,
    },
    InvalidExternalTarget {
        function: FunctionId,
        target: ExternalSymbolId,
    },
    InvalidValueDefinition {
        function: FunctionId,
        value: ValueId,
    },
    UndefinedValue {
        function: FunctionId,
        value: ValueId,
    },
}

impl fmt::Display for MirValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid RSScript MIR: {self:?}")
    }
}

impl Error for MirValidationError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn debug() -> Vec<MirFunctionDebug> {
        vec![MirFunctionDebug::new("main", vec!["value".into()])]
    }

    fn signature() -> MirFunctionSignature {
        MirFunctionSignature::new(Vec::new(), TypeId::new(0), false)
    }

    #[test]
    fn accepts_a_typed_branching_function() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            1,
            3,
            vec![
                BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Bool(true),
                    }],
                    MirTerminator::Branch {
                        condition: ValueId::new(0),
                        then_target: BlockId::new(1),
                        else_target: BlockId::new(2),
                    },
                ),
                BasicBlock::new(
                    BlockId::new(1),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(1),
                        value: MirLiteral::Int(1),
                    }],
                    MirTerminator::Return(Some(ValueId::new(1))),
                ),
                BasicBlock::new(
                    BlockId::new(2),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(2),
                        value: MirLiteral::Int(0),
                    }],
                    MirTerminator::Return(Some(ValueId::new(2))),
                ),
            ],
        );
        let module = MirModule::new(
            vec![WireType::Int {
                bits: 64,
                signed: true,
            }],
            vec![function],
            debug(),
            Vec::new(),
        )
        .unwrap();
        assert_eq!(module.functions().len(), 1);
        assert_eq!(
            module.ty(TypeId::new(0)),
            Some(&WireType::Int {
                bits: 64,
                signed: true
            })
        );
    }

    #[test]
    fn rejects_undefined_values_and_targets() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Branch {
                    condition: ValueId::new(0),
                    then_target: BlockId::new(1),
                    else_target: BlockId::new(0),
                },
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::InvalidBlockTarget { .. })
        ));
    }

    #[test]
    fn rejects_function_signatures_that_reference_unknown_types() {
        let function = MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::new(Vec::new(), TypeId::new(1), false),
            0,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::InvalidType { .. })
        ));
    }

    #[test]
    fn rejects_calls_to_unknown_function_ids() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: ValueId::new(0),
                    target: MirCallTarget::Function(FunctionId::new(1)),
                    arguments: Vec::new(),
                }],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::InvalidFunctionTarget { .. })
        ));
    }
}
