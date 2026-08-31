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

mod facts;
use facts::{effect, fact_type, legacy_signature_type, ownership, typed_executable_facts};

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
mod tests;
