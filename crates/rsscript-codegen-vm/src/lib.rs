#![forbid(unsafe_code)]

//! MIR-only scalar CFG code generator.
//!
//! The emitted payload deliberately follows the existing v1 register-bytecode
//! wire contract, but this crate has no dependency on the VM implementation.
//! That makes code generation independently testable and keeps the VM on the
//! load/link/execute side of the boundary.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use rsscript_abi_model::{ExternalImport, RUNTIME_ABI_VERSION};
use rsscript_bytecode::{BytecodeArtifact, BytecodeError, LANGUAGE_SEMANTICS_VERSION};
use rsscript_mir::{
    BlockId, MirBinaryOp, MirCallArgument, MirCallTarget, MirFunction, MirInstruction, MirLiteral,
    MirModule, MirParameterMode, MirTerminator, PlaceId, TaskId, ValueId, VerifiedMir,
    builtin_vm_name,
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
    let payload =
        rsscript_bytecode::encode_executable_payload(&wire_unit(mir)?).map_err(bytecode_error)?;
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
    BytecodeArtifact::new(
        LANGUAGE_SEMANTICS_VERSION,
        compiler_provenance,
        interface_catalog_digest,
        RUNTIME_ABI_VERSION,
        source_content_hash,
        imports,
        payload,
    )
    .map_err(bytecode_error)
}

fn bytecode_error(error: BytecodeError) -> CodegenError {
    CodegenError::Bytecode(error.to_string())
}

fn wire_unit(mir: &MirModule) -> Result<Value, CodegenError> {
    let mut ids = BTreeMap::new();
    let mut native_signatures = BTreeMap::new();
    let mut types = BTreeMap::new();
    let mut functions = Vec::new();
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
        functions.push(wire_function(mir, function)?);
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
    Ok(json!({
        "functions": functions,
        "function_ids": ids,
        "resource_drop_functions": BTreeMap::<String, usize>::new(),
        "types": types,
        "native_signatures": native_signatures,
        "closure_identity_observable": false,
    }))
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

fn wire_function(mir: &MirModule, function: &MirFunction) -> Result<Value, CodegenError> {
    let mut code = Vec::new();
    for (index, mode) in function.signature().parameter_modes().iter().enumerate() {
        if *mode == MirParameterMode::Read {
            code.push(instr("DeepCopy", [("reg", json!(index))]));
        }
    }
    let mut starts = BTreeMap::new();
    let mut patches = Vec::new();
    for block in function.blocks() {
        starts.insert(block.id(), code.len());
        for instruction in block.instructions() {
            lower_instruction(mir, function, instruction, &mut code)?;
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
    let locals = mir
        .function_debug(function.id())
        .expect("validated MIR debug table")
        .places()
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), json!(index)))
        .collect::<Map<_, _>>();
    Ok(json!({
        "name": function_name(mir, function)?, "params": function.signature().parameter_types().len(),
        "captures": 0, "regs": function.place_count() as usize + function.value_count() as usize + task_count(function) + 1,
        "local_regs": locals, "code": code,
    }))
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
        MirInstruction::AcquireResource { place, source, .. } => code.push(instr(
            "Move",
            [
                ("dst", json!(place_reg(*place))),
                ("src", json!(value_reg(function, *source))),
            ],
        )),
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
        MirInstruction::Cancel { .. } => {
            return Err(CodegenError::Unsupported("task cancellation"));
        }
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
                MirCallTarget::Function(id) => code.push(instr(
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
                    let intrinsic = builtin_vm_name(*id).ok_or(CodegenError::InvalidMir(
                        "builtin call references missing catalog identity".to_owned(),
                    ))?;
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
        BasicBlock, FunctionId, MirFunctionDebug, MirFunctionSignature, ResourceTypeId,
        TaskGroupId, TypeId,
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
            vec![MirFunctionDebug::new("main", vec![])],
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
        BytecodeVerifier::default()
            .verify(&artifact.to_bytes().unwrap())
            .unwrap();
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
            vec![WireType::Unit],
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
