//! Typed executable-facts derivation and wire-signature helpers, split out
//! of `lib.rs` for module-size partitioning. Pure MIR-analysis phase; emits
//! the same typed facts and changes no emitted Artifact bytes.

use super::*;

/// Project the static facts that survive the current MIR into an optional,
/// digest-bound bytecode side section. This deliberately emits `Unknown` when
/// MIR v1 does not retain a proof (notably ordinary generic substitutions and
/// closure return types); the JIT must never recover those facts from names.
pub(super) fn typed_executable_facts(
    mir: &MirModule,
    wire: &Value,
    artifact: &BytecodeArtifact,
) -> Result<TypedExecutableFactsV1, CodegenError> {
    let wire_functions = wire
        .get("functions")
        .and_then(Value::as_array)
        .ok_or_else(|| CodegenError::InvalidMir("generated unit has no functions".to_owned()))?;
    let functions = mir
        .functions()
        .iter()
        .zip(wire_functions)
        .enumerate()
        .map(|(ordinal, (function, wire_function))| {
            typed_function_facts(mir, function, wire_function, ordinal, artifact)
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut layouts = Vec::new();
    for layout in mir.type_layouts() {
        layouts.push(TypedLayoutV1 {
            layout_id: layouts.len() as u32,
            name: layout.name().to_owned(),
            kind: TypedLayoutKindV1::Record,
            fields: layout
                .fields()
                .iter()
                .map(|(name, ty)| TypedLayoutFieldV1 {
                    case: None,
                    name: name.clone(),
                    ty: fact_type(mir.ty(*ty)),
                })
                .collect(),
        });
    }
    for layout in mir.variant_layouts() {
        layouts.push(TypedLayoutV1 {
            layout_id: layouts.len() as u32,
            name: layout.name().to_owned(),
            kind: TypedLayoutKindV1::Variant,
            fields: layout
                .variants()
                .iter()
                .flat_map(|case| {
                    case.fields()
                        .iter()
                        .map(move |(name, ty)| TypedLayoutFieldV1 {
                            case: Some(case.name().to_owned()),
                            name: name.clone(),
                            ty: fact_type(mir.ty(*ty)),
                        })
                })
                .collect(),
        });
    }

    layouts.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then_with(|| layout_kind_rank(left.kind).cmp(&layout_kind_rank(right.kind)))
    });
    for (ordinal, layout) in layouts.iter_mut().enumerate() {
        layout.layout_id = ordinal as u32;
    }

    Ok(TypedExecutableFactsV1 {
        schema: TYPED_EXECUTABLE_FACTS_SCHEMA_V2.to_owned(),
        executable_hash: artifact.header.executable_hash.clone(),
        bytecode_isa_version: artifact.header.bytecode_isa_version,
        runtime_abi_version: artifact.header.runtime_abi_version,
        interface_catalog_digest: artifact.header.interface_catalog_digest.clone(),
        imports_hash: rsscript_bytecode::typed_facts_imports_hash(artifact)
            .map_err(bytecode_error)?,
        functions,
        layouts,
    })
}

fn layout_kind_rank(kind: TypedLayoutKindV1) -> u8 {
    match kind {
        TypedLayoutKindV1::Record => 0,
        TypedLayoutKindV1::Variant => 1,
    }
}

fn typed_function_facts(
    mir: &MirModule,
    function: &MirFunction,
    wire: &Value,
    ordinal: usize,
    artifact: &BytecodeArtifact,
) -> Result<TypedFunctionFactsV1, CodegenError> {
    let reg_count = wire
        .get("regs")
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(|| {
            CodegenError::InvalidMir("generated register count is invalid".to_owned())
        })?;
    let mut registers = vec![unknown_register(); reg_count];

    for (index, capture) in function.captures().iter().enumerate() {
        registers[index] = TypedRegisterFactV1 {
            ty: fact_type(mir.ty(capture.ty())),
            ownership: ownership(capture.mode()),
        };
    }
    let parameter_start = function.captures().len();
    for (index, (ty, mode)) in function
        .signature()
        .parameter_types()
        .iter()
        .zip(function.signature().parameter_modes())
        .enumerate()
    {
        registers[parameter_start + index] = TypedRegisterFactV1 {
            ty: fact_type(mir.ty(*ty)),
            ownership: ownership(*mode),
        };
    }
    registers[scratch_reg(function)] = TypedRegisterFactV1 {
        ty: TypedFactTypeV1::Known(WireType::Unit),
        ownership: TypedValueOwnershipV1::Copy,
    };

    // MIR v1 has no explicit value-type table. Preserve the exact facts carried
    // by constructors and signatures, then perform monotone copy/result
    // propagation with a dependency worklist. Every register moves at most
    // `Unknown -> Known -> Conflict`, so adversarial block order cannot cause
    // unbounded rescans.
    let mut conflicted = vec![false; registers.len()];
    let instructions = function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .collect::<Vec<_>>();
    let mut dependents = vec![Vec::<usize>::new(); registers.len()];
    let mut dependency_edges = 0usize;
    for (instruction_index, instruction) in instructions.iter().enumerate() {
        for input in fact_input_registers(function, instruction) {
            if let Some(dependents) = dependents.get_mut(input) {
                dependents.push(instruction_index);
                dependency_edges = dependency_edges.checked_add(1).ok_or_else(|| {
                    CodegenError::InvalidMir("typed-facts work budget overflow".to_owned())
                })?;
            }
        }
    }
    let mut pending = (0..instructions.len()).collect::<VecDeque<_>>();
    let mut queued = vec![true; instructions.len()];
    let max_work = instructions
        .len()
        .checked_add(dependency_edges.checked_mul(2).ok_or_else(|| {
            CodegenError::InvalidMir("typed-facts work budget overflow".to_owned())
        })?)
        .ok_or_else(|| CodegenError::InvalidMir("typed-facts work budget overflow".to_owned()))?;
    let mut work = 0usize;
    while let Some(instruction_index) = pending.pop_front() {
        work = work.checked_add(1).ok_or_else(|| {
            CodegenError::InvalidMir("typed-facts work budget overflow".to_owned())
        })?;
        if work > max_work {
            return Err(CodegenError::InvalidMir(
                "typed-facts work budget exceeded".to_owned(),
            ));
        }
        queued[instruction_index] = false;
        let instruction = instructions[instruction_index];
        let output = fact_output_register(function, instruction);
        let before = output.and_then(|register| registers.get(register)).cloned();
        apply_instruction_facts(mir, function, instruction, &mut registers, &mut conflicted);
        if let Some(output) = output.filter(|register| before.as_ref() != registers.get(*register))
        {
            for &dependent in &dependents[output] {
                if !queued[dependent] {
                    pending.push_back(dependent);
                    queued[dependent] = true;
                }
            }
        }
    }

    let code = wire
        .get("code")
        .and_then(Value::as_array)
        .ok_or_else(|| CodegenError::InvalidMir("generated function code is invalid".to_owned()))?;
    let call_ips = code
        .iter()
        .enumerate()
        .filter_map(|(ip, instruction)| {
            let opcode = instruction.as_object()?.keys().next()?;
            (opcode.starts_with("Call") || opcode == "SpawnTask").then_some(ip as u32)
        })
        .collect::<Vec<_>>();
    let calls = function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| typed_call_site(mir, artifact, instruction))
        .collect::<Vec<_>>();
    if calls.len() != call_ips.len() {
        return Err(CodegenError::InvalidMir(
            "typed call facts do not match emitted call instructions".to_owned(),
        ));
    }
    let call_sites = calls
        .into_iter()
        .zip(call_ips)
        .map(|(mut call, instruction)| {
            call.instruction = instruction;
            call
        })
        .collect();

    Ok(TypedFunctionFactsV1 {
        function_ordinal: ordinal as u32,
        registers,
        call_sites,
        // Function-level substitutions are not a sound model: one generic
        // function may be called with multiple instantiations. Concrete
        // substitutions live on each direct call site instead.
        generic_substitutions: Vec::new(),
    })
}

fn unknown_register() -> TypedRegisterFactV1 {
    TypedRegisterFactV1 {
        ty: TypedFactTypeV1::Unknown,
        ownership: TypedValueOwnershipV1::Unknown,
    }
}

pub(super) fn fact_type(ty: Option<&WireType>) -> TypedFactTypeV1 {
    ty.cloned()
        .map(TypedFactTypeV1::Known)
        .unwrap_or(TypedFactTypeV1::Unknown)
}

pub(super) fn ownership(mode: MirParameterMode) -> TypedValueOwnershipV1 {
    match mode {
        MirParameterMode::Read => TypedValueOwnershipV1::ReadBorrow,
        MirParameterMode::Mut => TypedValueOwnershipV1::UniqueBorrow,
        MirParameterMode::Take => TypedValueOwnershipV1::Owned,
    }
}

fn copy_type(ty: WireType) -> TypedFactTypeV1 {
    TypedFactTypeV1::Known(ty)
}

fn int_type() -> WireType {
    WireType::Int {
        bits: 64,
        signed: true,
    }
}

fn float_type() -> WireType {
    WireType::Float { bits: 64 }
}

fn set_register_type(
    registers: &mut [TypedRegisterFactV1],
    conflicted: &mut [bool],
    register: usize,
    candidate: TypedFactTypeV1,
    ownership: Option<TypedValueOwnershipV1>,
) {
    let Some(current) = registers.get_mut(register) else {
        return;
    };
    if conflicted.get(register).copied().unwrap_or(true) {
        return;
    }
    match (&current.ty, &candidate) {
        (TypedFactTypeV1::Unknown, _) => current.ty = candidate,
        (TypedFactTypeV1::Known(left), TypedFactTypeV1::Known(right)) if left == right => {}
        (_, TypedFactTypeV1::Unknown) => {}
        _ => {
            current.ty = TypedFactTypeV1::Unknown;
            conflicted[register] = true;
        }
    }
    if let Some(ownership) = ownership {
        if current.ownership == TypedValueOwnershipV1::Unknown || current.ownership == ownership {
            current.ownership = ownership;
        } else {
            current.ownership = TypedValueOwnershipV1::Unknown;
            conflicted[register] = true;
        }
    }
}

fn register_type(registers: &[TypedRegisterFactV1], register: usize) -> TypedFactTypeV1 {
    registers
        .get(register)
        .map(|fact| fact.ty.clone())
        .unwrap_or(TypedFactTypeV1::Unknown)
}

fn fact_input_registers(function: &MirFunction, instruction: &MirInstruction) -> Vec<usize> {
    let value = |id| value_reg(function, id);
    match instruction {
        MirInstruction::MakeList { items, .. } => items.iter().map(|item| value(*item)).collect(),
        MirInstruction::ReadPlace { place, .. }
        | MirInstruction::BorrowRead { place, .. }
        | MirInstruction::TakePlace { place, .. } => vec![place_reg(*place)],
        MirInstruction::WritePlace { value: source, .. } => vec![value(*source)],
        MirInstruction::Binary { left, right, .. } => vec![value(*left), value(*right)],
        _ => Vec::new(),
    }
}

fn fact_output_register(function: &MirFunction, instruction: &MirInstruction) -> Option<usize> {
    let value = |id| value_reg(function, id);
    match instruction {
        MirInstruction::LoadLiteral { destination, .. }
        | MirInstruction::MakeList { destination, .. }
        | MirInstruction::MakeStruct { destination, .. }
        | MirInstruction::MakeVariant { destination, .. }
        | MirInstruction::ReadPlace { destination, .. }
        | MirInstruction::BorrowRead { destination, .. }
        | MirInstruction::TakePlace { destination, .. }
        | MirInstruction::StringConcat { destination, .. }
        | MirInstruction::StringBuilderFinish { destination, .. }
        | MirInstruction::ListLen { destination, .. }
        | MirInstruction::Binary { destination, .. }
        | MirInstruction::Call { destination, .. } => Some(value(*destination)),
        MirInstruction::WritePlace { place, .. } => Some(place_reg(*place)),
        _ => None,
    }
}

fn apply_instruction_facts(
    mir: &MirModule,
    function: &MirFunction,
    instruction: &MirInstruction,
    registers: &mut [TypedRegisterFactV1],
    conflicted: &mut [bool],
) {
    let value = |id| value_reg(function, id);
    match instruction {
        MirInstruction::LoadLiteral {
            destination,
            value: literal,
        } => {
            let (ty, ownership) = match literal {
                MirLiteral::Unit => (WireType::Unit, TypedValueOwnershipV1::Copy),
                MirLiteral::Int(_) => (int_type(), TypedValueOwnershipV1::Copy),
                MirLiteral::Float(_) => (float_type(), TypedValueOwnershipV1::Copy),
                MirLiteral::Bool(_) => (WireType::Bool, TypedValueOwnershipV1::Copy),
                MirLiteral::String(_) => (WireType::String, TypedValueOwnershipV1::Owned),
                MirLiteral::Char(_) => (WireType::Char, TypedValueOwnershipV1::Copy),
            };
            set_register_type(
                registers,
                conflicted,
                value(*destination),
                copy_type(ty),
                Some(ownership),
            );
        }
        MirInstruction::MakeStruct {
            destination, ty, ..
        }
        | MirInstruction::MakeVariant {
            destination, ty, ..
        } => set_register_type(
            registers,
            conflicted,
            value(*destination),
            fact_type(mir.ty(*ty)),
            Some(TypedValueOwnershipV1::Owned),
        ),
        MirInstruction::MakeList { destination, items } => {
            let first = items
                .first()
                .map(|item| register_type(registers, value(*item)));
            let element = first.filter(|first| {
                items
                    .iter()
                    .all(|item| register_type(registers, value(*item)) == *first)
            });
            let ty = match element {
                Some(TypedFactTypeV1::Known(element)) => copy_type(WireType::List {
                    element: Box::new(element),
                }),
                _ => TypedFactTypeV1::Unknown,
            };
            set_register_type(
                registers,
                conflicted,
                value(*destination),
                ty,
                Some(TypedValueOwnershipV1::Owned),
            );
        }
        MirInstruction::ReadPlace { destination, place } => set_register_type(
            registers,
            conflicted,
            value(*destination),
            register_type(registers, place_reg(*place)),
            Some(TypedValueOwnershipV1::Shared),
        ),
        MirInstruction::BorrowRead { destination, place } => set_register_type(
            registers,
            conflicted,
            value(*destination),
            register_type(registers, place_reg(*place)),
            Some(TypedValueOwnershipV1::ReadBorrow),
        ),
        MirInstruction::TakePlace { destination, place } => set_register_type(
            registers,
            conflicted,
            value(*destination),
            register_type(registers, place_reg(*place)),
            Some(TypedValueOwnershipV1::Owned),
        ),
        MirInstruction::WritePlace {
            place,
            value: source,
        } => set_register_type(
            registers,
            conflicted,
            place_reg(*place),
            register_type(registers, value(*source)),
            None,
        ),
        MirInstruction::StringConcat { destination, .. }
        | MirInstruction::StringBuilderFinish { destination, .. } => set_register_type(
            registers,
            conflicted,
            value(*destination),
            copy_type(WireType::String),
            Some(TypedValueOwnershipV1::Owned),
        ),
        MirInstruction::ListLen { destination, .. } => set_register_type(
            registers,
            conflicted,
            value(*destination),
            copy_type(int_type()),
            Some(TypedValueOwnershipV1::Copy),
        ),
        MirInstruction::Binary {
            destination,
            op,
            left,
            right,
        } => {
            let ty = match op {
                MirBinaryOp::Equal
                | MirBinaryOp::NotEqual
                | MirBinaryOp::Less
                | MirBinaryOp::LessEqual
                | MirBinaryOp::Greater
                | MirBinaryOp::GreaterEqual
                | MirBinaryOp::LogicalAnd
                | MirBinaryOp::LogicalOr => copy_type(WireType::Bool),
                _ => {
                    let left = register_type(registers, value(*left));
                    if left == register_type(registers, value(*right)) {
                        left
                    } else {
                        TypedFactTypeV1::Unknown
                    }
                }
            };
            set_register_type(
                registers,
                conflicted,
                value(*destination),
                ty,
                Some(TypedValueOwnershipV1::Copy),
            );
        }
        MirInstruction::Call {
            destination,
            target,
            ..
        } => {
            let result = call_target_signature(mir, target)
                .map(|signature| TypedFactTypeV1::Known(signature.result))
                .unwrap_or(TypedFactTypeV1::Unknown);
            set_register_type(registers, conflicted, value(*destination), result, None);
        }
        _ => {}
    }
}

fn typed_call_site(
    mir: &MirModule,
    artifact: &BytecodeArtifact,
    instruction: &MirInstruction,
) -> Option<TypedCallSiteV1> {
    match instruction {
        MirInstruction::Call {
            target, arguments, ..
        } => {
            let proven = call_target_signature(mir, target);
            let parameters = proven
                .as_ref()
                .map(|value| value.parameters.clone())
                .unwrap_or_else(|| vec![WireType::Unit; arguments.len()]);
            let result = proven
                .as_ref()
                .map(|value| TypedFactTypeV1::Known(value.result.clone()))
                .unwrap_or(TypedFactTypeV1::Unknown);
            let effects = proven
                .as_ref()
                .map(|value| value.effects.clone())
                .unwrap_or_else(|| call_target_effects(target));
            let (type_parameters, type_arguments) = match target {
                MirCallTarget::Builtin { type_arguments, .. } => (
                    Vec::new(),
                    type_arguments
                        .iter()
                        .filter_map(|id| mir.ty(*id).cloned())
                        .collect(),
                ),
                MirCallTarget::FunctionInstance {
                    type_substitutions, ..
                } => (
                    type_substitutions
                        .iter()
                        .filter_map(|(parameter, _)| mir.ty(*parameter).cloned())
                        .collect(),
                    type_substitutions
                        .iter()
                        .filter_map(|(_, argument)| mir.ty(*argument).cloned())
                        .collect(),
                ),
                _ => (Vec::new(), Vec::new()),
            };
            Some(TypedCallSiteV1 {
                instruction: 0,
                target: match target {
                    MirCallTarget::Function(id) => {
                        TypedCallTargetV1::KnownFunction(id.index() as u32)
                    }
                    MirCallTarget::FunctionInstance { function, .. } => {
                        TypedCallTargetV1::KnownFunction(function.index() as u32)
                    }
                    MirCallTarget::Builtin { id, .. } => TypedCallTargetV1::Builtin(
                        builtin_descriptor(*id)
                            .map(|descriptor| descriptor.vm_name.to_owned())
                            .unwrap_or_default(),
                    ),
                    MirCallTarget::External(id) => {
                        let symbol = mir.external_imports().get(id.index())?.symbol();
                        let ordinal = artifact
                            .imports
                            .iter()
                            .position(|import| &import.symbol == symbol)?;
                        TypedCallTargetV1::Provider(ordinal as u32)
                    }
                    MirCallTarget::Dynamic { .. } => TypedCallTargetV1::Dynamic,
                },
                parameters: if proven.is_some() {
                    parameters.into_iter().map(copy_type).collect()
                } else {
                    vec![TypedFactTypeV1::Unknown; arguments.len()]
                },
                result,
                parameter_effects: effects,
                type_parameters,
                type_arguments,
            })
        }
        MirInstruction::CallClosure {
            parameter_types,
            parameter_modes,
            ..
        } => Some(TypedCallSiteV1 {
            instruction: 0,
            target: TypedCallTargetV1::Closure,
            parameters: parameter_types
                .iter()
                .map(|id| fact_type(mir.ty(*id)))
                .collect(),
            result: TypedFactTypeV1::Unknown,
            parameter_effects: parameter_modes.iter().copied().map(effect).collect(),
            type_parameters: Vec::new(),
            type_arguments: Vec::new(),
        }),
        MirInstruction::Spawn { target, .. } => {
            let callee = mir.function(*target)?;
            Some(TypedCallSiteV1 {
                instruction: 0,
                target: TypedCallTargetV1::KnownFunction(target.index() as u32),
                parameters: callee
                    .signature()
                    .parameter_types()
                    .iter()
                    .map(|id| fact_type(mir.ty(*id)))
                    .collect(),
                result: fact_type(mir.ty(callee.signature().result())),
                parameter_effects: callee
                    .signature()
                    .parameter_modes()
                    .iter()
                    .copied()
                    .map(effect)
                    .collect(),
                type_parameters: Vec::new(),
                type_arguments: Vec::new(),
            })
        }
        _ => None,
    }
}

struct ProvenCallSignature {
    parameters: Vec<WireType>,
    result: WireType,
    effects: Vec<TypedDataEffectV1>,
}

fn call_target_signature(mir: &MirModule, target: &MirCallTarget) -> Option<ProvenCallSignature> {
    match target {
        MirCallTarget::Function(id) => {
            let function = mir.function(*id)?;
            Some(ProvenCallSignature {
                parameters: function
                    .signature()
                    .parameter_types()
                    .iter()
                    .map(|id| mir.ty(*id).cloned())
                    .collect::<Option<Vec<_>>>()?,
                result: mir.ty(function.signature().result())?.clone(),
                effects: function
                    .signature()
                    .parameter_modes()
                    .iter()
                    .copied()
                    .map(effect)
                    .collect(),
            })
        }
        MirCallTarget::FunctionInstance {
            function: id,
            type_substitutions,
        } => {
            let function = mir.function(*id)?;
            let substitutions = type_substitutions
                .iter()
                .map(|(parameter, argument)| {
                    Some((mir.ty(*parameter)?.clone(), mir.ty(*argument)?.clone()))
                })
                .collect::<Option<Vec<_>>>()?;
            Some(ProvenCallSignature {
                parameters: function
                    .signature()
                    .parameter_types()
                    .iter()
                    .map(|id| {
                        mir.ty(*id)
                            .map(|ty| substitute_wire_type(ty, &substitutions))
                    })
                    .collect::<Option<Vec<_>>>()?,
                result: substitute_wire_type(
                    mir.ty(function.signature().result())?,
                    &substitutions,
                ),
                effects: function
                    .signature()
                    .parameter_modes()
                    .iter()
                    .copied()
                    .map(effect)
                    .collect(),
            })
        }
        MirCallTarget::External(id) => {
            let signature = mir.external_imports().get(id.index())?.signature();
            Some(ProvenCallSignature {
                parameters: signature
                    .parameters
                    .iter()
                    .map(|parameter| parameter.ty.clone())
                    .collect(),
                result: signature.result.clone(),
                effects: signature
                    .parameters
                    .iter()
                    .map(|parameter| match parameter.effect {
                        DataEffect::Read => TypedDataEffectV1::Read,
                        DataEffect::Mut => TypedDataEffectV1::Mutate,
                        DataEffect::Take => TypedDataEffectV1::Take,
                    })
                    .collect(),
            })
        }
        MirCallTarget::Dynamic {
            dispatch,
            parameter_modes,
        } => {
            let (_, function) = dispatch.first()?;
            let signature = mir.function(*function)?.signature();
            Some(ProvenCallSignature {
                parameters: signature
                    .parameter_types()
                    .iter()
                    .map(|id| mir.ty(*id).cloned())
                    .collect::<Option<Vec<_>>>()?,
                result: mir.ty(signature.result())?.clone(),
                effects: parameter_modes.iter().copied().map(effect).collect(),
            })
        }
        MirCallTarget::Builtin { .. } => None,
    }
}

fn substitute_wire_type(ty: &WireType, substitutions: &[(WireType, WireType)]) -> WireType {
    if let Some((_, concrete)) = substitutions.iter().find(|(parameter, _)| parameter == ty) {
        return concrete.clone();
    }
    match ty {
        WireType::List { element } => WireType::List {
            element: Box::new(substitute_wire_type(element, substitutions)),
        },
        WireType::Map { key, value } => WireType::Map {
            key: Box::new(substitute_wire_type(key, substitutions)),
            value: Box::new(substitute_wire_type(value, substitutions)),
        },
        WireType::Option { value } => WireType::Option {
            value: Box::new(substitute_wire_type(value, substitutions)),
        },
        WireType::Result { ok, error } => WireType::Result {
            ok: Box::new(substitute_wire_type(ok, substitutions)),
            error: Box::new(substitute_wire_type(error, substitutions)),
        },
        WireType::Tuple { elements } => WireType::Tuple {
            elements: elements
                .iter()
                .map(|element| substitute_wire_type(element, substitutions))
                .collect(),
        },
        WireType::Named {
            package,
            name,
            arguments,
        } => WireType::Named {
            package: package.clone(),
            name: name.clone(),
            arguments: arguments
                .iter()
                .map(|argument| substitute_wire_type(argument, substitutions))
                .collect(),
        },
        WireType::Qualified { qualifier, value } => WireType::Qualified {
            qualifier: *qualifier,
            value: Box::new(substitute_wire_type(value, substitutions)),
        },
        other => other.clone(),
    }
}

fn call_target_effects(target: &MirCallTarget) -> Vec<TypedDataEffectV1> {
    match target {
        MirCallTarget::Dynamic {
            parameter_modes, ..
        }
        | MirCallTarget::Builtin {
            parameter_modes, ..
        } => parameter_modes.iter().copied().map(effect).collect(),
        _ => Vec::new(),
    }
}

pub(super) fn effect(mode: MirParameterMode) -> TypedDataEffectV1 {
    match mode {
        MirParameterMode::Read => TypedDataEffectV1::Read,
        MirParameterMode::Mut => TypedDataEffectV1::Mutate,
        MirParameterMode::Take => TypedDataEffectV1::Take,
    }
}

/// Render the legacy register-VM signature spelling from canonical MIR types.
///
/// The transitional v1 payload still carries entry-point signatures as text,
/// but that spelling must be derived from the type table rather than Rust's
/// debug representation. In particular, `List<String>` is part of the VM's
/// explicit `main` ABI and cannot be emitted as `List { .. }`.
pub(super) fn legacy_signature_type(ty: &rsscript_abi_model::WireType) -> String {
    use rsscript_abi_model::{WireQualifier, WireType};

    match ty {
        WireType::Unit => "Unit".to_owned(),
        WireType::Bool => "Bool".to_owned(),
        WireType::Int { .. } => "Int".to_owned(),
        WireType::Float { .. } => "Float".to_owned(),
        WireType::String => "String".to_owned(),
        WireType::Char => "Char".to_owned(),
        WireType::Bytes => "Bytes".to_owned(),
        WireType::List { element } => format!("List<{}>", legacy_signature_type(element)),
        WireType::Map { key, value } => format!(
            "Map<{}, {}>",
            legacy_signature_type(key),
            legacy_signature_type(value)
        ),
        WireType::Option { value } => format!("Option<{}>", legacy_signature_type(value)),
        WireType::Result { ok, error } => format!(
            "Result<{}, {}>",
            legacy_signature_type(ok),
            legacy_signature_type(error)
        ),
        WireType::Tuple { elements } => {
            let elements = elements
                .iter()
                .map(legacy_signature_type)
                .collect::<Vec<_>>()
                .join(", ");
            format!("({elements})")
        }
        WireType::Named {
            package,
            name,
            arguments,
        } => {
            let qualified = package
                .as_ref()
                .map_or_else(|| name.clone(), |package| format!("{package}.{name}"));
            if arguments.is_empty() {
                qualified
            } else {
                let arguments = arguments
                    .iter()
                    .map(legacy_signature_type)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{qualified}<{arguments}>")
            }
        }
        WireType::Resource { name } | WireType::Handle { name } => name.clone(),
        WireType::Qualified { qualifier, value } => {
            let qualifier = match qualifier {
                WireQualifier::Fresh => "fresh",
                WireQualifier::Owned => "owned",
                WireQualifier::NoEscape => "noescape",
            };
            format!("{qualifier} {}", legacy_signature_type(value))
        }
    }
}
