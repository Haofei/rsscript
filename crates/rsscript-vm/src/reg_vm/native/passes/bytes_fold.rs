//! Byte-string length folding for native regions.

use super::*;

/// OSR × scalar replacement for BYTES LENGTH-LAW FOLDING (the read-only sibling of
/// [`native_string_length_fold_in_region`]): dissolve a non-escaping Bytes value built
/// ONLY to be measured (`Bytes.len` of `Bytes.slice`/`Bytes.from_string`/`Move`/a
/// constant-length source) into byte-length arithmetic, DELETING the now-dead Bytes
/// allocation. Read-only (no heap write; the transactional fallback contract holds), turning a length-only
/// Bytes loop into pure-scalar Int code the native subset accepts.
///
/// Why a separate pass from the String fold: Bytes are RAW bytes with NO char/grapheme
/// boundary, so the slice length law is the EXACT clamp arithmetic of [`bytes_slice`]
/// with NO ASCII gate — verified identical: `bytes_slice` does `s'=max(start,0); if
/// s'>=L {0} else { min(s'+max(len,0), L) - s' }`, which is precisely the
/// overflow-free `emit_slice_len` law (`sc=max(start,0); if sc>=L {0} else
/// {min(max(len,0),L-sc)}`). `Bytes.len` is
/// `value.len()` (raw byte count) and `Bytes.from_string(s)` is `s.as_bytes().len()`,
/// so a from-string's byte length equals the source String's byte length.
///
/// A length source may be (a) an in-region foldable Bytes producer, (b) ANY register
/// whose byte length resolves to a COMPILE-TIME CONSTANT through its unique global def,
/// or (c) a dynamic Bytes input with no in-region definition. Constants are materialized
/// as in-region `LoadInt`s. Dynamic inputs retain a validating `Bytes.len` at each folded
/// slice site; whole-function loop memoization can make that helper activation-local
/// O(1) work. The constant trace requires every register in the chain to have exactly one
/// whole-function definition.
///
/// Conservative bail: any escaping foldable Bytes register or unmodelled read/write
/// footprint ⇒ `None` (no fold). Unrelated direct dynamic `Bytes.len` calls remain
/// ordinary helpers.
#[cfg(feature = "native-jit")]
// The Bytes rewrite coordinates code, origin maps, and fact tables by source IP.
pub(in crate::reg_vm) fn native_bytes_length_fold_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    let is_bytes_len = |instr: &RegInstr| {
        matches!(
            instr,
            RegInstr::CallIntrinsic { intrinsic, args, .. }
                | RegInstr::CallTypedIntrinsic { intrinsic, args, .. }
                if args.len() == 1
                    && intrinsic_descriptor(*intrinsic).bytes_fold_role
                        == Some(BytesFoldRole::LengthQuery)
        )
    };
    // No in-region Bytes.len ⇒ nothing to fold ⇒ identity (plain OSR, byte-for-byte).
    if !(header..exit).any(|i| is_bytes_len(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map));
    }
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;
    // Preserve this pass's old whole-function conservatism: an unknown write footprint
    // makes the unique-def tracer unverifiable, so the fold does not fire.
    analysis.global_def_counts.as_ref()?;

    // --- Constant byte-length tracer -------------------------------------------------
    // `const_len[r] = Some(L)` iff register `r` is defined EXACTLY ONCE in the whole
    // function by a chain of immutable constant ops whose byte length is the compile-
    // time constant `L`. Multiply-defined (or `All`-footprint) registers get `None`.
    //
    //   LoadString lit                 -> lit.as_bytes().len()      (== lit.len())
    //   Bytes.from_string(s)           -> const_len[s]
    //   Bytes.slice(s, start, len)     -> clamp(const_len[s], start, len)  [const args]
    //   Move(dst, src)                 -> const_len[src]
    //
    // `start`/`len` for a constant slice must themselves be constant Ints (LoadInt) for
    // the result to be a compile-time constant. Anything else ⇒ not a constant.
    // Single-def constant-Int values (for slice start/len constant args).
    let int_const = |r: usize| -> Option<i64> {
        let ip = analysis.single_def_ip_of(code, r)?;
        match &code[ip] {
            RegInstr::LoadInt { dst, value } if *dst == r => Some(*value),
            _ => None,
        }
    };
    // Resolve a register's constant byte length with a depth-bounded trace over its
    // unique def. Depth bound guards against any pathological chain; immutability is
    // guaranteed by `analysis.global_def_count(r) == 1` at each hop.
    // The analysis intentionally scans a bytecode interval by stable source IP.
    fn const_byte_len(
        r: usize,
        depth: usize,
        code: &[RegInstr],
        n_regs: usize,
        analysis: &NativeRegionAnalysis,
        int_const: &dyn Fn(usize) -> Option<i64>,
    ) -> Option<i64> {
        if depth == 0 || r >= n_regs || analysis.global_def_count(r)? != 1 {
            return None;
        }
        let def = &code[analysis.single_def_ip_of(code, r)?];
        match def {
            RegInstr::LoadString { value, .. } => Some(value.len() as i64),
            RegInstr::Move { src, .. } => {
                const_byte_len(*src, depth - 1, code, n_regs, analysis, int_const)
            }
            RegInstr::CallIntrinsic {
                intrinsic, args, ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic, args, ..
            } => {
                match intrinsic_descriptor(*intrinsic).bytes_fold_role {
                    Some(BytesFoldRole::ProducerFromString) if args.len() == 1 => {
                        const_byte_len(args[0], depth - 1, code, n_regs, analysis, int_const)
                    }
                    Some(BytesFoldRole::ProducerSlice) if args.len() == 3 => {
                        let l =
                            const_byte_len(args[0], depth - 1, code, n_regs, analysis, int_const)?;
                        let start = int_const(args[1])?;
                        let len = int_const(args[2])?;
                        // Mirror `bytes_slice` exactly on the constant operands.
                        let sc = start.max(0);
                        if sc >= l {
                            return Some(0);
                        }
                        let lc = len.max(0);
                        let end = sc.saturating_add(lc).min(l);
                        Some(end - sc)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }
    let mut const_len: Vec<Option<i64>> = vec![None; n_regs];
    for r in parallel_indices(0..n_regs) {
        const_len[r] = const_byte_len(r, 32, code, n_regs, &analysis, &int_const);
    }

    // --- In-region foldable Bytes producers ------------------------------------------
    #[derive(Clone)]
    enum BProducer {
        FromString {
            src: usize,
        },
        Slice {
            src: usize,
            start: usize,
            len: usize,
        },
        Alias {
            src: usize,
        },
    }
    let mut producer: Vec<Option<BProducer>> = vec![None; n_regs];
    let mut multiply_defined = vec![false; n_regs];
    for i in parallel_indices(header..exit) {
        let dst_prod: Option<(usize, BProducer)> = match &code[i] {
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
            } if intrinsic_descriptor(*intrinsic).bytes_fold_role
                == Some(BytesFoldRole::ProducerFromString)
                && args.len() == 1 =>
            {
                Some((*dst, BProducer::FromString { src: args[0] }))
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
            } if intrinsic_descriptor(*intrinsic).bytes_fold_role
                == Some(BytesFoldRole::ProducerSlice)
                && args.len() == 3 =>
            {
                Some((
                    *dst,
                    BProducer::Slice {
                        src: args[0],
                        start: args[1],
                        len: args[2],
                    },
                ))
            }
            RegInstr::Move { dst, src } => Some((*dst, BProducer::Alias { src: *src })),
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
    // A register defined out-of-region or multiply-defined in-region is not a sound
    // single-producer in-region Bytes value (its out-of-region def may differ). Drop
    // it — but it can still serve as a CONSTANT-length source via `const_len`.
    analysis.mark_external_writes(code, &mut multiply_defined)?;
    for r in parallel_indices(0..n_regs) {
        if multiply_defined[r] {
            producer[r] = None;
        }
    }

    // A loop-invariant dynamic Bytes input can also provide its length without
    // materializing a slice. Keep this deliberately narrow: only operands already
    // proven to be Bytes by `Bytes.len`/`Bytes.slice`, and only registers with no
    // in-region definition. The validating `Bytes.len` helper remains at the original
    // slice site, preserving the first possible failure point; the later native
    // memoization pass may cache it when the surrounding loop proves invariance.
    let mut dynamic_len_source = vec![false; n_regs];
    for instr in &code[header..exit] {
        let (role, args) = match instr {
            RegInstr::CallIntrinsic {
                intrinsic, args, ..
            }
            | RegInstr::CallTypedIntrinsic {
                intrinsic, args, ..
            } => (intrinsic_descriptor(*intrinsic).bytes_fold_role, args),
            _ => continue,
        };
        if role == Some(BytesFoldRole::ProducerSlice) && !args.is_empty() {
            let src = args[0];
            if src < n_regs && const_len[src].is_none() && analysis.region_def_count(src)? == 0 {
                dynamic_len_source[src] = true;
            }
        }
    }

    // A length source resolves iff it is an in-region foldable producer, has a
    // constant byte length, or is a loop-invariant dynamic Bytes input.
    let resolves = |r: usize, foldable: &[bool]| -> bool {
        r < n_regs && (foldable[r] || const_len[r].is_some() || dynamic_len_source[r])
    };
    let mut foldable = vec![false; n_regs];
    let mut changed = true;
    while changed {
        changed = false;
        for r in parallel_indices(0..n_regs) {
            if foldable[r] {
                continue;
            }
            let ok = match &producer[r] {
                Some(BProducer::FromString { src }) => resolves(*src, &foldable),
                Some(BProducer::Slice { src, .. }) => resolves(*src, &foldable),
                Some(BProducer::Alias { src }) => resolves(*src, &foldable),
                None => false,
            };
            if ok {
                foldable[r] = true;
                changed = true;
            }
        }
    }

    // Escape analysis: every use of a foldable (to-be-dissolved) Bytes register must be
    // a sanctioned Bytes consumer — a foldable producer operand, a `Move` to a foldable
    // register, or a `Bytes.len`. Any other read (in OR out of region) escapes; drop it
    // and re-resolve to a fixpoint. (Constant-length sources are NOT dissolved, so their
    // other uses — e.g. the still-live `data` slice arg — do not need to be sanctioned.)
    loop {
        let mut escaped = vec![false; n_regs];
        let note_reads = |rs: &[usize], escaped: &mut Vec<bool>| {
            for &r in rs {
                if r < n_regs && foldable[r] {
                    escaped[r] = true;
                }
            }
        };
        analysis.mark_external_reads_touching(code, &foldable, &mut escaped)?;
        for i in parallel_indices(header..exit) {
            match &code[i] {
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
                    && intrinsic_descriptor(*intrinsic).bytes_fold_role
                        == Some(BytesFoldRole::ProducerFromString)
                    && args.len() == 1 => {}
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
                    && intrinsic_descriptor(*intrinsic).bytes_fold_role
                        == Some(BytesFoldRole::ProducerSlice)
                    && args.len() == 3 =>
                {
                    // args[0] is the (foldable or constant) source consumed as Bytes;
                    // args[1]/args[2] are Int operands. Fine.
                }
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {}
                _ if is_bytes_len(&code[i]) => {}
                other => match instr_read_regs(other) {
                    RegFootprint::Some(rs) => note_reads(&rs, &mut escaped),
                    RegFootprint::All => return None,
                },
            }
        }
        if !escaped.iter().any(|&e| e) {
            break;
        }
        for r in parallel_indices(0..n_regs) {
            if escaped[r] {
                foldable[r] = false;
            }
        }
        let mut c2 = true;
        while c2 {
            c2 = false;
            for r in parallel_indices(0..n_regs) {
                if !foldable[r] {
                    continue;
                }
                let still = match &producer[r] {
                    Some(BProducer::FromString { src })
                    | Some(BProducer::Slice { src, .. })
                    | Some(BProducer::Alias { src }) => resolves(*src, &foldable),
                    None => false,
                };
                if !still {
                    foldable[r] = false;
                    c2 = true;
                }
            }
        }
    }

    // Unresolved direct `Bytes.len` reads remain ordinary native helpers. This lets the
    // pass dissolve an independent non-escaping slice without making unrelated dynamic
    // length reads a precondition for the whole region.

    // --- Length registers + constant materialization ---------------------------------
    // One fresh Int `len_reg` per resolvable register (foldable producer OR constant
    // source). Constant sources get a `LoadInt` materialized at the region head so the
    // value is live after the native OSR entry (which lands at `header`).
    let needs_len = |r: usize, foldable: &[bool]| -> bool {
        r < n_regs && (foldable[r] || const_len[r].is_some() || dynamic_len_source[r])
    };
    let mut len_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    let mut const_sources: Vec<usize> = Vec::new();
    for r in parallel_indices(0..n_regs) {
        if needs_len(r, &foldable) {
            len_reg[r] = next_reg;
            next_reg += 1;
            if !foldable[r] {
                // a pure constant source (not an in-region producer we dissolve)
                if const_len[r].is_some() {
                    const_sources.push(r);
                }
            }
        }
    }

    fn emit_max_b(out_code: &mut Vec<RegInstr>, out: usize, a: usize, b: usize) {
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
    fn emit_min_b(out_code: &mut Vec<RegInstr>, out: usize, a: usize, b: usize) {
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
    // out = clamp slice length, byte-exact mirror of `bytes_slice`:
    //   sc = max(start,0); if sc >= L { 0 } else { min(max(len,0), L-sc) }.
    // This form cannot overflow for `len == i64::MAX`, unlike computing `sc + len`,
    // and matches the runtime's saturating `usize` addition.
    // The emitter updates an origin entry for each synthesized instruction.
    fn emit_slice_len_b(
        out_code: &mut Vec<RegInstr>,
        out: usize,
        l_src: usize,
        start: usize,
        len: usize,
        next_reg: &mut usize,
    ) {
        let zero = *next_reg;
        let sc = *next_reg + 1;
        let available = *next_reg + 2;
        let lc = *next_reg + 3;
        *next_reg += 4;
        out_code.push(RegInstr::LoadInt {
            dst: zero,
            value: 0,
        });
        emit_max_b(out_code, sc, start, zero);
        let empty = out_code.len();
        out_code.push(RegInstr::JumpIfIntCompare {
            lhs: sc,
            rhs: l_src,
            op: RegIntCompare::GreaterEqual,
            expected: true,
            target: 0,
        });
        out_code.push(RegInstr::SubInt {
            dst: available,
            lhs: l_src,
            rhs: sc,
        });
        emit_max_b(out_code, lc, len, zero);
        emit_min_b(out_code, out, lc, available);
        let done = out_code.len();
        out_code.push(RegInstr::Jump { target: 0 });
        let empty_target = out_code.len();
        out_code.push(RegInstr::LoadInt { dst: out, value: 0 });
        let merge = out_code.len();
        if let RegInstr::JumpIfIntCompare { target, .. } = &mut out_code[empty] {
            *target = empty_target;
        }
        if let RegInstr::Jump { target } = &mut out_code[done] {
            *target = merge;
        }
    }

    // --- Rewrite the stream ----------------------------------------------------------
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
        // Materialize the constant-length sources AT the header position, emitted BEFORE
        // the header instruction is pushed but AFTER `index_map[header]` is set — so
        // `index_map[header]` points at these `LoadInt`s. This makes them dominate EVERY
        // in-region use INCLUDING the header's own folded instruction: a loop whose
        // CONDITION reads a folded `Bytes.len` (e.g. `while i < Bytes.len(data)`) lowers
        // the header to read `len_reg[src]`, which must already be initialized when the
        // header runs. Native OSR entry lands at the header's mapped ip
        // (`index_map[header]`), and the loop back-edge also targets it, so on every
        // iteration (incl. the first OSR one) these constants run before the condition.
        // Re-running these idempotent `LoadInt`s per iteration is cheap.
        if i == header {
            for &r in &const_sources {
                if let Some(value) = const_len[r] {
                    new_code.push(RegInstr::LoadInt {
                        dst: len_reg[r],
                        value,
                    });
                }
            }
        }
        if in_region(i) {
            let folded = match instr {
                RegInstr::CallIntrinsic { dst, intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { dst, intrinsic, .. }
                    if foldable[*dst]
                        && intrinsic_descriptor(*intrinsic).bytes_fold_role
                            == Some(BytesFoldRole::ProducerFromString) =>
                {
                    if let Some(BProducer::FromString { src }) = &producer[*dst] {
                        new_code.push(RegInstr::Move {
                            dst: len_reg[*dst],
                            src: len_reg[*src],
                        });
                    }
                    true
                }
                RegInstr::CallIntrinsic { dst, intrinsic, .. }
                | RegInstr::CallTypedIntrinsic { dst, intrinsic, .. }
                    if foldable[*dst]
                        && intrinsic_descriptor(*intrinsic).bytes_fold_role
                            == Some(BytesFoldRole::ProducerSlice) =>
                {
                    if let Some(BProducer::Slice { src, start, len }) = &producer[*dst] {
                        if dynamic_len_source[*src] {
                            new_code.push(RegInstr::CallIntrinsic {
                                dst: len_reg[*src],
                                intrinsic: RegIntrinsic::BytesLen,
                                args: vec![*src],
                            });
                        }
                        emit_slice_len_b(
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
                RegInstr::Move { dst, src } if foldable[*dst] && foldable[*src] => {
                    new_code.push(RegInstr::Move {
                        dst: len_reg[*dst],
                        src: len_reg[*src],
                    });
                    true
                }
                _ if is_bytes_len(instr) => {
                    let (dst, src) = match instr {
                        RegInstr::CallIntrinsic { dst, args, .. }
                        | RegInstr::CallTypedIntrinsic { dst, args, .. } => (*dst, args[0]),
                        _ => unreachable!(),
                    };
                    if foldable[src] || const_len[src].is_some() {
                        new_code.push(RegInstr::Move {
                            dst,
                            src: len_reg[src],
                        });
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if folded {
                continue;
            }
        }
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
