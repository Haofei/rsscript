//! Nested and loop-carried struct scalar replacement (OSR region passes),
//! split out of `scalar_replacement.rs` for module-size partitioning. These
//! passes are `#[cfg(test)]` regression scaffolding on the supported
//! `native-jit` path; the non-test fallbacks leave aggregates unchanged and
//! fall closed to the verified interpreter (see the native-jit contract).

use super::*;

// Private region-rewrite alias, matching the sibling pass modules
// (`region_optimization.rs`, `scalar_replacement.rs`) which each keep their own.
type RegionRewrite<Recipe> = (Vec<RegInstr>, usize, Vec<usize>, Vec<Recipe>);

/// OSR × scalar replacement for non-escaping FLAT user STRUCTS. Mirrors
/// Resolve the declared layout shape (`field_names`) of a struct-valued register by
/// walking its in-region definitions. A register defined by `MakeStruct` carries the
/// shape directly; a `Move` forwards its source's shape; a `GetFieldSlot{dst, base,
/// slot}` reading a struct-typed field has the shape of whatever `MakeStruct` wrote
/// that slot of `base` (i.e. the inner field's own struct shape). Returns `None` when
/// the shape is ambiguous or not statically resolvable (⇒ the caller bails OSR).
#[cfg(all(feature = "native-jit", test))]
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
        for i in parallel_indices(header..exit) {
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
        for i in parallel_indices(header..exit) {
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
#[cfg(all(feature = "native-jit", test))]
pub(in crate::reg_vm) fn native_scalar_replace_structs_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<RegionRewrite<OsrMaterializeRecipe>> {
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
    for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(header..exit) {
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
        for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(0..code.len()) {
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
    for i in parallel_indices(header..exit) {
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
        for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(header..exit) {
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
        for slot in parallel_indices(0..shape.len()) {
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
    for i in parallel_indices(header..exit) {
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

    struct StructRecipeContext<'a> {
        class_layout: &'a HashMap<usize, Rc<crate::vm_value::TypeLayout>>,
        nested_slots: &'a HashMap<Vec<Rc<str>>, std::collections::HashSet<usize>>,
        anchors: &'a HashMap<(usize, usize), usize>,
        class_slot_reg: &'a HashMap<(usize, usize), usize>,
        nodes: usize,
    }

    fn build_struct_recipe_value(
        reg: usize,
        parent: &mut [usize],
        depth: usize,
        context: &mut StructRecipeContext<'_>,
    ) -> Option<OsrMaterializeValue> {
        if depth >= MAX_OSR_MATERIALIZE_DEPTH || context.nodes >= MAX_OSR_MATERIALIZE_NODES {
            return None;
        }
        context.nodes += 1;
        let root = find(parent, reg);
        let layout = Rc::clone(context.class_layout.get(&root)?);
        let nested = context.nested_slots.get(&layout.field_names);
        let mut fields = Vec::with_capacity(layout.field_names.len());
        for slot in parallel_indices(0..layout.field_names.len()) {
            if context.nodes >= MAX_OSR_MATERIALIZE_NODES {
                return None;
            }
            if nested.is_some_and(|slots| slots.contains(&slot)) {
                let anchor = *context.anchors.get(&(root, slot))?;
                fields.push(build_struct_recipe_value(
                    anchor,
                    parent,
                    depth + 1,
                    context,
                )?);
            } else {
                context.nodes += 1;
                fields.push(OsrMaterializeValue::Register(
                    *context.class_slot_reg.get(&(root, slot))?,
                ));
            }
        }
        Some(OsrMaterializeValue::Struct { layout, fields })
    }

    let mut recipes = Vec::new();
    for (reg, &needs) in reconstruct.iter().enumerate() {
        if needs {
            let mut context = StructRecipeContext {
                class_layout: &class_layout,
                nested_slots: &nested_slots,
                anchors: &anchors,
                class_slot_reg: &class_slot_reg,
                nodes: 0,
            };
            recipes.push(OsrMaterializeRecipe {
                dst_reg: reg,
                value: build_struct_recipe_value(reg, &mut parent, 0, &mut context)?,
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

/// Stable native-JIT builds intentionally leave struct aggregates to the
/// interpreter until this transform meets the canonical retention threshold.
#[cfg(all(feature = "native-jit", not(test)))]
pub(in crate::reg_vm) fn native_scalar_replace_structs_in_region(
    code: &[RegInstr],
    n_regs: usize,
    header: usize,
    exit: usize,
) -> Option<RegionRewrite<OsrMaterializeRecipe>> {
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
#[cfg(all(feature = "native-jit", test))]
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
    for i in parallel_indices(header..exit) {
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
    for i in parallel_indices(0..code.len()) {
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
    for i in parallel_indices(header..exit) {
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
    Some((new_code, n_regs, ip_map))
}

/// Stable native-JIT builds leave loop-carried structs unchanged so native
/// eligibility fails closed and the verified interpreter remains authoritative.
#[cfg(all(feature = "native-jit", not(test)))]
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
