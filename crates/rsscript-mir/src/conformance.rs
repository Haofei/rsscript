//! Test-only migration support for the typed MIR rollout.
//!
//! This deliberately small interpreter is an oracle for the currently
//! migrated pure subset. It is not a production VM and is only compiled with
//! the `conformance` feature. Migration tests use it to compare the legacy VM
//! path with the typed MIR path before a capability can become MIR-only.

use std::fmt;

use crate::{
    FunctionId, MirBinaryOp, MirCallArgument, MirCallTarget, MirInstruction, MirLiteral, MirModule,
    MirTerminator, ValueId,
};

/// Lifecycle of a language capability during the MIR migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationStage {
    /// The feature remains only on the legacy executable-IR path.
    LegacyOnly,
    /// Both paths execute it and are required to agree in the conformance
    /// corpus.
    DualPath,
    /// The feature has a MIR execution path and may no longer add legacy-only
    /// behavior.
    MirOnly,
}

/// Declarative entry used by the migration corpus.
#[derive(Debug, Clone, Copy)]
pub struct MigrationCase {
    pub name: &'static str,
    pub capability: &'static str,
    pub stage: MigrationStage,
    pub source: &'static str,
}

/// Scalar value model used only by the reference interpreter.
#[derive(Debug, Clone, PartialEq)]
pub enum MirValue {
    Unit,
    Int(i64),
    Float(f64),
    Bool(bool),
    String(String),
    Char(char),
    List(Vec<MirValue>),
    Map(Vec<(MirValue, MirValue)>),
    JsonObject(Vec<(String, MirValue)>),
    Record {
        name: String,
        fields: Vec<(String, MirValue)>,
    },
    Variant {
        name: String,
        fields: Vec<(String, MirValue)>,
    },
    ResultOk(Box<MirValue>),
    ResultErr(Box<MirValue>),
}

impl MirValue {
    /// Render using the legacy VM's scalar result convention.
    pub fn render(&self) -> String {
        match self {
            Self::Unit => "Unit".into(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::String(value) => value.clone(),
            Self::Char(value) => value.to_string(),
            Self::List(items) => format!(
                "[{}]",
                items
                    .iter()
                    .map(Self::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Map(entries) => {
                let mut rendered = entries
                    .iter()
                    .map(|(key, value)| format!("{}: {}", key.render(), value.render()))
                    .collect::<Vec<_>>();
                rendered.sort();
                format!("{{{}}}", rendered.join(", "))
            }
            Self::JsonObject(fields) => {
                let mut rendered = fields
                    .iter()
                    .map(|(name, value)| format!("\"{name}\":{}", value.render()))
                    .collect::<Vec<_>>();
                rendered.sort();
                format!("{{{}}}", rendered.join(","))
            }
            Self::Record { name, fields } => {
                let fields = fields
                    .iter()
                    .map(|(field, value)| format!("{field}: {}", value.render()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}({fields})")
            }
            Self::Variant { name, fields } => {
                let fields = fields
                    .iter()
                    .map(|(field, value)| format!("{field}: {}", value.render()))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}({fields})")
            }
            Self::ResultOk(value) => format!("Ok({})", value.render()),
            Self::ResultErr(value) => format!("Err({})", value.render()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirExecutionError {
    MissingEntrypoint(String),
    InvalidArgumentCount {
        function: FunctionId,
        expected: usize,
        actual: usize,
    },
    UninitializedValue(ValueId),
    UninitializedPlace(usize),
    InvalidBranchCondition,
    InvalidOperation(&'static str),
    DivisionByZero,
    UnsupportedExternalCall,
    UnsupportedStructuredConcurrency,
    RecursionLimit,
    StepLimit,
}

impl fmt::Display for MirExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "MIR conformance execution failed: {self:?}")
    }
}

impl std::error::Error for MirExecutionError {}

/// Execute a pure MIR module from its debug-name entry point.
pub fn execute_named(
    module: &MirModule,
    entry: &str,
    arguments: Vec<MirValue>,
) -> Result<MirValue, MirExecutionError> {
    let function = module
        .functions()
        .iter()
        .find(|function| {
            module
                .function_debug(function.id())
                .is_some_and(|debug| debug.name() == entry)
        })
        .map(|function| function.id())
        .ok_or_else(|| MirExecutionError::MissingEntrypoint(entry.into()))?;
    let mut interpreter = Interpreter {
        module,
        steps_remaining: 100_000,
        recursion_remaining: 128,
    };
    interpreter
        .call(function, arguments)
        .map(|outcome| outcome.value)
}

struct FrameOutcome {
    value: MirValue,
    places: Vec<Option<MirValue>>,
}

struct Interpreter<'a> {
    module: &'a MirModule,
    steps_remaining: u64,
    recursion_remaining: u32,
}

impl<'a> Interpreter<'a> {
    fn call(
        &mut self,
        function_id: FunctionId,
        arguments: Vec<MirValue>,
    ) -> Result<FrameOutcome, MirExecutionError> {
        if self.recursion_remaining == 0 {
            return Err(MirExecutionError::RecursionLimit);
        }
        self.recursion_remaining -= 1;
        let result = self.call_inner(function_id, arguments);
        self.recursion_remaining += 1;
        result
    }

    fn call_inner(
        &mut self,
        function_id: FunctionId,
        arguments: Vec<MirValue>,
    ) -> Result<FrameOutcome, MirExecutionError> {
        let function = self
            .module
            .function(function_id)
            .expect("verified MIR function target must exist");
        if arguments.len() != function.signature().parameter_types().len() {
            return Err(MirExecutionError::InvalidArgumentCount {
                function: function_id,
                expected: function.signature().parameter_types().len(),
                actual: arguments.len(),
            });
        }
        let mut places = vec![None; function.place_count() as usize];
        for (index, value) in arguments.into_iter().enumerate() {
            places[index] = Some(value);
        }
        let mut values = vec![None; function.value_count() as usize];
        let mut block = 0usize;
        loop {
            let current = &function.blocks()[block];
            for instruction in current.instructions() {
                self.step()?;
                match instruction {
                    MirInstruction::LoadLiteral { destination, value } => {
                        values[destination.index()] = Some(literal(value));
                    }
                    MirInstruction::MakeList { destination, items } => {
                        values[destination.index()] = Some(MirValue::List(
                            items
                                .iter()
                                .map(|item| value_at(&values, *item))
                                .collect::<Result<Vec<_>, _>>()?,
                        ));
                    }
                    MirInstruction::MakeMap {
                        destination,
                        entries,
                    } => {
                        values[destination.index()] = Some(MirValue::Map(
                            entries
                                .iter()
                                .map(|(key, value)| {
                                    Ok((value_at(&values, *key)?, value_at(&values, *value)?))
                                })
                                .collect::<Result<Vec<_>, MirExecutionError>>()?,
                        ));
                    }
                    MirInstruction::MakeObject {
                        destination,
                        fields,
                    } => {
                        values[destination.index()] = Some(MirValue::JsonObject(
                            fields
                                .iter()
                                .map(|(name, value)| Ok((name.clone(), value_at(&values, *value)?)))
                                .collect::<Result<Vec<_>, MirExecutionError>>()?,
                        ));
                    }
                    MirInstruction::MakeStruct {
                        destination,
                        ty,
                        fields,
                    } => {
                        let name = match self.module.ty(*ty) {
                            Some(rsscript_abi_model::WireType::Named { name, .. }) => name.clone(),
                            _ => return Err(MirExecutionError::InvalidOperation("record type")),
                        };
                        values[destination.index()] = Some(MirValue::Record {
                            name,
                            fields: fields
                                .iter()
                                .map(|(field, value)| {
                                    Ok((field.clone(), value_at(&values, *value)?))
                                })
                                .collect::<Result<Vec<_>, MirExecutionError>>()?,
                        });
                    }
                    MirInstruction::MakeVariant {
                        destination,
                        variant,
                        fields,
                        ..
                    } => {
                        values[destination.index()] = Some(MirValue::Variant {
                            name: variant.clone(),
                            fields: fields
                                .iter()
                                .map(|(field, value)| {
                                    Ok((field.clone(), value_at(&values, *value)?))
                                })
                                .collect::<Result<Vec<_>, MirExecutionError>>()?,
                        });
                    }
                    MirInstruction::MakeResult {
                        destination,
                        ok,
                        value,
                    } => {
                        let value = value_at(&values, *value)?;
                        values[destination.index()] = Some(if *ok {
                            MirValue::ResultOk(Box::new(value))
                        } else {
                            MirValue::ResultErr(Box::new(value))
                        });
                    }
                    MirInstruction::TryResult {
                        destination,
                        source,
                        cleanup,
                    } => match value_at(&values, *source)? {
                        MirValue::ResultOk(value) => {
                            values[destination.index()] = Some(*value);
                        }
                        failure @ MirValue::ResultErr(_) => {
                            for place in cleanup {
                                let _ = places[place.index()]
                                    .take()
                                    .ok_or(MirExecutionError::UninitializedPlace(place.index()))?;
                            }
                            return Ok(FrameOutcome {
                                value: failure,
                                places,
                            });
                        }
                        _ => return Err(MirExecutionError::InvalidOperation("Result try source")),
                    },
                    MirInstruction::ListGet {
                        destination,
                        list,
                        index,
                    } => {
                        let index = match value_at(&values, *index)? {
                            MirValue::Int(value) if value >= 0 => value as usize,
                            _ => return Err(MirExecutionError::InvalidOperation("list index")),
                        };
                        let value = match value_at(&values, *list)? {
                            MirValue::List(items) => items.get(index).cloned().ok_or(
                                MirExecutionError::InvalidOperation("list index out of bounds"),
                            )?,
                            _ => return Err(MirExecutionError::InvalidOperation("list base")),
                        };
                        values[destination.index()] = Some(value);
                    }
                    MirInstruction::GetField {
                        destination,
                        base,
                        field,
                    } => {
                        let value = match value_at(&values, *base)? {
                            MirValue::JsonObject(fields)
                            | MirValue::Record { fields, .. }
                            | MirValue::Variant { fields, .. } => fields
                                .into_iter()
                                .find_map(|(name, value)| (name == *field).then_some(value))
                                .ok_or(MirExecutionError::InvalidOperation(
                                    "missing object field",
                                ))?,
                            _ => return Err(MirExecutionError::InvalidOperation("field base")),
                        };
                        values[destination.index()] = Some(value);
                    }
                    MirInstruction::ListLen { destination, list } => {
                        let length = match value_at(&values, *list)? {
                            MirValue::List(items) => items.len() as i64,
                            _ => return Err(MirExecutionError::InvalidOperation("list base")),
                        };
                        values[destination.index()] = Some(MirValue::Int(length));
                    }
                    MirInstruction::ReadPlace { destination, place } => {
                        values[destination.index()] = Some(
                            places[place.index()]
                                .clone()
                                .ok_or(MirExecutionError::UninitializedPlace(place.index()))?,
                        );
                    }
                    MirInstruction::BorrowRead { destination, place } => {
                        values[destination.index()] = Some(
                            places[place.index()]
                                .clone()
                                .ok_or(MirExecutionError::UninitializedPlace(place.index()))?,
                        );
                    }
                    MirInstruction::TakePlace { destination, place } => {
                        values[destination.index()] = Some(
                            places[place.index()]
                                .take()
                                .ok_or(MirExecutionError::UninitializedPlace(place.index()))?,
                        );
                    }
                    MirInstruction::Retain { place } => {
                        let _ = place_value(&places, place.index())?;
                    }
                    MirInstruction::Drop { place } => {
                        let _ = places[place.index()]
                            .take()
                            .ok_or(MirExecutionError::UninitializedPlace(place.index()))?;
                    }
                    MirInstruction::AcquireResource { place, source, .. } => {
                        // This migration oracle has no host resource implementation,
                        // but it preserves the acquired source value in the live place.
                        places[place.index()] = Some(value_at(&values, *source)?);
                    }
                    MirInstruction::ReleaseResource { place } => {
                        let _ = places[place.index()]
                            .take()
                            .ok_or(MirExecutionError::UninitializedPlace(place.index()))?;
                    }
                    MirInstruction::Spawn { .. }
                    | MirInstruction::Await { .. }
                    | MirInstruction::Cancel { .. }
                    | MirInstruction::Join { .. } => {
                        return Err(MirExecutionError::UnsupportedStructuredConcurrency);
                    }
                    MirInstruction::WritePlace { place, value } => {
                        places[place.index()] = Some(value_at(&values, *value)?);
                    }
                    MirInstruction::Binary {
                        destination,
                        op,
                        left,
                        right,
                    } => {
                        values[destination.index()] = Some(binary(
                            *op,
                            value_at(&values, *left)?,
                            value_at(&values, *right)?,
                        )?);
                    }
                    MirInstruction::Call {
                        destination,
                        target,
                        arguments,
                    } => {
                        let mut call_arguments = Vec::with_capacity(arguments.len());
                        let mut writebacks = Vec::new();
                        for (index, argument) in arguments.iter().enumerate() {
                            match argument {
                                MirCallArgument::Value(value) => {
                                    call_arguments.push(value_at(&values, *value)?);
                                }
                                MirCallArgument::BorrowRead(place) => {
                                    call_arguments.push(place_value(&places, place.index())?);
                                }
                                MirCallArgument::BorrowMut(place) => {
                                    call_arguments.push(place_value(&places, place.index())?);
                                    writebacks.push((place.index(), index));
                                }
                                MirCallArgument::Take(place) => {
                                    call_arguments.push(places[place.index()].take().ok_or(
                                        MirExecutionError::UninitializedPlace(place.index()),
                                    )?);
                                }
                            }
                        }
                        let outcome = match target {
                            MirCallTarget::Function(function) => {
                                self.call(*function, call_arguments)?
                            }
                            MirCallTarget::External(_) => {
                                return Err(MirExecutionError::UnsupportedExternalCall);
                            }
                        };
                        for (caller_place, callee_parameter) in writebacks {
                            places[caller_place] = outcome.places[callee_parameter].clone();
                        }
                        values[destination.index()] = Some(outcome.value);
                    }
                    MirInstruction::Discard { value } => {
                        let _ = value_at(&values, *value)?;
                    }
                }
            }
            self.step()?;
            match current.terminator() {
                MirTerminator::Return(value) => {
                    let value = value
                        .map(|value| value_at(&values, value))
                        .transpose()
                        .map(|value| value.unwrap_or(MirValue::Unit))?;
                    return Ok(FrameOutcome { value, places });
                }
                MirTerminator::Jump(target) => block = target.index(),
                MirTerminator::Branch {
                    condition,
                    then_target,
                    else_target,
                } => match value_at(&values, *condition)? {
                    MirValue::Bool(true) => block = then_target.index(),
                    MirValue::Bool(false) => block = else_target.index(),
                    _ => return Err(MirExecutionError::InvalidBranchCondition),
                },
                MirTerminator::MatchVariant {
                    value,
                    expected,
                    match_target,
                    else_target,
                } => match value_at(&values, *value)? {
                    MirValue::Variant { name, .. } if name == *expected => {
                        block = match_target.index();
                    }
                    _ => block = else_target.index(),
                },
                MirTerminator::Unreachable => {
                    return Err(MirExecutionError::InvalidOperation("unreachable"));
                }
            }
        }
    }

    fn step(&mut self) -> Result<(), MirExecutionError> {
        self.steps_remaining = self
            .steps_remaining
            .checked_sub(1)
            .ok_or(MirExecutionError::StepLimit)?;
        Ok(())
    }
}

fn value_at(values: &[Option<MirValue>], id: ValueId) -> Result<MirValue, MirExecutionError> {
    values[id.index()]
        .clone()
        .ok_or(MirExecutionError::UninitializedValue(id))
}

fn place_value(places: &[Option<MirValue>], index: usize) -> Result<MirValue, MirExecutionError> {
    places[index]
        .clone()
        .ok_or(MirExecutionError::UninitializedPlace(index))
}

fn literal(value: &MirLiteral) -> MirValue {
    match value {
        MirLiteral::Unit => MirValue::Unit,
        MirLiteral::Int(value) => MirValue::Int(*value),
        MirLiteral::Float(value) => MirValue::Float(*value),
        MirLiteral::Bool(value) => MirValue::Bool(*value),
        MirLiteral::String(value) => MirValue::String(value.clone()),
        MirLiteral::Char(value) => MirValue::Char(*value),
    }
}

fn binary(op: MirBinaryOp, left: MirValue, right: MirValue) -> Result<MirValue, MirExecutionError> {
    use MirBinaryOp as Op;
    use MirValue as Value;
    match (op, left, right) {
        (Op::Add, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left + right)),
        (Op::Subtract, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left - right)),
        (Op::Multiply, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left * right)),
        (Op::Divide, Value::Int(_), Value::Int(0)) | (Op::Modulo, Value::Int(_), Value::Int(0)) => {
            Err(MirExecutionError::DivisionByZero)
        }
        (Op::Divide, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left / right)),
        (Op::Modulo, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left % right)),
        (Op::BitAnd, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left & right)),
        (Op::BitOr, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left | right)),
        (Op::BitXor, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left ^ right)),
        (Op::ShiftLeft, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left << right)),
        (Op::ShiftRight, Value::Int(left), Value::Int(right)) => Ok(Value::Int(left >> right)),
        (Op::Add, Value::Float(left), Value::Float(right)) => Ok(Value::Float(left + right)),
        (Op::Subtract, Value::Float(left), Value::Float(right)) => Ok(Value::Float(left - right)),
        (Op::Multiply, Value::Float(left), Value::Float(right)) => Ok(Value::Float(left * right)),
        (Op::Divide, Value::Float(_), Value::Float(0.0)) => Err(MirExecutionError::DivisionByZero),
        (Op::Divide, Value::Float(left), Value::Float(right)) => Ok(Value::Float(left / right)),
        (Op::Equal, left, right) => Ok(Value::Bool(left == right)),
        (Op::NotEqual, left, right) => Ok(Value::Bool(left != right)),
        (Op::Less, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left < right)),
        (Op::LessEqual, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left <= right)),
        (Op::Greater, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left > right)),
        (Op::GreaterEqual, Value::Int(left), Value::Int(right)) => Ok(Value::Bool(left >= right)),
        (Op::Less, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left < right)),
        (Op::LessEqual, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left <= right)),
        (Op::Greater, Value::Float(left), Value::Float(right)) => Ok(Value::Bool(left > right)),
        (Op::GreaterEqual, Value::Float(left), Value::Float(right)) => {
            Ok(Value::Bool(left >= right))
        }
        (Op::LogicalAnd, Value::Bool(left), Value::Bool(right)) => Ok(Value::Bool(left && right)),
        (Op::LogicalOr, Value::Bool(left), Value::Bool(right)) => Ok(Value::Bool(left || right)),
        _ => Err(MirExecutionError::InvalidOperation("binary operand types")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BasicBlock, BlockId, MirFunction, MirFunctionDebug, MirFunctionSignature, TypeId, WireType,
    };

    #[test]
    fn result_try_returns_the_failure_from_the_current_frame() {
        let types = vec![
            WireType::Unit,
            WireType::Int {
                bits: 64,
                signed: true,
            },
            WireType::String,
            WireType::Result {
                ok: Box::new(WireType::Int {
                    bits: 64,
                    signed: true,
                }),
                error: Box::new(WireType::String),
            },
        ];
        let result = TypeId::new(3);
        let fail = MirFunction::new(
            FunctionId::new(0),
            MirFunctionSignature::new(vec![], result, false),
            0,
            2,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::LoadLiteral {
                        destination: ValueId::new(0),
                        value: MirLiteral::String("boom".into()),
                    },
                    MirInstruction::MakeResult {
                        destination: ValueId::new(1),
                        ok: false,
                        value: ValueId::new(0),
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(1))),
            )],
        );
        let main = MirFunction::new(
            FunctionId::new(1),
            MirFunctionSignature::new(vec![], result, false),
            0,
            2,
            vec![BasicBlock::new(
                BlockId::new(0),
                vec![
                    MirInstruction::Call {
                        destination: ValueId::new(0),
                        target: MirCallTarget::Function(FunctionId::new(0)),
                        arguments: vec![],
                    },
                    MirInstruction::TryResult {
                        destination: ValueId::new(1),
                        source: ValueId::new(0),
                        cleanup: vec![],
                    },
                ],
                MirTerminator::Return(Some(ValueId::new(1))),
            )],
        );
        let module = MirModule::new(
            types,
            vec![fail, main],
            vec![
                MirFunctionDebug::new("fail", vec![]),
                MirFunctionDebug::new("main", vec![]),
            ],
            vec![],
        )
        .expect("Result MIR should verify");

        assert_eq!(
            execute_named(&module, "main", vec![])
                .expect("execute")
                .render(),
            "Err(boom)"
        );
    }
}
