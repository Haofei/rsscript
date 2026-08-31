//! Option/Result scalar-replacement and string-length-fold region passes,
//! split out of `region_optimization.rs` for module-size partitioning. All
//! `pub(in crate::reg_vm)`, re-exported through the passes module glob.

use super::*;

// Private region-rewrite alias, matching the sibling pass modules which each keep their own.
type RegionRewrite<Recipe> = (Vec<RegInstr>, usize, Vec<usize>, Vec<Recipe>);

/// OSR × scalar replacement (string length-law folding): a non-escaping string register `s`
/// inside the loop region `[header, exit)` whose EVERY use is `String.len(s)`,
/// another foldable producer's operand, or a `Move` to a foldable register, can
/// have its allocation DELETED — every `String.len(s)` is replaced by arithmetic
/// on operand lengths, and the producer instruction(s) are dropped. This stays
/// READ-ONLY (it removes allocations, never performs a heap write — Exec Spec
/// the transactional fallback contract holds), so a length-only string loop becomes pure-scalar and OSRs.
///
/// VERIFIED length laws (against the interpreter's exact `String` semantics):
/// - `String.len` is the BYTE length (`str::len`, see [`RegIntrinsic::StringLen`]).
/// - `String.concat` is byte concatenation (see [`RegInstr::StringConcat`]), so
///   `len(concat(a,b)) = len(a) + len(b)` exactly, REGARDLESS of encoding.
/// - `String.from_int(k)` is `i64::to_string()`: ASCII decimal digits with a
///   leading `-` for negatives, `"0"` for zero, all bytes 1-wide. Its byte length
///   is the decimal-digit count (`+1` for the sign when `k < 0`), computed natively
///   by a forward branch ladder that handles `0`, negatives, and `i64::MIN` (which
///   cannot be negated) by comparing `k` directly against ± powers of ten.
/// - `String.slice(s, start, n)` (see [`string_slice_range`]) clamps to CHAR
///   boundaries in BYTE units: `bs = clamp_cb(s, max(start,0))`,
///   `be = clamp_cb(s, min(bs + max(n,0), len(s)))`, result byte-length `be - bs`.
///   The char-boundary clamp depends on the actual bytes of `s`, so the law is only
///   provable when `s` is ASCII (every byte is a boundary ⇒ clamp is identity):
///   `len = min(min(max(start,0), L) + max(n,0), L) - min(max(start,0), L)` with
///   `L = len(s)`. A slice of a NON-ASCII (unprovably-ASCII) string ⇒ NOT foldable.
/// - `LoadString` / string `Move`: constant byte length / alias of the source.
///
/// Conservative bails (when unsure REJECT ⇒ no OSR, never unsound): any escaping
/// string use (stored / returned / captured / compared / passed to a non-`len`
/// intrinsic / live at a loop boundary); a `String.slice` of an unprovably-ASCII
/// string; a `String.len` whose source is not a fully-foldable producer. A leaf
/// non-foldable `String.len` is simply left un-folded — the `StringLen` host helper
/// IS native-subset (a plain `String.len` loop OSRs), so it runs as a host call rather
/// than blocking OSR; this pass only declines to FOLD it. `RegFootprint::All` ⇒ bail.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→
/// original `ip_map` discipline as the sibling region passes. Identity (no
/// foldable `String.len`) ⇒ code unchanged with an identity ip-map.
#[cfg(feature = "native-jit")]
// Rewrites coordinate code and source-IP maps by the original instruction index.
pub(in crate::reg_vm) fn native_string_length_fold_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // The only thing this pass targets is an in-region `String.len`. Without one,
    // there is nothing to fold ⇒ identity (plain OSR, byte-for-byte the old path).
    // The length-query *classification* reads the central registry's
    // `string_fold_role`; the single-arg shape check stays here.
    let is_string_len = |instr: &RegInstr| {
        matches!(
            instr,
            RegInstr::CallIntrinsic { intrinsic, args, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, args, .. }
                if args.len() == 1
                    && intrinsic_descriptor(*intrinsic).string_fold_role
                        == Some(StringFoldRole::LengthQuery)
        )
    };
    if !(header..exit).any(|i| is_string_len(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // Classify each in-region string producer. A producer register is a candidate
    // iff it is defined EXACTLY ONCE in-region by a foldable op and never defined
    // out-of-region. `ascii` records whether the produced string is provably ASCII
    // (needed only for the slice length law).
    #[derive(Clone)]
    enum Producer {
        Literal {
            len: i64,
            ascii: bool,
        },
        FromInt {
            src: usize,
        },
        Concat {
            left: usize,
            right: usize,
        },
        Slice {
            src: usize,
            start: usize,
            len: usize,
        },
        Alias {
            src: usize,
        }, // string `Move`
    }
    let mut producer: Vec<Option<Producer>> = vec![None; n_regs];
    let mut multiply_defined = vec![false; n_regs];

    let slice_args = |args: &[usize]| -> Option<(usize, usize, usize)> {
        if args.len() == 3 {
            Some((args[0], args[1], args[2]))
        } else {
            None
        }
    };
    for i in parallel_indices(header..exit) {
        let dst_prod: Option<(usize, Producer)> = match &code[i] {
            RegInstr::LoadString { dst, value } => Some((
                *dst,
                Producer::Literal {
                    len: value.len() as i64,
                    ascii: value.is_ascii(),
                },
            )),
            RegInstr::StringConcat { dst, left, right } => Some((
                *dst,
                Producer::Concat {
                    left: *left,
                    right: *right,
                },
            )),
            // Foldable string *producer* intrinsics — recognized via the central
            // registry's `string_fold_role`; the per-role operand extraction and the
            // length laws stay here. (`String.from_int` is recognized only in the
            // untyped `CallIntrinsic` form, exactly as before; `String.slice` in both
            // the untyped and typed forms.)
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            } if intrinsic_descriptor(*intrinsic).string_fold_role
                == Some(StringFoldRole::ProducerFromInt)
                && args.len() == 1 =>
            {
                Some((*dst, Producer::FromInt { src: args[0] }))
            }
            RegInstr::CallIntrinsic {
                dst,
                intrinsic,
                args,
            }
            | RegInstr::CallTypedIntrinsic {
                dst,
                intrinsic,
                args,
                ..
            } if intrinsic_descriptor(*intrinsic).string_fold_role
                == Some(StringFoldRole::ProducerSlice) =>
            {
                slice_args(args)
                    .map(|(src, start, len)| (*dst, Producer::Slice { src, start, len }))
            }
            // A `Move` whose src is a (candidate) string is a potential alias; we
            // only mark it a producer if the src is itself a string producer (below,
            // after all defs are seen). Record it provisionally as an Alias.
            RegInstr::Move { dst, src } => Some((*dst, Producer::Alias { src: *src })),
            _ => None,
        };
        if let Some((dst, prod)) = dst_prod {
            if dst >= n_regs {
                return None;
            }
            if analysis.region_def_count(dst)? > 1 {
                multiply_defined[dst] = true;
            }
            producer[dst] = Some(prod);
        }
    }

    // A register defined out-of-region, or defined more than once in-region, cannot
    // be a sound single-producer string ⇒ drop it from the candidate set. (Out-of-
    // region defs would change the value the loop observes.)
    analysis.mark_external_writes(code, &mut multiply_defined)?;
    for r in parallel_indices(0..n_regs) {
        if multiply_defined[r] {
            producer[r] = None;
        }
    }

    // Resolve foldability with a fixpoint: a producer is FOLDABLE iff all the
    // operands its length law needs are themselves foldable (or, for slice/from_int,
    // satisfy the ASCII / integer requirements). `Move`/`Concat`/`Slice` of a non-
    // foldable string is itself non-foldable. `ascii` is tracked alongside.
    let mut foldable = vec![false; n_regs];
    let mut ascii = vec![false; n_regs];
    let mut changed = true;
    while changed {
        changed = false;
        for r in parallel_indices(0..n_regs) {
            if foldable[r] {
                continue;
            }
            let Some(prod) = &producer[r] else { continue };
            let (ok, is_ascii) = match prod {
                Producer::Literal { ascii, .. } => (true, *ascii),
                // `from_int` is always ASCII; its operand is an Int (not a string),
                // so no string dependency.
                Producer::FromInt { .. } => (true, true),
                Producer::Concat { left, right } => {
                    let ok =
                        *left < n_regs && *right < n_regs && foldable[*left] && foldable[*right];
                    (ok, ok && ascii[*left] && ascii[*right])
                }
                Producer::Slice { src, .. } => {
                    // The slice length law needs the SOURCE to be provably ASCII
                    // (so the char-boundary clamp is the identity). A slice of an
                    // ASCII string is itself ASCII.
                    let ok = *src < n_regs && foldable[*src] && ascii[*src];
                    (ok, ok)
                }
                Producer::Alias { src } => {
                    let ok = *src < n_regs && foldable[*src];
                    (ok, ok && ascii[*src])
                }
            };
            if ok && !foldable[r] {
                foldable[r] = true;
                ascii[r] = is_ascii;
                changed = true;
            }
        }
    }

    // STRING = registers we (provisionally) treat as string-valued and intend to
    // dissolve: every foldable producer register. For soundness we now require that
    // EVERY use of a STRING register is itself foldable — i.e. an operand of another
    // foldable producer, a `Move` to a foldable register, or a `String.len`. Any
    // other in-region use, or ANY out-of-region use, ESCAPES ⇒ that register cannot
    // be dissolved. We don't need partial dissolution: if a `String.len` source is
    // foldable but the foldable register also escapes elsewhere, the producer must
    // stay live, so we cannot delete it — bail that whole register out of `foldable`
    // and re-resolve, then finally require every in-region `String.len` to be
    // foldable (else bail the pass: a live `StringLen` is not native-subset).
    //
    // Compute "escapes": a foldable register read by an instruction that is neither
    // (a) a foldable producer consuming it as a string operand, nor (b) a
    // `String.len`. Iterate to a fixpoint (dropping an escaping register can make a
    // consumer's operand non-foldable, propagating).
    loop {
        let mut escaped = vec![false; n_regs];
        // Out-of-region reads of any foldable register ⇒ escape.
        analysis.mark_external_reads_touching(code, &foldable, &mut escaped)?;
        // In-region uses: each read of a foldable register must be a sanctioned
        // string consumer.
        for i in parallel_indices(header..exit) {
            match &code[i] {
                // Sanctioned: foldable producers consuming foldable string operands.
                RegInstr::StringConcat { dst, left, right } if foldable[*dst] => {
                    // operands consumed as strings — fine (handled by being foldable)
                    let _ = (left, right);
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic,
                    args,
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic,
                    args,
                    ..
                } if foldable[*dst]
                    && args.len() == 3
                    && intrinsic_descriptor(*intrinsic).string_fold_role
                        == Some(StringFoldRole::ProducerSlice) =>
                {
                    // start/len (args[1], args[2]) are Int operands, not strings.
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {
                    // alias of a foldable string into a foldable register — fine.
                }
                // Sanctioned: the query itself.
                _ if is_string_len(&code[i]) => {
                    // its single arg is consumed as a string — fine.
                }
                // Any other instruction: any foldable register it reads escapes.
                other => match instr_read_regs(other) {
                    RegFootprint::Some(rs) => {
                        for r in rs {
                            if r < n_regs && foldable[r] {
                                escaped[r] = true;
                            }
                        }
                    }
                    RegFootprint::All => return None,
                },
            }
        }
        if !escaped.iter().any(|&e| e) {
            break;
        }
        // Drop escaped registers and re-resolve foldability (a dropped operand can
        // un-fold its consumers).
        for r in parallel_indices(0..n_regs) {
            if escaped[r] {
                foldable[r] = false;
                ascii[r] = false;
            }
        }
        let mut changed2 = true;
        while changed2 {
            changed2 = false;
            for r in parallel_indices(0..n_regs) {
                if !foldable[r] {
                    continue;
                }
                let Some(prod) = &producer[r] else { continue };
                let still = match prod {
                    Producer::Literal { .. } | Producer::FromInt { .. } => true,
                    Producer::Concat { left, right } => foldable[*left] && foldable[*right],
                    Producer::Slice { src, .. } => foldable[*src] && ascii[*src],
                    Producer::Alias { src } => foldable[*src],
                };
                if !still {
                    foldable[r] = false;
                    ascii[r] = false;
                    changed2 = true;
                }
            }
        }
    }

    // A `String.len` whose source is NOT foldable is left UNCHANGED: the real
    // `StringLen` intrinsic IS in the native subset (it lowers to the host `string_len`
    // helper), so a surviving `StringLen` does NOT block OSR — only foldable-source
    // lengths are dissolved below (the rewrite arm guards on `foldable[args[0]]`). This
    // is what lets an EXPANDED-path loop (e.g. a two-armed `Result<String,_>` whose arms
    // call `String.len` on a live heap payload) OSR instead of declining the whole loop.

    // Nothing dissolvable after escape analysis ⇒ identity (no fold): every in-region
    // `String.len` had a non-foldable source (all left in place), or there was no
    // foldable producer at all.
    if !foldable.iter().any(|&f| f) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }

    // Allocate a fresh Int `len_reg` per foldable string register: it will hold that
    // string's byte length, computed at the producer site. `from_int` needs scratch
    // registers for its digit-count ladder; reserve them lazily.
    let mut len_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for r in parallel_indices(0..n_regs) {
        if foldable[r] {
            len_reg[r] = next_reg;
            next_reg += 1;
        }
    }

    // Emit the byte-length computation for `from_int(src)` into `out`, appending to
    // `out_code` (using absolute new-code indices for intra-fragment forward jumps,
    // back-patched after emission). Allocates scratch registers from `next_reg`.
    //
    // Matches `i64::to_string().len()`: `1` for `0`; `digits(k)` for `k > 0`;
    // `1 + digits(|k|)` for `k < 0`. Avoids negating `i64::MIN` by comparing `k`
    // directly against negative powers of ten.
    fn emit_from_int_len(out_code: &mut Vec<RegInstr>, out: usize, k: usize, next_reg: &mut usize) {
        // result accumulator is `out`. Strategy:
        //   if k >= 0:  out = pos_digits(k)
        //   else:       out = 1 + neg_digits(k)
        // pos_digits(k): 1 + count of thresholds {10,100,...,1e18} that k >= t.
        // neg_digits(k): 1 + count of thresholds {-10,...,-1e18} that k <= t.
        // Both are computed branchlessly-by-cascade using comparisons that write a
        // Bool then a conditional add — but Bool isn't Int-addable, so we instead use
        // a forward branch ladder: for k >= 0, test largest threshold first and on
        // the first hit LoadInt the digit count and Jump to the merge.
        let zero = *next_reg;
        *next_reg += 1;
        let thr = *next_reg;
        *next_reg += 1;
        // Positive powers of ten (10^1 .. 10^18); 10^19 overflows i64, so 19-digit
        // positives (>= 10^18) are the final else of the positive ladder.
        const POW10: [i64; 18] = [
            10,
            100,
            1_000,
            10_000,
            100_000,
            1_000_000,
            10_000_000,
            100_000_000,
            1_000_000_000,
            10_000_000_000,
            100_000_000_000,
            1_000_000_000_000,
            10_000_000_000_000,
            100_000_000_000_000,
            1_000_000_000_000_000,
            10_000_000_000_000_000,
            100_000_000_000_000_000,
            1_000_000_000_000_000_000,
        ];
        out_code.push(RegInstr::LoadInt {
            dst: zero,
            value: 0,
        });
        // Branch: if k < 0 jump to neg-ladder.
        let neg_start_patch = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: k,
            rhs: zero,
            op: RegIntCompare::Less,
            expected: true,
            target: 0, // back-patched
        });
        // --- positive (and zero) ladder: emit largest threshold first ---
        // For d in 19..=2: if k >= 10^(d-1) -> out = d; Jump merge.
        let mut to_merge: Vec<usize> = Vec::new();
        for d in parallel_indices((2..=19usize).rev()) {
            let t = POW10[d - 2];
            out_code.push(RegInstr::LoadInt { dst: thr, value: t });
            // if k >= t -> set out=d, jump merge
            let skip_patch = out_code.len();
            out_code.push(RegInstr::JumpIfIntCompare {
                lhs: k,
                rhs: thr,
                op: RegIntCompare::GreaterEqual,
                expected: false, // if NOT (k>=t) skip the assignment
                target: 0,       // back-patched to the next threshold test
            });
            out_code.push(RegInstr::LoadInt {
                dst: out,
                value: d as i64,
            });
            to_merge.push(out_code.len());
            out_code.push(RegInstr::Jump { target: 0 }); // -> merge
            // back-patch skip to here (next threshold test / final else)
            let here = out_code.len();
            if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[skip_patch] {
                *target = here;
            }
        }
        // positive final else: k in [0,10) -> 1 digit.
        out_code.push(RegInstr::LoadInt { dst: out, value: 1 });
        to_merge.push(out_code.len());
        out_code.push(RegInstr::Jump { target: 0 }); // -> merge
        // --- negative ladder ---
        let neg_start = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[neg_start_patch] {
            *target = neg_start;
        }
        // out (magnitude digits) then +1 for sign. For d in 19..=2: if k <= -10^(d-1)
        // -> magnitude d. Final else -> magnitude 1.
        let mut neg_to_add: Vec<usize> = Vec::new();
        for d in parallel_indices((2..=19usize).rev()) {
            let t = -POW10[d - 2];
            out_code.push(RegInstr::LoadInt { dst: thr, value: t });
            let skip_patch = out_code.len();
            out_code.push(RegInstr::JumpIfIntCompare {
                lhs: k,
                rhs: thr,
                op: RegIntCompare::LessEqual,
                expected: false, // if NOT (k<=t) skip
                target: 0,
            });
            out_code.push(RegInstr::LoadInt {
                dst: out,
                value: d as i64,
            });
            neg_to_add.push(out_code.len());
            out_code.push(RegInstr::Jump { target: 0 }); // -> add-sign
            let here = out_code.len();
            if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[skip_patch] {
                *target = here;
            }
        }
        out_code.push(RegInstr::LoadInt { dst: out, value: 1 });
        // fallthrough to add-sign
        let add_sign = out_code.len();
        for p in neg_to_add {
            if let RegInstr::Jump { target } = &mut out_code[p] {
                *target = add_sign;
            }
        }
        // out = out + 1 (sign byte). Reuse `thr` as the constant 1.
        out_code.push(RegInstr::LoadInt { dst: thr, value: 1 });
        out_code.push(RegInstr::AddInt {
            dst: out,
            lhs: out,
            rhs: thr,
        });
        // fallthrough to merge
        let merge = out_code.len();
        for p in to_merge {
            if let RegInstr::Jump { target } = &mut out_code[p] {
                *target = merge;
            }
        }
    }

    // Emit the slice byte-length law for an ASCII source into `out_code`, writing
    // `out`. `l_src` is the source's length register; `start`,`len` the Int operands.
    //   sc = max(start,0); s_clamp = min(sc, L); ec = s_clamp + max(len,0);
    //   e_clamp = min(ec, L); out = e_clamp - s_clamp.
    fn emit_slice_len(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        l_src: usize,
        start: usize,
        len: usize,
        next_reg: &mut usize,
    ) {
        let zero = *next_reg;
        let sc = *next_reg + 1;
        let sclamp = *next_reg + 2;
        let lc = *next_reg + 3;
        let ec = *next_reg + 4;
        *next_reg += 5;
        out_code.push(RegInstr::LoadInt {
            dst: zero,
            value: 0,
        });
        // sc = max(start, 0)
        emit_max(out_code, sc, start, zero, next_reg);
        // s_clamp = min(sc, L)
        emit_min(out_code, sclamp, sc, l_src, next_reg);
        // lc = max(len, 0)
        emit_max(out_code, lc, len, zero, next_reg);
        // ec = s_clamp + lc
        out_code.push(RegInstr::AddInt {
            dst: ec,
            lhs: sclamp,
            rhs: lc,
        });
        // e_clamp = min(ec, L)  -> reuse `ec`
        emit_min(out_code, ec, ec, l_src, next_reg);
        // out = e_clamp - s_clamp
        out_code.push(RegInstr::SubInt {
            dst: out,
            lhs: ec,
            rhs: sclamp,
        });
    }

    // out = max(a, b): if a >= b -> out=a else out=b (forward branch).
    fn emit_max(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        a: usize,
        b: usize,
        _next_reg: &mut usize,
    ) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::GreaterEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }

    // out = min(a, b): if a <= b -> out=a else out=b.
    // The emitter updates an origin entry for each synthesized instruction.
    fn emit_min(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        a: usize,
        b: usize,
        _next_reg: &mut usize,
    ) {
        let take_b = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: a,
            rhs: b,
            op: RegIntCompare::LessEqual,
            expected: false,
            target: 0,
        });
        out_code.push(RegInstr::Move { dst: out, src: a });
        let jmp = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let here = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[take_b] {
            *target = here;
        }
        out_code.push(RegInstr::Move { dst: out, src: b });
        let merge = out_code.len();
        if let RegInstr::Jump { target } = &mut out_code[jmp] {
            *target = merge;
        }
    }

    // Rewrite the whole stream. In-region foldable producers are replaced by the
    // length computation writing `len_reg[dst]` (the original heap allocation is
    // DELETED); each `String.len(s)` becomes `Move(dst, len_reg[s])`. Everything
    // else is copied through, remapping inter-instruction jump/match targets through
    // `index_map`. Intra-fragment jumps emitted by the helpers above already carry
    // absolute new-code positions (back-patched at emit time) and must NOT be
    // remapped, so producer fragments are spliced AFTER recording `index_map[i]` and
    // the fragment's internal jumps are left untouched.
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
        MapGet { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        // A foldable producer in-region: emit its length computation, drop the alloc.
        if region {
            let folded = match instr {
                RegInstr::LoadString { dst, .. } if foldable[*dst] => {
                    if let Some(Producer::Literal { len, .. }) = &producer[*dst] {
                        new_code.push(RegInstr::LoadInt {
                            dst: len_reg[*dst],
                            value: *len,
                        });
                    }
                    true
                }
                RegInstr::StringConcat { dst, .. } if foldable[*dst] => {
                    if let Some(Producer::Concat { left, right }) = &producer[*dst] {
                        new_code.push(RegInstr::AddInt {
                            dst: len_reg[*dst],
                            lhs: len_reg[*left],
                            rhs: len_reg[*right],
                        });
                    }
                    true
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {
                    new_code.push(RegInstr::Move {
                        dst: len_reg[*dst],
                        src: len_reg[*src],
                    });
                    true
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::StringFromInt,
                    ..
                } if foldable[*dst] => {
                    if let Some(Producer::FromInt { src }) = &producer[*dst] {
                        emit_from_int_len(&mut new_code, len_reg[*dst], *src, &mut next_reg);
                    }
                    true
                }
                RegInstr::CallIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::StringSlice,
                    ..
                }
                | RegInstr::CallTypedIntrinsic {
                    dst,
                    intrinsic: RegIntrinsic::StringSlice,
                    ..
                } if foldable[*dst] => {
                    if let Some(Producer::Slice { src, start, len }) = &producer[*dst] {
                        emit_slice_len(
                            &mut new_code,
                            len_reg[*dst],
                            len_reg[*src],
                            *start,
                            *len,
                            &mut next_reg,
                        );
                    }
                    true
                }
                _ if is_string_len(instr)
                    && matches!(instr,
                        RegInstr::CallIntrinsic { args, .. }
                        | RegInstr::CallTypedIntrinsic { args, .. }
                        if args[0] < n_regs && foldable[args[0]]) =>
                {
                    let (dst, src) = match instr {
                        RegInstr::CallIntrinsic { dst, args, .. }
                        | RegInstr::CallTypedIntrinsic { dst, args, .. } => (*dst, args[0]),
                        _ => unreachable!(),
                    };
                    new_code.push(RegInstr::Move {
                        dst,
                        src: len_reg[src],
                    });
                    true
                }
                _ => false,
            };
            if folded {
                continue;
            }
        }
        // Copy-through, remapping jump/match targets to the new index space.
        match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::MapGet {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = sa;
                        *err_ip = sb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = sa;
                        *else_ip = sb;
                    }
                    _ => {}
                }
            }
            Fix::MapGet { a, b } => {
                let (sa, sb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = sa;
                        *none_ip = sb;
                    }
                    _ => {}
                }
            }
        }
    }
    // Inverse ip-map: every fragment instruction maps back to the producer's
    // original index (`String.len` → its own index; copy-through 1:1).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in parallel_indices(0..code.len()) {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in parallel_indices(start..end) {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map))
}

/// OSR × scalar replacement: scalar-replace non-escaping scalar `Option`s that live entirely
/// inside the loop region `[header, exit)` of an otherwise native-INELIGIBLE
/// function (one whose pre/post-loop code does I/O — calls, `Output.write`, …, which
/// the whole-function [`native_scalar_replace_options`] would reject).
///
/// Soundness model (region-scoped, conservative): an `Option` register is
/// scalar-replaced only if EVERY one of its definitions and uses lies strictly
/// inside `[header, exit)`, every in-region instruction is native-subset, one of
/// the four Option ops, or a `TryResult` consuming a scalar-replaced Option, and
/// the register never appears outside the region (so it is dead at both OSR
/// boundaries and the non-subset I/O outside never touches it). Anything we cannot
/// prove ⇒ `None` (no OSR; the interpreter runs the loop). The loop-carried
/// (boundary) registers keep their original indices; only fresh tag/payload regs
/// (>= `n_regs`) are added, and they are loop-internal.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` where
/// `ip_map[transformed_ip] = original_ip` (each rewritten Option op's fragments map
/// to that op's original index; copy-through maps one-to-one). Out-of-region
/// instructions are copied through verbatim, so the I/O before/after the loop is
/// preserved exactly.
#[cfg(feature = "native-jit")]
// The region rewrite coordinates code, recipes, and source maps by IP.
pub(in crate::reg_vm) fn native_scalar_replace_options_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<RegionRewrite<OsrMaterializeRecipe>> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no Option op inside the region ⇒ identity transform (plain OSR).
    if !(header..exit).any(|i| is_option_op(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map, Vec::new()));
    }

    // Every in-region instruction must be native-subset or one of the four Option
    // ops; otherwise the loop body cannot become a native loop anyway — bail.
    for i in parallel_indices(header..exit) {
        if !native_subset_instruction(&code[i])
            && !is_option_op(&code[i])
            && !matches!(&code[i], RegInstr::TryResult { .. })
        {
            return None;
        }
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // OPT = registers carrying an Option value: seed from in-region
    // `MakeSome`/`LoadNone` dsts, close under in-region `Move` aliasing.
    let mut opt = vec![false; n_regs];
    for i in parallel_indices(header..exit) {
        match &code[i] {
            RegInstr::MakeSome { dst, .. }
            | RegInstr::LoadNone { dst }
            | RegInstr::DequePopFront { dst, .. }
            | RegInstr::DequePopBack { dst, .. } => opt[*dst] = true,
            _ => {}
        }
    }
    analysis.close_region_move_aliases(code, &mut opt)?;

    // Validate in-region uses/defs of OPT registers (identical recognition rules to
    // the whole-function pass), and require a SCALAR payload.
    for i in parallel_indices(header..exit) {
        match &code[i] {
            RegInstr::LoadNone { dst } if opt[*dst] => {}
            RegInstr::MakeSome { dst, value } if opt[*dst] => {
                if opt[*value] {
                    return None; // Option payload ⇒ non-scalar
                }
            }
            RegInstr::DequePopFront { dst, .. } | RegInstr::DequePopBack { dst, .. }
                if opt[*dst] => {}
            RegInstr::Move { dst, src } if opt[*dst] => {
                if !opt[*src] {
                    return None;
                }
            }
            RegInstr::MatchOption { src, .. } if opt[*src] => {}
            RegInstr::UnwrapSome { dst, src } if opt[*src] => {
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::TryResult { dst, src, .. } if opt[*src] => {
                if opt[*dst] {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if opt[*src] => {}
            other => {
                let reads = subset_or_option_reads(other)?;
                if reads.into_iter().any(|r| opt[r]) {
                    return None;
                }
                if let RegInstr::UnwrapSome { dst, .. }
                | RegInstr::MakeSome { dst, .. }
                | RegInstr::LoadNone { dst }
                | RegInstr::DequePopFront { dst, .. }
                | RegInstr::DequePopBack { dst, .. } = other
                    && opt[*dst]
                {
                    return None;
                }
            }
        }
    }

    // CRITICAL boundary soundness. After scalar replacement the ORIGINAL Option
    // register `o` is NEVER written inside the transformed region (its defs became
    // tag/payload writes), so the interpreter slot for `o` is stale after a clean OSR
    // exit. That is still sound when:
    //
    //   1. every in-region read of an OPT register is definitely assigned by an
    //      in-region Option def before the read, so native never needs a live-in
    //      interpreter Option value, and
    //   2. no out-of-region instruction reads an OPT register after native exits.
    //
    // Out-of-region writes are harmless under those two facts: pre-loop writes are
    // overwritten before every in-loop read, and post-loop writes overwrite the stale
    // interpreter slot before it can be observed. This matters for OSR × inlining,
    // where a call result register is commonly reused outside the loop even though
    // the Option value produced by the inlined callee is loop-local.
    if !option_regs_definitely_assigned_before_region_reads(code, n_regs, header, exit, &opt) {
        return None;
    }

    // heap-aware deopt(b) live-after always-`Some` Option reconstruction (the Option analog of the
    // Result pass). Originally ANY out-of-region read of an OPT register bailed. We now
    // allow a read AFTER the region by reconstructing `Some(payload)` at OSR-exit from
    // its scalar payload register, with the same soundness obligations as the Result
    // pass plus the always-`Some` requirement:
    //   * no OPT register written at ip >= exit (pre-loop init, ip < header, is fine);
    //   * a read BEFORE the region (live-in Option) is out of scope;
    //   * the OPT register is always-`Some` (no in-region `LoadNone` def) — a `None`
    //     outcome has no scalar payload to reconstruct and would make the payload only
    //     maybe-assigned; and
    //   * a single in-region `MakeSome` def reached UNCONDITIONALLY each iteration (so
    //     the payload is definitely-assigned after >=1 iteration).
    // (Conservative register footprints still hold: an unanalyzable instruction reports
    // `RegFootprint::All`, which bails. The scalar-payload-TYPE check is deferred to the
    // OsrEntry build site.)
    let mut reconstruct = vec![false; n_regs];
    for i in parallel_indices(0..code.len()) {
        if i >= header && i < exit {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && opt[r]) {
                    return None;
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && opt[r] {
                        if i < header {
                            return None; // live-in Option (read before the loop)
                        }
                        reconstruct[r] = true;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }
    for (reg, &needs) in reconstruct.iter().enumerate() {
        if !needs {
            continue;
        }
        // Always-`Some`: no in-region `LoadNone` def for this register.
        if (header..exit).any(|i| matches!(&code[i], RegInstr::LoadNone { dst } if *dst == reg)) {
            return None;
        }
        let in_region_defs: Vec<usize> = analysis
            .writer_ips_of(code, reg)?
            .into_iter()
            .filter(|&i| i >= header && i < exit)
            .collect();
        if in_region_defs.len() != 1 {
            return None;
        }
        let def_ip = in_region_defs[0];
        for i in parallel_indices(header..def_ip) {
            match &code[i] {
                RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. }
                    if *target >= exit => {}
                RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. } => return None,
                _ => {}
            }
        }
    }

    // Allocate fresh tag/payload regs per OPT register.
    let mut tag_reg = vec![0usize; n_regs];
    let mut payload_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for (reg, is_opt) in opt.iter().enumerate() {
        if *is_opt {
            tag_reg[reg] = next_reg;
            payload_reg[reg] = next_reg + 1;
            next_reg += 2;
        }
    }

    // heap-aware deopt(b) Some-Option reconstruction recipes.
    let option_recipes: Vec<OsrMaterializeRecipe> = reconstruct
        .iter()
        .enumerate()
        .filter(|&(_, &needs)| needs)
        .map(|(reg, _)| OsrMaterializeRecipe {
            dst_reg: reg,
            value: OsrMaterializeValue::OptionSome(Box::new(OsrMaterializeValue::Register(
                payload_reg[reg],
            ))),
        })
        .collect();

    // Rewrite the WHOLE code, scalar-replacing in-region Option ops and copying
    // everything else through verbatim; remap all jump/match targets through the
    // index map. (Out-of-region jumps keep pointing at the right place after the
    // region's instructions expand.)
    enum Fix {
        Target(usize),
        Match { some_ip: usize, none_ip: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeSome { dst, value } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: *value,
                });
            }
            RegInstr::LoadNone { dst } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: false,
                });
            }
            RegInstr::DequePopFront { dst, deque } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::DequePopFront {
                    dst: payload_reg[*dst],
                    deque: *deque,
                });
            }
            RegInstr::DequePopBack { dst, deque } if region && opt[*dst] => {
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: true,
                });
                new_code.push(RegInstr::DequePopBack {
                    dst: payload_reg[*dst],
                    deque: *deque,
                });
            }
            RegInstr::Move { dst, src } if region && opt[*dst] => {
                new_code.push(RegInstr::Move {
                    dst: tag_reg[*dst],
                    src: tag_reg[*src],
                });
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: payload_reg[*src],
                });
            }
            RegInstr::MatchOption {
                src,
                some_ip,
                none_ip,
            } if region && opt[*src] => {
                fixups.push((new_code.len(), Fix::Target(*some_ip)));
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: 0,
                });
                fixups.push((new_code.len(), Fix::Target(*none_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::UnwrapSome { dst, src } if region && opt[*src] => {
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            RegInstr::TryResult { dst, src, .. } if region && opt[*src] => {
                let some_target = new_code.len() + 2;
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: some_target,
                });
                new_code.push(RegInstr::RuntimeError {
                    message: String::new(),
                });
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            // Copy-through, remapping jump targets (covers both in-region native
            // branches and the pre/post-loop control flow). `MatchOption` outside the
            // region (or on a non-OPT src) is copied with BOTH targets remapped.
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        some_ip: *some_ip,
                        none_ip: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { some_ip, none_ip } => {
                let (s, n) = (index_map[some_ip], index_map[none_ip]);
                if let RegInstr::MatchOption {
                    some_ip: sd,
                    none_ip: nd,
                    ..
                } = &mut new_code[pos]
                {
                    *sd = s;
                    *nd = n;
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in parallel_indices(0..code.len()) {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in parallel_indices(start..end) {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, option_recipes))
}

/// Whether a `MakeVariant` layout name is a `Result` constructor (`Ok`/`Err`). These
/// are reserved by the language for `Result`, are matched by the dedicated
/// `MatchResult` op (not `MatchVariant`), and are dissolved by the Result region pass
/// — never by the user-variant pass.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn is_result_ctor_name(name: &str) -> bool {
    name == "Ok" || name == "Err"
}

/// OSR × scalar replacement for RESULTS (deopt-before-heap, Slice 1): scalar-replace a non-escaping
/// `Result<Scalar, _>` that is **statically always-`Ok`** on the native path and
/// lives entirely inside the loop region `[header, exit)` of an otherwise native-
/// INELIGIBLE function. Mirrors [`native_scalar_replace_options_in_region`] but for
/// the `Result` shape (`MakeVariant{Ok|Err}` + `MatchResult` +
/// `UnwrapVariantValue`/`TryResult`).
///
/// KEY (the deopt-before-heap interplay): when a leaf's `Err` arm built a heap value,
/// [`native_inline_leaf_calls`] already replaced that arm with a native `Bail`, so the
/// inlined stream contains NO reachable `MakeVariant{Err}` — the only constructor of
/// the Result register is `MakeVariant{Ok, [scalar]}`. The Result is therefore
/// statically always-`Ok`, and this pass dissolves it to a single scalar **payload**
/// register (no tag needed): every `MatchResult{src:R}` becomes an unconditional
/// `Jump → ok_ip` (the `Err` arm goes dead) and every `UnwrapVariantValue{src:R,
/// expected:"Ok"}` becomes a `Move` from the payload. A LIVE heap `Err` (a reachable
/// `MakeVariant{Err}` def of `R`) ⇒ BAIL the pass (leave the loop on the interpreter):
/// such a Result is genuinely two-armed with a heap payload and cannot be scalarized.
///
/// Soundness: identical region discipline to the sibling passes. `R` is replaced only
/// if every def/use lies strictly inside `[header, exit)`, the `Ok` payload is scalar,
/// and `R` is dead at both OSR boundaries (`instr_read_regs`/`instr_written_reg`, with
/// `RegFootprint::All ⇒ bail`). The always-`Ok` rewrite is sound because the only way
/// the program reaches the dissolved `MatchResult` is by having built an `Ok` (the
/// `Err` constructor bailed to the interpreter before any heap op — the transactional fallback contract), so
/// the `Err` arm is dynamically unreachable on the native path; rewriting it to a
/// statically-dead `Jump`/`Bail` cannot change observable behavior.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→original
/// `ip_map` discipline as the other region passes.
///
/// A live-after Result reconstruction recipe:
/// `(variant_reg, ok_payload_reg, err_payload_reg, tag_reg)`. `tag_reg` is `None` for an
/// always-`Ok` Result (reconstruct `Ok(ok_payload)`; `err_payload` is unused, set equal
/// to `ok_payload`) and `Some(tag)` for a two-armed Result (reconstruct
/// `Ok(ok_payload)` if the tag's live value is non-zero, else `Err(err_payload)`).
/// PER-ARM payloads: the `Ok` and `Err` arms keep SEPARATE payload registers so arms
/// of different native types (e.g. `Result<Int, String>`) don't force a single payload
/// register into conflicting types. Only the arm matching the live `tag` is read at
/// reconstruction, so the other (possibly stale) payload is never observed.
#[cfg(feature = "native-jit")]
// The region rewrite coordinates code, recipes, and source maps by IP.
pub(in crate::reg_vm) fn native_scalar_replace_results_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<RegionRewrite<ResultRecipe>> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no `MatchResult` and no `Result` constructor in the region ⇒ nothing
    // for THIS pass to do (identity transform; preserves the byte-for-byte old path).
    let has_result_op = (header..exit).any(|i| {
        matches!(&code[i], RegInstr::MatchResult { .. })
            || matches!(&code[i], RegInstr::MakeVariant { layout, .. } if is_result_ctor_name(&layout.name))
    });
    if !has_result_op {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map, Vec::new()));
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // RES = registers carrying a (replaceable) Result value: seed from in-region
    // `MakeVariant{Ok|Err}` dsts, close under in-region `Move` aliasing.
    let mut res = vec![false; n_regs];
    for i in parallel_indices(header..exit) {
        if let RegInstr::MakeVariant { dst, layout, .. } = &code[i]
            && is_result_ctor_name(&layout.name)
        {
            res[*dst] = true;
        }
    }
    analysis.close_region_move_aliases(code, &mut res)?;
    // A `MatchResult{src}` whose `src` is not (yet) a RES register means a Result we
    // cannot see being constructed in-region (it flows in from outside, e.g. a heap
    // Result param) ⇒ this pass cannot dissolve it. Bail so the loop stays on the
    // interpreter (the boundary/escape gates below would also catch it, but bailing
    // early is clearer and conservative).
    for i in parallel_indices(header..exit) {
        if let RegInstr::MatchResult { src, .. } = &code[i]
            && !res[*src]
        {
            return None;
        }
    }

    // Two-armed (heap-aware deopt #7 follow-up): a reachable `MakeVariant{Err}` on a RES register
    // means the Result is NOT statically always-`Ok`. Instead of bailing, dissolve it
    // with an explicit `Ok`/`Err` tag + a shared scalar payload register (the tag routes
    // the `MatchResult` and selects which arm's `UnwrapVariantValue` reads the payload).
    // Scoped to dead-at-boundary: a two-armed RES that is live-after, or short-circuited
    // by `?` (`TryResult`), bails (live-after Ok/Err reconstruction stays future).
    let two_armed = (header..exit).any(|i| {
        matches!(&code[i],
            RegInstr::MakeVariant { dst, layout, .. }
                if res[*dst] && layout.name.as_ref() == "Err")
    });
    if two_armed {
        return native_scalar_replace_two_armed_results_in_region(code, n_regs, header, exit, &res);
    }

    // Validate every in-region def/use of a RES register. The Result must be
    // statically always-`Ok`: a reachable `MakeVariant{Err}` def is a LIVE heap Err
    // ⇒ bail. `Ok` payload must be scalar (not itself a RES register). Recognized uses:
    // `MatchResult{src:R}`, `UnwrapVariantValue{src:R}` (Ok scalar payload, or the dead
    // Err-arm unwrap which the rewrite drops), `TryResult{src:R}` (the `?` success
    // projection), and `Move` aliases. Anything else that touches a RES register ⇒ bail.
    for i in parallel_indices(header..exit) {
        match &code[i] {
            RegInstr::MakeVariant {
                dst,
                layout,
                fields,
            } if res[*dst] => {
                if layout.name.as_ref() == "Err" {
                    return None; // live heap Err ⇒ not always-Ok ⇒ leave on interpreter
                }
                // Ok constructor: exactly one scalar field `value`.
                if fields.len() != 1 || fields.iter().any(|(_, r)| res[*r]) {
                    return None;
                }
            }
            RegInstr::Move { dst, src } if res[*dst] => {
                if !res[*src] {
                    return None;
                }
            }
            RegInstr::MatchResult { src, .. } if res[*src] => {}
            RegInstr::TryResult { dst, src, .. } if res[*src] => {
                if res[*dst] {
                    return None;
                }
            }
            RegInstr::UnwrapVariantValue { dst, src, expected } if res[*src] => {
                // The Ok-arm unwrap yields the scalar payload; its `dst` must not be a
                // RES register (a Result payload would be non-scalar). The Err-arm
                // unwrap (`expected == "Err"`) lies on the statically-dead arm — its
                // `dst` is unused on the native path; allow it (rewritten to a Bail).
                let _ = expected;
                if res[*dst] {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if res[*src] => {}
            other => {
                // Any other instruction must not read a RES register, nor (re)define one
                // through an unrecognized destination.
                match instr_read_regs(other) {
                    RegFootprint::Some(reads) => {
                        if reads.into_iter().any(|r| r < n_regs && res[r]) {
                            return None;
                        }
                    }
                    RegFootprint::All => return None,
                }
                if let RegInstr::UnwrapVariantValue { dst, .. } | RegInstr::MakeVariant { dst, .. } =
                    other
                    && res[*dst]
                {
                    return None;
                }
            }
        }
    }

    // Boundary soundness + heap-aware deopt(b) live-after reconstruction. Originally every RES
    // register had to be DEAD outside `[header, exit)`. We now also allow a RES
    // register that is only READ after the region by reconstructing `Ok(payload)` at
    // OSR-exit from its scalar payload register, because the pass already proved every
    // RES register is always-`Ok` with a scalar `Ok` payload (a heap `Err` became a
    // native `Bail`), so a completed native loop guarantees the value is `Ok(payload)`.
    // Conditions to keep it sound:
    //   * No RES register may be WRITTEN at ip >= exit (post-loop reassignment is out
    //     of scope). A write BEFORE the region (pre-loop `let mut r = Ok(..)`) is fine:
    //     native never touches the original RES slot and reconstruction overwrites it
    //     at exit, or — after 0 native iterations — the pre-loop value already in the
    //     slot is exactly correct.
    //   * A RES register read BEFORE the region (a live-in Result) is out of scope.
    //   * Each live-after RES register needs a single in-region definition reached
    //     UNCONDITIONALLY each iteration (no branch between the header and the def
    //     except the header's own loop-exit condition), so its payload register is
    //     definitely-assigned after >=1 iteration (hence present in the OSR-exit deopt
    //     live set). A conditional/multiply-defined RES register would leave the
    //     payload only maybe-assigned ⇒ bail (conservative).
    // The scalar-payload-TYPE check is deferred to the OsrEntry build site (where
    // native register types are known); a non-scalar `Ok` payload declines OSR there.
    let mut reconstruct = vec![false; n_regs];
    for i in parallel_indices(0..code.len()) {
        if in_region(i) {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && res[r]) {
                    return None; // post-loop reassignment of a dissolved Result
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && res[r] {
                        if i < header {
                            return None; // live-in Result (read before the loop)
                        }
                        reconstruct[r] = true;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }
    // Require a single, unconditionally-reached in-region def for every RES register we
    // must reconstruct.
    for (reg, &needs) in reconstruct.iter().enumerate() {
        if !needs {
            continue;
        }
        let in_region_defs: Vec<usize> = analysis
            .writer_ips_of(code, reg)?
            .into_iter()
            .filter(|&i| in_region(i))
            .collect();
        if in_region_defs.len() != 1 {
            return None;
        }
        let def_ip = in_region_defs[0];
        for i in parallel_indices(header..def_ip) {
            match &code[i] {
                // The header's loop-exit condition (target outside the loop) is fine.
                RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. }
                    if *target >= exit => {}
                // Any other branch/match/return between the header and the def could
                // skip the def on some iteration ⇒ payload not definitely-assigned.
                RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. } => return None,
                _ => {}
            }
        }
    }

    // Allocate one fresh payload register per RES register (always-Ok ⇒ no tag).
    let mut payload_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for (reg, is_res) in res.iter().enumerate() {
        if *is_res {
            payload_reg[reg] = next_reg;
            next_reg += 1;
        }
    }

    // heap-aware deopt(b) recipes for each live-after RES register. Always-`Ok` recipes carry no
    // tag (`None`) ⇒ reconstruct `Ok(payload)`; this path has a single payload register,
    // so the (unused) err-payload slot mirrors it.
    let recipes: Vec<ResultRecipe> = reconstruct
        .iter()
        .enumerate()
        .filter(|&(_, &needs)| needs)
        .map(|(reg, _)| (reg, payload_reg[reg], payload_reg[reg], None))
        .collect();

    // Rewrite the WHOLE code, dissolving in-region Result ops and copying everything
    // else through verbatim; remap all jump/match targets through the index map.
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeVariant { dst, fields, .. } if region && res[*dst] => {
                // Always-Ok constructor: payload = the single scalar field.
                let (_, field_reg) = &fields[0];
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: *field_reg,
                });
            }
            RegInstr::Move { dst, src } if region && res[*dst] => {
                new_code.push(RegInstr::Move {
                    dst: payload_reg[*dst],
                    src: payload_reg[*src],
                });
            }
            RegInstr::MatchResult { src, ok_ip, err_ip } if region && res[*src] => {
                // Statically always-Ok ⇒ unconditional jump to the Ok arm. The Err arm
                // (`err_ip`) becomes unreachable.
                let _ = err_ip;
                fixups.push((new_code.len(), Fix::Target(*ok_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::TryResult { dst, src, .. } if region && res[*src] => {
                // Statically always-Ok `?` ⇒ payload projection. The short-circuit arm is
                // unreachable on the native path; any real Err was already replaced by a
                // Bail before constructing heap state, so the interpreter rerun performs
                // the normal cleanup/return behavior.
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: payload_reg[*src],
                });
            }
            RegInstr::UnwrapVariantValue { dst, src, expected } if region && res[*src] => {
                if expected.as_str() == "Err" {
                    // Dead Err-arm unwrap: unreachable after the always-Ok rewrite. Emit
                    // a Bail sentinel so that, even if some path reached it, native would
                    // safely deopt rather than read a non-existent heap Err payload.
                    new_code.push(RegInstr::RuntimeError {
                        message: String::new(),
                    });
                } else {
                    // Ok-arm unwrap ⇒ the scalar payload.
                    new_code.push(RegInstr::Move {
                        dst: *dst,
                        src: payload_reg[*src],
                    });
                }
            }
            // Copy-through, remapping jump targets (in-region native branches and the
            // pre/post-loop control flow). A `MatchResult` outside the region (or on a
            // non-RES src) is copied with BOTH targets remapped; same for the other
            // match ops the body may carry.
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => {
                fixups.push((new_code.len(), Fix::Target(*target)));
                new_code.push(instr.clone());
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *ok_ip,
                        b: *err_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
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
                    Fix::Match {
                        a: *some_ip,
                        b: *none_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::Match {
                        a: *match_ip,
                        b: *else_ip,
                    },
                ));
                new_code.push(instr.clone());
            }
            other => new_code.push(other.clone()),
        }
    }
    for (pos, fix) in fixups {
        match fix {
            Fix::Target(t) => {
                let target = index_map[t];
                match &mut new_code[pos] {
                    RegInstr::Jump { target: dst }
                    | RegInstr::JumpIfBool { target: dst, .. }
                    | RegInstr::JumpIfIntCompare { target: dst, .. } => *dst = target,
                    _ => {}
                }
            }
            Fix::Match { a, b } => {
                let (na, nb) = (index_map[a], index_map[b]);
                match &mut new_code[pos] {
                    RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                        *ok_ip = na;
                        *err_ip = nb;
                    }
                    RegInstr::MatchOption {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchMapGet {
                        some_ip, none_ip, ..
                    }
                    | RegInstr::MatchSortedMapGet {
                        some_ip, none_ip, ..
                    } => {
                        *some_ip = na;
                        *none_ip = nb;
                    }
                    RegInstr::MatchVariant {
                        match_ip, else_ip, ..
                    } => {
                        *match_ip = na;
                        *else_ip = nb;
                    }
                    _ => {}
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in parallel_indices(0..code.len()) {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in parallel_indices(start..end) {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, recipes))
}
