//! Transitional typed-MIR to register-VM code generator.
//!
//! This module is intentionally narrow: it is the first physical execution
//! boundary for the MIR rollout, not a second source-language lowerer. It sees
//! only typed IDs and CFG blocks, then sends its result through the ordinary
//! bytecode encoder and verifier in [`super::compile_mir`].

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::rc::Rc;

use rsscript_mir::{
    BlockId, FunctionId, MirBinaryOp, MirCallArgument, MirCallTarget, MirFunction, MirInstruction,
    MirLiteral, MirModule, MirParameterMode, MirTerminator, PlaceId, ValueId,
};

use super::{
    EvalError, Reg, RegFunction, RegInstr, RegNativeSignature, RegUnit, compute_jit_eligibility,
    jit_function_has_loop,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MirCodegenError {
    Unsupported(&'static str),
    DuplicateFunctionName(String),
}

impl fmt::Display for MirCodegenError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(construct) => write!(
                formatter,
                "MIR-to-VM codegen does not support `{construct}` yet"
            ),
            Self::DuplicateFunctionName(name) => {
                write!(formatter, "MIR contains duplicate function name `{name}`")
            }
        }
    }
}

impl Error for MirCodegenError {}

pub(super) fn lower(mir: &MirModule) -> Result<RegUnit, EvalError> {
    mir.verify()
        .map_err(|error| EvalError::Runtime(format!("invalid MIR input: {error}")))?;

    let mut function_ids = HashMap::with_capacity(mir.functions().len());
    for function in mir.functions() {
        let name = function_name(mir, function.id())?.to_owned();
        let id = function.id().index();
        if function_ids.insert(name.clone(), id).is_some() {
            return Err(MirCodegenError::DuplicateFunctionName(name).into_eval_error());
        }
    }

    let mut functions = Vec::with_capacity(mir.functions().len());
    for function in mir.functions() {
        functions.push(Rc::new(lower_function(mir, function)?));
    }

    let eligibility = compute_jit_eligibility(
        &functions
            .iter()
            .map(|function| (**function).clone())
            .collect::<Vec<_>>(),
    );
    for (function, eligible) in functions.iter().zip(eligibility) {
        function
            .jit_analysis
            .set(Some((eligible, jit_function_has_loop(&function.code))));
    }

    let native_signatures = mir
        .functions()
        .iter()
        .map(|function| {
            let name = function_name(mir, function.id())?.to_owned();
            let signature = RegNativeSignature {
                params: function
                    .signature()
                    .parameter_types()
                    .iter()
                    .map(|ty| format!("{:?}", mir.ty(*ty).expect("validated type ID")))
                    .collect(),
                return_type: Some(format!(
                    "{:?}",
                    mir.ty(function.signature().result())
                        .expect("validated result type ID")
                )),
            };
            Ok((name, signature))
        })
        .collect::<Result<HashMap<_, _>, EvalError>>()?;

    Ok(RegUnit {
        functions,
        function_ids,
        resource_drop_functions: HashMap::new(),
        types: HashMap::new(),
        native_signatures,
        closure_identity_observable: false,
    })
}

trait IntoEvalError {
    fn into_eval_error(self) -> EvalError;
}

impl IntoEvalError for MirCodegenError {
    fn into_eval_error(self) -> EvalError {
        EvalError::Runtime(self.to_string())
    }
}

fn lower_function(mir: &MirModule, function: &MirFunction) -> Result<RegFunction, EvalError> {
    if function.signature().is_async() {
        return Err(MirCodegenError::Unsupported("async function").into_eval_error());
    }
    let name = function_name(mir, function.id())?.to_owned();
    let mut lowered = RegFunction::placeholder(name);
    lowered.params = function.signature().parameter_types().len();
    // Reserve one scratch register for synthetic Unit returns in unreachable
    // continuation blocks that the CFG lowerer leaves behind after an explicit
    // source-level return.
    lowered.regs = function.place_count() as usize + function.value_count() as usize + 1;
    lowered.local_regs = mir
        .function_debug(function.id())
        .expect("validated MIR function has debug metadata")
        .places()
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect();

    for (index, mode) in function.signature().parameter_modes().iter().enumerate() {
        // The established VM ABI deep-copies read parameters to retain the
        // source language's value isolation. `mut` aliases and `take` transfers
        // are intentionally left untouched.
        if *mode == MirParameterMode::Read {
            lowered.code.push(RegInstr::DeepCopy { reg: index });
        }
    }

    let mut block_starts = HashMap::with_capacity(function.blocks().len());
    let mut patches = Vec::new();
    for block in function.blocks() {
        block_starts.insert(block.id(), lowered.code.len());
        for instruction in block.instructions() {
            lower_instruction(mir, function, instruction, &mut lowered.code)?;
        }
        lower_terminator(
            function,
            block.terminator(),
            &mut lowered.code,
            &mut patches,
        )?;
    }
    for (instruction, target) in patches {
        let target_ip = *block_starts.get(&target).ok_or_else(|| {
            EvalError::Runtime(format!(
                "MIR codegen references missing block {}",
                target.index()
            ))
        })?;
        match &mut lowered.code[instruction] {
            RegInstr::Jump { target } | RegInstr::JumpIfBool { target, .. } => *target = target_ip,
            _ => {
                return Err(EvalError::Runtime(
                    "MIR codegen internal jump patch did not name a branch".to_string(),
                ));
            }
        }
    }
    Ok(lowered)
}

fn lower_instruction(
    mir: &MirModule,
    function: &MirFunction,
    instruction: &MirInstruction,
    code: &mut Vec<RegInstr>,
) -> Result<(), EvalError> {
    match instruction {
        MirInstruction::LoadLiteral { destination, value } => {
            code.push(load_literal(value, value_reg(function, *destination)))
        }
        MirInstruction::ReadPlace { destination, place }
        | MirInstruction::BorrowRead { destination, place } => {
            code.push(RegInstr::Move {
                dst: value_reg(function, *destination),
                src: place_reg(*place),
            });
        }
        MirInstruction::WritePlace { place, value } => {
            code.push(RegInstr::Move {
                dst: place_reg(*place),
                src: value_reg(function, *value),
            });
        }
        MirInstruction::Binary {
            destination,
            op,
            left,
            right,
        } => {
            code.push(binary_instruction(
                *op,
                value_reg(function, *destination),
                value_reg(function, *left),
                value_reg(function, *right),
            )?);
        }
        MirInstruction::Call {
            destination,
            target,
            arguments,
        } => {
            let (args, mut_args) = lower_call_arguments(function, arguments);
            let dst = value_reg(function, *destination);
            match target {
                MirCallTarget::Function(id) => code.push(RegInstr::CallKnown {
                    dst,
                    function: id.index(),
                    args,
                    mut_args,
                }),
                MirCallTarget::External(id) => {
                    let import = mir.external_imports().get(id.index()).ok_or_else(|| {
                        EvalError::Runtime(format!(
                            "MIR codegen references missing import {}",
                            id.index()
                        ))
                    })?;
                    code.push(RegInstr::CallExternal {
                        dst,
                        key: import.symbol().as_str().to_owned(),
                        args,
                        mut_args,
                    });
                }
            }
        }
        // MIR values are registers in the VM. A discard carries ownership facts
        // for validation but has no additional register-machine operation yet.
        MirInstruction::Discard { .. } => {}
    }
    Ok(())
}

fn lower_terminator(
    function: &MirFunction,
    terminator: &MirTerminator,
    code: &mut Vec<RegInstr>,
    patches: &mut Vec<(usize, BlockId)>,
) -> Result<(), EvalError> {
    match terminator {
        MirTerminator::Return(value) => {
            let src = value.map_or_else(
                || {
                    let scratch = function.place_count() as usize + function.value_count() as usize;
                    code.push(RegInstr::LoadUnit { dst: scratch });
                    scratch
                },
                |value| value_reg(function, value),
            );
            code.push(RegInstr::Return { src });
        }
        MirTerminator::Jump(target) => {
            let instruction = code.len();
            code.push(RegInstr::Jump { target: 0 });
            patches.push((instruction, *target));
        }
        MirTerminator::Branch {
            condition,
            then_target,
            else_target,
        } => {
            let branch = code.len();
            code.push(RegInstr::JumpIfBool {
                cond: value_reg(function, *condition),
                expected: true,
                target: 0,
            });
            patches.push((branch, *then_target));
            let otherwise = code.len();
            code.push(RegInstr::Jump { target: 0 });
            patches.push((otherwise, *else_target));
        }
        MirTerminator::Unreachable => code.push(RegInstr::RuntimeError {
            message: "entered unreachable MIR block".to_string(),
        }),
    }
    Ok(())
}

fn load_literal(value: &MirLiteral, dst: Reg) -> RegInstr {
    match value {
        MirLiteral::Unit => RegInstr::LoadUnit { dst },
        MirLiteral::Int(value) => RegInstr::LoadInt { dst, value: *value },
        MirLiteral::Float(value) => RegInstr::LoadFloat { dst, value: *value },
        MirLiteral::Bool(value) => RegInstr::LoadBool { dst, value: *value },
        MirLiteral::String(value) => RegInstr::LoadString {
            dst,
            value: Rc::new(value.clone()),
        },
        MirLiteral::Char(value) => RegInstr::LoadChar { dst, value: *value },
    }
}

fn binary_instruction(
    op: MirBinaryOp,
    dst: Reg,
    lhs: Reg,
    rhs: Reg,
) -> Result<RegInstr, EvalError> {
    let instruction = match op {
        MirBinaryOp::Add => RegInstr::AddInt { dst, lhs, rhs },
        MirBinaryOp::Subtract => RegInstr::SubInt { dst, lhs, rhs },
        MirBinaryOp::Multiply => RegInstr::MulInt { dst, lhs, rhs },
        MirBinaryOp::Divide => RegInstr::DivInt { dst, lhs, rhs },
        MirBinaryOp::Modulo => RegInstr::ModInt { dst, lhs, rhs },
        MirBinaryOp::BitAnd => RegInstr::BitAndInt { dst, lhs, rhs },
        MirBinaryOp::BitOr => RegInstr::BitOrInt { dst, lhs, rhs },
        MirBinaryOp::BitXor => RegInstr::BitXorInt { dst, lhs, rhs },
        MirBinaryOp::ShiftLeft => RegInstr::ShiftLeftInt { dst, lhs, rhs },
        MirBinaryOp::ShiftRight => RegInstr::ShiftRightInt { dst, lhs, rhs },
        MirBinaryOp::Equal => RegInstr::Equal { dst, lhs, rhs },
        MirBinaryOp::NotEqual => RegInstr::NotEqual { dst, lhs, rhs },
        MirBinaryOp::Less => RegInstr::LessInt { dst, lhs, rhs },
        MirBinaryOp::LessEqual => RegInstr::LessEqualInt { dst, lhs, rhs },
        MirBinaryOp::Greater => RegInstr::GreaterInt { dst, lhs, rhs },
        MirBinaryOp::GreaterEqual => RegInstr::GreaterEqualInt { dst, lhs, rhs },
        MirBinaryOp::LogicalAnd | MirBinaryOp::LogicalOr => {
            return Err(MirCodegenError::Unsupported("logical binary operation").into_eval_error());
        }
    };
    Ok(instruction)
}

fn lower_call_arguments(
    function: &MirFunction,
    arguments: &[MirCallArgument],
) -> (Vec<Reg>, Vec<usize>) {
    let mut args = Vec::with_capacity(arguments.len());
    let mut mut_args = Vec::new();
    for (index, argument) in arguments.iter().enumerate() {
        match argument {
            MirCallArgument::Value(value) => args.push(value_reg(function, *value)),
            MirCallArgument::BorrowRead(place) | MirCallArgument::Take(place) => {
                args.push(place_reg(*place))
            }
            MirCallArgument::BorrowMut(place) => {
                args.push(place_reg(*place));
                mut_args.push(index);
            }
        }
    }
    (args, mut_args)
}

fn function_name(mir: &MirModule, id: FunctionId) -> Result<&str, EvalError> {
    mir.function_debug(id)
        .map(|debug| debug.name())
        .ok_or_else(|| {
            EvalError::Runtime(format!(
                "MIR codegen has no debug name for function {}",
                id.index()
            ))
        })
}

fn place_reg(place: PlaceId) -> Reg {
    place.index()
}

fn value_reg(function: &MirFunction, value: ValueId) -> Reg {
    function.place_count() as usize + value.index()
}
