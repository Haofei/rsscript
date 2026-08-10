//! Transitional lowering from owned executable IR to typed CFG MIR.
//!
//! This bridge deliberately supports only the pure control-flow subset. It
//! gives the new backend boundary an executable-independent, verified model
//! without pretending that resources, structured async, or external calls have
//! already migrated. Unsupported nodes fail closed and stay on the legacy path.

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fmt;

use rsscript_abi_model::WireType;
use rsscript_exec_ir::{
    BinaryOp, Callee, ExecutableExpr, ExecutableFunction, ExecutableIr, ExecutableStmt, ParamEffect,
};
use rsscript_mir::{
    BasicBlock, BlockId, FunctionId, MirBinaryOp, MirCallArgument, MirCallTarget,
    MirExternalImport, MirFunction, MirFunctionDebug, MirFunctionSignature, MirInstruction,
    MirLiteral, MirModule, MirParameterMode, MirTerminator, PlaceId, ResourceTypeId, TaskGroupId,
    TaskId, TypeId, ValueId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirLoweringError {
    Unsupported {
        function: String,
        construct: &'static str,
    },
    Invalid(rsscript_mir::MirValidationError),
}

impl fmt::Display for MirLoweringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported {
                function,
                construct,
            } => write!(
                formatter,
                "cannot lower `{function}` to the typed MIR control-flow subset: {construct}"
            ),
            Self::Invalid(error) => error.fmt(formatter),
        }
    }
}

impl Error for MirLoweringError {}

impl From<rsscript_mir::MirValidationError> for MirLoweringError {
    fn from(value: rsscript_mir::MirValidationError) -> Self {
        Self::Invalid(value)
    }
}

/// Lower the currently supported pure subset of owned executable IR into typed
/// CFG MIR. This is intentionally a transitional entry point; the final path
/// will consume checked HIR directly once all semantic facts are owned by the
/// semantics query boundary.
pub fn lower_executable_ir_to_mir(
    executable: &ExecutableIr,
) -> Result<MirModule, MirLoweringError> {
    let functions = executable.functions().collect::<Vec<_>>();
    let mut types = TypeTable::default();
    let signatures = functions
        .iter()
        .map(|function| types.function_signature(&function.signature))
        .collect::<Vec<_>>();
    let targets = CallTargets {
        functions: functions
            .iter()
            .enumerate()
            .map(|(index, function)| (function.name.clone(), FunctionId::new(index as u32)))
            .collect(),
        external_imports: executable
            .external_imports()
            .iter()
            .enumerate()
            .map(|(index, import)| {
                (
                    import.symbol.as_str().to_owned(),
                    rsscript_mir::ExternalSymbolId::new(index as u32),
                )
            })
            .collect(),
    };
    let mut lowered = Vec::with_capacity(functions.len());
    let mut debug = Vec::with_capacity(functions.len());
    for ((index, function), signature) in functions.iter().enumerate().zip(signatures) {
        let output = FunctionLowerer::new(
            FunctionId::new(index as u32),
            function,
            signature,
            targets.clone(),
            &mut types,
        )
        .lower()?;
        lowered.push(output.function);
        debug.push(output.debug);
    }
    let imports = executable
        .external_imports()
        .iter()
        .enumerate()
        .map(|(index, import)| {
            MirExternalImport::new(
                rsscript_mir::ExternalSymbolId::new(index as u32),
                import.symbol.clone(),
                import.signature.clone(),
            )
        })
        .collect();
    Ok(MirModule::new(types.into_types(), lowered, debug, imports)?)
}

#[derive(Clone)]
struct CallTargets {
    functions: BTreeMap<String, FunctionId>,
    external_imports: BTreeMap<String, rsscript_mir::ExternalSymbolId>,
}

#[derive(Default)]
struct TypeTable {
    ids: BTreeMap<WireType, TypeId>,
    types: Vec<WireType>,
}

impl TypeTable {
    fn function_signature(
        &mut self,
        signature: &rsscript_exec_ir::ExecutableSignature,
    ) -> MirFunctionSignature {
        MirFunctionSignature::with_modes(
            signature
                .params
                .iter()
                .map(|parameter| self.intern(WireType::parse(&parameter.type_name)))
                .collect(),
            signature
                .params
                .iter()
                .map(
                    |parameter| match parameter.effect.unwrap_or(ParamEffect::Read) {
                        ParamEffect::Read => MirParameterMode::Read,
                        ParamEffect::Mut => MirParameterMode::Mut,
                        ParamEffect::Take => MirParameterMode::Take,
                    },
                )
                .collect(),
            self.intern(
                signature
                    .return_type
                    .as_deref()
                    .map(WireType::parse)
                    .unwrap_or(WireType::Unit),
            ),
            signature.is_async,
        )
    }

    fn intern(&mut self, ty: WireType) -> TypeId {
        if let Some(id) = self.ids.get(&ty) {
            return *id;
        }
        let id = TypeId::new(self.types.len() as u32);
        self.types.push(ty.clone());
        self.ids.insert(ty, id);
        id
    }

    fn into_types(self) -> Vec<WireType> {
        self.types
    }
}

struct LoweredFunction {
    function: MirFunction,
    debug: MirFunctionDebug,
}

struct BlockDraft {
    instructions: Vec<MirInstruction>,
    terminator: Option<MirTerminator>,
}

impl BlockDraft {
    fn new() -> Self {
        Self {
            instructions: Vec::new(),
            terminator: None,
        }
    }
}

struct LoopTargets {
    continue_target: BlockId,
    break_target: BlockId,
    cleanup_depth: usize,
}

struct FunctionLowerer<'source, 'types> {
    id: FunctionId,
    source: &'source ExecutableFunction,
    signature: MirFunctionSignature,
    targets: CallTargets,
    types: &'types mut TypeTable,
    blocks: Vec<BlockDraft>,
    current: BlockId,
    places: HashMap<String, PlaceId>,
    place_names: Vec<String>,
    next_value: u32,
    tasks: HashMap<String, TaskId>,
    next_task: u32,
    loops: Vec<LoopTargets>,
    resource_scopes: Vec<PlaceId>,
}

impl<'source, 'types> FunctionLowerer<'source, 'types> {
    fn new(
        id: FunctionId,
        source: &'source ExecutableFunction,
        signature: MirFunctionSignature,
        targets: CallTargets,
        types: &'types mut TypeTable,
    ) -> Self {
        let mut lowerer = Self {
            id,
            source,
            signature,
            targets,
            types,
            blocks: vec![BlockDraft::new()],
            current: BlockId::new(0),
            places: HashMap::new(),
            place_names: Vec::new(),
            next_value: 0,
            tasks: HashMap::new(),
            next_task: 0,
            loops: Vec::new(),
            resource_scopes: Vec::new(),
        };
        for parameter in &source.signature.params {
            lowerer.place(&parameter.name);
        }
        lowerer
    }

    fn lower(mut self) -> Result<LoweredFunction, MirLoweringError> {
        self.lower_statements(&self.source.body.statements)?;
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Return(None));
        }
        let blocks = self
            .blocks
            .into_iter()
            .enumerate()
            .map(|(index, block)| {
                BasicBlock::new(
                    BlockId::new(index as u32),
                    block.instructions,
                    block.terminator.unwrap_or(MirTerminator::Unreachable),
                )
            })
            .collect();
        Ok(LoweredFunction {
            function: MirFunction::new(
                self.id,
                self.signature,
                self.place_names.len() as u32,
                self.next_value,
                blocks,
            ),
            debug: MirFunctionDebug::new(self.source.name.clone(), self.place_names),
        })
    }

    fn lower_statements(&mut self, statements: &[ExecutableStmt]) -> Result<(), MirLoweringError> {
        for statement in statements {
            self.lower_statement(statement)?;
        }
        Ok(())
    }

    fn lower_statement(&mut self, statement: &ExecutableStmt) -> Result<(), MirLoweringError> {
        match statement {
            ExecutableStmt::Let {
                name,
                value,
                is_async,
            } => {
                if *is_async {
                    return self.lower_async_binding(name, value.as_ref());
                }
                let place = self.place(name);
                if let Some(value) = value {
                    let value = self.lower_expression(value)?;
                    self.emit(MirInstruction::WritePlace { place, value });
                }
            }
            ExecutableStmt::Return { value } => {
                let value = value
                    .as_ref()
                    .map(|value| self.lower_expression(value))
                    .transpose()?;
                self.emit_resource_cleanup_from(0);
                self.terminate(MirTerminator::Return(value));
                self.start_detached_block();
            }
            ExecutableStmt::If {
                condition,
                then_body,
                else_body,
            } => self.lower_if(condition, then_body, else_body.as_ref())?,
            ExecutableStmt::Loop { condition, body } => {
                self.lower_loop(condition.as_ref(), body)?
            }
            ExecutableStmt::Assign { target, value } => {
                let ExecutableExpr::Ident { name, .. } = target else {
                    return self.unsupported("non-local assignment");
                };
                let place = self.lookup_place(name)?;
                let value = self.lower_expression(value)?;
                self.emit(MirInstruction::WritePlace { place, value });
            }
            ExecutableStmt::Break => {
                let Some(targets) = self.loops.last() else {
                    return self.unsupported("break outside loop");
                };
                let (cleanup_depth, target) = (targets.cleanup_depth, targets.break_target);
                self.emit_resource_cleanup_from(cleanup_depth);
                self.terminate(MirTerminator::Jump(target));
                self.start_detached_block();
            }
            ExecutableStmt::Continue => {
                let Some(targets) = self.loops.last() else {
                    return self.unsupported("continue outside loop");
                };
                let (cleanup_depth, target) = (targets.cleanup_depth, targets.continue_target);
                self.emit_resource_cleanup_from(cleanup_depth);
                self.terminate(MirTerminator::Jump(target));
                self.start_detached_block();
            }
            ExecutableStmt::Expr(expression) => {
                let value = self.lower_expression(expression)?;
                self.emit(MirInstruction::Discard { value });
            }
            ExecutableStmt::With {
                resource,
                binding,
                body,
            } => self.lower_with(resource, binding, body)?,
            ExecutableStmt::For { .. } => return self.unsupported("for loop"),
            ExecutableStmt::Match { .. } => return self.unsupported("match"),
            ExecutableStmt::Select { .. } => return self.unsupported("select"),
            ExecutableStmt::Unknown => return self.unsupported("unknown statement"),
        }
        Ok(())
    }

    fn lower_if(
        &mut self,
        condition: &ExecutableExpr,
        then_body: &rsscript_exec_ir::ExecutableBlock,
        else_body: Option<&rsscript_exec_ir::ExecutableBlock>,
    ) -> Result<(), MirLoweringError> {
        let condition = self.lower_expression(condition)?;
        let then_block = self.new_block();
        let else_block = self.new_block();
        let join_block = self.new_block();
        self.terminate(MirTerminator::Branch {
            condition,
            then_target: then_block,
            else_target: else_block,
        });

        self.current = then_block;
        self.lower_statements(&then_body.statements)?;
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(join_block));
        }

        self.current = else_block;
        if let Some(else_body) = else_body {
            self.lower_statements(&else_body.statements)?;
        }
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(join_block));
        }

        self.current = join_block;
        Ok(())
    }

    fn lower_with(
        &mut self,
        resource: &ExecutableExpr,
        binding: &str,
        body: &rsscript_exec_ir::ExecutableBlock,
    ) -> Result<(), MirLoweringError> {
        let ExecutableExpr::Manage {
            value, type_name, ..
        } = resource
        else {
            return self.unsupported("unmanaged resource scope");
        };
        let Some(type_name) = type_name.as_deref() else {
            return self.unsupported("resource scope without canonical type");
        };
        // Evaluate the managed source before entering the lifetime. The current
        // MIR acquire primitive represents ownership of the resulting host
        // resource; bytecode execution remains intentionally unsupported.
        let source = self.lower_expression(value)?;
        let place = self.place(binding);
        let resource_type = self.intern_resource_type(type_name);
        self.emit(MirInstruction::AcquireResource {
            place,
            resource_type,
            source,
        });
        self.resource_scopes.push(place);
        self.lower_statements(&body.statements)?;
        if self.current_block().terminator.is_none() {
            self.emit(MirInstruction::ReleaseResource { place });
        }
        let released = self.resource_scopes.pop();
        debug_assert_eq!(released, Some(place));
        Ok(())
    }

    fn intern_resource_type(&mut self, type_name: &str) -> ResourceTypeId {
        let wire = match WireType::parse(type_name) {
            WireType::Resource { name } => WireType::Resource { name },
            WireType::Qualified { value, .. }
                if matches!(value.as_ref(), WireType::Resource { .. }) =>
            {
                *value
            }
            _ => WireType::Resource {
                name: type_name.to_owned(),
            },
        };
        ResourceTypeId::new(self.types.intern(wire).index() as u32)
    }

    fn lower_loop(
        &mut self,
        condition: Option<&ExecutableExpr>,
        body: &rsscript_exec_ir::ExecutableBlock,
    ) -> Result<(), MirLoweringError> {
        let header = self.new_block();
        let body_block = self.new_block();
        let exit = self.new_block();
        self.terminate(MirTerminator::Jump(header));

        self.current = header;
        if let Some(condition) = condition {
            let condition = self.lower_expression(condition)?;
            self.terminate(MirTerminator::Branch {
                condition,
                then_target: body_block,
                else_target: exit,
            });
        } else {
            self.terminate(MirTerminator::Jump(body_block));
        }

        self.current = body_block;
        self.loops.push(LoopTargets {
            continue_target: header,
            break_target: exit,
            cleanup_depth: self.resource_scopes.len(),
        });
        self.lower_statements(&body.statements)?;
        self.loops.pop();
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(header));
        }
        self.current = exit;
        Ok(())
    }

    fn lower_expression(
        &mut self,
        expression: &ExecutableExpr,
    ) -> Result<ValueId, MirLoweringError> {
        match expression {
            ExecutableExpr::Ident { name, .. } => {
                let destination = self.value();
                let place = self.lookup_place(name)?;
                self.emit(MirInstruction::ReadPlace { destination, place });
                Ok(destination)
            }
            ExecutableExpr::Number { value } => {
                let literal = value
                    .parse::<i64>()
                    .map(MirLiteral::Int)
                    .or_else(|_| value.parse::<f64>().map(MirLiteral::Float))
                    .map_err(|_| MirLoweringError::Unsupported {
                        function: self.source.name.clone(),
                        construct: "non-numeric literal",
                    })?;
                let destination = self.value();
                self.emit(MirInstruction::LoadLiteral {
                    destination,
                    value: literal,
                });
                Ok(destination)
            }
            ExecutableExpr::String { value } => self.literal(MirLiteral::String(value.clone())),
            ExecutableExpr::Char { value } => {
                let mut chars = value.chars();
                let Some(character) = chars.next() else {
                    return self.unsupported("empty char literal");
                };
                if chars.next().is_some() {
                    return self.unsupported("multi-character char literal");
                }
                self.literal(MirLiteral::Char(character))
            }
            ExecutableExpr::Binary { op, left, right } => {
                let left = self.lower_expression(left)?;
                let right = self.lower_expression(right)?;
                let destination = self.value();
                self.emit(MirInstruction::Binary {
                    destination,
                    op: binary_op(*op),
                    left,
                    right,
                });
                Ok(destination)
            }
            ExecutableExpr::ObjectLiteral { .. } => self.unsupported("object literal"),
            ExecutableExpr::MapLiteral { .. } => self.unsupported("map literal"),
            ExecutableExpr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.lower_expression(item))
                    .collect::<Result<Vec<_>, _>>()?;
                let destination = self.value();
                self.emit(MirInstruction::MakeList { destination, items });
                Ok(destination)
            }
            ExecutableExpr::Field { .. } => self.unsupported("field access"),
            ExecutableExpr::Index { .. } => self.unsupported("index access"),
            ExecutableExpr::Call {
                callee,
                receiver,
                args,
                ..
            } => self.lower_call(callee, receiver.is_some(), args),
            ExecutableExpr::Effect { effect, value, .. } => match effect {
                ParamEffect::Read => self.lower_read_borrow(value),
                ParamEffect::Mut => self.unsupported("mutable borrow"),
                ParamEffect::Take => self.lower_take(value),
            },
            ExecutableExpr::Manage { .. } => self.unsupported("managed resource"),
            ExecutableExpr::Spawn { .. } => self.unsupported("standalone spawn"),
            ExecutableExpr::Await { value, .. } => self.lower_await(value),
            ExecutableExpr::Try { .. } => self.unsupported("try"),
            ExecutableExpr::Closure { .. } => self.unsupported("closure"),
            ExecutableExpr::Match { .. } => self.unsupported("match expression"),
            ExecutableExpr::Unknown => self.unsupported("unknown expression"),
        }
    }

    fn literal(&mut self, value: MirLiteral) -> Result<ValueId, MirLoweringError> {
        let destination = self.value();
        self.emit(MirInstruction::LoadLiteral { destination, value });
        Ok(destination)
    }

    fn lower_call(
        &mut self,
        callee: &Callee,
        has_receiver: bool,
        args: &[rsscript_exec_ir::ExecutableCallArg],
    ) -> Result<ValueId, MirLoweringError> {
        if has_receiver {
            return self.unsupported("receiver call");
        }
        let name = match callee {
            Callee::Name(name) => name.clone(),
            Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
            Callee::ReceiverCall { .. } => return self.unsupported("receiver call"),
        };
        let target = self
            .targets
            .functions
            .get(&name)
            .copied()
            .map(MirCallTarget::Function)
            .or_else(|| {
                self.targets
                    .external_imports
                    .get(&name)
                    .copied()
                    .map(MirCallTarget::External)
            })
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.source.name.clone(),
                construct: "unresolved direct call",
            })?;
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let arguments = ordered
            .into_iter()
            .map(|argument| self.lower_call_argument(&argument.value))
            .collect::<Result<Vec<_>, _>>()?;
        let destination = self.value();
        self.emit(MirInstruction::Call {
            destination,
            target,
            arguments,
        });
        Ok(destination)
    }

    fn lower_async_binding(
        &mut self,
        name: &str,
        value: Option<&ExecutableExpr>,
    ) -> Result<(), MirLoweringError> {
        let Some(ExecutableExpr::Call {
            callee,
            receiver,
            args,
            ..
        }) = value
        else {
            return self.unsupported("async binding without direct call");
        };
        if receiver.is_some() {
            return self.unsupported("async receiver call");
        }
        let name_key = match callee {
            Callee::Name(name) => name.clone(),
            Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
            Callee::ReceiverCall { .. } => return self.unsupported("async receiver call"),
        };
        let Some(target) = self.targets.functions.get(&name_key).copied() else {
            return self.unsupported("async external or unresolved call");
        };
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let arguments = ordered
            .into_iter()
            .map(|argument| self.lower_call_argument(&argument.value))
            .collect::<Result<Vec<_>, _>>()?;
        let task = self.task();
        if self.tasks.insert(name.to_owned(), task).is_some() {
            return self.unsupported("duplicate async binding");
        }
        self.emit(MirInstruction::Spawn {
            task,
            group: TaskGroupId::new(0),
            target,
            arguments,
        });
        Ok(())
    }

    fn lower_await(&mut self, value: &ExecutableExpr) -> Result<ValueId, MirLoweringError> {
        let ExecutableExpr::Ident { name, .. } = value else {
            return self.unsupported("await of non-task local");
        };
        let Some(task) = self.tasks.get(name).copied() else {
            return self.unsupported("await of unknown task");
        };
        let destination = self.value();
        self.emit(MirInstruction::Await { destination, task });
        Ok(destination)
    }

    fn lower_call_argument(
        &mut self,
        argument: &ExecutableExpr,
    ) -> Result<MirCallArgument, MirLoweringError> {
        let ExecutableExpr::Effect { effect, value, .. } = argument else {
            return self.lower_expression(argument).map(MirCallArgument::Value);
        };
        let ExecutableExpr::Ident { name, .. } = value.as_ref() else {
            return self.unsupported("data effect on non-local value");
        };
        let place = self.lookup_place(name)?;
        Ok(match effect {
            ParamEffect::Read => MirCallArgument::BorrowRead(place),
            ParamEffect::Mut => MirCallArgument::BorrowMut(place),
            ParamEffect::Take => MirCallArgument::Take(place),
        })
    }

    fn lower_read_borrow(&mut self, value: &ExecutableExpr) -> Result<ValueId, MirLoweringError> {
        let ExecutableExpr::Ident { name, .. } = value else {
            return self.unsupported("read borrow of non-local value");
        };
        let destination = self.value();
        let place = self.lookup_place(name)?;
        self.emit(MirInstruction::BorrowRead { destination, place });
        Ok(destination)
    }

    fn lower_take(&mut self, value: &ExecutableExpr) -> Result<ValueId, MirLoweringError> {
        let ExecutableExpr::Ident { name, .. } = value else {
            return self.unsupported("move of non-local value");
        };
        let destination = self.value();
        let place = self.lookup_place(name)?;
        self.emit(MirInstruction::TakePlace { destination, place });
        Ok(destination)
    }

    fn place(&mut self, name: &str) -> PlaceId {
        if let Some(place) = self.places.get(name) {
            return *place;
        }
        let place = PlaceId::new(self.place_names.len() as u32);
        self.places.insert(name.to_owned(), place);
        self.place_names.push(name.to_owned());
        place
    }

    fn lookup_place(&self, name: &str) -> Result<PlaceId, MirLoweringError> {
        self.places
            .get(name)
            .copied()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.source.name.clone(),
                construct: "unresolved local",
            })
    }

    fn value(&mut self) -> ValueId {
        let value = ValueId::new(self.next_value);
        self.next_value += 1;
        value
    }

    fn task(&mut self) -> TaskId {
        let task = TaskId::new(self.next_task);
        self.next_task += 1;
        task
    }

    fn emit_resource_cleanup_from(&mut self, depth: usize) {
        let resources = self.resource_scopes[depth..]
            .iter()
            .copied()
            .rev()
            .collect::<Vec<_>>();
        for place in resources {
            self.emit(MirInstruction::ReleaseResource { place });
        }
    }

    fn emit(&mut self, instruction: MirInstruction) {
        self.current_block_mut().instructions.push(instruction);
    }

    fn terminate(&mut self, terminator: MirTerminator) {
        debug_assert!(self.current_block().terminator.is_none());
        self.current_block_mut().terminator = Some(terminator);
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId::new(self.blocks.len() as u32);
        self.blocks.push(BlockDraft::new());
        id
    }

    fn start_detached_block(&mut self) {
        self.current = self.new_block();
    }

    fn current_block(&self) -> &BlockDraft {
        &self.blocks[self.current.index()]
    }

    fn current_block_mut(&mut self) -> &mut BlockDraft {
        &mut self.blocks[self.current.index()]
    }

    fn unsupported<T>(&self, construct: &'static str) -> Result<T, MirLoweringError> {
        Err(MirLoweringError::Unsupported {
            function: self.source.name.clone(),
            construct,
        })
    }
}

fn binary_op(op: BinaryOp) -> MirBinaryOp {
    match op {
        BinaryOp::Add => MirBinaryOp::Add,
        BinaryOp::Subtract => MirBinaryOp::Subtract,
        BinaryOp::Multiply => MirBinaryOp::Multiply,
        BinaryOp::Divide => MirBinaryOp::Divide,
        BinaryOp::Modulo => MirBinaryOp::Modulo,
        BinaryOp::BitAnd => MirBinaryOp::BitAnd,
        BinaryOp::BitOr => MirBinaryOp::BitOr,
        BinaryOp::BitXor => MirBinaryOp::BitXor,
        BinaryOp::ShiftLeft => MirBinaryOp::ShiftLeft,
        BinaryOp::ShiftRight => MirBinaryOp::ShiftRight,
        BinaryOp::Equal => MirBinaryOp::Equal,
        BinaryOp::NotEqual => MirBinaryOp::NotEqual,
        BinaryOp::Less => MirBinaryOp::Less,
        BinaryOp::LessEqual => MirBinaryOp::LessEqual,
        BinaryOp::Greater => MirBinaryOp::Greater,
        BinaryOp::GreaterEqual => MirBinaryOp::GreaterEqual,
        BinaryOp::LogicalAnd => MirBinaryOp::LogicalAnd,
        BinaryOp::LogicalOr => MirBinaryOp::LogicalOr,
    }
}

#[cfg(test)]
mod tests {
    use rsscript_exec_ir::{
        ExecutableBlock, ExecutableFunction, ExecutableProgram, ExecutableSignature,
    };

    use super::*;

    fn module(function: ExecutableFunction) -> ExecutableIr {
        module_with_functions(vec![function])
    }

    fn module_with_functions(functions: Vec<ExecutableFunction>) -> ExecutableIr {
        let mut program = ExecutableProgram::default();
        for function in functions {
            program.insert_function(function.name.clone(), function);
        }
        ExecutableIr::new(program, Box::new([]))
    }

    fn signature() -> ExecutableSignature {
        ExecutableSignature {
            namespace: None,
            name: "main".into(),
            is_async: false,
            params: Vec::new(),
            return_type: Some("Int".into()),
            is_external: false,
        }
    }

    #[test]
    fn lowers_scalar_branching_to_verified_cfg() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![
                    ExecutableStmt::Let {
                        name: "value".into(),
                        value: Some(ExecutableExpr::Number { value: "1".into() }),
                        is_async: false,
                    },
                    ExecutableStmt::If {
                        condition: ExecutableExpr::Binary {
                            op: BinaryOp::Less,
                            left: Box::new(ExecutableExpr::Ident {
                                name: "value".into(),
                                type_name: Some("Int".into()),
                            }),
                            right: Box::new(ExecutableExpr::Number { value: "2".into() }),
                        },
                        then_body: ExecutableBlock {
                            statements: vec![ExecutableStmt::Return {
                                value: Some(ExecutableExpr::Ident {
                                    name: "value".into(),
                                    type_name: Some("Int".into()),
                                }),
                            }],
                        },
                        else_body: Some(ExecutableBlock {
                            statements: vec![ExecutableStmt::Return {
                                value: Some(ExecutableExpr::Number { value: "0".into() }),
                            }],
                        }),
                    },
                ],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).unwrap();
        assert_eq!(mir.functions().len(), 1);
        assert_eq!(
            mir.types(),
            &[WireType::Int {
                bits: 64,
                signed: true
            }]
        );
        assert_eq!(mir.functions()[0].signature().result(), TypeId::new(0));
        assert!(mir.functions()[0].blocks().len() >= 4);
        mir.verify().unwrap();
    }

    #[test]
    fn lowers_standalone_take_of_a_local_to_explicit_mir_move() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![
                    ExecutableStmt::Let {
                        name: "value".into(),
                        value: Some(ExecutableExpr::Number { value: "1".into() }),
                        is_async: false,
                    },
                    ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Effect {
                            effect: ParamEffect::Take,
                            value: Box::new(ExecutableExpr::Ident {
                                name: "value".into(),
                                type_name: Some("Int".into()),
                            }),
                            type_name: Some("Int".into()),
                        }),
                    },
                ],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).expect("lower standalone take");
        assert!(matches!(
            mir.functions()[0].blocks()[0].instructions(),
            [
                MirInstruction::LoadLiteral { .. },
                MirInstruction::WritePlace { .. },
                MirInstruction::TakePlace { .. }
            ]
        ));
        mir.verify().expect("verify explicit move");
    }

    #[test]
    fn lowers_array_literals_to_owned_mir_list_construction() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::Return {
                    value: Some(ExecutableExpr::ArrayLiteral {
                        items: vec![
                            ExecutableExpr::Number { value: "1".into() },
                            ExecutableExpr::Number { value: "2".into() },
                        ],
                        type_name: Some("List<Int>".into()),
                    }),
                }],
            },
        });

        let mir = lower_executable_ir_to_mir(&executable).expect("lower array literal");
        assert!(matches!(
            mir.functions()[0].blocks()[0].instructions(),
            [
                MirInstruction::LoadLiteral { .. },
                MirInstruction::LoadLiteral { .. },
                MirInstruction::MakeList { items, .. },
            ] if items.len() == 2
        ));
        mir.verify().expect("verify list construction");
    }

    #[test]
    fn rejects_unmanaged_resource_and_async_nodes_until_their_mir_ops_exist() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::With {
                    resource: ExecutableExpr::Number { value: "1".into() },
                    binding: "resource".into(),
                    body: ExecutableBlock { statements: vec![] },
                }],
            },
        });

        assert!(matches!(
            lower_executable_ir_to_mir(&executable),
            Err(MirLoweringError::Unsupported {
                construct: "unmanaged resource scope",
                ..
            })
        ));
    }

    #[test]
    fn lowers_managed_resource_scope_cleanup_before_return() {
        let executable = module(ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::With {
                    resource: ExecutableExpr::Manage {
                        value: Box::new(ExecutableExpr::Call {
                            callee: Callee::Name("open".into()),
                            receiver: None,
                            args: Vec::new(),
                            type_name: Some("File".into()),
                        }),
                        type_name: Some("host.fs.File".into()),
                    },
                    binding: "file".into(),
                    body: ExecutableBlock {
                        statements: vec![ExecutableStmt::Return {
                            value: Some(ExecutableExpr::Number { value: "9".into() }),
                        }],
                    },
                }],
            },
        });
        let mut program = ExecutableProgram::default();
        program.insert_function(
            "open".into(),
            ExecutableFunction {
                name: "open".into(),
                is_async: false,
                signature: ExecutableSignature {
                    namespace: None,
                    name: "open".into(),
                    is_async: false,
                    params: Vec::new(),
                    return_type: Some("host.fs.File".into()),
                    is_external: false,
                },
                body: ExecutableBlock { statements: vec![] },
            },
        );
        program.insert_function(
            "main".into(),
            executable.functions().next().unwrap().clone(),
        );
        let mir = lower_executable_ir_to_mir(&ExecutableIr::new(program, Box::new([])))
            .expect("lower managed resource scope");
        let main = mir
            .functions()
            .iter()
            .find(|function| {
                mir.function_debug(function.id())
                    .is_some_and(|debug| debug.name() == "main")
            })
            .expect("main function");
        assert!(matches!(
            main.blocks()[0].instructions(),
            [
                MirInstruction::Call { .. },
                MirInstruction::AcquireResource { .. },
                MirInstruction::LoadLiteral { .. },
                MirInstruction::ReleaseResource { .. }
            ]
        ));
        mir.verify().expect("verify resource lifetime");
    }

    #[test]
    fn lowers_managed_resource_scope_cleanup_before_loop_break() {
        let main = ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![
                    ExecutableStmt::Loop {
                        condition: None,
                        body: ExecutableBlock {
                            statements: vec![ExecutableStmt::With {
                                resource: ExecutableExpr::Manage {
                                    value: Box::new(ExecutableExpr::Call {
                                        callee: Callee::Name("open".into()),
                                        receiver: None,
                                        args: Vec::new(),
                                        type_name: Some("host.fs.File".into()),
                                    }),
                                    type_name: Some("host.fs.File".into()),
                                },
                                binding: "file".into(),
                                body: ExecutableBlock {
                                    statements: vec![ExecutableStmt::Break],
                                },
                            }],
                        },
                    },
                    ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Number { value: "1".into() }),
                    },
                ],
            },
        };
        let open = ExecutableFunction {
            name: "open".into(),
            is_async: false,
            signature: ExecutableSignature {
                namespace: None,
                name: "open".into(),
                is_async: false,
                params: Vec::new(),
                return_type: Some("host.fs.File".into()),
                is_external: false,
            },
            body: ExecutableBlock { statements: vec![] },
        };
        let mir = lower_executable_ir_to_mir(&module_with_functions(vec![main, open]))
            .expect("lower resource scope in loop");
        mir.verify()
            .expect("break path releases the scoped resource before the loop exit");
    }

    #[test]
    fn lowers_async_binding_and_await_to_structured_task_ops() {
        let worker = ExecutableFunction {
            name: "worker".into(),
            is_async: true,
            signature: ExecutableSignature {
                namespace: None,
                name: "worker".into(),
                is_async: true,
                params: Vec::new(),
                return_type: Some("Int".into()),
                is_external: false,
            },
            body: ExecutableBlock {
                statements: vec![ExecutableStmt::Return {
                    value: Some(ExecutableExpr::Number { value: "7".into() }),
                }],
            },
        };
        let main = ExecutableFunction {
            name: "main".into(),
            is_async: false,
            signature: signature(),
            body: ExecutableBlock {
                statements: vec![
                    ExecutableStmt::Let {
                        name: "job".into(),
                        value: Some(ExecutableExpr::Call {
                            callee: Callee::Name("worker".into()),
                            receiver: None,
                            args: Vec::new(),
                            type_name: Some("Int".into()),
                        }),
                        is_async: true,
                    },
                    ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Await {
                            value: Box::new(ExecutableExpr::Ident {
                                name: "job".into(),
                                type_name: Some("Task<Int>".into()),
                            }),
                            type_name: Some("Int".into()),
                        }),
                    },
                ],
            },
        };
        let mir = lower_executable_ir_to_mir(&module_with_functions(vec![main, worker]))
            .expect("lower async binding");
        let main = mir
            .functions()
            .iter()
            .find(|function| {
                mir.function_debug(function.id())
                    .is_some_and(|debug| debug.name() == "main")
            })
            .expect("main function");
        assert!(matches!(
            main.blocks()[0].instructions(),
            [MirInstruction::Spawn { .. }, MirInstruction::Await { .. }]
        ));
        mir.verify().expect("verify structured task lifetime");
    }

    #[test]
    fn lowers_direct_calls_to_function_ids() {
        let executable = module_with_functions(vec![
            ExecutableFunction {
                name: "main".into(),
                is_async: false,
                signature: signature(),
                body: ExecutableBlock {
                    statements: vec![ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Call {
                            callee: Callee::Name("helper".into()),
                            receiver: None,
                            args: Vec::new(),
                            type_name: Some("Int".into()),
                        }),
                    }],
                },
            },
            ExecutableFunction {
                name: "helper".into(),
                is_async: false,
                signature: signature(),
                body: ExecutableBlock {
                    statements: vec![ExecutableStmt::Return {
                        value: Some(ExecutableExpr::Number { value: "1".into() }),
                    }],
                },
            },
        ]);

        let mir = lower_executable_ir_to_mir(&executable).unwrap();
        let instructions = mir.functions()[1].blocks()[0].instructions();
        assert!(matches!(
            instructions,
            [MirInstruction::Call {
                target: MirCallTarget::Function(target),
                ..
            }] if *target == FunctionId::new(0)
        ));
    }
}
