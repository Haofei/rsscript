//! Native-tier **profitability** cost model (Step 1).
//!
//! This is deliberately kept SEPARATE from native **eligibility**. Eligibility —
//! "is this region valid native code?" — is answered by `translate_*` returning
//! `Some`/`None`, and ~43 tests plus the `RSS_JIT_REPORT` path depend on that
//! meaning. Profitability — "this region *can* run native, but *should* it?" — is
//! a distinct judgement made only by the tiering layer (`tier.rs`) AFTER a
//! successful translation. Declining for profitability is always correctness-safe:
//! it just keeps the function on the (deterministic) interpreter.
//!
//! The score is a cheap static proxy over the *lowered* `JitInstr` stream (the
//! form that actually runs), crediting work native does much faster than the
//! interpreter's per-op dispatch (scalar ALU, direct flat-list reads) and debiting
//! per-iteration boundary costs that native cannot amortise (host-helper crossings,
//! polymorphic closure-dispatch guards). It is intentionally simple; the weights
//! are calibrated against the perf-gate kernels via `RSS_JIT_COST_MODEL=report`.

#![cfg(feature = "native-jit")]

use vm_jit::{HostHelper, JitFunction, JitInstr};

/// Tri-state env gate (`RSS_JIT_COST_MODEL`):
/// - `enforce` (DEFAULT, i.e. unset): an unprofitable region stays on the
///   interpreter, so the JIT does not compile native code that is ≈ the interpreter.
/// - `report`: score every region and surface the verdict via telemetry/log, but
///   NEVER change execution — lets us see what *would* decline before enforcing.
/// - `off`: the cost model never runs; behaviour as before the model existed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::reg_vm) enum CostMode {
    Off,
    Report,
    Enforce,
}

impl CostMode {
    pub(in crate::reg_vm) fn from_env() -> CostMode {
        match std::env::var("RSS_JIT_COST_MODEL")
            .ok()
            .as_deref()
            .map(str::trim)
        {
            Some("off") => CostMode::Off,
            Some("report") => CostMode::Report,
            // Default ON: an unset (or unrecognised) value enforces, so the JIT does
            // not by default compile a region the cost model judges unprofitable
            // (native ≈ interpreter). Explicit `off` opts out (e.g. native-mechanism
            // tests that must see the region compile). See `effective_cost_mode` for
            // the per-thread override used by those tests.
            _ => CostMode::Enforce,
        }
    }

    /// Whether the model should run at all (report or enforce).
    pub(in crate::reg_vm) fn active(self) -> bool {
        !matches!(self, CostMode::Off)
    }
}

thread_local! {
    /// Per-thread cost-mode override. `None` falls back to `CostMode::from_env`.
    /// This is the RACE-FREE way for a test to pin a mode while OTHER tests run in
    /// parallel in the same process (the process-global `RSS_JIT_COST_MODEL` env
    /// would leak across threads). Native compilation/execution runs synchronously
    /// on the calling thread, so a guard set before the eval call governs it.
    static COST_MODE_OVERRIDE: std::cell::Cell<Option<CostMode>> =
        const { std::cell::Cell::new(None) };
}

/// The cost mode in effect on this thread: the thread-local override if set, else
/// the process env (`CostMode::from_env`, which now defaults to enforce).
pub(in crate::reg_vm) fn effective_cost_mode() -> CostMode {
    COST_MODE_OVERRIDE
        .with(std::cell::Cell::get)
        .unwrap_or_else(CostMode::from_env)
}

/// RAII guard pinning this thread's cost mode for its lifetime; restores the prior
/// value on drop (including across panics).
pub(in crate::reg_vm) struct CostModeGuard(Option<CostMode>);

impl CostModeGuard {
    pub(in crate::reg_vm) fn new(mode: CostMode) -> Self {
        CostModeGuard(COST_MODE_OVERRIDE.with(|c| c.replace(Some(mode))))
    }
}

impl Drop for CostModeGuard {
    fn drop(&mut self) {
        COST_MODE_OVERRIDE.with(|c| c.set(self.0));
    }
}

// --- Credit weights (native-favourable per-op work) -------------------------
const W_SCALAR_ALU: i64 = 2; // Add/Sub/Mul/.../Compare/Equal: native arithmetic vs VM dispatch
const W_LOAD_MOVE: i64 = 1; // LoadInt/Move/conversions: cheap but still beats a dispatch
const W_BRANCH: i64 = 1; // Jump/JumpIf*: native control flow
const W_PROFILED_BRANCH: i64 = 2; // profile-guided branch: a genuine native win
const W_DIRECT_LIST: i64 = 3; // List*Direct: replaced a per-element host call — big win
const W_MATCH_MAP_GET: i64 = 2; // fused Map.get+match: partly native
const W_MEMOIZED_HOSTCALL: i64 = 1; // hoisted loop-invariant helper: amortised

// --- Debit weights (per-iteration boundary cost native cannot amortise) -----
const W_HOST_CALL: i64 = 3; // every HostCall re-crosses the FFI boundary the VM also pays
const W_CLOSURE_GUARD: i64 = 4; // GuardClosureId: monomorphic closure dispatch guard
// A polymorphic closure inline-cache dispatch (`HostCall{helper:ClosureId}` + its
// LoadInt/Equal/JumpIfBool arm ladder). The ladder otherwise scores as cheap scalar
// CREDITS, but it is really per-iteration dispatch overhead — the canonical
// "native barely beats the VM" case (the `profile_closure_pic` kernel). Debit it
// heavily so a region whose hot work IS the PIC declines, while a region that merely
// contains a PIC amid substantial real work keeps its (much larger) base score.
const W_CLOSURE_PIC: i64 = 18;

/// Per-region threshold for BACK-EDGE (loop) regions: decline only when
/// `score <= DECLINE_AT`, i.e. the estimated benefit is strictly NEGATIVE. A
/// break-even loop (`score == 0`, e.g. a host-call-bound loop where native still
/// saves the interpreter's per-iteration dispatch) is KEPT — declining it would be
/// an over-decline. Conservative on purpose now that the model is on by default.
const DECLINE_AT: i64 = -1;

/// Explainable profitability verdict for one native region.
#[derive(Clone, Debug, Default)]
pub(in crate::reg_vm) struct Profitability {
    pub score: i64,
    pub decline: bool,
    pub scalar_ops: u32,
    pub direct_reads: u32,
    pub host_calls: u32,
    pub closure_guards: u32,
    pub pic_sites: u32,
    pub native_calls: u32,
    pub has_backedge: bool,
    pub hot_instrs: u32,
}

impl Profitability {
    /// A stable, single-line reason string for telemetry / the missed-opt report.
    /// Used only for DECLINED regions (the `native_decline_reasons` map key).
    pub(in crate::reg_vm) fn reason(&self, region: &str) -> String {
        format!(
            "unprofitable[{region}]: score={} scalar={} direct={} host_calls={} closure_guard={} pic_sites={} native_calls={} backedge={} hot_instrs={}",
            self.score,
            self.scalar_ops,
            self.direct_reads,
            self.host_calls,
            self.closure_guards,
            self.pic_sites,
            self.native_calls,
            self.has_backedge,
            self.hot_instrs,
        )
    }

    /// A verdict-neutral breakdown for `report`-mode logging of EVERY scored
    /// region (kept and declined alike), so the weights/threshold can be
    /// calibrated against the real per-kernel score distribution.
    pub(in crate::reg_vm) fn summary(&self, region: &str) -> String {
        format!(
            "profit[{region}]: verdict={} score={} scalar={} direct={} host_calls={} closure_guard={} pic_sites={} native_calls={} backedge={} hot_instrs={}",
            if self.decline { "DECLINE" } else { "keep" },
            self.score,
            self.scalar_ops,
            self.direct_reads,
            self.host_calls,
            self.closure_guards,
            self.pic_sites,
            self.native_calls,
            self.has_backedge,
            self.hot_instrs,
        )
    }
}

/// Score a lowered native region. `has_backedge` is true for any region that does
/// O(n) work per native dispatch (all OSR loops, or a whole function with an
/// internal loop) and therefore amortises fixed entry/marshalling cost; a loop-free
/// body dispatched once per interpreter iteration cannot, so it is held to a
/// stricter bar.
pub(in crate::reg_vm) fn native_region_profitability(
    jit_fn: &JitFunction,
    has_backedge: bool,
) -> Profitability {
    let mut p = Profitability {
        has_backedge,
        ..Default::default()
    };
    let mut score: i64 = 0;
    for instr in &jit_fn.code {
        p.hot_instrs += 1;
        match instr {
            JitInstr::Add { .. }
            | JitInstr::Sub { .. }
            | JitInstr::Mul { .. }
            | JitInstr::Div { .. }
            | JitInstr::Mod { .. }
            | JitInstr::BitAnd { .. }
            | JitInstr::BitOr { .. }
            | JitInstr::BitXor { .. }
            | JitInstr::Shl { .. }
            | JitInstr::Shr { .. }
            | JitInstr::Compare { .. }
            | JitInstr::Equal { .. }
            | JitInstr::NotEqual { .. } => {
                p.scalar_ops += 1;
                score += W_SCALAR_ALU;
            }
            JitInstr::LoadInt { .. }
            | JitInstr::LoadFloat { .. }
            | JitInstr::LoadBool { .. }
            | JitInstr::Move { .. }
            | JitInstr::IntToFloat { .. }
            | JitInstr::FloatToInt { .. } => {
                p.scalar_ops += 1;
                score += W_LOAD_MOVE;
            }
            JitInstr::Jump { .. }
            | JitInstr::JumpIfBool { .. }
            | JitInstr::JumpIfIntCompare { .. } => {
                score += W_BRANCH;
            }
            JitInstr::ProfiledJumpIfBool { .. } | JitInstr::ProfiledJumpIfIntCompare { .. } => {
                score += W_PROFILED_BRANCH;
            }
            JitInstr::ListGetIntDirect { .. }
            | JitInstr::ListGetFloatDirect { .. }
            | JitInstr::ListSetIntDirect { .. }
            | JitInstr::ListSetFloatDirect { .. }
            | JitInstr::ListLenDirect { .. }
            | JitInstr::ListIsEmptyDirect { .. } => {
                p.direct_reads += 1;
                score += W_DIRECT_LIST;
            }
            JitInstr::MatchMapGetInt { .. }
            | JitInstr::MatchMapGetFloat { .. }
            | JitInstr::MatchSortedMapGetInt { .. }
            | JitInstr::MatchSortedMapGetFloat { .. } => {
                score += W_MATCH_MAP_GET;
            }
            JitInstr::MemoizedHostCall { .. } => {
                score += W_MEMOIZED_HOSTCALL;
            }
            JitInstr::HostCall {
                helper: HostHelper::ClosureId,
                ..
            } => {
                // The polymorphic closure inline-cache dispatch read. Its arm ladder
                // is credited above as scalar ops; this heavy debit reflects that the
                // ladder is really per-iteration dispatch overhead, not useful work.
                p.pic_sites += 1;
                score -= W_CLOSURE_PIC;
            }
            JitInstr::HostCall { .. } => {
                p.host_calls += 1;
                score -= W_HOST_CALL;
            }
            JitInstr::GuardClosureId { .. } => {
                p.closure_guards += 1;
                score -= W_CLOSURE_GUARD;
            }
            JitInstr::CallNative { .. }
            | JitInstr::CallSelf { .. }
            | JitInstr::CallGroup { .. } => {
                // Native->native call edges are neutral in v1: they avoid an
                // interpreter re-entry (favourable) but a tiny callee is a debit.
                // Without callee-size context here we leave them at 0 and revisit
                // after report-mode data.
                p.native_calls += 1;
            }
            JitInstr::Nop | JitInstr::Return { .. } | JitInstr::Bail | JitInstr::OsrExit => {}
        }
    }
    p.score = score;
    // Decline in two narrow, conservative situations:
    //
    // 1. A BACK-EDGE (loop) region whose score clears nothing: the work amortises
    //    fixed entry cost, so a non-positive score genuinely means "native ≈ VM for
    //    this loop".
    //
    // 2. A LOOP-FREE region that contains a polymorphic closure inline-cache
    //    (`pic_sites > 0`): such a body is, by construction, a per-iteration
    //    dispatch helper (you do not place a polymorphic closure call in a one-shot
    //    function and compile it hot), and the PIC dispatch is per-call overhead
    //    native cannot amortise — the canonical `profile_closure_pic` case. This is
    //    distinct from a clean native-call leaf (`pic_sites == 0`), which is left to
    //    the adaptive `NATIVE_NOAMORTIZE_GIVEUP` demotion and never declined here.
    //
    // A loop-free region WITHOUT a PIC is never statically declined (it may be a
    // beneficial native-call leaf); the adaptive give-up path handles it.
    p.decline = (has_backedge && score <= DECLINE_AT) || (!has_backedge && p.pic_sites > 0);
    p
}
