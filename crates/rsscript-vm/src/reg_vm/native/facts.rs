//! Evaluation-local facts derived from a verified v1 register program.
//!
//! The v1 executable payload intentionally does not persist a typed register
//! table.  This module therefore exposes only facts that can be checked again
//! from the decoded `RegUnit`: scalar storage classes, opaque heap handles,
//! call targets/signatures that remain present in v1, and conservative
//! instruction effects.  Nominal layouts, generic substitutions, and precise
//! `read`/`take` effects are never reconstructed by guesswork.

use super::*;

use crate::text_util::strip_fresh_type;

/// Storage class proved for a v1 register across all of its constraints.
///
/// `Handle` deliberately does not claim a nominal language type. `Unknown` is
/// fail-closed: it covers erased generics, conflicting register reuse, and
/// operations for which v1 carries insufficient type evidence.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::reg_vm) enum VerifiedStorageType {
    #[default]
    Unknown,
    Unit,
    Int,
    Bool,
    Float,
    Char,
    Handle,
}

impl VerifiedStorageType {
    pub(in crate::reg_vm) const fn native_ty(self) -> Option<NativeTy> {
        match self {
            Self::Int => Some(NativeTy::Int),
            Self::Bool => Some(NativeTy::Bool),
            Self::Float => Some(NativeTy::Float),
            Self::Handle => Some(NativeTy::Handle),
            Self::Unknown | Self::Unit | Self::Char => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum VerifiedCallTarget {
    Known(usize),
    Dynamic(Box<[usize]>),
    Closure,
    Provider(Box<str>),
    Intrinsic(Box<str>),
}

/// Effect information retained by v1 at an individual call site.
///
/// v1 records `mut_args`, but does not distinguish a non-mutating `read` from
/// `take`; those positions remain `Unknown` rather than being misclassified.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::reg_vm) enum VerifiedParamEffect {
    #[default]
    Unknown,
    Mut,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) struct VerifiedCallSite {
    pub(in crate::reg_vm) target: VerifiedCallTarget,
    pub(in crate::reg_vm) params: Box<[VerifiedStorageType]>,
    pub(in crate::reg_vm) result: VerifiedStorageType,
    pub(in crate::reg_vm) param_effects: Box<[VerifiedParamEffect]>,
    /// Concrete generic arguments admitted by the typed-facts verifier. An
    /// empty slice means unavailable in the current v1 lowering; it must never
    /// be reconstructed from a symbol spelling or runtime value.
    pub(in crate::reg_vm) type_arguments: Box<[WireType]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum VerifiedRegFootprint {
    Exact(Box<[Reg]>),
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) struct VerifiedInstrEffects {
    pub(in crate::reg_vm) reads: VerifiedRegFootprint,
    pub(in crate::reg_vm) writes: VerifiedRegFootprint,
    pub(in crate::reg_vm) writes_heap: bool,
    pub(in crate::reg_vm) may_allocate: bool,
    pub(in crate::reg_vm) may_call_provider: bool,
    pub(in crate::reg_vm) may_suspend: bool,
    pub(in crate::reg_vm) may_spawn: bool,
    pub(in crate::reg_vm) touches_resource: bool,
}

#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct VerifiedFunctionFacts {
    pub(in crate::reg_vm) reg_types: Box<[VerifiedStorageType]>,
    pub(in crate::reg_vm) call_sites: Box<[Option<VerifiedCallSite>]>,
    pub(in crate::reg_vm) effects: Box<[VerifiedInstrEffects]>,
    /// Concrete substitutions for this emitted function instance, when the
    /// lowering retained them. v1 ordinary direct calls currently leave this
    /// empty, so native caches use an explicit unavailable key rather than
    /// pretending that the function is monomorphic.
    pub(in crate::reg_vm) generic_substitutions: Box<[WireType]>,
}

impl VerifiedFunctionFacts {
    pub(in crate::reg_vm) fn call_site(&self, instruction: usize) -> Option<&VerifiedCallSite> {
        self.call_sites.get(instruction).and_then(Option::as_ref)
    }

    pub(in crate::reg_vm) fn native_type_seed(
        &self,
        registers: usize,
        allow_extended: bool,
    ) -> Option<Vec<Option<NativeTy>>> {
        if self.reg_types.len() > registers
            || (!allow_extended && self.reg_types.len() != registers)
        {
            return None;
        }
        Some(
            self.reg_types
                .iter()
                .map(|ty| ty.native_ty())
                .chain(std::iter::repeat_n(None, registers - self.reg_types.len()))
                .collect(),
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::reg_vm) struct VerifiedFactsSummary {
    pub(in crate::reg_vm) known_reg_types: u64,
    pub(in crate::reg_vm) unknown_reg_types: u64,
    pub(in crate::reg_vm) known_call_sites: u64,
    pub(in crate::reg_vm) instruction_effects: u64,
}

#[derive(Clone, Debug)]
pub(in crate::reg_vm) struct VerifiedExecutableFacts {
    functions: Box<[VerifiedFunctionFacts]>,
}

impl VerifiedExecutableFacts {
    /// Derive facts from a `RegUnit` produced by the verified bytecode decoder.
    ///
    /// The API is crate-private so an embedding caller cannot attach unchecked
    /// facts to arbitrary bytes. Structural limits are checked again before any
    /// facts allocation, even though the bytecode verifier already checked the
    /// same executable envelope.
    pub(in crate::reg_vm) fn derive(unit: &RegUnit) -> Result<Self, VerifiedFactsError> {
        Self::derive_with_limits(unit, VerifiedFactsLimits::default())
    }

    /// Project verifier-owned typed facts onto the execution-local facts used
    /// by native lowering. Instruction effects still come from the decoded
    /// executable. Persisted scalar storage claims must agree with an
    /// independently derived class; until full typed data-flow verification
    /// exists they never promote a conservative v1-derived `Unknown`.
    pub(in crate::reg_vm) fn derive_with_typed(
        unit: &RegUnit,
        typed: &rsscript_bytecode::BoundTypedExecutableFactsV1,
    ) -> Result<Self, VerifiedFactsError> {
        let mut derived = Self::derive(unit)?;
        let typed = typed.facts();
        if typed.functions.len() != derived.functions.len() {
            return Err(VerifiedFactsError::TypedFactsMismatch);
        }
        for (function, typed_function) in derived.functions.iter_mut().zip(&typed.functions) {
            if function.reg_types.len() != typed_function.registers.len() {
                return Err(VerifiedFactsError::TypedFactsMismatch);
            }
            for (storage, typed_register) in
                function.reg_types.iter_mut().zip(&typed_function.registers)
            {
                if let Some(proved) = typed_storage_type(&typed_register.ty) {
                    match *storage {
                        VerifiedStorageType::Unknown => {}
                        existing if existing == proved => {}
                        _ => return Err(VerifiedFactsError::TypedFactsMismatch),
                    }
                }
            }
            for typed_call in &typed_function.call_sites {
                let Some(Some(call)) = function.call_sites.get_mut(typed_call.instruction as usize)
                else {
                    return Err(VerifiedFactsError::TypedFactsMismatch);
                };
                if typed_call.parameters.len() != call.params.len()
                    || typed_call.parameter_effects.len() != call.param_effects.len()
                {
                    return Err(VerifiedFactsError::TypedFactsMismatch);
                }
                for (storage, ty) in call.params.iter_mut().zip(&typed_call.parameters) {
                    if let Some(proved) = typed_storage_type(ty) {
                        match *storage {
                            VerifiedStorageType::Unknown => {}
                            existing if existing == proved => {}
                            _ => return Err(VerifiedFactsError::TypedFactsMismatch),
                        }
                    }
                }
                if let Some(proved) = typed_storage_type(&typed_call.result) {
                    match call.result {
                        VerifiedStorageType::Unknown => {}
                        existing if existing == proved => {}
                        _ => return Err(VerifiedFactsError::TypedFactsMismatch),
                    }
                }
                for (effect, typed_effect) in call
                    .param_effects
                    .iter_mut()
                    .zip(&typed_call.parameter_effects)
                {
                    *effect = match typed_effect {
                        rsscript_bytecode::TypedDataEffectV1::Mutate => VerifiedParamEffect::Mut,
                        rsscript_bytecode::TypedDataEffectV1::Read
                        | rsscript_bytecode::TypedDataEffectV1::Take
                        | rsscript_bytecode::TypedDataEffectV1::Unknown => {
                            VerifiedParamEffect::Unknown
                        }
                    };
                }
                call.type_arguments = typed_call.type_arguments.clone().into_boxed_slice();
            }
            if !typed_function.generic_substitutions.is_empty() {
                return Err(VerifiedFactsError::TypedFactsMismatch);
            }
        }
        Ok(derived)
    }

    pub(in crate::reg_vm) fn function(&self, ordinal: usize) -> Option<&VerifiedFunctionFacts> {
        self.functions.get(ordinal)
    }

    pub(in crate::reg_vm) fn summary(&self) -> VerifiedFactsSummary {
        let mut summary = VerifiedFactsSummary::default();
        for function in &self.functions {
            for ty in &function.reg_types {
                if *ty == VerifiedStorageType::Unknown {
                    summary.unknown_reg_types = summary.unknown_reg_types.saturating_add(1);
                } else {
                    summary.known_reg_types = summary.known_reg_types.saturating_add(1);
                }
            }
            summary.known_call_sites = summary.known_call_sites.saturating_add(
                function
                    .call_sites
                    .iter()
                    .filter(|site| site.is_some())
                    .count() as u64,
            );
            summary.instruction_effects = summary
                .instruction_effects
                .saturating_add(function.effects.len() as u64);
        }
        summary
    }

    fn derive_with_limits(
        unit: &RegUnit,
        limits: VerifiedFactsLimits,
    ) -> Result<Self, VerifiedFactsError> {
        if unit.functions.len() > limits.max_functions {
            return Err(VerifiedFactsError::TooManyFunctions);
        }

        let mut total_registers = 0usize;
        let mut total_instructions = 0usize;
        let mut total_operands = 0usize;
        let mut functions = Vec::with_capacity(unit.functions.len());
        for (ordinal, function) in unit.functions.iter().enumerate() {
            if function.regs > limits.max_registers_per_function {
                return Err(VerifiedFactsError::TooManyRegisters { function: ordinal });
            }
            total_registers = total_registers
                .checked_add(function.regs)
                .ok_or(VerifiedFactsError::FactsBudgetExceeded)?;
            if total_registers > limits.max_total_registers {
                return Err(VerifiedFactsError::FactsBudgetExceeded);
            }
            total_instructions = total_instructions
                .checked_add(function.code.len())
                .ok_or(VerifiedFactsError::TooManyInstructions)?;
            if total_instructions > limits.max_instructions {
                return Err(VerifiedFactsError::TooManyInstructions);
            }

            functions.push(derive_function_facts(
                unit,
                ordinal,
                function,
                &mut total_operands,
                limits.max_fact_operands,
            )?);
        }
        Ok(Self {
            functions: functions.into_boxed_slice(),
        })
    }
}

#[derive(Clone, Copy, Debug)]
struct VerifiedFactsLimits {
    max_functions: usize,
    max_registers_per_function: usize,
    max_total_registers: usize,
    max_instructions: usize,
    max_fact_operands: usize,
}

impl Default for VerifiedFactsLimits {
    fn default() -> Self {
        let bytecode = rsscript_bytecode::BytecodeLimits::default();
        Self {
            max_functions: bytecode.max_functions,
            max_registers_per_function: bytecode.max_registers_per_function,
            // A verified v1 payload cannot reasonably encode more independent
            // registers/operand facts than these envelope-derived budgets.
            max_total_registers: bytecode.max_instructions,
            max_instructions: bytecode.max_instructions,
            max_fact_operands: bytecode.max_payload_bytes / std::mem::size_of::<Reg>(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::reg_vm) enum VerifiedFactsError {
    TooManyFunctions,
    TooManyRegisters { function: usize },
    TooManyInstructions,
    FactsBudgetExceeded,
    InvalidRegister { function: usize, register: Reg },
    TypedFactsMismatch,
}

fn typed_storage_type(ty: &rsscript_bytecode::TypedFactTypeV1) -> Option<VerifiedStorageType> {
    use rsscript_abi_model::WireType;
    let rsscript_bytecode::TypedFactTypeV1::Known(ty) = ty else {
        return None;
    };
    fn wire_storage_type(ty: &WireType) -> VerifiedStorageType {
        match ty {
            WireType::Unit => VerifiedStorageType::Unit,
            WireType::Bool => VerifiedStorageType::Bool,
            WireType::Int { .. } => VerifiedStorageType::Int,
            WireType::Float { .. } => VerifiedStorageType::Float,
            WireType::Char => VerifiedStorageType::Char,
            WireType::String
            | WireType::Bytes
            | WireType::List { .. }
            | WireType::Map { .. }
            | WireType::Option { .. }
            | WireType::Result { .. }
            | WireType::Tuple { .. }
            | WireType::Named { .. }
            | WireType::Resource { .. }
            | WireType::Handle { .. } => VerifiedStorageType::Handle,
            WireType::Qualified { value, .. } => wire_storage_type(value),
        }
    }
    Some(wire_storage_type(ty))
}

#[derive(Clone, Copy, Debug)]
enum TypeFact {
    Unset,
    Known(VerifiedStorageType),
    Conflict,
}

#[derive(Debug)]
struct TypeConstraints {
    parent: Vec<usize>,
    rank: Vec<u8>,
    facts: Vec<TypeFact>,
}

impl TypeConstraints {
    fn new(registers: usize) -> Self {
        Self {
            parent: (0..registers).collect(),
            rank: vec![0; registers],
            facts: vec![TypeFact::Unset; registers],
        }
    }

    fn find(&mut self, reg: Reg) -> usize {
        let parent = self.parent[reg];
        if parent != reg {
            let root = self.find(parent);
            self.parent[reg] = root;
        }
        self.parent[reg]
    }

    fn union(&mut self, lhs: Reg, rhs: Reg) {
        let mut lhs = self.find(lhs);
        let mut rhs = self.find(rhs);
        if lhs == rhs {
            return;
        }
        if self.rank[lhs] < self.rank[rhs] {
            std::mem::swap(&mut lhs, &mut rhs);
        }
        self.parent[rhs] = lhs;
        if self.rank[lhs] == self.rank[rhs] {
            self.rank[lhs] = self.rank[lhs].saturating_add(1);
        }
        self.facts[lhs] = merge_type_fact(self.facts[lhs], self.facts[rhs]);
    }

    fn constrain(&mut self, reg: Reg, ty: VerifiedStorageType) {
        if ty == VerifiedStorageType::Unknown {
            return;
        }
        let root = self.find(reg);
        self.facts[root] = merge_type_fact(self.facts[root], TypeFact::Known(ty));
    }

    fn finish(mut self) -> Box<[VerifiedStorageType]> {
        (0..self.parent.len())
            .map(|reg| {
                let root = self.find(reg);
                match self.facts[root] {
                    TypeFact::Known(ty) => ty,
                    TypeFact::Unset | TypeFact::Conflict => VerifiedStorageType::Unknown,
                }
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
    }
}

fn merge_type_fact(lhs: TypeFact, rhs: TypeFact) -> TypeFact {
    match (lhs, rhs) {
        (TypeFact::Conflict, _) | (_, TypeFact::Conflict) => TypeFact::Conflict,
        (TypeFact::Unset, fact) | (fact, TypeFact::Unset) => fact,
        (TypeFact::Known(lhs), TypeFact::Known(rhs)) if lhs == rhs => TypeFact::Known(lhs),
        (TypeFact::Known(_), TypeFact::Known(_)) => TypeFact::Conflict,
    }
}

fn derive_function_facts(
    unit: &RegUnit,
    ordinal: usize,
    function: &RegFunction,
    total_operands: &mut usize,
    max_fact_operands: usize,
) -> Result<VerifiedFunctionFacts, VerifiedFactsError> {
    let mut constraints = TypeConstraints::new(function.regs);
    if let Some(signature) = unit.native_signatures.get(&function.name) {
        for (reg, type_name) in signature.params.iter().enumerate().take(function.params) {
            checked_constrain(
                &mut constraints,
                ordinal,
                function.regs,
                reg,
                storage_type_name(type_name),
            )?;
        }
    }

    let mut call_sites = Vec::with_capacity(function.code.len());
    let mut effects = Vec::with_capacity(function.code.len());
    for instruction in &function.code {
        constrain_instruction(unit, ordinal, function.regs, instruction, &mut constraints)?;
        call_sites.push(call_site(unit, instruction));

        let (read_footprint, write_footprint) = instruction_footprints(instruction);
        let reads = verified_footprint(read_footprint, total_operands, max_fact_operands)?;
        let writes = verified_footprint(write_footprint, total_operands, max_fact_operands)?;
        effects.push(VerifiedInstrEffects {
            reads,
            writes,
            writes_heap: instruction_writes_heap(instruction),
            may_allocate: instruction_may_allocate(instruction),
            may_call_provider: matches!(instruction, RegInstr::CallExternal { .. }),
            may_suspend: matches!(
                instruction,
                RegInstr::AwaitJoin { .. }
                    | RegInstr::JoinTasks { .. }
                    | RegInstr::SelectWait { .. }
            ),
            may_spawn: matches!(instruction, RegInstr::SpawnTask { .. }),
            touches_resource: matches!(
                instruction,
                RegInstr::Manage { .. }
                    | RegInstr::ResourceAcquire { .. }
                    | RegInstr::ResourceDrop { .. }
            ),
        });
    }

    Ok(VerifiedFunctionFacts {
        reg_types: constraints.finish(),
        call_sites: call_sites.into_boxed_slice(),
        effects: effects.into_boxed_slice(),
        generic_substitutions: Box::new([]),
    })
}

fn checked_reg(function: usize, registers: usize, reg: Reg) -> Result<(), VerifiedFactsError> {
    if reg < registers {
        Ok(())
    } else {
        Err(VerifiedFactsError::InvalidRegister {
            function,
            register: reg,
        })
    }
}

fn checked_constrain(
    constraints: &mut TypeConstraints,
    function: usize,
    registers: usize,
    reg: Reg,
    ty: VerifiedStorageType,
) -> Result<(), VerifiedFactsError> {
    checked_reg(function, registers, reg)?;
    constraints.constrain(reg, ty);
    Ok(())
}

fn checked_union(
    constraints: &mut TypeConstraints,
    function: usize,
    registers: usize,
    lhs: Reg,
    rhs: Reg,
) -> Result<(), VerifiedFactsError> {
    checked_reg(function, registers, lhs)?;
    checked_reg(function, registers, rhs)?;
    constraints.union(lhs, rhs);
    Ok(())
}

fn constrain_instruction(
    unit: &RegUnit,
    function: usize,
    registers: usize,
    instruction: &RegInstr,
    constraints: &mut TypeConstraints,
) -> Result<(), VerifiedFactsError> {
    macro_rules! constrain {
        ($reg:expr, $ty:expr) => {
            checked_constrain(constraints, function, registers, $reg, $ty)?
        };
    }
    match instruction {
        RegInstr::LoadUnit { dst } => constrain!(*dst, VerifiedStorageType::Unit),
        RegInstr::LoadInt { dst, .. } => constrain!(*dst, VerifiedStorageType::Int),
        RegInstr::LoadFloat { dst, .. } => constrain!(*dst, VerifiedStorageType::Float),
        RegInstr::LoadBool { dst, .. } => constrain!(*dst, VerifiedStorageType::Bool),
        RegInstr::LoadChar { dst, .. } => constrain!(*dst, VerifiedStorageType::Char),
        RegInstr::LoadString { dst, .. }
        | RegInstr::MakeStruct { dst, .. }
        | RegInstr::MakeVariant { dst, .. }
        | RegInstr::MakeList { dst, .. }
        | RegInstr::MakeObject { dst, .. }
        | RegInstr::MakeMap { dst, .. }
        | RegInstr::MakeClosure { dst, .. } => constrain!(*dst, VerifiedStorageType::Handle),
        RegInstr::Move { dst, src } => {
            checked_union(constraints, function, registers, *dst, *src)?;
        }
        RegInstr::AddInt { dst, lhs, rhs }
        | RegInstr::SubInt { dst, lhs, rhs }
        | RegInstr::MulInt { dst, lhs, rhs }
        | RegInstr::DivInt { dst, lhs, rhs } => {
            checked_union(constraints, function, registers, *lhs, *rhs)?;
            checked_union(constraints, function, registers, *dst, *lhs)?;
        }
        RegInstr::ModInt { dst, lhs, rhs }
        | RegInstr::BitAndInt { dst, lhs, rhs }
        | RegInstr::BitOrInt { dst, lhs, rhs }
        | RegInstr::BitXorInt { dst, lhs, rhs }
        | RegInstr::ShiftLeftInt { dst, lhs, rhs }
        | RegInstr::ShiftRightInt { dst, lhs, rhs } => {
            constrain!(*dst, VerifiedStorageType::Int);
            constrain!(*lhs, VerifiedStorageType::Int);
            constrain!(*rhs, VerifiedStorageType::Int);
        }
        RegInstr::LessInt { dst, lhs, rhs }
        | RegInstr::LessEqualInt { dst, lhs, rhs }
        | RegInstr::GreaterInt { dst, lhs, rhs }
        | RegInstr::GreaterEqualInt { dst, lhs, rhs } => {
            constrain!(*dst, VerifiedStorageType::Bool);
            checked_union(constraints, function, registers, *lhs, *rhs)?;
        }
        RegInstr::Equal { dst, lhs, rhs } | RegInstr::NotEqual { dst, lhs, rhs } => {
            constrain!(*dst, VerifiedStorageType::Bool);
            checked_union(constraints, function, registers, *lhs, *rhs)?;
        }
        RegInstr::JumpIfBool { cond, .. } => constrain!(*cond, VerifiedStorageType::Bool),
        RegInstr::JumpIfIntCompare { lhs, rhs, .. } => {
            constrain!(*lhs, VerifiedStorageType::Int);
            constrain!(*rhs, VerifiedStorageType::Int);
        }
        RegInstr::ListLen { dst, list } => {
            constrain!(*dst, VerifiedStorageType::Int);
            constrain!(*list, VerifiedStorageType::Handle);
        }
        RegInstr::ListGet { list, index, .. } | RegInstr::ListSet { list, index, .. } => {
            constrain!(*list, VerifiedStorageType::Handle);
            constrain!(*index, VerifiedStorageType::Int);
        }
        RegInstr::GetField { base, .. }
        | RegInstr::GetFieldSlot { base, .. }
        | RegInstr::SetField { base, .. }
        | RegInstr::SetFieldSlot { base, .. } => constrain!(*base, VerifiedStorageType::Handle),
        RegInstr::StringConcat { dst, left, right } => {
            constrain!(*dst, VerifiedStorageType::Handle);
            constrain!(*left, VerifiedStorageType::Handle);
            constrain!(*right, VerifiedStorageType::Handle);
        }
        RegInstr::CallKnown {
            dst,
            function: callee,
            args,
            ..
        } => constrain_known_call(
            function,
            registers,
            function_signature(unit, *callee),
            *dst,
            args,
            constraints,
            None,
        )?,
        RegInstr::SpawnTask {
            dst,
            function: callee,
            args,
        } => constrain_known_call(
            function,
            registers,
            function_signature(unit, *callee),
            *dst,
            args,
            constraints,
            Some(VerifiedStorageType::Handle),
        )?,
        RegInstr::CallIntrinsic {
            dst,
            intrinsic,
            args,
        } => constrain_intrinsic(
            function,
            registers,
            *dst,
            *intrinsic,
            None,
            args,
            constraints,
        )?,
        RegInstr::CallTypedIntrinsic {
            dst,
            intrinsic,
            type_arg,
            args,
        } => constrain_intrinsic(
            function,
            registers,
            *dst,
            *intrinsic,
            Some(type_arg),
            args,
            constraints,
        )?,
        RegInstr::Return { src } => {
            if let Some(signature) = unit
                .functions
                .get(function)
                .and_then(|function| unit.native_signatures.get(&function.name))
                .and_then(|signature| signature.return_type.as_deref())
            {
                constrain!(*src, storage_type_name(signature));
            }
        }
        _ => {}
    }
    Ok(())
}

fn constrain_known_call(
    function: usize,
    registers: usize,
    signature: Option<&RegNativeSignature>,
    dst: Reg,
    args: &[Reg],
    constraints: &mut TypeConstraints,
    result_override: Option<VerifiedStorageType>,
) -> Result<(), VerifiedFactsError> {
    checked_reg(function, registers, dst)?;
    for &arg in args {
        checked_reg(function, registers, arg)?;
    }
    if let Some(signature) = signature {
        for (&arg, type_name) in args.iter().zip(&signature.params) {
            constraints.constrain(arg, storage_type_name(type_name));
        }
        if result_override.is_none()
            && let Some(result) = signature.return_type.as_deref()
        {
            constraints.constrain(dst, storage_type_name(result));
        }
    }
    if let Some(result) = result_override {
        constraints.constrain(dst, result);
    }
    Ok(())
}

fn constrain_intrinsic(
    function: usize,
    registers: usize,
    dst: Reg,
    intrinsic: RegIntrinsic,
    type_arg: Option<&String>,
    args: &[Reg],
    constraints: &mut TypeConstraints,
) -> Result<(), VerifiedFactsError> {
    checked_reg(function, registers, dst)?;
    for &arg in args {
        checked_reg(function, registers, arg)?;
    }
    if let Some(spec) = native_host_typed_intrinsic(intrinsic, type_arg.map(String::as_str)) {
        constraints.constrain(dst, storage_type(spec.result_ty));
        for (&arg, ty) in args.iter().zip(spec.arg_tys()) {
            constraints.constrain(arg, storage_type(ty));
        }
    } else {
        match intrinsic {
            RegIntrinsic::IntToFloat if args.len() == 1 => {
                constraints.constrain(dst, VerifiedStorageType::Float);
                constraints.constrain(args[0], VerifiedStorageType::Int);
            }
            RegIntrinsic::MathFloor | RegIntrinsic::MathCeil if args.len() == 1 => {
                constraints.constrain(dst, VerifiedStorageType::Int);
                constraints.constrain(args[0], VerifiedStorageType::Float);
            }
            _ => {}
        }
    }
    Ok(())
}

fn call_site(unit: &RegUnit, instruction: &RegInstr) -> Option<VerifiedCallSite> {
    match instruction {
        RegInstr::CallKnown {
            function,
            args,
            mut_args,
            ..
        } => Some(call_site_from_signature(
            VerifiedCallTarget::Known(*function),
            function_signature(unit, *function),
            args.len(),
            mut_args,
        )),
        RegInstr::SpawnTask { function, args, .. } => {
            let mut call = call_site_from_signature(
                VerifiedCallTarget::Known(*function),
                function_signature(unit, *function),
                args.len(),
                &[],
            );
            call.result = VerifiedStorageType::Handle;
            Some(call)
        }
        RegInstr::CallDynamic {
            dispatch,
            args,
            mut_args,
            ..
        } => {
            let targets = dispatch
                .iter()
                .map(|(_, function)| *function)
                .collect::<Vec<_>>()
                .into_boxed_slice();
            let common = common_signature(unit, &targets);
            Some(call_site_from_signature(
                VerifiedCallTarget::Dynamic(targets),
                common,
                args.len(),
                mut_args,
            ))
        }
        RegInstr::CallClosure { args, mut_args, .. } => Some(call_site_from_signature(
            VerifiedCallTarget::Closure,
            None,
            args.len(),
            mut_args,
        )),
        RegInstr::CallExternal {
            key,
            args,
            mut_args,
            ..
        } => Some(call_site_from_signature(
            VerifiedCallTarget::Provider(key.clone().into_boxed_str()),
            None,
            args.len(),
            mut_args,
        )),
        RegInstr::CallIntrinsic {
            intrinsic, args, ..
        } => Some(intrinsic_call_site(*intrinsic, None, args.len())),
        RegInstr::CallTypedIntrinsic {
            intrinsic,
            type_arg,
            args,
            ..
        } => Some(intrinsic_call_site(
            *intrinsic,
            Some(type_arg.as_str()),
            args.len(),
        )),
        _ => None,
    }
}

fn call_site_from_signature(
    target: VerifiedCallTarget,
    signature: Option<&RegNativeSignature>,
    arg_count: usize,
    mut_args: &[usize],
) -> VerifiedCallSite {
    let params = (0..arg_count)
        .map(|index| {
            signature
                .and_then(|signature| signature.params.get(index))
                .map_or(VerifiedStorageType::Unknown, |ty| storage_type_name(ty))
        })
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let result = signature
        .and_then(|signature| signature.return_type.as_deref())
        .map_or(VerifiedStorageType::Unknown, storage_type_name);
    VerifiedCallSite {
        target,
        params,
        result,
        param_effects: parameter_effects(arg_count, mut_args),
        type_arguments: Box::new([]),
    }
}

fn intrinsic_call_site(
    intrinsic: RegIntrinsic,
    type_arg: Option<&str>,
    arg_count: usize,
) -> VerifiedCallSite {
    let (params, result) = native_host_typed_intrinsic(intrinsic, type_arg)
        .map(|spec| {
            (
                spec.arg_tys()
                    .into_iter()
                    .map(storage_type)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                storage_type(spec.result_ty),
            )
        })
        .unwrap_or_else(|| {
            (
                vec![VerifiedStorageType::Unknown; arg_count].into_boxed_slice(),
                VerifiedStorageType::Unknown,
            )
        });
    VerifiedCallSite {
        target: VerifiedCallTarget::Intrinsic(format!("{intrinsic:?}").into_boxed_str()),
        params,
        result,
        param_effects: vec![VerifiedParamEffect::Unknown; arg_count].into_boxed_slice(),
        type_arguments: Box::new([]),
    }
}

fn parameter_effects(arg_count: usize, mut_args: &[usize]) -> Box<[VerifiedParamEffect]> {
    let mut effects = vec![VerifiedParamEffect::Unknown; arg_count];
    for &index in mut_args {
        if let Some(effect) = effects.get_mut(index) {
            *effect = VerifiedParamEffect::Mut;
        }
    }
    effects.into_boxed_slice()
}

fn function_signature(unit: &RegUnit, function: usize) -> Option<&RegNativeSignature> {
    unit.functions
        .get(function)
        .and_then(|function| unit.native_signatures.get(&function.name))
}

fn common_signature<'a>(unit: &'a RegUnit, targets: &[usize]) -> Option<&'a RegNativeSignature> {
    let first = function_signature(unit, *targets.first()?)?;
    targets
        .iter()
        .skip(1)
        .all(|target| function_signature(unit, *target) == Some(first))
        .then_some(first)
}

fn storage_type_name(type_name: &str) -> VerifiedStorageType {
    let root = type_root_name(strip_fresh_type(type_name));
    match root {
        "Unit" => VerifiedStorageType::Unit,
        "Int" => VerifiedStorageType::Int,
        "Bool" => VerifiedStorageType::Bool,
        "Float" => VerifiedStorageType::Float,
        "Char" => VerifiedStorageType::Char,
        name if name.len() == 1 && name.chars().all(|ch| ch.is_ascii_uppercase()) => {
            VerifiedStorageType::Unknown
        }
        _ => VerifiedStorageType::Handle,
    }
}

fn storage_type(native: NativeTy) -> VerifiedStorageType {
    match native {
        NativeTy::Int => VerifiedStorageType::Int,
        NativeTy::Bool => VerifiedStorageType::Bool,
        NativeTy::Float => VerifiedStorageType::Float,
        NativeTy::Handle
        | NativeTy::FlatInt
        | NativeTy::FlatIntMut
        | NativeTy::FlatFloat
        | NativeTy::FlatFloatMut => VerifiedStorageType::Handle,
    }
}

/// Exact footprints for the compact, high-value v1 core. Instructions whose
/// complete operand contract is not represented here stay `All`; this is an
/// intentional over-approximation, never an optimistic omission.
fn instruction_footprints(instruction: &RegInstr) -> (RegFootprint, RegFootprint) {
    use RegFootprint::{All, Some as Registers};

    match instruction {
        RegInstr::LoadUnit { dst }
        | RegInstr::LoadInt { dst, .. }
        | RegInstr::LoadFloat { dst, .. }
        | RegInstr::LoadBool { dst, .. }
        | RegInstr::LoadString { dst, .. }
        | RegInstr::LoadChar { dst, .. }
        | RegInstr::LoadNone { dst } => (Registers(vec![]), Registers(vec![*dst])),
        RegInstr::Move { dst, src }
        | RegInstr::Manage { dst, src }
        | RegInstr::UnwrapSome { dst, src }
        | RegInstr::UnwrapVariantValue { dst, src, .. }
        | RegInstr::AwaitJoin { dst, src } => (Registers(vec![*src]), Registers(vec![*dst])),
        RegInstr::DeepCopy { reg } | RegInstr::DeepCopyElided { reg } => {
            (Registers(vec![*reg]), Registers(vec![]))
        }
        RegInstr::AddInt { dst, lhs, rhs }
        | RegInstr::SubInt { dst, lhs, rhs }
        | RegInstr::MulInt { dst, lhs, rhs }
        | RegInstr::DivInt { dst, lhs, rhs }
        | RegInstr::ModInt { dst, lhs, rhs }
        | RegInstr::BitAndInt { dst, lhs, rhs }
        | RegInstr::BitOrInt { dst, lhs, rhs }
        | RegInstr::BitXorInt { dst, lhs, rhs }
        | RegInstr::ShiftLeftInt { dst, lhs, rhs }
        | RegInstr::ShiftRightInt { dst, lhs, rhs }
        | RegInstr::LessInt { dst, lhs, rhs }
        | RegInstr::LessEqualInt { dst, lhs, rhs }
        | RegInstr::GreaterInt { dst, lhs, rhs }
        | RegInstr::GreaterEqualInt { dst, lhs, rhs }
        | RegInstr::Equal { dst, lhs, rhs }
        | RegInstr::NotEqual { dst, lhs, rhs } => {
            (Registers(vec![*lhs, *rhs]), Registers(vec![*dst]))
        }
        RegInstr::Jump { .. } | RegInstr::TailCallGuard | RegInstr::RuntimeError { .. } => {
            (Registers(vec![]), Registers(vec![]))
        }
        RegInstr::JumpIfBool { cond, .. } => (Registers(vec![*cond]), Registers(vec![])),
        RegInstr::JumpIfIntCompare { lhs, rhs, .. } => {
            (Registers(vec![*lhs, *rhs]), Registers(vec![]))
        }
        RegInstr::Return { src }
        | RegInstr::ResourceAcquire { resource: src }
        | RegInstr::ResourceDrop { resource: src }
        | RegInstr::CancelTask { src } => (Registers(vec![*src]), Registers(vec![])),
        RegInstr::CallKnown { dst, args, .. }
        | RegInstr::CallDynamic { dst, args, .. }
        | RegInstr::SpawnTask { dst, args, .. }
        | RegInstr::CallExternal { dst, args, .. }
        | RegInstr::CallIntrinsic { dst, args, .. }
        | RegInstr::CallTypedIntrinsic { dst, args, .. } => {
            (Registers(args.clone()), Registers(vec![*dst]))
        }
        RegInstr::CallClosure {
            dst, closure, args, ..
        } => {
            let mut reads = Vec::with_capacity(args.len() + 1);
            reads.push(*closure);
            reads.extend(args.iter().copied());
            (Registers(reads), Registers(vec![*dst]))
        }
        RegInstr::MakeList { dst, items } => (Registers(items.clone()), Registers(vec![*dst])),
        RegInstr::MakeStruct { dst, fields, .. }
        | RegInstr::MakeVariant { dst, fields, .. }
        | RegInstr::MakeObject { dst, fields } => (
            Registers(fields.iter().map(|(_, reg)| *reg).collect()),
            Registers(vec![*dst]),
        ),
        RegInstr::MakeMap { dst, entries } => (
            Registers(
                entries
                    .iter()
                    .flat_map(|(key, value)| [*key, *value])
                    .collect(),
            ),
            Registers(vec![*dst]),
        ),
        RegInstr::MakeClosure { dst, captures, .. } => {
            (Registers(captures.clone()), Registers(vec![*dst]))
        }
        RegInstr::MakeSome { dst, value } => (Registers(vec![*value]), Registers(vec![*dst])),
        RegInstr::GetField { dst, base, .. } | RegInstr::GetFieldSlot { dst, base, .. } => {
            (Registers(vec![*base]), Registers(vec![*dst]))
        }
        RegInstr::SetField {
            dst, base, value, ..
        }
        | RegInstr::SetFieldSlot {
            dst, base, value, ..
        } => (Registers(vec![*base, *value]), Registers(vec![*dst])),
        RegInstr::StringConcat { dst, left, right } => {
            (Registers(vec![*left, *right]), Registers(vec![*dst]))
        }
        _ => (All, All),
    }
}

fn instruction_writes_heap(instruction: &RegInstr) -> bool {
    matches!(
        instruction,
        RegInstr::SetFieldSlot { .. }
            | RegInstr::ListSet { .. }
            | RegInstr::ListPush { .. }
            | RegInstr::ListAppend { .. }
            | RegInstr::ListClear { .. }
            | RegInstr::ListPop { .. }
            | RegInstr::ListSort { .. }
            | RegInstr::ListRemoveAt { .. }
            | RegInstr::MapInsert { .. }
            | RegInstr::SetInsert { .. }
            | RegInstr::SortedSetInsert { .. }
            | RegInstr::SortedMapInsert { .. }
            | RegInstr::DequePushBack { .. }
            | RegInstr::DequePushFront { .. }
            | RegInstr::DequePopFront { .. }
            | RegInstr::DequePopBack { .. }
    )
}

fn verified_footprint(
    footprint: RegFootprint,
    total_operands: &mut usize,
    max_fact_operands: usize,
) -> Result<VerifiedRegFootprint, VerifiedFactsError> {
    match footprint {
        RegFootprint::All => Ok(VerifiedRegFootprint::All),
        RegFootprint::Some(registers) => {
            *total_operands = total_operands
                .checked_add(registers.len())
                .ok_or(VerifiedFactsError::FactsBudgetExceeded)?;
            if *total_operands > max_fact_operands {
                return Err(VerifiedFactsError::FactsBudgetExceeded);
            }
            Ok(VerifiedRegFootprint::Exact(registers.into_boxed_slice()))
        }
    }
}

fn instruction_may_allocate(instruction: &RegInstr) -> bool {
    matches!(
        instruction,
        RegInstr::LoadString { .. }
            | RegInstr::DeepCopy { .. }
            | RegInstr::Manage { .. }
            | RegInstr::MakeStruct { .. }
            | RegInstr::MakeVariant { .. }
            | RegInstr::MakeList { .. }
            | RegInstr::MakeObject { .. }
            | RegInstr::MakeMap { .. }
            | RegInstr::MakeClosure { .. }
            | RegInstr::MakeSome { .. }
            | RegInstr::StringConcat { .. }
            | RegInstr::CallKnown { .. }
            | RegInstr::CallDynamic { .. }
            | RegInstr::CallExternal { .. }
            | RegInstr::CallClosure { .. }
            | RegInstr::CallIntrinsic { .. }
            | RegInstr::CallTypedIntrinsic { .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serde_json;
    use std::collections::HashMap;
    use std::rc::Rc;

    fn function(name: &str, params: usize, regs: usize, code: Vec<RegInstr>) -> Rc<RegFunction> {
        Rc::new(RegFunction {
            name: name.to_owned(),
            params,
            captures: 0,
            regs,
            local_regs: HashMap::new(),
            code,
        })
    }

    fn unit(functions: Vec<Rc<RegFunction>>) -> RegUnit {
        RegUnit {
            functions,
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            variant_layouts: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        }
    }

    #[test]
    fn derives_scalar_storage_without_runtime_shapes() {
        let mut unit = unit(vec![function(
            "main",
            1,
            4,
            vec![
                RegInstr::LoadInt { dst: 1, value: 2 },
                RegInstr::Move { dst: 2, src: 0 },
                RegInstr::AddInt {
                    dst: 3,
                    lhs: 2,
                    rhs: 1,
                },
                RegInstr::Return { src: 3 },
            ],
        )]);
        unit.native_signatures.insert(
            "main".into(),
            RegNativeSignature {
                params: vec!["Int".into()],
                return_type: Some("Int".into()),
            },
        );

        let facts = VerifiedExecutableFacts::derive(&unit).expect("derive facts");
        assert_eq!(
            facts.function(0).expect("function").reg_types.as_ref(),
            [
                VerifiedStorageType::Int,
                VerifiedStorageType::Int,
                VerifiedStorageType::Int,
                VerifiedStorageType::Int,
            ]
        );
    }

    #[test]
    fn derives_float_arithmetic_from_the_static_signature() {
        let mut unit = unit(vec![function(
            "sum",
            2,
            3,
            vec![
                RegInstr::AddInt {
                    dst: 2,
                    lhs: 0,
                    rhs: 1,
                },
                RegInstr::Return { src: 2 },
            ],
        )]);
        unit.native_signatures.insert(
            "sum".into(),
            RegNativeSignature {
                params: vec!["Float".into(), "Float".into()],
                return_type: Some("Float".into()),
            },
        );

        let facts = VerifiedExecutableFacts::derive(&unit).expect("derive facts");
        assert_eq!(
            facts.function(0).expect("function").reg_types.as_ref(),
            [
                VerifiedStorageType::Float,
                VerifiedStorageType::Float,
                VerifiedStorageType::Float,
            ]
        );
    }

    #[test]
    fn option_representation_stays_unknown_without_static_evidence() {
        let unit = unit(vec![function(
            "main",
            0,
            3,
            vec![
                RegInstr::LoadInt { dst: 0, value: 1 },
                RegInstr::MakeSome { dst: 1, value: 0 },
                RegInstr::LoadNone { dst: 2 },
            ],
        )]);

        let facts = VerifiedExecutableFacts::derive(&unit).expect("derive facts");
        assert_eq!(
            facts.function(0).expect("function").reg_types[1],
            VerifiedStorageType::Unknown
        );
        assert_eq!(
            facts.function(0).expect("function").reg_types[2],
            VerifiedStorageType::Unknown
        );
    }

    #[test]
    fn preserves_known_call_signature_and_only_observable_mut_effects() {
        let mut unit = unit(vec![
            function(
                "caller",
                0,
                3,
                vec![RegInstr::CallKnown {
                    dst: 2,
                    function: 1,
                    args: vec![0, 1],
                    mut_args: vec![1],
                }],
            ),
            function("callee", 2, 2, vec![]),
        ]);
        unit.native_signatures.insert(
            "callee".into(),
            RegNativeSignature {
                params: vec!["Int".into(), "List<Int>".into()],
                return_type: Some("Bool".into()),
            },
        );

        let facts = VerifiedExecutableFacts::derive(&unit).expect("derive facts");
        let call = facts.function(0).expect("caller").call_sites[0]
            .as_ref()
            .expect("call facts");
        assert_eq!(call.target, VerifiedCallTarget::Known(1));
        assert_eq!(
            call.params.as_ref(),
            [VerifiedStorageType::Int, VerifiedStorageType::Handle]
        );
        assert_eq!(call.result, VerifiedStorageType::Bool);
        assert_eq!(
            call.param_effects.as_ref(),
            [VerifiedParamEffect::Unknown, VerifiedParamEffect::Mut]
        );
    }

    #[test]
    fn conflicting_register_reuse_is_unknown() {
        let unit = unit(vec![function(
            "main",
            0,
            1,
            vec![
                RegInstr::LoadInt { dst: 0, value: 1 },
                RegInstr::LoadBool {
                    dst: 0,
                    value: true,
                },
            ],
        )]);

        let facts = VerifiedExecutableFacts::derive(&unit).expect("derive facts");
        assert_eq!(
            facts.function(0).expect("function").reg_types[0],
            VerifiedStorageType::Unknown
        );
    }

    #[test]
    fn derives_conservative_instruction_effects() {
        let unit = unit(vec![function(
            "main",
            0,
            2,
            vec![RegInstr::CallExternal {
                dst: 1,
                key: "log.write".into(),
                args: vec![0],
                mut_args: vec![],
            }],
        )]);

        let facts = VerifiedExecutableFacts::derive(&unit).expect("derive facts");
        let effect = &facts.function(0).expect("function").effects[0];
        assert!(effect.may_call_provider);
        assert!(effect.may_allocate);
        assert_eq!(effect.reads, VerifiedRegFootprint::Exact(vec![0].into()));
        assert_eq!(effect.writes, VerifiedRegFootprint::Exact(vec![1].into()));
    }

    #[test]
    fn rejects_before_allocating_beyond_configured_limits() {
        let unit = unit(vec![function("main", 0, 2, vec![])]);
        let error = VerifiedExecutableFacts::derive_with_limits(
            &unit,
            VerifiedFactsLimits {
                max_functions: 1,
                max_registers_per_function: 1,
                max_total_registers: 1,
                max_instructions: 1,
                max_fact_operands: 1,
            },
        )
        .expect_err("register limit must reject");
        assert_eq!(error, VerifiedFactsError::TooManyRegisters { function: 0 });
    }

    #[test]
    fn persisted_type_claim_conflict_is_rejected_before_vm_projection() {
        let payload = rsscript_bytecode::encode_executable_payload(&serde_json::json!({
            "functions": [{
                "name": "main", "params": 0, "captures": 0, "regs": 1,
                "local_regs": {},
                "code": [
                    {"LoadInt": {"dst": 0, "value": 7}},
                    {"Return": {"src": 0}}
                ]
            }],
            "function_ids": {"main": 0},
            "resource_drop_functions": {},
            "types": {},
            "native_signatures": {"main": {"params": [], "return_type": "Int"}},
            "closure_identity_observable": false
        }))
        .expect("payload");
        let mut artifact = rsscript_bytecode::BytecodeArtifact::new(
            "0.1.0",
            "0.1.0",
            format!("sha256:{}", "a".repeat(64)),
            rsscript_abi_model::RUNTIME_ABI_VERSION,
            format!("sha256:{}", "b".repeat(64)),
            vec![],
            payload,
        )
        .expect("artifact");
        let facts = rsscript_bytecode::TypedExecutableFactsV1 {
            schema: rsscript_bytecode::TYPED_EXECUTABLE_FACTS_SCHEMA_V1.to_owned(),
            executable_hash: artifact.header.executable_hash.clone(),
            bytecode_isa_version: artifact.header.bytecode_isa_version,
            runtime_abi_version: artifact.header.runtime_abi_version,
            interface_catalog_digest: artifact.header.interface_catalog_digest.clone(),
            imports_hash: rsscript_bytecode::typed_facts_imports_hash(&artifact)
                .expect("imports hash"),
            functions: vec![rsscript_bytecode::TypedFunctionFactsV1 {
                function_ordinal: 0,
                registers: vec![rsscript_bytecode::TypedRegisterFactV1 {
                    ty: rsscript_bytecode::TypedFactTypeV1::Known(
                        rsscript_abi_model::WireType::Float { bits: 64 },
                    ),
                    ownership: rsscript_bytecode::TypedValueOwnershipV1::Copy,
                }],
                call_sites: vec![],
                generic_substitutions: vec![],
            }],
            layouts: vec![],
        };
        artifact
            .attach_typed_executable_facts(&facts)
            .expect("attach facts");
        let error = rsscript_bytecode::BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("artifact bytes"))
            .expect_err("conflicting storage claim must fail before VM projection");
        assert!(
            matches!(
                error,
                rsscript_bytecode::BytecodeError::InvalidTypedExecutableFacts(_)
            ),
            "unexpected verifier error: {error:?}"
        );
    }
}
