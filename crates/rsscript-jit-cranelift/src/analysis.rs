use super::{
    HostHelper, JitCompare, JitFunction, JitInstr, JitValueType, instr_def, reachable_jit_instrs,
    successors,
};

/// Definite-assignment ("must") analysis over the [`JitInstr`] CFG. Returns, per
/// instruction index `i`, the set (as a `reg -> bool` vector of width `n_regs`) of
/// registers **definitely assigned on entry to `i`** — i.e. assigned on *every*
/// path from the function entry to `i`.
///
/// Lattice: forward must-analysis. The entry-to-instruction-0 set is the parameter
/// registers `0..n_params`. Ordinary instructions add `defs(i)` to every outgoing
/// edge; fused map matches add `value_dst` only to their `Some` edge. For a
/// non-entry instruction `assigned_in[j] = ⋂ assigned_out[p -> j]` over predecessors `p`
/// (intersection — a register is live on entry only if every incoming path assigns
/// it). Non-entry `assigned_in` starts at the full set and the intersection shrinks
/// it to the fixpoint; instruction 0's entry set is the params and is never
/// intersected down.
pub(super) fn definite_assignment(program: &JitFunction, osr_entry: bool) -> Vec<Vec<bool>> {
    let n = program.code.len();
    let n_regs = program.n_regs as usize;
    if n == 0 {
        return Vec::new();
    }

    // Predecessor lists, derived from the forward CFG.
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        for s in successors(program, i) {
            preds[s].push(i);
        }
    }

    // Entry set for instruction 0: the parameters are assigned, nothing else.
    let mut entry0 = vec![osr_entry; n_regs];
    if !osr_entry {
        for slot in entry0
            .iter_mut()
            .take((program.n_params as usize).min(n_regs))
        {
            *slot = true;
        }
        for &reg in &program.zero_init_regs {
            entry0[reg as usize] = true;
        }
    }

    // `assigned_in[0]` is pinned to the params; every other block starts at the
    // full (all-true) set so intersection can only shrink it toward the fixpoint.
    let mut assigned_in: Vec<Vec<bool>> = (0..n)
        .map(|i| {
            if i == 0 {
                entry0.clone()
            } else {
                vec![true; n_regs]
            }
        })
        .collect();

    let out_for_edge = |in_set: &[bool], i: usize, successor: usize| -> Vec<bool> {
        let mut out = in_set.to_vec();
        match &program.code[i] {
            JitInstr::MatchMapGetInt {
                value_dst,
                some_ip,
                none_ip,
                ..
            }
            | JitInstr::MatchMapGetFloat {
                value_dst,
                some_ip,
                none_ip,
                ..
            }
            | JitInstr::MatchSortedMapGetInt {
                value_dst,
                some_ip,
                none_ip,
                ..
            }
            | JitInstr::MatchSortedMapGetFloat {
                value_dst,
                some_ip,
                none_ip,
                ..
            } if successor == *some_ip as usize && some_ip != none_ip => {
                out[*value_dst as usize] = true;
            }
            JitInstr::MatchMapGetInt { .. }
            | JitInstr::MatchMapGetFloat { .. }
            | JitInstr::MatchSortedMapGetInt { .. }
            | JitInstr::MatchSortedMapGetFloat { .. } => {}
            _ => {
                if let Some(d) = instr_def(&program.code[i])
                    && (d as usize) < n_regs
                {
                    out[d as usize] = true;
                }
            }
        }
        out
    };

    // Iterate to a fixpoint. Intersection is monotone (only clears bits), so the
    // loop terminates in at most `n_regs * n` bit-clears.
    let mut changed = true;
    while changed {
        changed = false;
        for j in 1..n {
            if preds[j].is_empty() {
                // Unreachable block: leave at the full set (it has no resume site of
                // its own that we rely on; its inputs are vacuously satisfied).
                continue;
            }
            let mut new_in = vec![true; n_regs];
            for &p in &preds[j] {
                let out = out_for_edge(&assigned_in[p], p, j);
                for r in 0..n_regs {
                    new_in[r] = new_in[r] && out[r];
                }
            }
            if new_in != assigned_in[j] {
                assigned_in[j] = new_in;
                changed = true;
            }
        }
    }

    assigned_in
}

/// A sound integer interval `[lo, hi]` (inclusive) over `i128`, an
/// over-approximation of the set of `i64` values an Int register may hold. Held in
/// `i128` so the analysis arithmetic itself never overflows. `TOP` is the full
/// `i64` range — the safe default for any value we cannot track precisely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Interval {
    pub(super) lo: i128,
    pub(super) hi: i128,
}

impl Interval {
    pub(super) const TOP: Interval = Interval {
        lo: i64::MIN as i128,
        hi: i64::MAX as i128,
    };

    pub(super) fn constant(c: i64) -> Interval {
        Interval {
            lo: c as i128,
            hi: c as i128,
        }
    }

    /// Convex hull (union over-approximation) of two intervals.
    fn hull(self, other: Interval) -> Interval {
        Interval {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// True iff every value in `[lo, hi]` fits in an `i64`. When this holds for the
    /// *result* interval of an Int op, that op provably cannot overflow.
    fn fits_i64(self) -> bool {
        self.lo >= i64::MIN as i128 && self.hi <= i64::MAX as i128
    }
}

/// Forward integer-interval ("range") analysis over the Int registers of a native
/// function. Returns, per instruction index `i`, an `i128` interval per register
/// that **soundly over-approximates** every value the register may hold on entry to
/// `i`. Used solely to prove — conservatively, at compile time — that a particular
/// `Add`/`Sub`/`Mul` cannot overflow, so the checked `*_overflow` + bail can be
/// replaced by a plain `iadd`/`isub`/`imul` with byte-identical results.
///
/// Soundness is the whole point: every transfer function is an over-approximation,
/// and any register/operation we cannot prove a finite bound for is `TOP`
/// (`[i64::MIN, i64::MAX]`), which forces the checked path. Non-Int registers carry
/// `TOP` and are never consulted. The lattice has height 3 per register
/// (constant-derived range ⊑ wider range ⊑ TOP); a widening at every join jumps any
/// register whose merged interval grew straight to `TOP`, which both guarantees
/// termination (no infinite ascending chains across loop back-edges — an
/// unbounded-in-a-loop register widens to `TOP`, the safe answer) and keeps the
/// analysis cheap.
pub(super) fn interval_analysis(program: &JitFunction) -> Vec<Vec<Interval>> {
    let n = program.code.len();
    let n_regs = program.n_regs as usize;
    if n == 0 {
        return Vec::new();
    }

    let is_int = |r: u32| program.reg_types[r as usize] == JitValueType::Int;
    // Read an operand's interval from an in-set: TOP for any non-Int register (we
    // only reason about Int arithmetic) and for out-of-range indices.
    let read = |set: &[Interval], r: u32| -> Interval {
        if is_int(r) {
            set.get(r as usize).copied().unwrap_or(Interval::TOP)
        } else {
            Interval::TOP
        }
    };

    // Predecessor lists from the reachable CFG. Dead islands must not contribute
    // facts to reachable code: a dead backedge into instruction zero otherwise
    // narrows unknown function parameters and can unsafely remove overflow checks.
    let reachable = reachable_jit_instrs(program);
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); n];
    for i in 0..n {
        if !reachable[i] {
            continue;
        }
        for s in successors(program, i) {
            if reachable[s] {
                preds[s].push(i);
            }
        }
    }

    // Transfer function: given the in-set at `i`, produce the out-set. Only the
    // proven cases narrow below TOP; everything else stays/becomes TOP.
    let out_of = |in_set: &[Interval], i: usize| -> Vec<Interval> {
        let mut out = in_set.to_vec();
        let set = |out: &mut Vec<Interval>, r: u32, v: Interval| {
            if (r as usize) < n_regs {
                // Collapse any range that doesn't fit i64 straight to TOP. Such a
                // range can never prove an op safe anyway, and clamping here keeps the
                // analysis's own i128 arithmetic bounded (a register always holds an
                // i64-fitting range or TOP, so downstream corner products stay well
                // within i128). Also never claim a tighter bound than the register's
                // storage class allows.
                out[r as usize] = if is_int(r) && v.fits_i64() {
                    v
                } else {
                    Interval::TOP
                };
            }
        };
        match &program.code[i] {
            JitInstr::LoadInt { dst, value } => set(&mut out, *dst, Interval::constant(*value)),
            JitInstr::LoadBool { dst, value } => {
                set(&mut out, *dst, Interval::constant(i64::from(*value)));
            }
            JitInstr::Move { dst, src } => {
                let v = read(in_set, *src);
                set(&mut out, *dst, v);
            }
            // A list/array length is a non-negative element count; we soundly bound
            // it to `[0, i64::MAX]` and deliberately assume NO tighter upper bound.
            JitInstr::ListLenDirect { dst, .. }
            | JitInstr::HostCall {
                helper: HostHelper::ListLen,
                dst,
                ..
            } => {
                set(
                    &mut out,
                    *dst,
                    Interval {
                        lo: 0,
                        hi: i64::MAX as i128,
                    },
                );
            }
            JitInstr::Add { dst, lhs, rhs } if is_int(*lhs) => {
                let a = read(in_set, *lhs);
                let b = read(in_set, *rhs);
                set(
                    &mut out,
                    *dst,
                    Interval {
                        lo: a.lo + b.lo,
                        hi: a.hi + b.hi,
                    },
                );
            }
            JitInstr::Sub { dst, lhs, rhs } if is_int(*lhs) => {
                let a = read(in_set, *lhs);
                let b = read(in_set, *rhs);
                set(
                    &mut out,
                    *dst,
                    Interval {
                        lo: a.lo - b.hi,
                        hi: a.hi - b.lo,
                    },
                );
            }
            JitInstr::Mul { dst, lhs, rhs } if is_int(*lhs) => {
                let a = read(in_set, *lhs);
                let b = read(in_set, *rhs);
                // Product range is the hull of the four corner products (i128, so
                // the proof arithmetic cannot itself overflow for i64 operands).
                let c1 = a.lo * b.lo;
                let c2 = a.lo * b.hi;
                let c3 = a.hi * b.lo;
                let c4 = a.hi * b.hi;
                let lo = c1.min(c2).min(c3).min(c4);
                let hi = c1.max(c2).max(c3).max(c4);
                set(&mut out, *dst, Interval { lo, hi });
            }
            JitInstr::Mod { dst, lhs, rhs } => {
                let numerator = read(in_set, *lhs);
                let divisor = read(in_set, *rhs);
                // On the successful continuation of signed remainder, the result
                // has the numerator's sign and its magnitude is smaller than both
                // |numerator| and |divisor|. Divide-by-zero and MIN % -1 deopt
                // before defining `dst`, so they need not be represented here.
                let max_divisor_abs = divisor.lo.abs().max(divisor.hi.abs());
                let max_remainder = max_divisor_abs
                    .saturating_sub(1)
                    .min(numerator.lo.abs().max(numerator.hi.abs()));
                let result = if max_divisor_abs == 0 {
                    Interval::TOP
                } else if numerator.lo >= 0 {
                    Interval {
                        lo: 0,
                        hi: numerator.hi.min(max_remainder),
                    }
                } else if numerator.hi <= 0 {
                    Interval {
                        lo: numerator.lo.max(-max_remainder),
                        hi: 0,
                    }
                } else {
                    Interval {
                        lo: numerator.lo.max(-max_remainder),
                        hi: numerator.hi.min(max_remainder),
                    }
                };
                set(&mut out, *dst, result);
            }
            // Every other definer produces an untracked value ⇒ TOP. (Covers Div,
            // bitops, shifts, compares, heap reads, ClosureId, params, etc.)
            other => {
                if let Some(d) = instr_def(other) {
                    set(&mut out, d, Interval::TOP);
                }
            }
        }
        out
    };

    // Edge-sensitive (branch-conditioned) refinement (branch-conditioned range refinement). When predecessor `p`
    // is a `JumpIfIntCompare` and the edge `p -> succ` is governed by a comparison
    // fact, tighten the operand intervals flowed along that *specific* edge by the
    // asserted relation. This is what lets a loop counter `i` — TOP at the loop
    // header after the join — be proven `<= N - 1` on the loop-body edge (the guard's
    // taken edge), so the body's `i = i + 1` provably fits i64.
    //
    // Soundness: each rule narrows an interval only to values that genuinely satisfy
    // the asserted relation; all arithmetic is in i128 so it cannot overflow; if a
    // refinement would invert an interval (`lo > hi`) the edge is unreachable, so we
    // leave the operand at its un-refined (still sound) value rather than emit a
    // malformed interval. We refine ONLY the cmps/edges enumerated below; everything
    // else flows un-refined. The join still hulls + widens to TOP (termination is
    // unaffected — refinement only narrows along an edge, never grows the lattice).
    let refine_edge = |out: &mut Vec<Interval>, p: usize, succ: usize| {
        let (lhs, rhs, op, expected, target) = match &program.code[p] {
            JitInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            }
            | JitInstr::ProfiledJumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
                ..
            } => (*lhs, *rhs, *op, *expected, *target),
            _ => return,
        };
        if !is_int(lhs) || !is_int(rhs) {
            return;
        }
        // Is this edge the one on which `(lhs op rhs)` holds, or its negation?
        // The target edge is taken when `(lhs op rhs) == expected`.
        let is_target = succ == target as usize;
        // Could this edge also be the fall-through? (e.g. target == next). If the
        // edge is ambiguous (both target and fall-through reach `succ`), we cannot
        // attribute a single fact to it — flow un-refined (sound).
        let is_fallthrough = succ == p + 1;
        if is_target && is_fallthrough {
            return;
        }
        if !is_target && !is_fallthrough {
            return;
        }
        // The relation that holds on THIS edge: on the target edge it is the raw
        // comparison iff `expected` is true; on the fall-through edge it is the
        // negation of "comparison == expected".
        let cond_holds = if is_target { expected } else { !expected };
        // `cond_holds == true`  => `lhs op rhs`
        // `cond_holds == false` => `!(lhs op rhs)`
        let (a, b) = (out[lhs as usize], out[rhs as usize]);
        // Apply a refinement to register `r`, intersecting with [lo, hi]; if the
        // intersection inverts, the edge is unreachable -> leave un-refined (sound).
        let apply = |out: &mut Vec<Interval>, r: u32, lo: i128, hi: i128| {
            let cur = out[r as usize];
            let nlo = cur.lo.max(lo);
            let nhi = cur.hi.min(hi);
            if nlo <= nhi {
                out[r as usize] = Interval { lo: nlo, hi: nhi };
            }
        };
        // Effective relation between lhs and rhs that holds on this edge, expressed
        // as one of the four primitive forms by folding `cond_holds`.
        // Lt: lhs <  rhs ;  !Lt => lhs >= rhs
        // Le: lhs <= rhs ;  !Le => lhs >  rhs
        // Gt: lhs >  rhs ;  !Gt => lhs <= rhs
        // Ge: lhs >= rhs ;  !Ge => lhs <  rhs
        use JitCompare::*;
        // Normalize to: does `lhs < rhs`, `lhs <= rhs`, `lhs > rhs`, or `lhs >= rhs`
        // hold on this edge?
        let rel = match (op, cond_holds) {
            (Lt, true) | (Ge, false) => Lt,
            (Le, true) | (Gt, false) => Le,
            (Gt, true) | (Le, false) => Gt,
            (Ge, true) | (Lt, false) => Ge,
        };
        match rel {
            // lhs < rhs: lhs.hi <= rhs.hi - 1 ; rhs.lo >= lhs.lo + 1
            Lt => {
                apply(out, lhs, i64::MIN as i128, b.hi - 1);
                apply(out, rhs, a.lo + 1, i64::MAX as i128);
            }
            // lhs <= rhs: lhs.hi <= rhs.hi ; rhs.lo >= lhs.lo
            Le => {
                apply(out, lhs, i64::MIN as i128, b.hi);
                apply(out, rhs, a.lo, i64::MAX as i128);
            }
            // lhs > rhs: lhs.lo >= rhs.lo + 1 ; rhs.hi <= lhs.hi - 1
            Gt => {
                apply(out, lhs, b.lo + 1, i64::MAX as i128);
                apply(out, rhs, i64::MIN as i128, a.hi - 1);
            }
            // lhs >= rhs: lhs.lo >= rhs.lo ; rhs.hi <= lhs.hi
            Ge => {
                apply(out, lhs, b.lo, i64::MAX as i128);
                apply(out, rhs, i64::MIN as i128, a.hi);
            }
        }
    };

    // Entry to instruction 0: params are untracked (TOP); everything else TOP too.
    // Every per-instruction register starts at TOP and the analysis narrows it only
    // where a definer/merge proves a bound. `initialized[j]` tracks whether block
    // `j`'s in-set has been computed from predecessors at least once (the first such
    // pass *replaces* the all-TOP seed rather than hulling against it).
    let mut interval_in: Vec<Vec<Interval>> = (0..n).map(|_| vec![Interval::TOP; n_regs]).collect();
    let mut initialized = vec![false; n];
    // Sticky-widened marker per (instruction, register): once a register at block
    // `j` widens to TOP (e.g. across a loop back-edge), it is pinned to TOP and
    // never narrowed again. This makes each register monotone — narrowed at most
    // once, then only ever widened to the sticky TOP — which guarantees the fixpoint
    // terminates (no narrow/widen oscillation) and keeps any unbounded-in-a-loop
    // register at the safe TOP. Lattice height per register ≤ 3.
    let mut pinned_top: Vec<Vec<bool>> = (0..n).map(|_| vec![false; n_regs]).collect();

    // PHASE 1 — plain monotone fixpoint (unchanged interval range analysis lattice). Branch refinement
    // is deliberately NOT applied here: it is a non-propagating, query-time narrowing
    // (Phase 2 below). Keeping it out of the fixpoint preserves the original
    // termination argument exactly (height-3 lattice + sticky-TOP pinning) — a refined
    // bound never feeds back across a loop back-edge, so there is no slow descending
    // ratchet, and the loop still converges in O(n_regs * n) passes.
    let mut changed = true;
    while changed {
        changed = false;
        for j in 0..n {
            // Instruction zero always has a virtual external predecessor carrying
            // TOP, even when a real loop backedge also targets zero. Unreachable and
            // predecessor-less blocks likewise stay at the conservative TOP seed.
            if j == 0 || !reachable[j] || preds[j].is_empty() {
                initialized[j] = true;
                continue;
            }
            let bottom = Interval {
                lo: i128::MAX,
                hi: i128::MIN,
            };
            let mut new_in = vec![bottom; n_regs];
            // Optimistic worklist join: only hull predecessors whose own in-set has
            // been computed at least once. A not-yet-`initialized` predecessor still
            // carries the all-TOP seed (e.g. an unvisited loop back-edge on the first
            // pass); folding that seed in would prematurely widen loop-invariant
            // registers (like a constant increment) to TOP and never recover. Treating
            // it as bottom until it is real is the standard monotone worklist; once it
            // is initialized it joins normally, and the widening below still caps any
            // genuine loop-carried growth, so termination is unaffected.
            let mut any_pred = false;
            for &p in &preds[j] {
                if !initialized[p] {
                    continue;
                }
                any_pred = true;
                let out = out_of(&interval_in[p], p);
                for r in 0..n_regs {
                    new_in[r] = new_in[r].hull(out[r]);
                }
            }
            // No initialized predecessor yet (e.g. a block reachable only via a
            // back-edge whose source hasn't been seen): leave it for a later pass
            // rather than adopt the malformed bottom seed.
            if !any_pred {
                continue;
            }
            let first = !initialized[j];
            for r in 0..n_regs {
                if pinned_top[j][r] {
                    new_in[r] = Interval::TOP;
                    continue;
                }
                let merged = new_in[r];
                if first {
                    new_in[r] = merged;
                    continue;
                }
                let old = interval_in[j][r];
                // If the merged interval grew strictly wider than the current value
                // (e.g. across a loop back-edge), widen straight to TOP and pin it.
                if merged.lo < old.lo || merged.hi > old.hi {
                    new_in[r] = Interval::TOP;
                    pinned_top[j][r] = true;
                } else {
                    new_in[r] = merged;
                }
            }
            if new_in != interval_in[j] || first {
                interval_in[j] = new_in;
                initialized[j] = true;
                changed = true;
            }
        }
    }

    // PHASE 2 — branch-conditioned refinement (branch-conditioned range refinement), propagated through
    // single-predecessor body chains but NOT across joins.
    //
    // A loop counter is tightened on the loop GUARD's body edge (e.g. `i <= N - 1`),
    // but the increment `i = i + 1` may sit a few straight-line instructions later, so
    // the refined bound must flow forward to it. We therefore recompute a refined
    // in-set per block:
    //   * a MULTI-predecessor block (a join: loop header, if/else merge) is PINNED to
    //     its Phase-1 widened value — refinement never enters or crosses a join, so the
    //     widening/termination story is exactly Phase 1's;
    //   * a SINGLE-predecessor block `j` (pred `p`) takes `transfer(refined[p])` and
    //     then intersects the branch fact asserted on the `p -> j` edge.
    // Because every CFG cycle passes through a join (a loop header has ≥2 preds: entry
    // + back-edge), and joins are pinned to a fixed value, the refined overlay has NO
    // cycles — it is a DAG rooted at the pinned joins, so this fixpoint converges in at
    // most `n` ordered passes. Every value is `⊑` its Phase-1 counterpart (refinement
    // and the transfer only narrow off a sound base), so the result stays a sound
    // over-approximation. `refine_edge` already drops any inverted (unreachable-edge)
    // refinement, so no malformed interval is ever produced.
    let multi_pred: Vec<bool> = (0..n)
        .map(|j| preds[j].iter().filter(|&&p| initialized[p]).count() > 1)
        .collect();
    let mut refined_in = interval_in.clone();
    // DAG depth ≤ n, so `n + 1` ordered sweeps reach the fixpoint; the loop also exits
    // early once nothing changes.
    for _ in 0..=n {
        let mut changed = false;
        for j in 0..n {
            if j == 0 || !reachable[j] || multi_pred[j] || preds[j].is_empty() {
                // Joins and entry/unreachable blocks keep their Phase-1 value.
                continue;
            }
            // Exactly one (initialized) predecessor: attribute the edge fact to it.
            let Some(&p) = preds[j].iter().find(|&&p| initialized[p]) else {
                continue;
            };
            let mut row = out_of(&refined_in[p], p);
            refine_edge(&mut row, p, j);
            if row != refined_in[j] {
                refined_in[j] = row;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    refined_in
}

/// Whether the result of the `Add`/`Sub`/`Mul` at instruction `i` provably fits in
/// `i64` given the operand intervals on entry — i.e. the op cannot overflow, so the
/// checked `*_overflow` + bail may be replaced by a plain unchecked op with
/// identical results. Conservative: any TOP operand (or a non-Int op) makes the
/// result range TOP, which does NOT fit, so the checked path is kept. The result
/// interval is computed in `i128` exactly as the analysis transfer function does.
pub(super) fn arith_cannot_overflow(intervals: &[Interval], instr: &JitInstr) -> bool {
    let get = |r: u32| intervals.get(r as usize).copied().unwrap_or(Interval::TOP);
    let result = match instr {
        JitInstr::Add { lhs, rhs, .. } => {
            let a = get(*lhs);
            let b = get(*rhs);
            Interval {
                lo: a.lo + b.lo,
                hi: a.hi + b.hi,
            }
        }
        JitInstr::Sub { lhs, rhs, .. } => {
            let a = get(*lhs);
            let b = get(*rhs);
            Interval {
                lo: a.lo - b.hi,
                hi: a.hi - b.lo,
            }
        }
        JitInstr::Mul { lhs, rhs, .. } => {
            let a = get(*lhs);
            let b = get(*rhs);
            let c1 = a.lo * b.lo;
            let c2 = a.lo * b.hi;
            let c3 = a.hi * b.lo;
            let c4 = a.hi * b.hi;
            Interval {
                lo: c1.min(c2).min(c3).min(c4),
                hi: c1.max(c2).max(c3).max(c4),
            }
        }
        _ => return false,
    };
    result.fits_i64()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UniqueValueOrigin {
    Unknown,
    Constant(i64),
    ListLen(u32),
    Mod {
        ip: usize,
        numerator: u32,
        divisor: u32,
    },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct ListBoundsPlan {
    pub(super) unchecked_ips: std::collections::HashSet<usize>,
    /// Minimum entry length required for each flat-list base. Multiple constant
    /// modulo accesses on one base are consolidated to the strongest guard.
    pub(super) entry_min_len: std::collections::BTreeMap<u32, i64>,
}

/// Resolve a register through its sole reachable definition. This is deliberately
/// stricter than reaching-definition analysis: any reachable second definition,
/// even on a disjoint branch, makes the value ambiguous and keeps accesses checked.
fn unique_value_origin(
    reg: u32,
    program: &JitFunction,
    unique_defs: &[Option<usize>],
    ambiguous: &[bool],
    visiting: &mut [bool],
) -> UniqueValueOrigin {
    let r = reg as usize;
    if r >= unique_defs.len() || ambiguous[r] || visiting[r] {
        return UniqueValueOrigin::Unknown;
    }
    let Some(ip) = unique_defs[r] else {
        return UniqueValueOrigin::Unknown;
    };
    visiting[r] = true;
    let origin = match &program.code[ip] {
        JitInstr::LoadInt { value, .. } => UniqueValueOrigin::Constant(*value),
        JitInstr::ListLenDirect { base, .. } => UniqueValueOrigin::ListLen(*base),
        JitInstr::Move { src, .. } => {
            unique_value_origin(*src, program, unique_defs, ambiguous, visiting)
        }
        JitInstr::Mod {
            lhs: numerator,
            rhs: divisor,
            ..
        } => UniqueValueOrigin::Mod {
            ip,
            numerator: *numerator,
            divisor: *divisor,
        },
        _ => UniqueValueOrigin::Unknown,
    };
    visiting[r] = false;
    origin
}

/// Find direct list accesses whose modulo-derived index is provably in range.
/// Public `JitInstr` semantics stay checked; this plan only selects machine-code
/// sites where a stronger entry guard or same-base length provenance proves safety.
pub(super) fn list_bounds_plan(
    program: &JitFunction,
    intervals: &[Vec<Interval>],
    osr_entry: bool,
) -> ListBoundsPlan {
    let reachable = reachable_jit_instrs(program);
    let n_regs = program.n_regs as usize;
    let mut unique_defs = vec![None; n_regs];
    let mut ambiguous = vec![false; n_regs];
    // Entry values are definitions too. A normal function defines params and
    // explicit zero-init scratch; OSR defines the whole register window. If code
    // later writes one of these registers, provenance is necessarily multi-def.
    if osr_entry {
        ambiguous.fill(true);
    } else {
        ambiguous
            .iter_mut()
            .take(program.n_params as usize)
            .for_each(|entry_defined| *entry_defined = true);
        for &reg in &program.zero_init_regs {
            ambiguous[reg as usize] = true;
        }
    }
    for (ip, instr) in program.code.iter().enumerate() {
        if !reachable[ip] {
            continue;
        }
        let Some(dst) = instr_def(instr) else {
            continue;
        };
        let r = dst as usize;
        if unique_defs[r].replace(ip).is_some() {
            ambiguous[r] = true;
        }
    }

    let origin = |reg| {
        unique_value_origin(
            reg,
            program,
            &unique_defs,
            &ambiguous,
            &mut vec![false; n_regs],
        )
    };
    let mut plan = ListBoundsPlan::default();
    for (ip, instr) in program.code.iter().enumerate() {
        if !reachable[ip] {
            continue;
        }
        let (base, index) = match instr {
            JitInstr::ListGetIntDirect { base, index, .. }
            | JitInstr::ListSetIntDirect { base, index, .. }
            | JitInstr::ListGetFloatDirect { base, index, .. }
            | JitInstr::ListSetFloatDirect { base, index, .. } => (*base, *index),
            _ => continue,
        };
        let UniqueValueOrigin::Mod {
            ip: mod_ip,
            numerator,
            divisor,
        } = origin(index)
        else {
            continue;
        };
        let numerator_nonnegative = intervals
            .get(mod_ip)
            .and_then(|row| row.get(numerator as usize))
            .is_some_and(|range| range.lo >= 0);
        if !numerator_nonnegative {
            continue;
        }
        match origin(divisor) {
            UniqueValueOrigin::Constant(divisor) if divisor > 0 => {
                plan.unchecked_ips.insert(ip);
                plan.entry_min_len
                    .entry(base)
                    .and_modify(|minimum| *minimum = (*minimum).max(divisor))
                    .or_insert(divisor);
            }
            UniqueValueOrigin::ListLen(len_base) if len_base == base => {
                plan.unchecked_ips.insert(ip);
            }
            _ => {}
        }
    }
    plan
}
