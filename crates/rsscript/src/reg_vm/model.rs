use super::*;

#[derive(Debug, Clone)]
pub(crate) struct RegUnit {
    pub(crate) functions: Vec<Rc<RegFunction>>,
    pub(crate) function_ids: HashMap<String, usize>,
    pub(crate) resource_drop_functions: HashMap<String, usize>,
    pub(crate) types: HashMap<String, TypeInfo>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegNativeSignature {
    pub(crate) params: Vec<String>,
    pub(crate) return_type: Option<String>,
}

/// Call count at which a function becomes "warm" and starts collecting a
/// [`FunctionProfile`] (J1). Below this threshold a function allocates and
/// records nothing — cold code pays only a single saturating `Cell<u32>`
/// increment at frame entry. Tuned high enough that one-shot/setup functions
/// never profile, low enough that a genuinely hot dispatcher is observed within
/// the first handful of native-tier warm-ups.
pub(crate) const PROFILE_WARMUP: u32 = 50;

/// Per-function dynamic-call count at which J1 stops sampling: once a function's
/// `call_count` reaches this, [`record_call_site`] freezes (a single `Cell`
/// read + compare, then return) so a dynamic call driven by a hot loop has an
/// essentially-free steady state. The window `PROFILE_WARMUP..PROFILE_RECORD_LIMIT`
/// is more than enough samples to settle every site's mono/poly/mega state.
pub(crate) const PROFILE_RECORD_LIMIT: u32 = PROFILE_WARMUP + 256;

/// Minimum branch samples before branch feedback is strong enough to guide J2
/// speculation. Reporting can show smaller samples, but codegen should not treat
/// them as a stable bias.
pub(crate) const PROFILE_BRANCH_MIN_SAMPLES: u32 = 16;

/// Branch edge share required before a direction is considered hot.
pub(crate) const PROFILE_BRANCH_HOT_NUMERATOR: u32 = 9;
pub(crate) const PROFILE_BRANCH_HOT_DENOMINATOR: u32 = 10;

/// Maximum number of distinct callee identities tracked at one dynamic call
/// site before it is declared megamorphic. Past this the observed list stops
/// growing (bounded memory) and [`MonoState::Megamorphic`] sticks.
pub(crate) const PROFILE_MAX_CALLEES: usize = 4;

/// Per-call-site monomorphism state, derived from the number of *distinct*
/// callee identities observed at a dynamic call site.
///
/// Feeds J2 monomorphic-inlining COMPILE DECISIONS ONLY; it never feeds a
/// computed value and never alters control flow or results (determinism).
// Read by the J1 tests and J2 profile-guided native inliner; not consumed by
// production interpreter dispatch, which only *writes* feedback.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MonoState {
    /// Exactly one distinct callee observed so far — inlinable.
    Monomorphic,
    /// Two or three distinct callees — a small polymorphic set.
    Polymorphic,
    /// More than [`PROFILE_MAX_CALLEES`] distinct callees — not inlinable.
    Megamorphic,
}

/// Compiler-facing classification of dynamic branch feedback.
///
/// Only `TakenHot` and `FallthroughHot` are actionable for speculative native
/// transforms. The other states are still useful in reports and tests, but should
/// not drive codegen decisions.
#[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchBias {
    NoSamples,
    UnderSampled,
    TakenHot,
    FallthroughHot,
    Mixed,
}

#[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
impl BranchBias {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            BranchBias::NoSamples => "none",
            BranchBias::UnderSampled => "undersampled",
            BranchBias::TakenHot => "taken-hot",
            BranchBias::FallthroughHot => "fallthrough-hot",
            BranchBias::Mixed => "mixed",
        }
    }

    /// Returns the hot dynamic edge when this bias is strong enough for
    /// speculative native codegen. `true` means the explicit jump target is hot;
    /// `false` means the fallthrough edge is hot.
    #[allow(dead_code)]
    pub(crate) fn hot_edge(self) -> Option<bool> {
        match self {
            BranchBias::TakenHot => Some(true),
            BranchBias::FallthroughHot => Some(false),
            BranchBias::NoSamples | BranchBias::UnderSampled | BranchBias::Mixed => None,
        }
    }
}

/// Type feedback recorded at a single dynamic call site (`CallDynamic` /
/// `CallClosure`): the set of resolved callee identities and how often each was
/// seen. Counts saturate; the observed list is capped at
/// [`PROFILE_MAX_CALLEES`].
///
/// Drives J2 compile decisions ONLY — never a computed value (determinism).
#[derive(Debug, Clone)]
pub(crate) struct CallSiteFeedback {
    /// `(callee_key, saturating_count)` for each distinct callee, in first-seen
    /// order. `callee_key` is the callee's underlying function id (stable
    /// identity), so "same callee every time" reads as exactly one entry.
    pub(crate) observed: Vec<(u64, u32)>,
    /// `true` once a distinct callee beyond [`PROFILE_MAX_CALLEES`] was seen, so
    /// the site is permanently megamorphic even though `observed` is capped.
    pub(crate) overflowed: bool,
    /// `false` once ANY observation at this site saw a closure with a non-scalar
    /// (heap) capture. Capturing-closure inlining (OSR × J2) materializes captures
    /// as scalars via the `closure_capture` host helper, so a site that ever saw a
    /// heap capture is not eligible — the gate then leaves it on the interpreter
    /// path (no inline, no OSR). Starts `true`; ANDed monotonically downward.
    pub(crate) captures_all_scalar: bool,
}

impl Default for CallSiteFeedback {
    fn default() -> Self {
        CallSiteFeedback {
            observed: Vec::new(),
            overflowed: false,
            captures_all_scalar: true,
        }
    }
}

impl CallSiteFeedback {
    /// Record one observation of `callee_key` (saturating). Pure bookkeeping:
    /// has no effect on the call dispatch decision or any value.
    pub(crate) fn record(&mut self, callee_key: u64, captures_scalar: bool) {
        // Monotone AND: one heap-capture observation disqualifies the site forever.
        self.captures_all_scalar &= captures_scalar;
        if let Some(entry) = self.observed.iter_mut().find(|(key, _)| *key == callee_key) {
            entry.1 = entry.1.saturating_add(1);
            return;
        }
        if self.observed.len() >= PROFILE_MAX_CALLEES {
            // Bounded memory: stop growing and remember we saw more than the cap.
            self.overflowed = true;
            return;
        }
        self.observed.push((callee_key, 1));
    }

    /// Monomorphism state derived from the distinct-callee count. Read by the J1
    /// tests and the forthcoming J2 inliner.
    #[allow(dead_code)]
    pub(crate) fn state(&self) -> MonoState {
        if self.overflowed || self.observed.len() > PROFILE_MAX_CALLEES {
            MonoState::Megamorphic
        } else if self.observed.len() <= 1 {
            MonoState::Monomorphic
        } else {
            MonoState::Polymorphic
        }
    }
}

/// Dynamic branch feedback for one conditional branch site. `taken` means the
/// branch jumped to its explicit target; `fallthrough` means execution continued
/// at the next instruction.
#[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
#[derive(Debug, Clone, Default)]
pub(crate) struct BranchFeedback {
    pub(crate) taken: u32,
    pub(crate) fallthrough: u32,
}

#[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
impl BranchFeedback {
    pub(crate) fn record(&mut self, taken: bool) {
        if taken {
            self.taken = self.taken.saturating_add(1);
        } else {
            self.fallthrough = self.fallthrough.saturating_add(1);
        }
    }

    pub(crate) fn total(&self) -> u32 {
        self.taken.saturating_add(self.fallthrough)
    }

    pub(crate) fn taken_percent(&self) -> f64 {
        let total = self.total();
        if total == 0 {
            0.0
        } else {
            (self.taken as f64 * 100.0) / total as f64
        }
    }

    pub(crate) fn bias(&self) -> BranchBias {
        let total = self.total();
        if total == 0 {
            return BranchBias::NoSamples;
        }
        if total < PROFILE_BRANCH_MIN_SAMPLES {
            return BranchBias::UnderSampled;
        }

        let hot_num = u64::from(PROFILE_BRANCH_HOT_NUMERATOR);
        let hot_den = u64::from(PROFILE_BRANCH_HOT_DENOMINATOR);
        let total = u64::from(total);
        if u64::from(self.taken).saturating_mul(hot_den) >= total.saturating_mul(hot_num) {
            BranchBias::TakenHot
        } else if u64::from(self.fallthrough).saturating_mul(hot_den)
            >= total.saturating_mul(hot_num)
        {
            BranchBias::FallthroughHot
        } else {
            BranchBias::Mixed
        }
    }

    #[allow(dead_code)]
    pub(crate) fn hot_edge(&self) -> Option<bool> {
        self.bias().hot_edge()
    }
}

/// Per-function type-feedback profile (J1): feedback for each dynamic call site,
/// keyed by the site's instruction index within the function's `code`.
///
/// Allocated lazily once a function crosses [`PROFILE_WARMUP`]; cold functions
/// never allocate one. Consumed by J2 monomorphic inlining to decide what to
/// compile — it NEVER feeds a computed value and NEVER changes program behavior
/// (determinism is non-negotiable).
#[derive(Debug, Clone, Default)]
pub(crate) struct FunctionProfile {
    pub(crate) call_sites: HashMap<usize, CallSiteFeedback>,
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) branch_sites: HashMap<usize, BranchFeedback>,
}

impl FunctionProfile {
    /// Record `callee_key` at the dynamic call site whose instruction index is
    /// `instr_idx`. Observation only — never affects dispatch or values.
    fn record_call(&mut self, instr_idx: usize, callee_key: u64, captures_scalar: bool) {
        self.call_sites
            .entry(instr_idx)
            .or_default()
            .record(callee_key, captures_scalar);
    }

    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    fn record_branch(&mut self, instr_idx: usize, taken: bool) {
        self.branch_sites
            .entry(instr_idx)
            .or_default()
            .record(taken);
    }

    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) fn branch_feedback(&self, instr_idx: usize) -> Option<&BranchFeedback> {
        self.branch_sites.get(&instr_idx)
    }

    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) fn branch_bias(&self, instr_idx: usize) -> BranchBias {
        self.branch_feedback(instr_idx)
            .map(BranchFeedback::bias)
            .unwrap_or(BranchBias::NoSamples)
    }

    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) fn branch_feedback_sites(&self) -> impl Iterator<Item = (usize, &BranchFeedback)> {
        self.branch_sites
            .iter()
            .map(|(instr_idx, feedback)| (*instr_idx, feedback))
    }
}

/// Warm-gated, bounded J1 type-feedback recording. Called ONLY from the
/// `CallDynamic`/`CallClosure` interpreter handlers (the sole sites resolving a
/// dynamic callee); nothing is added to `try_exec_pure`, the frame-entry path,
/// or any branch/loop body, so the hot dispatch path is untouched.
///
/// Cost by phase, all driven off one `Cell<u32>` (`func.call_count`):
/// - **cold** (`count < PROFILE_WARMUP`): a single saturating `Cell` bump, no
///   allocation, no `RefCell` touch. A site executed only a few times (e.g. a
///   `main` that calls a closure once) never profiles.
/// - **warm-up crossing** (`count == PROFILE_WARMUP`): allocate the
///   `FunctionProfile` once.
/// - **sampling** (`PROFILE_WARMUP <= count < PROFILE_RECORD_LIMIT`): record the
///   resolved `callee_key`.
/// - **frozen** (`count >= PROFILE_RECORD_LIMIT`): a single `Cell` read +
///   compare, then return. A site driven by a hot loop (the dynamic call IS the
///   loop body) reaches this after a fixed sample budget, so its steady state
///   costs essentially nothing — this is why it does not regress the interpreter.
///
/// `PROFILE_RECORD_LIMIT` samples are far more than enough to settle the
/// mono/poly/mega classification J2 needs. Observation only: feeds J2 compile
/// decisions, never a computed value and never control flow (determinism).
pub(crate) fn record_call_site(
    func: &RegFunction,
    instr_idx: usize,
    callee_key: u64,
    captures_scalar: bool,
) {
    let count = func.call_count.get();
    if count >= PROFILE_RECORD_LIMIT {
        // Frozen: enough samples collected. Cheapest possible steady state.
        return;
    }
    func.call_count.set(count.saturating_add(1));
    if count < PROFILE_WARMUP {
        // Still cold (one bump above is the whole cost).
        if count + 1 == PROFILE_WARMUP {
            // Allocate the profile exactly once, on the warm-up crossing.
            if let Ok(mut slot) = func.profile.try_borrow_mut() {
                if slot.is_none() {
                    *slot = Some(Box::new(FunctionProfile::default()));
                }
            }
        }
        return;
    }
    // Warm and within the sample budget: record this observation.
    if let Ok(mut slot) = func.profile.try_borrow_mut() {
        if let Some(profile) = slot.as_mut() {
            profile.record_call(instr_idx, callee_key, captures_scalar);
        }
    }
}

/// Warm-gated, bounded branch-feedback recording. This is deliberately separate
/// from [`record_call_site`]: branch profiling must not advance the dynamic-call
/// counter used by profile-guided closure inlining and OSR re-probe logic.
#[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
pub(crate) fn record_branch_site(func: &RegFunction, instr_idx: usize, taken: bool) {
    let count = func.branch_count.get();
    if count >= PROFILE_RECORD_LIMIT {
        return;
    }
    func.branch_count.set(count.saturating_add(1));
    if count < PROFILE_WARMUP {
        if count + 1 == PROFILE_WARMUP {
            if let Ok(mut slot) = func.profile.try_borrow_mut() {
                if slot.is_none() {
                    *slot = Some(Box::new(FunctionProfile::default()));
                }
            }
        }
        return;
    }
    if let Ok(mut slot) = func.profile.try_borrow_mut() {
        if let Some(profile) = slot.as_mut() {
            profile.record_branch(instr_idx, taken);
        }
    }
}

/// Whether every capture of `closure` is a scalar (`Int`/`Float`/`Bool`) — the
/// precondition for materializing captures into an inlined native body via the
/// `closure_capture` host helper. A non-scalar (heap) capture makes the
/// capturing-closure inline ineligible; a `Managed` wrapper is unwrapped first.
pub(crate) fn closure_captures_all_scalar(closure: &VmClosure) -> bool {
    closure.captures.iter().all(|c| {
        fn scalar(v: &VmValue) -> bool {
            match v {
                VmValue::Int(_) | VmValue::Float(_) | VmValue::Bool(_) => true,
                VmValue::Managed(inner) => scalar(&inner.borrow()),
                _ => false,
            }
        }
        scalar(c)
    })
}

fn scalar_param_type_needs_no_deep_copy(type_name: &str) -> bool {
    matches!(
        type_name,
        "Bool"
            | "Byte"
            | "Char"
            | "Float"
            | "Float32"
            | "Float64"
            | "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Unit"
    )
}

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
    /// Cached tier-0 JIT analysis `(all_instructions_supported, has_loop)`,
    /// computed once after `code` is emitted.
    pub(crate) jit_analysis: std::cell::Cell<Option<(bool, bool)>>,
    /// Cached verdict for the scalar self-recursive fast paths (native `CallSelf`
    /// + tier-0 i64 executor). `None` means not inspected yet; `Some(Ineligible)`
    /// is the hot-path negative cache for ordinary call-heavy functions; `Int`/`Bool`
    /// record the i64-representable return kind so the dispatcher wraps the result.
    pub(crate) jit_self_recursion_kind: std::cell::Cell<Option<SelfRecursionKind>>,
    /// Cached native-tier verdict, an invariant property of the function:
    /// `0` unknown, `1` known not native-eligible. Lets `try_native` skip all
    /// per-call tiering/cache/name-hash work once a function is known to never
    /// compile (so `jit-native` isn't slower than the VM on uncompilable code).
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) native_status: std::cell::Cell<u8>,
    /// J1 dynamic-call counter, bumped ONLY inside [`record_call_site`] (the
    /// `CallDynamic`/`CallClosure` handlers), never on the frame-entry path. Gates
    /// the warm-up ([`PROFILE_WARMUP`]) and sampling-budget
    /// ([`PROFILE_RECORD_LIMIT`]) phases; a function with no dynamic call site
    /// never reaches the helper, so its counter stays `0` and it pays nothing.
    /// Below `PROFILE_WARMUP` no profile is allocated.
    pub(crate) call_count: std::cell::Cell<u32>,
    /// J1 conditional-branch counter, bumped ONLY inside [`record_branch_site`].
    /// Kept separate from `call_count` so branch feedback cannot perturb closure
    /// speculation warm-up or OSR profile-progress checks.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) branch_count: std::cell::Cell<u32>,
    /// Lazily-allocated type-feedback profile, populated once `call_count`
    /// or `branch_count` crosses [`PROFILE_WARMUP`]. `None` for cold functions
    /// (zero allocation). Feeds J2/J4 compile decisions only — never a value
    /// (determinism).
    pub(crate) profile: RefCell<Option<Box<FunctionProfile>>>,
    /// OSR hot-backedge auto-trigger state (Pending #2). Lazily resolved ONCE on
    /// first `drive` entry into this function: a cheap single-natural-loop
    /// detection decides `NotCandidate` (no loop / unanalyzable — the common case,
    /// which then pays nothing per-instruction) vs `Counting` (has a candidate
    /// header). For a candidate, the interpreter counts backedges to the header and
    /// at [`OSR_BACKEDGE_THRESHOLD`] calls `try_osr` (the real detect+compile); on
    /// success the loop runs native and the counter cost is bounded to the warm-up
    /// iterations, on failure the state goes `GaveUp` (never retried). A `Cell`
    /// (interior-mut, no allocation), so a non-candidate function pays one `Cell`
    /// read per call and zero per-instruction cost.
    #[cfg_attr(not(feature = "native-jit"), allow(dead_code))]
    pub(crate) osr_state: std::cell::Cell<OsrTrigger>,
}

/// Per-function OSR auto-trigger state machine (Pending #2). Lives in a `Cell` on
/// [`RegFunction`]; see `osr_state`.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OsrTrigger {
    /// Not yet inspected — resolved on first `drive` entry.
    Unknown,
    /// No qualifying single natural loop (or step/cancel budget armed): pays only a
    /// single hoisted `Cell` read per call, NO per-instruction cost.
    NotCandidate,
    /// Has a candidate loop header; the interpreter counts backedges to `header_ip`.
    /// At [`OSR_BACKEDGE_THRESHOLD`] it fires `try_osr`. `probe_cc` is the function's
    /// dynamic-call count (`call_count`) as of the LAST `try_osr` probe (0 before the
    /// first), used to gate re-probes: a pending-profile decline only resets the counter
    /// if the profile has ADVANCED (`call_count` increased) since then — otherwise the
    /// site is dynamically dead/stalled and we `GaveUp`. `call_count` is capped at
    /// `PROFILE_RECORD_LIMIT`, so the number of progress-resets is bounded.
    Counting {
        header_ip: usize,
        count: u32,
        probe_cc: u32,
    },
    /// OSR fired (or `try_osr` declined at threshold): stop counting. `GaveUp` and
    /// `Fired` collapse to the same terminal "do nothing" behavior, but are kept
    /// distinct for telemetry/clarity.
    GaveUp,
}

/// Mirror of `OsrTrigger` that is always present (so the non-`native-jit`
/// `RegFunction` constructor compiles). Only the `native-jit` build reads it.
#[cfg(not(feature = "native-jit"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OsrTrigger {
    Unknown,
}

impl RegFunction {
    pub(crate) fn placeholder(name: String) -> Self {
        Self {
            name,
            params: 0,
            captures: 0,
            regs: 0,
            local_regs: HashMap::new(),
            code: Vec::new(),
            jit_analysis: std::cell::Cell::new(None),
            jit_self_recursion_kind: std::cell::Cell::new(None),
            native_status: std::cell::Cell::new(0),
            call_count: std::cell::Cell::new(0),
            branch_count: std::cell::Cell::new(0),
            profile: RefCell::new(None),
            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
        }
    }
}

#[derive(Debug, Clone)]
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
    Move {
        dst: Reg,
        src: Reg,
    },
    /// Replace `reg` with a deep copy of its value (fresh `Rc` for every mutable
    /// container in the tree, recursing through structs/variants/options; shared
    /// reference values like `Managed` keep their handle). Emitted at the function
    /// prologue for every non-`mut` parameter so the callee owns an isolated copy,
    /// mirroring the Rust backend, which passes `mut` as `&mut` (mutations
    /// propagate) and everything else by value/`&` + an inserted `.clone()`.
    DeepCopy {
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
    /// how capability objects and generic protocol bounds dispatch in the VM,
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
    /// `select { ... }`: each `handles` reg holds a spawned arm task. Park until
    /// the first arm finishes, then write its index to `winner` and its value to
    /// `value`; a branch ladder afterwards dispatches to the winning arm's body.
    SelectWait {
        handles: Vec<Reg>,
        winner: Reg,
        value: Reg,
    },
    CallNative {
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
    /// Synthetic, native-JIT-only guard (J2 profile-guided monomorphic inlining).
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
    /// Synthetic, native-JIT-only id read (J2.2 polymorphic inline cache). NEVER
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
    /// Synthetic, native-JIT-only capture materialization (OSR × J2 capturing-
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
    CounterAdd {
        dst: Reg,
        counter: Reg,
        amount: Reg,
    },
    ConfigStoreReplace {
        dst: Reg,
        store: Reg,
        value: Reg,
    },
    GlobalConfigReplace {
        dst: Reg,
        global: Reg,
        value: Reg,
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

#[derive(Debug, Clone, Copy)]
pub(crate) enum RegIntrinsic {
    ArgsAll,
    ArgsCount,
    ArgsGet,
    ArgsGetOrDefault,
    AssertEqual,
    AssertEqualBool,
    AssertEqualInt,
    Base64Decode,
    Base64DecodeString,
    Base64Encode,
    Base64EncodeBytes,
    BytesConcat,
    BytesConsume,
    BytesFromString,
    BytesFromUints,
    BytesIsEmpty,
    BytesLen,
    BytesSlice,
    BytesToString,
    BytesToUints,
    BytesViewStartsWith,
    BytesViewToBytes,
    BufferNew,
    CacheGet,
    CacheLookup,
    CancellationSourceCancel,
    CancellationSourceNew,
    CancellationSourceToken,
    CancellationTokenIsCancelled,
    ChannelBounded,
    ChannelReceiver,
    ChannelSender,
    ChannelErrorMessage,
    TensorFromF32Slice,
    TensorToF32Slice,
    TensorShape,
    TensorRank,
    TensorF32ToLeBytes,
    TensorF32FromLeBytes,
    TensorMatmul,
    TensorMatmulMetal,
    TensorMetalAvailable,
    TensorMetalDeviceName,
    TensorGpuRunMsl,
    TensorAdd,
    TensorSub,
    TensorMul,
    TensorDiv,
    TensorNeg,
    TensorExp,
    TensorLog,
    TensorSqrt,
    TensorRelu,
    TensorSumAll,
    TensorSumAxis,
    TensorMaxAxis,
    TensorMeanAxis,
    TensorArgmaxAxis,
    TensorReshape,
    TensorTranspose,
    TensorPermute,
    TensorBroadcastTo,
    TensorCmplt,
    TensorCmpne,
    TensorCmpeq,
    TensorSelect,
    TensorMaximum,
    TensorMinimum,
    TensorCastF32,
    TensorCastI32,
    TensorCastBool,
    TensorDtypeCode,
    // movement+gather (ops B)
    TensorPad,
    TensorShrink,
    TensorFlip,
    TensorGather,
    // reductions+math (ops C)
    TensorProdAxis,
    TensorMinAxis,
    TensorSumAxes,
    TensorProdAxes,
    TensorMaxAxes,
    TensorMinAxes,
    TensorMeanAxes,
    TensorReciprocal,
    TensorExp2,
    TensorLog2,
    TensorRsqrt,
    TensorSin,
    TensorTrunc,
    TensorPow,
    // bmm+int/bit (ops D)
    TensorBmm,
    TensorIdiv,
    TensorMod,
    TensorFloordiv,
    TensorFloormod,
    TensorShl,
    TensorShr,
    TensorAnd,
    TensorOr,
    TensorXor,
    TensorBitcastF32ToI32,
    TensorBitcastI32ToF32,
    // rng (slice E)
    TensorRand,
    TensorRandint,
    TensorRandn,
    // nn (slice F)
    TensorIota,
    TensorOneHot,
    TensorSoftmax,
    TensorLogSoftmax,
    TensorCrossEntropy,
    // conv (slice G)
    TensorConv2d,
    TensorMaxPool2d,
    TensorAvgPool2d,
    // scatter
    TensorScatterAdd,
    TensorErrorMessage,
    CharCompare,
    CharFromCode,
    CharIsAlphanumeric,
    CharIsAlpha,
    CharIsDigit,
    CharIsLower,
    CharIsUpper,
    CharIsWhitespace,
    CharToCode,
    CharToLower,
    CharToString,
    CharToUpper,
    CloneClone,
    ClockNow,
    ClockSystemUnixMs,
    ConfigLoad,
    CapabilityFrom,
    ConfigName,
    ConfigNew,
    ConfigRuleCount,
    ConfigStoreName,
    ConfigStoreNew,
    CounterNew,
    CounterValue,
    CsvOpenRead,
    CsvParseRow,
    CsvReadInto,
    CsvRows,
    DateAddDays,
    DateAddMs,
    DateDay,
    DateDaysBetween,
    DateDaysInMonth,
    DateFormatIso,
    DateFormatYmd,
    DateHour,
    DateIsLeapYear,
    DateMinute,
    DateMonth,
    DateParseIso,
    DateParseYmd,
    DateSecond,
    DateStartOfDay,
    DateWeekday,
    DateYear,
    DecodeErrorMessage,
    DeadlineAfter,
    DeadlineAfterMs,
    DeadlineIsExpired,
    DeadlineRemainingMs,
    DequeIsEmpty,
    DequeLen,
    DequeNew,
    DequeToList,
    DiffUnified,
    DirectoryCopyFile,
    DirectoryCreate,
    DirectoryCreateAll,
    DirectoryCreateDirAll,
    DirectoryExists,
    DirectoryIsDir,
    DirectoryIsFile,
    DirectoryListFiles,
    DirectoryListPaths,
    DirectoryMetadata,
    DirectoryReadString,
    DirectoryRemoveDirAll,
    DirectoryRemoveFile,
    DirectoryRename,
    DirectoryWriteString,
    DbClose,
    DbConnectionOpen,
    DbConnectionQuery,
    DbConnectionTryOpen,
    DurationAdd,
    DurationAsMs,
    DurationAsSeconds,
    DurationMs,
    DurationSeconds,
    EnvironmentBindFunction,
    EnvironmentChild,
    EnvironmentHasFunction,
    EnvironmentHasParent,
    EnvironmentRoot,
    EnvCurrentDir,
    EnvGet,
    EnvGetOrDefault,
    EnvHomeDir,
    EnvRunWorkspaceRoot,
    EnvSet,
    EnvSetCurrentDir,
    EnvTempDir,
    FileAppendBytes,
    FileAppendString,
    FileBytesStream,
    FileExists,
    FileErrorMessage,
    FileOpen,
    FileOpenRead,
    FileOpenWrite,
    FileReadAll,
    FileReadAllAsync,
    FileReadAllString,
    FileReadAllStringAsync,
    FileReadBytes,
    FileReadInto,
    FileReadString,
    FileRemove,
    FileWrite,
    FileWriteAsync,
    FileWriteAtomic,
    FileWriteBytes,
    FileWriteBytesView,
    FileWriteBuffer,
    FileWriteBufferView,
    FileWriteString,
    FileWriteStringAsync,
    FileWriteStringToPath,
    FalliblePipelineCollect,
    FalliblePipelineEach,
    FalliblePipelineFilter,
    FalliblePipelineMap,
    FalliblePipelineTryMap,
    FunctionObjectHasClosure,
    FunctionObjectNew,
    HashSha256Bytes,
    HashSha256File,
    HashSha256String,
    HashSha3_224Bytes,
    HashSha3_256Bytes,
    HashShake128Bytes,
    HmacSha256Bytes,
    HmacSha256String,
    GlobalConfigNew,
    GlobalConfigRuleCount,
    GzipDecompressBytes,
    HexDecode,
    HexEncode,
    HexEncodeString,
    HttpErrorMessage,
    HttpGet,
    HttpGetAsync,
    HttpGetRetryAsync,
    HttpGetTimeoutAsync,
    HttpPostForm,
    HttpPostFormAsync,
    HttpPostJson,
    HttpPostJsonAsync,
    HttpPostJsonBearerRetryAsync,
    HttpPostJsonRetryAsync,
    HttpPostJsonTimeoutAsync,
    HttpSendAsync,
    HttpRequestJson,
    HttpRequestWithHeader,
    HttpRequestWithRetry,
    HttpRequestWithTimeout,
    HttpResponseBytes,
    HttpResponseIsSuccess,
    HttpResponseLines,
    HttpResponseStatus,
    HttpResponseText,
    ImageInspect,
    ImageLoad,
    ImageNormalize,
    ImageResize,
    ImageSave,
    ImageSharpen,
    InstantElapsed,
    FloatToString,
    FloatIsFinite,
    FloatIsInfinite,
    FloatIsNan,
    IntToString,
    IntToFloat,
    IntBitAnd,
    IntBitNot,
    IntBitOr,
    IntBitXor,
    IntShiftLeft,
    IntShiftRight,
    MathAbs,
    MathAbsFloat,
    MathCeil,
    MathClamp,
    MathClampFloat,
    MathCos,
    MathExp,
    MathExp2,
    MathFloor,
    MathLog,
    MathLog2,
    MathMax,
    MathMaxFloat,
    MathMin,
    MathMinFloat,
    MathPow,
    MathPowFloat,
    MathRound,
    MathSaturatingAdd,
    MathSaturatingMul,
    MathSaturatingSub,
    MathSin,
    MathSqrt,
    MathTanh,
    MathTruncFloat,
    MathWrappingAdd,
    MathWrappingMul,
    MathWrappingSub,
    JsonArray,
    JsonArrayBools,
    JsonArrayContainsPrefix,
    JsonArrayContainsString,
    JsonArrayContainsSubstring,
    JsonArrayCountWhere,
    JsonArrayFold,
    JsonArrayGet,
    JsonArrayInts,
    JsonArrayLen,
    JsonArrayStrings,
    JsonAt,
    JsonAtBool,
    JsonAtBoolOr,
    JsonAtInt,
    JsonAtIntOr,
    JsonAtOptional,
    JsonAtOptionalBool,
    JsonAtOptionalInt,
    JsonAtOptionalString,
    JsonAtOr,
    JsonAtString,
    JsonAtStringOr,
    JsonAtToString,
    JsonAtToStringOr,
    JsonAsBool,
    JsonAsInt,
    JsonAsString,
    JsonBoolAt,
    JsonBoolAtOr,
    JsonBoolField,
    JsonClone,
    JsonDecode,
    JsonDecodeText,
    JsonEncode,
    JsonErrorMessage,
    JsonField,
    JsonFieldBool,
    JsonFieldInt,
    /// Native-JIT internal: checked `Json.parse(...)?` payload helper.
    JsonParseOk,
    /// Native-JIT internal: checked `Json.field(...)?` payload helper.
    JsonFieldOk,
    /// Native-JIT internal: checked `Json.field_int(...)?` payload helper.
    JsonFieldIntOk,
    JsonFieldOptional,
    JsonFieldOptionalBool,
    JsonFieldOptionalInt,
    JsonFieldOptionalString,
    JsonFieldString,
    JsonIntAt,
    JsonIntAtOr,
    JsonIntField,
    JsonIsArray,
    JsonIsNull,
    JsonIsObject,
    JsonKind,
    JsonObject,
    JsonObjectKeys,
    JsonObjectLen,
    JsonParse,
    JsonParseFile,
    JsonQuoteString,
    JsonRawField,
    JsonStringAt,
    JsonStringAtOr,
    JsonStringArray,
    JsonStringField,
    JsonStrings,
    JsonToStringAt,
    JsonToStringAtOr,
    JsonToString,
    JsonValue,
    JsonValues,
    ListAll,
    ListAny,
    ListConsume,
    ListContains,
    ListContainsValue,
    ListCountWhere,
    ListEnumerate,
    ListFind,
    ListFlatMap,
    ListFlatten,
    ListFirst,
    ListGroupBy,
    ListIsEmpty,
    ListJoin,
    ListLast,
    ListDedup,
    ListMax,
    ListMin,
    ListNew,
    ListPartition,
    ListPipeline,
    ListReverse,
    ListSkip,
    ListSlice,
    ListSum,
    ListTake,
    ListZip,
    ListToJsonStrings,
    ListToJsonValues,
    ListTryFold,
    LogError,
    LogErrorJson,
    LogTrace,
    LogWrite,
    LogWriteJson,
    MapContainsKey,
    MapFilter,
    MapFold,
    MapForEach,
    MapGetOrDefault,
    MapIsEmpty,
    MapKeys,
    MapLen,
    MapMapValues,
    MapMerge,
    MapNew,
    MapTryFold,
    MapValues,
    OptionIsNone,
    OptionIsSome,
    OptionAndThen,
    OptionFilter,
    OptionMap,
    OptionOkOr,
    OptionOr,
    OptionUnwrapOr,
    OptionUnwrapOrElse,
    OrdCompare,
    OsClose,
    PatchApplyText,
    PathExists,
    PathExtension,
    PathFileName,
    PathFromString,
    PathIsAbsolute,
    PathIsDir,
    PathIsFile,
    PathJoin,
    PathListFiles,
    PathListPaths,
    PathNormalize,
    PathParent,
    PathReadString,
    PathResolveRelative,
    PathSafeRelative,
    PathStartsWith,
    PathToString,
    PathWithExtension,
    PathWriteString,
    PersistentMapClear,
    PersistentMapContainsKey,
    PersistentMapGet,
    PersistentMapInsert,
    PersistentMapIsEmpty,
    PersistentMapLen,
    PersistentMapNew,
    PersistentMapRemove,
    PipelineCollect,
    PipelineEach,
    PoolErrorMessage,
    PoolStatsAvailable,
    PoolStatsCapacity,
    PoolStatsCreated,
    PoolStatsInUse,
    PipelineTryMap,
    ProcessRun,
    ProcessRunAsync,
    ProcessRunManyStdout,
    ProcessRunManyStdoutAsync,
    ProcessRunManyStdoutTimeout,
    ProcessRunManyStdoutTimeoutAsync,
    ProcessRunRequest,
    ProcessRunRequestAsync,
    ProcessRunRequestCancellableAsync,
    ProcessRunStdout,
    ProcessRunStdoutAsync,
    ProcessRunStdoutTimeout,
    ProcessRunStdoutTimeoutAsync,
    ProcessRunTimeout,
    ProcessRunTimeoutAsync,
    ProcessStream,
    RandomBool,
    RandomBytes,
    RandomFloat,
    RandomInt,
    RandomString,
    RegexCaptures,
    RegexCompile,
    RegexErrorMessage,
    RegexFind,
    RegexIsMatch,
    RegexReplaceAll,
    RegexSplit,
    ResultErr,
    ResultErrMessage,
    ResultAndThen,
    ResultIsErr,
    ResultIsOk,
    ResultMap,
    ResultMapError,
    ResultOk,
    ResultUnwrapOr,
    ResultUnwrapOrElse,
    RequestNew,
    RequestPath,
    ReceiverClose,
    ReceiverIntoStream,
    ReceiverRecv,
    ReceiverRecvCancellable,
    ResponseBody,
    ResponseOk,
    ResponseStatus,
    RowBufferNew,
    RowFieldString,
    RuleLoaderLoadRules,
    ResourcePoolBorrow,
    ResourcePoolDiscard,
    ResourcePoolLazy,
    ResourcePoolNew,
    ResourcePoolStats,
    ResourcePoolTryBorrow,
    ResourcePoolTryLazy,
    ResourcePoolTryNew,
    SetContains,
    SetDifference,
    SetIntersection,
    SetIsEmpty,
    SetIsSubset,
    SetLen,
    SetNew,
    SetToList,
    SetUnion,
    SortedSetContains,
    SortedSetIsEmpty,
    SortedSetLen,
    SortedSetNew,
    SortedSetToList,
    SortedMapContainsKey,
    SortedMapGet,
    SortedMapIsEmpty,
    SortedMapKeys,
    SortedMapLen,
    SortedMapNew,
    SortedMapValues,
    StringAfter,
    StringBefore,
    StringBuilderNew,
    StringCharAt,
    StringChars,
    StringContains,
    StringCount,
    StringCopy,
    StringEndsWith,
    StringFromBool,
    StringFromFloat,
    StringFormat,
    StringIndexOf,
    StringFromInt,
    StringIsEmpty,
    StringJoin,
    StringLines,
    StringLen,
    StringPadLeft,
    StringPadRight,
    StringParseFloat,
    StringParseInt,
    StringRepeat,
    StringReplace,
    StringReplaceFirst,
    StringReverse,
    StringSlice,
    StringSplit,
    StringStartsWith,
    StringStripPrefix,
    StringToLowercase,
    StringToUppercase,
    StringTrim,
    StringTrimEnd,
    StringTrimStart,
    StreamCollectList,
    StreamFromList,
    StreamNext,
    SenderClose,
    SenderSend,
    SenderSendCancellable,
    TcpConnect,
    TcpErrorMessage,
    TcpStreamRead,
    TcpStreamShutdown,
    TcpStreamWrite,
    TcpStreamWriteAll,
    TempDirKeep,
    TempDirNew,
    TempDirNewIn,
    TempDirPath,
    TomlParseFile,
    UuidNewV4,
    UrlDecodeComponent,
    UrlEncodeComponent,
    UrlFromString,
    UrlToString,
    TimerSleep,
    TimerSleepCancellable,
    TimerSleepUntil,
    WebSocketClose,
    WebSocketConnect,
    WebSocketErrorMessage,
    WebSocketRecvBytes,
    WebSocketRecvText,
    WebSocketSendBytes,
    WebSocketSendText,
    YamlParse,
    YamlParseFile,
    WeakDowngrade,
    WeakFrom,
    WeakUpgrade,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum RegIntCompare {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

pub(crate) type Reg = usize;

#[derive(Debug, Default)]
pub(crate) struct LoopPatch {
    pub(crate) breaks: Vec<usize>,
    pub(crate) continues: Vec<usize>,
    pub(crate) cleanup_base: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum MatchFailurePatch {
    Jump(usize),
    OptionSome(usize),
    OptionNone(usize),
    ResultOk(usize),
    ResultErr(usize),
    VariantOther(usize),
}

impl RegUnit {
    pub(crate) fn lower(hir: &Hir) -> Result<Self, EvalError> {
        let names = hir
            .function_bodies()
            .filter_map(|(name, body)| body.block.as_ref().map(|_| name.to_string()))
            .collect::<Vec<_>>();
        let function_ids = names
            .iter()
            .enumerate()
            .map(|(id, name)| (name.clone(), id))
            .collect::<HashMap<_, _>>();
        let mut functions = names
            .iter()
            .cloned()
            .map(RegFunction::placeholder)
            .collect::<Vec<_>>();
        let mut native_signatures = HashMap::new();
        // Whole-program closure-identity gate, OR-accumulated across every
        // function (and drop-body) lowering below.
        let closure_identity_observable = std::cell::Cell::new(false);
        for (function_id, name) in names.into_iter().enumerate() {
            let body = hir
                .function_body(&name)
                .and_then(|body| body.block.as_ref())
                .ok_or_else(|| {
                    EvalError::Runtime(format!("reg VM cannot find function `{name}`."))
                })?;
            let signature = hir.resolve_function(None, &name).ok_or_else(|| {
                EvalError::Runtime(format!("reg VM cannot resolve function `{name}`."))
            })?;
            native_signatures.insert(
                name.clone(),
                RegNativeSignature {
                    params: signature
                        .params
                        .iter()
                        .map(|param| param.type_name.clone())
                        .collect(),
                    return_type: signature.return_type.clone(),
                },
            );
            let mut lowerer = RegLowerer {
                hir,
                function_ids: &function_ids,
                functions: &mut functions,
                function: RegFunction {
                    name,
                    params: signature.params.len(),
                    captures: 0,
                    regs: 0,
                    local_regs: HashMap::new(),
                    code: Vec::new(),
                    jit_analysis: std::cell::Cell::new(None),
                    jit_self_recursion_kind: std::cell::Cell::new(None),
                    native_status: std::cell::Cell::new(0),
                    call_count: std::cell::Cell::new(0),
                    branch_count: std::cell::Cell::new(0),
                    profile: RefCell::new(None),
                    osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
                },
                loop_stack: Vec::new(),
                cleanup_stack: Vec::new(),
                closure_identity_observable: &closure_identity_observable,
            };
            for param in &signature.params {
                let reg = lowerer.local(&param.name);
                // `mut` params alias the caller's value (the backend lowers them to
                // `&mut`), so mutations must propagate. Non-mut heap/value params
                // keep copy isolation; primitive scalars are already independent.
                if param.effect != Some(ParamEffect::Mut)
                    && !scalar_param_type_needs_no_deep_copy(&param.type_name)
                {
                    lowerer.emit(RegInstr::DeepCopy { reg });
                }
            }
            lowerer.block(body)?;
            let unit = lowerer.temp();
            lowerer.emit(RegInstr::LoadUnit { dst: unit });
            lowerer.emit(RegInstr::Return { src: unit });
            functions[function_id] = lowerer.function;
        }
        let mut resource_drop_functions = HashMap::new();
        for (type_name, body) in hir.resource_drop_bodies() {
            let function_id = functions.len();
            functions.push(RegFunction::placeholder(format!("<drop:{type_name}>")));
            let mut lowerer = RegLowerer {
                hir,
                function_ids: &function_ids,
                functions: &mut functions,
                function: RegFunction {
                    name: format!("<drop:{type_name}>"),
                    params: 0,
                    captures: 0,
                    regs: 0,
                    local_regs: HashMap::new(),
                    code: Vec::new(),
                    jit_analysis: std::cell::Cell::new(None),
                    jit_self_recursion_kind: std::cell::Cell::new(None),
                    native_status: std::cell::Cell::new(0),
                    call_count: std::cell::Cell::new(0),
                    branch_count: std::cell::Cell::new(0),
                    profile: RefCell::new(None),
                    osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
                },
                loop_stack: Vec::new(),
                cleanup_stack: Vec::new(),
                closure_identity_observable: &closure_identity_observable,
            };
            if let Some(info) = hir.type_info(type_name) {
                for field in &info.fields_ordered {
                    lowerer.local(&field.name);
                }
            }
            lowerer.block(body)?;
            let unit = lowerer.temp();
            lowerer.emit(RegInstr::LoadUnit { dst: unit });
            lowerer.emit(RegInstr::Return { src: unit });
            functions[function_id] = lowerer.function;
            resource_drop_functions.insert(type_name.to_string(), function_id);
        }
        // Self-tail-call optimization: rewrite a self-tail-call (a `CallKnown`
        // to this same function whose result is returned directly) into an
        // arg-rebind + backward `Jump`, turning self-tail-recursion into a loop.
        // Run BEFORE `compute_jit_eligibility` so the now-self-edge-free function
        // is seen as non-recursive and becomes native-eligible. Conservative: only
        // genuine tail position, no `mut`-args, self-only, and never a function
        // whose ONLY exits are self-tail-calls (no non-tail return) — see
        // `optimize_self_tail_calls` for the soundness gates, including how the
        // recursion-depth-limit (`VmLimits::max_depth`) observability is preserved.
        for (function_id, function) in functions.iter_mut().enumerate() {
            optimize_self_tail_calls(function, function_id);
        }
        // Cache tier-0 JIT analysis `(eligible, has_loop)` per function. Eligible
        // is the unit-wide non-suspending + non-recursive fixpoint, so it accounts
        // for cross-function calls; `has_loop` gates whether the production
        // heuristic bothers JIT-ing a given eligible function.
        let eligibility = compute_jit_eligibility(&functions);
        for (function, &eligible) in functions.iter().zip(&eligibility) {
            let has_loop = jit_function_has_loop(&function.code);
            function.jit_analysis.set(Some((eligible, has_loop)));
        }
        #[cfg(feature = "native-jit")]
        mark_predictably_native_ineligible(&functions);
        Ok(Self {
            functions: functions.into_iter().map(Rc::new).collect(),
            function_ids,
            resource_drop_functions,
            types: hir
                .types()
                .map(|type_info| (type_info.name.clone(), type_info.clone()))
                .collect(),
            native_signatures,
            closure_identity_observable: closure_identity_observable.get(),
        })
    }
}
