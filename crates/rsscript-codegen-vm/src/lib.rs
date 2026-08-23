#![forbid(unsafe_code)]

//! MIR-only scalar CFG code generator.
//!
//! The emitted payload deliberately follows the existing v1 register-bytecode
//! wire contract, but this crate has no dependency on the VM implementation.
//! That makes code generation independently testable and keeps the VM on the
//! load/link/execute side of the boundary.

use std::collections::{BTreeMap, VecDeque};
use std::error::Error;
use std::fmt;

use rsscript_abi_model::{DataEffect, ExternalImport, RUNTIME_ABI_VERSION, WireType};
use rsscript_bytecode::{
    BytecodeArtifact, BytecodeError, LANGUAGE_SEMANTICS_VERSION, TYPED_EXECUTABLE_FACTS_SCHEMA_V2,
    TypedCallSiteV1, TypedCallTargetV1, TypedDataEffectV1, TypedExecutableFactsV1, TypedFactTypeV1,
    TypedFunctionFactsV1, TypedLayoutFieldV1, TypedLayoutKindV1, TypedLayoutV1,
    TypedRegisterFactV1, TypedValueOwnershipV1,
};
use rsscript_mir::{
    BlockId, MirBinaryOp, MirCallArgument, MirCallTarget, MirFunction, MirInstruction, MirLiteral,
    MirModule, MirParameterMode, MirTerminator, PlaceId, TaskId, ValueId, VerifiedMir,
    builtin_descriptor,
};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodegenError {
    InvalidMir(String),
    Unsupported(&'static str),
    DuplicateFunctionName(String),
    Bytecode(String),
}

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMir(message) => write!(f, "invalid MIR input: {message}"),
            Self::Unsupported(construct) => {
                write!(f, "MIR-to-bytecode does not support `{construct}` yet")
            }
            Self::DuplicateFunctionName(name) => {
                write!(f, "MIR contains duplicate function name `{name}")
            }
            Self::Bytecode(message) => write!(f, "cannot encode bytecode: {message}"),
        }
    }
}
impl Error for CodegenError {}

/// Emit a canonical v1 Artifact from the currently supported scalar MIR CFG
/// subset. The caller still sends the bytes through `BytecodeVerifier` before
/// loading them into the VM.
pub fn emit_artifact(
    mir: &VerifiedMir,
    source_content_hash: &str,
    interface_catalog_digest: &str,
    compiler_provenance: &str,
) -> Result<BytecodeArtifact, CodegenError> {
    let wire = wire_unit(mir)?;
    let payload = rsscript_bytecode::encode_executable_payload(&wire).map_err(bytecode_error)?;
    let imports = mir
        .external_imports()
        .iter()
        .map(|import| ExternalImport {
            symbol: import.symbol().clone(),
            signature: import.signature().clone(),
            signature_hash: import.signature().hash(),
            abi_version: RUNTIME_ABI_VERSION,
        })
        .collect();
    let mut artifact = BytecodeArtifact::new(
        LANGUAGE_SEMANTICS_VERSION,
        compiler_provenance,
        interface_catalog_digest,
        RUNTIME_ABI_VERSION,
        source_content_hash,
        imports,
        payload,
    )
    .map_err(bytecode_error)?;
    let facts = typed_executable_facts(mir, &wire, &artifact)?;
    artifact
        .attach_typed_executable_facts(&facts)
        .map_err(bytecode_error)?;
    Ok(artifact)
}

fn bytecode_error(error: BytecodeError) -> CodegenError {
    CodegenError::Bytecode(error.to_string())
}

fn wire_unit(mir: &MirModule) -> Result<Value, CodegenError> {
    let mut ids = BTreeMap::new();
    let mut native_signatures = BTreeMap::new();
    let mut types = BTreeMap::new();
    let mut variant_layouts = BTreeMap::new();
    let mut functions = Vec::new();
    let mut source_map = Vec::new();
    for function in mir.functions() {
        let name = function_name(mir, function)?.to_owned();
        if ids.insert(name.clone(), function.id().index()).is_some() {
            return Err(CodegenError::DuplicateFunctionName(name));
        }
        native_signatures.insert(
            name.clone(),
            json!({
                "params": function.signature().parameter_types().iter().map(|id| legacy_signature_type(mir.ty(*id).expect("validated type"))).collect::<Vec<_>>(),
                "return_type": legacy_signature_type(mir.ty(function.signature().result()).expect("validated result type")),
            }),
        );
        let emitted = wire_function(mir, function)?;
        source_map.extend(emitted.source_map);
        functions.push(emitted.function);
    }
    for layout in mir.type_layouts() {
        if types
            .insert(
                layout.name().to_owned(),
                json!({
                    "name": layout.name(),
                    "fields": layout.fields().iter().map(|(name, ty)| {
                        let ty = mir.ty(*ty).expect("validated MIR type layout field");
                        json!({ "name": name, "type_name": legacy_signature_type(ty) })
                    }).collect::<Vec<_>>(),
                }),
            )
            .is_some()
        {
            return Err(CodegenError::InvalidMir(
                "MIR contains duplicate runtime type-layout name".to_owned(),
            ));
        }
    }
    for layout in mir.variant_layouts() {
        if variant_layouts
            .insert(
                layout.name().to_owned(),
                json!({
                    "name": layout.name(),
                    "variants": layout.variants().iter().map(|variant| {
                        json!({
                            "name": variant.name(),
                            "fields": variant.fields().iter().map(|(name, ty)| {
                                let ty = mir.ty(*ty).expect("validated MIR variant-layout field");
                                json!({ "name": name, "type_name": legacy_signature_type(ty) })
                            }).collect::<Vec<_>>(),
                        })
                    }).collect::<Vec<_>>(),
                }),
            )
            .is_some()
        {
            return Err(CodegenError::InvalidMir(
                "MIR contains duplicate runtime variant-layout name".to_owned(),
            ));
        }
    }
    Ok(json!({
        "functions": functions,
        "function_ids": ids,
        "resource_drop_functions": BTreeMap::<String, usize>::new(),
        "types": types,
        "variant_layouts": variant_layouts,
        "native_signatures": native_signatures,
        "closure_identity_observable": false,
        "source_map": source_map,
    }))
}

/// Project the static facts that survive the current MIR into an optional,
/// digest-bound bytecode side section. This deliberately emits `Unknown` when
/// MIR v1 does not retain a proof (notably ordinary generic substitutions and
/// closure return types); the JIT must never recover those facts from names.
fn typed_executable_facts(
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

fn fact_type(ty: Option<&WireType>) -> TypedFactTypeV1 {
    ty.cloned()
        .map(TypedFactTypeV1::Known)
        .unwrap_or(TypedFactTypeV1::Unknown)
}

fn ownership(mode: MirParameterMode) -> TypedValueOwnershipV1 {
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

fn effect(mode: MirParameterMode) -> TypedDataEffectV1 {
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
fn legacy_signature_type(ty: &rsscript_abi_model::WireType) -> String {
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

struct EmittedWireFunction {
    function: Value,
    source_map: Vec<Value>,
}

fn wire_function(
    mir: &MirModule,
    function: &MirFunction,
) -> Result<EmittedWireFunction, CodegenError> {
    let mut code = Vec::new();
    let mut source_map = Vec::new();
    let debug = mir
        .function_debug(function.id())
        .expect("validated MIR debug table");
    let instruction_sources = debug
        .instruction_sources()
        .iter()
        .map(|entry| ((entry.block(), entry.instruction_index()), entry.source()))
        .collect::<BTreeMap<_, _>>();
    for (index, mode) in function.signature().parameter_modes().iter().enumerate() {
        if *mode == MirParameterMode::Read {
            code.push(instr(
                "DeepCopy",
                [("reg", json!(function.captures().len() + index))],
            ));
        }
    }
    let mut starts = BTreeMap::new();
    let mut patches = Vec::new();
    for block in function.blocks() {
        starts.insert(block.id(), code.len());
        for (instruction_index, instruction) in block.instructions().iter().enumerate() {
            let first = code.len();
            lower_instruction(mir, function, instruction, &mut code)?;
            if let Some(source) = instruction_sources.get(&(block.id(), instruction_index as u32)) {
                for instruction in first..code.len() {
                    source_map.push(json!({
                        "function": function.id().index(),
                        "instruction": instruction,
                        "file": source.file(),
                        "line": source.line(),
                        "column": source.column(),
                        "length": source.length(),
                    }));
                }
            }
        }
        lower_terminator(function, block.terminator(), &mut code, &mut patches)?;
    }
    for (index, target, field) in patches {
        let ip = *starts.get(&target).ok_or(CodegenError::InvalidMir(
            "branch targets missing block".to_owned(),
        ))?;
        let fields = code[index]
            .as_object_mut()
            .and_then(|value| value.values_mut().next())
            .and_then(Value::as_object_mut)
            .ok_or(CodegenError::InvalidMir(
                "malformed generated branch".to_owned(),
            ))?;
        fields.insert(field.to_owned(), json!(ip));
    }
    let locals = debug
        .places()
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), json!(index)))
        .collect::<Map<_, _>>();
    Ok(EmittedWireFunction {
        function: json!({
            "name": function_name(mir, function)?, "params": function.signature().parameter_types().len(),
            "captures": function.captures().len(), "regs": function.place_count() as usize + function.value_count() as usize + task_count(function) + 1,
            "local_regs": locals, "code": code,
        }),
        source_map,
    })
}

fn lower_instruction(
    mir: &MirModule,
    function: &MirFunction,
    instruction: &MirInstruction,
    code: &mut Vec<Value>,
) -> Result<(), CodegenError> {
    match instruction {
        MirInstruction::LoadLiteral { destination, value } => {
            code.push(literal(value, value_reg(function, *destination)))
        }
        MirInstruction::MakeList { destination, items } => code.push(instr(
            "MakeList",
            [
                ("dst", json!(value_reg(function, *destination))),
                (
                    "items",
                    json!(
                        items
                            .iter()
                            .map(|value| value_reg(function, *value))
                            .collect::<Vec<_>>()
                    ),
                ),
            ],
        )),
        MirInstruction::MakeMap {
            destination,
            entries,
        } => code.push(instr(
            "MakeMap",
            [
                ("dst", json!(value_reg(function, *destination))),
                (
                    "entries",
                    json!(
                        entries
                            .iter()
                            .map(|(key, value)| [
                                value_reg(function, *key),
                                value_reg(function, *value),
                            ])
                            .collect::<Vec<_>>()
                    ),
                ),
            ],
        )),
        MirInstruction::MakeObject {
            destination,
            fields,
        } => code.push(instr(
            "MakeObject",
            [
                ("dst", json!(value_reg(function, *destination))),
                (
                    "fields",
                    json!(
                        fields
                            .iter()
                            .map(|(name, value)| [
                                serde_json::Value::String(name.clone()),
                                json!(value_reg(function, *value)),
                            ])
                            .collect::<Vec<_>>()
                    ),
                ),
            ],
        )),
        MirInstruction::MakeStruct {
            destination,
            ty,
            fields,
        } => {
            let name = match mir.ty(*ty) {
                Some(rsscript_abi_model::WireType::Named { name, .. }) => name,
                _ => return Err(CodegenError::InvalidMir("record type is not named".into())),
            };
            code.push(instr(
                "MakeStruct",
                [
                    ("dst", json!(value_reg(function, *destination))),
                    (
                        "layout",
                        json!({
                            "name": name,
                            "field_names": fields.iter().map(|(field, _)| field).collect::<Vec<_>>(),
                        }),
                    ),
                    (
                        "fields",
                        json!(
                            fields
                                .iter()
                                .map(|(field, value)| [
                                    serde_json::Value::String(field.clone()),
                                    json!(value_reg(function, *value)),
                                ])
                                .collect::<Vec<_>>()
                        ),
                    ),
                ],
            ));
        }
        MirInstruction::MakeVariant {
            destination,
            variant,
            fields,
            ..
        } => code.push(instr(
            "MakeVariant",
            [
                ("dst", json!(value_reg(function, *destination))),
                (
                    "layout",
                    json!({
                        "name": variant,
                        "field_names": fields.iter().map(|(field, _)| field).collect::<Vec<_>>(),
                    }),
                ),
                (
                    "fields",
                    json!(
                        fields
                            .iter()
                            .map(|(field, value)| [
                                serde_json::Value::String(field.clone()),
                                json!(value_reg(function, *value)),
                            ])
                            .collect::<Vec<_>>()
                    ),
                ),
            ],
        )),
        MirInstruction::MakeResult {
            destination,
            ok,
            value,
        } => {
            let name = if *ok { "Ok" } else { "Err" };
            code.push(instr(
                "MakeVariant",
                [
                    ("dst", json!(value_reg(function, *destination))),
                    ("layout", json!({"name": name, "field_names": ["value"]})),
                    ("fields", json!([["value", value_reg(function, *value)]])),
                ],
            ));
        }
        MirInstruction::UnwrapResult {
            destination,
            source,
            ok,
        } => code.push(instr(
            "UnwrapVariantValue",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("src", json!(value_reg(function, *source))),
                ("expected", json!(if *ok { "Ok" } else { "Err" })),
            ],
        )),
        MirInstruction::MakeOption { destination, value } => match value {
            Some(value) => code.push(instr(
                "MakeSome",
                [
                    ("dst", json!(value_reg(function, *destination))),
                    ("value", json!(value_reg(function, *value))),
                ],
            )),
            None => code.push(instr(
                "LoadNone",
                [("dst", json!(value_reg(function, *destination)))],
            )),
        },
        MirInstruction::UnwrapOption {
            destination,
            source,
        } => code.push(instr(
            "UnwrapSome",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("src", json!(value_reg(function, *source))),
            ],
        )),
        MirInstruction::ListGet {
            destination,
            list,
            index,
        } => code.push(instr(
            "ListGet",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("list", json!(value_reg(function, *list))),
                ("index", json!(value_reg(function, *index))),
            ],
        )),
        MirInstruction::ListAppend {
            destination,
            list,
            values,
        } => code.push(instr(
            "ListAppend",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("list", json!(place_reg(*list))),
                ("values", json!(value_reg(function, *values))),
            ],
        )),
        MirInstruction::ListClear { destination, list } => code.push(instr(
            "ListClear",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("list", json!(place_reg(*list))),
            ],
        )),
        MirInstruction::ListPop { destination, list } => code.push(instr(
            "ListPop",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("list", json!(place_reg(*list))),
            ],
        )),
        MirInstruction::ListPush {
            destination,
            list,
            value,
        } => code.push(instr(
            "ListPush",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("list", json!(place_reg(*list))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::ListRemoveAt {
            destination,
            list,
            index,
        } => code.push(instr(
            "ListRemoveAt",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("list", json!(place_reg(*list))),
                ("index", json!(value_reg(function, *index))),
            ],
        )),
        MirInstruction::ListSet {
            destination,
            list,
            index,
            value,
        } => code.push(instr(
            "ListSet",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("list", json!(place_reg(*list))),
                ("index", json!(value_reg(function, *index))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::SetClear { destination, set } => code.push(instr(
            "SetClear",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("set", json!(place_reg(*set))),
            ],
        )),
        MirInstruction::SetInsert {
            destination,
            set,
            value,
        } => code.push(instr(
            "SetInsert",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("set", json!(place_reg(*set))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::SetRemove {
            destination,
            set,
            value,
        } => code.push(instr(
            "SetRemove",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("set", json!(place_reg(*set))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::DequeClear { destination, deque } => code.push(instr(
            "DequeClear",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("deque", json!(place_reg(*deque))),
            ],
        )),
        MirInstruction::DequePopBack { destination, deque } => code.push(instr(
            "DequePopBack",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("deque", json!(place_reg(*deque))),
            ],
        )),
        MirInstruction::DequePopFront { destination, deque } => code.push(instr(
            "DequePopFront",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("deque", json!(place_reg(*deque))),
            ],
        )),
        MirInstruction::DequePushBack {
            destination,
            deque,
            value,
        } => code.push(instr(
            "DequePushBack",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("deque", json!(place_reg(*deque))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::DequePushFront {
            destination,
            deque,
            value,
        } => code.push(instr(
            "DequePushFront",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("deque", json!(place_reg(*deque))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::SortedMapClear { destination, map } => code.push(instr(
            "SortedMapClear",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("map", json!(place_reg(*map))),
            ],
        )),
        MirInstruction::SortedMapInsert {
            destination,
            map,
            key,
            value,
        } => code.push(instr(
            "SortedMapInsert",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("map", json!(place_reg(*map))),
                ("key", json!(value_reg(function, *key))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::SortedMapRemove {
            destination,
            map,
            key,
        } => code.push(instr(
            "SortedMapRemove",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("map", json!(place_reg(*map))),
                ("key", json!(value_reg(function, *key))),
            ],
        )),
        MirInstruction::SortedSetClear { destination, set } => code.push(instr(
            "SortedSetClear",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("set", json!(place_reg(*set))),
            ],
        )),
        MirInstruction::SortedSetInsert {
            destination,
            set,
            value,
        } => code.push(instr(
            "SortedSetInsert",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("set", json!(place_reg(*set))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::SortedSetRemove {
            destination,
            set,
            value,
        } => code.push(instr(
            "SortedSetRemove",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("set", json!(place_reg(*set))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::BufferClear {
            destination,
            buffer,
        } => code.push(instr(
            "BufferClear",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("buffer", json!(place_reg(*buffer))),
            ],
        )),
        MirInstruction::StringBuilderPush {
            destination,
            builder,
            value,
        } => code.push(instr(
            "StringBuilderPush",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("builder", json!(place_reg(*builder))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::StringBuilderFinish {
            destination,
            builder,
        } => code.push(instr(
            "StringBuilderFinish",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("builder", json!(value_reg(function, *builder))),
            ],
        )),
        MirInstruction::MapGet {
            destination,
            map,
            key,
        } => code.push(instr(
            "MapGet",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("map", json!(value_reg(function, *map))),
                ("key", json!(value_reg(function, *key))),
            ],
        )),
        MirInstruction::MapClear { destination, map } => code.push(instr(
            "MapClear",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("map", json!(place_reg(*map))),
            ],
        )),
        MirInstruction::MapInsert {
            destination,
            map,
            key,
            value,
        } => code.push(instr(
            "MapInsert",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("map", json!(place_reg(*map))),
                ("key", json!(value_reg(function, *key))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::MapInsertOld {
            destination,
            map,
            key,
            value,
        } => code.push(instr(
            "MapInsertOld",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("map", json!(place_reg(*map))),
                ("key", json!(value_reg(function, *key))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::MapRemove {
            destination,
            map,
            key,
        } => code.push(instr(
            "MapRemove",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("map", json!(place_reg(*map))),
                ("key", json!(value_reg(function, *key))),
            ],
        )),
        MirInstruction::GetField {
            destination,
            base,
            field,
        } => code.push(instr(
            "GetField",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("base", json!(value_reg(function, *base))),
                ("name", json!(field)),
            ],
        )),
        MirInstruction::SetField { base, field, value } => code.push(instr(
            "SetField",
            [
                ("dst", json!(scratch_reg(function))),
                ("base", json!(value_reg(function, *base))),
                ("name", json!(field)),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::ListLen { destination, list } => code.push(instr(
            "ListLen",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("list", json!(value_reg(function, *list))),
            ],
        )),
        MirInstruction::ReadPlace { destination, place }
        | MirInstruction::BorrowRead { destination, place }
        | MirInstruction::TakePlace { destination, place } => code.push(instr(
            "Move",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("src", json!(place_reg(*place))),
            ],
        )),
        MirInstruction::Manage {
            destination,
            source,
        } => code.push(instr(
            "Manage",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("src", json!(value_reg(function, *source))),
            ],
        )),
        // Retention is a verified ownership fact. It does not copy or destroy
        // a VM value by itself, so the v1 register payload has no runtime
        // instruction to emit; keeping it in MIR still prevents backends from
        // silently erasing the semantic boundary.
        MirInstruction::Retain { .. } => {}
        // MIR validates that no later read can observe the dropped place. Clear
        // the register as well, promptly releasing the VM reference instead of
        // retaining an otherwise-dead heap value until frame teardown.
        MirInstruction::Drop { place } => {
            code.push(instr("LoadUnit", [("dst", json!(place_reg(*place)))]))
        }
        MirInstruction::AcquireResource { place, source, .. } => {
            code.push(instr(
                "Move",
                [
                    ("dst", json!(place_reg(*place))),
                    ("src", json!(value_reg(function, *source))),
                ],
            ));
            code.push(instr(
                "ResourceAcquire",
                [("resource", json!(place_reg(*place)))],
            ));
        }
        MirInstruction::ReleaseResource { place } => code.push(instr(
            "ResourceDrop",
            [("resource", json!(place_reg(*place)))],
        )),
        MirInstruction::Spawn {
            task,
            target,
            arguments,
            ..
        } => {
            let args = arguments
                .iter()
                .map(|argument| match argument {
                    MirCallArgument::Value(value) => Ok(value_reg(function, *value)),
                    MirCallArgument::BorrowRead(place) | MirCallArgument::Take(place) => {
                        Ok(place_reg(*place))
                    }
                    MirCallArgument::BorrowMut(_) => {
                        Err(CodegenError::Unsupported("mutable async argument"))
                    }
                })
                .collect::<Result<Vec<_>, _>>()?;
            code.push(instr(
                "SpawnTask",
                [
                    ("dst", json!(task_reg(function, *task))),
                    ("function", json!(target.index())),
                    ("args", json!(args)),
                ],
            ));
        }
        MirInstruction::Await { destination, task } => code.push(instr(
            "AwaitJoin",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("src", json!(task_reg(function, *task))),
            ],
        )),
        MirInstruction::Select {
            tasks,
            winner,
            value,
        } => code.push(instr(
            "SelectWait",
            [
                (
                    "handles",
                    json!(
                        tasks
                            .iter()
                            .map(|task| task_reg(function, *task))
                            .collect::<Vec<_>>()
                    ),
                ),
                ("winner", json!(value_reg(function, *winner))),
                ("value", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::TryResult {
            destination,
            source,
            cleanup,
        } => code.push(instr(
            "TryResult",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("src", json!(value_reg(function, *source))),
                (
                    "cleanup",
                    json!(
                        cleanup
                            .iter()
                            .map(|place| place_reg(*place))
                            .collect::<Vec<_>>()
                    ),
                ),
            ],
        )),
        MirInstruction::Cancel { task } => code.push(instr(
            "CancelTask",
            [("src", json!(task_reg(function, *task)))],
        )),
        MirInstruction::Join { group } => code.push(instr(
            "JoinTasks",
            [(
                "handles",
                json!(
                    tasks_for_group(function, *group)
                        .into_iter()
                        .map(|task| task_reg(function, task))
                        .collect::<Vec<_>>()
                ),
            )],
        )),
        MirInstruction::WritePlace { place, value } => code.push(instr(
            "Move",
            [
                ("dst", json!(place_reg(*place))),
                ("src", json!(value_reg(function, *value))),
            ],
        )),
        MirInstruction::Binary {
            destination,
            op,
            left,
            right,
        } => code.push(binary(
            *op,
            value_reg(function, *destination),
            value_reg(function, *left),
            value_reg(function, *right),
        )?),
        MirInstruction::StringConcat {
            destination,
            left,
            right,
        } => code.push(instr(
            "StringConcat",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("left", json!(value_reg(function, *left))),
                ("right", json!(value_reg(function, *right))),
            ],
        )),
        MirInstruction::Call {
            destination,
            target,
            arguments,
        } => {
            let mut args = Vec::with_capacity(arguments.len());
            let mut mut_args = Vec::new();
            for (index, argument) in arguments.iter().enumerate() {
                match argument {
                    MirCallArgument::Value(value) => args.push(value_reg(function, *value)),
                    MirCallArgument::BorrowRead(place) | MirCallArgument::Take(place) => {
                        args.push(place_reg(*place));
                    }
                    MirCallArgument::BorrowMut(place) => {
                        args.push(place_reg(*place));
                        mut_args.push(index);
                    }
                }
            }
            let dst = value_reg(function, *destination);
            match target {
                MirCallTarget::Function(id)
                | MirCallTarget::FunctionInstance { function: id, .. } => code.push(instr(
                    "CallKnown",
                    [
                        ("dst", json!(dst)),
                        ("function", json!(id.index())),
                        ("args", json!(args)),
                        ("mut_args", json!(mut_args)),
                    ],
                )),
                MirCallTarget::Builtin {
                    id, type_arguments, ..
                } => {
                    let intrinsic = builtin_descriptor(*id)
                        .ok_or(CodegenError::InvalidMir(
                            "builtin call references missing registry identity".to_owned(),
                        ))?
                        .vm_name;
                    if type_arguments.is_empty() {
                        code.push(instr(
                            "CallIntrinsic",
                            [
                                ("dst", json!(dst)),
                                ("intrinsic", json!(intrinsic)),
                                ("args", json!(args)),
                            ],
                        ));
                    } else if type_arguments.len() == 1 {
                        let ty = mir.ty(type_arguments[0]).ok_or(CodegenError::InvalidMir(
                            "builtin call references missing type argument".to_owned(),
                        ))?;
                        let type_arg =
                            wire_runtime_type_name(ty).ok_or(CodegenError::InvalidMir(
                                "builtin call type argument has no v1 runtime identity".to_owned(),
                            ))?;
                        code.push(instr(
                            "CallTypedIntrinsic",
                            [
                                ("dst", json!(dst)),
                                ("intrinsic", json!(intrinsic)),
                                ("type_arg", json!(type_arg)),
                                ("args", json!(args)),
                            ],
                        ));
                    } else {
                        return Err(CodegenError::InvalidMir(
                            "v1 typed intrinsic supports exactly one type argument".to_owned(),
                        ));
                    }
                }
                MirCallTarget::Dynamic { dispatch, .. } => {
                    let dispatch = dispatch
                        .iter()
                        .map(|(receiver, target)| {
                            let ty = mir.ty(*receiver).ok_or(CodegenError::InvalidMir(
                                "dynamic dispatch references missing receiver type".to_owned(),
                            ))?;
                            let type_name =
                                wire_runtime_type_name(ty).ok_or(CodegenError::InvalidMir(
                                    "dynamic dispatch receiver has no v1 runtime identity"
                                        .to_owned(),
                                ))?;
                            Ok(json!([type_name, target.index()]))
                        })
                        .collect::<Result<Vec<_>, CodegenError>>()?;
                    code.push(instr(
                        "CallDynamic",
                        [
                            ("dst", json!(dst)),
                            ("dispatch", json!(dispatch)),
                            ("args", json!(args)),
                            ("mut_args", json!(mut_args)),
                        ],
                    ));
                }
                MirCallTarget::External(id) => {
                    let import =
                        mir.external_imports()
                            .get(id.index())
                            .ok_or(CodegenError::InvalidMir(
                                "external call references missing import".to_owned(),
                            ))?;
                    code.push(instr(
                        "CallExternal",
                        [
                            ("dst", json!(dst)),
                            ("key", json!(import.symbol().as_str())),
                            ("args", json!(args)),
                            ("mut_args", json!(mut_args)),
                        ],
                    ));
                }
            }
        }
        MirInstruction::MakeClosure {
            destination,
            function: target,
            captures,
        } => {
            let captures = captures
                .iter()
                .map(|capture| match capture {
                    MirCallArgument::Value(value) => Ok(value_reg(function, *value)),
                    MirCallArgument::BorrowRead(place)
                    | MirCallArgument::BorrowMut(place)
                    | MirCallArgument::Take(place) => Ok(place_reg(*place)),
                })
                .collect::<Result<Vec<_>, CodegenError>>()?;
            code.push(instr(
                "MakeClosure",
                [
                    ("dst", json!(value_reg(function, *destination))),
                    ("function", json!(target.index())),
                    ("captures", json!(captures)),
                ],
            ));
        }
        MirInstruction::CallClosure {
            destination,
            closure,
            arguments,
            ..
        } => {
            let mut args = Vec::with_capacity(arguments.len());
            let mut mut_args = Vec::new();
            for (index, argument) in arguments.iter().enumerate() {
                match argument {
                    MirCallArgument::Value(value) => args.push(value_reg(function, *value)),
                    MirCallArgument::BorrowRead(place) | MirCallArgument::Take(place) => {
                        args.push(place_reg(*place));
                    }
                    MirCallArgument::BorrowMut(place) => {
                        args.push(place_reg(*place));
                        mut_args.push(index);
                    }
                }
            }
            code.push(instr(
                "CallClosure",
                [
                    ("dst", json!(value_reg(function, *destination))),
                    ("closure", json!(value_reg(function, *closure))),
                    ("args", json!(args)),
                    ("mut_args", json!(mut_args)),
                ],
            ));
        }
        MirInstruction::Discard { .. } => {}
    };
    Ok(())
}

/// The v1 register VM accepts one legacy typed-intrinsic string. It is an
/// encoder-only projection of the already-verified `WireType`; source callee
/// spellings never reach this boundary.
fn wire_runtime_type_name(ty: &rsscript_abi_model::WireType) -> Option<String> {
    match ty {
        rsscript_abi_model::WireType::Unit => Some("Unit".to_owned()),
        rsscript_abi_model::WireType::Bool => Some("Bool".to_owned()),
        rsscript_abi_model::WireType::Int { .. } => Some("Int".to_owned()),
        rsscript_abi_model::WireType::Float { .. } => Some("Float".to_owned()),
        rsscript_abi_model::WireType::String => Some("String".to_owned()),
        rsscript_abi_model::WireType::Char => Some("Char".to_owned()),
        rsscript_abi_model::WireType::Bytes => Some("Bytes".to_owned()),
        rsscript_abi_model::WireType::Named { name, .. }
        | rsscript_abi_model::WireType::Resource { name }
        | rsscript_abi_model::WireType::Handle { name } => Some(name.clone()),
        rsscript_abi_model::WireType::Qualified { value, .. } => wire_runtime_type_name(value),
        rsscript_abi_model::WireType::List { .. }
        | rsscript_abi_model::WireType::Map { .. }
        | rsscript_abi_model::WireType::Option { .. }
        | rsscript_abi_model::WireType::Result { .. }
        | rsscript_abi_model::WireType::Tuple { .. } => None,
    }
}

fn lower_terminator(
    function: &MirFunction,
    term: &MirTerminator,
    code: &mut Vec<Value>,
    patches: &mut Vec<(usize, BlockId, &'static str)>,
) -> Result<(), CodegenError> {
    match term {
        MirTerminator::Return(value) => {
            let src = value.map_or_else(
                || {
                    let scratch = function.place_count() as usize + function.value_count() as usize;
                    code.push(instr("LoadUnit", [("dst", json!(scratch))]));
                    scratch
                },
                |value| value_reg(function, value),
            );
            code.push(instr("Return", [("src", json!(src))]));
        }
        MirTerminator::Jump(target) => {
            let index = code.len();
            code.push(instr("Jump", [("target", json!(0))]));
            patches.push((index, *target, "target"));
        }
        MirTerminator::Branch {
            condition,
            then_target,
            else_target,
        } => {
            let index = code.len();
            code.push(instr(
                "JumpIfBool",
                [
                    ("cond", json!(value_reg(function, *condition))),
                    ("expected", json!(true)),
                    ("target", json!(0)),
                ],
            ));
            patches.push((index, *then_target, "target"));
            let index = code.len();
            code.push(instr("Jump", [("target", json!(0))]));
            patches.push((index, *else_target, "target"));
        }
        MirTerminator::MatchVariant {
            value,
            expected,
            match_target,
            else_target,
        } => {
            let index = code.len();
            code.push(instr(
                "MatchVariant",
                [
                    ("src", json!(value_reg(function, *value))),
                    ("expected", json!(expected)),
                    ("match_ip", json!(0)),
                    ("else_ip", json!(0)),
                ],
            ));
            patches.push((index, *match_target, "match_ip"));
            patches.push((index, *else_target, "else_ip"));
        }
        MirTerminator::MatchResult {
            value,
            ok_target,
            err_target,
        } => {
            let index = code.len();
            code.push(instr(
                "MatchResult",
                [
                    ("src", json!(value_reg(function, *value))),
                    ("ok_ip", json!(0)),
                    ("err_ip", json!(0)),
                ],
            ));
            patches.push((index, *ok_target, "ok_ip"));
            patches.push((index, *err_target, "err_ip"));
        }
        MirTerminator::MatchOption {
            value,
            some_target,
            none_target,
        } => {
            let index = code.len();
            code.push(instr(
                "MatchOption",
                [
                    ("src", json!(value_reg(function, *value))),
                    ("some_ip", json!(0)),
                    ("none_ip", json!(0)),
                ],
            ));
            patches.push((index, *some_target, "some_ip"));
            patches.push((index, *none_target, "none_ip"));
        }
        MirTerminator::Unreachable => code.push(instr(
            "RuntimeError",
            [("message", json!("entered unreachable MIR block"))],
        )),
    }
    Ok(())
}

fn literal(value: &MirLiteral, dst: usize) -> Value {
    match value {
        MirLiteral::Unit => instr("LoadUnit", [("dst", json!(dst))]),
        MirLiteral::Int(value) => instr("LoadInt", [("dst", json!(dst)), ("value", json!(value))]),
        MirLiteral::Float(value) => {
            instr("LoadFloat", [("dst", json!(dst)), ("value", json!(value))])
        }
        MirLiteral::Bool(value) => {
            instr("LoadBool", [("dst", json!(dst)), ("value", json!(value))])
        }
        MirLiteral::String(value) => {
            instr("LoadString", [("dst", json!(dst)), ("value", json!(value))])
        }
        MirLiteral::Char(value) => {
            instr("LoadChar", [("dst", json!(dst)), ("value", json!(value))])
        }
    }
}
fn binary(op: MirBinaryOp, dst: usize, lhs: usize, rhs: usize) -> Result<Value, CodegenError> {
    let opcode = match op {
        MirBinaryOp::Add => "AddInt",
        MirBinaryOp::Subtract => "SubInt",
        MirBinaryOp::Multiply => "MulInt",
        MirBinaryOp::Divide => "DivInt",
        MirBinaryOp::Modulo => "ModInt",
        MirBinaryOp::BitAnd => "BitAndInt",
        MirBinaryOp::BitOr => "BitOrInt",
        MirBinaryOp::BitXor => "BitXorInt",
        MirBinaryOp::ShiftLeft => "ShiftLeftInt",
        MirBinaryOp::ShiftRight => "ShiftRightInt",
        MirBinaryOp::Equal => "Equal",
        MirBinaryOp::NotEqual => "NotEqual",
        MirBinaryOp::Less => "LessInt",
        MirBinaryOp::LessEqual => "LessEqualInt",
        MirBinaryOp::Greater => "GreaterInt",
        MirBinaryOp::GreaterEqual => "GreaterEqualInt",
        MirBinaryOp::LogicalAnd | MirBinaryOp::LogicalOr => {
            return Err(CodegenError::Unsupported("logical binary operation"));
        }
    };
    Ok(instr(
        opcode,
        [
            ("dst", json!(dst)),
            ("lhs", json!(lhs)),
            ("rhs", json!(rhs)),
        ],
    ))
}
fn instr<'a>(opcode: &str, fields: impl IntoIterator<Item = (&'a str, Value)>) -> Value {
    let mut values = Map::new();
    for (key, value) in fields {
        values.insert(key.to_owned(), value);
    }
    let mut instruction = Map::new();
    instruction.insert(opcode.to_owned(), Value::Object(values));
    Value::Object(instruction)
}
fn function_name<'a>(mir: &'a MirModule, function: &MirFunction) -> Result<&'a str, CodegenError> {
    mir.function_debug(function.id())
        .map(|debug| debug.name())
        .ok_or(CodegenError::InvalidMir(
            "function missing debug name".to_owned(),
        ))
}
fn place_reg(place: PlaceId) -> usize {
    place.index()
}
fn value_reg(function: &MirFunction, value: ValueId) -> usize {
    function.place_count() as usize + value.index()
}

fn task_reg(function: &MirFunction, task: TaskId) -> usize {
    function.place_count() as usize + function.value_count() as usize + task.index()
}

/// The legacy register instruction for field assignment returns `Unit` while
/// updating its base register. Reserve the final register declared for every
/// generated function as its discard-only result slot so that Unit can never
/// overwrite a typed MIR value.
fn scratch_reg(function: &MirFunction) -> usize {
    function.place_count() as usize + function.value_count() as usize + task_count(function)
}

fn task_count(function: &MirFunction) -> usize {
    function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            MirInstruction::Spawn { task, .. } => Some(task.index() + 1),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

fn tasks_for_group(function: &MirFunction, group: rsscript_mir::TaskGroupId) -> Vec<TaskId> {
    function
        .blocks()
        .iter()
        .flat_map(|block| block.instructions())
        .filter_map(|instruction| match instruction {
            MirInstruction::Spawn {
                task, group: owner, ..
            } if *owner == group => Some(*task),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_abi_model::{CORE_LIBRARY_ABI_VERSION, RUNTIME_ABI_VERSION, WireType};
    use rsscript_bytecode::BytecodeVerifier;
    use rsscript_mir::{
        BasicBlock, FunctionId, MirFunctionDebug, MirFunctionSignature, MirInstructionSource,
        MirSourceLocation, ResourceTypeId, TaskGroupId, TypeId,
    };

    #[test]
    fn scalar_cfg_emits_a_verifiable_vm_artifact_without_the_vm() {
        let module = MirModule::new(
            vec![WireType::Int {
                bits: 64,
                signed: true,
            }],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                0,
                3,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(20),
                        },
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(1),
                            value: MirLiteral::Int(22),
                        },
                        MirInstruction::Binary {
                            destination: ValueId::new(2),
                            op: MirBinaryOp::Add,
                            left: ValueId::new(0),
                            right: ValueId::new(1),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(2))),
                )],
            )],
            vec![
                MirFunctionDebug::new("main", vec![]).with_instruction_sources(vec![
                    MirInstructionSource::new(
                        BlockId::new(0),
                        0,
                        MirSourceLocation::new("main.rss", 1, 1, 2),
                    ),
                ]),
            ],
            vec![],
        )
        .unwrap();
        let module = module.into_verified().expect("scalar MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .unwrap();
        assert_eq!(artifact.header.runtime_abi_version, RUNTIME_ABI_VERSION);
        assert_eq!(
            artifact.header.core_library_abi_version,
            CORE_LIBRARY_ABI_VERSION
        );
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode source-mapped payload");
        assert_eq!(
            payload["source_map"],
            serde_json::json!([{
                "function": 0,
                "instruction": 0,
                "file": "main.rss",
                "line": 1,
                "column": 1,
                "length": 2,
            }])
        );
        let verified = BytecodeVerifier::default()
            .verify(&artifact.to_bytes().unwrap())
            .unwrap();
        let facts = verified
            .typed_executable_facts()
            .expect("codegen attaches verified typed facts")
            .facts();
        assert_eq!(facts.executable_hash, artifact.header.executable_hash);
        assert_eq!(facts.functions.len(), 1);
        assert!(facts.functions[0].registers[..3].iter().all(|register| {
            matches!(
                register.ty,
                TypedFactTypeV1::Known(WireType::Int {
                    bits: 64,
                    signed: true
                })
            )
        }));
    }

    #[test]
    fn typed_facts_propagate_through_more_than_three_place_hops() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let mut instructions = vec![MirInstruction::LoadLiteral {
            destination: ValueId::new(0),
            value: MirLiteral::Int(7),
        }];
        for index in 0..5 {
            instructions.push(MirInstruction::WritePlace {
                place: PlaceId::new(index),
                value: ValueId::new(index),
            });
            instructions.push(MirInstruction::ReadPlace {
                destination: ValueId::new(index + 1),
                place: PlaceId::new(index),
            });
        }
        let module = MirModule::new(
            vec![int.clone()],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                5,
                6,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    instructions,
                    MirTerminator::Return(Some(ValueId::new(5))),
                )],
            )],
            vec![MirFunctionDebug::new(
                "main",
                (0..5).map(|index| format!("p{index}")).collect(),
            )],
            vec![],
        )
        .expect("place-chain MIR verifies")
        .into_verified()
        .expect("place-chain MIR admission");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit place-chain bytecode");
        let verified = BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("artifact bytes"))
            .expect("verify place-chain facts");
        assert_eq!(
            verified
                .typed_executable_facts()
                .expect("typed facts")
                .facts()
                .functions[0]
                .registers[10]
                .ty,
            TypedFactTypeV1::Known(int)
        );
    }

    #[test]
    fn qualified_scalar_signature_survives_codegen_and_independent_verification() {
        let qualified_int = WireType::Qualified {
            qualifier: rsscript_abi_model::WireQualifier::Owned,
            value: Box::new(WireType::Int {
                bits: 64,
                signed: true,
            }),
        };
        let module = MirModule::new(
            vec![qualified_int.clone()],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::with_modes(
                    vec![TypeId::new(0)],
                    vec![MirParameterMode::Read],
                    TypeId::new(0),
                    false,
                ),
                1,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::ReadPlace {
                        destination: ValueId::new(0),
                        place: PlaceId::new(0),
                    }],
                    MirTerminator::Return(Some(ValueId::new(0))),
                )],
            )],
            vec![MirFunctionDebug::new("identity", vec!["value".to_owned()])],
            vec![],
        )
        .expect("qualified scalar MIR verifies")
        .into_verified()
        .expect("qualified scalar MIR admission");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit qualified scalar bytecode");
        let verified = BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("artifact bytes"))
            .expect("qualified scalar facts verify independently");
        let facts = verified
            .typed_executable_facts()
            .expect("qualified typed facts")
            .facts();
        assert_eq!(
            facts.functions[0].registers[0].ty,
            fact_type(Some(&qualified_int))
        );
        assert_eq!(
            facts.functions[0].registers[1].ty,
            fact_type(Some(&qualified_int))
        );
    }

    #[test]
    fn generic_direct_call_retains_bounded_type_arguments_without_changing_call_known() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let module = MirModule::new(
            vec![
                WireType::Named {
                    package: None,
                    name: "T".to_owned(),
                    arguments: Vec::new(),
                },
                int.clone(),
            ],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    MirFunctionSignature::new(vec![], TypeId::new(1), false),
                    0,
                    2,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::LoadLiteral {
                                destination: ValueId::new(0),
                                value: MirLiteral::Int(7),
                            },
                            MirInstruction::Call {
                                destination: ValueId::new(1),
                                target: MirCallTarget::FunctionInstance {
                                    function: FunctionId::new(1),
                                    type_substitutions: vec![(TypeId::new(0), TypeId::new(1))]
                                        .into_boxed_slice(),
                                },
                                arguments: vec![MirCallArgument::Value(ValueId::new(0))],
                            },
                        ],
                        MirTerminator::Return(Some(ValueId::new(1))),
                    )],
                ),
                MirFunction::new(
                    FunctionId::new(1),
                    MirFunctionSignature::with_modes(
                        vec![TypeId::new(0)],
                        vec![MirParameterMode::Read],
                        TypeId::new(0),
                        false,
                    ),
                    1,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![MirInstruction::ReadPlace {
                            destination: ValueId::new(0),
                            place: PlaceId::new(0),
                        }],
                        MirTerminator::Return(Some(ValueId::new(0))),
                    )],
                ),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("identity", vec!["value".to_owned()]),
            ],
            vec![],
        )
        .expect("generic instance MIR verifies")
        .into_verified()
        .expect("generic instance MIR admission");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit generic call");
        let executable: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode executable");
        assert!(executable["functions"][0]["code"][1]["CallKnown"].is_object());
        let verified = BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("artifact bytes"))
            .expect("v2 typed substitutions remain independently bounded");
        let facts = verified
            .typed_executable_facts()
            .expect("typed facts")
            .facts();
        assert_eq!(facts.schema, TYPED_EXECUTABLE_FACTS_SCHEMA_V2);
        assert_eq!(
            facts.functions[0].call_sites[0].type_parameters,
            vec![WireType::Named {
                package: None,
                name: "T".to_owned(),
                arguments: Vec::new(),
            }]
        );
        assert_eq!(facts.functions[0].call_sites[0].type_arguments, vec![int]);
    }

    #[test]
    fn aggregate_field_read_emits_verifiable_get_field_bytecode() {
        let module = MirModule::new(
            vec![WireType::Int {
                bits: 64,
                signed: true,
            }],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                0,
                3,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(42),
                        },
                        MirInstruction::MakeObject {
                            destination: ValueId::new(1),
                            fields: vec![("count".into(), ValueId::new(0))],
                        },
                        MirInstruction::GetField {
                            destination: ValueId::new(2),
                            base: ValueId::new(1),
                            field: "count".into(),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(2))),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec![])],
            vec![],
        )
        .expect("field MIR verifies");
        let module = module.into_verified().expect("field MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit field bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode field payload");
        assert_eq!(
            payload["functions"][0]["code"][2]["GetField"],
            serde_json::json!({"dst": 2, "base": 1, "name": "count"})
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode field bytecode"))
            .expect("verify field bytecode");
    }

    #[test]
    fn owned_list_construction_emits_a_verifiable_make_list_instruction() {
        let module = MirModule::new(
            vec![
                WireType::Int {
                    bits: 64,
                    signed: true,
                },
                WireType::List {
                    element: Box::new(WireType::Int {
                        bits: 64,
                        signed: true,
                    }),
                },
            ],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(1), false),
                0,
                3,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(1),
                        },
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(1),
                            value: MirLiteral::Int(2),
                        },
                        MirInstruction::MakeList {
                            destination: ValueId::new(2),
                            items: vec![ValueId::new(0), ValueId::new(1)],
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(2))),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec![])],
            vec![],
        )
        .expect("list MIR verifies");
        let module = module.into_verified().expect("list MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit list bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode list payload");
        assert_eq!(
            payload["functions"][0]["code"][2]["MakeList"]["items"],
            serde_json::json!([0, 1])
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode list bytecode"))
            .expect("verify list bytecode");
    }

    #[test]
    fn map_lookup_emits_a_verifiable_option_valued_map_get_instruction() {
        let module = MirModule::new(
            vec![WireType::Unit],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                0,
                4,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(1),
                        },
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(1),
                            value: MirLiteral::Int(42),
                        },
                        MirInstruction::MakeMap {
                            destination: ValueId::new(2),
                            entries: vec![(ValueId::new(0), ValueId::new(1))],
                        },
                        MirInstruction::MapGet {
                            destination: ValueId::new(3),
                            map: ValueId::new(2),
                            key: ValueId::new(0),
                        },
                    ],
                    MirTerminator::Return(Some(ValueId::new(3))),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec![])],
            vec![],
        )
        .expect("map-get MIR verifies");
        let module = module.into_verified().expect("map-get MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit map-get bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode map-get payload");
        assert_eq!(
            payload["functions"][0]["code"][3]["MapGet"],
            serde_json::json!({"dst": 3, "map": 2, "key": 0})
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode map-get bytecode"))
            .expect("verify map-get bytecode");
    }

    #[test]
    fn resource_lifetime_ops_preserve_resource_value_until_drop() {
        let module = MirModule::new(
            vec![
                WireType::Unit,
                WireType::Resource {
                    name: "host.test.Resource".into(),
                },
            ],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                1,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Unit,
                        },
                        MirInstruction::AcquireResource {
                            place: PlaceId::new(0),
                            resource_type: ResourceTypeId::new(1),
                            source: ValueId::new(0),
                        },
                        MirInstruction::ReleaseResource {
                            place: PlaceId::new(0),
                        },
                    ],
                    MirTerminator::Return(None),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec!["resource".into()])],
            vec![],
        )
        .unwrap();
        let module = module.into_verified().expect("resource MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit resource bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode resource payload");
        let opcodes = payload["functions"][0]["code"]
            .as_array()
            .expect("resource code")
            .iter()
            .map(|instruction| {
                instruction
                    .as_object()
                    .and_then(|instruction| instruction.keys().next())
                    .expect("single opcode")
                    .as_str()
            })
            .collect::<Vec<_>>();
        assert!(opcodes.contains(&"Move"));
        assert!(opcodes.contains(&"ResourceAcquire"));
        assert!(opcodes.contains(&"ResourceDrop"));
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode resource bytecode"))
            .expect("verify resource bytecode");
    }

    #[test]
    fn spawned_async_mir_emits_verifiable_task_bytecode() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let module = MirModule::new(
            vec![int],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    MirFunctionSignature::new(vec![], TypeId::new(0), false),
                    0,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::Spawn {
                                task: TaskId::new(0),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(1),
                                arguments: vec![],
                            },
                            MirInstruction::Await {
                                destination: ValueId::new(0),
                                task: TaskId::new(0),
                            },
                        ],
                        MirTerminator::Return(Some(ValueId::new(0))),
                    )],
                ),
                MirFunction::new(
                    FunctionId::new(1),
                    MirFunctionSignature::new(vec![], TypeId::new(0), true),
                    0,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(7),
                        }],
                        MirTerminator::Return(Some(ValueId::new(0))),
                    )],
                ),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        )
        .unwrap();
        let module = module.into_verified().expect("task MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit task bytecode");
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode task bytecode"))
            .expect("verify task bytecode");
    }

    #[test]
    fn cancelled_child_mir_emits_verifiable_task_bytecode() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let module = MirModule::new(
            vec![int],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    MirFunctionSignature::new(vec![], TypeId::new(0), false),
                    0,
                    0,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::Spawn {
                                task: TaskId::new(0),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(1),
                                arguments: vec![],
                            },
                            MirInstruction::Cancel {
                                task: TaskId::new(0),
                            },
                        ],
                        MirTerminator::Return(None),
                    )],
                ),
                MirFunction::new(
                    FunctionId::new(1),
                    MirFunctionSignature::new(vec![], TypeId::new(0), true),
                    0,
                    1,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Int(7),
                        }],
                        MirTerminator::Return(Some(ValueId::new(0))),
                    )],
                ),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("worker", vec![]),
            ],
            vec![],
        )
        .expect("cancel MIR verifies");
        let module = module.into_verified().expect("cancel MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit cancellation bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode cancellation payload");
        assert!(
            payload["functions"][0]["code"]
                .as_array()
                .expect("cancellation code")
                .iter()
                .any(|instruction| instruction.get("CancelTask").is_some())
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode cancellation bytecode"))
            .expect("verify cancellation bytecode");
    }

    #[test]
    fn select_mir_emits_verifiable_first_ready_bytecode() {
        let int = WireType::Int {
            bits: 64,
            signed: true,
        };
        let worker = |id, value| {
            MirFunction::new(
                FunctionId::new(id),
                MirFunctionSignature::new(vec![], TypeId::new(0), true),
                0,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::Int(value),
                    }],
                    MirTerminator::Return(Some(ValueId::new(0))),
                )],
            )
        };
        let module = MirModule::new(
            vec![int],
            vec![
                MirFunction::new(
                    FunctionId::new(0),
                    MirFunctionSignature::new(vec![], TypeId::new(0), false),
                    0,
                    2,
                    vec![BasicBlock::new(
                        BlockId::new(0),
                        vec![
                            MirInstruction::Spawn {
                                task: TaskId::new(0),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(1),
                                arguments: vec![],
                            },
                            MirInstruction::Spawn {
                                task: TaskId::new(1),
                                group: TaskGroupId::new(0),
                                target: FunctionId::new(2),
                                arguments: vec![],
                            },
                            MirInstruction::Select {
                                tasks: vec![TaskId::new(0), TaskId::new(1)],
                                winner: ValueId::new(0),
                                value: ValueId::new(1),
                            },
                        ],
                        MirTerminator::Return(Some(ValueId::new(1))),
                    )],
                ),
                worker(1, 7),
                worker(2, 9),
            ],
            vec![
                MirFunctionDebug::new("main", vec![]),
                MirFunctionDebug::new("first", vec![]),
                MirFunctionDebug::new("second", vec![]),
            ],
            vec![],
        )
        .expect("select MIR verifies");
        let module = module.into_verified().expect("select MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit select bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode select payload");
        assert!(
            payload["functions"][0]["code"]
                .as_array()
                .expect("select code")
                .iter()
                .any(|instruction| instruction.get("SelectWait").is_some())
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode select bytecode"))
            .expect("verify select bytecode");
    }

    #[test]
    fn ownership_retain_and_drop_emit_a_verifiable_cleanup_boundary() {
        let module = MirModule::new(
            vec![WireType::Unit],
            vec![MirFunction::new(
                FunctionId::new(0),
                MirFunctionSignature::new(vec![], TypeId::new(0), false),
                1,
                1,
                vec![BasicBlock::new(
                    BlockId::new(0),
                    vec![
                        MirInstruction::LoadLiteral {
                            destination: ValueId::new(0),
                            value: MirLiteral::Unit,
                        },
                        MirInstruction::WritePlace {
                            place: PlaceId::new(0),
                            value: ValueId::new(0),
                        },
                        MirInstruction::Retain {
                            place: PlaceId::new(0),
                        },
                        MirInstruction::Drop {
                            place: PlaceId::new(0),
                        },
                    ],
                    MirTerminator::Return(None),
                )],
            )],
            vec![MirFunctionDebug::new("main", vec!["owned".into()])],
            vec![],
        )
        .expect("ownership MIR verifies");
        let module = module.into_verified().expect("ownership MIR must verify");
        let artifact = emit_artifact(
            &module,
            &format!("sha256:{}", "a".repeat(64)),
            &format!("sha256:{}", "b".repeat(64)),
            "0.1.0",
        )
        .expect("emit ownership bytecode");
        let payload: serde_json::Value =
            rsscript_bytecode::decode_executable_payload(&artifact.payload)
                .expect("decode ownership payload");
        let opcodes = payload["functions"][0]["code"]
            .as_array()
            .expect("ownership code")
            .iter()
            .map(|instruction| {
                instruction
                    .as_object()
                    .and_then(|instruction| instruction.keys().next())
                    .expect("single opcode")
                    .as_str()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            opcodes,
            ["LoadUnit", "Move", "LoadUnit", "LoadUnit", "Return"],
            "retain has no VM side effect while drop clears its place before the unit return"
        );
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().expect("encode ownership bytecode"))
            .expect("verify ownership bytecode");
    }
}
