use super::*;

/// The result of [`loop_local_sinkable_closures`].
#[cfg(feature = "native-jit")]
#[derive(Default)]
pub(in crate::reg_vm) struct SinkableClosures {
    /// `call_operand_reg -> callee_id`: a `CallClosure{closure: reg}` whose `reg` is a
    /// sunk closure-value register dispatches statically to `callee_id`; its body is
    /// inlined at the call site.
    sink_calls: std::collections::HashMap<usize, usize>,
    /// `MakeClosure`/copy-`Move` instruction indices to DELETE: the heap alloc and the
    /// pure copies that only forward the (now non-existent) closure value.
    dead_defs: std::collections::HashSet<usize>,
    /// `call_operand_reg -> capture_regs`: the capture registers from the unique
    /// sunk `MakeClosure` definition behind that operand.
    capture_regs: std::collections::HashMap<usize, Vec<usize>>,
}

/// OSR × closure-allocation sinking. A `MakeClosure{dst, function:k, captures}` whose
/// closure value is **loop-local and non-escaping** — flowing (possibly through pure
/// copy `Move`s) ONLY into the `closure` operand of `CallClosure` sites, never stored,
/// returned, captured, or read as a plain value — can have its heap allocation
/// dissolved: the callee `k` is known STATICALLY from the `MakeClosure` (no profile
/// needed), so its body is inlined at every call site and the `MakeClosure` (plus its
/// dead copy `Move`s) is deleted. The loop then becomes pure-scalar and OSRs.
///
/// The analysis builds, for each in-region `MakeClosure`, the **closure-value set**
/// `S` = its dst plus every register that is a pure `Move`-copy of a member of `S`.
/// It is sinkable iff ALL of the following hold (any failure ⇒ skip it; the alloc
/// stays on its normal heap path — behavior unchanged):
/// - every register in `S` has a SINGLE definition (the `MakeClosure`, or a `Move`
///   from another `S` member) — a second definition of any `S` register (e.g. a
///   `MakeClosure` of a different callee on another path) is polymorphic ⇒ bail;
/// - EVERY read of any `S` register is either a copy-`Move` into another `S` member
///   or the `closure` operand of a `CallClosure` (never an `arg`, never any other
///   instruction — i.e. the closure value never escapes as a value); every such
///   `CallClosure` is in the region with no `mut` args;
/// - the callee `k` is native-inlinable at the call arity
///   ([`native_callee_inlinable`] when captureless, else
///   [`native_capturing_callee_inlinable`]) and `captures.len() == callee.captures`.
///   Each scalar capture is materialized at the inline site by a plain `Move` of the
///   (already-live) capture register; a non-scalar (heap) capture is caught by the
///   downstream OSR type inference (Int/Bool/Float only — the same safety net the
///   other region passes rely on), so such a body simply fails to compile.
///
/// An instruction that produces/consumes only scalar (Int/Bool/Float) values has
/// no heap operand. A heap value can only be consumed by a non-scalar instruction,
/// so a register read exclusively by scalar-only instructions cannot hold a heap
/// value.
#[cfg(feature = "native-jit")]
fn native_instr_scalar_only(instr: &RegInstr) -> bool {
    matches!(
        instr,
        RegInstr::LoadInt { .. }
            | RegInstr::LoadFloat { .. }
            | RegInstr::LoadBool { .. }
            | RegInstr::LoadUnit { .. }
            | RegInstr::Move { .. }
            | RegInstr::AddInt { .. }
            | RegInstr::SubInt { .. }
            | RegInstr::MulInt { .. }
            | RegInstr::DivInt { .. }
            | RegInstr::ModInt { .. }
            | RegInstr::BitAndInt { .. }
            | RegInstr::BitOrInt { .. }
            | RegInstr::BitXorInt { .. }
            | RegInstr::ShiftLeftInt { .. }
            | RegInstr::ShiftRightInt { .. }
            | RegInstr::LessInt { .. }
            | RegInstr::LessEqualInt { .. }
            | RegInstr::GreaterInt { .. }
            | RegInstr::GreaterEqualInt { .. }
            | RegInstr::Equal { .. }
            | RegInstr::NotEqual { .. }
            | RegInstr::Jump { .. }
            | RegInstr::JumpIfBool { .. }
            | RegInstr::JumpIfIntCompare { .. }
            | RegInstr::Return { .. }
            | RegInstr::RuntimeError { .. }
    )
}

/// Prove every capture of `callee` is a SCALAR value. Capture slots are callee regs
/// `0..callee.captures`; follow pure copy-`Move`s so an aliased capture is tracked.
/// If every read of a capture-derived register is [`native_instr_scalar_only`], the
/// captures are all scalar (a heap capture would necessarily be consumed by a
/// non-scalar op). Conservative: any non-scalar (or `All`-reading) use fails the
/// proof. This is the static analog of the runtime `captures_all_scalar` bit the
/// monomorphic inline path uses — needed because OSR type inference now admits
/// `Handle`, so it no longer rejects a sunk heap capture on its own.
#[cfg(feature = "native-jit")]
fn native_callee_captures_all_scalar(callee: &RegFunction) -> bool {
    use std::collections::HashSet;
    if callee.captures == 0 {
        return true;
    }
    let mut derived: HashSet<usize> = (0..callee.captures).collect();
    loop {
        let mut grew = false;
        for instr in &callee.code {
            if let RegInstr::Move { dst, src } = instr
                && derived.contains(src)
                && !derived.contains(dst)
            {
                derived.insert(*dst);
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    callee.code.iter().all(|instr| {
        let reads_capture = match instr_read_regs(instr) {
            RegFootprint::Some(regs) => regs.iter().any(|r| derived.contains(r)),
            RegFootprint::All => true,
        };
        !reads_capture || native_instr_scalar_only(instr)
    })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn loop_local_sinkable_closures(
    unit: &RegUnit,
    func: &RegFunction,
    header: usize,
    exit: usize,
    j3: bool,
) -> SinkableClosures {
    use std::collections::HashSet;
    let in_region = |i: usize| i >= header && i < exit;
    let mut out = SinkableClosures::default();

    'candidates: for (mi, instr) in func.code.iter().enumerate() {
        let RegInstr::MakeClosure {
            dst: c,
            function: k,
            captures,
        } = instr
        else {
            continue;
        };
        if !in_region(mi) {
            continue;
        }
        let k = *k;
        let callee = match unit.functions.get(k) {
            Some(callee) => callee,
            None => continue,
        };

        // Grow the closure-value set `S` by following pure copy-`Move`s: a
        // `Move{dst:d, src:s}` with `s in S` adds `d` to `S`. Record the defining
        // instruction index of each member (the MakeClosure or the copy Move).
        let mut value_regs: HashSet<usize> = HashSet::new();
        value_regs.insert(*c);
        let mut def_indices: HashSet<usize> = HashSet::new();
        def_indices.insert(mi);
        // Fixpoint over the copy graph (bounded by code length).
        loop {
            let mut grew = false;
            for (di, dinstr) in func.code.iter().enumerate() {
                if let RegInstr::Move { dst, src } = dinstr
                    && value_regs.contains(src)
                    && !value_regs.contains(dst)
                {
                    value_regs.insert(*dst);
                    def_indices.insert(di);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }

        // Single-definition guard: every `S` register must be written ONLY by a
        // recorded def (the MakeClosure or an `S`-copy Move). Any other writer means
        // the value is redefined on some path (polymorphic / clobbered) ⇒ bail.
        for (wi, winstr) in func.code.iter().enumerate() {
            if def_indices.contains(&wi) {
                continue;
            }
            let writes_value = match instr_written_reg(winstr) {
                RegFootprint::Some(regs) => regs.iter().any(|r| value_regs.contains(r)),
                RegFootprint::All => true,
            };
            if writes_value {
                continue 'candidates;
            }
        }

        // Non-escape: every read of an `S` register must be a copy-`Move` (already a
        // recorded def) or the `closure` operand of an in-region `CallClosure` with no
        // `mut` args, where the value is NOT also an arg.
        let mut call_operands: Vec<usize> = Vec::new();
        for (ri, rinstr) in func.code.iter().enumerate() {
            if def_indices.contains(&ri) {
                // A recorded copy-Move: it reads an `S` reg by construction; fine.
                continue;
            }
            match rinstr {
                RegInstr::CallClosure {
                    closure,
                    args,
                    mut_args,
                    ..
                } if value_regs.contains(closure) => {
                    if args.iter().any(|a| value_regs.contains(a))
                        || !mut_args.is_empty()
                        || !in_region(ri)
                    {
                        continue 'candidates;
                    }
                    // Under j3 (OSR×inline) a mapper that builds/destructures a
                    // non-escaping Option/Result/variant/struct also qualifies — it
                    // dissolves post-inline via the region SR passes (e.g. an
                    // `and_then` mapper `|v| Some(v*2)` / `|v| Ok(v*2)`).
                    let inlinable = if callee.captures == 0 {
                        if j3 {
                            native_callee_inlinable_j3(callee, args.len())
                        } else {
                            native_callee_inlinable(callee, args.len())
                        }
                    } else {
                        native_capturing_callee_inlinable(callee, args.len())
                    };
                    if callee.captures != captures.len()
                        || !inlinable
                        || !native_callee_captures_all_scalar(callee)
                    {
                        continue 'candidates;
                    }
                    call_operands.push(*closure);
                }
                _ => {
                    let reads_value = match instr_read_regs(rinstr) {
                        RegFootprint::Some(regs) => regs.iter().any(|r| value_regs.contains(r)),
                        RegFootprint::All => true,
                    };
                    if reads_value {
                        continue 'candidates;
                    }
                }
            }
        }
        if call_operands.is_empty() {
            continue;
        }
        for op in call_operands {
            out.sink_calls.insert(op, k);
            out.capture_regs.insert(op, captures.clone());
        }
        out.dead_defs.extend(def_indices);
    }
    out
}

#[cfg(feature = "native-jit")]
/// Inline straight-line leaf `CallKnown`/closure calls, returning the rewritten
/// code, the new register count, AND a transformed→original ip-map
/// (`ip_map[transformed_ip] = original_ip`).
///
/// For a copy-through (non-inlined) instruction the map is its original index. For
/// an instruction spliced in from an inlined callee — including the arg-marshalling
/// `Move`s and dispatch scaffolding — the map is the original index of the
/// `CallKnown`/`CallClosure` it was inlined from: if a deopt lands inside the
/// inlined region, the interpreter resumes by re-executing that original call.
///
/// `loop_region` is the `[header, exit)` index range (in ORIGINAL `func.code`) of
/// the OSR loop the caller intends to compile. ONLY calls whose original index lies
/// inside that range are subject to the inline-or-bail rule: an in-region call must
/// be inlinable (it must dissolve to reach the native subset) or the whole pass
/// bails (`None`). A call OUTSIDE the region (a pre-/post-loop helper such as
/// `bench_size`) is copied through verbatim — it never runs natively (OSR entry is
/// the header, the only native exit is `OsrExit`), so its inlinability is irrelevant
/// and must not veto OSR for the hot loop. Passing `None` makes EVERY call in-scope
/// (the conservative whole-function behavior), preserved for callers that do not
/// pre-detect a region.
pub(in crate::reg_vm) fn native_inline_leaf_calls(
    unit: &RegUnit,
    func: &RegFunction,
    profile: Option<&FunctionProfile>,
    call_count: u32,
    j3: bool,
    loop_region: Option<(usize, usize)>,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    native_inline_leaf_calls_inner(unit, func, profile, call_count, j3, loop_region, &|_| false)
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_inline_leaf_calls_preserving_known_calls(
    unit: &RegUnit,
    func: &RegFunction,
    profile: Option<&FunctionProfile>,
    call_count: u32,
    j3: bool,
    loop_region: Option<(usize, usize)>,
    preserve_call_known: &std::collections::HashSet<usize>,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    native_inline_leaf_calls_inner(unit, func, profile, call_count, j3, loop_region, &|i| {
        preserve_call_known.contains(&i)
    })
}

#[cfg(feature = "native-jit")]
fn native_inline_leaf_calls_inner(
    unit: &RegUnit,
    func: &RegFunction,
    profile: Option<&FunctionProfile>,
    call_count: u32,
    j3: bool,
    loop_region: Option<(usize, usize)>,
    preserve_call_known: &dyn Fn(usize) -> bool,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    // A call at original index `i` is subject to inline-or-bail only if it lies in
    // the loop region (or no region was supplied ⇒ whole function in-scope).
    let in_region = |i: usize| match loop_region {
        Some((h, e)) => i >= h && i < e,
        None => true,
    };
    // Closure-allocation sinking: a `MakeClosure` whose dst is local to the compiled
    // region/function + non-escaping + called only via `CallClosure{closure:dst}` is
    // sunk — its alloc is dissolved and the statically-known callee body is inlined
    // at each call. With `loop_region=None`, run the same analysis over the whole
    // function so normal native translation can remove local closure churn too.
    let sinkable = match loop_region {
        Some((h, e)) => loop_local_sinkable_closures(unit, func, h, e, j3),
        None => loop_local_sinkable_closures(unit, func, 0, func.code.len(), j3),
    };
    let has_inlinable_call = func.code.iter().enumerate().any(|(i, instr)| match instr {
        RegInstr::CallKnown { .. } => in_region(i) && !preserve_call_known(i),
        RegInstr::SpawnTask { .. } => in_region(i),
        RegInstr::CallClosure { closure, .. } => {
            let sinkable_call = sinkable.sink_calls.contains_key(closure);
            #[cfg(feature = "jit-speculation")]
            let speculative_call =
                monomorphic_closure_inline_target(unit, func, profile, call_count, i).is_some()
                    || polymorphic_closure_inline_targets(unit, func, profile, call_count, i)
                        .is_some();
            #[cfg(not(feature = "jit-speculation"))]
            let speculative_call = false;
            in_region(i) && (sinkable_call || speculative_call)
        }
        _ => false,
    });
    if !has_inlinable_call {
        let ip_map: Vec<usize> = (0..func.code.len()).collect();
        return Some((func.code.clone(), func.regs, ip_map));
    }

    let direct_call_results: Vec<usize> = func
        .code
        .iter()
        .enumerate()
        .filter_map(|(i, instr)| match instr {
            RegInstr::CallKnown { dst, .. } if in_region(i) && !preserve_call_known(i) => {
                Some(*dst)
            }
            RegInstr::SpawnTask { dst, .. } if in_region(i) => Some(*dst),
            _ => None,
        })
        .collect();

    /// A jump target to be resolved once all positions are known.
    enum Fix {
        /// Target is a caller instruction index (use `index_map`).
        Caller(usize),
        /// Like `Caller` but for a branch-shaped MATCH op in the OUTER caller body
        /// (`MatchOption`/`MatchResult`/`MatchVariant`/`MatchMapGet`): the caller's
        /// own indices shift when a leaf is spliced in, so both of its ip targets must
        /// be remapped through `index_map`. `second` selects the first/second target.
        CallerMatch { target: usize, second: bool },
        /// Target is a callee instruction index within splice `id` (use its `cmap`).
        Callee { id: usize, callee_target: usize },
        /// Like `Callee` but for a branch-shaped MATCH op spliced from a callee
        /// (`MatchOption`/`MatchResult`/`MatchVariant`/`MatchMapGet`): these carry TWO
        /// callee ip targets, so `which` selects the first (`false`) or second
        /// (`true`) target field to patch.
        CalleeMatch {
            id: usize,
            callee_target: usize,
            second: bool,
        },
        /// A `Return` jump to the shared join slot `slot` (use `joins[slot]`). A
        /// `CallKnown`/monomorphic-`CallClosure` splice owns a private join slot; a
        /// polymorphic dispatch shares ONE join slot across all of its arms.
        Join(usize),
    }
    struct Splice {
        cmap: Vec<usize>,
    }

    struct SpliceContext<'a> {
        unit: &'a RegUnit,
        j3: bool,
        new_code: &'a mut Vec<RegInstr>,
        ip_map: &'a mut Vec<usize>,
        fixups: &'a mut Vec<(usize, Fix)>,
        splices: &'a mut Vec<Splice>,
        joins: &'a mut Vec<usize>,
        next_reg: &'a mut usize,
    }

    let mut new_code: Vec<RegInstr> = Vec::new();
    // `ip_map[transformed_ip] = original_ip`, grown in lockstep with `new_code`.
    let mut ip_map: Vec<usize> = Vec::new();
    let mut index_map = vec![0usize; func.code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    let mut splices: Vec<Splice> = Vec::new();
    // Join positions, patched once known. One slot per `CallKnown`/mono splice; one
    // shared slot per polymorphic dispatch (all its arms target the same slot).
    let mut joins: Vec<usize> = Vec::new();
    let mut next_reg = func.regs;

    /// Splice one callee's reachable body into `new_code`, remapping its internal
    /// jumps and rewriting each `Return` into `Move dst <- result` + a `Jump` to the
    /// shared `join_slot`. Returns the splice id (registered in `splices` for
    /// `Fix::Callee` resolution). Args must already be moved into the `base` window.
    /// `None` if any callee instruction can't be offset into the native subset.
    fn splice_callee(
        context: &mut SpliceContext<'_>,
        callee: &RegFunction,
        dst: usize,
        base: usize,
        join_slot: usize,
        origin: usize,
    ) -> Option<()> {
        let unit = context.unit;
        let j3 = context.j3;
        let new_code = &mut *context.new_code;
        let ip_map = &mut *context.ip_map;
        let fixups = &mut *context.fixups;
        let splices = &mut *context.splices;
        let joins = &mut *context.joins;
        let next_reg = &mut *context.next_reg;
        let id = splices.len();
        let reachable = native_reachable_instructions(&callee.code);
        // Deopt-before-heap: a COLD arm (`cold[ci]`) is replaced by a single native
        // `Bail` (a `RuntimeError` sentinel) at the arm's start, with the rest of the arm
        // emitting nothing. Native bails at the arm start and NEVER executes the arm, and a
        // cold-arm `Bail` always takes the abort+replay fallback (it can never reach the
        // precise-resume path — see the soundness note in `deopt_replaceable_cold_arms`),
        // so the interpreter re-runs the loop and performs the arm itself — even when the
        // arm allocates, writes a (possibly caller-aliased) collection, or calls another
        // function. the transactional fallback contract holds unchanged.
        let (cold, arm_start) = deopt_replaceable_cold_arms(&callee.code, &reachable);
        let mut cmap = vec![0usize; callee.code.len()];
        let mut direct_spawn_results = vec![false; callee.regs];
        let mut direct_await_results = vec![false; callee.regs];
        for (ci, cinstr) in callee.code.iter().enumerate() {
            if !reachable[ci] {
                continue;
            }
            // Cold-arm interior (everything after the arm's start): emit nothing. The
            // interior is provably never a jump target from outside the arm (the
            // classifier enforces it), so no `cmap` entry is ever consulted; point it
            // at the arm's Bail for total safety.
            if cold[ci] && !arm_start[ci] {
                cmap[ci] = new_code.len();
                continue;
            }
            cmap[ci] = new_code.len();
            // Cold-arm start: a single `RuntimeError` ⇒ `JitInstr::Bail` at OSR-loop
            // translation. The arm's heap value is never built natively.
            if arm_start[ci] {
                new_code.push(RegInstr::RuntimeError {
                    message: String::new(),
                });
                ip_map.push(origin);
                continue;
            }
            match cinstr {
                RegInstr::SpawnTask {
                    dst: spawn_dst,
                    function,
                    args,
                } => {
                    let spawned = unit.functions.get(*function)?;
                    if !native_callee_inlinable_j3_with_spawns(unit, spawned, args.len()) {
                        return None;
                    }
                    let spawn_base = *next_reg;
                    *next_reg += spawned.regs;
                    for (param, arg) in args.iter().enumerate() {
                        new_code.push(RegInstr::Move {
                            dst: spawn_base + param,
                            src: base + arg,
                        });
                        ip_map.push(origin);
                    }
                    let spawned_join_slot = joins.len();
                    joins.push(0);
                    splice_callee(
                        &mut SpliceContext {
                            unit,
                            j3,
                            new_code,
                            ip_map,
                            fixups,
                            splices,
                            joins,
                            next_reg,
                        },
                        spawned,
                        base + spawn_dst,
                        spawn_base,
                        spawned_join_slot,
                        origin,
                    )?;
                    joins[spawned_join_slot] = new_code.len();
                    if *spawn_dst < direct_spawn_results.len() {
                        direct_spawn_results[*spawn_dst] = true;
                    }
                }
                RegInstr::AwaitJoin { dst, src }
                    if *src < direct_spawn_results.len() && direct_spawn_results[*src] =>
                {
                    new_code.push(RegInstr::Move {
                        dst: base + dst,
                        src: base + src,
                    });
                    ip_map.push(origin);
                    if *dst < direct_await_results.len() {
                        direct_await_results[*dst] = true;
                    }
                }
                RegInstr::Move { dst, src }
                    if *src < direct_spawn_results.len() && direct_spawn_results[*src] =>
                {
                    new_code.push(RegInstr::Move {
                        dst: base + dst,
                        src: base + src,
                    });
                    ip_map.push(origin);
                    if *dst < direct_spawn_results.len() {
                        direct_spawn_results[*dst] = true;
                    }
                }
                RegInstr::TryResult { dst, src, cleanup }
                    if cleanup.is_empty()
                        && *src < direct_await_results.len()
                        && direct_await_results[*src] =>
                {
                    new_code.push(RegInstr::TryResult {
                        dst: base + dst,
                        src: base + src,
                        cleanup: Vec::new(),
                    });
                    ip_map.push(origin);
                }
                RegInstr::Return { src } => {
                    new_code.push(RegInstr::Move {
                        dst,
                        src: base + src,
                    });
                    ip_map.push(origin);
                    fixups.push((new_code.len(), Fix::Join(join_slot)));
                    new_code.push(RegInstr::Jump { target: 0 });
                    ip_map.push(origin);
                }
                RegInstr::Jump { target } => {
                    fixups.push((
                        new_code.len(),
                        Fix::Callee {
                            id,
                            callee_target: *target,
                        },
                    ));
                    new_code.push(RegInstr::Jump { target: 0 });
                    ip_map.push(origin);
                }
                RegInstr::JumpIfBool {
                    cond,
                    expected,
                    target,
                } => {
                    fixups.push((
                        new_code.len(),
                        Fix::Callee {
                            id,
                            callee_target: *target,
                        },
                    ));
                    new_code.push(RegInstr::JumpIfBool {
                        cond: cond + base,
                        expected: *expected,
                        target: 0,
                    });
                    ip_map.push(origin);
                }
                RegInstr::JumpIfIntCompare {
                    lhs,
                    rhs,
                    op,
                    expected,
                    target,
                } => {
                    fixups.push((
                        new_code.len(),
                        Fix::Callee {
                            id,
                            callee_target: *target,
                        },
                    ));
                    new_code.push(RegInstr::JumpIfIntCompare {
                        lhs: lhs + base,
                        rhs: rhs + base,
                        op: *op,
                        expected: *expected,
                        target: 0,
                    });
                    ip_map.push(origin);
                }
                // scalar replacement branch-shaped match ops (OSR×inline only): two callee ip targets,
                // each remapped via the callee's `cmap`. The scalar replacement region passes dissolve
                // the matched variant/Option/struct afterward.
                RegInstr::MatchOption {
                    src,
                    some_ip,
                    none_ip,
                } if j3 => {
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *some_ip,
                            second: false,
                        },
                    ));
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *none_ip,
                            second: true,
                        },
                    ));
                    new_code.push(RegInstr::MatchOption {
                        src: src + base,
                        some_ip: 0,
                        none_ip: 0,
                    });
                    ip_map.push(origin);
                }
                RegInstr::MatchResult { src, ok_ip, err_ip } if j3 => {
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *ok_ip,
                            second: false,
                        },
                    ));
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *err_ip,
                            second: true,
                        },
                    ));
                    new_code.push(RegInstr::MatchResult {
                        src: src + base,
                        ok_ip: 0,
                        err_ip: 0,
                    });
                    ip_map.push(origin);
                }
                RegInstr::MatchVariant {
                    src,
                    expected,
                    match_ip,
                    else_ip,
                } if j3 => {
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *match_ip,
                            second: false,
                        },
                    ));
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *else_ip,
                            second: true,
                        },
                    ));
                    new_code.push(RegInstr::MatchVariant {
                        src: src + base,
                        expected: expected.clone(),
                        match_ip: 0,
                        else_ip: 0,
                    });
                    ip_map.push(origin);
                }
                RegInstr::MatchMapGet {
                    map,
                    key,
                    value_dst,
                    some_ip,
                    none_ip,
                }
                | RegInstr::MatchSortedMapGet {
                    map,
                    key,
                    value_dst,
                    some_ip,
                    none_ip,
                } if j3 => {
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *some_ip,
                            second: false,
                        },
                    ));
                    fixups.push((
                        new_code.len(),
                        Fix::CalleeMatch {
                            id,
                            callee_target: *none_ip,
                            second: true,
                        },
                    ));
                    let remapped = match cinstr {
                        RegInstr::MatchMapGet { .. } => RegInstr::MatchMapGet {
                            map: map + base,
                            key: key + base,
                            value_dst: value_dst + base,
                            some_ip: 0,
                            none_ip: 0,
                        },
                        RegInstr::MatchSortedMapGet { .. } => RegInstr::MatchSortedMapGet {
                            map: map + base,
                            key: key + base,
                            value_dst: value_dst + base,
                            some_ip: 0,
                            none_ip: 0,
                        },
                        _ => unreachable!(),
                    };
                    new_code.push(remapped);
                    ip_map.push(origin);
                }
                // A callee `RuntimeError` (e.g. the match-exhaustiveness fallback) is a
                // terminator with no registers: copy it through. In the native subset
                // it lowers to a bail, which re-runs the call from the interpreter —
                // sound because the inlined body is side-effect-free.
                RegInstr::RuntimeError { message } => {
                    new_code.push(RegInstr::RuntimeError {
                        message: message.clone(),
                    });
                    ip_map.push(origin);
                }
                pure => {
                    let offset = if j3 {
                        native_offset_regs_j3(pure, base)?
                    } else {
                        native_offset_regs(pure, base)?
                    };
                    new_code.push(offset);
                    ip_map.push(origin);
                }
            }
        }
        splices.push(Splice { cmap });
        Some(())
    }

    for (i, instr) in func.code.iter().enumerate() {
        index_map[i] = new_code.len();
        match instr {
            // OSR × closure-allocation sinking: DELETE the `MakeClosure` and the dead
            // copy `Move`s whose value is being sunk — the heap alloc is dissolved and
            // every call to it is inlined below. The captured registers stay live (the
            // inlined body materializes each via a `Move` at the call site). Emit
            // nothing; `index_map[i]` points at the next instruction so any branch to
            // this ip lands correctly.
            _ if sinkable.dead_defs.contains(&i) => {}
            // OSR × closure-allocation sinking: a `CallClosure` whose closure operand
            // is a loop-local non-escaping closure value (a `MakeClosure` dst, possibly
            // forwarded through copy `Move`s). The callee `k` is known STATICALLY (no
            // profile, no identity guard — the closure value never exists at runtime),
            // so we inline its body directly: materialize each scalar capture from the
            // `MakeClosure`'s (still-live) capture registers, bind the call args, and
            // splice the body. This is the sibling of the profile-guided inlining monomorphic path with the
            // guard removed and the captures sourced from the alloc site instead of a
            // heap closure handle.
            #[cfg(feature = "jit-speculation")]
            RegInstr::CallClosure {
                dst,
                closure,
                args,
                mut_args,
            } if in_region(i) && sinkable.sink_calls.contains_key(closure) => {
                debug_assert!(mut_args.is_empty());
                let k = sinkable.sink_calls[closure];
                let callee = unit.functions.get(k)?;
                // Read the unique sunk `MakeClosure`'s capture registers for THIS
                // closure operand. Keying by operand avoids confusing two sinkable
                // closures that share the same callee but capture different values.
                let captures = sinkable.capture_regs.get(closure)?.clone();
                if captures.len() != callee.captures {
                    return None;
                }
                let base = next_reg;
                next_reg += callee.regs;
                // Capture layout matches the profile-guided inlining inline path: capture regs `0..captures`
                // live BELOW the params. Materialize each capture by MOVING the alloc
                // site's (already-live) capture register into `base + k_cap` — no heap
                // closure ever exists, so there is no `NativeClosureCapture` read. A
                // non-scalar capture is caught by the downstream OSR type inference
                // (Int/Bool/Float only), so the body simply fails to compile.
                for (k_cap, &cap_reg) in captures.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + k_cap,
                        src: cap_reg,
                    });
                    ip_map.push(i);
                }
                for (param, arg) in args.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + callee.captures + param,
                        src: *arg,
                    });
                    ip_map.push(i);
                }
                let join_slot = joins.len();
                joins.push(0);
                splice_callee(
                    &mut SpliceContext {
                        unit,
                        j3,
                        new_code: &mut new_code,
                        ip_map: &mut ip_map,
                        fixups: &mut fixups,
                        splices: &mut splices,
                        joins: &mut joins,
                        next_reg: &mut next_reg,
                    },
                    callee,
                    *dst,
                    base,
                    join_slot,
                    i,
                )?;
                joins[join_slot] = new_code.len();
            }
            RegInstr::CallKnown {
                dst,
                function,
                args,
                mut_args,
            } if in_region(i) && !preserve_call_known(i) => {
                let callee0 = unit.functions.get(*function)?;
                // #7 foldable cold-arm sub-case: fold the callee's whole body first (a
                // measured-throwaway-string arm dissolves to scalar arithmetic), so a leaf
                // that was non-inlinable only because of that heap arm becomes inlinable.
                // The fold is semantics-preserving and a no-op for ordinary bodies. Use the
                // folded body CONSISTENTLY for both the inlinability verdict and the splice
                // below (candidacy applies the same fold) — an inconsistency only ever
                // declines OSR, never miscompiles.
                let folded_callee = if j3 {
                    native_string_folded_callee(callee0)
                } else {
                    None
                };
                let callee = folded_callee.as_ref().unwrap_or(callee0);
                // `mut` args need write-back at the callee join point, just like the
                // interpreter frame completion path. Heap copy-updated params are
                // now ordinary handle registers by that point, so a `Move` back to
                // the caller arg slot preserves the same semantics without a frame.
                // Under `j3` (OSR×inline) a callee that builds/destructures a
                // non-escaping Option/variant/struct also qualifies — it dissolves
                // post-inline.
                let inlinable = if j3 {
                    native_callee_inlinable_j3_with_spawns(unit, callee, args.len())
                } else {
                    native_callee_inlinable(callee, args.len())
                };
                if !inlinable {
                    return None;
                }
                let base = next_reg;
                next_reg += callee.regs;
                for (param, arg) in args.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + param,
                        src: *arg,
                    });
                    ip_map.push(i);
                }
                let join_slot = joins.len();
                joins.push(0);
                splice_callee(
                    &mut SpliceContext {
                        unit,
                        j3,
                        new_code: &mut new_code,
                        ip_map: &mut ip_map,
                        fixups: &mut fixups,
                        splices: &mut splices,
                        joins: &mut joins,
                        next_reg: &mut next_reg,
                    },
                    callee,
                    *dst,
                    base,
                    join_slot,
                    i,
                )?;
                joins[join_slot] = new_code.len();
                for &pos in mut_args {
                    new_code.push(RegInstr::Move {
                        dst: args[pos],
                        src: base + pos,
                    });
                    ip_map.push(i);
                }
            }
            RegInstr::SpawnTask {
                dst,
                function,
                args,
            } if in_region(i) => {
                let callee = unit.functions.get(*function)?;
                if !j3 || !native_callee_inlinable_j3_with_spawns(unit, callee, args.len()) {
                    return None;
                }
                let base = next_reg;
                next_reg += callee.regs;
                for (param, arg) in args.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + param,
                        src: *arg,
                    });
                    ip_map.push(i);
                }
                let join_slot = joins.len();
                joins.push(0);
                splice_callee(
                    &mut SpliceContext {
                        unit,
                        j3,
                        new_code: &mut new_code,
                        ip_map: &mut ip_map,
                        fixups: &mut fixups,
                        splices: &mut splices,
                        joins: &mut joins,
                        next_reg: &mut next_reg,
                    },
                    callee,
                    *dst,
                    base,
                    join_slot,
                    i,
                )?;
                joins[join_slot] = new_code.len();
            }
            RegInstr::AwaitJoin { dst, src }
                if in_region(i) && direct_call_results.contains(src) =>
            {
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: *src,
                });
                ip_map.push(i);
            }
            #[cfg(feature = "jit-speculation")]
            RegInstr::CallClosure {
                dst,
                closure,
                args,
                mut_args,
            } if in_region(i)
                && monomorphic_closure_inline_target(unit, func, profile, call_count, i)
                    .is_some() =>
            {
                // profile-guided monomorphic inlining: bounded type profiled this site as
                // calling exactly one callee `k` (non-capturing, native-inlinable).
                // Guard the closure's identity, then inline `k`'s body. On a callee
                // mismatch the guard bails (re-run-from-top fallback), so the
                // interpreter handles the unexpected closure — sound because the
                // inlined subset is side-effect-free. `mut_args` is always empty for
                // an inlinable (side-effect-free) callee; the target check enforces it.
                debug_assert!(mut_args.is_empty());
                let k = monomorphic_closure_inline_target(unit, func, profile, call_count, i)?;
                let callee = unit.functions.get(k)?;
                // Identity guard up front: bail before any inlined work if the
                // observed closure isn't the speculated callee `k`.
                new_code.push(RegInstr::NativeGuardClosureId {
                    closure: *closure,
                    expected: k,
                });
                ip_map.push(i);
                let base = next_reg;
                next_reg += callee.regs;
                // Capturing-closure inlining (OSR × profile-guided inlining): a closure callee lays out
                // its capture registers `0..captures` BELOW its params, so the
                // splice window is `[captures.. params.. locals]`. Materialize each
                // scalar capture into `base + k` via the host helper, then bind the
                // call args ABOVE the captures at `base + captures + param`. For a
                // non-capturing callee (`captures == 0`) this is exactly the shipped
                // path: no `NativeClosureCapture`, args at `base + param`.
                for k_cap in parallel_indices(0..callee.captures) {
                    new_code.push(RegInstr::NativeClosureCapture {
                        dst: base + k_cap,
                        closure: *closure,
                        index: k_cap,
                    });
                    ip_map.push(i);
                }
                for (param, arg) in args.iter().enumerate() {
                    new_code.push(RegInstr::Move {
                        dst: base + callee.captures + param,
                        src: *arg,
                    });
                    ip_map.push(i);
                }
                let join_slot = joins.len();
                joins.push(0);
                splice_callee(
                    &mut SpliceContext {
                        unit,
                        j3,
                        new_code: &mut new_code,
                        ip_map: &mut ip_map,
                        fixups: &mut fixups,
                        splices: &mut splices,
                        joins: &mut joins,
                        next_reg: &mut next_reg,
                    },
                    callee,
                    *dst,
                    base,
                    join_slot,
                    i,
                )?;
                joins[join_slot] = new_code.len();
            }
            RegInstr::CallClosure {
                dst,
                closure,
                args,
                mut_args,
            } if in_region(i)
                && polymorphic_closure_inline_targets(unit, func, profile, call_count, i)
                    .is_some() =>
            {
                // polymorphic inline cache: bounded type profiled this site as calling 2–3
                // distinct callees, EVERY one non-capturing and native-inlinable. Read
                // the closure's function id ONCE, then dispatch: `if id == Kj { inline
                // body of Kj; jump join }` for each speculated callee; if NONE match,
                // bail via the existing re-run-from-top fallback (a `RuntimeError`,
                // which lowers to `JitInstr::Bail`). Sound: every inlined body is
                // side-effect-free, so re-running from the top on a miss is safe —
                // identical discipline to monomorphic inlining's single-guard bail.
                debug_assert!(mut_args.is_empty());
                let targets =
                    polymorphic_closure_inline_targets(unit, func, profile, call_count, i)?;
                // Scratch registers for the dispatch: the id (read once), a per-arm
                // key constant, and the equality result. They live above every
                // inlined callee's window, so no arm can clobber them.
                let id_reg = next_reg;
                let key_reg = next_reg + 1;
                let eq_reg = next_reg + 2;
                next_reg += 3;
                // Read the closure's function id exactly once.
                new_code.push(RegInstr::NativeClosureId {
                    dst: id_reg,
                    closure: *closure,
                });
                ip_map.push(i);
                // One shared join past the last arm: every arm's `Return` jumps here.
                let join_slot = joins.len();
                joins.push(0);
                // Dispatch prologue: for each callee `Kj`, compare `id == Kj` and, on
                // match, jump to that arm (target patched once the arm is emitted).
                // Record one fixup slot per arm to backpatch its branch target.
                let mut arm_branch_pos: Vec<usize> = Vec::with_capacity(targets.len());
                for &k in &targets {
                    new_code.push(RegInstr::LoadInt {
                        dst: key_reg,
                        value: k as i64,
                    });
                    ip_map.push(i);
                    new_code.push(RegInstr::Equal {
                        dst: eq_reg,
                        lhs: id_reg,
                        rhs: key_reg,
                    });
                    ip_map.push(i);
                    arm_branch_pos.push(new_code.len());
                    new_code.push(RegInstr::JumpIfBool {
                        cond: eq_reg,
                        expected: true,
                        target: 0, // patched to the arm start below
                    });
                    ip_map.push(i);
                }
                // No-match: bail to the interpreter (re-run from the top). Sound
                // because every candidate body is side-effect-free.
                new_code.push(RegInstr::RuntimeError {
                    message: String::new(),
                });
                ip_map.push(i);
                // Emit each arm's inlined body; record its start to patch the branch.
                for (arm, &k) in targets.iter().enumerate() {
                    let callee = unit.functions.get(k)?;
                    let arm_start = new_code.len();
                    // Backpatch this arm's dispatch branch to its body start.
                    if let RegInstr::JumpIfBool { target, .. } = &mut new_code[arm_branch_pos[arm]]
                    {
                        *target = arm_start;
                    }
                    let base = next_reg;
                    next_reg += callee.regs;
                    // Capturing-closure inline (OSR × polymorphic inline cache): like the monomorphic
                    // path, a closure callee lays out its capture registers `0..
                    // captures` BELOW its params. Materialize each scalar capture
                    // from THIS arm's matched closure handle (`*closure`) into
                    // `base + k_cap`, then bind the call args ABOVE the captures at
                    // `base + captures + param`. For a non-capturing callee
                    // (`captures == 0`) this is exactly the shipped path (args at
                    // `base + param`).
                    for k_cap in parallel_indices(0..callee.captures) {
                        new_code.push(RegInstr::NativeClosureCapture {
                            dst: base + k_cap,
                            closure: *closure,
                            index: k_cap,
                        });
                        ip_map.push(i);
                    }
                    for (param, arg) in args.iter().enumerate() {
                        new_code.push(RegInstr::Move {
                            dst: base + callee.captures + param,
                            src: *arg,
                        });
                        ip_map.push(i);
                    }
                    splice_callee(
                        &mut SpliceContext {
                            unit,
                            j3,
                            new_code: &mut new_code,
                            ip_map: &mut ip_map,
                            fixups: &mut fixups,
                            splices: &mut splices,
                            joins: &mut joins,
                            next_reg: &mut next_reg,
                        },
                        callee,
                        *dst,
                        base,
                        join_slot,
                        i,
                    )?;
                }
                // Shared join lands just past the final arm.
                joins[join_slot] = new_code.len();
            }
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Caller(*target)));
                new_code.push(instr.clone());
                ip_map.push(i);
            }
            // OUTER-caller branch-shaped match ops: when a leaf is inlined, the
            // caller's instruction indices shift, so the two ip targets of an outer
            // `MatchResult`/`MatchOption`/`MatchVariant`/`MatchMapGet` must be remapped
            // through `index_map` (the scalar replacement region passes later dissolve these). Without
            // a region leaf inline these copy-through unchanged (identity index_map).
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *some_ip,
                        second: false,
                    },
                ));
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *none_ip,
                        second: true,
                    },
                ));
                new_code.push(instr.clone());
                ip_map.push(i);
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *ok_ip,
                        second: false,
                    },
                ));
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *err_ip,
                        second: true,
                    },
                ));
                new_code.push(instr.clone());
                ip_map.push(i);
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *match_ip,
                        second: false,
                    },
                ));
                fixups.push((
                    new_code.len(),
                    Fix::CallerMatch {
                        target: *else_ip,
                        second: true,
                    },
                ));
                new_code.push(instr.clone());
                ip_map.push(i);
            }
            other => {
                new_code.push(other.clone());
                ip_map.push(i);
            }
        }
    }

    for (pos, fix) in fixups {
        // For a `*Match`, remember which of the two match targets to patch.
        let second = matches!(
            fix,
            Fix::CalleeMatch { second: true, .. } | Fix::CallerMatch { second: true, .. }
        );
        let is_match = matches!(fix, Fix::CalleeMatch { .. } | Fix::CallerMatch { .. });
        let target = match fix {
            Fix::Caller(t) | Fix::CallerMatch { target: t, .. } => index_map[t],
            Fix::Callee { id, callee_target }
            | Fix::CalleeMatch {
                id, callee_target, ..
            } => splices[id].cmap[callee_target],
            Fix::Join(slot) => joins[slot],
        };
        match &mut new_code[pos] {
            // Single-target branches.
            RegInstr::Jump { target: t }
            | RegInstr::JumpIfBool { target: t, .. }
            | RegInstr::JumpIfIntCompare { target: t, .. }
                if !is_match =>
            {
                *t = target
            }
            // Two-target match ops: patch the first/second ip per `second`.
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                if second {
                    *none_ip = target;
                } else {
                    *some_ip = target;
                }
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                if second {
                    *err_ip = target;
                } else {
                    *ok_ip = target;
                }
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                if second {
                    *else_ip = target;
                } else {
                    *match_ip = target;
                }
            }
            _ => {}
        }
    }
    debug_assert_eq!(ip_map.len(), new_code.len());
    Some((new_code, next_reg, ip_map))
}

/// JIT side-table native-status value: the function is known not native-eligible.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) const NATIVE_STATUS_NOT_ELIGIBLE: u8 = 1;
#[cfg(feature = "jit-speculation")]
pub(in crate::reg_vm) const NATIVE_STATUS_PROFILE_PENDING: u8 = 2;

/// Consecutive runtime-bail count at which the native tier gives up on a
/// structurally-eligible function (predict-and-skip, like a JSC/V8 deopt count).
/// A function that passes the structural predictor but bails on *every* call
/// (arg-type mismatch or a runtime guard) otherwise re-compiles/marshals/bails
/// forever; after this many consecutive bails we mark it `NOT_ELIGIBLE` so the
/// cheap-negative early-return in `try_native` short-circuits all future calls.
/// Counter resets on any successful native completion, so a hot function that
/// bails only on a rare data edge keeps its fast path. Candidate for the
/// data-driven tuning of the native-tier give-up heuristic.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) const NATIVE_BAIL_GIVEUP_THRESHOLD: u32 = 3;

/// Native-dispatch count at which the native tier gives up on a *back-edge-free*
/// (loop-free) function and demotes it to `NOT_ELIGIBLE` (a profitability
/// governor, mirroring the `NATIVE_BAIL_GIVEUP_THRESHOLD` predict-and-skip).
///
/// Invariant: a native body with no internal back-edge does bounded O(1) work
/// per dispatch, so calling it once per interpreter loop iteration (a tiny leaf
/// or closure body — `closure_alloc`, `option_result_chain`) pays FFI +
/// marshalling cost it can never amortize across the body's own iterations. A
/// *loop-bearing* body (the whole hot loop compiled into one native call —
/// `native_scalar_loop`, every `osr_*` kernel) does O(n) work per dispatch and
/// is dispatched `calls=1`, so it amortizes the FFI cost and is EXEMPT here
/// regardless of call count. The combination (back-edge-free AND dispatched many
/// times) is exactly the diagnosed per-iteration-dispatch loss pattern.
///
/// `K = 64` is large enough that a `calls=1` whole-loop win NEVER trips it and
/// small enough to cap the wasted FFI churn at a fixed bounded prefix before the
/// function falls back to the cheap interpreter path for the rest of the loop.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) const NATIVE_NOAMORTIZE_GIVEUP: u32 = 64;

/// Interpreted-work units at which a counting OSR candidate fires `try_osr`.
/// Tiny loop bodies need more backedges to accumulate this much work, while
/// genuinely heavy loops cross it quickly and then run native for the rest of
/// their life. The explicit eager plan uses threshold 0.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) const OSR_BACKEDGE_THRESHOLD: u32 = 1000;
