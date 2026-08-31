#![forbid(unsafe_code)]

//! Typed, owned, control-flow MIR shared by RSScript executable backends.
//!
//! MIR deliberately has no dependency on syntax, HIR, compiler orchestration,
//! Providers, or a runtime. Human-readable names are retained only in debug
//! tables; instructions use typed local identities.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::ops::Deref;

use rsscript_abi_model::{ExternalSymbol, FunctionSignature, WireType};

// MIR verification pass lives in a child module (module-size split).
mod verify;
use verify::*;

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

/// Whether a deterministic core-library builtin depends solely on its explicit
/// arguments or on the current execution state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinDeterminism {
    Deterministic,
    ExecutionState,
}

/// Coarse static cost class for a builtin call. Detailed cost accounting stays
/// with the VM until a future registry schema can express input-size formulas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinCost {
    Constant,
    InputDependent,
}

/// Execution ownership of a catalog direct call. Provider calls are never
/// builtins: they are represented separately by [`MirCallTarget::External`]
/// and therefore cannot accidentally acquire a VM instruction identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinClass {
    VmPrimitive,
    DeterministicBuiltin,
}

/// Origin of the canonical signature carried by a builtin registry entry.
/// Public library calls use their `.rssi` declaration; implementation-only VM
/// primitives remain explicitly marked until their library family is moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinSignatureSource {
    Interface,
    Internal,
}

/// Versioned contract for a catalog-owned builtin. This is the shared source
/// of identity, ABI spelling, determinism and coarse cost metadata for MIR
/// validation and bytecode code generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinDescriptor {
    pub id: BuiltinId,
    pub namespace: &'static str,
    pub name: &'static str,
    pub vm_name: &'static str,
    pub signature: &'static str,
    pub signature_source: BuiltinSignatureSource,
    pub determinism: BuiltinDeterminism,
    pub cost: BuiltinCost,
    pub class: BuiltinClass,
}

include!(concat!(env!("OUT_DIR"), "/rss-mir-builtin-catalog.rs"));

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
    /// Construct an owned map from already-defined key/value pairs. Both sides
    /// are value identities so the verifier can require every pair member to
    /// dominate construction.
    MakeMap {
        destination: ValueId,
        entries: Vec<(ValueId, ValueId)>,
    },
    /// Construct a JSON object from data field names and already-defined
    /// values. Field names here are serialized JSON data, not unresolved
    /// language record identities.
    MakeObject {
        destination: ValueId,
        fields: Vec<(String, ValueId)>,
    },
    /// Construct a resolved struct/class value. The layout type is a canonical
    /// module-local `TypeId`; field labels are aggregate layout data in
    /// declaration order, never a source-level constructor target.
    MakeStruct {
        destination: ValueId,
        ty: TypeId,
        fields: Vec<(String, ValueId)>,
    },
    /// Construct a resolved sum variant. `ty` identifies the owning sum type;
    /// `variant` and field labels are verified layout data, not a callee name.
    MakeVariant {
        destination: ValueId,
        ty: TypeId,
        variant: String,
        fields: Vec<(String, ValueId)>,
    },
    /// Build a canonical `Result` variant without routing a language builtin
    /// name through a backend. `ok = true` is `Ok(value)` and `ok = false` is
    /// `Err(value)`.
    MakeResult {
        destination: ValueId,
        ok: bool,
        value: ValueId,
    },
    /// Project the payload from a `Result` arm after a matching
    /// `MatchResult` terminator. `ok = true` projects `Ok(value)`; `false`
    /// projects `Err(value)`.
    UnwrapResult {
        destination: ValueId,
        source: ValueId,
        ok: bool,
    },
    /// Construct a canonical `Option`. `Some(value)` carries a value and
    /// `None` carries no payload.
    MakeOption {
        destination: ValueId,
        value: Option<ValueId>,
    },
    /// Project a `Some(value)` payload after a matching `MatchOption` edge.
    UnwrapOption {
        destination: ValueId,
        source: ValueId,
    },
    /// Read an element from a resolved list value. The lowerer emits this only
    /// when checked type facts identify the base as `List<...>`.
    ListGet {
        destination: ValueId,
        list: ValueId,
        index: ValueId,
    },
    /// Append an owned list of values to a resolved mutable list place.
    ListAppend {
        destination: ValueId,
        list: PlaceId,
        values: ValueId,
    },
    /// Clear a resolved mutable list place.
    ListClear {
        destination: ValueId,
        list: PlaceId,
    },
    /// Pop the last value from a resolved mutable list place, returning an
    /// explicit `Option`.
    ListPop {
        destination: ValueId,
        list: PlaceId,
    },
    /// Push a value into a resolved mutable list place.
    ListPush {
        destination: ValueId,
        list: PlaceId,
        value: ValueId,
    },
    /// Remove an indexed value from a resolved mutable list place, returning
    /// an explicit `Option` when the index is out of range.
    ListRemoveAt {
        destination: ValueId,
        list: PlaceId,
        index: ValueId,
    },
    /// Replace an indexed value in a resolved mutable list place.
    ListSet {
        destination: ValueId,
        list: PlaceId,
        index: ValueId,
        value: ValueId,
    },
    /// Clear a resolved mutable hash-set place.
    SetClear {
        destination: ValueId,
        set: PlaceId,
    },
    /// Insert a value into a resolved mutable hash-set place, returning whether
    /// it was absent. The retained value and mutable place remain explicit.
    SetInsert {
        destination: ValueId,
        set: PlaceId,
        value: ValueId,
    },
    /// Remove a value from a resolved mutable hash-set place, returning whether
    /// it was present.
    SetRemove {
        destination: ValueId,
        set: PlaceId,
        value: ValueId,
    },
    /// Clear a resolved mutable deque place.
    DequeClear {
        destination: ValueId,
        deque: PlaceId,
    },
    /// Pop the back value from a resolved mutable deque place as an `Option`.
    DequePopBack {
        destination: ValueId,
        deque: PlaceId,
    },
    /// Pop the front value from a resolved mutable deque place as an `Option`.
    DequePopFront {
        destination: ValueId,
        deque: PlaceId,
    },
    /// Push a value at the back of a resolved mutable deque place.
    DequePushBack {
        destination: ValueId,
        deque: PlaceId,
        value: ValueId,
    },
    /// Push a value at the front of a resolved mutable deque place.
    DequePushFront {
        destination: ValueId,
        deque: PlaceId,
        value: ValueId,
    },
    /// Clear a resolved mutable ordered-map place.
    SortedMapClear {
        destination: ValueId,
        map: PlaceId,
    },
    /// Insert a key/value pair into a resolved mutable ordered-map place.
    SortedMapInsert {
        destination: ValueId,
        map: PlaceId,
        key: ValueId,
        value: ValueId,
    },
    /// Remove a key from a resolved mutable ordered-map place and return the
    /// removed value as an `Option`.
    SortedMapRemove {
        destination: ValueId,
        map: PlaceId,
        key: ValueId,
    },
    /// Clear a resolved mutable ordered-set place.
    SortedSetClear {
        destination: ValueId,
        set: PlaceId,
    },
    /// Insert a value into a resolved mutable ordered-set place.
    SortedSetInsert {
        destination: ValueId,
        set: PlaceId,
        value: ValueId,
    },
    /// Remove a value from a resolved mutable ordered-set place.
    SortedSetRemove {
        destination: ValueId,
        set: PlaceId,
        value: ValueId,
    },
    /// Clear a resolved mutable buffer place.
    BufferClear {
        destination: ValueId,
        buffer: PlaceId,
    },
    /// Append one string value to a resolved mutable string builder place.
    StringBuilderPush {
        destination: ValueId,
        builder: PlaceId,
        value: ValueId,
    },
    /// Consume a string builder value and return its completed string.
    StringBuilderFinish {
        destination: ValueId,
        builder: ValueId,
    },
    /// Read a value from a resolved mutable map. The result remains an
    /// `Option` so absence is explicit in the typed control-flow graph rather
    /// than hidden in a source-level `Map.get` spelling.
    MapGet {
        destination: ValueId,
        map: ValueId,
        key: ValueId,
    },
    /// Clear a resolved mutable map in place. The map is a `PlaceId` so the
    /// mutation contract is verifier-visible and cannot be reconstructed from
    /// a source-level `mut` qualifier by a backend.
    MapClear {
        destination: ValueId,
        map: PlaceId,
    },
    /// Insert a key/value pair into a resolved mutable map in place. The
    /// mutable map place and the independently evaluated key/value operands
    /// make the update and its ownership boundary explicit to verification and
    /// code generation.
    MapInsert {
        destination: ValueId,
        map: PlaceId,
        key: ValueId,
        value: ValueId,
    },
    /// Insert a key/value pair into a resolved mutable map and return the
    /// previous value as an `Option`. This remains separate from `MapInsert`
    /// because its result contract is observable in typed control flow.
    MapInsertOld {
        destination: ValueId,
        map: PlaceId,
        key: ValueId,
        value: ValueId,
    },
    /// Remove a key from a resolved mutable map and return the removed value
    /// as an `Option`.
    MapRemove {
        destination: ValueId,
        map: PlaceId,
        key: ValueId,
    },
    /// Read a checked field from an already-resolved aggregate value. The
    /// field spelling is data for the runtime object representation; it is not
    /// a source-level callee or type identity.
    GetField {
        destination: ValueId,
        base: ValueId,
        field: String,
    },
    /// Rebuild an aggregate with one field replaced. The update is fed back
    /// through its `base` value by lowering, so nested source assignments stay
    /// explicit and backend-independent. The operation has no result: it
    /// updates the owned base value in place, matching the VM's value-rebuild
    /// instruction without pretending that its `Unit` result is a struct.
    SetField {
        base: ValueId,
        field: String,
        value: ValueId,
    },
    /// Read the length of a resolved list value. This keeps lowered list
    /// iteration free of source-level iterator identity.
    ListLen {
        destination: ValueId,
        list: ValueId,
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
    /// Move an owned value into managed storage. This is distinct from an
    /// ordinary move: the VM preserves the managed-cell identity so later
    /// mutable aliases observe the same graph.
    Manage {
        destination: ValueId,
        source: ValueId,
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
    /// Wait for the first completed child from `tasks`. The instruction
    /// consumes every listed task: the winner's value is written to `value`
    /// and the runtime cancels/reaps the losing children before control reaches
    /// the arm dispatch CFG. `winner` is the zero-based index in `tasks`.
    Select {
        tasks: Vec<TaskId>,
        winner: ValueId,
        value: ValueId,
    },
    /// Unwrap an `Ok`/`Some` value, or short-circuit the current function with
    /// its `Err`/`None` after releasing the listed lexical resources. The
    /// cleanup edge is data in MIR rather than a backend-specific inference.
    TryResult {
        destination: ValueId,
        source: ValueId,
        cleanup: Vec<PlaceId>,
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
    /// Concatenate two immutable strings. This is a primitive operation rather
    /// than a source-level `String.concat` call, so backends never need to
    /// reconstruct a special intrinsic spelling from syntax.
    StringConcat {
        destination: ValueId,
        left: ValueId,
        right: ValueId,
    },
    Call {
        destination: ValueId,
        target: MirCallTarget,
        arguments: Vec<MirCallArgument>,
    },
    /// Materialize a first-class closure from a resolved synthetic function and
    /// explicit capture arguments. Capture ownership stays verifier-visible
    /// rather than implicit in a backend-specific environment.
    MakeClosure {
        destination: ValueId,
        function: FunctionId,
        captures: Vec<MirCallArgument>,
    },
    /// Invoke a first-class closure value. Its checked function ABI is carried
    /// as typed data because the concrete target is selected from the runtime
    /// value rather than a source spelling.
    CallClosure {
        destination: ValueId,
        closure: ValueId,
        parameter_types: Box<[TypeId]>,
        parameter_modes: Box<[MirParameterMode]>,
        arguments: Vec<MirCallArgument>,
    },
    Discard {
        value: ValueId,
    },
}

/// Fully resolved direct call target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirCallTarget {
    Function(FunctionId),
    /// Direct generic call with a complete semantic substitution in declaration
    /// order. The v1 executable still encodes `CallKnown`; these type IDs feed
    /// only the optional versioned facts used by bounded native instances.
    FunctionInstance {
        function: FunctionId,
        /// `(generic parameter type, concrete argument type)` pairs in source
        /// declaration order.
        type_substitutions: Box<[(TypeId, TypeId)]>,
    },
    /// Closed-world protocol dispatch. The receiver's runtime layout is matched
    /// against a canonical `TypeId`; the selected implementation is a resolved
    /// `FunctionId`. Neither source protocol nor method spellings reach MIR.
    Dynamic {
        dispatch: Box<[(TypeId, FunctionId)]>,
        parameter_modes: Box<[MirParameterMode]>,
    },
    /// Catalog-owned direct core-library call. The source namespace/name was
    /// resolved by semantic lowering; v1 bytecode spelling is an encoder-only
    /// compatibility projection through [`builtin_descriptor`].
    Builtin {
        id: BuiltinId,
        parameter_modes: Box<[MirParameterMode]>,
        /// Concrete type arguments required by a catalog-owned builtin.
        ///
        /// The values are module-local `TypeId`s rather than source spellings,
        /// so generic runtime contracts remain verifier-visible even while the
        /// v1 bytecode encoder still projects a small typed-intrinsic subset
        /// onto its legacy string operand.
        type_arguments: Box<[TypeId]>,
    },
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
    /// Branch on a resolved sum-variant tag. The expected tag is layout data
    /// from semantic HIR, not a source-level pattern node.
    MatchVariant {
        value: ValueId,
        expected: String,
        match_target: BlockId,
        else_target: BlockId,
    },
    /// Branch on the canonical `Result` tag without routing `Ok`/`Err`
    /// source names through a backend.
    MatchResult {
        value: ValueId,
        ok_target: BlockId,
        err_target: BlockId,
    },
    /// Branch on a canonical `Option` without representing `Some`/`None` as
    /// unresolved source-level constructor names.
    MatchOption {
        value: ValueId,
        some_target: BlockId,
        none_target: BlockId,
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
    source: Option<MirSourceLocation>,
    instruction_sources: Vec<MirInstructionSource>,
}

/// Source-only location retained beside executable MIR identities.
///
/// This is deliberately debug metadata rather than an instruction operand:
/// backends identify functions, blocks, values, and types through their typed
/// IDs. The location lets diagnostics and future source maps retain a stable
/// origin without reintroducing syntax or compiler dependencies into MIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirSourceLocation {
    file: String,
    line: usize,
    column: usize,
    length: usize,
}

impl MirSourceLocation {
    pub fn new(file: impl Into<String>, line: usize, column: usize, length: usize) -> Self {
        Self {
            file: file.into(),
            line,
            column,
            length,
        }
    }

    pub fn file(&self) -> &str {
        &self.file
    }

    pub fn line(&self) -> usize {
        self.line
    }

    pub fn column(&self) -> usize {
        self.column
    }

    pub fn length(&self) -> usize {
        self.length
    }
}

/// Source-only location for one instruction in a function's CFG.
///
/// This table deliberately lives beside executable MIR: instructions remain
/// typed-ID-only and backends can ignore source evidence entirely. The MIR
/// verifier nevertheless checks every entry so diagnostics cannot silently
/// point at a different function, block, or instruction after a lowering
/// change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirInstructionSource {
    block: BlockId,
    instruction_index: u32,
    source: MirSourceLocation,
}

impl MirInstructionSource {
    pub fn new(block: BlockId, instruction_index: u32, source: MirSourceLocation) -> Self {
        Self {
            block,
            instruction_index,
            source,
        }
    }

    pub fn block(&self) -> BlockId {
        self.block
    }

    pub fn instruction_index(&self) -> u32 {
        self.instruction_index
    }

    pub fn source(&self) -> &MirSourceLocation {
        &self.source
    }
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

/// Typed ownership contract for one synthetic closure-environment slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirClosureCapture {
    ty: TypeId,
    mode: MirParameterMode,
}

impl MirClosureCapture {
    pub fn new(ty: TypeId, mode: MirParameterMode) -> Self {
        Self { ty, mode }
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn mode(&self) -> MirParameterMode {
        self.mode
    }
}

impl MirFunctionDebug {
    pub fn new(name: impl Into<String>, places: Vec<String>) -> Self {
        Self {
            name: name.into(),
            places,
            source: None,
            instruction_sources: Vec::new(),
        }
    }

    /// Attach optional source-map evidence without changing executable MIR.
    pub fn with_source(
        name: impl Into<String>,
        places: Vec<String>,
        source: MirSourceLocation,
    ) -> Self {
        Self {
            name: name.into(),
            places,
            source: Some(source),
            instruction_sources: Vec::new(),
        }
    }

    /// Attach validated, source-only instruction mappings without changing
    /// executable identities or instruction operands.
    pub fn with_instruction_sources(
        mut self,
        instruction_sources: Vec<MirInstructionSource>,
    ) -> Self {
        self.instruction_sources = instruction_sources;
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn places(&self) -> &[String] {
        &self.places
    }

    pub fn source(&self) -> Option<&MirSourceLocation> {
        self.source.as_ref()
    }

    pub fn instruction_sources(&self) -> &[MirInstructionSource] {
        &self.instruction_sources
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirFunction {
    id: FunctionId,
    signature: MirFunctionSignature,
    captures: Vec<MirClosureCapture>,
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
            captures: Vec::new(),
            place_count,
            value_count,
            blocks,
        }
    }

    /// Construct a synthetic closure body. Capture slots precede ordinary
    /// parameter slots in the place table, matching the VM call ABI while
    /// keeping their type and ownership facts in MIR.
    pub fn with_captures(
        id: FunctionId,
        signature: MirFunctionSignature,
        captures: Vec<MirClosureCapture>,
        place_count: u32,
        value_count: u32,
        blocks: Vec<BasicBlock>,
    ) -> Self {
        Self {
            id,
            signature,
            captures,
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

    pub fn captures(&self) -> &[MirClosureCapture] {
        &self.captures
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

/// Runtime layout evidence for a named value type. The executable type table
/// owns the canonical `TypeId`; this side table carries only the ordered field
/// shape needed by type-directed builtins and the v1 VM value representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirTypeLayout {
    ty: TypeId,
    name: String,
    fields: Vec<(String, TypeId)>,
}

impl MirTypeLayout {
    pub fn new(ty: TypeId, name: impl Into<String>, fields: Vec<(String, TypeId)>) -> Self {
        Self {
            ty,
            name: name.into(),
            fields,
        }
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn fields(&self) -> &[(String, TypeId)] {
        &self.fields
    }
}

/// Runtime layout evidence for one named sum type.  The executable type table
/// owns the canonical [`TypeId`]; this side table preserves declaration-order
/// case and field shape for Artifact consumers that must materialize a typed
/// wire value without recovering identity from a runtime string value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirVariantLayout {
    ty: TypeId,
    name: String,
    variants: Vec<MirVariantCaseLayout>,
}

/// One declaration-order case in a [`MirVariantLayout`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirVariantCaseLayout {
    name: String,
    fields: Vec<(String, TypeId)>,
}

impl MirVariantLayout {
    pub fn new(ty: TypeId, name: impl Into<String>, variants: Vec<MirVariantCaseLayout>) -> Self {
        Self {
            ty,
            name: name.into(),
            variants,
        }
    }

    pub fn ty(&self) -> TypeId {
        self.ty
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn variants(&self) -> &[MirVariantCaseLayout] {
        &self.variants
    }
}

impl MirVariantCaseLayout {
    pub fn new(name: impl Into<String>, fields: Vec<(String, TypeId)>) -> Self {
        Self {
            name: name.into(),
            fields,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn fields(&self) -> &[(String, TypeId)] {
        &self.fields
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MirModule {
    types: Vec<WireType>,
    type_layouts: Vec<MirTypeLayout>,
    variant_layouts: Vec<MirVariantLayout>,
    functions: Vec<MirFunction>,
    function_debug: Vec<MirFunctionDebug>,
    external_imports: Vec<MirExternalImport>,
}

/// A MIR module that has passed the structural, ownership, resource, and task
/// lifetime verifier.
///
/// Backends consume this phase type rather than raw [`MirModule`] so their
/// public entry points cannot accidentally accept an unchecked intermediate
/// representation. `MirModule` keeps its validated constructor for lowering
/// and test construction; this wrapper additionally marks the explicit
/// verifier boundary used between lowering and code generation.
#[derive(Debug, Clone, PartialEq)]
pub struct VerifiedMir {
    module: MirModule,
}

impl VerifiedMir {
    /// Re-run MIR verification at a backend admission boundary.
    pub fn verify(module: MirModule) -> Result<Self, MirValidationError> {
        module.verify()?;
        Ok(Self { module })
    }

    pub fn module(&self) -> &MirModule {
        &self.module
    }

    /// Consume the verified wrapper after a backend has finished admission.
    /// The returned module has no mutable public fields, but callers crossing
    /// another trust boundary must verify it again before execution.
    pub fn into_module(self) -> MirModule {
        self.module
    }
}

impl Deref for VerifiedMir {
    type Target = MirModule;

    fn deref(&self) -> &Self::Target {
        self.module()
    }
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
            type_layouts: Vec::new(),
            variant_layouts: Vec::new(),
            functions,
            function_debug,
            external_imports,
        };
        module.verify()?;
        Ok(module)
    }

    /// Construct a module with explicit runtime type layouts. Lowerers use
    /// this when a typed builtin must inspect a named record at execution;
    /// ordinary scalar modules retain the smaller [`new`](Self::new) path.
    pub fn with_type_layouts(
        types: Vec<WireType>,
        type_layouts: Vec<MirTypeLayout>,
        functions: Vec<MirFunction>,
        function_debug: Vec<MirFunctionDebug>,
        external_imports: Vec<MirExternalImport>,
    ) -> Result<Self, MirValidationError> {
        let module = Self {
            types,
            type_layouts,
            variant_layouts: Vec::new(),
            functions,
            function_debug,
            external_imports,
        };
        module.verify()?;
        Ok(module)
    }

    /// Construct a module with explicit named record and sum layouts.  Both
    /// tables are part of the typed executable contract; backends must not
    /// infer a sum's cases from observed `MakeVariant` instructions.
    pub fn with_layouts(
        types: Vec<WireType>,
        type_layouts: Vec<MirTypeLayout>,
        variant_layouts: Vec<MirVariantLayout>,
        functions: Vec<MirFunction>,
        function_debug: Vec<MirFunctionDebug>,
        external_imports: Vec<MirExternalImport>,
    ) -> Result<Self, MirValidationError> {
        let module = Self {
            types,
            type_layouts,
            variant_layouts,
            functions,
            function_debug,
            external_imports,
        };
        module.verify()?;
        Ok(module)
    }

    /// Mark this validated module for a backend admission boundary. The
    /// verifier is deliberately run again so a caller cannot mistake
    /// construction-time validation for a later phase admission check.
    pub fn into_verified(self) -> Result<VerifiedMir, MirValidationError> {
        VerifiedMir::verify(self)
    }

    pub fn functions(&self) -> &[MirFunction] {
        &self.functions
    }

    pub fn types(&self) -> &[WireType] {
        &self.types
    }

    pub fn type_layouts(&self) -> &[MirTypeLayout] {
        &self.type_layouts
    }

    pub fn variant_layouts(&self) -> &[MirVariantLayout] {
        &self.variant_layouts
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
        verify_type_layouts(&self.types, &self.type_layouts)?;
        verify_variant_layouts(&self.types, &self.variant_layouts)?;
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
            verify_instruction_sources(function, &self.function_debug[index])?;
            verify_resource_types(function, &self.types)?;
            verify_record_types(function, &self.types)?;
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


#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirValidationError {
    FunctionDebugCount {
        functions: usize,
        debug: usize,
    },
    InvalidInstructionSourceBlock {
        function: FunctionId,
        block: BlockId,
    },
    InvalidInstructionSourceIndex {
        function: FunctionId,
        block: BlockId,
        instruction_index: u32,
    },
    DuplicateInstructionSource {
        function: FunctionId,
        block: BlockId,
        instruction_index: u32,
    },
    FunctionParameterModeCount {
        function: FunctionId,
        types: usize,
        modes: usize,
    },
    ClosureFrameTooSmall {
        function: FunctionId,
        required: usize,
        actual: usize,
    },
    InvalidClosureCaptureType {
        function: FunctionId,
        ty: TypeId,
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
    InvalidRecordType {
        function: FunctionId,
        ty: TypeId,
    },
    InvalidAggregateField {
        function: FunctionId,
        field: String,
    },
    InvalidVariantTag {
        function: FunctionId,
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
    EmptyDynamicDispatch {
        function: FunctionId,
    },
    InvalidDynamicDispatchType {
        function: FunctionId,
        ty: TypeId,
    },
    DynamicDispatchSignatureMismatch {
        function: FunctionId,
        target: FunctionId,
    },
    ClosureCaptureArityMismatch {
        function: FunctionId,
        target: FunctionId,
        expected: usize,
        actual: usize,
    },
    ClosureCaptureModeMismatch {
        function: FunctionId,
        target: FunctionId,
        capture: usize,
        expected: MirParameterMode,
        actual: MirCallArgumentMode,
    },
    ClosureParameterModeCount {
        function: FunctionId,
        types: usize,
        modes: usize,
    },
    InvalidClosureParameterType {
        function: FunctionId,
        ty: TypeId,
    },
    InvalidExternalTarget {
        function: FunctionId,
        target: ExternalSymbolId,
    },
    InvalidBuiltinTarget {
        function: FunctionId,
        target: BuiltinId,
    },
    InvalidBuiltinTypeArgument {
        function: FunctionId,
        ty: TypeId,
    },
    BuiltinTypeArgumentArity {
        function: FunctionId,
        target: BuiltinId,
        expected: usize,
        actual: usize,
    },
    InvalidTypeLayout {
        ty: TypeId,
        name: String,
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
mod tests;
