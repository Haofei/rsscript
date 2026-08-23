//! Evaluation-local typed region IR.
//!
//! The persisted executable remains register bytecode.  After verification we
//! project it into a small, bounded, block-shaped IR which owns typed value,
//! call, field and aggregate operations.  The IR deliberately retains the
//! original register instruction as its lowering payload: this makes adoption
//! incremental while ensuring every backend path consumes the same verified
//! storage/effect facts instead of reconstructing them independently.

use super::*;
use std::collections::BTreeSet;

const MAX_TYPED_REGION_INSTRUCTIONS: usize = 4_096;
const MAX_TYPED_REGION_BLOCKS: usize = 2_048;
const MAX_TYPED_REGION_OPERANDS: usize = 32_768;
const MAX_TYPED_REGION_WORK_UNITS: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(in crate::reg_vm) struct TypedValueId(u32);

impl TypedValueId {
    fn new(value: usize) -> Option<Self> {
        Some(Self(value.try_into().ok()?))
    }
}

/// Ownership evidence provable from bytecode-v1 storage and definitions.
///
/// Borrow modes are intentionally absent: v1 does not retain enough
/// program-point `read`/`mut`/`take` information to prove them. `Shared` and
/// `Unknown` are conservative and cannot authorize an alias-sensitive pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum TypedValueOwnership {
    Copy,
    Owned,
    Shared,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum TypedAliasClass {
    NoAlias,
    Unique(TypedValueId),
    /// Function input slot; captures precede ordinary parameters in bytecode v1.
    Param(u32),
    Immutable,
    Shared,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) struct TypedRegionValue {
    pub(in crate::reg_vm) id: TypedValueId,
    pub(in crate::reg_vm) vm_reg: Reg,
    pub(in crate::reg_vm) storage: VerifiedStorageType,
    pub(in crate::reg_vm) ownership: TypedValueOwnership,
    pub(in crate::reg_vm) alias: TypedAliasClass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum TypedFieldAccessKind {
    Read {
        result: TypedValueId,
    },
    Write {
        result: TypedValueId,
        value: TypedValueId,
    },
}

/// A statically slot-resolved field operation.
///
/// `layout` is deliberately absent. Bytecode v1 proves the slot and storage
/// classes but does not always retain a nominal type identity for the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) struct TypedFieldAccess {
    pub(in crate::reg_vm) instruction: usize,
    pub(in crate::reg_vm) base: TypedValueId,
    pub(in crate::reg_vm) slot: usize,
    pub(in crate::reg_vm) field_storage: VerifiedStorageType,
    pub(in crate::reg_vm) kind: TypedFieldAccessKind,
}

#[derive(Clone, Debug)]
pub(in crate::reg_vm) enum TypedAggregateKind {
    OptionNone,
    OptionSome,
    Result(Rc<crate::vm_value::TypeLayout>),
    Variant(Rc<crate::vm_value::TypeLayout>),
    Struct(Rc<crate::vm_value::TypeLayout>),
    List,
    Map,
    Object,
    Closure { function: usize },
}

#[derive(Clone, Debug)]
pub(in crate::reg_vm) enum TypedRegionOp {
    Constant {
        result: TypedValueId,
        storage: VerifiedStorageType,
    },
    Move {
        result: TypedValueId,
        source: TypedValueId,
        storage: VerifiedStorageType,
    },
    Call {
        result: TypedValueId,
        result_storage: VerifiedStorageType,
        target: VerifiedCallTarget,
        arguments: Box<[TypedValueId]>,
        parameter_effects: Box<[VerifiedParamEffect]>,
    },
    Field(TypedFieldAccess),
    Aggregate {
        result: TypedValueId,
        kind: TypedAggregateKind,
        fields: Box<[TypedValueId]>,
    },
    Control,
    Other,
}

#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct TypedRegionInstruction {
    pub(in crate::reg_vm) source_ip: usize,
    pub(in crate::reg_vm) reads: Box<[TypedValueId]>,
    pub(in crate::reg_vm) writes: Box<[TypedValueId]>,
    pub(in crate::reg_vm) op: TypedRegionOp,
    source: RegInstr,
}

#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct TypedRegionBlock {
    pub(in crate::reg_vm) id: u32,
    pub(in crate::reg_vm) entry_ip: usize,
    pub(in crate::reg_vm) instructions: Box<[TypedRegionInstruction]>,
    pub(in crate::reg_vm) successors: Box<[usize]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::reg_vm) struct TypedRegionSummary {
    pub(in crate::reg_vm) blocks: usize,
    pub(in crate::reg_vm) instructions: usize,
    pub(in crate::reg_vm) values: usize,
    pub(in crate::reg_vm) calls: usize,
    pub(in crate::reg_vm) field_accesses: usize,
    pub(in crate::reg_vm) aggregates: usize,
    pub(in crate::reg_vm) work_units: usize,
}

/// A verified-facts-backed, evaluation-local region IR.
///
/// Construction is all-or-nothing. Any active `Unknown`, imprecise register
/// footprint, missing call-site fact, invalid edge, or exceeded work limit
/// declines native compilation and leaves the interpreter authoritative.
#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct TypedRegionIr {
    typed: TypedRegion,
    blocks: Box<[TypedRegionBlock]>,
    summary: TypedRegionSummary,
}

#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct TypedRegion {
    values: Box<[TypedRegionValue]>,
    value_by_reg: Box<[Option<TypedValueId>]>,
    field_accesses: Box<[TypedFieldAccess]>,
    included: Box<[bool]>,
}

impl TypedRegion {
    pub(in crate::reg_vm) fn derive(
        function: &RegFunction,
        facts: &VerifiedFunctionFacts,
        included: &[bool],
    ) -> Option<Self> {
        if included.len() != function.code.len()
            || facts.reg_types.len() != function.regs
            || facts.effects.len() != function.code.len()
        {
            return None;
        }

        let mut active = vec![false; function.regs];
        for (ip, effect) in facts.effects.iter().enumerate() {
            if !included[ip] {
                continue;
            }
            mark_footprint(&effect.reads, &mut active);
            mark_footprint(&effect.writes, &mut active);
        }

        let mut parent: Vec<usize> = (0..function.regs).collect();
        let mut rank = vec![0u8; function.regs];
        for (ip, instruction) in function.code.iter().enumerate() {
            if !included[ip] {
                continue;
            }
            if let RegInstr::Move { dst, src } = instruction
                && facts.reg_types.get(*dst) == Some(&VerifiedStorageType::Handle)
                && facts.reg_types.get(*src) == Some(&VerifiedStorageType::Handle)
            {
                union(&mut parent, &mut rank, *dst, *src);
            }
        }

        let mut members = vec![0usize; function.regs];
        let mut defs = vec![0usize; function.regs];
        let mut constructor = vec![false; function.regs];
        let mut immutable = vec![false; function.regs];
        let mut heap_mutated = vec![false; function.regs];
        for (reg, is_active) in active.iter().copied().enumerate() {
            if is_active {
                let root = find(&mut parent, reg);
                members[root] = members[root].saturating_add(1);
            }
        }
        for (ip, instruction) in function.code.iter().enumerate() {
            if !included[ip] {
                continue;
            }
            if let VerifiedRegFootprint::Exact(writes) = &facts.effects[ip].writes {
                for &reg in writes.iter() {
                    if reg < function.regs {
                        defs[reg] = defs[reg].saturating_add(1);
                    }
                }
            }
            if let Some(dst) = fresh_handle_constructor(instruction)
                && dst < function.regs
            {
                constructor[dst] = true;
            }
            if let RegInstr::LoadString { dst, .. } = instruction
                && *dst < function.regs
            {
                immutable[*dst] = true;
            }
            if facts.effects[ip].writes_heap
                && let VerifiedRegFootprint::Exact(reads) = &facts.effects[ip].reads
            {
                for &reg in reads.iter() {
                    if reg < function.regs && facts.reg_types[reg] == VerifiedStorageType::Handle {
                        let root = find(&mut parent, reg);
                        heap_mutated[root] = true;
                    }
                }
            }
        }
        let mut immutable_root = vec![false; function.regs];
        for (reg, immutable) in immutable.iter().copied().enumerate() {
            if immutable {
                let root = find(&mut parent, reg);
                immutable_root[root] = true;
            }
        }

        let active_regs = active
            .iter()
            .enumerate()
            .filter_map(|(reg, active)| active.then_some(reg))
            .collect::<Vec<_>>();
        let mut value_by_reg = vec![None; function.regs];
        let mut values = Vec::with_capacity(active_regs.len());
        for reg in active_regs {
            let id = TypedValueId::new(values.len())?;
            value_by_reg[reg] = Some(id);
            let storage = facts.reg_types[reg];
            let (ownership, alias) = match storage {
                VerifiedStorageType::Unit
                | VerifiedStorageType::Int
                | VerifiedStorageType::Bool
                | VerifiedStorageType::Float
                | VerifiedStorageType::Char => {
                    (TypedValueOwnership::Copy, TypedAliasClass::NoAlias)
                }
                VerifiedStorageType::Unknown => {
                    (TypedValueOwnership::Unknown, TypedAliasClass::Unknown)
                }
                VerifiedStorageType::Handle => {
                    let root = find(&mut parent, reg);
                    if reg < function.captures.saturating_add(function.params) {
                        (
                            TypedValueOwnership::Shared,
                            TypedAliasClass::Param(reg.try_into().ok()?),
                        )
                    } else if immutable_root[root] && !heap_mutated[root] {
                        (TypedValueOwnership::Shared, TypedAliasClass::Immutable)
                    } else if constructor[reg]
                        && defs[reg] == 1
                        && members[root] == 1
                        && !heap_mutated[root]
                    {
                        (TypedValueOwnership::Owned, TypedAliasClass::Unique(id))
                    } else {
                        (TypedValueOwnership::Shared, TypedAliasClass::Shared)
                    }
                }
            };
            values.push(TypedRegionValue {
                id,
                vm_reg: reg,
                storage,
                ownership,
                alias,
            });
        }

        let mut field_accesses = Vec::new();
        for (instruction, op) in function.code.iter().enumerate() {
            if !included[instruction] {
                continue;
            }
            let access = match op {
                RegInstr::GetFieldSlot { dst, base, slot } => TypedFieldAccess {
                    instruction,
                    base: value_by_reg.get(*base).copied().flatten()?,
                    slot: *slot,
                    field_storage: *facts.reg_types.get(*dst)?,
                    kind: TypedFieldAccessKind::Read {
                        result: value_by_reg.get(*dst).copied().flatten()?,
                    },
                },
                RegInstr::SetFieldSlot {
                    dst,
                    base,
                    slot,
                    value,
                } => TypedFieldAccess {
                    instruction,
                    base: value_by_reg.get(*base).copied().flatten()?,
                    slot: *slot,
                    field_storage: *facts.reg_types.get(*value)?,
                    kind: TypedFieldAccessKind::Write {
                        result: value_by_reg.get(*dst).copied().flatten()?,
                        value: value_by_reg.get(*value).copied().flatten()?,
                    },
                },
                _ => continue,
            };
            if values.get(access.base.index_for_runtime())?.storage != VerifiedStorageType::Handle {
                return None;
            }
            field_accesses.push(access);
        }

        Some(Self {
            values: values.into_boxed_slice(),
            value_by_reg: value_by_reg.into_boxed_slice(),
            field_accesses: field_accesses.into_boxed_slice(),
            included: included.to_vec().into_boxed_slice(),
        })
    }

    pub(in crate::reg_vm) fn value(&self, reg: Reg) -> Option<&TypedRegionValue> {
        let id = self.value_by_reg.get(reg).copied().flatten()?;
        self.values.get(id.index_for_runtime())
    }

    #[cfg(test)]
    pub(in crate::reg_vm) fn values(&self) -> &[TypedRegionValue] {
        &self.values
    }

    pub(in crate::reg_vm) fn field_accesses(&self) -> &[TypedFieldAccess] {
        &self.field_accesses
    }

    pub(in crate::reg_vm) fn reg_for_value(&self, value: TypedValueId) -> Option<Reg> {
        self.values
            .get(value.index_for_runtime())
            .map(|value| value.vm_reg)
    }

    pub(in crate::reg_vm) fn contains_instruction(&self, ip: usize) -> bool {
        self.included.get(ip).copied().unwrap_or(false)
    }
}

impl TypedRegionIr {
    pub(in crate::reg_vm) fn derive(
        function: &RegFunction,
        facts: &VerifiedFunctionFacts,
        included: &[bool],
    ) -> Option<Self> {
        let typed = TypedRegion::derive(function, facts, included)?;
        let instruction_count = included.iter().filter(|included| **included).count();
        if instruction_count == 0 || instruction_count > MAX_TYPED_REGION_INSTRUCTIONS {
            return None;
        }

        let mut work = TypedRegionWorkBudget::new(MAX_TYPED_REGION_WORK_UNITS);
        work.charge(instruction_count)?;
        work.charge(typed.values.len())?;

        let mut instructions = vec![None; function.code.len()];
        let mut total_operands = 0usize;
        let mut summary = TypedRegionSummary {
            instructions: instruction_count,
            values: typed.values.len(),
            ..TypedRegionSummary::default()
        };
        for (source_ip, source) in function.code.iter().enumerate() {
            if !included[source_ip] {
                continue;
            }
            let effect = facts.effects.get(source_ip)?;
            let reads = typed_values_for_footprint(&typed, &effect.reads)?;
            let writes = typed_values_for_footprint(&typed, &effect.writes)?;
            let operands = reads.len().checked_add(writes.len())?;
            total_operands = total_operands.checked_add(operands)?;
            if total_operands > MAX_TYPED_REGION_OPERANDS {
                return None;
            }
            work.charge(operands.saturating_add(1))?;
            let op = typed_region_op(function, facts, &typed, source_ip, source)?;
            match op {
                TypedRegionOp::Call { .. } => summary.calls = summary.calls.saturating_add(1),
                TypedRegionOp::Field(_) => {
                    summary.field_accesses = summary.field_accesses.saturating_add(1)
                }
                TypedRegionOp::Aggregate { .. } => {
                    summary.aggregates = summary.aggregates.saturating_add(1)
                }
                _ => {}
            }
            instructions[source_ip] = Some(TypedRegionInstruction {
                source_ip,
                reads,
                writes,
                op,
                source: source.clone(),
            });
        }

        let mut leaders = vec![false; function.code.len()];
        let first = included.iter().position(|included| *included)?;
        leaders[first] = true;
        for (ip, source) in function.code.iter().enumerate() {
            if !included[ip] {
                continue;
            }
            let mut successors = Vec::new();
            native_instr_successors(source, ip, function.code.len(), |target| {
                if included.get(target).copied().unwrap_or(false) {
                    successors.push(target);
                }
            });
            successors.sort_unstable();
            successors.dedup();
            work.charge(successors.len().max(1))?;
            for &target in &successors {
                if target != ip.saturating_add(1) {
                    leaders[target] = true;
                }
            }
            if (successors.len() != 1 || successors.first().copied() != ip.checked_add(1))
                && included.get(ip.saturating_add(1)).copied().unwrap_or(false)
            {
                leaders[ip + 1] = true;
            }
        }

        let mut blocks = Vec::new();
        let mut ip = 0usize;
        while ip < function.code.len() {
            if !included[ip] {
                ip += 1;
                continue;
            }
            let entry_ip = ip;
            let mut block_instructions = Vec::new();
            loop {
                block_instructions.push(instructions.get_mut(ip)?.take()?);
                let next = ip.saturating_add(1);
                if next >= function.code.len()
                    || !included[next]
                    || (leaders[next] && next != entry_ip)
                {
                    break;
                }
                ip = next;
            }
            let last = block_instructions.last()?.source_ip;
            let mut successors = Vec::new();
            native_instr_successors(&function.code[last], last, function.code.len(), |target| {
                if included.get(target).copied().unwrap_or(false) {
                    successors.push(target);
                }
            });
            successors.sort_unstable();
            successors.dedup();
            work.charge(successors.len().max(1))?;
            let id = u32::try_from(blocks.len()).ok()?;
            blocks.push(TypedRegionBlock {
                id,
                entry_ip,
                instructions: block_instructions.into_boxed_slice(),
                successors: successors.into_boxed_slice(),
            });
            if blocks.len() > MAX_TYPED_REGION_BLOCKS {
                return None;
            }
            ip = last.saturating_add(1);
        }
        if instructions.iter().any(Option::is_some) {
            return None;
        }
        summary.blocks = blocks.len();
        summary.work_units = work.consumed;
        let ir = Self {
            typed,
            blocks: blocks.into_boxed_slice(),
            summary,
        };
        ir.validate()?;
        Some(ir)
    }

    pub(in crate::reg_vm) fn typed(&self) -> &TypedRegion {
        &self.typed
    }

    pub(in crate::reg_vm) fn blocks(&self) -> &[TypedRegionBlock] {
        &self.blocks
    }

    pub(in crate::reg_vm) fn summary(&self) -> TypedRegionSummary {
        self.summary
    }

    /// Project typed blocks into the existing register stream consumed by the
    /// mature VM-to-Cranelift backend.
    ///
    /// Typed field operations and known calls are rebuilt from verified IDs,
    /// storage, and parameter-effect facts; their source instruction is not the
    /// lowering authority. Operations not migrated to typed lowering yet retain
    /// their validated source payload. Non-region instructions become a
    /// defensive boundary and cannot execute natively.
    pub(in crate::reg_vm) fn lower_to_reg_code(
        &self,
        function: &RegFunction,
    ) -> Option<Vec<RegInstr>> {
        let mut lowered = vec![
            RegInstr::RuntimeError {
                message: "typed native region boundary".to_string(),
            };
            function.code.len()
        ];
        let mut lowered_count = 0usize;
        for block in &self.blocks {
            for instruction in &block.instructions {
                *lowered.get_mut(instruction.source_ip)? =
                    self.lower_instruction_for_native(instruction)?;
                lowered_count = lowered_count.checked_add(1)?;
            }
        }
        (lowered_count == self.summary.instructions).then_some(lowered)
    }

    fn lower_instruction_for_native(
        &self,
        instruction: &TypedRegionInstruction,
    ) -> Option<RegInstr> {
        match &instruction.op {
            TypedRegionOp::Field(access) => {
                let base = self.typed.reg_for_value(access.base)?;
                match access.kind {
                    TypedFieldAccessKind::Read { result } => Some(RegInstr::GetFieldSlot {
                        dst: self.typed.reg_for_value(result)?,
                        base,
                        slot: access.slot,
                    }),
                    TypedFieldAccessKind::Write { result, value } => Some(RegInstr::SetFieldSlot {
                        dst: self.typed.reg_for_value(result)?,
                        base,
                        slot: access.slot,
                        value: self.typed.reg_for_value(value)?,
                    }),
                }
            }
            TypedRegionOp::Call {
                result,
                target: VerifiedCallTarget::Known(function),
                arguments,
                parameter_effects,
                ..
            } if matches!(instruction.source, RegInstr::CallKnown { .. }) => {
                let args = arguments
                    .iter()
                    .map(|argument| self.typed.reg_for_value(*argument))
                    .collect::<Option<Vec<_>>>()?;
                let mut_args = parameter_effects
                    .iter()
                    .enumerate()
                    .filter_map(|(index, effect)| {
                        (*effect == VerifiedParamEffect::Mut).then_some(index)
                    })
                    .collect();
                Some(RegInstr::CallKnown {
                    dst: self.typed.reg_for_value(*result)?,
                    function: *function,
                    args,
                    mut_args,
                })
            }
            _ => Some(instruction.source.clone()),
        }
    }

    fn validate(&self) -> Option<()> {
        let entries = self
            .blocks
            .iter()
            .map(|block| block.entry_ip)
            .collect::<BTreeSet<_>>();
        for (ordinal, block) in self.blocks.iter().enumerate() {
            if block.id as usize != ordinal
                || block.instructions.first()?.source_ip != block.entry_ip
                || block
                    .successors
                    .iter()
                    .any(|successor| !entries.contains(successor))
            {
                return None;
            }
            for instruction in &block.instructions {
                if instruction
                    .reads
                    .iter()
                    .chain(instruction.writes.iter())
                    .any(|value| value.index_for_runtime() >= self.typed.values.len())
                {
                    return None;
                }
                let valid = match &instruction.op {
                    TypedRegionOp::Constant { result, storage } => {
                        *storage != VerifiedStorageType::Unknown
                            && instruction.writes.contains(result)
                            && self.typed.values[result.index_for_runtime()].storage == *storage
                    }
                    TypedRegionOp::Move {
                        result,
                        source,
                        storage,
                    } => {
                        *storage != VerifiedStorageType::Unknown
                            && instruction.writes.contains(result)
                            && instruction.reads.contains(source)
                            && self.typed.values[result.index_for_runtime()].storage == *storage
                            && self.typed.values[source.index_for_runtime()].storage == *storage
                    }
                    TypedRegionOp::Call {
                        result,
                        result_storage,
                        target,
                        arguments,
                        parameter_effects,
                    } => {
                        let target_valid = match target {
                            VerifiedCallTarget::Known(function) => *function < usize::MAX,
                            VerifiedCallTarget::Dynamic(functions) => !functions.is_empty(),
                            VerifiedCallTarget::Closure => true,
                            VerifiedCallTarget::Provider(symbol)
                            | VerifiedCallTarget::Intrinsic(symbol) => !symbol.is_empty(),
                        };
                        *result_storage != VerifiedStorageType::Unknown
                            && instruction.writes.contains(result)
                            && self.typed.values[result.index_for_runtime()].storage
                                == *result_storage
                            && target_valid
                            && arguments.len() == parameter_effects.len()
                            && arguments.iter().all(|argument| {
                                instruction.reads.contains(argument)
                                    && self.typed.values[argument.index_for_runtime()].storage
                                        != VerifiedStorageType::Unknown
                            })
                    }
                    TypedRegionOp::Field(access) => {
                        access.instruction == instruction.source_ip
                            && instruction.reads.contains(&access.base)
                            && access.field_storage != VerifiedStorageType::Unknown
                            && match access.kind {
                                TypedFieldAccessKind::Read { result } => {
                                    instruction.writes.contains(&result)
                                }
                                TypedFieldAccessKind::Write { result, value } => {
                                    instruction.writes.contains(&result)
                                        && instruction.reads.contains(&value)
                                }
                            }
                    }
                    TypedRegionOp::Aggregate {
                        result,
                        kind,
                        fields,
                    } => {
                        let kind_valid = match kind {
                            TypedAggregateKind::Result(layout)
                            | TypedAggregateKind::Variant(layout)
                            | TypedAggregateKind::Struct(layout) => !layout.name.is_empty(),
                            TypedAggregateKind::Closure { function } => *function < usize::MAX,
                            TypedAggregateKind::OptionNone
                            | TypedAggregateKind::OptionSome
                            | TypedAggregateKind::List
                            | TypedAggregateKind::Map
                            | TypedAggregateKind::Object => true,
                        };
                        kind_valid
                            && instruction.writes.contains(result)
                            && fields.iter().all(|field| instruction.reads.contains(field))
                    }
                    TypedRegionOp::Control | TypedRegionOp::Other => true,
                };
                if !valid {
                    return None;
                }
            }
        }
        Some(())
    }
}

#[derive(Debug)]
struct TypedRegionWorkBudget {
    remaining: usize,
    consumed: usize,
}

impl TypedRegionWorkBudget {
    fn new(limit: usize) -> Self {
        Self {
            remaining: limit,
            consumed: 0,
        }
    }

    fn charge(&mut self, units: usize) -> Option<()> {
        self.remaining = self.remaining.checked_sub(units)?;
        self.consumed = self.consumed.checked_add(units)?;
        Some(())
    }
}

fn typed_values_for_footprint(
    typed: &TypedRegion,
    footprint: &VerifiedRegFootprint,
) -> Option<Box<[TypedValueId]>> {
    let VerifiedRegFootprint::Exact(registers) = footprint else {
        return None;
    };
    registers
        .iter()
        .map(|reg| {
            let value = typed.value(*reg)?;
            (value.storage != VerifiedStorageType::Unknown).then_some(value.id)
        })
        .collect::<Option<Vec<_>>>()
        .map(Vec::into_boxed_slice)
}

fn typed_region_op(
    _function: &RegFunction,
    facts: &VerifiedFunctionFacts,
    typed: &TypedRegion,
    source_ip: usize,
    source: &RegInstr,
) -> Option<TypedRegionOp> {
    let value = |reg: Reg| typed.value(reg).map(|value| value.id);
    let storage = |reg: Reg| {
        let storage = typed.value(reg)?.storage;
        (storage != VerifiedStorageType::Unknown).then_some(storage)
    };
    let aggregate = |dst: Reg, kind: TypedAggregateKind, fields: Vec<Reg>| {
        Some(TypedRegionOp::Aggregate {
            result: value(dst)?,
            kind,
            fields: fields
                .into_iter()
                .map(value)
                .collect::<Option<Vec<_>>>()?
                .into_boxed_slice(),
        })
    };

    match source {
        RegInstr::LoadUnit { dst }
        | RegInstr::LoadInt { dst, .. }
        | RegInstr::LoadFloat { dst, .. }
        | RegInstr::LoadBool { dst, .. }
        | RegInstr::LoadString { dst, .. }
        | RegInstr::LoadChar { dst, .. } => Some(TypedRegionOp::Constant {
            result: value(*dst)?,
            storage: storage(*dst)?,
        }),
        RegInstr::Move { dst, src } => {
            let destination_storage = storage(*dst)?;
            (destination_storage == storage(*src)?).then_some(TypedRegionOp::Move {
                result: value(*dst)?,
                source: value(*src)?,
                storage: destination_storage,
            })
        }
        RegInstr::GetFieldSlot { .. } | RegInstr::SetFieldSlot { .. } => typed
            .field_accesses
            .iter()
            .find(|access| access.instruction == source_ip)
            .copied()
            .map(TypedRegionOp::Field),
        RegInstr::LoadNone { dst } => aggregate(*dst, TypedAggregateKind::OptionNone, Vec::new()),
        RegInstr::MakeSome { dst, value: field } => {
            aggregate(*dst, TypedAggregateKind::OptionSome, vec![*field])
        }
        RegInstr::MakeStruct {
            dst,
            layout,
            fields,
        } => aggregate(
            *dst,
            TypedAggregateKind::Struct(Rc::clone(layout)),
            fields.iter().map(|(_, reg)| *reg).collect(),
        ),
        RegInstr::MakeVariant {
            dst,
            layout,
            fields,
        } => aggregate(
            *dst,
            if matches!(layout.name.as_ref(), "Ok" | "Err") {
                TypedAggregateKind::Result(Rc::clone(layout))
            } else {
                TypedAggregateKind::Variant(Rc::clone(layout))
            },
            fields.iter().map(|(_, reg)| *reg).collect(),
        ),
        RegInstr::MakeList { dst, items } => {
            aggregate(*dst, TypedAggregateKind::List, items.clone())
        }
        RegInstr::MakeMap { dst, entries } => aggregate(
            *dst,
            TypedAggregateKind::Map,
            entries
                .iter()
                .flat_map(|(key, value)| [*key, *value])
                .collect(),
        ),
        RegInstr::MakeObject { dst, fields } => aggregate(
            *dst,
            TypedAggregateKind::Object,
            fields.iter().map(|(_, reg)| *reg).collect(),
        ),
        RegInstr::MakeClosure {
            dst,
            function,
            captures,
        } => aggregate(
            *dst,
            TypedAggregateKind::Closure {
                function: *function,
            },
            captures.clone(),
        ),
        _ if facts.call_site(source_ip).is_some() => {
            let call = facts.call_site(source_ip)?;
            let arguments = typed_call_argument_regs(source)?;
            let result = typed_call_result_reg(source)?;
            if call.params.contains(&VerifiedStorageType::Unknown)
                || call.result == VerifiedStorageType::Unknown
                || call.params.len() != arguments.len()
                || call.param_effects.len() != arguments.len()
            {
                return None;
            }
            Some(TypedRegionOp::Call {
                result: value(result)?,
                result_storage: call.result,
                target: call.target.clone(),
                arguments: arguments
                    .iter()
                    .copied()
                    .map(value)
                    .collect::<Option<Vec<_>>>()?
                    .into_boxed_slice(),
                parameter_effects: call.param_effects.clone(),
            })
        }
        RegInstr::Jump { .. }
        | RegInstr::JumpIfBool { .. }
        | RegInstr::JumpIfIntCompare { .. }
        | RegInstr::MatchOption { .. }
        | RegInstr::MatchResult { .. }
        | RegInstr::MatchVariant { .. }
        | RegInstr::MatchMapGet { .. }
        | RegInstr::MatchSortedMapGet { .. }
        | RegInstr::Return { .. }
        | RegInstr::RuntimeError { .. } => Some(TypedRegionOp::Control),
        _ => Some(TypedRegionOp::Other),
    }
}

fn typed_call_argument_regs(source: &RegInstr) -> Option<&[Reg]> {
    match source {
        RegInstr::CallKnown { args, .. }
        | RegInstr::CallDynamic { args, .. }
        | RegInstr::SpawnTask { args, .. }
        | RegInstr::CallExternal { args, .. }
        | RegInstr::CallClosure { args, .. }
        | RegInstr::CallIntrinsic { args, .. }
        | RegInstr::CallTypedIntrinsic { args, .. } => Some(args),
        _ => None,
    }
}

fn typed_call_result_reg(source: &RegInstr) -> Option<Reg> {
    match source {
        RegInstr::CallKnown { dst, .. }
        | RegInstr::CallDynamic { dst, .. }
        | RegInstr::SpawnTask { dst, .. }
        | RegInstr::CallExternal { dst, .. }
        | RegInstr::CallClosure { dst, .. }
        | RegInstr::CallIntrinsic { dst, .. }
        | RegInstr::CallTypedIntrinsic { dst, .. } => Some(*dst),
        _ => None,
    }
}

impl TypedValueId {
    fn index_for_runtime(self) -> usize {
        self.0 as usize
    }
}

fn mark_footprint(footprint: &VerifiedRegFootprint, active: &mut [bool]) {
    match footprint {
        VerifiedRegFootprint::Exact(registers) => {
            for &reg in registers.iter() {
                if let Some(active) = active.get_mut(reg) {
                    *active = true;
                }
            }
        }
        VerifiedRegFootprint::All => active.fill(true),
    }
}

fn fresh_handle_constructor(instruction: &RegInstr) -> Option<Reg> {
    match instruction {
        RegInstr::LoadString { dst, .. }
        | RegInstr::MakeStruct { dst, .. }
        | RegInstr::MakeVariant { dst, .. }
        | RegInstr::MakeList { dst, .. }
        | RegInstr::MakeMap { dst, .. }
        | RegInstr::MakeObject { dst, .. }
        | RegInstr::MakeClosure { dst, .. }
        | RegInstr::MakeSome { dst, .. } => Some(*dst),
        _ => None,
    }
}

fn find(parent: &mut [usize], value: usize) -> usize {
    if parent[value] != value {
        parent[value] = find(parent, parent[value]);
    }
    parent[value]
}

fn union(parent: &mut [usize], rank: &mut [u8], left: usize, right: usize) {
    let mut left = find(parent, left);
    let mut right = find(parent, right);
    if left == right {
        return;
    }
    if rank[left] < rank[right] {
        std::mem::swap(&mut left, &mut right);
    }
    parent[right] = left;
    if rank[left] == rank[right] {
        rank[left] = rank[left].saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::rc::Rc;

    fn function(regs: usize, code: Vec<RegInstr>) -> RegFunction {
        RegFunction {
            name: "typed".into(),
            params: 0,
            captures: 0,
            regs,
            local_regs: HashMap::new(),
            code,
        }
    }

    fn facts(function: &RegFunction) -> VerifiedFunctionFacts {
        let unit = RegUnit {
            functions: vec![Rc::new(function.clone())],
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            variant_layouts: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        };
        VerifiedExecutableFacts::derive(&unit)
            .expect("verified facts")
            .function(0)
            .expect("function facts")
            .clone()
    }

    #[test]
    fn derives_dense_typed_values_and_slot_accesses() {
        let layout = Rc::new(crate::vm_value::TypeLayout::new(
            Rc::from("Point"),
            vec![Rc::from("x")],
        ));
        let function = function(
            5,
            vec![
                RegInstr::LoadInt { dst: 0, value: 7 },
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
        let facts = facts(&function);
        let region = TypedRegion::derive(&function, &facts, &[true; 6]).expect("typed region");

        assert_eq!(region.values().len(), 5);
        assert_eq!(
            region.value(0).expect("value").ownership,
            TypedValueOwnership::Copy
        );
        assert_eq!(
            region.value(1).expect("value").ownership,
            TypedValueOwnership::Owned
        );
        assert_eq!(region.field_accesses().len(), 1);
        assert_eq!(region.field_accesses()[0].slot, 0);
        assert_eq!(
            region.field_accesses()[0].field_storage,
            VerifiedStorageType::Int
        );
    }

    #[test]
    fn immutable_move_aliases_remain_immutable() {
        let function = function(
            2,
            vec![
                RegInstr::LoadString {
                    dst: 0,
                    value: Rc::new("value".to_owned()),
                },
                RegInstr::Move { dst: 1, src: 0 },
                RegInstr::Return { src: 1 },
            ],
        );
        let facts = facts(&function);
        let region = TypedRegion::derive(&function, &facts, &[true; 3]).expect("typed region");

        assert_eq!(
            region.value(0).expect("source").alias,
            TypedAliasClass::Immutable
        );
        assert_eq!(
            region.value(1).expect("alias").alias,
            TypedAliasClass::Immutable
        );
    }

    #[test]
    fn mutable_object_move_aliases_remove_unique_ownership() {
        let layout = Rc::new(crate::vm_value::TypeLayout::new(
            Rc::from("Point"),
            vec![Rc::from("x")],
        ));
        let function = function(
            3,
            vec![
                RegInstr::LoadInt { dst: 0, value: 7 },
                RegInstr::MakeStruct {
                    dst: 1,
                    layout,
                    fields: vec![("x".into(), 0)],
                },
                RegInstr::Move { dst: 2, src: 1 },
                RegInstr::Return { src: 2 },
            ],
        );
        let facts = facts(&function);
        let region = TypedRegion::derive(&function, &facts, &[true; 4]).expect("typed region");
        assert_eq!(
            region.value(1).expect("source").alias,
            TypedAliasClass::Shared
        );
        assert_eq!(
            region.value(2).expect("alias").alias,
            TypedAliasClass::Shared
        );
    }

    #[test]
    fn captures_and_parameters_are_both_shared_input_aliases() {
        let mut function = function(
            2,
            vec![
                RegInstr::Move { dst: 1, src: 0 },
                RegInstr::Return { src: 1 },
            ],
        );
        function.captures = 1;
        function.params = 1;
        let mut facts = facts(&function);
        facts.reg_types.fill(VerifiedStorageType::Handle);
        let region = TypedRegion::derive(&function, &facts, &[true; 2]).expect("typed region");

        assert_eq!(
            region.value(0).expect("capture").alias,
            TypedAliasClass::Param(0)
        );
        assert_eq!(
            region.value(1).expect("parameter").alias,
            TypedAliasClass::Param(1)
        );
    }

    #[test]
    fn conflicting_storage_remains_unknown() {
        let function = function(
            1,
            vec![
                RegInstr::LoadInt { dst: 0, value: 1 },
                RegInstr::LoadBool {
                    dst: 0,
                    value: true,
                },
            ],
        );
        let facts = facts(&function);
        let region = TypedRegion::derive(&function, &facts, &[true; 2]).expect("typed region");
        assert_eq!(
            region.value(0).expect("value").storage,
            VerifiedStorageType::Unknown
        );
        assert_eq!(
            region.value(0).expect("value").alias,
            TypedAliasClass::Unknown
        );
        assert!(
            TypedRegionIr::derive(&function, &facts, &[true; 2]).is_none(),
            "typed IR must never promote or carry an active Unknown"
        );
    }

    #[test]
    fn typed_ir_owns_blocks_aggregates_and_round_trips_to_register_code() {
        let layout = Rc::new(crate::vm_value::TypeLayout::new(
            Rc::from("Point"),
            vec![Rc::from("x")],
        ));
        let function = function(
            5,
            vec![
                RegInstr::LoadInt { dst: 0, value: 7 },
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
        let facts = facts(&function);
        let ir = TypedRegionIr::derive(&function, &facts, &[true; 6]).expect("typed IR");
        assert_eq!(ir.summary().aggregates, 1);
        assert_eq!(ir.summary().field_accesses, 1);
        assert_eq!(ir.summary().instructions, 6);
        assert!(!ir.blocks().is_empty());
        assert!(ir.summary().work_units <= MAX_TYPED_REGION_WORK_UNITS);
        let lowered = ir.lower_to_reg_code(&function).expect("lowering");
        assert!(matches!(lowered[1], RegInstr::MakeStruct { .. }));
        assert!(matches!(lowered[2], RegInstr::GetFieldSlot { slot: 0, .. }));
    }

    #[test]
    fn typed_ir_is_bounded_before_block_construction() {
        let code = (0..=MAX_TYPED_REGION_INSTRUCTIONS)
            .map(|dst| RegInstr::LoadInt { dst, value: 1 })
            .collect::<Vec<_>>();
        let function = function(code.len(), code);
        let facts = facts(&function);
        assert!(
            TypedRegionIr::derive(&function, &facts, &vec![true; function.code.len()]).is_none()
        );
    }

    #[test]
    fn typed_ir_owns_verified_call_signature_and_effects() {
        let function = function(
            2,
            vec![
                RegInstr::LoadString {
                    dst: 0,
                    value: Rc::new("seven".to_owned()),
                },
                RegInstr::CallIntrinsic {
                    dst: 1,
                    intrinsic: RegIntrinsic::StringLen,
                    args: vec![0],
                },
                RegInstr::Return { src: 1 },
            ],
        );
        let facts = facts(&function);
        let ir = TypedRegionIr::derive(&function, &facts, &[true; 3]).expect("typed call IR");
        assert_eq!(ir.summary().calls, 1);
        let call = ir
            .blocks()
            .iter()
            .flat_map(|block| block.instructions.iter())
            .find_map(|instruction| match &instruction.op {
                TypedRegionOp::Call {
                    result,
                    result_storage,
                    target,
                    arguments,
                    parameter_effects,
                } => Some((result, result_storage, target, arguments, parameter_effects)),
                _ => None,
            })
            .expect("call op");
        assert_eq!(ir.typed().values[call.0.index_for_runtime()].vm_reg, 1);
        assert_eq!(*call.1, VerifiedStorageType::Int);
        assert!(matches!(call.2, VerifiedCallTarget::Intrinsic(_)));
        assert_eq!(call.3.len(), 1);
        assert_eq!(call.4.len(), 1);
    }

    #[test]
    fn known_call_native_projection_is_rebuilt_from_typed_facts() {
        let function = function(
            2,
            vec![
                RegInstr::LoadInt { dst: 0, value: 7 },
                RegInstr::CallKnown {
                    dst: 1,
                    function: 99,
                    args: vec![0],
                    mut_args: vec![0],
                },
                RegInstr::Return { src: 1 },
            ],
        );
        let mut facts = facts(&function);
        facts.reg_types[1] = VerifiedStorageType::Int;
        facts.call_sites[1] = Some(VerifiedCallSite {
            target: VerifiedCallTarget::Known(99),
            params: vec![VerifiedStorageType::Int].into_boxed_slice(),
            result: VerifiedStorageType::Int,
            param_effects: vec![VerifiedParamEffect::Mut].into_boxed_slice(),
            type_arguments: Box::new([]),
        });
        let ir = TypedRegionIr::derive(&function, &facts, &[true; 3]).expect("typed call IR");
        let lowered = ir.lower_to_reg_code(&function).expect("native projection");
        assert!(matches!(
            &lowered[1],
            RegInstr::CallKnown {
                dst: 1,
                function: 99,
                args,
                mut_args,
            } if args == &[0] && mut_args == &[0]
        ));
    }
}
