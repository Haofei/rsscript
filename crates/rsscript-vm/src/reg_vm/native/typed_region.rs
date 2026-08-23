//! Evaluation-local typed region facts.
//!
//! This is intentionally a projection of [`VerifiedFunctionFacts`], not a new
//! language IR.  It gives native passes dense value identities, conservative
//! ownership/alias facts, and typed slot accesses without importing frontend
//! types or claiming nominal identities erased by bytecode v1.

use super::*;

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
                    if reg < function.params {
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

    pub(in crate::reg_vm) fn contains_instruction(&self, ip: usize) -> bool {
        self.included.get(ip).copied().unwrap_or(false)
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
    }
}
