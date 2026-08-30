use super::*;

#[derive(Debug, Clone)]
pub(crate) struct RegUnit {
    pub(crate) functions: Vec<Rc<RegFunction>>,
    pub(crate) function_ids: HashMap<String, usize>,
    pub(crate) resource_drop_functions: HashMap<String, usize>,
    pub(crate) types: HashMap<String, RegTypeInfo>,
    /// Complete declared layouts for user sum types. This is compatibility
    /// metadata for the v1 string-named VM representation; it lets the
    /// reviewed report bridge a completed value into numeric `WireValue`
    /// identities without inferring cases from executed instructions.
    pub(crate) variant_layouts: HashMap<String, RegVariantInfo>,
    /// Declared HIR signatures keyed by lowered function name. The register VM
    /// bytecode remains untyped, but native lowering can use this as a conservative
    /// seed for scalar/handle ABI inference when a function body is otherwise
    /// polymorphic (for example `Float` parameters used only in arithmetic).
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) native_signatures: HashMap<String, RegNativeSignature>,
    /// Whether the program can ever *observe closure identity* — i.e. whether any
    /// user-source `==`/`!=` could compare operands whose static type is, or
    /// transitively contains, a `Fn`/closure value. Computed conservatively at
    /// lower time (see [`type_name_may_contain_fn`]): `true` whenever closure
    /// identity *might* leak, `false` only when it provably cannot.
    ///
    /// When `false`, the VM may share one cached `Rc<VmClosure>` for repeated
    /// `MakeClosure` of the same non-capturing function (a refcount bump instead
    /// of a heap allocation per iteration). That share is unobservable precisely
    /// because the sole remaining identity observer is `==`/`!=` → structural
    /// `PartialEq` → `Rc::ptr_eq` (closures are not `Hashable`, so they can never
    /// be `Map`/`Set` keys — see `is_hashable`), and this flag certifies no such
    /// comparison can reach a closure. When `true`, caching is disabled and every
    /// `MakeClosure` allocates a fresh `Rc`, exactly as the compiled backend does.
    pub(crate) closure_identity_observable: bool,
}

/// Runtime-only projection of a checked type. Keeping syntax expressions and
/// semantic caches out of bytecode makes the artifact deterministic and keeps
/// the VM independent from frontend implementation details.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RegTypeInfo {
    pub(crate) name: String,
    pub(crate) fields: Vec<RegFieldInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RegFieldInfo {
    pub(crate) name: String,
    pub(crate) type_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RegVariantInfo {
    pub(crate) name: String,
    pub(crate) variants: Vec<RegVariantCaseInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RegVariantCaseInfo {
    pub(crate) name: String,
    pub(crate) fields: Vec<RegFieldInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct RegNativeSignature {
    pub(crate) params: Vec<String>,
    pub(crate) return_type: Option<String>,
}

#[cfg(feature = "native-jit")]
mod profile;

#[cfg(feature = "native-jit")]
pub(crate) use profile::*;

/// Cached classification of a function as a scalar self-recursion JIT candidate
/// (computed lazily by the tier dispatcher). `Int` and `Bool` are the
/// i64-representable return kinds that the native `CallSelf` fast path and the
/// tier-0 i64 scalar executor can both run and wrap. Non-i64 kinds (e.g. Float)
/// are not classified here — they route through the general native path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelfRecursionKind {
    Ineligible,
    Int,
    Bool,
}

#[derive(Debug, Clone)]
pub(crate) struct RegFunction {
    /// Stable ordinal assigned by the verified bytecode decoder. This is a
    /// general program identity used by frames and side tables; it is not JIT
    /// feedback and never changes during execution.
    pub(crate) ordinal: usize,
    // `params`/`captures` are metadata read only by the native JIT (translation);
    // `name` is retained as diagnostic/debug metadata.
    #[allow(dead_code)]
    pub(crate) name: String,
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) params: usize,
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) captures: usize,
    pub(crate) regs: usize,
    pub(crate) local_regs: HashMap<String, Reg>,
    pub(crate) code: Vec<RegInstr>,
}

#[cfg(feature = "native-jit")]
pub(crate) const MAX_OSR_REGIONS_PER_FUNCTION: usize = 4;

/// Stable identity for one OSR region. A function can own several independently
/// compiled loop regions, so function identity alone is not a sufficient cache key.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RegionKey {
    pub(crate) function: usize,
    pub(crate) header: usize,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OsrCandidate {
    pub(crate) header_ip: usize,
    pub(crate) iteration_work: u32,
}

/// Fixed-capacity candidate list. The bound keeps selection and the interpreter
/// header check predictable without allocating per frame.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OsrCandidates {
    pub(crate) entries: [Option<OsrCandidate>; MAX_OSR_REGIONS_PER_FUNCTION],
}

#[cfg(feature = "native-jit")]
impl OsrCandidates {
    pub(crate) fn iter(self) -> impl Iterator<Item = OsrCandidate> {
        self.entries.into_iter().flatten()
    }

    pub(crate) fn is_empty(self) -> bool {
        self.entries[0].is_none()
    }
}

/// Per-region OSR auto-trigger state machine. Live instances are evaluation-local
/// entries in [`NativeState::osr_triggers`].
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OsrTrigger {
    /// The interpreter accumulates this region's estimated per-iteration work. At
    /// [`OSR_BACKEDGE_THRESHOLD`] work units it fires `try_osr`. `probe_cc` is the
    /// function's dynamic-call count (`call_count`) as of the LAST `try_osr` probe
    /// (0 before the first), used to gate re-probes: a pending-profile decline only
    /// resets the counter if the profile has ADVANCED (`call_count` increased) since
    /// then — otherwise the site is dynamically dead/stalled and we `GaveUp`.
    /// `call_count` is capped at `PROFILE_RECORD_LIMIT`, so the number of
    /// progress-resets is bounded.
    Counting {
        /// Saturating interpreted-work units, not a raw backedge count.
        count: u32,
        probe_cc: u32,
    },
    /// This region declined stably. Other regions in the function remain active.
    GaveUp,
}

/// Shared lifecycle used by every native region kind. Region-specific planners
/// retain their own cache keys, but compilation hotness and consecutive dynamic
/// failure use one state machine so whole/OSR/continuation policy cannot silently
/// invent incompatible meanings for "cold", "compiled", and "disabled".
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegionTierState {
    Cold {
        interpreted_work: u64,
    },
    Baseline {
        consecutive_bails: u32,
        native_work: u64,
    },
    Optimized {
        consecutive_bails: u32,
        native_work: u64,
    },
    Disabled,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RegionController {
    pub(crate) state: RegionTierState,
}

#[cfg(feature = "native-jit")]
impl Default for RegionController {
    fn default() -> Self {
        Self {
            state: RegionTierState::Cold {
                interpreted_work: 0,
            },
        }
    }
}

#[cfg(feature = "native-jit")]
impl RegionController {
    pub(crate) fn observe_interpreted_work(&mut self, work: u64, threshold: u64) -> bool {
        match &mut self.state {
            RegionTierState::Cold { interpreted_work } => {
                *interpreted_work = interpreted_work.saturating_add(work);
                *interpreted_work >= threshold
            }
            RegionTierState::Baseline { .. } | RegionTierState::Optimized { .. } => true,
            RegionTierState::Disabled => false,
        }
    }

    pub(crate) fn compiled(&mut self, optimized: bool) {
        self.state = if optimized {
            RegionTierState::Optimized {
                consecutive_bails: 0,
                native_work: 0,
            }
        } else {
            RegionTierState::Baseline {
                consecutive_bails: 0,
                native_work: 0,
            }
        };
    }

    pub(crate) fn observe_native_work(&mut self, work: u64, threshold: u64) -> bool {
        match &mut self.state {
            RegionTierState::Baseline { native_work, .. } => {
                *native_work = native_work.saturating_add(work);
                *native_work >= threshold
            }
            RegionTierState::Optimized { .. } => true,
            RegionTierState::Cold { .. } | RegionTierState::Disabled => false,
        }
    }

    pub(crate) fn successful_entry(&mut self) {
        match &mut self.state {
            RegionTierState::Baseline {
                consecutive_bails, ..
            }
            | RegionTierState::Optimized {
                consecutive_bails, ..
            } => *consecutive_bails = 0,
            RegionTierState::Cold { .. } | RegionTierState::Disabled => {}
        }
    }

    pub(crate) fn dynamic_bail(&mut self, threshold: u32) -> bool {
        let count = match &mut self.state {
            RegionTierState::Baseline {
                consecutive_bails, ..
            }
            | RegionTierState::Optimized {
                consecutive_bails, ..
            } => consecutive_bails,
            RegionTierState::Cold { .. } => return false,
            RegionTierState::Disabled => return true,
        };
        *count = count.saturating_add(1);
        if *count >= threshold {
            self.state = RegionTierState::Disabled;
            true
        } else {
            false
        }
    }

    pub(crate) fn disable(&mut self) {
        self.state = RegionTierState::Disabled;
    }
}

impl RegFunction {
    #[cfg(test)]
    pub(crate) fn placeholder(name: String) -> Self {
        Self {
            ordinal: 0,
            name,
            params: 0,
            captures: 0,
            regs: 0,
            local_regs: HashMap::new(),
            code: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) enum RegInstr {
    LoadUnit {
        dst: Reg,
    },
    LoadInt {
        dst: Reg,
        value: i64,
    },
    LoadFloat {
        dst: Reg,
        value: f64,
    },
    LoadBool {
        dst: Reg,
        value: bool,
    },
    LoadString {
        dst: Reg,
        value: Rc<String>,
    },
    LoadChar {
        dst: Reg,
        value: char,
    },
    Move {
        dst: Reg,
        src: Reg,
    },
    /// Logical self-tail-call boundary inserted by the TCO pass. The interpreter
    /// increments the current frame's elided-call count and enforces
    /// `VmLimits::max_depth` exactly as if another frame had been pushed.
    TailCallGuard,
    /// Replace `reg` with a deep copy of its value (fresh `Rc` for every mutable
    /// container in the tree, recursing through structs/variants/options; shared
    /// reference values like `Managed` keep their handle). Emitted at the function
    /// prologue for every non-`mut` parameter so the callee owns an isolated copy,
    /// mirroring the Rust backend, which passes `mut` as `&mut` (mutations
    /// propagate) and everything else by value/`&` + an inserted `.clone()`.
    DeepCopy {
        reg: Reg,
    },
    /// Marker-preserving elided `DeepCopy`: produced ONLY by the compile-time elision pass
    /// (`RSS_VM_ELIDE_DEEPCOPY`) in place of a `DeepCopy` proven redundant. The INTERPRETER
    /// treats it as a no-op (share the caller's `Rc`, skip the copy — this is the win), while
    /// EVERY native-tier site treats it BYTE-IDENTICALLY to `DeepCopy` (soundness seed, flat-param
    /// ABI marker, tier-0 eligibility, `Nop` lowering). Keeping the marker — rather than rewriting
    /// to a self-`Move` — is what lets the interp elide the copy without perturbing native tiering.
    DeepCopyElided {
        reg: Reg,
    },
    Manage {
        dst: Reg,
        src: Reg,
    },
    GetField {
        dst: Reg,
        base: Reg,
        name: String,
    },
    /// Read a struct/variant field by precomputed slot (the lowerer resolved the
    /// declaration-order index from the static type) — no name lookup at runtime.
    GetFieldSlot {
        dst: Reg,
        base: Reg,
        slot: usize,
    },
    /// Slot-indexed counterpart of `SetField` (copy-on-write by slot).
    SetFieldSlot {
        dst: Reg,
        base: Reg,
        slot: usize,
        value: Reg,
    },
    /// Produce a copy of the struct in `base` with field `name` set to `value`.
    /// Structs are value types, so this rebuilds the struct rather than mutating
    /// in place; nested assignment targets compose these writes back up the path.
    SetField {
        dst: Reg,
        base: Reg,
        name: String,
        value: Reg,
    },
    MakeStruct {
        dst: Reg,
        /// The interned shared layout (V2.0), precomputed at lowering time from the
        /// canonical `(name, field_names)` so the hot construction path is a single
        /// refcount bump — no per-construction `(name, field_names)` re-hash.
        layout: Rc<crate::vm_value::TypeLayout>,
        fields: Vec<(String, Reg)>,
    },
    ResourceDrop {
        resource: Reg,
    },
    /// Enter an explicitly verified resource scope. The v1 register VM still
    /// carries the resource value in an ordinary register, but this marker
    /// records its lexical lifetime so terminal failure/cancellation can run
    /// the same drop path as normal scope exit.
    ResourceAcquire {
        resource: Reg,
    },
    MakeVariant {
        dst: Reg,
        /// Interned shared layout (V2.0), precomputed at lowering time (see
        /// `MakeStruct::layout`).
        layout: Rc<crate::vm_value::TypeLayout>,
        fields: Vec<(String, Reg)>,
    },
    MakeList {
        dst: Reg,
        items: Vec<Reg>,
    },
    MakeObject {
        dst: Reg,
        fields: Vec<(String, Reg)>,
    },
    MakeMap {
        dst: Reg,
        entries: Vec<(Reg, Reg)>,
    },
    AddInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    SubInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    MulInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    DivInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    ModInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BitAndInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BitOrInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    BitXorInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    ShiftLeftInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    ShiftRightInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    LessInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    LessEqualInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    GreaterInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    GreaterEqualInt {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Equal {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    NotEqual {
        dst: Reg,
        lhs: Reg,
        rhs: Reg,
    },
    Jump {
        target: usize,
    },
    JumpIfBool {
        cond: Reg,
        expected: bool,
        target: usize,
    },
    JumpIfIntCompare {
        lhs: Reg,
        rhs: Reg,
        op: RegIntCompare,
        expected: bool,
        target: usize,
    },
    MatchOption {
        src: Reg,
        some_ip: usize,
        none_ip: usize,
    },
    MatchResult {
        src: Reg,
        ok_ip: usize,
        err_ip: usize,
    },
    MatchVariant {
        src: Reg,
        expected: String,
        match_ip: usize,
        else_ip: usize,
    },
    RuntimeError {
        message: String,
    },
    MatchMapGet {
        map: Reg,
        key: Reg,
        value_dst: Reg,
        some_ip: usize,
        none_ip: usize,
    },
    MatchSortedMapGet {
        map: Reg,
        key: Reg,
        value_dst: Reg,
        some_ip: usize,
        none_ip: usize,
    },
    UnwrapSome {
        dst: Reg,
        src: Reg,
    },
    UnwrapVariantValue {
        dst: Reg,
        src: Reg,
        expected: String,
    },
    MakeClosure {
        dst: Reg,
        function: usize,
        captures: Vec<Reg>,
    },
    MakeSome {
        dst: Reg,
        value: Reg,
    },
    LoadNone {
        dst: Reg,
    },
    CallKnown {
        dst: Reg,
        function: usize,
        args: Vec<Reg>,
        /// Argument positions passed with `mut` (the callee's `mut` params). After
        /// the call returns, each such argument's (possibly mutated) value is
        /// written back to the caller's argument register, so a `mut` parameter's
        /// field/element mutations propagate to the caller — matching AOT's
        /// `&mut` semantics.
        mut_args: Vec<usize>,
    },
    /// Dynamic protocol dispatch: a `Protocol.method(self: x, ...)` call whose
    /// concrete impl is chosen at runtime by `args[0]`'s struct type name. This is
    /// how dynamic protocol values and generic protocol bounds dispatch in the VM,
    /// mirroring the compiled backend's closed-world enum dispatch. `dispatch`
    /// maps each implementing struct name to the impl's target function id.
    CallDynamic {
        dst: Reg,
        dispatch: Vec<(String, usize)>,
        args: Vec<Reg>,
        mut_args: Vec<usize>,
    },
    /// `spawn f(args)` / `async let`: start `function` as a new concurrent task
    /// and put a Task handle in `dst` (the spawning task keeps running).
    SpawnTask {
        dst: Reg,
        function: usize,
        args: Vec<Reg>,
    },
    /// `await x`: if `src` is a Task handle, join it (park until it finishes and
    /// receive its value); otherwise it is an already-evaluated async result and
    /// this is the identity move.
    AwaitJoin {
        dst: Reg,
        src: Reg,
    },
    /// Stop one still-live child task and discard its eventual result. This is
    /// deliberately distinct from `JoinTasks`: cancellation closes the task
    /// lifetime immediately, while run-owned Provider resources remain under
    /// the VM's exact-once finalizer.
    CancelTask {
        src: Reg,
    },
    /// Wait for every still-live child task named by `handles`. Unlike
    /// repeated `AwaitJoin`, group join discards child values and resumes the
    /// parent only after the entire lexical set has completed.
    JoinTasks {
        handles: Vec<Reg>,
    },
    /// `select { ... }`: each `handles` reg holds a spawned arm task. Park until
    /// the first arm finishes, then write its index to `winner` and its value to
    /// `value`; a branch ladder afterwards dispatches to the winning arm's body.
    SelectWait {
        handles: Vec<Reg>,
        winner: Reg,
        value: Reg,
    },
    CallExternal {
        dst: Reg,
        key: String,
        args: Vec<Reg>,
        /// Positions within `args` whose corresponding parameter is `mut`. After
        /// the call the host writes the mutated value back to those arg
        /// registers, so native in-place mutation propagates to the caller.
        mut_args: Vec<usize>,
    },
    CallClosure {
        dst: Reg,
        closure: Reg,
        args: Vec<Reg>,
        /// Argument positions passed with `mut` (the stored `Fn`'s `mut`
        /// parameters). After the closure returns, each such argument's
        /// (possibly mutated) value is written back to the caller's argument
        /// register, so a `mut Ctx` closure parameter's field mutations
        /// propagate to the caller — identical to `CallKnown`'s `mut_args` and to
        /// AOT's `&mut` argument semantics.
        mut_args: Vec<usize>,
    },
    /// Synthetic, native-JIT-only guard (profile-guided monomorphic inlining).
    /// NEVER emitted by the lowerer and NEVER executed by the interpreter — it is
    /// synthesized only inside [`native_inline_leaf_calls`], in code consumed solely
    /// by [`translate_to_native_jit`]. Lowers to a [`vm_jit::JitInstr::GuardClosureId`]:
    /// reads `closure`'s underlying function id and, if it differs from `expected`,
    /// bails to the interpreter (the existing re-run-from-top fallback). Guards an
    /// inlined monomorphic `CallClosure` so a mispredicted callee never runs native.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    NativeGuardClosureId {
        closure: Reg,
        expected: usize,
    },
    /// Synthetic, native-JIT-only id read (polymorphic inline cache). NEVER
    /// emitted by the lowerer and NEVER executed by the interpreter — synthesized
    /// only inside [`native_inline_leaf_calls`] and consumed solely by
    /// [`translate_to_native_jit`]. Lowers to a [`vm_jit::JitInstr::ClosureId`]:
    /// reads `closure`'s underlying function id once into the `Int` register `dst`,
    /// which the dispatcher then compares against each speculated callee key with
    /// ordinary integer compare/branch instructions (the no-match arm bails via the
    /// existing re-run-from-top fallback). `closure` is a parameter handle.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    NativeClosureId {
        dst: Reg,
        closure: Reg,
    },
    /// Synthetic, native-JIT-only capture materialization (OSR × profile-guided inlining capturing-
    /// closure inlining). NEVER emitted by the lowerer and NEVER executed by the
    /// interpreter — synthesized only inside [`native_inline_leaf_calls`] (right
    /// after a [`NativeGuardClosureId`]) and consumed solely by
    /// [`translate_to_native_jit`]. Lowers to a [`vm_jit::JitInstr::HostCall`] with
    /// [`vm_jit::HostHelper::ClosureCapture`]: reads the scalar bits of capture
    /// `index` of the param-handle `closure` into `dst` (the inlined callee body's
    /// capture register `base + index`). A
    /// non-scalar/out-of-range capture bails out-of-band (defensive — the inline
    /// gate only fires for profiled-scalar captures).
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    NativeClosureCapture {
        dst: Reg,
        closure: Reg,
        index: usize,
    },
    /// Synthetic, native-JIT-only fused closure-id read from a heap struct/variant
    /// field. This is equivalent to:
    ///
    /// ```text
    /// tmp = GetFieldSlot(base, slot)   // heap-valued closure field
    /// dst = NativeClosureId(tmp)
    /// ```
    ///
    /// but avoids materializing the intermediate heap handle when `tmp` is only used
    /// for closure metadata reads.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    NativeFieldClosureId {
        dst: Reg,
        base: Reg,
        slot: usize,
    },
    /// Synthetic, native-JIT-only fused closure-capture read from a heap
    /// struct/variant field. See [`RegInstr::NativeFieldClosureId`].
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    NativeFieldClosureCapture {
        dst: Reg,
        base: Reg,
        slot: usize,
        index: usize,
    },
    ListFilter {
        dst: Reg,
        list: Reg,
        predicate: Reg,
    },
    ListFold {
        dst: Reg,
        list: Reg,
        state: Reg,
        folder: Reg,
    },
    ListGet {
        dst: Reg,
        list: Reg,
        index: Reg,
    },
    ListLen {
        dst: Reg,
        list: Reg,
    },
    ListMap {
        dst: Reg,
        list: Reg,
        mapper: Reg,
    },
    ListAppend {
        dst: Reg,
        list: Reg,
        values: Reg,
    },
    ListClear {
        dst: Reg,
        list: Reg,
    },
    ListPop {
        dst: Reg,
        list: Reg,
    },
    ListPush {
        dst: Reg,
        list: Reg,
        value: Reg,
    },
    ListRemoveAt {
        dst: Reg,
        list: Reg,
        index: Reg,
    },
    ListSet {
        dst: Reg,
        list: Reg,
        index: Reg,
        value: Reg,
    },
    ListSort {
        dst: Reg,
        list: Reg,
    },
    ListSortBy {
        dst: Reg,
        list: Reg,
        key: Reg,
        compare: Reg,
    },
    ListSortWith {
        dst: Reg,
        list: Reg,
        compare: Reg,
    },
    DequeClear {
        dst: Reg,
        deque: Reg,
    },
    DequePopBack {
        dst: Reg,
        deque: Reg,
    },
    DequePopFront {
        dst: Reg,
        deque: Reg,
    },
    DequePushBack {
        dst: Reg,
        deque: Reg,
        value: Reg,
    },
    DequePushFront {
        dst: Reg,
        deque: Reg,
        value: Reg,
    },
    SetClear {
        dst: Reg,
        set: Reg,
    },
    SetForEach {
        dst: Reg,
        set: Reg,
        callback: Reg,
    },
    SetInsert {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SetRemove {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SortedSetClear {
        dst: Reg,
        set: Reg,
    },
    SortedSetInsert {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SortedSetRemove {
        dst: Reg,
        set: Reg,
        value: Reg,
    },
    SortedMapClear {
        dst: Reg,
        map: Reg,
    },
    SortedMapInsert {
        dst: Reg,
        map: Reg,
        key: Reg,
        value: Reg,
    },
    SortedMapRemove {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    MapGet {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    MapClear {
        dst: Reg,
        map: Reg,
    },
    MapInsertOld {
        dst: Reg,
        map: Reg,
        key: Reg,
        value: Reg,
    },
    MapRemove {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    BufferClear {
        dst: Reg,
        buffer: Reg,
    },
    MapInsert {
        dst: Reg,
        map: Reg,
        key: Reg,
        value: Reg,
    },
    StringBuilderPush {
        dst: Reg,
        builder: Reg,
        value: Reg,
    },
    StringBuilderFinish {
        dst: Reg,
        builder: Reg,
    },
    StringConcat {
        dst: Reg,
        left: Reg,
        right: Reg,
    },
    CallIntrinsic {
        dst: Reg,
        intrinsic: RegIntrinsic,
        args: Vec<Reg>,
    },
    CallTypedIntrinsic {
        dst: Reg,
        intrinsic: RegIntrinsic,
        type_arg: String,
        args: Vec<Reg>,
    },
    TryResult {
        dst: Reg,
        src: Reg,
        cleanup: Vec<Reg>,
    },
    Return {
        src: Reg,
    },
}

include!(concat!(env!("OUT_DIR"), "/rss-reg-intrinsic-enum.rs"));

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(crate) enum RegIntCompare {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

pub(crate) type Reg = usize;
