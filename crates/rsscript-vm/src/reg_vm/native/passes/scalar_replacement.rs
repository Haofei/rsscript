use super::*;

/// Two-armed scalar `Result` dissolution (heap-aware deopt #7 follow-up). Companion to
/// [`native_scalar_replace_results_in_region`]'s always-`Ok` path: handles a Result
/// constructed as EITHER `Ok(scalar)` or `Err(scalar)` in-region and consumed (matched)
/// in-region. Each RES register becomes a boolean `tag` (true = `Ok`) plus one shared
/// scalar `payload`; `MatchResult` routes on the tag and each arm's `UnwrapVariantValue`
/// reads the payload (which holds the constructed arm's value).
///
/// Live-after is supported: a RES register read after the region reconstructs `Ok` or
/// `Err` from the tag + payload at OSR-exit. This is sound WITHOUT a definite-assignment
/// analysis because the rewrite sets `tag` and `payload` at EVERY point the original
/// Result is assigned, so they are definitely-assigned wherever the Result is — hence in
/// the OSR-exit deopt live set whenever the Result is live there. `?`-short-circuit
/// (`TryResult`) and a RES register written AFTER the region (post-loop reassignment) or
/// read BEFORE it (live-in) are out of scope ⇒ bail. `res` is the move-closed RES set.
///
/// LIMITATION (the `Ok` and `Err` arms share ONE `payload` register): this only OSRs
/// when both arms carry the SAME native type (e.g. `Result<Int, Int>`). A Result whose
/// arms differ (e.g. `Result<Int, String>` — Int `Ok`, Handle `Err`) assigns the shared
/// payload conflicting types, so the native type inference rejects it and the loop
/// declines to OSR (SAFE — runs on the interpreter, never incorrect). Supporting
/// different-typed arms needs a per-arm payload register; the heap arm additionally
/// needs the extended deopt ABI (carrying Handle payloads) for the live-after case.
/// Verified by probe 2026-06-28: same-typed arms OSR; `Result<Int,String>` declines.
#[cfg(feature = "native-jit")]
#[allow(clippy::needless_range_loop, clippy::type_complexity)]
pub(super) fn native_scalar_replace_two_armed_results_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
    res: &[bool],
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<ResultRecipe>)> {
    let in_region = |i: usize| i >= header && i < exit;

    // Validate every in-region def/use of a RES register: defs are `Ok`/`Err`
    // constructors with one scalar (non-RES) field, or a `Move` from another RES; uses
    // are `MatchResult`/`UnwrapVariantValue`. `?` (`TryResult`) and any other touch bail.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeVariant {
                dst,
                layout,
                fields,
            } if res[*dst] => {
                let name = layout.name.as_ref();
                if name != "Ok" && name != "Err" {
                    return None;
                }
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
            RegInstr::UnwrapVariantValue { dst, src, .. } if res[*src] => {
                if res[*dst] {
                    return None;
                }
            }
            RegInstr::TryResult { src, .. } if res[*src] => return None,
            other => {
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

    // Boundary: a RES register WRITTEN after the region (post-loop reassignment) or READ
    // before it (live-in) is out of scope ⇒ bail. A RES register READ after the region is
    // live-after ⇒ mark it for OSR-exit reconstruction.
    let mut reconstruct = vec![false; n_regs];
    for i in 0..code.len() {
        if in_region(i) {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && res[r]) {
                    return None;
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && res[r] {
                        if i < header {
                            return None; // live-in Result
                        }
                        reconstruct[r] = true;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }

    // Allocate a `tag` + PER-ARM `ok_payload` + `err_payload` register per RES register
    // (consecutively). Separate per-arm payloads let arms of different native types
    // (e.g. `Result<Int, String>`: Int `Ok`, Handle `Err`) each carry their own typed
    // value instead of forcing one shared payload register into conflicting types.
    let mut tag_reg = vec![0usize; n_regs];
    let mut ok_payload_reg = vec![0usize; n_regs];
    let mut err_payload_reg = vec![0usize; n_regs];
    let mut next_reg = n_regs;
    for (reg, &is_res) in res.iter().enumerate() {
        if is_res {
            tag_reg[reg] = next_reg;
            ok_payload_reg[reg] = next_reg + 1;
            err_payload_reg[reg] = next_reg + 2;
            next_reg += 3;
        }
    }

    // Live-after reconstruction recipes:
    // `(variant_reg, ok_payload, err_payload, Some(tag_reg))` ⇒ rebuild `Ok(ok_payload)`
    // / `Err(err_payload)` from the tag at OSR-exit. Sound without a definite-assignment
    // pass: `tag` + the taken arm's payload are written at every RES def, so the payload
    // matching the live tag is in the deopt live set wherever the Result is live (the
    // other arm's payload is never read).
    let recipes: Vec<ResultRecipe> = reconstruct
        .iter()
        .enumerate()
        .filter(|&(_, &needs)| needs)
        .map(|(reg, _)| {
            (
                reg,
                ok_payload_reg[reg],
                err_payload_reg[reg],
                Some(tag_reg[reg]),
            )
        })
        .collect();

    // Rewrite, dissolving in-region Result ops; remap jump/match targets through the map.
    enum Fix {
        Target(usize),
        Match { a: usize, b: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len() + 8);
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeVariant {
                dst,
                layout,
                fields,
                ..
            } if region && res[*dst] => {
                let is_ok = layout.name.as_ref() == "Ok";
                new_code.push(RegInstr::LoadBool {
                    dst: tag_reg[*dst],
                    value: is_ok,
                });
                // Write only the taken arm's payload register (the tag routes the
                // matching unwrap; the other arm's payload is never read for this value).
                let (_, field_reg) = &fields[0];
                new_code.push(RegInstr::Move {
                    dst: if is_ok {
                        ok_payload_reg[*dst]
                    } else {
                        err_payload_reg[*dst]
                    },
                    src: *field_reg,
                });
            }
            RegInstr::Move { dst, src } if region && res[*dst] => {
                new_code.push(RegInstr::Move {
                    dst: tag_reg[*dst],
                    src: tag_reg[*src],
                });
                new_code.push(RegInstr::Move {
                    dst: ok_payload_reg[*dst],
                    src: ok_payload_reg[*src],
                });
                new_code.push(RegInstr::Move {
                    dst: err_payload_reg[*dst],
                    src: err_payload_reg[*src],
                });
            }
            RegInstr::MatchResult { src, ok_ip, err_ip } if region && res[*src] => {
                // tag true (Ok) ⇒ ok_ip; else fall through to an unconditional jump to err_ip.
                fixups.push((new_code.len(), Fix::Target(*ok_ip)));
                new_code.push(RegInstr::JumpIfBool {
                    cond: tag_reg[*src],
                    expected: true,
                    target: 0,
                });
                fixups.push((new_code.len(), Fix::Target(*err_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            RegInstr::UnwrapVariantValue {
                dst, src, expected, ..
            } if region && res[*src] => {
                // Read the payload register for the arm this unwrap belongs to: the `Err`
                // unwrap (reached only on the `Err` branch) reads `err_payload`; `Ok`
                // (the default) reads `ok_payload`.
                let src_payload = if expected.as_str() == "Err" {
                    err_payload_reg[*src]
                } else {
                    ok_payload_reg[*src]
                };
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: src_payload,
                });
            }
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
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, recipes))
}

/// OSR × scalar replacement for VARIANTS: scalar-replace non-escaping user `sum`/variant values that
/// live entirely inside the loop region `[header, exit)` of an otherwise native-
/// INELIGIBLE function. Mirrors [`native_scalar_replace_options_in_region`] but for
/// `MakeVariant`/`MatchVariant`/`UnwrapVariantValue`.
///
/// Scope (multiple scalar payload fields per arm; N>=0 per arm, possibly different N
/// per arm): a variant register `R` is replaced iff every definition is a `MakeVariant`
/// whose arm carries only **scalar fields** (each payload register must end up
/// Int/Float/Bool — never a heap/Option/variant value, and never itself a replaceable
/// variant register) or a `Move` from another replaceable variant register; every use
/// is `MatchVariant{src:R}`, `UnwrapVariantValue{src:R}`, a `GetField{base:R}` (the
/// payload-field read a struct-style arm pattern lowers to), or a `Move{src:R}` to
/// another replaceable variant register; and `R` never appears as a value operand of
/// anything else. Heap/sum-payload arms (incl. nested variants/structs) ⇒ bail. If any
/// in-region variant register is not replaceable, the whole region pass bails (no OSR),
/// exactly like the Option pass.
///
/// Rewrite: each `R` becomes a `tag` register (Int holding the arm index) PLUS one
/// fresh scalar **leaf register per `(arm, slot)`** — i.e. every payload field of every
/// arm in `R`'s alias class gets its own leaf register (so a 3-field arm dissolves to a
/// tag plus three leaf registers). A per-class arm-name→tag-index map is built by
/// scanning the class's `MakeVariant` arm names AND its `MatchVariant`/
/// `UnwrapVariantValue` `expected` names, assigning each distinct name a stable index.
/// Per arm, the slot order is the arm's `MakeVariant` `layout.field_names` (all defs of
/// one arm must agree on field names; otherwise bail).
/// - `MakeVariant{dst:R, layout, fields}` → `LoadInt tag = idx(layout.name)`; for each
///   slot, `Move leaf[(arm, slot)] = <that field reg>` (fieldless arms write only the
///   tag).
/// - `MatchVariant{src:R, expected, match_ip, else_ip}` → `LoadInt c = idx(expected)`;
///   `Equal eq = tag, c`; `JumpIfBool eq==true → match_ip`; `Jump → else_ip`.
/// - `GetField{dst, base:R, name}` → `Move dst = leaf[(arm, slot)]`, where the arm is
///   the unique class arm that declares field `name` (ambiguous/absent name ⇒ bail) and
///   the slot is `name`'s position in that arm's field list.
/// - `UnwrapVariantValue{dst, src:R, expected}` → `Move dst = leaf[(expected, 0)]` (the
///   single-field tuple-arm case; its arm is `expected`, slot 0).
/// - `Move` aliases copy the tag and every leaf register.
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→original
/// `ip_map` discipline as the Option region pass (each rewritten op's fragments map to
/// the original op's index; copy-through maps one-to-one).
#[cfg(feature = "native-jit")]
#[allow(clippy::needless_range_loop, clippy::type_complexity)]
pub(in crate::reg_vm) fn native_scalar_replace_variants_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<OsrMaterializeRecipe>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no variant op inside the region ⇒ nothing for THIS pass to do.
    if !(header..exit).any(|i| is_variant_op(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map, Vec::new()));
    }

    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // VAR = registers carrying a (replaceable) variant value: seed from in-region
    // `MakeVariant` dsts, close under in-region `Move` aliasing.
    let mut var = vec![false; n_regs];
    for i in header..exit {
        if let RegInstr::MakeVariant { dst, .. } = &code[i] {
            var[*dst] = true;
        }
    }
    analysis.close_region_move_aliases(code, &mut var)?;

    // Every in-region instruction must be native-subset, a variant op, or a
    // `GetField` reading a VAR register (the payload-field read a struct-style arm
    // pattern lowers to — its `base` is the matched variant). `GetField` on a
    // non-VAR base is a heap struct read this pass can't lower ⇒ bail.
    let is_var_getfield =
        |i: usize| -> bool { matches!(&code[i], RegInstr::GetField { base, .. } if var[*base]) };
    for i in header..exit {
        if !native_subset_instruction(&code[i]) && !is_variant_op(&code[i]) && !is_var_getfield(i) {
            return None;
        }
    }

    // Validate in-region uses/defs of VAR registers. Each `MakeVariant` arm carries
    // only scalar fields (no field may itself be a VAR register — that would be a
    // nested variant, out of scope ⇒ bail). Anything else touching a VAR register that
    // is not a recognized consumer ⇒ bail.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeVariant { dst, fields, .. } if var[*dst] => {
                if fields.iter().any(|(_, field_reg)| var[*field_reg]) {
                    return None; // nested variant payload ⇒ non-scalar
                }
            }
            RegInstr::Move { dst, src } if var[*dst] => {
                if !var[*src] {
                    return None;
                }
            }
            RegInstr::MatchVariant { src, .. } if var[*src] => {}
            RegInstr::UnwrapVariantValue { dst, src, .. } if var[*src] => {
                if var[*dst] {
                    return None; // unwrapped payload aliased as a variant ⇒ non-scalar
                }
            }
            // A payload-field read of a struct-style arm (`Rect(w, h) => ... read w`).
            // Its `dst` must NOT be a VAR register (a struct/variant-typed field is a
            // nested aggregate ⇒ out of scope, bail).
            RegInstr::GetField { dst, base, .. } if var[*base] => {
                if var[*dst] {
                    return None;
                }
            }
            // `DeepCopy` of a VAR register (e.g. from a `read`/param-marshalling of a
            // heap variant): a no-op once the variant is scalar-replaced (tag/payload
            // are copied by value). Allowed here; dropped in the rewrite.
            RegInstr::DeepCopy { reg } | RegInstr::DeepCopyElided { reg } if var[*reg] => {}
            RegInstr::Move { src, .. } if var[*src] => {}
            other => {
                let reads = subset_or_option_reads(other)?;
                if reads.into_iter().any(|r| var[r]) {
                    return None;
                }
                if let RegInstr::UnwrapVariantValue { dst, .. } | RegInstr::MakeVariant { dst, .. } =
                    other
                    && var[*dst]
                {
                    return None;
                }
            }
        }
    }

    // Permit a post-loop read only when the dissolved variant can be rebuilt from
    // its tag and selected arm leaves. Pre-loop reads and post-loop writes remain
    // unsupported and conservatively decline OSR.
    let mut reconstruct = vec![false; n_regs];
    for i in 0..code.len() {
        if in_region(i) {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && var[r]) {
                    return None;
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && var[r] {
                        if i < header {
                            return None;
                        }
                        reconstruct[r] = true;
                    }
                }
            }
            RegFootprint::All => return None,
        }
    }

    // `Move`-aliased VAR registers share ONE tag/payload register pair (the alias
    // copies both halves), and therefore must agree on the arm-name→index map. Group
    // VAR registers into alias classes with a union-find over in-region `Move` edges.
    let mut parent: Vec<usize> = (0..n_regs).collect();
    #[allow(clippy::needless_range_loop)]
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = x;
        while parent[c] != c {
            let n = parent[c];
            parent[c] = r;
            c = n;
        }
        r
    }
    for i in header..exit {
        if let RegInstr::Move { dst, src } = &code[i]
            && var[*dst]
            && var[*src]
        {
            let (a, b) = (find(&mut parent, *dst), find(&mut parent, *src));
            if a != b {
                parent[a] = b;
            }
        }
    }

    // Build ONE canonical arm-name→tag-index map per alias class. Scan every in-region
    // `MakeVariant` arm name AND `MatchVariant` `expected` name, collecting them under
    // the register's class root; assign each distinct name a stable index in sorted-
    // name order (deterministic, and identical across all registers in the class).
    let mut class_names: HashMap<usize, BTreeSet<String>> = HashMap::new();
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeVariant { dst, layout, .. } if var[*dst] => {
                let root = find(&mut parent, *dst);
                class_names
                    .entry(root)
                    .or_default()
                    .insert(layout.name.to_string());
            }
            RegInstr::MatchVariant { src, expected, .. } if var[*src] => {
                let root = find(&mut parent, *src);
                class_names
                    .entry(root)
                    .or_default()
                    .insert(expected.clone());
            }
            _ => {}
        }
    }
    let class_arm_index: HashMap<usize, HashMap<String, i64>> = class_names
        .into_iter()
        .map(|(root, names)| {
            let map: HashMap<String, i64> = names
                .into_iter()
                .enumerate()
                .map(|(k, name)| (name, k as i64))
                .collect();
            (root, map)
        })
        .collect();
    let arm_idx = |reg: usize, parent: &mut Vec<usize>, name: &str| -> i64 {
        let root = find(parent, reg);
        *class_arm_index
            .get(&root)
            .and_then(|m| m.get(name))
            .expect("arm name interned for its class")
    };

    // Per class, per arm-name, the slot-ordered payload field names (from each arm's
    // `MakeVariant` `layout.field_names`). All `MakeVariant` defs of one arm in a class
    // MUST agree on the field-name vector (they always do for a single `sum` type; a
    // disagreement means an unresolvable shape ⇒ bail). `(root, arm_name)` keys.
    let mut class_arm_fields: HashMap<(usize, String), Vec<Rc<str>>> = HashMap::new();
    let mut class_arm_layout: HashMap<(usize, String), Rc<crate::vm_value::TypeLayout>> =
        HashMap::new();
    for i in header..exit {
        if let RegInstr::MakeVariant { dst, layout, .. } = &code[i]
            && var[*dst]
        {
            let root = find(&mut parent, *dst);
            let key = (root, layout.name.to_string());
            let shape = layout.field_names.clone();
            match class_arm_fields.get(&key) {
                Some(prev) if *prev != shape => return None, // shape contradiction
                Some(_) => {}
                None => {
                    class_arm_fields.insert(key, shape);
                }
            }
            let key = (root, layout.name.to_string());
            match class_arm_layout.get(&key) {
                Some(previous)
                    if previous.name != layout.name
                        || previous.field_names != layout.field_names =>
                {
                    return None;
                }
                Some(_) => {}
                None => {
                    class_arm_layout.insert(key, Rc::clone(layout));
                }
            }
        }
    }

    // Per class, the field-name → owning arm-name map, used to resolve a
    // `GetField{base:R, name}` to its arm. A field name owned by MORE THAN ONE arm in a
    // class is ambiguous (we can't statically know which arm's leaf the read refers to)
    // ⇒ bail conservatively. Within one arm a field name is unique by construction.
    let mut class_field_owner: HashMap<(usize, String), String> = HashMap::new();
    for ((root, arm_name), fields) in &class_arm_fields {
        for fname in fields {
            let key = (*root, fname.to_string());
            match class_field_owner.get(&key) {
                Some(existing) if existing != arm_name => return None, // ambiguous field
                Some(_) => {}
                None => {
                    class_field_owner.insert(key, arm_name.clone());
                }
            }
        }
    }

    // Allocate fresh registers per alias class: ONE tag register, plus one leaf scalar
    // register per `(arm, slot)` across all arms of the class. `class_tag[root]` is the
    // tag register; `leaf_reg[(root, arm_name, slot)]` is the per-field leaf register.
    let mut tag_reg = vec![0usize; n_regs];
    let mut class_tag: HashMap<usize, usize> = HashMap::new();
    let mut leaf_reg: HashMap<(usize, String, usize), usize> = HashMap::new();
    let mut next_reg = n_regs;
    // Stable allocation order: roots ascending, then arms by tag-index, then slot.
    let mut roots: Vec<usize> = (0..n_regs)
        .filter(|&r| var[r])
        .map(|r| find(&mut parent, r))
        .collect();
    roots.sort_unstable();
    roots.dedup();
    for root in &roots {
        let t = next_reg;
        next_reg += 1;
        class_tag.insert(*root, t);
        // Arms in tag-index order for determinism.
        let mut arms: Vec<(String, Vec<Rc<str>>)> = class_arm_fields
            .iter()
            .filter(|((r, _), _)| r == root)
            .map(|((_, arm), fields)| (arm.clone(), fields.clone()))
            .collect();
        arms.sort_by_key(|(arm, _)| {
            class_arm_index
                .get(root)
                .and_then(|m| m.get(arm))
                .copied()
                .unwrap_or(i64::MAX)
        });
        for (arm, fields) in arms {
            for slot in 0..fields.len() {
                leaf_reg.insert((*root, arm.clone(), slot), next_reg);
                next_reg += 1;
            }
        }
    }
    for reg in 0..n_regs {
        if var[reg] {
            let root = find(&mut parent, reg);
            tag_reg[reg] = class_tag[&root];
        }
    }
    // Resolve a `(reg, arm, slot)` to its leaf register.
    let leaf_of = |reg: usize, parent: &mut Vec<usize>, arm: &str, slot: usize| -> Option<usize> {
        let root = find(parent, reg);
        leaf_reg.get(&(root, arm.to_string(), slot)).copied()
    };
    // Resolve a `GetField{base:R, name}` to its `(arm, slot)`: `name`'s owning arm in
    // R's class, and `name`'s position in that arm's field list.
    let getfield_arm_slot =
        |reg: usize, parent: &mut Vec<usize>, name: &str| -> Option<(String, usize)> {
            let root = find(parent, reg);
            let arm = class_field_owner.get(&(root, name.to_string()))?.clone();
            let slot = class_arm_fields
                .get(&(root, arm.clone()))?
                .iter()
                .position(|f| &**f == name)?;
            Some((arm, slot))
        };

    let mut recipes = Vec::new();
    for (reg, &needs) in reconstruct.iter().enumerate() {
        if !needs {
            continue;
        }
        let root = find(&mut parent, reg);
        let mut arms: Vec<OsrMaterializeVariantArm> = class_arm_layout
            .iter()
            .filter(|((class, _), _)| *class == root)
            .map(|((_, arm_name), layout)| {
                let tag = class_arm_index.get(&root)?.get(arm_name).copied()?;
                let fields = layout
                    .field_names
                    .iter()
                    .enumerate()
                    .map(|(slot, _)| {
                        leaf_of(reg, &mut parent, arm_name, slot).map(OsrMaterializeValue::Register)
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(OsrMaterializeVariantArm {
                    tag,
                    layout: Rc::clone(layout),
                    fields,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        arms.sort_by_key(|arm| arm.tag);
        if arms.is_empty() {
            return None;
        }
        recipes.push(OsrMaterializeRecipe {
            dst_reg: reg,
            value: OsrMaterializeValue::Variant {
                tag_reg: Some(tag_reg[reg]),
                arms,
            },
        });
    }

    // PRE-FLIGHT (bail-rather-than-panic): every in-region payload read of a VAR
    // register must resolve to a concrete leaf register BEFORE we rewrite. A
    // `GetField` whose name has no owning arm / unresolvable slot, or an
    // `UnwrapVariantValue` on an arm with no in-region `MakeVariant` (hence no leaf),
    // is rejected here so the rewrite below can rely on the lookups succeeding.
    for i in header..exit {
        match &code[i] {
            RegInstr::GetField { base, name, .. } if var[*base] => {
                let (arm, slot) = getfield_arm_slot(*base, &mut parent, name)?;
                leaf_of(*base, &mut parent, &arm, slot)?;
            }
            RegInstr::UnwrapVariantValue { src, expected, .. } if var[*src] => {
                // Tuple-style single-field arm: payload is slot 0 of `expected`.
                leaf_of(*src, &mut parent, expected, 0)?;
            }
            _ => {}
        }
    }

    // Rewrite the whole code, scalar-replacing in-region variant ops and copying the
    // rest through, remapping all jump/match targets through the index map. Each
    // rewritten op may emit MULTIPLE instructions; jump fixups are resolved after.
    enum Fix {
        Target(usize),
        Match { some_ip: usize, none_ip: usize },
        VariantMatch { match_ip: usize, else_ip: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    // Scratch registers for `MatchVariant` lowering (constant + equality result). One
    // dedicated pair, reused across all matches (each match fully consumes them before
    // branching, and they are never live across the branch).
    let cmp_const_reg = next_reg;
    let cmp_eq_reg = next_reg + 1;
    next_reg += 2;
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeVariant {
                dst,
                layout,
                fields,
            } if region && var[*dst] => {
                let idx = arm_idx(*dst, &mut parent, &layout.name);
                new_code.push(RegInstr::LoadInt {
                    dst: tag_reg[*dst],
                    value: idx,
                });
                // Each scalar payload field → `Move` into its `(arm, slot)` leaf
                // register. `fields` is in canonical slot order (matching the arm's
                // `layout.field_names`, validated above), so slot = field index.
                for (slot, (_, field_reg)) in fields.iter().enumerate() {
                    let leaf = leaf_of(*dst, &mut parent, &layout.name, slot)
                        .expect("MakeVariant arm/slot leaf interned");
                    new_code.push(RegInstr::Move {
                        dst: leaf,
                        src: *field_reg,
                    });
                }
            }
            RegInstr::Move { dst, src } if region && var[*dst] => {
                // Alias copy: source and dst share a class ⇒ identical tag and leaf
                // registers ⇒ these would be self-copies. Emit nothing.
                let _ = (dst, src);
            }
            RegInstr::MatchVariant {
                src,
                expected,
                match_ip,
                else_ip,
            } if region && var[*src] => {
                let idx = arm_idx(*src, &mut parent, expected);
                new_code.push(RegInstr::LoadInt {
                    dst: cmp_const_reg,
                    value: idx,
                });
                new_code.push(RegInstr::Equal {
                    dst: cmp_eq_reg,
                    lhs: tag_reg[*src],
                    rhs: cmp_const_reg,
                });
                fixups.push((new_code.len(), Fix::Target(*match_ip)));
                new_code.push(RegInstr::JumpIfBool {
                    cond: cmp_eq_reg,
                    expected: true,
                    target: 0,
                });
                fixups.push((new_code.len(), Fix::Target(*else_ip)));
                new_code.push(RegInstr::Jump { target: 0 });
            }
            // Tuple-style single-field arm payload read: slot 0 of `expected`.
            RegInstr::UnwrapVariantValue { dst, src, expected } if region && var[*src] => {
                let leaf = leaf_of(*src, &mut parent, expected, 0)
                    .expect("UnwrapVariantValue arm leaf interned (pre-flight checked)");
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: leaf,
                });
            }
            // Struct-style arm payload-field read: `Move dst = leaf[(arm, slot)]`.
            RegInstr::GetField { dst, base, name } if region && var[*base] => {
                let (arm, slot) = getfield_arm_slot(*base, &mut parent, name)
                    .expect("GetField arm/slot resolvable (pre-flight checked)");
                let leaf = leaf_of(*base, &mut parent, &arm, slot)
                    .expect("GetField leaf interned (pre-flight checked)");
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: leaf,
                });
            }
            // `DeepCopy` of a scalar-replaced variant: drop it (scalars copy by value).
            RegInstr::DeepCopy { reg } | RegInstr::DeepCopyElided { reg }
                if region && var[*reg] => {}
            // Copy-through, remapping jump/match targets.
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
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::VariantMatch {
                        match_ip: *match_ip,
                        else_ip: *else_ip,
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
            Fix::VariantMatch { match_ip, else_ip } => {
                let (m, e) = (index_map[match_ip], index_map[else_ip]);
                if let RegInstr::MatchVariant {
                    match_ip: md,
                    else_ip: ed,
                    ..
                } = &mut new_code[pos]
                {
                    *md = m;
                    *ed = e;
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, recipes))
}

/// OSR × scalar replacement for non-escaping FLAT user STRUCTS. Mirrors
/// Resolve the declared layout shape (`field_names`) of a struct-valued register by
/// walking its in-region definitions. A register defined by `MakeStruct` carries the
/// shape directly; a `Move` forwards its source's shape; a `GetFieldSlot{dst, base,
/// slot}` reading a struct-typed field has the shape of whatever `MakeStruct` wrote
/// that slot of `base` (i.e. the inner field's own struct shape). Returns `None` when
/// the shape is ambiguous or not statically resolvable (⇒ the caller bails OSR).
#[cfg(all(
    feature = "native-jit",
    any(test, feature = "jit-struct-sr-experimental")
))]
pub(in crate::reg_vm) fn struct_shape_of_reg(
    code: &[RegInstr],
    header: usize,
    exit: usize,
    reg: usize,
) -> Option<Vec<Rc<str>>> {
    fn go(
        code: &[RegInstr],
        header: usize,
        exit: usize,
        reg: usize,
        depth: usize,
    ) -> Option<Vec<Rc<str>>> {
        if depth > 64 {
            return None;
        }
        let mut found: Option<Vec<Rc<str>>> = None;
        for i in header..exit {
            match &code[i] {
                RegInstr::MakeStruct { dst, layout, .. } if *dst == reg => {
                    let shape = layout.field_names.clone();
                    match &found {
                        Some(prev) if *prev != shape => return None,
                        _ => found = Some(shape),
                    }
                }
                RegInstr::Move { dst, src } if *dst == reg => {
                    let shape = go(code, header, exit, *src, depth + 1)?;
                    match &found {
                        Some(prev) if *prev != shape => return None,
                        _ => found = Some(shape),
                    }
                }
                RegInstr::GetFieldSlot { dst, base, slot } if *dst == reg => {
                    // The shape of `reg` is the shape of the struct stored in `base`'s
                    // `slot`: resolve the field-source register that filled that slot of
                    // `base` (following `base` through Move aliases to its `MakeStruct`),
                    // then recurse on that source's shape.
                    let base_shape = go(code, header, exit, *base, depth + 1)?;
                    let slot_name = base_shape.get(*slot)?.clone();
                    let mut field_shape: Option<Vec<Rc<str>>> = None;
                    for &fsrc in &field_srcs_of(code, header, exit, *base, &slot_name, depth + 1) {
                        let s = go(code, header, exit, fsrc, depth + 1)?;
                        match &field_shape {
                            Some(prev) if *prev != s => return None,
                            _ => field_shape = Some(s),
                        }
                    }
                    let shape = field_shape?;
                    match &found {
                        Some(prev) if *prev != shape => return None,
                        _ => found = Some(shape),
                    }
                }
                _ => {}
            }
        }
        found
    }
    // Every register that feeds field `name` across ALL of `base`'s `MakeStruct`
    // definitions, resolving `base` through `Move` aliases. (A class can have several
    // `MakeStruct` defs along different paths; each must agree on shape, validated by
    // the caller.)
    fn field_srcs_of(
        code: &[RegInstr],
        header: usize,
        exit: usize,
        base: usize,
        name: &Rc<str>,
        depth: usize,
    ) -> Vec<usize> {
        if depth > 64 {
            return vec![];
        }
        let mut out = Vec::new();
        for i in header..exit {
            match &code[i] {
                RegInstr::MakeStruct { dst, fields, .. } if *dst == base => {
                    if let Some((_, fsrc)) = fields.iter().find(|(n, _)| **n == **name) {
                        out.push(*fsrc);
                    }
                }
                // Resolve through a Move alias: `base = Move(src)` ⇒ inherit src's defs.
                RegInstr::Move { dst, src } if *dst == base => {
                    out.extend(field_srcs_of(code, header, exit, *src, name, depth + 1));
                }
                _ => {}
            }
        }
        out
    }
    go(code, header, exit, reg, 0)
}

/// [`native_scalar_replace_variants_in_region`] but for `MakeStruct`/`GetFieldSlot`,
/// with RECURSIVE (nested-struct) dissolution.
///
/// A struct register `R` is dissolvable iff it is non-escaping and every field is
/// EITHER a scalar OR itself a dissolvable struct register, recursively. The whole
/// nested struct dissolves to ONE fresh register per LEAF SCALAR field (innermost-
/// first): a struct-typed field slot owns no register of its own — it aliases the
/// inner struct register's leaf registers (union-find), so a chained read `a.b.c`
/// becomes plain register moves end-to-end and the nested struct never allocates.
///
/// Membership grows to a fixpoint over three relations: `Move` aliasing, a
/// `MakeStruct` field whose source is itself a struct register (⇒ that slot is
/// struct-typed), and a `GetFieldSlot` reading a struct-typed slot (⇒ its dst is a
/// struct register too). A struct-typed slot's per-slot anchor is unioned with the
/// inner struct register's class, so they literally share leaf registers.
///
/// Bails (conservative; when unsure REJECT) on: any escaping use of a struct register
/// (read as a non-field/non-alias value operand, returned, stored, captured, alive at
/// either OSR boundary), a field that is a heap value or a NON-dissolvable struct, a
/// shape contradiction (scalar in a struct slot or vice-versa), or an unresolvable
/// shape. The flat-struct case is the depth-1 instance of this recursion.
///
/// Rewrite:
/// - `MakeStruct{dst:R}`: scalar field → `Move leaf = src`; nested field → nothing
///   (aliased to the inner's already-written leaf registers).
/// - `GetFieldSlot{dst, base:R, slot}`: scalar slot → `Move dst = leaf`; struct slot →
///   nothing (`dst` aliases the inner's leaf registers).
/// - `Move` struct alias → nothing (shared leaf registers ⇒ self-copies).
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→original
/// `ip_map` discipline as the other two region passes.
#[cfg(all(
    feature = "native-jit",
    any(test, feature = "jit-struct-sr-experimental")
))]
#[allow(clippy::needless_range_loop, clippy::type_complexity)]
pub(in crate::reg_vm) fn native_scalar_replace_structs_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<OsrMaterializeRecipe>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;

    // Fast path: no `MakeStruct` inside the region ⇒ nothing for THIS pass to do.
    // (A bare `GetFieldSlot` whose base is a handle param stays a native heap read.)
    if !(header..exit).any(|i| is_make_struct_op(&code[i])) {
        let ip_map: Vec<usize> = (0..code.len()).collect();
        return Some((code.to_vec(), n_regs, ip_map, Vec::new()));
    }

    // Every in-region instruction must be native-subset or a `MakeStruct`.
    for i in header..exit {
        if !native_subset_instruction(&code[i]) && !is_make_struct_op(&code[i]) {
            return None;
        }
    }

    // STR = registers carrying a (replaceable) struct value. Nested support: a struct
    // register is dissolvable when every field is EITHER a scalar OR itself a
    // dissolvable struct register, recursively. We seed STR from in-region
    // `MakeStruct` dsts, then close under THREE relations to a fixpoint:
    //   (1) `Move{dst,src}` with `src` STR ⇒ `dst` is STR (alias);
    //   (2) a `MakeStruct{dst:R}` field whose `src` is STR makes that field a NESTED
    //       (struct-typed) slot of R's shape;
    //   (3) `GetFieldSlot{dst, base:R, slot}` reading a NESTED slot of R ⇒ `dst` is a
    //       STR register (it aliases the inner struct), which can in turn expose more
    //       nested slots / Move-aliases.
    // The slot-kind map (per layout shape, by field name) records whether a slot is
    // scalar or struct-typed; we discover it incrementally as STR membership grows.
    let mut strv = vec![false; n_regs];
    for i in header..exit {
        if let RegInstr::MakeStruct { dst, .. } = &code[i] {
            strv[*dst] = true;
        }
    }
    // `nested_slots[shape_field_names] = set of field-name indices that are struct-typed`.
    // Keyed by the layout's `field_names` (the canonical shape) so all structs of the
    // same declared type share one slot-kind classification.
    let mut nested_slots: HashMap<Vec<Rc<str>>, std::collections::HashSet<usize>> = HashMap::new();
    let mut changed = true;
    while changed {
        changed = false;
        for i in header..exit {
            match &code[i] {
                RegInstr::Move { dst, src } => {
                    if strv[*src] && !strv[*dst] {
                        strv[*dst] = true;
                        changed = true;
                    }
                }
                RegInstr::MakeStruct {
                    dst,
                    layout,
                    fields,
                } if strv[*dst] => {
                    let set = nested_slots.entry(layout.field_names.clone()).or_default();
                    for (name, src) in fields {
                        if strv[*src]
                            && let Some(slot) =
                                layout.field_names.iter().position(|n| **n == **name)
                            && set.insert(slot)
                        {
                            changed = true;
                        }
                    }
                }
                RegInstr::GetFieldSlot { dst, base, slot } if strv[*base] => {
                    // Is `slot` a nested (struct-typed) slot of `base`'s shape? Find
                    // `base`'s shape via any in-region `MakeStruct` defining its class;
                    // for now match against EVERY shape whose nested set contains `slot`
                    // AND that `base` could carry. We resolve `base`'s exact shape during
                    // class shaping; here we conservatively promote `dst` to STR when the
                    // slot is nested under any shape that `base` is built with. Since a
                    // register has a single shape, we look it up from its def.
                    if !strv[*dst]
                        && let Some(shape) = struct_shape_of_reg(code, header, exit, *base)
                        && nested_slots.get(&shape).is_some_and(|s| s.contains(slot))
                    {
                        strv[*dst] = true;
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }

    // Validate in-region uses/defs of STR registers.
    // - `MakeStruct{dst:R}`: each field `src` is EITHER a scalar (non-STR) OR an STR
    //   register sitting in a nested slot (recorded above). A scalar reg in a nested
    //   slot, or an STR reg in a non-nested slot, is a shape contradiction ⇒ bail.
    // - `Move{dst,src}` writing an STR reg: `src` must be STR (alias copy).
    // - `GetFieldSlot{dst, base:R}`: `dst` is STR exactly when the slot is nested.
    // - Any OTHER read of an STR reg as a value operand ⇒ escape ⇒ bail.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeStruct {
                dst,
                layout,
                fields,
            } if strv[*dst] => {
                let set = nested_slots.get(&layout.field_names);
                for (name, src) in fields {
                    let slot = layout.field_names.iter().position(|n| **n == **name)?;
                    let is_nested = set.is_some_and(|s| s.contains(&slot));
                    if strv[*src] != is_nested {
                        return None; // scalar-in-struct-slot or struct-in-scalar-slot
                    }
                }
            }
            RegInstr::Move { dst, src } if strv[*dst] => {
                if !strv[*src] {
                    return None;
                }
            }
            RegInstr::GetFieldSlot { dst, base, slot } if strv[*base] => {
                let shape = struct_shape_of_reg(code, header, exit, *base)?;
                let is_nested = nested_slots.get(&shape).is_some_and(|s| s.contains(slot));
                if strv[*dst] != is_nested {
                    return None;
                }
            }
            RegInstr::Move { src, .. } if strv[*src] => {}
            other => {
                let reads = subset_or_option_reads(other)?;
                if reads.into_iter().any(|r| strv[r]) {
                    return None;
                }
                if let RegInstr::GetFieldSlot { dst, .. } | RegInstr::MakeStruct { dst, .. } = other
                    && strv[*dst]
                {
                    return None;
                }
            }
        }
    }

    // Permit clean live-after reconstruction, but continue to reject live-in reads,
    // post-loop writes, and imprecise register footprints.
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;
    let mut reconstruct = vec![false; n_regs];
    for i in 0..code.len() {
        if in_region(i) {
            continue;
        }
        match instr_written_reg(&code[i]) {
            RegFootprint::Some(regs) => {
                if i >= exit && regs.iter().any(|&r| r < n_regs && strv[r]) {
                    return None;
                }
            }
            RegFootprint::All => return None,
        }
        match instr_read_regs(&code[i]) {
            RegFootprint::Some(regs) => {
                for r in regs {
                    if r < n_regs && strv[r] {
                        if i < header {
                            return None;
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
        let defs: Vec<usize> = analysis
            .writer_ips_of(code, reg)?
            .into_iter()
            .filter(|&i| in_region(i))
            .collect();
        if defs.len() != 1 {
            return None;
        }
        for instr in &code[header..defs[0]] {
            match instr {
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

    // Alias union-find. STR registers that name the SAME logical struct value share
    // ONE leaf-register layout. We union over:
    //   (a) in-region `Move{dst,src}` where both are STR (plain alias);
    //   (b) `MakeStruct{dst:R, field src}` nested slot ⇒ union R's per-slot ANCHOR with
    //       `src`'s class (the slot IS the inner struct's registers);
    //   (c) `GetFieldSlot{dst, base:R, slot}` nested ⇒ union `dst` with R's slot anchor.
    // Per-slot anchors are virtual ids ≥ n_regs (one per (class-representative, slot)).
    // Because anchors are created lazily and keyed by a register's find-root, we resolve
    // them through a small interning map below.
    let mut parent: Vec<usize> = (0..n_regs).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut c = x;
        while parent[c] != c {
            let n = parent[c];
            parent[c] = r;
            c = n;
        }
        r
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[ra] = rb;
        }
    }
    // Lazily intern a virtual anchor id for the (struct-value, slot) pair. Keyed by the
    // current find-root of `base` so all aliases of `base` share the anchor.
    fn anchor_of(
        parent: &mut Vec<usize>,
        anchors: &mut HashMap<(usize, usize), usize>,
        base: usize,
        slot: usize,
    ) -> usize {
        let root = find(parent, base);
        if let Some(&a) = anchors.get(&(root, slot)) {
            return a;
        }
        let id = parent.len();
        parent.push(id);
        anchors.insert((root, slot), id);
        id
    }
    let mut anchors: HashMap<(usize, usize), usize> = HashMap::new();
    // (a) plain Move aliases.
    for i in header..exit {
        if let RegInstr::Move { dst, src } = &code[i]
            && strv[*dst]
            && strv[*src]
        {
            union(&mut parent, *dst, *src);
        }
    }
    // (b)+(c): union nested-slot anchors with their inner struct registers. Iterate to a
    // fixpoint because anchors are keyed by find-roots that the unions themselves change.
    let mut changed = true;
    while changed {
        changed = false;
        for i in header..exit {
            match &code[i] {
                RegInstr::MakeStruct {
                    dst,
                    layout,
                    fields,
                } if strv[*dst] => {
                    for (name, src) in fields {
                        if strv[*src] {
                            let slot = layout.field_names.iter().position(|n| **n == **name)?;
                            let a = anchor_of(&mut parent, &mut anchors, *dst, slot);
                            let before = find(&mut parent, a);
                            union(&mut parent, a, *src);
                            if find(&mut parent, a) != before {
                                changed = true;
                            }
                        }
                    }
                }
                RegInstr::GetFieldSlot { dst, base, slot } if strv[*base] && strv[*dst] => {
                    let a = anchor_of(&mut parent, &mut anchors, *base, *slot);
                    let before = find(&mut parent, *dst);
                    union(&mut parent, a, *dst);
                    if find(&mut parent, *dst) != before {
                        changed = true;
                    }
                }
                _ => {}
            }
        }
    }

    // Determine ONE canonical layout shape (field_names) per alias class from its
    // `MakeStruct` defs. All defs in a class must agree on the shape.
    let mut class_shape: HashMap<usize, Vec<Rc<str>>> = HashMap::new();
    let mut class_layout: HashMap<usize, Rc<crate::vm_value::TypeLayout>> = HashMap::new();
    for i in header..exit {
        if let RegInstr::MakeStruct { dst, layout, .. } = &code[i]
            && strv[*dst]
        {
            let root = find(&mut parent, *dst);
            let shape = layout.field_names.clone();
            match class_shape.get(&root) {
                Some(existing) if *existing != shape => return None, // shape mismatch
                Some(_) => {}
                None => {
                    class_shape.insert(root, shape);
                }
            }
            match class_layout.get(&root) {
                Some(previous)
                    if previous.name != layout.name
                        || previous.field_names != layout.field_names =>
                {
                    return None;
                }
                Some(_) => {}
                None => {
                    class_layout.insert(root, Rc::clone(layout));
                }
            }
        }
    }

    // Allocate one fresh LEAF register per SCALAR slot, per alias class. A nested slot
    // owns no register here — it resolves through its anchor to the inner class's leaf
    // registers. `class_slot_reg[(root, slot)]` is the leaf reg for a scalar slot.
    let mut next_reg = parent.len();
    let mut class_slot_reg: HashMap<(usize, usize), usize> = HashMap::new();
    let roots: Vec<usize> = class_shape.keys().copied().collect();
    for root in roots {
        let shape = class_shape.get(&root).cloned().expect("root has shape");
        let nested = nested_slots.get(&shape).cloned().unwrap_or_default();
        for slot in 0..shape.len() {
            if !nested.contains(&slot) {
                let r = next_reg;
                next_reg += 1;
                class_slot_reg.insert((root, slot), r);
            }
        }
    }
    // The leaf register backing `reg`'s scalar field `slot` (resolving Move aliases).
    let scalar_slot_reg = |parent: &mut Vec<usize>,
                           class_slot_reg: &HashMap<(usize, usize), usize>,
                           reg: usize,
                           slot: usize|
     -> usize {
        let root = find(parent, reg);
        *class_slot_reg
            .get(&(root, slot))
            .expect("scalar slot has a leaf register")
    };
    let slot_of_name = |parent: &mut Vec<usize>,
                        class_shape: &HashMap<usize, Vec<Rc<str>>>,
                        reg: usize,
                        name: &str|
     -> usize {
        let root = find(parent, reg);
        class_shape
            .get(&root)
            .and_then(|shape| shape.iter().position(|n| &**n == name))
            .expect("field name in class shape")
    };

    // Pre-flight: every struct op the rewrite will touch must resolve (every STR class
    // has a shape; every SCALAR slot it reads/writes has an allocated leaf register).
    // Bail rather than panic on any unresolved case — conservative REJECT.
    for i in header..exit {
        match &code[i] {
            RegInstr::MakeStruct {
                dst,
                layout,
                fields,
            } if strv[*dst] => {
                let root = find(&mut parent, *dst);
                if class_shape.get(&root) != Some(&layout.field_names) {
                    return None;
                }
                for (name, src) in fields {
                    if strv[*src] {
                        continue;
                    }
                    let slot = layout.field_names.iter().position(|n| **n == **name)?;
                    if !class_slot_reg.contains_key(&(root, slot)) {
                        return None;
                    }
                }
            }
            RegInstr::GetFieldSlot { dst, base, slot } if strv[*base] => {
                let root = find(&mut parent, *base);
                let shape = class_shape.get(&root)?;
                if *slot >= shape.len() {
                    return None;
                }
                if !strv[*dst] && !class_slot_reg.contains_key(&(root, *slot)) {
                    return None;
                }
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_struct_recipe_value(
        reg: usize,
        parent: &mut [usize],
        class_layout: &HashMap<usize, Rc<crate::vm_value::TypeLayout>>,
        nested_slots: &HashMap<Vec<Rc<str>>, std::collections::HashSet<usize>>,
        anchors: &HashMap<(usize, usize), usize>,
        class_slot_reg: &HashMap<(usize, usize), usize>,
        depth: usize,
        nodes: &mut usize,
    ) -> Option<OsrMaterializeValue> {
        if depth >= MAX_OSR_MATERIALIZE_DEPTH || *nodes >= MAX_OSR_MATERIALIZE_NODES {
            return None;
        }
        *nodes += 1;
        let root = find(parent, reg);
        let layout = Rc::clone(class_layout.get(&root)?);
        let nested = nested_slots.get(&layout.field_names);
        let mut fields = Vec::with_capacity(layout.field_names.len());
        for slot in 0..layout.field_names.len() {
            if *nodes >= MAX_OSR_MATERIALIZE_NODES {
                return None;
            }
            if nested.is_some_and(|slots| slots.contains(&slot)) {
                let anchor = *anchors.get(&(root, slot))?;
                fields.push(build_struct_recipe_value(
                    anchor,
                    parent,
                    class_layout,
                    nested_slots,
                    anchors,
                    class_slot_reg,
                    depth + 1,
                    nodes,
                )?);
            } else {
                *nodes += 1;
                fields.push(OsrMaterializeValue::Register(
                    *class_slot_reg.get(&(root, slot))?,
                ));
            }
        }
        Some(OsrMaterializeValue::Struct { layout, fields })
    }

    let mut recipes = Vec::new();
    for (reg, &needs) in reconstruct.iter().enumerate() {
        if needs {
            let mut nodes = 0;
            recipes.push(OsrMaterializeRecipe {
                dst_reg: reg,
                value: build_struct_recipe_value(
                    reg,
                    &mut parent,
                    &class_layout,
                    &nested_slots,
                    &anchors,
                    &class_slot_reg,
                    0,
                    &mut nodes,
                )?,
            });
        }
    }

    // Rewrite the whole code, scalar-replacing in-region struct ops and copying the
    // rest through, remapping jump/match targets through the index map.
    enum Fix {
        Target(usize),
        Match { some_ip: usize, none_ip: usize },
        VariantMatch { match_ip: usize, else_ip: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        let region = in_region(i);
        match instr {
            RegInstr::MakeStruct { dst, fields, .. } if region && strv[*dst] => {
                // For each SCALAR field, move the source into its slot's leaf register.
                // A NESTED field is a struct-typed slot whose per-slot anchor is unioned
                // with the inner struct register's class, so the inner's leaf registers
                // ARE this slot's registers — they were already written by the inner's
                // own (earlier) `MakeStruct`. Emit nothing for it.
                for (name, src) in fields {
                    if strv[*src] {
                        continue; // nested struct field: aliased, no copy needed
                    }
                    let slot = slot_of_name(&mut parent, &class_shape, *dst, name);
                    let leaf = scalar_slot_reg(&mut parent, &class_slot_reg, *dst, slot);
                    new_code.push(RegInstr::Move {
                        dst: leaf,
                        src: *src,
                    });
                }
            }
            RegInstr::GetFieldSlot { dst, base, slot } if region && strv[*base] => {
                if strv[*dst] {
                    // Reading a struct-typed slot: `dst` is unioned with `base`'s slot
                    // anchor, so `dst` already names the inner struct's leaf registers.
                    // A subsequent `GetFieldSlot{base:dst, inner_slot}` reads the inner's
                    // scalar leaf directly — the `a.b.c` chain collapses to register
                    // moves end-to-end. Emit nothing here.
                } else {
                    let leaf = scalar_slot_reg(&mut parent, &class_slot_reg, *base, *slot);
                    new_code.push(RegInstr::Move {
                        dst: *dst,
                        src: leaf,
                    });
                }
            }
            RegInstr::Move { dst, src } if region && strv[*dst] => {
                // Plain struct alias: `dst` and `src` share an alias class ⇒ identical
                // leaf registers ⇒ every per-slot copy is a self-Move. Emit nothing.
                let _ = (dst, src);
            }
            // Copy-through, remapping jump/match targets.
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
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::VariantMatch {
                        match_ip: *match_ip,
                        else_ip: *else_ip,
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
            Fix::VariantMatch { match_ip, else_ip } => {
                let (m, e) = (index_map[match_ip], index_map[else_ip]);
                if let RegInstr::MatchVariant {
                    match_ip: md,
                    else_ip: ed,
                    ..
                } = &mut new_code[pos]
                {
                    *md = m;
                    *ed = e;
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`).
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, next_reg, ip_map, recipes))
}

/// Stable native-JIT builds intentionally leave struct aggregates to the
/// interpreter until this transform meets the canonical retention threshold.
#[cfg(all(
    feature = "native-jit",
    not(any(test, feature = "jit-struct-sr-experimental"))
))]
#[allow(clippy::type_complexity)]
pub(in crate::reg_vm) fn native_scalar_replace_structs_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>, Vec<OsrMaterializeRecipe>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    Some((code.to_vec(), n_regs, (0..code.len()).collect(), Vec::new()))
}

/// OSR × scalar replacement (loop-carried struct scalar replacement). Extends
/// [`native_scalar_replace_structs_in_region`] to a struct that is CREATED BEFORE the
/// loop, MUTATED IN PLACE across iterations (`SetFieldSlot`), and DEAD after the loop.
///
/// The shipped struct pass only dissolves a struct allocated AND dead INSIDE the loop
/// (the `MakeStruct` lives in the region; the dead-at-boundary gate forbids any
/// outside reference). A struct mutated loop-carried fails that pass two ways: its
/// `MakeStruct` sits in the PRE-HEADER (so the strict "every in-region instr is
/// native-subset or `MakeStruct`" check trips on the in-loop `SetFieldSlot`, which the
/// shipped pass does not even model), and the struct register is WRITTEN before the
/// region (so the dead-at-boundary gate bails). The in-loop `SetFieldSlot` is a heap
/// write the native tier cannot perform, so without this pass the loop never OSRs.
///
/// This pass dissolves such a struct into one LOOP-CARRIED scalar register per field:
/// - the pre-header `MakeStruct{m}` + `Move{p, m}` becomes nothing — each field's
///   INIT SOURCE register is REUSED as that field's loop-carried leaf register, so the
///   interpreter has already written it (definite assignment) before the header and it
///   marshals live-in through the normal `try_osr` window with no new channel;
/// - in-loop `GetFieldSlot{dst, base:p, slot}` → `Move dst := leaf_slot` (register read);
/// - in-loop `SetFieldSlot{dst, base:p, slot, value}` → `Move leaf_slot := value`
///   (the heap write becomes a register write) plus `LoadUnit dst` (its old `dst`,
///   which the interpreter set to `Unit`, is preserved in case it is read);
/// - in-loop self-`Move{p, p}` (the lowerer's redundant copy after each `SetFieldSlot`)
///   → nothing.
///
/// The leaf registers are loop-carried (live-in at the header, carried across the
/// backedge, in the OSR window). Because `p` is DEAD after the loop, no heap struct is
/// reconstructed at the OSR-exit; the scalar leaves simply flow out (each leaf is a
/// real register `< n_regs`, restored by the precise deopt — harmless, since dead).
///
/// Conservative bails (when unsure REJECT — never unsound): no in-region
/// `SetFieldSlot` (nothing for THIS pass — return unchanged); the struct ESCAPES (read
/// as a plain value / stored / returned / captured / any non-(Get/Set)FieldSlot,
/// non-alias use); the struct is LIVE after the loop (read at/after `exit` ⇒ would need
/// heap reconstruction ⇒ out of scope); the pre-header def is not a clean
/// `MakeStruct` (single def reachable through `Move` aliases); a field INIT SOURCE
/// register is not REUSABLE as a leaf (shared between two fields, or read/written
/// anywhere other than the defining `MakeStruct` — reuse would corrupt a live value);
/// two struct handles that might alias the same heap object; a footprint we cannot
/// model (`RegFootprint::All`). Every existing loop-LOCAL / nested struct behavior is
/// untouched — this pass only fires when the shipped struct pass could not (an
/// in-region `SetFieldSlot` on a pre-header struct).
///
/// Returns `(transformed_code, new_n_regs, ip_map)` with the same transformed→original
/// `ip_map` discipline as the other region passes; `new_n_regs == n_regs` (no fresh
/// regs — leaves reuse the init sources).
#[cfg(all(
    feature = "native-jit",
    any(test, feature = "jit-struct-sr-experimental")
))]
#[allow(clippy::needless_range_loop)]
pub(in crate::reg_vm) fn native_loop_carried_struct_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    let in_region = |i: usize| i >= header && i < exit;
    let identity = || -> (Vec<RegInstr>, usize, Vec<usize>) {
        (code.to_vec(), n_regs, (0..code.len()).collect())
    };

    // Fast path: no in-region `SetFieldSlot` ⇒ nothing for THIS pass. (A loop-LOCAL
    // struct, or a bare native-subset body, is handled by the earlier passes.)
    if !(header..exit).any(|i| matches!(&code[i], RegInstr::SetFieldSlot { .. })) {
        return Some(identity());
    }
    let analysis = NativeRegionAnalysis::compute_prefix(code, n_regs, header, exit)?;

    // Candidate struct handles: every register that is the `base` of an in-region
    // `GetFieldSlot`/`SetFieldSlot`. We require EXACTLY ONE such base register (a
    // single loop-carried struct); multiple distinct bases (possible aliasing of two
    // heap structs) ⇒ bail. A `base` that is a Move-alias of the same pre-header
    // struct still appears as one register here because the lowering threads the
    // in-place mutation back through that one register (`SetFieldSlot` rewrites its
    // `base` slot, and the self-`Move{p,p}` keeps it that register).
    let mut base_regs: Vec<usize> = Vec::new();
    for i in header..exit {
        match &code[i] {
            RegInstr::GetFieldSlot { base, .. } | RegInstr::SetFieldSlot { base, .. }
                if !base_regs.contains(base) =>
            {
                base_regs.push(*base);
            }
            _ => {}
        }
    }
    if base_regs.len() != 1 {
        return None;
    }
    let p = base_regs[0];
    if p >= n_regs {
        return None;
    }

    // The pre-header definition of `p`: follow `Move{p, src}` aliases backward to a
    // single `MakeStruct`. `p` must have EXACTLY ONE writer outside the region, that
    // writer must be either a `MakeStruct{p,..}` directly or `Move{p, m}` where `m`
    // has exactly one writer, a `MakeStruct{m,..}`, and `m` is used ONLY by that Move.
    // All such defs must precede the region (a pre-header def).
    //
    // `instr_written_reg` reports the literal `dst` field. A struct handle mutated in
    // place by `SetFieldSlot{base:p}` writes `p`'s slot at RUNTIME but its modelled
    // `dst` is the (unused) result reg — so a SetFieldSlot does NOT appear as a writer
    // of `p` here. What DOES is the lowerer's redundant self-`Move{p,p}` emitted after
    // each in-region `SetFieldSlot` (a semantic no-op). Exclude those self-moves so the
    // sole REAL writer of `p` is its pre-header definition.
    let is_self_move = |i: usize| matches!(&code[i], RegInstr::Move { dst, src } if dst == src);
    let p_writers: Vec<usize> = analysis
        .writer_ips_of(code, p)?
        .into_iter()
        .filter(|&i| !(in_region(i) && is_self_move(i)))
        .collect();
    if p_writers.len() != 1 {
        return None;
    }
    let p_def = p_writers[0];
    if in_region(p_def) || p_def >= header {
        return None;
    }
    // Resolve the `MakeStruct` providing `p`'s fields (directly, or through one Move).
    let make_idx = match &code[p_def] {
        RegInstr::MakeStruct { dst, .. } if *dst == p => p_def,
        RegInstr::Move { dst, src } if *dst == p => {
            let m = *src;
            let m_writers = analysis.writer_ips_of(code, m)?;
            if m_writers.len() != 1 {
                return None;
            }
            let mi = m_writers[0];
            if !matches!(&code[mi], RegInstr::MakeStruct { dst, .. } if *dst == m) {
                return None;
            }
            // `m` must be used ONLY by this Move (otherwise the struct also flows
            // elsewhere — an escape we are not modelling).
            for (i, instr) in code.iter().enumerate() {
                if i == mi {
                    continue;
                }
                let reads = match instr_read_regs(instr) {
                    RegFootprint::Some(rs) => rs,
                    RegFootprint::All => return None,
                };
                if reads.contains(&m) && i != p_def {
                    return None;
                }
            }
            mi
        }
        _ => return None,
    };
    if make_idx >= header {
        return None;
    }
    let RegInstr::MakeStruct { layout, fields, .. } = &code[make_idx] else {
        return None;
    };

    // Non-escaping + dead-after-loop check on `p`. EVERY read of `p` in the whole
    // function must be: an in-region `GetFieldSlot`/`SetFieldSlot` base, an in-region
    // self-`Move{p,p}`, or the pre-header `Move{p_def}` source-side... `p` is never a
    // Move SOURCE in the pre-header (it is the dst there). So: every read of `p` must
    // be a (Get/Set)FieldSlot base or a self-Move — both IN-REGION. Any read of `p`
    // OUTSIDE the region (incl. at/after `exit`) ⇒ live-after-loop or escape ⇒ bail.
    for (i, instr) in code.iter().enumerate() {
        let reads = match instr_read_regs(instr) {
            RegFootprint::Some(rs) => rs,
            RegFootprint::All => return None,
        };
        if !reads.contains(&p) {
            continue;
        }
        match instr {
            RegInstr::GetFieldSlot { base, .. } | RegInstr::SetFieldSlot { base, .. }
                if *base == p && in_region(i) => {}
            RegInstr::Move { dst, src } if *dst == p && *src == p && in_region(i) => {}
            _ => return None, // escapes or live after the loop
        }
    }
    // `p` must not be WRITTEN anywhere except its single pre-header def and the
    // in-region redundant self-`Move{p,p}` (deleted by the rewrite). `SetFieldSlot`
    // does not model `p` as a written reg (its `dst` is the unused result). Any OTHER
    // writer of `p` (e.g. a second struct construction on another path) ⇒
    // aliasing/polymorphism ⇒ bail.
    for i in 0..code.len() {
        if i == p_def {
            continue;
        }
        if in_region(i) && is_self_move(i) {
            continue; // redundant self-Move, deleted below
        }
        let writes = match instr_written_reg(&code[i]) {
            RegFootprint::Some(ws) => ws,
            RegFootprint::All => return None,
        };
        if writes.contains(&p) {
            return None;
        }
    }

    // Allocate one loop-carried leaf register per field slot by REUSING the field's
    // init-source register. Reuse is sound iff the source register is used ONLY as
    // that single `MakeStruct` field source (so the native loop owning/overwriting it
    // corrupts nothing) and the sources are pairwise DISTINCT (each leaf is its own
    // register). Build `slot -> leaf` from the layout's declaration order.
    let n_slots = layout.field_names.len();
    let mut slot_leaf: Vec<Option<usize>> = vec![None; n_slots];
    let mut seen_leaves: Vec<usize> = Vec::new();
    for (name, src) in fields.iter() {
        let slot = layout.field_names.iter().position(|n| **n == **name)?;
        if slot >= n_slots {
            return None;
        }
        if slot_leaf[slot].is_some() {
            return None; // duplicate field
        }
        // Distinct across fields.
        if seen_leaves.contains(src) {
            return None;
        }
        // The source must be a real interpreter register that the OSR window can
        // marshal (`< n_regs`).
        if *src >= n_regs {
            return None;
        }
        // The source must be used ONLY by this `MakeStruct` (any other read would be
        // clobbered when the native loop overwrites the leaf). Reads elsewhere ⇒ bail.
        for (i, instr) in code.iter().enumerate() {
            if i == make_idx {
                continue;
            }
            let reads = match instr_read_regs(instr) {
                RegFootprint::Some(rs) => rs,
                RegFootprint::All => return None,
            };
            if reads.contains(src) {
                return None;
            }
        }
        // The source must have a single writer (its pre-header init), and that writer
        // must precede the region. (It feeds the live-in marshalling.)
        let src_writers = analysis.writer_ips_of(code, *src)?;
        if src_writers.len() != 1 || src_writers[0] >= header {
            return None;
        }
        slot_leaf[slot] = Some(*src);
        seen_leaves.push(*src);
    }
    if slot_leaf.iter().any(|s| s.is_none()) {
        return None; // a slot had no field source
    }
    let slot_leaf: Vec<usize> = slot_leaf.into_iter().map(|s| s.expect("checked")).collect();

    // Each in-region `SetFieldSlot{dst}` writes `dst := Unit` at runtime (an unused
    // result of the in-place mutation). The rewrite turns the SetFieldSlot into a plain
    // `Move leaf := value` and emits NOTHING for the (dead) `dst`. Require every such
    // `dst` to be DEAD (never read anywhere in the function) so dropping its `Unit`
    // write is observationally invisible; a read of it would need a `LoadUnit` (not in
    // the native subset) ⇒ bail instead.
    for i in header..exit {
        if let RegInstr::SetFieldSlot { dst, base, .. } = &code[i] {
            if *base != p {
                continue;
            }
            for (j, instr) in code.iter().enumerate() {
                let reads = match instr_read_regs(instr) {
                    RegFootprint::Some(rs) => rs,
                    RegFootprint::All => return None,
                };
                if j != i && reads.contains(dst) {
                    return None;
                }
            }
        }
    }

    // Rewrite. The pre-header `MakeStruct` (and its forwarding `Move{p, m}`) are
    // DELETED — the field sources already hold the init values as the leaf registers.
    // In-region field ops become register Moves. Everything else copies through with
    // jump/match targets remapped via the index map.
    enum Fix {
        Target(usize),
        Match { some_ip: usize, none_ip: usize },
        VariantMatch { match_ip: usize, else_ip: usize },
    }
    let mut new_code: Vec<RegInstr> = Vec::with_capacity(code.len());
    let mut index_map = vec![0usize; code.len()];
    let mut fixups: Vec<(usize, Fix)> = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        index_map[i] = new_code.len();
        match instr {
            // Pre-header struct construction + its forwarding Move: delete (leaves are
            // the init-source registers, already written by the interpreter).
            RegInstr::MakeStruct { dst, .. } if i == make_idx && *dst == p => {}
            RegInstr::MakeStruct { dst, .. } if i == make_idx => {
                // The `MakeStruct{m}` whose result a `Move{p,m}` forwards: delete it;
                // its dst `m` is otherwise unused (validated above).
                let _ = dst;
            }
            RegInstr::Move { dst, src } if i == p_def && *dst == p && *src != p => {
                // The forwarding `Move{p, m}`: delete (m is gone).
            }
            // In-region field reads/writes on `p`.
            RegInstr::GetFieldSlot { dst, base, slot } if in_region(i) && *base == p => {
                let leaf = *slot_leaf.get(*slot)?;
                new_code.push(RegInstr::Move {
                    dst: *dst,
                    src: leaf,
                });
            }
            RegInstr::SetFieldSlot {
                base, slot, value, ..
            } if in_region(i) && *base == p => {
                let leaf = *slot_leaf.get(*slot)?;
                // The heap write becomes a register write into the loop-carried leaf.
                // The SetFieldSlot's `dst` (the unused `Unit` result) is validated dead
                // above, so emit nothing for it.
                new_code.push(RegInstr::Move {
                    dst: leaf,
                    src: *value,
                });
            }
            // The lowerer's redundant self-`Move{p,p}` after each SetFieldSlot.
            RegInstr::Move { dst, src } if in_region(i) && *dst == p && *src == p => {}
            // Copy-through, remapping jump/match targets.
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
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                fixups.push((
                    new_code.len(),
                    Fix::VariantMatch {
                        match_ip: *match_ip,
                        else_ip: *else_ip,
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
            Fix::VariantMatch { match_ip, else_ip } => {
                let (m, e) = (index_map[match_ip], index_map[else_ip]);
                if let RegInstr::MatchVariant {
                    match_ip: md,
                    else_ip: ed,
                    ..
                } = &mut new_code[pos]
                {
                    *md = m;
                    *ed = e;
                }
            }
        }
    }
    // Inverse ip-map (see `native_scalar_replace_options`). A deleted pre-header
    // instruction maps its (empty) range to the next emitted index; the OSR boundary
    // (header/exit) is in-region/post-region control flow, never a deleted pre-header
    // index, so the boundary mapping stays unambiguous.
    let mut ip_map = vec![0usize; new_code.len()];
    for i in 0..code.len() {
        let start = index_map[i];
        let end = if i + 1 < code.len() {
            index_map[i + 1]
        } else {
            new_code.len()
        };
        for t in start..end {
            ip_map[t] = i;
        }
    }
    Some((new_code, n_regs, ip_map))
}

/// Stable native-JIT builds leave loop-carried structs unchanged so native
/// eligibility fails closed and the verified interpreter remains authoritative.
#[cfg(all(
    feature = "native-jit",
    not(any(test, feature = "jit-struct-sr-experimental"))
))]
pub(in crate::reg_vm) fn native_loop_carried_struct_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    if header >= exit || exit > code.len() {
        return None;
    }
    Some((code.to_vec(), n_regs, (0..code.len()).collect()))
}

/// Registers an instruction READS (value operands). Returns [`RegFootprint::All`]
/// for any variant whose read set we do not exhaustively model. Used by OSR × scalar replacement to
/// prove a scalar-replaced Option register is dead at the loop boundary.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn instr_read_regs(instr: &RegInstr) -> RegFootprint {
    native_instr_semantics(instr).reads
}

/// The register an instruction WRITES (its `dst`), or [`RegFootprint::All`] for a
/// variant we do not model (treated as writing every register — sound).
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn instr_written_reg(instr: &RegInstr) -> RegFootprint {
    native_instr_semantics(instr).writes
}

/// Fuse a heap field read that is used only as the closure operand for native
/// closure metadata reads:
///
/// ```text
/// f  = GetFieldSlot(op, slot)      // lowers to FieldHandle
/// id = NativeClosureId(f)
/// c0 = NativeClosureCapture(f, 0)
/// ```
///
/// becomes:
///
/// ```text
/// f  = Move(op)                    // dead handle-shaped placeholder
/// id = NativeFieldClosureId(op, slot)
/// c0 = NativeFieldClosureCapture(op, slot, 0)
/// ```
///
/// Keeping the instruction count stable preserves all branch/ip maps. The dummy
/// `Move` writes the old destination register without performing a host helper call;
/// the pass only fires when every read of that register is rewritten.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_fuse_field_closure_metadata_reads(
    code: &[RegInstr],
    n_regs: usize,
) -> Option<(Vec<RegInstr>, usize, Vec<usize>)> {
    let mut field_defs: Vec<Option<(usize, usize, usize)>> = vec![None; n_regs];
    let mut invalid = vec![false; n_regs];

    for (ip, instr) in code.iter().enumerate() {
        // A field-closure def captures its base register's value at def time, so a
        // later read fused into `GetFieldSlot(base, slot)` is only sound while `base`
        // still holds that value. If `base` is clobbered between the def and the read,
        // invalidate every def that captured it — otherwise the fused read resolves
        // against the new base and dispatches the wrong closure.
        match instr_written_reg(instr) {
            RegFootprint::Some(writes) => {
                for w in writes {
                    if w < n_regs {
                        for def in field_defs.iter_mut() {
                            if def.is_some_and(|(base, _, _)| base == w) {
                                *def = None;
                            }
                        }
                    }
                }
            }
            RegFootprint::All => {
                field_defs.fill(None);
                invalid.fill(true);
            }
        }
        match instr {
            RegInstr::GetFieldSlot { dst, base, slot } if *dst < n_regs && !invalid[*dst] => {
                if field_defs[*dst].is_none() {
                    field_defs[*dst] = Some((*base, *slot, ip));
                } else {
                    field_defs[*dst] = None;
                    invalid[*dst] = true;
                }
            }
            RegInstr::Move { dst, src }
                if *dst < n_regs
                    && *src < n_regs
                    && field_defs[*src].is_some()
                    && !invalid[*dst] =>
            {
                if field_defs[*dst].is_none() {
                    field_defs[*dst] = field_defs[*src];
                } else {
                    field_defs[*dst] = None;
                    invalid[*dst] = true;
                }
            }
            _ => match instr_written_reg(instr) {
                RegFootprint::Some(writes) => {
                    for dst in writes {
                        if dst < n_regs {
                            field_defs[dst] = None;
                            invalid[dst] = true;
                        }
                    }
                }
                RegFootprint::All => {
                    field_defs.fill(None);
                    invalid.fill(true);
                }
            },
        }
    }

    let mut reads = vec![0usize; n_regs];
    let mut rewritable_reads = vec![0usize; n_regs];
    for (ip, instr) in code.iter().enumerate() {
        match instr {
            RegInstr::NativeClosureId { closure, .. }
            | RegInstr::NativeClosureCapture { closure, .. }
                if *closure < n_regs && field_defs[*closure].is_some() =>
            {
                reads[*closure] += 1;
                if field_defs[*closure].is_some_and(|(_, _, def_ip)| def_ip < ip) {
                    rewritable_reads[*closure] += 1;
                }
            }
            RegInstr::Move { dst, src }
                if *dst < n_regs
                    && *src < n_regs
                    && field_defs[*src].is_some()
                    && field_defs[*dst] == field_defs[*src] =>
            {
                reads[*src] += 1;
                if field_defs[*src].is_some_and(|(_, _, def_ip)| def_ip < ip) {
                    rewritable_reads[*src] += 1;
                }
            }
            _ => match instr_read_regs(instr) {
                RegFootprint::Some(regs) => {
                    for reg in regs {
                        if reg < n_regs {
                            reads[reg] += 1;
                        }
                    }
                }
                RegFootprint::All => {
                    for (reg, def) in field_defs.iter().enumerate() {
                        if def.is_some() {
                            reads[reg] += 1;
                        }
                    }
                }
            },
        }
    }

    let can_rewrite = |reg: usize| {
        let Some(def) = (reg < n_regs).then(|| field_defs[reg]).flatten() else {
            return false;
        };
        let mut group_has_read = false;
        for (alias, alias_def) in field_defs.iter().enumerate() {
            if *alias_def == Some(def) {
                group_has_read |= reads[alias] > 0;
                if reads[alias] != rewritable_reads[alias] {
                    return false;
                }
            }
        }
        reg < n_regs && group_has_read && reads[reg] == rewritable_reads[reg]
    };

    let mut changed = false;
    let mut out = Vec::with_capacity(code.len());
    for instr in code {
        let rewritten = match instr {
            RegInstr::GetFieldSlot { dst, base, .. } if can_rewrite(*dst) => {
                changed = true;
                RegInstr::Move {
                    dst: *dst,
                    src: *base,
                }
            }
            RegInstr::Move { dst, src }
                if *dst < n_regs
                    && *src < n_regs
                    && field_defs[*dst] == field_defs[*src]
                    && can_rewrite(*src) =>
            {
                let (base, _, _) = field_defs[*src]?;
                changed = true;
                RegInstr::Move {
                    dst: *dst,
                    src: base,
                }
            }
            RegInstr::NativeClosureId { dst, closure } if can_rewrite(*closure) => {
                let (base, slot, _) = field_defs[*closure]?;
                changed = true;
                RegInstr::NativeFieldClosureId {
                    dst: *dst,
                    base,
                    slot,
                }
            }
            RegInstr::NativeClosureCapture {
                dst,
                closure,
                index,
            } if can_rewrite(*closure) => {
                let (base, slot, _) = field_defs[*closure]?;
                changed = true;
                RegInstr::NativeFieldClosureCapture {
                    dst: *dst,
                    base,
                    slot,
                    index: *index,
                }
            }
            other => other.clone(),
        };
        out.push(rewritten);
    }

    Some(if changed {
        (out, n_regs, (0..code.len()).collect())
    } else {
        (code.to_vec(), n_regs, (0..code.len()).collect())
    })
}
