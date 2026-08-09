#![forbid(unsafe_code)]

//! Typed, owned, control-flow MIR shared by RSScript executable backends.
//!
//! MIR deliberately has no dependency on syntax, HIR, compiler orchestration,
//! Providers, or a runtime. Human-readable names are retained only in debug
//! tables; instructions use typed local identities.

use std::collections::{BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;

use rsscript_abi_model::{ExternalSymbol, FunctionSignature, WireType};

#[cfg(feature = "conformance")]
pub mod conformance;

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
    /// A checked `read` borrow at a call boundary. This is intentionally
    /// distinct from an ordinary local read so later retain/escape validation
    /// has a concrete operation to inspect.
    BorrowRead {
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
        arguments: Vec<MirCallArgument>,
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

/// Resolved call argument mode. Borrow and move operations name a local place
/// directly so they remain visible to the verifier and cannot be accidentally
/// erased into an ordinary copied value by a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirCallArgument {
    Value(ValueId),
    BorrowRead(PlaceId),
    BorrowMut(PlaceId),
    Take(PlaceId),
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
                &self.functions,
                &self.external_imports,
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
    functions: &[MirFunction],
    external_imports: &[MirExternalImport],
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
        let mut block_moved_places = BTreeSet::new();
        for instruction in &block.instructions {
            verify_instruction(
                function,
                instruction,
                &mut defined,
                &mut used,
                &mut block_moved_places,
                functions,
                external_imports,
            )?;
        }
        verify_terminator(function, block.terminator(), &mut used)?;
    }
    verify_move_dataflow(function)?;
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

/// A place is considered moved at a join when any reachable predecessor moves
/// it. This is deliberately conservative: a later read must be valid on every
/// control-flow path. Assigning the place reinitializes it on that path.
fn verify_move_dataflow(function: &MirFunction) -> Result<(), MirValidationError> {
    let mut entries = vec![BTreeSet::new(); function.blocks.len()];
    let mut queued = vec![false; function.blocks.len()];
    let mut visited = vec![false; function.blocks.len()];
    let mut worklist = VecDeque::from([BlockId::new(0)]);
    queued[0] = true;

    while let Some(block_id) = worklist.pop_front() {
        queued[block_id.index()] = false;
        visited[block_id.index()] = true;
        let block = &function.blocks[block_id.index()];
        let mut moved_places = entries[block_id.index()].clone();
        for instruction in &block.instructions {
            transfer_move_state(function, instruction, &mut moved_places)?;
        }
        for successor in successors(block.terminator()) {
            let entry = &mut entries[successor.index()];
            let before = entry.len();
            entry.extend(moved_places.iter().copied());
            if (!visited[successor.index()] || entry.len() != before) && !queued[successor.index()]
            {
                worklist.push_back(successor);
                queued[successor.index()] = true;
            }
        }
    }
    Ok(())
}

fn successors(terminator: &MirTerminator) -> impl Iterator<Item = BlockId> {
    let mut successors = [None; 2];
    match terminator {
        MirTerminator::Jump(target) => successors[0] = Some(*target),
        MirTerminator::Branch {
            then_target,
            else_target,
            ..
        } => {
            successors[0] = Some(*then_target);
            successors[1] = Some(*else_target);
        }
        MirTerminator::Return(_) | MirTerminator::Unreachable => {}
    }
    successors.into_iter().flatten()
}

fn transfer_move_state(
    function: &MirFunction,
    instruction: &MirInstruction,
    moved_places: &mut BTreeSet<PlaceId>,
) -> Result<(), MirValidationError> {
    let check_live = |place: PlaceId, moved_places: &BTreeSet<PlaceId>| {
        if moved_places.contains(&place) {
            Err(MirValidationError::UseAfterMove {
                function: function.id,
                place,
            })
        } else {
            Ok(())
        }
    };
    match instruction {
        MirInstruction::ReadPlace { place, .. } | MirInstruction::BorrowRead { place, .. } => {
            check_live(*place, moved_places)
        }
        MirInstruction::WritePlace { place, .. } => {
            moved_places.remove(place);
            Ok(())
        }
        MirInstruction::Call { arguments, .. } => {
            for argument in arguments {
                match argument {
                    MirCallArgument::Value(_) => {}
                    MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) => {
                        check_live(*place, moved_places)?;
                    }
                    MirCallArgument::Take(place) => {
                        check_live(*place, moved_places)?;
                        moved_places.insert(*place);
                    }
                }
            }
            Ok(())
        }
        MirInstruction::LoadLiteral { .. }
        | MirInstruction::Binary { .. }
        | MirInstruction::Discard { .. } => Ok(()),
    }
}

fn verify_instruction(
    function: &MirFunction,
    instruction: &MirInstruction,
    defined: &mut BTreeSet<ValueId>,
    used: &mut Vec<ValueId>,
    moved_places: &mut BTreeSet<PlaceId>,
    functions: &[MirFunction],
    external_imports: &[MirExternalImport],
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
    let check_live_place = |place: PlaceId, moved_places: &BTreeSet<PlaceId>| {
        check_place(place)?;
        if moved_places.contains(&place) {
            Err(MirValidationError::UseAfterMove {
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
            check_live_place(*place, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::BorrowRead { destination, place } => {
            check_live_place(*place, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::WritePlace { place, value } => {
            check_place(*place)?;
            moved_places.remove(place);
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
            let expected_arguments = match target {
                MirCallTarget::Function(target) if target.index() < functions.len() => {
                    functions[target.index()].signature.parameter_types().len()
                }
                MirCallTarget::Function(target) => {
                    return Err(MirValidationError::InvalidFunctionTarget {
                        function: function.id,
                        target: *target,
                    });
                }
                MirCallTarget::External(target) if target.index() < external_imports.len() => {
                    external_imports[target.index()].signature.parameters.len()
                }
                MirCallTarget::External(target) => {
                    return Err(MirValidationError::InvalidExternalTarget {
                        function: function.id,
                        target: *target,
                    });
                }
            };
            if arguments.len() != expected_arguments {
                return Err(MirValidationError::CallArityMismatch {
                    function: function.id,
                    expected: expected_arguments,
                    actual: arguments.len(),
                });
            }
            for argument in arguments {
                match argument {
                    MirCallArgument::Value(value) => used.push(*value),
                    MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) => {
                        check_live_place(*place, moved_places)?;
                    }
                    MirCallArgument::Take(place) => {
                        check_live_place(*place, moved_places)?;
                        moved_places.insert(*place);
                    }
                }
            }
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
    CallArityMismatch {
        function: FunctionId,
        expected: usize,
        actual: usize,
    },
    UseAfterMove {
        function: FunctionId,
        place: PlaceId,
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

    #[test]
    fn rejects_direct_calls_with_the_wrong_arity() {
        let callee = MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::new(vec![TypeId::new(0)], TypeId::new(0), false),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        let caller = MirFunction::new(
            FunctionId::new(1),
            signature(),
            0,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: ValueId::new(0),
                    target: MirCallTarget::Function(FunctionId::new(0)),
                    arguments: Vec::new(),
                }],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        let debug = vec![
            MirFunctionDebug::new("callee", vec!["input".into()]),
            MirFunctionDebug::new("caller", Vec::new()),
        ];
        assert!(matches!(
            MirModule::new(
                vec![WireType::Unit],
                vec![callee, caller],
                debug,
                Vec::new()
            ),
            Err(MirValidationError::CallArityMismatch {
                expected: 1,
                actual: 0,
                ..
            })
        ));
    }

    #[test]
    fn rejects_reading_a_place_after_it_is_taken() {
        let callee = MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::new(vec![TypeId::new(0)], TypeId::new(0), false),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        let caller = MirFunction::new(
            FunctionId::new(1),
            signature(),
            1,
            2,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::Call {
                        destination: ValueId::new(0),
                        target: MirCallTarget::Function(FunctionId::new(0)),
                        arguments: vec![MirCallArgument::Take(PlaceId::new(0))],
                    },
                    MirInstruction::ReadPlace {
                        destination: ValueId::new(1),
                        place: PlaceId::new(0),
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(1))),
            )],
        );
        let debug = vec![
            MirFunctionDebug::new("callee", vec!["value".into()]),
            MirFunctionDebug::new("caller", vec!["value".into()]),
        ];
        assert!(matches!(
            MirModule::new(
                vec![WireType::Unit],
                vec![callee, caller],
                debug,
                Vec::new()
            ),
            Err(MirValidationError::UseAfterMove { .. })
        ));
    }

    fn unit_taking_callee() -> MirFunction {
        MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::new(vec![TypeId::new(0)], TypeId::new(0), false),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        )
    }

    #[test]
    fn rejects_a_read_after_take_on_one_branch_at_a_join() {
        let caller = MirFunction::new(
            FunctionId::new(1),
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
                    vec![MirInstruction::Call {
                        destination: ValueId::new(1),
                        target: MirCallTarget::Function(FunctionId::new(0)),
                        arguments: vec![MirCallArgument::Take(PlaceId::new(0))],
                    }],
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(2),
                    Vec::new(),
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(3),
                    vec![MirInstruction::ReadPlace {
                        destination: ValueId::new(2),
                        place: PlaceId::new(0),
                    }],
                    MirTerminator::Return(Some(ValueId::new(2))),
                ),
            ],
        );
        let debug = vec![
            MirFunctionDebug::new("callee", vec!["value".into()]),
            MirFunctionDebug::new("caller", vec!["value".into()]),
        ];
        assert!(matches!(
            MirModule::new(
                vec![WireType::Unit],
                vec![unit_taking_callee(), caller],
                debug,
                Vec::new()
            ),
            Err(MirValidationError::UseAfterMove { .. })
        ));
    }

    #[test]
    fn permits_reinitialization_after_a_branch_local_take() {
        let caller = MirFunction::new(
            FunctionId::new(1),
            signature(),
            1,
            4,
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
                    vec![MirInstruction::Call {
                        destination: ValueId::new(1),
                        target: MirCallTarget::Function(FunctionId::new(0)),
                        arguments: vec![MirCallArgument::Take(PlaceId::new(0))],
                    }],
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(2),
                    Vec::new(),
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(3),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(2),
                            value: MirLiteral::Int(42),
                        },
                        MirInstruction::WritePlace {
                            place: PlaceId::new(0),
                            value: ValueId::new(2),
                        },
                        MirInstruction::ReadPlace {
                            destination: ValueId::new(3),
                            place: PlaceId::new(0),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(3))),
                ),
            ],
        );
        let debug = vec![
            MirFunctionDebug::new("callee", vec!["value".into()]),
            MirFunctionDebug::new("caller", vec!["value".into()]),
        ];
        MirModule::new(
            vec![WireType::Unit],
            vec![unit_taking_callee(), caller],
            debug,
            Vec::new(),
        )
        .expect("write reinitializes a place on every path after the join");
    }
}
