//! Natural-loop discovery and OSR region metadata.

use super::*;

/// A single natural loop identified for OSR (J5.2): the conservative shape this
/// slice compiles. `header` is the loop's entry instruction (a conditional branch
/// that is the target of the loop's backedge); `exit` is the post-loop instruction
/// the header's branch leaves to. Native execution OSR-enters at `header` and
/// OSR-exits (deopts) at `exit`.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) struct OsrLoop {
    pub(in crate::reg_vm) header: usize,
    pub(in crate::reg_vm) exit: usize,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) struct OsrDerivedLiveIn {
    pub(in crate::reg_vm) native_reg: usize,
    pub(in crate::reg_vm) base_reg: usize,
    pub(in crate::reg_vm) field_slot: usize,
    pub(in crate::reg_vm) ty: NativeTy,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) struct OsrScalarField {
    pub(in crate::reg_vm) native_reg: usize,
    pub(in crate::reg_vm) base_reg: usize,
    pub(in crate::reg_vm) field_slot: usize,
    pub(in crate::reg_vm) writeback: bool,
}

/// A compiled OSR loop cached per function. The OSR loop is detected and compiled
/// on the (possibly J3-scalar-replaced) `code`, so its native `resume_ip` indexes
/// that transformed stream. The interpreter, however, executes the ORIGINAL
/// `func.code`; the two stored `orig_*` ips translate the OSR boundary back:
///   - `orig_header`: the original-code header ip the interpreter must be at for
///     the OSR to fire (the header gate). When no Option was scalar-replaced this
///     equals `trans_exit`'s loop header in original space (identity ip-map).
///   - `trans_exit`: the transformed-code exit ip (= the native `resume_ip` the
///     OSR-exit deopt reports). Used to validate the deopt resumed at the loop's
///     single exit.
///   - `orig_exit`: the ORIGINAL-code post-loop ip the interpreter resumes at —
///     `ip_map[trans_exit]`. Set the frame ip to this after an OSR-exit.
/// Loop-carried (live-in/out) registers keep their original indices (J3 only adds
/// fresh tag/payload regs used strictly inside the loop and dead at both
/// boundaries), so the marshalling window and live-out restore are unchanged.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone)]
pub(in crate::reg_vm) struct OsrEntry {
    pub(in crate::reg_vm) id: vm_jit::CompiledId,
    pub(in crate::reg_vm) orig_header: usize,
    pub(in crate::reg_vm) trans_exit: usize,
    pub(in crate::reg_vm) orig_exit: usize,
    /// Width of the OSR register window the native ABI expects — the TRANSFORMED
    /// register count (`func.regs` plus any J3-added tag/payload regs). The
    /// marshalling window and `lens` slice must be exactly this wide.
    pub(in crate::reg_vm) n_jit_regs: usize,
    pub(in crate::reg_vm) param_types: Vec<NativeTy>,
    pub(in crate::reg_vm) derived_liveins: Vec<OsrDerivedLiveIn>,
    pub(in crate::reg_vm) scalar_fields: Vec<OsrScalarField>,
    pub(in crate::reg_vm) heap_input_regs: Vec<usize>,
    /// Per-register native types of the compiled OSR body. Used at OSR-exit to skip
    /// restoring **Handle**-class registers: a loop-internal handle (a stored
    /// struct/closure fetched via `FieldHandle`/`ListGetHandle`) is dead at the exit
    /// and its live-out "value" is only a heap-table index — restoring it as an Int
    /// into the interpreter slot would corrupt the register. The interpreter re-
    /// derives any still-needed heap value; a dead one is simply never read.
    pub(in crate::reg_vm) reg_types: Vec<NativeTy>,
    /// Registers written by the native OSR loop body. Live-through registers that
    /// are assigned before the loop but never written natively must not be restored
    /// from the scalar deopt payload: a heap/list live-through slot is already
    /// correct in the interpreter window, while its native payload word may be an
    /// opaque handle-table index or an untyped zero.
    pub(in crate::reg_vm) written_regs: Vec<bool>,
    pub(in crate::reg_vm) string_literals: Vec<Rc<String>>,
    /// Bounded clean-exit reconstruction trees for scalar-replaced aggregates that
    /// remain live after the OSR region. Every leaf is verified as a scalar or Handle
    /// register before this entry is cached.
    pub(in crate::reg_vm) materialize_recipes: Vec<super::passes::OsrMaterializeRecipe>,
}

/// Detect one natural loop at a specific header, allowing other disjoint loops
/// elsewhere in the same function. This is intentionally narrower than arbitrary
/// CFG loop discovery: it uses the same single-entry/single-exit validation as
/// [`detect_single_natural_loop`] for the selected `[header, exit)` region, but it
/// does not reject merely because a setup loop exists before the hot loop.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn detect_natural_loop_at(
    code: &[RegInstr],
    header: usize,
) -> Option<OsrLoop> {
    let n = code.len();
    if header >= n {
        return None;
    }
    let backedges = NativeRegionCfg::prefix(code, code.len())?.backedges_to(header);
    if backedges.is_empty() {
        return None;
    }
    let body_end = backedges.into_iter().max().unwrap();

    let mut cond_ip = header;
    while cond_ip < n
        && !matches!(
            code[cond_ip],
            RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. }
        )
    {
        cond_ip += 1;
    }
    if cond_ip > body_end {
        return None;
    }
    let exit = match &code[cond_ip] {
        RegInstr::JumpIfIntCompare { target, .. } | RegInstr::JumpIfBool { target, .. } => *target,
        _ => return None,
    };
    if exit <= body_end || exit > n {
        return None;
    }

    for i in header..=body_end {
        let in_region = |t: usize| t >= header && t < exit;
        match &code[i] {
            RegInstr::Jump { target } => {
                if !in_region(*target) {
                    return None;
                }
            }
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                if i == cond_ip {
                    continue;
                }
                if !in_region(*target) {
                    return None;
                }
            }
            RegInstr::Return { .. } => return None,
            RegInstr::RuntimeError { .. } => {}
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                if !in_region(*some_ip) || !in_region(*none_ip) {
                    return None;
                }
            }
            _ => {}
        }
    }

    for (i, instr) in code.iter().enumerate() {
        if i >= header && i < exit {
            continue;
        }
        let enters_interior = |t: usize| t > header && t < exit;
        let bad = match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => enters_interior(*target),
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => enters_interior(*some_ip) || enters_interior(*none_ip),
            _ => false,
        };
        if bad {
            return None;
        }
    }
    Some(OsrLoop { header, exit })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn detect_natural_loops(code: &[RegInstr]) -> Vec<OsrLoop> {
    let mut headers = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        let mut push_header = |target: usize| {
            if target <= i && !headers.contains(&target) {
                headers.push(target);
            }
        };
        match instr {
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                push_header(*some_ip);
                push_header(*none_ip);
            }
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => push_header(*target),
            _ => {}
        }
    }
    headers.sort_unstable();
    headers
        .into_iter()
        .filter_map(|header| detect_natural_loop_at(code, header))
        .collect()
}

/// Identify the single natural loop OSR will compile, **conservatively** (any
/// shape we cannot analyze soundly returns `None`, so OSR does not apply).
///
/// The accepted shape is a **reducible natural loop with a single header `h`**,
/// lowered as `while cond { body }` (the body may contain internal forward control
/// flow, e.g. an `if x { ... }` reset):
///   - `header` `h`: a `JumpIfIntCompare`/`JumpIfBool` at `h` whose `target` is the
///     post-loop `exit` (the branch *leaves* the loop; fall-through stays in body),
///   - one or more **backedges** `b → h` (a `Jump`/`JumpIf*`/`MatchOption` arm whose
///     target is `≤ b`), **ALL targeting the same header `h`** (multiple backedges
///     are collapsed; backedges to two different headers ⇒ nested/multiple loops ⇒
///     reject), and
///   - the contiguous region `[h, exit)` is **single-exit**: the ONLY edge leaving
///     it is the header's exit edge to `exit`. Every other in-body branch (forward
///     `if`/`match`, or a backedge) stays within `[h, exit)`. No in-body
///     `Return` (a value-producing extra exit) is allowed.
///
/// A single header (all backedges collapsed), a single exit edge, and a contiguous
/// `[h, exit)` body make the region single-entry/single-exit — the only thing we can
/// OSR soundly. Multi-header / multi-exit / non-contiguous shapes return `None`.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn detect_single_natural_loop(code: &[RegInstr]) -> Option<OsrLoop> {
    let n = code.len();
    // Collect backedges from the shared native CFG descriptor. This matters when
    // this runs on UNTRANSFORMED code (OSR x J3, before scalar replacement): a
    // match arm that jumps backward must be treated like any other control edge.
    let backedges = NativeRegionCfg::prefix(code, code.len())?.backedges();
    // At least one backedge ⇒ a loop exists.
    if backedges.is_empty() {
        return None;
    }
    // Collapse multiple backedges to the SAME header. Backedges to two DIFFERENT
    // headers mean nested/sibling loops — out of scope, reject. The single shared
    // header `h` is the loop entry; `body_end` is the furthest backedge source, so
    // the contiguous loop body is `h..=body_end`.
    let header = backedges[0].1;
    if backedges.iter().any(|&(_, h)| h != header) {
        return None;
    }
    if header >= n {
        return None;
    }
    let body_end = backedges.iter().map(|&(from, _)| from).max().unwrap();
    // The header BLOCK is a (possibly empty) leading run of STRAIGHT-LINE instructions
    // (no jump/branch/match/return) followed by the loop's conditional branch. This
    // admits a `while cond { body }` whose CONDITION computes a value before the
    // compare — e.g. `while i < Bytes.len(data)` lowers to `BytesLen -> t; JumpIf i <
    // t` with the backedge targeting the `BytesLen`. The Bytes length-fold then (a)
    // rewrites that in-header `Bytes.len` to a `Move` from a constant length register
    // and (b) materializes the constant length as a `LoadInt` AT the header, so the
    // value is definitely-assigned on entry to the native OSR header block (which is
    // where OSR-entry lands) and dominates the condition's read. Because the prefix is
    // straight-line, the block is single-entry (the backedge targets `header`, the
    // sole entry); if the prefix is not foldable to the native subset, `translate_osr_
    // loop` rejects it and the loop simply stays on the interpreter (safe). A loop
    // whose condition is a bare compare has `cond_ip == header`, exactly as before, so
    // ordinary loops are unaffected.
    let mut cond_ip = header;
    while cond_ip < n
        && !matches!(
            code[cond_ip],
            RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. }
        )
    {
        cond_ip += 1;
    }
    if cond_ip > body_end {
        return None;
    }
    // The condition must be a `JumpIfIntCompare`/`JumpIfBool` whose `target` is the
    // post-loop exit (the fall-through stays in the loop body). The exit must lie
    // outside the body.
    let exit = match &code[cond_ip] {
        RegInstr::JumpIfIntCompare { target, .. } | RegInstr::JumpIfBool { target, .. } => *target,
        _ => return None,
    };
    // The loop body is `header..=body_end`; the exit must be after it (the loop's
    // only way out). A header whose exit target points back inside the body is not
    // the while-shape we accept.
    if exit <= body_end || exit > n {
        return None;
    }
    // The set of backedge source indices (each must be a Jump/JumpIf* back to the
    // header; checked in-region below — a backedge to `header` is in `[header, exit)`,
    // so it is NOT an escaping edge and needs no special exemption).
    //
    // No instruction in the body `header..=body_end` may transfer control outside
    // `[header, exit)` except the header's own exit edge. (Any other escape would
    // mean multiple exits / an irreducible shape.) Internal forward branches and
    // backedges to `header` stay in-region, so they pass the same `in_region` test.
    for i in header..=body_end {
        let in_region = |t: usize| t >= header && t < exit;
        match &code[i] {
            RegInstr::Jump { target } => {
                if !in_region(*target) {
                    return None;
                }
            }
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                // The header condition's exit edge to `exit` is the sole permitted
                // escape (the condition sits at `cond_ip`, after any `LoadInt` prefix).
                if i == cond_ip {
                    continue;
                }
                if !in_region(*target) {
                    return None;
                }
            }
            // A `Return` inside the loop is a value-producing exit we do not model in
            // the single OSR-exit — bail conservatively.
            RegInstr::Return { .. } => return None,
            // A `RuntimeError` inside the loop is a trap, not a normal loop exit. It
            // is compiled to `JitInstr::Bail` (deopt to the interpreter, which then
            // re-runs the loop and raises the error itself if actually reached). The
            // exhaustive-match lowering emits a statically-reachable-but-dynamically-
            // dead `RuntimeError` after an `Option` match, so accepting it (as a bail)
            // is what lets Option-bearing loops OSR at all.
            RegInstr::RuntimeError { .. } => {}
            // `MatchOption` (untransformed-code path): both arms must stay in-region.
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                if !in_region(*some_ip) || !in_region(*none_ip) {
                    return None;
                }
            }
            // Any non-straight-line call inside the body is rejected by the subset
            // check in `translate_osr_loop`; control-flow-wise it falls through,
            // which stays in-region.
            _ => {}
        }
    }
    // Single-ENTRY check: OSR enters the region only at `header`. No instruction
    // OUTSIDE `[header, exit)` may branch INTO the body interior `(header, exit)`
    // (an edge to `header` itself is the legal loop entry / fall-through). An
    // external edge into the middle would make the region multi-entry and the
    // contiguous-region/ip-map assumptions unsound, so reject. (Lowered while-loops
    // never do this; this guards an irreducible CFG defensively.)
    for (i, instr) in code.iter().enumerate() {
        if i >= header && i < exit {
            continue; // in-body edges already validated above
        }
        let enters_interior = |t: usize| t > header && t < exit;
        let bad = match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => enters_interior(*target),
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => enters_interior(*some_ip) || enters_interior(*none_ip),
            _ => false,
        };
        if bad {
            return None;
        }
    }
    Some(OsrLoop { header, exit })
}
