#![forbid(unsafe_code)]

//! Typed, owned, control-flow MIR shared by RSScript executable backends.
//!
//! MIR deliberately has no dependency on syntax, HIR, compiler orchestration,
//! Providers, or a runtime. Human-readable names are retained only in debug
//! tables; instructions use typed local identities.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
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
mir_id!(TaskId);
mir_id!(TaskGroupId);

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
    /// Construct an owned list from already-defined values. Element identity is
    /// carried by the surrounding typed MIR table; the instruction itself has
    /// no source spelling or unresolved constructor name.
    MakeList {
        destination: ValueId,
        items: Vec<ValueId>,
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
    /// Move a local value out of its place. Unlike a call argument this keeps
    /// standalone `take value` expressions visible to backends and CFG checks.
    TakePlace {
        destination: ValueId,
        place: PlaceId,
    },
    /// A checked escape/retention boundary. The place stays live, but a
    /// backend can no longer erase the fact that its value may outlive a call.
    Retain {
        place: PlaceId,
    },
    /// Explicitly end a local value's ownership. Later reads must fail until a
    /// write reinitializes the place, including across CFG joins.
    Drop {
        place: PlaceId,
    },
    /// Begin a runtime-owned resource lifetime in `place`. The resource type
    /// names a canonical `WireType::Resource` entry rather than a string.
    AcquireResource {
        place: PlaceId,
        resource_type: ResourceTypeId,
        source: ValueId,
    },
    /// End a resource lifetime explicitly. Every reachable return edge must
    /// have released all acquired resources.
    ReleaseResource {
        place: PlaceId,
    },
    /// Start an async internal function under its lexical task group. The
    /// task must be awaited, cancelled, or joined before every return path.
    Spawn {
        task: TaskId,
        group: TaskGroupId,
        target: FunctionId,
        arguments: Vec<MirCallArgument>,
    },
    /// Await one child task and make its result available to subsequent MIR.
    Await {
        destination: ValueId,
        task: TaskId,
    },
    /// Cancel one child task. Cancellation is a lifecycle transition, not a
    /// best-effort backend hint.
    Cancel {
        task: TaskId,
    },
    /// Close all still-live tasks in a lexical task group.
    Join {
        group: TaskGroupId,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirCallArgumentMode {
    Value,
    Read,
    Mut,
    Take,
}

impl MirCallArgument {
    fn mode(self) -> MirCallArgumentMode {
        match self {
            Self::Value(_) => MirCallArgumentMode::Value,
            Self::BorrowRead(_) => MirCallArgumentMode::Read,
            Self::BorrowMut(_) => MirCallArgumentMode::Mut,
            Self::Take(_) => MirCallArgumentMode::Take,
        }
    }
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
    parameter_modes: Vec<MirParameterMode>,
    result: TypeId,
    asynchronous: bool,
}

impl MirFunctionSignature {
    pub fn new(parameter_types: Vec<TypeId>, result: TypeId, asynchronous: bool) -> Self {
        let parameter_modes = vec![MirParameterMode::Read; parameter_types.len()];
        Self::with_modes(parameter_types, parameter_modes, result, asynchronous)
    }

    pub fn with_modes(
        parameter_types: Vec<TypeId>,
        parameter_modes: Vec<MirParameterMode>,
        result: TypeId,
        asynchronous: bool,
    ) -> Self {
        Self {
            parameter_types,
            parameter_modes,
            result,
            asynchronous,
        }
    }

    pub fn parameter_types(&self) -> &[TypeId] {
        &self.parameter_types
    }

    pub fn parameter_modes(&self) -> &[MirParameterMode] {
        &self.parameter_modes
    }

    pub fn result(&self) -> TypeId {
        self.result
    }

    pub fn is_async(&self) -> bool {
        self.asynchronous
    }
}

/// Required ownership mode for a direct function parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MirParameterMode {
    Read,
    Mut,
    Take,
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
            verify_resource_types(function, &self.types)?;
            verify_resource_lifetimes(function)?;
            verify_task_lifetimes(function)?;
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

fn verify_resource_types(
    function: &MirFunction,
    types: &[WireType],
) -> Result<(), MirValidationError> {
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let MirInstruction::AcquireResource { resource_type, .. } = instruction {
                match types.get(resource_type.index()) {
                    Some(WireType::Resource { .. }) => {}
                    _ => {
                        return Err(MirValidationError::InvalidResourceType {
                            function: function.id,
                            resource_type: *resource_type,
                        });
                    }
                }
            }
        }
    }
    Ok(())
}

/// Track resources independently from ordinary move state: an acquired
/// resource must be released on every reachable return path. Joining paths is
/// conservative (a resource live on any predecessor is live at the join).
fn verify_resource_lifetimes(function: &MirFunction) -> Result<(), MirValidationError> {
    let mut entries = vec![BTreeSet::new(); function.blocks.len()];
    let mut queued = vec![false; function.blocks.len()];
    let mut visited = vec![false; function.blocks.len()];
    let mut worklist = VecDeque::from([BlockId::new(0)]);
    queued[0] = true;
    while let Some(block_id) = worklist.pop_front() {
        queued[block_id.index()] = false;
        visited[block_id.index()] = true;
        let block = &function.blocks[block_id.index()];
        let mut live = entries[block_id.index()].clone();
        for instruction in block.instructions() {
            match instruction {
                MirInstruction::AcquireResource { place, .. } if !live.insert(*place) => {
                    return Err(MirValidationError::ResourceAlreadyLive {
                        function: function.id,
                        place: *place,
                    });
                }
                MirInstruction::ReleaseResource { place } if !live.remove(place) => {
                    return Err(MirValidationError::ResourceNotLive {
                        function: function.id,
                        place: *place,
                    });
                }
                _ => {}
            }
        }
        if matches!(block.terminator(), MirTerminator::Return(_)) && !live.is_empty() {
            return Err(MirValidationError::ResourceLeak {
                function: function.id,
                place: *live.iter().next().expect("non-empty resource set"),
            });
        }
        for successor in successors(block.terminator()) {
            let entry = &mut entries[successor.index()];
            let before = entry.len();
            entry.extend(live.iter().copied());
            if (!visited[successor.index()] || entry.len() != before) && !queued[successor.index()]
            {
                worklist.push_back(successor);
                queued[successor.index()] = true;
            }
        }
    }
    Ok(())
}

/// Child tasks are lexically owned. A task cannot silently escape a return
/// edge: it must have been awaited, cancelled, or joined with its task group.
fn verify_task_lifetimes(function: &MirFunction) -> Result<(), MirValidationError> {
    let mut spawn_sites = BTreeSet::new();
    for block in function.blocks() {
        for instruction in block.instructions() {
            if let MirInstruction::Spawn { task, .. } = instruction
                && !spawn_sites.insert(*task)
            {
                return Err(MirValidationError::DuplicateTaskId {
                    function: function.id,
                    task: *task,
                });
            }
        }
    }

    let mut entries = vec![BTreeMap::<TaskId, TaskGroupId>::new(); function.blocks.len()];
    let mut queued = vec![false; function.blocks.len()];
    let mut visited = vec![false; function.blocks.len()];
    let mut worklist = VecDeque::from([BlockId::new(0)]);
    queued[0] = true;
    while let Some(block_id) = worklist.pop_front() {
        queued[block_id.index()] = false;
        visited[block_id.index()] = true;
        let block = &function.blocks[block_id.index()];
        let mut live = entries[block_id.index()].clone();
        for instruction in block.instructions() {
            match instruction {
                MirInstruction::Spawn { task, group, .. }
                    if live.insert(*task, *group).is_some() =>
                {
                    return Err(MirValidationError::TaskAlreadyLive {
                        function: function.id,
                        task: *task,
                    });
                }
                MirInstruction::Await { task, .. } | MirInstruction::Cancel { task }
                    if live.remove(task).is_none() =>
                {
                    return Err(MirValidationError::TaskNotLive {
                        function: function.id,
                        task: *task,
                    });
                }
                MirInstruction::Join { group } => live.retain(|_, owner| owner != group),
                _ => {}
            }
        }
        if matches!(block.terminator(), MirTerminator::Return(_))
            && let Some((task, _)) = live.iter().next()
        {
            return Err(MirValidationError::TaskLeak {
                function: function.id,
                task: *task,
            });
        }
        for successor in successors(block.terminator()) {
            let entry = &mut entries[successor.index()];
            let mut changed = false;
            for (task, group) in &live {
                match entry.get(task) {
                    Some(existing) if existing != group => {
                        return Err(MirValidationError::TaskGroupMismatch {
                            function: function.id,
                            task: *task,
                        });
                    }
                    Some(_) => {}
                    None => {
                        entry.insert(*task, *group);
                        changed = true;
                    }
                }
            }
            if (!visited[successor.index()] || changed) && !queued[successor.index()] {
                worklist.push_back(successor);
                queued[successor.index()] = true;
            }
        }
    }
    Ok(())
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
    if function.signature.parameter_types().len() != function.signature.parameter_modes().len() {
        return Err(MirValidationError::FunctionParameterModeCount {
            function: function.id,
            types: function.signature.parameter_types().len(),
            modes: function.signature.parameter_modes().len(),
        });
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
    verify_value_dominance(function)?;
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

/// Every value use must be reached by a definition on every control-flow path.
/// MIR has no phi instruction yet, so a value defined in only one branch cannot
/// be consumed after that branch joins. This catches a class of malformed CFGs
/// that a whole-function "defined somewhere" set cannot distinguish.
fn verify_value_dominance(function: &MirFunction) -> Result<(), MirValidationError> {
    let mut entries = vec![None::<BTreeSet<ValueId>>; function.blocks.len()];
    entries[0] = Some(BTreeSet::new());
    let mut changed = true;
    while changed {
        changed = false;
        for block in &function.blocks {
            let Some(entry) = entries[block.id.index()].clone() else {
                continue;
            };
            let mut exit = entry;
            for instruction in &block.instructions {
                if let Some(destination) = instruction_definition(instruction) {
                    exit.insert(destination);
                }
            }
            for successor in successors(block.terminator()) {
                let slot = &mut entries[successor.index()];
                let merged = match slot {
                    Some(existing) => existing.intersection(&exit).copied().collect(),
                    None => exit.clone(),
                };
                if slot.as_ref() != Some(&merged) {
                    *slot = Some(merged);
                    changed = true;
                }
            }
        }
    }

    for block in &function.blocks {
        let Some(mut defined) = entries[block.id.index()].clone() else {
            continue;
        };
        for instruction in &block.instructions {
            for value in instruction_uses(instruction) {
                if !defined.contains(&value) {
                    return Err(MirValidationError::ValueDoesNotDominate {
                        function: function.id,
                        block: block.id,
                        value,
                    });
                }
            }
            if let Some(destination) = instruction_definition(instruction) {
                defined.insert(destination);
            }
        }
        for value in terminator_uses(block.terminator()) {
            if !defined.contains(&value) {
                return Err(MirValidationError::ValueDoesNotDominate {
                    function: function.id,
                    block: block.id,
                    value,
                });
            }
        }
    }
    Ok(())
}

fn instruction_definition(instruction: &MirInstruction) -> Option<ValueId> {
    match instruction {
        MirInstruction::LoadLiteral { destination, .. }
        | MirInstruction::MakeList { destination, .. }
        | MirInstruction::ReadPlace { destination, .. }
        | MirInstruction::BorrowRead { destination, .. }
        | MirInstruction::TakePlace { destination, .. }
        | MirInstruction::Binary { destination, .. }
        | MirInstruction::Call { destination, .. }
        | MirInstruction::Await { destination, .. } => Some(*destination),
        MirInstruction::WritePlace { .. }
        | MirInstruction::Retain { .. }
        | MirInstruction::Drop { .. }
        | MirInstruction::AcquireResource { .. }
        | MirInstruction::ReleaseResource { .. }
        | MirInstruction::Spawn { .. }
        | MirInstruction::Cancel { .. }
        | MirInstruction::Join { .. }
        | MirInstruction::Discard { .. } => None,
    }
}

fn instruction_uses(instruction: &MirInstruction) -> Vec<ValueId> {
    match instruction {
        MirInstruction::WritePlace { value, .. } | MirInstruction::Discard { value } => {
            vec![*value]
        }
        MirInstruction::MakeList { items, .. } => items.clone(),
        MirInstruction::AcquireResource { source, .. } => vec![*source],
        MirInstruction::Binary { left, right, .. } => vec![*left, *right],
        MirInstruction::Call { arguments, .. } => arguments
            .iter()
            .filter_map(|argument| match argument {
                MirCallArgument::Value(value) => Some(*value),
                MirCallArgument::BorrowRead(_)
                | MirCallArgument::BorrowMut(_)
                | MirCallArgument::Take(_) => None,
            })
            .collect(),
        MirInstruction::Spawn { arguments, .. } => arguments
            .iter()
            .filter_map(|argument| match argument {
                MirCallArgument::Value(value) => Some(*value),
                MirCallArgument::BorrowRead(_)
                | MirCallArgument::BorrowMut(_)
                | MirCallArgument::Take(_) => None,
            })
            .collect(),
        MirInstruction::LoadLiteral { .. }
        | MirInstruction::ReadPlace { .. }
        | MirInstruction::BorrowRead { .. }
        | MirInstruction::TakePlace { .. }
        | MirInstruction::Retain { .. }
        | MirInstruction::Drop { .. }
        | MirInstruction::ReleaseResource { .. }
        | MirInstruction::Await { .. }
        | MirInstruction::Cancel { .. }
        | MirInstruction::Join { .. } => Vec::new(),
    }
}

fn terminator_uses(terminator: &MirTerminator) -> Vec<ValueId> {
    match terminator {
        MirTerminator::Return(Some(value)) => vec![*value],
        MirTerminator::Branch { condition, .. } => vec![*condition],
        MirTerminator::Return(None) | MirTerminator::Jump(_) | MirTerminator::Unreachable => {
            Vec::new()
        }
    }
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
        MirInstruction::TakePlace { place, .. } => {
            check_live(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
        }
        MirInstruction::Retain { place } => check_live(*place, moved_places),
        MirInstruction::Drop { place } => {
            check_live(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
        }
        MirInstruction::AcquireResource { place, .. } => {
            moved_places.remove(place);
            Ok(())
        }
        MirInstruction::ReleaseResource { place } => {
            check_live(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
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
        MirInstruction::Spawn { arguments, .. } => {
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
        | MirInstruction::MakeList { .. }
        | MirInstruction::Binary { .. }
        | MirInstruction::Await { .. }
        | MirInstruction::Cancel { .. }
        | MirInstruction::Join { .. }
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
        MirInstruction::LoadLiteral { destination, .. }
        | MirInstruction::MakeList { destination, .. } => define(*destination, defined),
        MirInstruction::ReadPlace { destination, place } => {
            check_live_place(*place, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::BorrowRead { destination, place } => {
            check_live_place(*place, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::TakePlace { destination, place } => {
            check_live_place(*place, moved_places)?;
            moved_places.insert(*place);
            define(*destination, defined)
        }
        MirInstruction::Retain { place } => check_live_place(*place, moved_places),
        MirInstruction::Drop { place } => {
            check_live_place(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
        }
        MirInstruction::AcquireResource { place, source, .. } => {
            check_place(*place)?;
            moved_places.remove(place);
            used.push(*source);
            Ok(())
        }
        MirInstruction::ReleaseResource { place } => {
            check_live_place(*place, moved_places)?;
            moved_places.insert(*place);
            Ok(())
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
            let expected_modes = match target {
                MirCallTarget::Function(target) if target.index() < functions.len() => functions
                    [target.index()]
                .signature
                .parameter_modes()
                .to_vec(),
                MirCallTarget::Function(target) => {
                    return Err(MirValidationError::InvalidFunctionTarget {
                        function: function.id,
                        target: *target,
                    });
                }
                MirCallTarget::External(target) if target.index() < external_imports.len() => {
                    external_imports[target.index()]
                        .signature
                        .parameters
                        .iter()
                        .map(|parameter| match parameter.effect {
                            rsscript_abi_model::DataEffect::Read => MirParameterMode::Read,
                            rsscript_abi_model::DataEffect::Mut => MirParameterMode::Mut,
                            rsscript_abi_model::DataEffect::Take => MirParameterMode::Take,
                        })
                        .collect()
                }
                MirCallTarget::External(target) => {
                    return Err(MirValidationError::InvalidExternalTarget {
                        function: function.id,
                        target: *target,
                    });
                }
            };
            if arguments.len() != expected_modes.len() {
                return Err(MirValidationError::CallArityMismatch {
                    function: function.id,
                    expected: expected_modes.len(),
                    actual: arguments.len(),
                });
            }
            for (parameter, (argument, expected)) in
                arguments.iter().zip(expected_modes).enumerate()
            {
                let actual = argument.mode();
                if !call_argument_compatible(actual, expected) {
                    return Err(MirValidationError::CallArgumentModeMismatch {
                        function: function.id,
                        parameter,
                        expected,
                        actual,
                    });
                }
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
        MirInstruction::Spawn {
            target, arguments, ..
        } => {
            let Some(callee) = functions.get(target.index()) else {
                return Err(MirValidationError::InvalidFunctionTarget {
                    function: function.id,
                    target: *target,
                });
            };
            if !callee.signature.is_async() {
                return Err(MirValidationError::SpawnTargetNotAsync {
                    function: function.id,
                    target: *target,
                });
            }
            if arguments.len() != callee.signature.parameter_modes().len() {
                return Err(MirValidationError::CallArityMismatch {
                    function: function.id,
                    expected: callee.signature.parameter_modes().len(),
                    actual: arguments.len(),
                });
            }
            for (parameter, (argument, expected)) in arguments
                .iter()
                .zip(callee.signature.parameter_modes())
                .enumerate()
            {
                let actual = argument.mode();
                if !call_argument_compatible(actual, *expected) {
                    return Err(MirValidationError::CallArgumentModeMismatch {
                        function: function.id,
                        parameter,
                        expected: *expected,
                        actual,
                    });
                }
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
        MirInstruction::Await { destination, .. } => define(*destination, defined),
        MirInstruction::Cancel { .. } | MirInstruction::Join { .. } => Ok(()),
        MirInstruction::Discard { value } => {
            used.push(*value);
            Ok(())
        }
    }
}

fn call_argument_compatible(actual: MirCallArgumentMode, expected: MirParameterMode) -> bool {
    matches!(
        (actual, expected),
        (
            MirCallArgumentMode::Value | MirCallArgumentMode::Read,
            MirParameterMode::Read
        ) | (MirCallArgumentMode::Mut, MirParameterMode::Mut)
            | (MirCallArgumentMode::Take, MirParameterMode::Take)
    )
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
    FunctionParameterModeCount {
        function: FunctionId,
        types: usize,
        modes: usize,
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
    InvalidResourceType {
        function: FunctionId,
        resource_type: ResourceTypeId,
    },
    ResourceAlreadyLive {
        function: FunctionId,
        place: PlaceId,
    },
    ResourceNotLive {
        function: FunctionId,
        place: PlaceId,
    },
    ResourceLeak {
        function: FunctionId,
        place: PlaceId,
    },
    DuplicateTaskId {
        function: FunctionId,
        task: TaskId,
    },
    TaskAlreadyLive {
        function: FunctionId,
        task: TaskId,
    },
    TaskNotLive {
        function: FunctionId,
        task: TaskId,
    },
    TaskLeak {
        function: FunctionId,
        task: TaskId,
    },
    TaskGroupMismatch {
        function: FunctionId,
        task: TaskId,
    },
    SpawnTargetNotAsync {
        function: FunctionId,
        target: FunctionId,
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
    CallArgumentModeMismatch {
        function: FunctionId,
        parameter: usize,
        expected: MirParameterMode,
        actual: MirCallArgumentMode,
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
    ValueDoesNotDominate {
        function: FunctionId,
        block: BlockId,
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

    fn taking_signature() -> MirFunctionSignature {
        MirFunctionSignature::with_modes(
            vec![TypeId::new(0)],
            vec![MirParameterMode::Take],
            TypeId::new(0),
            false,
        )
    }

    #[test]
    fn resource_lifetimes_require_canonical_type_and_release_before_return() {
        let resource = WireType::Resource {
            name: "host.fs.File".into(),
        };
        let valid = MirModule::new(
            vec![WireType::Unit, resource.clone()],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                1,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Unit,
                        },
                        MirInstruction::AcquireResource {
                            place: PlaceId::new(0),
                            resource_type: ResourceTypeId::new(1),
                            source: ValueId::new(0),
                        },
                        MirInstruction::ReleaseResource {
                            place: PlaceId::new(0),
                        },
                    ],
                    MirTerminator::Return(None),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec!["file".into()])],
            vec![],
        );
        assert!(valid.is_ok());

        let leaked = MirModule::new(
            vec![WireType::Unit, resource],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                1,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Unit,
                        },
                        MirInstruction::AcquireResource {
                            place: PlaceId::new(0),
                            resource_type: ResourceTypeId::new(1),
                            source: ValueId::new(0),
                        },
                    ],
                    MirTerminator::Return(None),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec!["file".into()])],
            vec![],
        );
        assert!(matches!(
            leaked,
            Err(MirValidationError::ResourceLeak { .. })
        ));
    }

    #[test]
    fn structured_tasks_must_be_closed_before_return() {
        let worker = MirFunction::new(
            FunctionId::new(1),
            MirFunctionSignature::new(vec![], TypeId::new(0), true),
            1,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![],
                MirTerminator::Return(None),
            )],
        );
        let valid = MirModule::new(
            vec![WireType::Unit],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    signature(),
                    0,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::Spawn {
                                task: TaskId::new(0),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(1),
                                arguments: vec![],
                            },
                            MirInstruction::Await {
                                destination: ValueId::new(0),
                                task: TaskId::new(0),
                            },
                        ],
                        MirTerminator::Return(Some(ValueId::new(0))),
                    )],
                ),
                worker.clone(),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        );
        assert!(valid.is_ok());

        let leaked = MirModule::new(
            vec![WireType::Unit],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    signature(),
                    0,
                    0,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![MirInstruction::Spawn {
                            task: TaskId::new(0),
                            group: TaskGroupId::new(0),
                            target: FunctionId::new(1),
                            arguments: vec![],
                        }],
                        MirTerminator::Return(None),
                    )],
                ),
                worker,
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        );
        assert!(matches!(leaked, Err(MirValidationError::TaskLeak { .. })));
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
    fn rejects_value_defined_on_only_one_predecessor_of_a_join() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            2,
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
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(2),
                    Vec::new(),
                    MirTerminator::Jump(BlockId::new(3)),
                ),
                BasicBlock::new(
                    BlockId::new(3),
                    Vec::new(),
                    MirTerminator::Return(Some(ValueId::new(1))),
                ),
            ],
        );
        let error = MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new())
            .expect_err("join must reject a non-dominating value");
        assert!(matches!(
            error,
            MirValidationError::ValueDoesNotDominate { block, value, .. }
                if block == BlockId::new(3) && value == ValueId::new(1)
        ));
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
            taking_signature(),
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
    fn rejects_call_arguments_with_the_wrong_ownership_mode() {
        let callee = MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::with_modes(
                vec![TypeId::new(0)],
                vec![MirParameterMode::Mut],
                TypeId::new(0),
                false,
            ),
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
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: ValueId::new(0),
                    target: MirCallTarget::Function(FunctionId::new(0)),
                    arguments: vec![MirCallArgument::BorrowRead(PlaceId::new(0))],
                }],
                MirTerminator::Return(Some(ValueId::new(0))),
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
            Err(MirValidationError::CallArgumentModeMismatch {
                expected: MirParameterMode::Mut,
                actual: MirCallArgumentMode::Read,
                ..
            })
        ));
    }

    #[test]
    fn rejects_reading_a_place_after_it_is_taken() {
        let callee = MirFunction::new(
            FunctionId::new(0),
            taking_signature(),
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

    #[test]
    fn explicit_retain_keeps_a_place_live_but_drop_invalidates_it() {
        let retained = MirFunction::new(
            FunctionId::new(0),
            signature(),
            1,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::Retain {
                        place: PlaceId::new(0),
                    },
                    MirInstruction::ReadPlace {
                        destination: ValueId::new(0),
                        place: PlaceId::new(0),
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        assert!(MirModule::new(vec![WireType::Unit], vec![retained], debug(), Vec::new()).is_ok());

        let dropped = MirFunction::new(
            FunctionId::new(0),
            signature(),
            1,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::Drop {
                        place: PlaceId::new(0),
                    },
                    MirInstruction::ReadPlace {
                        destination: ValueId::new(0),
                        place: PlaceId::new(0),
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![dropped], debug(), Vec::new()),
            Err(MirValidationError::UseAfterMove { .. })
        ));
    }

    fn unit_taking_callee() -> MirFunction {
        MirFunction::new(
            FunctionId::new(0),
            taking_signature(),
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
