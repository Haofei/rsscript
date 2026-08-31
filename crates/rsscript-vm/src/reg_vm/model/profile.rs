use super::super::*;


/// Minimum branch samples before branch feedback is strong enough to guide profile-guided inlining
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
/// Feeds profile-guided inlining monomorphic-inlining COMPILE DECISIONS ONLY; it never feeds a
/// computed value and never alters control flow or results (determinism).
// Read by the bounded profile collection tests and profile-guided native inliner; not consumed by
// production interpreter dispatch, which only *writes* feedback.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BranchBias {
    NoSamples,
    UnderSampled,
    TakenHot,
    FallthroughHot,
    Mixed,
}

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
/// Drives profile-guided inlining compile decisions ONLY — never a computed value (determinism).
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
    /// (heap) capture. Capturing-closure inlining (OSR × profile-guided inlining) materializes captures
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

    /// Monomorphism state derived from the distinct-callee count. Read by the bounded profile collection
    /// tests and the forthcoming profile-guided inlining inliner.
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
#[derive(Debug, Clone, Default)]
pub(crate) struct BranchFeedback {
    pub(crate) taken: u32,
    pub(crate) fallthrough: u32,
}

impl BranchFeedback {

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

    pub(crate) fn hot_edge(&self) -> Option<bool> {
        self.bias().hot_edge()
    }
}

/// Per-function type-feedback profile (bounded profile collection): feedback for each dynamic call site,
/// keyed by the site's instruction index within the function's `code`.
///
/// Allocated lazily once a function crosses [`PROFILE_WARMUP`]; cold functions
/// never allocate one. Consumed by profile-guided inlining monomorphic inlining to decide what to
/// compile — it NEVER feeds a computed value and NEVER changes program behavior
/// (determinism is non-negotiable).
#[derive(Debug, Clone, Default)]
pub(crate) struct FunctionProfile {
    pub(crate) call_sites: HashMap<usize, CallSiteFeedback>,
    pub(crate) branch_sites: HashMap<usize, BranchFeedback>,
}

impl FunctionProfile {


    pub(crate) fn branch_feedback(&self, instr_idx: usize) -> Option<&BranchFeedback> {
        self.branch_sites.get(&instr_idx)
    }

    pub(crate) fn branch_bias(&self, instr_idx: usize) -> BranchBias {
        self.branch_feedback(instr_idx)
            .map(BranchFeedback::bias)
            .unwrap_or(BranchBias::NoSamples)
    }

    pub(crate) fn branch_feedback_sites(&self) -> impl Iterator<Item = (usize, &BranchFeedback)> {
        self.branch_sites
            .iter()
            .map(|(instr_idx, feedback)| (*instr_idx, feedback))
    }
}

