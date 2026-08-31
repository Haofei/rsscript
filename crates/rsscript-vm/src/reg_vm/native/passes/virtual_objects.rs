//! Shared virtual-object, escape, and materialization facts.
//!
//! Existing Option/Result/Variant scalar replacement keeps its proven rewrite
//! behavior. This module supplies the common conservative vocabulary and owns
//! the shared exit materialization tree so later passes do not grow another
//! aggregate-specific escape model.

use super::*;

/// Maximum constructor roots plus included `Move` edges that the virtual
/// alias analysis will propagate for one region.
///
/// Continuation regions are currently capped well below this value. Keeping a
/// separate limit here makes the analysis fail closed if that surrounding
/// policy changes, and prevents a future caller from turning alias discovery
/// into unbounded compile work.
const MAX_VIRTUAL_ALIAS_WORK_UNITS: usize = 8_192;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::reg_vm) struct VirtualObjectId(u32);

#[derive(Clone, Debug)]
pub(in crate::reg_vm) enum VirtualObjectKind {
    Option,
    Result,
    Variant(Rc<crate::vm_value::TypeLayout>),
    Struct(Rc<crate::vm_value::TypeLayout>),
    Closure { function: usize },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum VirtualFieldValue {
    Register(Reg),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::reg_vm) enum VirtualEscapeClass {
    NoEscape,
    ExitOnly,
    Escapes,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum VirtualMaterializationPlan {
    None,
    AtExit { destinations: Box<[Reg]> },
    DynamicState,
    RequiredAt { instruction: usize },
    Unavailable,
}

#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct VirtualObject {
    pub(in crate::reg_vm) id: VirtualObjectId,
    pub(in crate::reg_vm) root_reg: Reg,
    pub(in crate::reg_vm) aliases: Box<[Reg]>,
    pub(in crate::reg_vm) kind: VirtualObjectKind,
    pub(in crate::reg_vm) fields: Box<[VirtualFieldValue]>,
    pub(in crate::reg_vm) escape: VirtualEscapeClass,
    pub(in crate::reg_vm) materialization: VirtualMaterializationPlan,
}

#[derive(Clone, Debug, Default)]
pub(in crate::reg_vm) struct VirtualObjectAnalysis {
    objects: Box<[VirtualObject]>,
    object_by_reg: Box<[Option<VirtualObjectId>]>,
}

impl VirtualObjectAnalysis {
    pub(in crate::reg_vm) fn derive_ir(
        function: &RegFunction,
        typed: &TypedRegionIr,
    ) -> Option<Self> {
        let mut constructors = Vec::new();
        for instruction in typed
            .blocks()
            .iter()
            .flat_map(|block| block.instructions.iter())
        {
            if let Some(constructor) = typed_virtual_constructor(typed.typed(), instruction)? {
                constructors.push(constructor);
            }
        }
        Self::derive_with_constructors(function, typed.typed(), constructors)
    }

    /// Analyze virtual aggregate candidates in one typed region.
    ///
    /// This is intentionally conservative and register based. Conflicting
    /// definitions, incomplete footprints, or unrecognized consumers classify
    /// an object as escaping/unknown; no caller may turn that into an allocation
    /// elimination. The analysis itself performs no rewrite.
    #[cfg(test)]
    pub(in crate::reg_vm) fn derive(function: &RegFunction, typed: &TypedRegion) -> Option<Self> {
        let constructors = function
            .code
            .iter()
            .enumerate()
            .filter(|(ip, _)| typed.contains_instruction(*ip))
            .filter_map(|(_, instruction)| virtual_constructor(instruction))
            .collect::<Vec<_>>();
        Self::derive_with_constructors(function, typed, constructors)
    }

    fn derive_with_constructors(
        function: &RegFunction,
        typed: &TypedRegion,
        constructors: Vec<(Reg, VirtualObjectKind, Vec<VirtualFieldValue>)>,
    ) -> Option<Self> {
        let mut builders: Vec<VirtualBuilder> = Vec::new();
        let mut object_by_reg: Vec<Option<VirtualObjectId>> = vec![None; function.regs];

        for (dst, kind, fields) in constructors {
            if dst >= function.regs {
                return None;
            }
            match object_by_reg[dst] {
                Some(id) => {
                    let builder = builders.get_mut(id.0 as usize)?;
                    builder.definitions = builder.definitions.saturating_add(1);
                    if !same_virtual_kind(&builder.kind, &kind) || builder.fields != fields {
                        builder.escape = VirtualEscapeClass::Unknown;
                        builder.dynamic_state = true;
                    }
                }
                None => {
                    let id = VirtualObjectId(builders.len().try_into().ok()?);
                    object_by_reg[dst] = Some(id);
                    builders.push(VirtualBuilder {
                        id,
                        root_reg: dst,
                        aliases: vec![dst],
                        kind,
                        fields,
                        definitions: 1,
                        escape: VirtualEscapeClass::NoEscape,
                        exit_regs: BTreeSet::new(),
                        first_escape: None,
                        dynamic_state: false,
                    });
                }
            }
        }

        // Move is the sole bytecode-v1 alias fact used here. Propagate its
        // dependency graph once instead of repeatedly rescanning every move to
        // a fixed point. An overwrite that merges distinct virtual identities
        // is unknown and disables elimination for both identities.
        let moves = function
            .code
            .iter()
            .enumerate()
            .filter_map(|(ip, instruction)| {
                typed
                    .contains_instruction(ip)
                    .then_some(instruction)
                    .and_then(|instruction| match instruction {
                        RegInstr::Move { dst, src } => Some((*src, *dst)),
                        _ => None,
                    })
            })
            .collect::<Vec<_>>();
        propagate_virtual_move_aliases(
            &moves,
            &mut object_by_reg,
            &mut builders,
            MAX_VIRTUAL_ALIAS_WORK_UNITS,
        )?;

        for (ip, instruction) in function.code.iter().enumerate() {
            let reads = match instr_read_regs(instruction) {
                RegFootprint::Some(reads) => reads,
                RegFootprint::All => {
                    for builder in &mut builders {
                        builder.note_escape(ip, VirtualEscapeClass::Unknown);
                    }
                    continue;
                }
            };
            for reg in reads {
                let Some(id) = object_by_reg.get(reg).copied().flatten() else {
                    continue;
                };
                let builder = &mut builders[id.0 as usize];
                if !typed.contains_instruction(ip) {
                    builder.exit_regs.insert(reg);
                    builder.escape = builder.escape.max(VirtualEscapeClass::ExitOnly);
                } else if !recognized_virtual_use(instruction, reg, &builder.kind, &object_by_reg) {
                    builder.note_escape(ip, VirtualEscapeClass::Escapes);
                }
            }
        }

        let objects = builders
            .into_iter()
            .map(|mut builder| {
                builder.aliases.sort_unstable();
                builder.aliases.dedup();
                builder.dynamic_state |= builder.definitions > 1;
                let materialization = if builder.escape == VirtualEscapeClass::Unknown {
                    VirtualMaterializationPlan::Unavailable
                } else if let Some(instruction) = builder.first_escape {
                    VirtualMaterializationPlan::RequiredAt { instruction }
                } else if builder.dynamic_state {
                    VirtualMaterializationPlan::DynamicState
                } else if !builder.exit_regs.is_empty() {
                    VirtualMaterializationPlan::AtExit {
                        destinations: builder.exit_regs.into_iter().collect(),
                    }
                } else {
                    VirtualMaterializationPlan::None
                };
                VirtualObject {
                    id: builder.id,
                    root_reg: builder.root_reg,
                    aliases: builder.aliases.into_boxed_slice(),
                    kind: builder.kind,
                    fields: builder.fields.into_boxed_slice(),
                    escape: builder.escape,
                    materialization,
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Some(Self {
            objects,
            object_by_reg: object_by_reg.into_boxed_slice(),
        })
    }

    #[cfg(test)]
    pub(in crate::reg_vm) fn object_for_reg(&self, reg: Reg) -> Option<&VirtualObject> {
        let id = self.object_by_reg.get(reg).copied().flatten()?;
        self.objects.get(id.0 as usize)
    }

    #[cfg(test)]
    pub(in crate::reg_vm) fn safely_virtual(&self, id: VirtualObjectId) -> bool {
        self.objects.get(id.0 as usize).is_some_and(|object| {
            matches!(
                object.escape,
                VirtualEscapeClass::NoEscape | VirtualEscapeClass::ExitOnly
            ) && !matches!(
                object.materialization,
                VirtualMaterializationPlan::Unavailable
                    | VirtualMaterializationPlan::RequiredAt { .. }
            )
        })
    }

    pub(in crate::reg_vm) fn is_well_formed(
        &self,
        function: &RegFunction,
        typed: &TypedRegion,
    ) -> bool {
        self.objects.iter().enumerate().all(|(index, object)| {
            object.id.0 as usize == index
                && object.root_reg < function.regs
                && object.aliases.contains(&object.root_reg)
                && object.aliases.iter().all(|reg| {
                    self.object_by_reg.get(*reg).copied().flatten() == Some(object.id)
                        && typed.value(*reg).is_some()
                })
                && object.fields.iter().all(|field| match field {
                    VirtualFieldValue::Register(reg) => typed.value(*reg).is_some(),
                })
                && materialization_matches_escape(object.escape, &object.materialization)
                && match &object.kind {
                    VirtualObjectKind::Option | VirtualObjectKind::Result => true,
                    VirtualObjectKind::Variant(layout) | VirtualObjectKind::Struct(layout) => {
                        !layout.name.is_empty()
                    }
                    VirtualObjectKind::Closure { .. } => true,
                }
        })
    }

    pub(in crate::reg_vm) fn summary(&self) -> VirtualObjectSummary {
        let mut summary = VirtualObjectSummary::default();
        for object in &self.objects {
            match &object.kind {
                VirtualObjectKind::Option => summary.options = summary.options.saturating_add(1),
                VirtualObjectKind::Result => summary.results = summary.results.saturating_add(1),
                VirtualObjectKind::Variant(_) => {
                    summary.variants = summary.variants.saturating_add(1)
                }
                VirtualObjectKind::Struct(_) => summary.structs = summary.structs.saturating_add(1),
                VirtualObjectKind::Closure { .. } => {
                    summary.closures = summary.closures.saturating_add(1)
                }
            }
            match object.escape {
                VirtualEscapeClass::NoEscape => {
                    summary.no_escape = summary.no_escape.saturating_add(1)
                }
                VirtualEscapeClass::ExitOnly => {
                    summary.exit_only = summary.exit_only.saturating_add(1)
                }
                VirtualEscapeClass::Escapes | VirtualEscapeClass::Unknown => {
                    summary.declined = summary.declined.saturating_add(1)
                }
            }
        }
        summary
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::reg_vm) struct VirtualObjectSummary {
    pub(in crate::reg_vm) options: usize,
    pub(in crate::reg_vm) results: usize,
    pub(in crate::reg_vm) variants: usize,
    pub(in crate::reg_vm) structs: usize,
    pub(in crate::reg_vm) closures: usize,
    pub(in crate::reg_vm) no_escape: usize,
    pub(in crate::reg_vm) exit_only: usize,
    pub(in crate::reg_vm) declined: usize,
}

/// Propagate the included move graph in bounded `O(registers + moves)` work.
///
/// A register is queued only when it first acquires a virtual identity, so each
/// outgoing edge is visited once. The caller provides the work limit to make
/// the bound directly testable without constructing oversized VM functions.
fn propagate_virtual_move_aliases(
    moves: &[(Reg, Reg)],
    object_by_reg: &mut [Option<VirtualObjectId>],
    builders: &mut [VirtualBuilder],
    max_work_units: usize,
) -> Option<()> {
    let required_work = builders.len().checked_add(moves.len())?;
    if required_work > max_work_units {
        return None;
    }

    let mut outgoing = vec![Vec::<Reg>::new(); object_by_reg.len()];
    for &(src, dst) in moves {
        if src >= object_by_reg.len() || dst >= object_by_reg.len() {
            return None;
        }
        outgoing[src].push(dst);
    }

    // Builder IDs and roots are allocated in deterministic instruction order.
    let mut pending = std::collections::VecDeque::with_capacity(builders.len());
    for builder in builders.iter() {
        if builder.root_reg >= object_by_reg.len()
            || object_by_reg[builder.root_reg] != Some(builder.id)
        {
            return None;
        }
        pending.push_back(builder.root_reg);
    }

    let mut visited_edges = 0usize;
    while let Some(src) = pending.pop_front() {
        let source = object_by_reg.get(src).copied().flatten()?;
        for &dst in &outgoing[src] {
            visited_edges = visited_edges.checked_add(1)?;
            if builders.len().checked_add(visited_edges)? > max_work_units {
                return None;
            }
            match object_by_reg[dst] {
                None => {
                    object_by_reg[dst] = Some(source);
                    builders.get_mut(source.0 as usize)?.aliases.push(dst);
                    pending.push_back(dst);
                }
                Some(destination) if source != destination => {
                    builders.get_mut(source.0 as usize)?.escape = VirtualEscapeClass::Unknown;
                    builders.get_mut(destination.0 as usize)?.escape = VirtualEscapeClass::Unknown;
                }
                Some(_) => {}
            }
        }
    }
    Some(())
}

fn materialization_matches_escape(
    escape: VirtualEscapeClass,
    materialization: &VirtualMaterializationPlan,
) -> bool {
    match escape {
        VirtualEscapeClass::NoEscape => matches!(
            materialization,
            VirtualMaterializationPlan::None | VirtualMaterializationPlan::DynamicState
        ),
        VirtualEscapeClass::ExitOnly => matches!(
            materialization,
            VirtualMaterializationPlan::AtExit { .. } | VirtualMaterializationPlan::DynamicState
        ),
        VirtualEscapeClass::Escapes => {
            matches!(
                materialization,
                VirtualMaterializationPlan::RequiredAt { .. }
            )
        }
        VirtualEscapeClass::Unknown => {
            matches!(materialization, VirtualMaterializationPlan::Unavailable)
        }
    }
}

#[derive(Clone, Debug)]
struct VirtualBuilder {
    id: VirtualObjectId,
    root_reg: Reg,
    aliases: Vec<Reg>,
    kind: VirtualObjectKind,
    fields: Vec<VirtualFieldValue>,
    definitions: usize,
    escape: VirtualEscapeClass,
    exit_regs: BTreeSet<Reg>,
    first_escape: Option<usize>,
    dynamic_state: bool,
}

impl VirtualBuilder {
    fn note_escape(&mut self, instruction: usize, class: VirtualEscapeClass) {
        self.escape = self.escape.max(class);
        self.first_escape = Some(
            self.first_escape
                .map_or(instruction, |old| old.min(instruction)),
        );
    }
}

fn virtual_constructor(
    instruction: &RegInstr,
) -> Option<(Reg, VirtualObjectKind, Vec<VirtualFieldValue>)> {
    match instruction {
        RegInstr::LoadNone { dst } => Some((*dst, VirtualObjectKind::Option, Vec::new())),
        RegInstr::MakeSome { dst, value } => Some((
            *dst,
            VirtualObjectKind::Option,
            vec![VirtualFieldValue::Register(*value)],
        )),
        RegInstr::MakeVariant {
            dst,
            layout,
            fields,
        } => Some((
            *dst,
            if matches!(layout.name.as_ref(), "Ok" | "Err") {
                VirtualObjectKind::Result
            } else {
                VirtualObjectKind::Variant(Rc::clone(layout))
            },
            fields
                .iter()
                .map(|(_, reg)| VirtualFieldValue::Register(*reg))
                .collect(),
        )),
        RegInstr::MakeStruct {
            dst,
            layout,
            fields,
        } => Some((
            *dst,
            VirtualObjectKind::Struct(Rc::clone(layout)),
            fields
                .iter()
                .map(|(_, reg)| VirtualFieldValue::Register(*reg))
                .collect(),
        )),
        RegInstr::MakeClosure {
            dst,
            function,
            captures,
        } => Some((
            *dst,
            VirtualObjectKind::Closure {
                function: *function,
            },
            captures
                .iter()
                .map(|reg| VirtualFieldValue::Register(*reg))
                .collect(),
        )),
        _ => None,
    }
}

fn typed_virtual_constructor(
    typed: &TypedRegion,
    instruction: &TypedRegionInstruction,
) -> Option<Option<(Reg, VirtualObjectKind, Vec<VirtualFieldValue>)>> {
    let TypedRegionOp::Aggregate {
        result,
        kind,
        fields,
    } = &instruction.op
    else {
        return Some(None);
    };
    let kind = match kind {
        TypedAggregateKind::OptionNone | TypedAggregateKind::OptionSome => {
            VirtualObjectKind::Option
        }
        TypedAggregateKind::Result(_) => VirtualObjectKind::Result,
        TypedAggregateKind::Variant(layout) => VirtualObjectKind::Variant(Rc::clone(layout)),
        TypedAggregateKind::Struct(layout) => VirtualObjectKind::Struct(Rc::clone(layout)),
        TypedAggregateKind::Closure { function } => VirtualObjectKind::Closure {
            function: *function,
        },
        TypedAggregateKind::List | TypedAggregateKind::Map | TypedAggregateKind::Object => {
            return Some(None);
        }
    };
    Some(Some((
        typed.reg_for_value(*result)?,
        kind,
        fields
            .iter()
            .map(|field| typed.reg_for_value(*field).map(VirtualFieldValue::Register))
            .collect::<Option<Vec<_>>>()?,
    )))
}

fn same_virtual_kind(left: &VirtualObjectKind, right: &VirtualObjectKind) -> bool {
    match (left, right) {
        (VirtualObjectKind::Option, VirtualObjectKind::Option)
        | (VirtualObjectKind::Result, VirtualObjectKind::Result) => true,
        (VirtualObjectKind::Variant(left), VirtualObjectKind::Variant(right))
        | (VirtualObjectKind::Struct(left), VirtualObjectKind::Struct(right)) => {
            left.name == right.name && left.field_names == right.field_names
        }
        (
            VirtualObjectKind::Closure { function: left },
            VirtualObjectKind::Closure { function: right },
        ) => left == right,
        _ => false,
    }
}

fn recognized_virtual_use(
    instruction: &RegInstr,
    reg: Reg,
    kind: &VirtualObjectKind,
    objects: &[Option<VirtualObjectId>],
) -> bool {
    if matches!(instruction, RegInstr::Move { src, .. } if *src == reg) {
        return true;
    }
    if virtual_constructor(instruction).is_some() {
        // A virtual value nested in another virtual value stays virtual when
        // the destination was recognized by this same analysis.
        return match instr_written_reg(instruction) {
            RegFootprint::Some(writes) => writes
                .iter()
                .any(|dst| objects.get(*dst).is_some_and(Option::is_some)),
            RegFootprint::All => false,
        };
    }
    match kind {
        VirtualObjectKind::Option => matches!(
            instruction,
            RegInstr::MatchOption { src, .. }
                | RegInstr::UnwrapSome { src, .. }
                | RegInstr::TryResult { src, .. }
                if *src == reg
        ),
        VirtualObjectKind::Result => matches!(
            instruction,
            RegInstr::MatchResult { src, .. }
                | RegInstr::UnwrapVariantValue { src, .. }
                | RegInstr::TryResult { src, .. }
                if *src == reg
        ),
        VirtualObjectKind::Variant(_) => matches!(
            instruction,
            RegInstr::MatchVariant { src, .. }
                | RegInstr::UnwrapVariantValue { src, .. }
                | RegInstr::GetField { base: src, .. }
                | RegInstr::GetFieldSlot { base: src, .. }
                if *src == reg
        ),
        VirtualObjectKind::Struct(_) => matches!(
            instruction,
            RegInstr::GetField { base, .. } | RegInstr::GetFieldSlot { base, .. }
                if *base == reg
        ),
        VirtualObjectKind::Closure { .. } => false,
    }
}

/// Shared bounded clean-exit reconstruction tree used by all aggregate passes.
#[cfg(feature = "native-jit")]
#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct VirtualMaterializeRecipe {
    pub(in crate::reg_vm) dst_reg: usize,
    pub(in crate::reg_vm) value: VirtualMaterializeValue,
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct VirtualMaterializeVariantArm {
    pub(in crate::reg_vm) tag: i64,
    pub(in crate::reg_vm) layout: Rc<crate::vm_value::TypeLayout>,
    pub(in crate::reg_vm) fields: Vec<VirtualMaterializeValue>,
}

#[cfg(feature = "native-jit")]
#[derive(Clone, Debug)]
pub(in crate::reg_vm) enum VirtualMaterializeValue {
    Register(usize),
    OptionSome(Box<VirtualMaterializeValue>),
    #[cfg(test)]
    Struct {
        layout: Rc<crate::vm_value::TypeLayout>,
        fields: Vec<VirtualMaterializeValue>,
    },
    Variant {
        tag_reg: Option<usize>,
        arms: Vec<VirtualMaterializeVariantArm>,
    },
}

pub(in crate::reg_vm) type OsrMaterializeRecipe = VirtualMaterializeRecipe;
pub(in crate::reg_vm) type OsrMaterializeVariantArm = VirtualMaterializeVariantArm;
pub(in crate::reg_vm) type OsrMaterializeValue = VirtualMaterializeValue;

pub(in crate::reg_vm) const MAX_OSR_MATERIALIZE_DEPTH: usize = 8;
pub(in crate::reg_vm) const MAX_OSR_MATERIALIZE_NODES: usize = 64;

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) type ResultRecipe = (usize, usize, usize, Option<usize>);

/// Aggregate-independent accounting for the shared virtual-object rewrite
/// pipeline.  The counters describe actual constructor removal rather than
/// merely reporting that a legacy specialized pass was invoked.
#[cfg(feature = "native-jit")]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::reg_vm) struct VirtualEliminationSummary {
    pub(in crate::reg_vm) option_candidates: usize,
    pub(in crate::reg_vm) result_candidates: usize,
    pub(in crate::reg_vm) variant_candidates: usize,
    pub(in crate::reg_vm) struct_candidates: usize,
    pub(in crate::reg_vm) constructors_eliminated: usize,
    pub(in crate::reg_vm) materialization_recipes: usize,
    pub(in crate::reg_vm) work_units: usize,
}

/// One unified whole-function virtual-object rewrite result.
///
/// The mature aggregate-specific rewrites remain the proven mechanics for now,
/// but orchestration, origin composition, work accounting and telemetry are
/// owned here. This is the cutover point for replacing those mechanics one kind
/// at a time without maintaining four independent backend pipelines.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) struct VirtualEliminationResult {
    pub(in crate::reg_vm) code: Vec<RegInstr>,
    pub(in crate::reg_vm) n_regs: usize,
    /// Final instruction index to the input stream passed to the pipeline.
    pub(in crate::reg_vm) ip_map: Vec<usize>,
    pub(in crate::reg_vm) zero_init_regs: Vec<usize>,
    pub(in crate::reg_vm) summary: VirtualEliminationSummary,
}

#[cfg(feature = "native-jit")]
const MAX_VIRTUAL_REWRITE_WORK_UNITS: usize = 262_144;

/// Eliminate non-escaping Option/Result/Variant/Struct allocations through one
/// bounded, fail-closed pipeline.
///
/// Each underlying rewrite already owns differential-tested exit/deopt
/// materialization. Whole-function compilation does not need those recipes at a
/// normal return, but retaining their count here makes accidental loss visible
/// and lets the same result vocabulary be reused by the OSR cutover.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn native_eliminate_virtual_objects_whole(
    code: &[RegInstr],
    n_regs: usize,
) -> Option<VirtualEliminationResult> {
    let mut summary = virtual_constructor_summary(code);
    let input_candidates = virtual_candidate_total(summary)?;
    let mut work_units = code.len();
    if work_units > MAX_VIRTUAL_REWRITE_WORK_UNITS {
        return None;
    }

    let exit = native_whole_function_region_exit(code);
    let (code_r, regs_r, map_r, result_recipes) =
        native_scalar_replace_results_in_region(code, n_regs, 0, exit)?;
    charge_virtual_rewrite_work(&mut work_units, code_r.len())?;

    let (code_o, regs_o, zero_init_regs, map_o) = native_scalar_replace_options(&code_r, regs_r)?;
    charge_virtual_rewrite_work(&mut work_units, code_o.len())?;

    let exit = native_whole_function_region_exit(&code_o);
    let (code_v, regs_v, map_v, variant_recipes) =
        native_scalar_replace_variants_in_region(&code_o, regs_o, 0, exit)?;
    charge_virtual_rewrite_work(&mut work_units, code_v.len())?;

    let exit = native_whole_function_region_exit(&code_v);
    let (code_s, regs_s, map_s, struct_recipes) =
        native_scalar_replace_structs_in_region(&code_v, regs_v, 0, exit)?;
    charge_virtual_rewrite_work(&mut work_units, code_s.len())?;

    let ip_map = map_s
        .iter()
        .map(|&s| {
            let v = *map_v.get(s)?;
            let o = *map_o.get(v)?;
            map_r.get(o).copied()
        })
        .collect::<Option<Vec<_>>>()?;
    let remaining = virtual_candidate_total(virtual_constructor_summary(&code_s))?;
    summary.constructors_eliminated = input_candidates.saturating_sub(remaining);
    summary.materialization_recipes = result_recipes
        .len()
        .checked_add(variant_recipes.len())?
        .checked_add(struct_recipes.len())?;
    summary.work_units = work_units;
    Some(VirtualEliminationResult {
        code: code_s,
        n_regs: regs_s,
        ip_map,
        zero_init_regs,
        summary,
    })
}

#[cfg(feature = "native-jit")]
fn virtual_constructor_summary(code: &[RegInstr]) -> VirtualEliminationSummary {
    let mut summary = VirtualEliminationSummary::default();
    for instruction in code {
        match instruction {
            RegInstr::LoadNone { .. } | RegInstr::MakeSome { .. } => {
                summary.option_candidates = summary.option_candidates.saturating_add(1);
            }
            RegInstr::MakeVariant { layout, .. }
                if matches!(layout.name.as_ref(), "Ok" | "Err") =>
            {
                summary.result_candidates = summary.result_candidates.saturating_add(1);
            }
            RegInstr::MakeVariant { .. } => {
                summary.variant_candidates = summary.variant_candidates.saturating_add(1);
            }
            RegInstr::MakeStruct { .. } => {
                summary.struct_candidates = summary.struct_candidates.saturating_add(1);
            }
            _ => {}
        }
    }
    summary
}

#[cfg(feature = "native-jit")]
fn virtual_candidate_total(summary: VirtualEliminationSummary) -> Option<usize> {
    summary
        .option_candidates
        .checked_add(summary.result_candidates)?
        .checked_add(summary.variant_candidates)?
        .checked_add(summary.struct_candidates)
}

#[cfg(feature = "native-jit")]
fn charge_virtual_rewrite_work(work: &mut usize, units: usize) -> Option<()> {
    *work = work.checked_add(units)?;
    (*work <= MAX_VIRTUAL_REWRITE_WORK_UNITS).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn option_builder(root_reg: Reg) -> VirtualBuilder {
        VirtualBuilder {
            id: VirtualObjectId(0),
            root_reg,
            aliases: vec![root_reg],
            kind: VirtualObjectKind::Option,
            fields: Vec::new(),
            definitions: 1,
            escape: VirtualEscapeClass::NoEscape,
            exit_regs: BTreeSet::new(),
            first_escape: None,
            dynamic_state: false,
        }
    }

    fn function(regs: usize, code: Vec<RegInstr>) -> RegFunction {
        RegFunction {
            ordinal: 0,
            name: "virtual".into(),
            params: 0,
            captures: 0,
            regs,
            local_regs: HashMap::new(),
            code,
        }
    }

    fn typed(function: &RegFunction, included: &[bool]) -> TypedRegion {
        let unit = RegUnit {
            functions: vec![Rc::new(function.clone())],
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            variant_layouts: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        };
        let executable = VerifiedExecutableFacts::derive(&unit).expect("facts");
        TypedRegion::derive(
            function,
            executable.function(0).expect("function facts"),
            included,
        )
        .expect("typed region")
    }

    fn typed_ir(function: &RegFunction, included: &[bool]) -> TypedRegionIr {
        let unit = RegUnit {
            functions: vec![Rc::new(function.clone())],
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            variant_layouts: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        };
        let executable = VerifiedExecutableFacts::derive(&unit).expect("facts");
        TypedRegionIr::derive(
            function,
            executable.function(0).expect("function facts"),
            included,
        )
        .expect("typed region IR")
    }

    #[test]
    fn typed_struct_consumed_inside_region_does_not_escape() {
        let layout = Rc::new(crate::vm_value::TypeLayout::new(
            Rc::from("Point"),
            vec![Rc::from("x")],
        ));
        let function = function(
            5,
            vec![
                RegInstr::LoadInt { dst: 0, value: 1 },
                RegInstr::MakeStruct {
                    dst: 1,
                    layout,
                    fields: vec![("x".into(), 0)],
                },
                RegInstr::GetFieldSlot {
                    dst: 2,
                    base: 1,
                    slot: 0,
                },
                RegInstr::LoadInt { dst: 3, value: 1 },
                RegInstr::AddInt {
                    dst: 4,
                    lhs: 2,
                    rhs: 3,
                },
                RegInstr::Return { src: 4 },
            ],
        );
        let typed = typed_ir(&function, &[true; 6]);
        let analysis = VirtualObjectAnalysis::derive_ir(&function, &typed).expect("analysis");
        let object = analysis.object_for_reg(1).expect("struct");
        assert_eq!(object.escape, VirtualEscapeClass::NoEscape);
        assert_eq!(object.materialization, VirtualMaterializationPlan::None);
        assert!(analysis.safely_virtual(object.id));
    }

    #[test]
    fn live_after_virtual_value_requires_exit_materialization() {
        let function = function(
            3,
            vec![
                RegInstr::LoadInt { dst: 0, value: 1 },
                RegInstr::MakeSome { dst: 1, value: 0 },
                RegInstr::UnwrapSome { dst: 2, src: 1 },
            ],
        );
        let typed = typed(&function, &[true, true, false]);
        let analysis = VirtualObjectAnalysis::derive(&function, &typed).expect("analysis");
        let object = analysis.object_for_reg(1).expect("option");
        assert_eq!(object.escape, VirtualEscapeClass::ExitOnly);
        assert_eq!(
            object.materialization,
            VirtualMaterializationPlan::AtExit {
                destinations: vec![1].into_boxed_slice()
            }
        );
    }

    #[test]
    fn provider_argument_is_a_real_escape() {
        let layout = Rc::new(crate::vm_value::TypeLayout::new(
            Rc::from("Point"),
            vec![Rc::from("x")],
        ));
        let function = function(
            3,
            vec![
                RegInstr::LoadInt { dst: 0, value: 1 },
                RegInstr::MakeStruct {
                    dst: 1,
                    layout,
                    fields: vec![("x".into(), 0)],
                },
                RegInstr::CallExternal {
                    dst: 2,
                    key: "sink.write".into(),
                    args: vec![1],
                    mut_args: Vec::new(),
                },
            ],
        );
        let typed = typed(&function, &[true; 3]);
        let analysis = VirtualObjectAnalysis::derive(&function, &typed).expect("analysis");
        let object = analysis.object_for_reg(1).expect("struct");
        assert_eq!(object.escape, VirtualEscapeClass::Escapes);
        assert_eq!(
            object.materialization,
            VirtualMaterializationPlan::RequiredAt { instruction: 2 }
        );
        assert!(!analysis.safely_virtual(object.id));
    }

    #[test]
    fn reverse_order_long_move_chain_uses_bounded_worklist() {
        const CHAIN_LEN: usize = 4_096;
        let root = CHAIN_LEN;
        let moves = (0..CHAIN_LEN).map(|dst| (dst + 1, dst)).collect::<Vec<_>>();
        let mut object_by_reg = vec![None; CHAIN_LEN + 1];
        object_by_reg[root] = Some(VirtualObjectId(0));
        let mut builders = vec![option_builder(root)];

        propagate_virtual_move_aliases(
            &moves,
            &mut object_by_reg,
            &mut builders,
            MAX_VIRTUAL_ALIAS_WORK_UNITS,
        )
        .expect("reverse-order chain remains within the linear work budget");

        assert!(
            object_by_reg
                .iter()
                .all(|object| *object == Some(VirtualObjectId(0)))
        );
        builders[0].aliases.sort_unstable();
        builders[0].aliases.dedup();
        assert_eq!(builders[0].aliases.len(), CHAIN_LEN + 1);
    }

    #[test]
    fn move_alias_work_limit_fails_closed_before_propagation() {
        let moves = vec![(1, 0)];
        let mut object_by_reg = vec![None, Some(VirtualObjectId(0))];
        let original = object_by_reg.clone();
        let mut builders = vec![option_builder(1)];

        assert!(
            propagate_virtual_move_aliases(&moves, &mut object_by_reg, &mut builders, 1).is_none()
        );
        assert_eq!(object_by_reg, original);
        assert_eq!(builders[0].aliases, vec![1]);
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn unified_pipeline_eliminates_non_escaping_option_allocation() {
        let code = vec![
            RegInstr::LoadInt { dst: 0, value: 7 },
            RegInstr::MakeSome { dst: 1, value: 0 },
            RegInstr::UnwrapSome { dst: 2, src: 1 },
            RegInstr::Return { src: 2 },
        ];
        let result = native_eliminate_virtual_objects_whole(&code, 3).expect("rewrite");
        assert_eq!(result.summary.option_candidates, 1);
        assert!(result.summary.constructors_eliminated >= 1);
        assert!(result.summary.work_units <= MAX_VIRTUAL_REWRITE_WORK_UNITS);
        assert!(!result.code.iter().any(|instruction| matches!(
            instruction,
            RegInstr::MakeSome { .. } | RegInstr::LoadNone { .. }
        )));
        assert_eq!(result.ip_map.len(), result.code.len());
    }

    #[cfg(feature = "native-jit")]
    #[test]
    fn unified_summary_covers_result_variant_and_struct_kinds() {
        let ok = result_ok_layout();
        let user_variant = Rc::new(crate::vm_value::TypeLayout::new(
            Rc::from("Present"),
            vec![Rc::from("value")],
        ));
        let structure = Rc::new(crate::vm_value::TypeLayout::new(
            Rc::from("Point"),
            vec![Rc::from("x")],
        ));
        let code = vec![
            RegInstr::LoadInt { dst: 0, value: 1 },
            RegInstr::MakeVariant {
                dst: 1,
                layout: ok,
                fields: vec![("value".into(), 0)],
            },
            RegInstr::MakeVariant {
                dst: 2,
                layout: user_variant,
                fields: vec![("value".into(), 0)],
            },
            RegInstr::MakeStruct {
                dst: 3,
                layout: structure,
                fields: vec![("x".into(), 0)],
            },
        ];
        let summary = virtual_constructor_summary(&code);
        assert_eq!(summary.result_candidates, 1);
        assert_eq!(summary.variant_candidates, 1);
        assert_eq!(summary.struct_candidates, 1);
    }
}
