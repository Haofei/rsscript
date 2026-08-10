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
    MirModule, MirParameterMode, MirTerminator, PlaceId, ValueId,
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
    mir: &MirModule,
    source_content_hash: &str,
    interface_catalog_digest: &str,
    compiler_provenance: &str,
) -> Result<BytecodeArtifact, CodegenError> {
    mir.verify()
        .map_err(|error| CodegenError::InvalidMir(error.to_string()))?;
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
    let mut functions = Vec::new();
    for function in mir.functions() {
        let name = function_name(mir, function)?.to_owned();
        if ids.insert(name.clone(), function.id().index()).is_some() {
            return Err(CodegenError::DuplicateFunctionName(name));
        }
        native_signatures.insert(
            name.clone(),
            json!({
                "params": function.signature().parameter_types().iter().map(|id| format!("{:?}", mir.ty(*id).expect("validated type"))).collect::<Vec<_>>(),
                "return_type": format!("{:?}", mir.ty(function.signature().result()).expect("validated result type")),
            }),
        );
        functions.push(wire_function(mir, function)?);
    }
    Ok(json!({
        "functions": functions,
        "function_ids": ids,
        "resource_drop_functions": BTreeMap::<String, usize>::new(),
        "types": BTreeMap::<String, Value>::new(),
        "native_signatures": native_signatures,
        "closure_identity_observable": false,
    }))
}

fn wire_function(mir: &MirModule, function: &MirFunction) -> Result<Value, CodegenError> {
    if function.signature().is_async() {
        return Err(CodegenError::Unsupported("async function"));
    }
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
        "captures": 0, "regs": function.place_count() as usize + function.value_count() as usize + 1,
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
        MirInstruction::ReadPlace { destination, place }
        | MirInstruction::BorrowRead { destination, place }
        | MirInstruction::TakePlace { destination, place } => code.push(instr(
            "Move",
            [
                ("dst", json!(value_reg(function, *destination))),
                ("src", json!(place_reg(*place))),
            ],
        )),
        MirInstruction::Retain { .. } => return Err(CodegenError::Unsupported("retain")),
        MirInstruction::Drop { .. } => return Err(CodegenError::Unsupported("drop")),
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

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_abi_model::{CORE_LIBRARY_ABI_VERSION, RUNTIME_ABI_VERSION, WireType};
    use rsscript_bytecode::BytecodeVerifier;
    use rsscript_mir::{BasicBlock, FunctionId, MirFunctionDebug, MirFunctionSignature, TypeId};

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
}
