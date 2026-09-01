use super::*;

type RegionRewrite<Recipe> = (Vec<RegInstr>, usize, Vec<usize>, Vec<Recipe>);

/// Two-armed scalar `Result` dissolution.
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
pub(super) fn native_scalar_replace_two_armed_results_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
    res: &[bool],
) -> Option<RegionRewrite<ResultRecipe>> {
    let in_region = |i: usize| i >= header && i < exit;

    // Validate every in-region def/use of a RES register: defs are `Ok`/`Err`
    // constructors with one scalar (non-RES) field, or a `Move` from another RES; uses
    // are `MatchResult`/`UnwrapVariantValue`. `?` (`TryResult`) and any other touch bail.
    for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(0..code.len()) {
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
pub(in crate::reg_vm) fn native_scalar_replace_variants_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<RegionRewrite<OsrMaterializeRecipe>> {
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
    for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(header..exit) {
        if !native_subset_instruction(&code[i]) && !is_variant_op(&code[i]) && !is_var_getfield(i) {
            return None;
        }
    }

    // Validate in-region uses/defs of VAR registers. Each `MakeVariant` arm carries
    // only scalar fields (no field may itself be a VAR register — that would be a
    // nested variant, out of scope ⇒ bail). Anything else touching a VAR register that
    // is not a recognized consumer ⇒ bail.
    for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(0..code.len()) {
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
    for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(header..exit) {
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
            for slot in parallel_indices(0..fields.len()) {
                leaf_reg.insert((*root, arm.clone(), slot), next_reg);
                next_reg += 1;
            }
        }
    }
    for reg in parallel_indices(0..n_regs) {
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
    for i in parallel_indices(header..exit) {
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
