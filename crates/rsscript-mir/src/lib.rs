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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirCallTarget {
    Function(FunctionId),
    /// Catalog-owned direct core-library call. The source namespace/name was
    /// resolved by semantic lowering; v1 bytecode spelling is an encoder-only
    /// compatibility projection through [`builtin_vm_name`].
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
            source: None,
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
        }
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

#[derive(Debug, Clone, PartialEq)]
pub struct MirModule {
    types: Vec<WireType>,
    type_layouts: Vec<MirTypeLayout>,
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

fn verify_type_layouts(
    types: &[WireType],
    layouts: &[MirTypeLayout],
) -> Result<(), MirValidationError> {
    let mut names = BTreeSet::new();
    let mut layout_types = BTreeSet::new();
    for layout in layouts {
        if layout.name.is_empty() || !names.insert(layout.name.clone()) {
            return Err(MirValidationError::InvalidTypeLayout {
                ty: layout.ty,
                name: layout.name.clone(),
            });
        }
        if !layout_types.insert(layout.ty)
            || !matches!(
                types.get(layout.ty.index()),
                Some(WireType::Named { name, .. }) if name == &layout.name
            )
        {
            return Err(MirValidationError::InvalidTypeLayout {
                ty: layout.ty,
                name: layout.name.clone(),
            });
        }
        let mut fields = BTreeSet::new();
        for (name, ty) in &layout.fields {
            if name.is_empty() || !fields.insert(name) || ty.index() >= types.len() {
                return Err(MirValidationError::InvalidTypeLayout {
                    ty: layout.ty,
                    name: layout.name.clone(),
                });
            }
        }
    }
    Ok(())
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

fn verify_record_types(
    function: &MirFunction,
    types: &[WireType],
) -> Result<(), MirValidationError> {
    for block in function.blocks() {
        for instruction in block.instructions() {
            match instruction {
                MirInstruction::MakeStruct { ty, fields, .. }
                | MirInstruction::MakeVariant { ty, fields, .. } => {
                    if !matches!(types.get(ty.index()), Some(WireType::Named { .. })) {
                        return Err(MirValidationError::InvalidRecordType {
                            function: function.id,
                            ty: *ty,
                        });
                    }
                    let mut names = BTreeSet::new();
                    for (field, _) in fields {
                        if field.is_empty() || !names.insert(field) {
                            return Err(MirValidationError::InvalidAggregateField {
                                function: function.id,
                                field: field.clone(),
                            });
                        }
                    }
                }
                MirInstruction::GetField { field, .. } if field.is_empty() => {
                    return Err(MirValidationError::InvalidAggregateField {
                        function: function.id,
                        field: field.clone(),
                    });
                }
                _ => {}
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
                // A first-ready selection transfers the winning result to
                // ordinary values and cancels/reaps every losing child before
                // arm dispatch. It therefore closes all selected task
                // lifetimes at this explicit boundary.
                MirInstruction::Select { tasks, .. } => {
                    for task in tasks {
                        if live.remove(task).is_none() {
                            return Err(MirValidationError::TaskNotLive {
                                function: function.id,
                                task: *task,
                            });
                        }
                    }
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
                type_count,
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
                for destination in instruction_definitions(instruction) {
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
            for destination in instruction_definitions(instruction) {
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

fn instruction_definitions(instruction: &MirInstruction) -> Vec<ValueId> {
    match instruction {
        MirInstruction::LoadLiteral { destination, .. }
        | MirInstruction::MakeList { destination, .. }
        | MirInstruction::MakeMap { destination, .. }
        | MirInstruction::MakeObject { destination, .. }
        | MirInstruction::MakeStruct { destination, .. }
        | MirInstruction::MakeVariant { destination, .. }
        | MirInstruction::MakeResult { destination, .. }
        | MirInstruction::UnwrapResult { destination, .. }
        | MirInstruction::MakeOption { destination, .. }
        | MirInstruction::UnwrapOption { destination, .. }
        | MirInstruction::ListGet { destination, .. }
        | MirInstruction::ListAppend { destination, .. }
        | MirInstruction::ListClear { destination, .. }
        | MirInstruction::ListPop { destination, .. }
        | MirInstruction::ListPush { destination, .. }
        | MirInstruction::ListRemoveAt { destination, .. }
        | MirInstruction::ListSet { destination, .. }
        | MirInstruction::SetClear { destination, .. }
        | MirInstruction::SetInsert { destination, .. }
        | MirInstruction::SetRemove { destination, .. }
        | MirInstruction::DequeClear { destination, .. }
        | MirInstruction::DequePopBack { destination, .. }
        | MirInstruction::DequePopFront { destination, .. }
        | MirInstruction::DequePushBack { destination, .. }
        | MirInstruction::DequePushFront { destination, .. }
        | MirInstruction::SortedMapClear { destination, .. }
        | MirInstruction::SortedMapInsert { destination, .. }
        | MirInstruction::SortedMapRemove { destination, .. }
        | MirInstruction::SortedSetClear { destination, .. }
        | MirInstruction::SortedSetInsert { destination, .. }
        | MirInstruction::SortedSetRemove { destination, .. }
        | MirInstruction::BufferClear { destination, .. }
        | MirInstruction::StringBuilderPush { destination, .. }
        | MirInstruction::StringBuilderFinish { destination, .. }
        | MirInstruction::MapGet { destination, .. }
        | MirInstruction::MapClear { destination, .. }
        | MirInstruction::MapInsert { destination, .. }
        | MirInstruction::MapInsertOld { destination, .. }
        | MirInstruction::MapRemove { destination, .. }
        | MirInstruction::GetField { destination, .. }
        | MirInstruction::ListLen { destination, .. }
        | MirInstruction::ReadPlace { destination, .. }
        | MirInstruction::BorrowRead { destination, .. }
        | MirInstruction::TakePlace { destination, .. }
        | MirInstruction::Manage { destination, .. }
        | MirInstruction::Binary { destination, .. }
        | MirInstruction::Call { destination, .. }
        | MirInstruction::Await { destination, .. }
        | MirInstruction::TryResult { destination, .. } => vec![*destination],
        MirInstruction::Select { winner, value, .. } => vec![*winner, *value],
        MirInstruction::WritePlace { .. }
        | MirInstruction::Retain { .. }
        | MirInstruction::Drop { .. }
        | MirInstruction::AcquireResource { .. }
        | MirInstruction::ReleaseResource { .. }
        | MirInstruction::Spawn { .. }
        | MirInstruction::Cancel { .. }
        | MirInstruction::Join { .. }
        | MirInstruction::Discard { .. } => Vec::new(),
    }
}

fn instruction_uses(instruction: &MirInstruction) -> Vec<ValueId> {
    match instruction {
        MirInstruction::WritePlace { value, .. } | MirInstruction::Discard { value } => {
            vec![*value]
        }
        MirInstruction::MakeList { items, .. } => items.clone(),
        MirInstruction::MakeMap { entries, .. } => entries
            .iter()
            .flat_map(|(key, value)| [*key, *value])
            .collect(),
        MirInstruction::MakeObject { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstruction::MakeStruct { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstruction::MakeVariant { fields, .. } => {
            fields.iter().map(|(_, value)| *value).collect()
        }
        MirInstruction::MakeResult { value, .. } => vec![*value],
        MirInstruction::UnwrapResult { source, .. } => vec![*source],
        MirInstruction::MakeOption { value, .. } => value.iter().copied().collect(),
        MirInstruction::UnwrapOption { source, .. } => vec![*source],
        MirInstruction::ListGet { list, index, .. } => vec![*list, *index],
        MirInstruction::ListAppend { values, .. }
        | MirInstruction::ListPush { value: values, .. } => {
            vec![*values]
        }
        MirInstruction::ListRemoveAt { index, .. } => vec![*index],
        MirInstruction::ListSet { index, value, .. } => vec![*index, *value],
        MirInstruction::SetInsert { value, .. } | MirInstruction::SetRemove { value, .. } => {
            vec![*value]
        }
        MirInstruction::DequePushBack { value, .. }
        | MirInstruction::DequePushFront { value, .. } => vec![*value],
        MirInstruction::SortedMapInsert { key, value, .. } => vec![*key, *value],
        MirInstruction::SortedMapRemove { key, .. } => vec![*key],
        MirInstruction::SortedSetInsert { value, .. }
        | MirInstruction::SortedSetRemove { value, .. } => vec![*value],
        MirInstruction::StringBuilderPush { value, .. }
        | MirInstruction::StringBuilderFinish { builder: value, .. } => vec![*value],
        MirInstruction::MapGet { map, key, .. } => vec![*map, *key],
        MirInstruction::MapInsert { key, value, .. }
        | MirInstruction::MapInsertOld { key, value, .. } => vec![*key, *value],
        MirInstruction::MapRemove { key, .. } => vec![*key],
        MirInstruction::GetField { base, .. } => vec![*base],
        MirInstruction::ListLen { list, .. } => vec![*list],
        MirInstruction::AcquireResource { source, .. } => vec![*source],
        MirInstruction::Manage { source, .. } => vec![*source],
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
        | MirInstruction::Select { .. }
        | MirInstruction::Cancel { .. }
        | MirInstruction::Join { .. }
        | MirInstruction::MapClear { .. }
        | MirInstruction::ListClear { .. }
        | MirInstruction::ListPop { .. }
        | MirInstruction::SetClear { .. }
        | MirInstruction::DequeClear { .. }
        | MirInstruction::DequePopBack { .. }
        | MirInstruction::DequePopFront { .. }
        | MirInstruction::SortedMapClear { .. }
        | MirInstruction::SortedSetClear { .. }
        | MirInstruction::BufferClear { .. } => Vec::new(),
        MirInstruction::TryResult { source, .. } => vec![*source],
    }
}

fn terminator_uses(terminator: &MirTerminator) -> Vec<ValueId> {
    match terminator {
        MirTerminator::Return(Some(value)) => vec![*value],
        MirTerminator::Branch { condition, .. } => vec![*condition],
        MirTerminator::MatchVariant { value, .. } => vec![*value],
        MirTerminator::MatchResult { value, .. } => vec![*value],
        MirTerminator::MatchOption { value, .. } => vec![*value],
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
        MirTerminator::MatchVariant {
            match_target,
            else_target,
            ..
        } => {
            successors[0] = Some(*match_target);
            successors[1] = Some(*else_target);
        }
        MirTerminator::MatchResult {
            ok_target,
            err_target,
            ..
        } => {
            successors[0] = Some(*ok_target);
            successors[1] = Some(*err_target);
        }
        MirTerminator::MatchOption {
            some_target,
            none_target,
            ..
        } => {
            successors[0] = Some(*some_target);
            successors[1] = Some(*none_target);
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
        MirInstruction::Manage { .. } => Ok(()),
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
        MirInstruction::MapClear { map, .. } => check_live(*map, moved_places),
        MirInstruction::ListAppend { list, .. }
        | MirInstruction::ListClear { list, .. }
        | MirInstruction::ListPop { list, .. }
        | MirInstruction::ListPush { list, .. }
        | MirInstruction::ListRemoveAt { list, .. }
        | MirInstruction::ListSet { list, .. } => check_live(*list, moved_places),
        MirInstruction::SetClear { set, .. }
        | MirInstruction::SetInsert { set, .. }
        | MirInstruction::SetRemove { set, .. } => check_live(*set, moved_places),
        MirInstruction::DequeClear { deque, .. }
        | MirInstruction::DequePopBack { deque, .. }
        | MirInstruction::DequePopFront { deque, .. }
        | MirInstruction::DequePushBack { deque, .. }
        | MirInstruction::DequePushFront { deque, .. } => check_live(*deque, moved_places),
        MirInstruction::SortedMapClear { map, .. }
        | MirInstruction::SortedMapInsert { map, .. }
        | MirInstruction::SortedMapRemove { map, .. } => check_live(*map, moved_places),
        MirInstruction::SortedSetClear { set, .. }
        | MirInstruction::SortedSetInsert { set, .. }
        | MirInstruction::SortedSetRemove { set, .. } => check_live(*set, moved_places),
        MirInstruction::BufferClear { buffer, .. }
        | MirInstruction::StringBuilderPush {
            builder: buffer, ..
        } => check_live(*buffer, moved_places),
        MirInstruction::MapInsert { map, .. }
        | MirInstruction::MapInsertOld { map, .. }
        | MirInstruction::MapRemove { map, .. } => check_live(*map, moved_places),
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
        | MirInstruction::MakeMap { .. }
        | MirInstruction::MakeObject { .. }
        | MirInstruction::MakeStruct { .. }
        | MirInstruction::MakeVariant { .. }
        | MirInstruction::MakeResult { .. }
        | MirInstruction::UnwrapResult { .. }
        | MirInstruction::MakeOption { .. }
        | MirInstruction::UnwrapOption { .. }
        | MirInstruction::ListGet { .. }
        | MirInstruction::MapGet { .. }
        | MirInstruction::StringBuilderFinish { .. }
        | MirInstruction::GetField { .. }
        | MirInstruction::ListLen { .. }
        | MirInstruction::Binary { .. }
        | MirInstruction::Await { .. }
        | MirInstruction::Select { .. }
        | MirInstruction::Cancel { .. }
        | MirInstruction::Join { .. }
        | MirInstruction::Discard { .. } => Ok(()),
        MirInstruction::TryResult { cleanup, .. } => {
            for place in cleanup {
                check_live(*place, moved_places)?;
            }
            Ok(())
        }
    }
}

fn verify_instruction(
    function: &MirFunction,
    instruction: &MirInstruction,
    defined: &mut BTreeSet<ValueId>,
    used: &mut Vec<ValueId>,
    moved_places: &mut BTreeSet<PlaceId>,
    type_count: usize,
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
        | MirInstruction::MakeList { destination, .. }
        | MirInstruction::MakeMap { destination, .. }
        | MirInstruction::MakeObject { destination, .. }
        | MirInstruction::MakeResult { destination, .. }
        | MirInstruction::ListGet { destination, .. }
        | MirInstruction::MapGet { destination, .. }
        | MirInstruction::GetField { destination, .. }
        | MirInstruction::ListLen { destination, .. } => define(*destination, defined),
        MirInstruction::MakeStruct { destination, .. }
        | MirInstruction::MakeVariant { destination, .. } => define(*destination, defined),
        MirInstruction::UnwrapResult {
            destination,
            source,
            ..
        } => {
            define(*destination, defined)?;
            used.push(*source);
            Ok(())
        }
        MirInstruction::MakeOption { destination, .. } => define(*destination, defined),
        MirInstruction::UnwrapOption {
            destination,
            source,
        } => {
            define(*destination, defined)?;
            used.push(*source);
            Ok(())
        }
        MirInstruction::ListAppend {
            destination,
            list,
            values,
        } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)?;
            used.push(*values);
            Ok(())
        }
        MirInstruction::ListClear { destination, list }
        | MirInstruction::ListPop { destination, list } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::ListPush {
            destination,
            list,
            value,
        } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::ListRemoveAt {
            destination,
            list,
            index,
        } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)?;
            used.push(*index);
            Ok(())
        }
        MirInstruction::ListSet {
            destination,
            list,
            index,
            value,
        } => {
            check_live_place(*list, moved_places)?;
            define(*destination, defined)?;
            used.push(*index);
            used.push(*value);
            Ok(())
        }
        MirInstruction::SetClear { destination, set } => {
            check_live_place(*set, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::SetInsert {
            destination,
            set,
            value,
        }
        | MirInstruction::SetRemove {
            destination,
            set,
            value,
        } => {
            check_live_place(*set, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::DequeClear { destination, deque }
        | MirInstruction::DequePopBack { destination, deque }
        | MirInstruction::DequePopFront { destination, deque } => {
            check_live_place(*deque, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::DequePushBack {
            destination,
            deque,
            value,
        }
        | MirInstruction::DequePushFront {
            destination,
            deque,
            value,
        } => {
            check_live_place(*deque, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::SortedMapClear { destination, map } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::SortedMapInsert {
            destination,
            map,
            key,
            value,
        } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)?;
            used.push(*key);
            used.push(*value);
            Ok(())
        }
        MirInstruction::SortedMapRemove {
            destination,
            map,
            key,
        } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)?;
            used.push(*key);
            Ok(())
        }
        MirInstruction::SortedSetClear { destination, set } => {
            check_live_place(*set, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::SortedSetInsert {
            destination,
            set,
            value,
        }
        | MirInstruction::SortedSetRemove {
            destination,
            set,
            value,
        } => {
            check_live_place(*set, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::BufferClear {
            destination,
            buffer,
        } => {
            check_live_place(*buffer, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::StringBuilderPush {
            destination,
            builder,
            value,
        } => {
            check_live_place(*builder, moved_places)?;
            define(*destination, defined)?;
            used.push(*value);
            Ok(())
        }
        MirInstruction::StringBuilderFinish {
            destination,
            builder,
        } => {
            define(*destination, defined)?;
            used.push(*builder);
            Ok(())
        }
        MirInstruction::MapClear { destination, map } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)
        }
        MirInstruction::MapInsert {
            destination,
            map,
            key,
            value,
        }
        | MirInstruction::MapInsertOld {
            destination,
            map,
            key,
            value,
        } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)?;
            used.push(*key);
            used.push(*value);
            Ok(())
        }
        MirInstruction::MapRemove {
            destination,
            map,
            key,
        } => {
            check_live_place(*map, moved_places)?;
            define(*destination, defined)?;
            used.push(*key);
            Ok(())
        }
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
        MirInstruction::Manage {
            destination,
            source,
        } => {
            define(*destination, defined)?;
            used.push(*source);
            Ok(())
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
        MirInstruction::TryResult {
            destination,
            source,
            cleanup,
        } => {
            define(*destination, defined)?;
            used.push(*source);
            for place in cleanup {
                check_live_place(*place, moved_places)?;
            }
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
                MirCallTarget::Builtin {
                    id,
                    parameter_modes,
                    type_arguments,
                } if builtin_vm_name(*id).is_some() => parameter_modes.to_vec(),
                MirCallTarget::Builtin { id, .. } => {
                    return Err(MirValidationError::InvalidBuiltinTarget {
                        function: function.id,
                        target: *id,
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
            if let MirCallTarget::Builtin {
                id, type_arguments, ..
            } = target
            {
                let expected_type_arguments = match builtin_vm_name(*id) {
                    Some("JsonDecode" | "JsonDecodeText") => 1,
                    Some(_) => 0,
                    None => unreachable!("builtin target was validated above"),
                };
                if type_arguments.len() != expected_type_arguments {
                    return Err(MirValidationError::BuiltinTypeArgumentArity {
                        function: function.id,
                        target: *id,
                        expected: expected_type_arguments,
                        actual: type_arguments.len(),
                    });
                }
                for ty in type_arguments {
                    if ty.index() >= type_count {
                        return Err(MirValidationError::InvalidBuiltinTypeArgument {
                            function: function.id,
                            ty: *ty,
                        });
                    }
                }
            }
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
        MirInstruction::Select { winner, value, .. } => {
            define(*winner, defined)?;
            define(*value, defined)
        }
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
        MirTerminator::MatchVariant {
            expected,
            match_target,
            else_target,
            ..
        } => {
            if expected.is_empty() {
                return Err(MirValidationError::InvalidVariantTag {
                    function: function.id,
                });
            }
            check_target(*match_target)?;
            check_target(*else_target)?;
        }
        MirTerminator::MatchResult {
            ok_target,
            err_target,
            ..
        } => {
            check_target(*ok_target)?;
            check_target(*err_target)?;
        }
        MirTerminator::MatchOption {
            some_target,
            none_target,
            ..
        } => {
            check_target(*some_target)?;
            check_target(*none_target)?;
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
    fn resource_cleanup_is_required_on_every_reachable_return_edge() {
        let types = vec![
            WireType::Unit,
            WireType::Bool,
            WireType::Resource {
                name: "host.fs.File".into(),
            },
        ];
        let entry = BasicBlock::new(
            BlockId::new(0),
            vec![
                MirInstruction::LoadLiteral {
                    destination: ValueId::new(0),
                    value: MirLiteral::Bool(true),
                },
                MirInstruction::LoadLiteral {
                    destination: ValueId::new(1),
                    value: MirLiteral::Unit,
                },
                MirInstruction::AcquireResource {
                    place: PlaceId::new(0),
                    resource_type: ResourceTypeId::new(2),
                    source: ValueId::new(1),
                },
            ],
            MirTerminator::Branch {
                condition: ValueId::new(0),
                then_target: BlockId::new(1),
                else_target: BlockId::new(2),
            },
        );
        let released = BasicBlock::new(
            BlockId::new(1),
            vec![MirInstruction::ReleaseResource {
                place: PlaceId::new(0),
            }],
            MirTerminator::Return(None),
        );
        let also_released = BasicBlock::new(
            BlockId::new(2),
            vec![MirInstruction::ReleaseResource {
                place: PlaceId::new(0),
            }],
            MirTerminator::Return(None),
        );
        let valid = MirModule::new(
            types.clone(),
            vec![MirFunction::new(
                FunctionId::new(0),
                signature(),
                1,
                2,
                vec![entry.clone(), released.clone(), also_released],
            )],
            vec![MirFunctionDebug::new("main", vec!["file".into()])],
            vec![],
        );
        assert!(valid.is_ok(), "every branch releases the resource");

        let missing_release = BasicBlock::new(BlockId::new(2), vec![], MirTerminator::Return(None));
        let leaked = MirModule::new(
            types,
            vec![MirFunction::new(
                FunctionId::new(0),
                signature(),
                1,
                2,
                vec![entry, released, missing_release],
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
    fn task_groups_must_close_on_every_reachable_return_edge() {
        let entry = BasicBlock::new(
            BlockId::new(0),
            vec![
                MirInstruction::LoadLiteral {
                    destination: ValueId::new(0),
                    value: MirLiteral::Bool(true),
                },
                MirInstruction::Spawn {
                    task: TaskId::new(0),
                    group: TaskGroupId::new(0),
                    target: FunctionId::new(1),
                    arguments: vec![],
                },
            ],
            MirTerminator::Branch {
                condition: ValueId::new(0),
                then_target: BlockId::new(1),
                else_target: BlockId::new(2),
            },
        );
        let joined = BasicBlock::new(
            BlockId::new(1),
            vec![MirInstruction::Join {
                group: TaskGroupId::new(0),
            }],
            MirTerminator::Return(None),
        );
        let also_joined = BasicBlock::new(
            BlockId::new(2),
            vec![MirInstruction::Join {
                group: TaskGroupId::new(0),
            }],
            MirTerminator::Return(None),
        );
        let worker = MirFunction::new(
            FunctionId::new(1),
            MirFunctionSignature::new(vec![], TypeId::new(0), true),
            0,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![],
                MirTerminator::Return(None),
            )],
        );
        let valid = MirModule::new(
            vec![WireType::Unit, WireType::Bool],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    signature(),
                    0,
                    1,
                    vec![entry.clone(), joined.clone(), also_joined],
                ),
                worker.clone(),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        );
        assert!(valid.is_ok(), "every branch drains the task group");

        let missing_join = BasicBlock::new(BlockId::new(2), vec![], MirTerminator::Return(None));
        let leaked = MirModule::new(
            vec![WireType::Unit, WireType::Bool],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    signature(),
                    0,
                    1,
                    vec![entry, joined, missing_join],
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
    fn select_consumes_each_live_task_exactly_once() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let invalid = MirModule::new(
            vec![int],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    MirFunctionSignature::new(vec![], TypeId::new(0), false),
                    0,
                    2,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::Spawn {
                                task: TaskId::new(0),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(1),
                                arguments: vec![],
                            },
                            MirInstruction::Select {
                                tasks: vec![TaskId::new(0), TaskId::new(0)],
                                winner: ValueId::new(0),
                                value: ValueId::new(1),
                            },
                        ],
                        MirTerminator::Return(Some(ValueId::new(1))),
                    )],
                ),
                MirFunction::new(
                    FunctionId::new(1),
                    MirFunctionSignature::new(vec![], TypeId::new(0), true),
                    0,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(1),
                        }],
                        MirTerminator::Return(Some(ValueId::new(0))),
                    )],
                ),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        );
        assert!(matches!(
            invalid,
            Err(MirValidationError::TaskNotLive {
                task,
                ..
            }) if task == TaskId::new(0)
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

    #[test]
    fn rejects_record_construction_without_a_named_layout_type() {
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            2,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Unit,
                    },
                    MirInstruction::MakeStruct {
                        destination: ValueId::new(1),
                        ty: TypeId::new(0),
                        fields: vec![("value".into(), ValueId::new(0))],
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(1))),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::InvalidRecordType { .. })
        ));
    }

    #[test]
    fn rejects_invalid_builtin_type_metadata_and_runtime_layouts() {
        let decode = builtin_id("Json", "decode").expect("JSON decode is catalog-owned");
        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            1,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![MirInstruction::Call {
                    destination: ValueId::new(0),
                    target: MirCallTarget::Builtin {
                        id: decode,
                        parameter_modes: vec![MirParameterMode::Read].into_boxed_slice(),
                        type_arguments: vec![TypeId::new(0), TypeId::new(0)].into_boxed_slice(),
                    },
                    arguments: Vec::new(),
                }],
                MirTerminator::Return(Some(ValueId::new(0))),
            )],
        );
        assert!(matches!(
            MirModule::new(vec![WireType::Unit], vec![function], debug(), Vec::new()),
            Err(MirValidationError::BuiltinTypeArgumentArity { .. })
        ));

        let function = MirFunction::new(
            FunctionId::new(0),
            signature(),
            0,
            0,
            vec![BasicBlock::new(
                BlockId::new(0),
                Vec::new(),
                MirTerminator::Return(None),
            )],
        );
        assert!(matches!(
            MirModule::with_type_layouts(
                vec![WireType::Named {
                    package: None,
                    name: "Actual".into(),
                    arguments: Vec::new(),
                }],
                vec![MirTypeLayout::new(TypeId::new(0), "Wrong", Vec::new())],
                vec![function],
                debug(),
                Vec::new(),
            ),
            Err(MirValidationError::InvalidTypeLayout { .. })
        ));
    }
}
