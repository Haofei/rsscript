//! `CheckedHirLowerer` lowering methods (part 1 of 2), split out of `mir.rs`
//! for module-size partitioning. Continued in `lowerer_calls.rs`.

//! `CheckedHirLowerer` method implementation, split out of `mir.rs` for
//! module-size partitioning. The lowerer struct stays defined in `mir.rs`.

use super::*;

impl<'source, 'types, 'closures> CheckedHirLowerer<'source, 'types, 'closures> {
    pub(super) fn new(
        input: CheckedHirLowererInput<'source>,
        types: &'types mut TypeTable,
        closure_registry: &'closures mut ClosureRegistry,
    ) -> Self {
        let CheckedHirLowererInput {
            id,
            function_name,
            body,
            mir_signature,
            initial_places,
            captures,
            targets,
        } = input;
        let mut lowerer = Self {
            id,
            function_name,
            body,
            mir_signature,
            captures,
            targets,
            types,
            closure_registry,
            blocks: vec![BlockDraft::new()],
            current: BlockId::new(0),
            places: HashMap::new(),
            place_types: HashMap::new(),
            closure_abis: HashMap::new(),
            place_names: Vec::new(),
            instruction_sources: Vec::new(),
            next_value: 0,
            tasks: HashMap::new(),
            next_task: 0,
            loops: Vec::new(),
            resource_scopes: Vec::new(),
        };
        for (name, ty) in initial_places {
            lowerer.place_with_type(&name, ty);
        }
        lowerer
    }

    pub(super) fn lower(mut self) -> Result<LoweredFunction, MirLoweringError> {
        for statement in &self.body.statements {
            if self.current_block().terminator.is_some() {
                return self.unsupported("statement after return");
            }
            self.lower_statement(statement)?;
        }
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
            function: MirFunction::with_captures(
                self.id,
                self.mir_signature,
                self.captures,
                self.place_names.len() as u32,
                self.next_value,
                blocks,
            ),
            debug: MirFunctionDebug::with_source(
                self.function_name.to_owned(),
                self.place_names,
                MirSourceLocation::new(
                    self.body.span.file.clone(),
                    self.body.span.line,
                    self.body.span.column,
                    self.body.span.length,
                ),
            )
            .with_instruction_sources(self.instruction_sources),
        })
    }

    pub(super) fn lower_statement(
        &mut self,
        statement: &checked::HirStmt,
    ) -> Result<(), MirLoweringError> {
        match statement {
            checked::HirStmt::Let {
                name,
                value,
                ty,
                is_async: false,
                ..
            } => {
                let place = self.place(name);
                if let Some(checked::HirExpr::Closure { ty: Some(ty), .. }) = value {
                    let signature = checked_closure_signature(self.types, ty, &self.function_name)?;
                    self.closure_abis
                        .insert(name.clone(), ClosureAbi::from(&signature));
                }
                if let Some(ty) = ty
                    && let Ok(wire) = checked_type_to_wire(ty, &self.function_name)
                {
                    let ty = self.types.intern(wire);
                    self.place_types.insert(name.clone(), ty);
                }
                if let Some(value) = value {
                    let value = self.lower_expression(value)?;
                    self.emit(MirInstruction::WritePlace { place, value });
                }
                Ok(())
            }
            checked::HirStmt::Return { value, .. } => {
                let value = value
                    .as_ref()
                    .map(|value| self.lower_expression(value))
                    .transpose()?;
                self.emit_resource_cleanup_from(0);
                self.terminate(MirTerminator::Return(value));
                Ok(())
            }
            checked::HirStmt::Assign { target, value, .. } => {
                let value = self.lower_expression(value)?;
                self.lower_assignment_target(target, value)
            }
            checked::HirStmt::Expr(expression) => {
                let value = self.lower_expression(expression)?;
                self.emit(MirInstruction::Discard { value });
                Ok(())
            }
            checked::HirStmt::Let {
                name,
                value,
                is_async: true,
                ..
            } => self.lower_async_binding(name, value.as_ref()),
            checked::HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => self.lower_if(condition, then_body, else_body.as_ref()),
            checked::HirStmt::Loop {
                condition, body, ..
            } => self.lower_loop(condition.as_ref(), body),
            checked::HirStmt::With {
                resource,
                resource_type,
                binding,
                body,
                ..
            } => self.lower_with(resource, resource_type.as_ref(), binding, body),
            checked::HirStmt::For {
                binding,
                iterable,
                iterable_type,
                is_async,
                body,
                ..
            } => self.lower_for(binding, iterable, iterable_type.as_ref(), *is_async, body),
            checked::HirStmt::Match { value, arms, .. } => self.lower_match(value, arms),
            checked::HirStmt::Select { arms, .. } => self.lower_select(arms),
            checked::HirStmt::Break(_) => {
                let Some(targets) = self.loops.last() else {
                    return self.unsupported("checked HIR break outside loop");
                };
                let (cleanup_depth, target) = (targets.cleanup_depth, targets.break_target);
                self.emit_resource_cleanup_from(cleanup_depth);
                self.terminate(MirTerminator::Jump(target));
                self.start_detached_block();
                Ok(())
            }
            checked::HirStmt::Continue(_) => {
                let Some(targets) = self.loops.last() else {
                    return self.unsupported("checked HIR continue outside loop");
                };
                let (cleanup_depth, target) = (targets.cleanup_depth, targets.continue_target);
                self.emit_resource_cleanup_from(cleanup_depth);
                self.terminate(MirTerminator::Jump(target));
                self.start_detached_block();
                Ok(())
            }
            checked::HirStmt::Unknown(_) => self.unsupported("unknown checked HIR statement"),
        }
    }

    /// Lower assignment as an explicit rebuild chain. A field assignment first
    /// produces the updated aggregate and then assigns that value to its base;
    /// this preserves value semantics for nested paths without asking a
    /// backend to inspect source-shaped assignment syntax.
    pub(super) fn lower_assignment_target(
        &mut self,
        target: &checked::HirExpr,
        value: ValueId,
    ) -> Result<(), MirLoweringError> {
        match target {
            checked::HirExpr::Ident { name, .. } => {
                let place = self.lookup_place(name)?;
                self.emit(MirInstruction::WritePlace { place, value });
                Ok(())
            }
            checked::HirExpr::Field { base, name, .. } => {
                let base_value = self.lower_expression(base)?;
                self.emit(MirInstruction::SetField {
                    base: base_value,
                    field: name.clone(),
                    value,
                });
                self.lower_assignment_target(base, base_value)
            }
            _ => self.unsupported("non-place checked HIR assignment"),
        }
    }

    pub(super) fn lower_expression(
        &mut self,
        expression: &checked::HirExpr,
    ) -> Result<ValueId, MirLoweringError> {
        match expression {
            checked::HirExpr::Ident { name, span, .. } if name == "Unit" => self
                .literal_with_source(
                    MirLiteral::Unit,
                    MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
                ),
            checked::HirExpr::Ident { name, span, .. } if name == "true" => self
                .literal_with_source(
                    MirLiteral::Bool(true),
                    MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
                ),
            checked::HirExpr::Ident { name, span, .. } if name == "false" => self
                .literal_with_source(
                    MirLiteral::Bool(false),
                    MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
                ),
            checked::HirExpr::Ident { name, .. } => {
                let destination = self.value();
                let place = self.lookup_place(name)?;
                self.emit(MirInstruction::ReadPlace { destination, place });
                Ok(destination)
            }
            checked::HirExpr::Number { value, span } => {
                let value = value
                    .parse::<i64>()
                    .map(MirLiteral::Int)
                    .or_else(|_| value.parse::<f64>().map(MirLiteral::Float))
                    .map_err(|_| MirLoweringError::Unsupported {
                        function: self.function_name.to_owned(),
                        construct: "non-numeric checked HIR literal",
                    })?;
                self.literal_with_source(
                    value,
                    MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
                )
            }
            checked::HirExpr::String { value, span } => self.literal_with_source(
                MirLiteral::String(decode_string_token(value)),
                MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
            ),
            checked::HirExpr::Char { value, span } => self.literal_with_source(
                MirLiteral::Char(decode_char_token(value)),
                MirSourceLocation::new(span.file.clone(), span.line, span.column, span.length),
            ),
            checked::HirExpr::Binary {
                op:
                    op @ (rsscript_syntax::ast::BinaryOp::LogicalAnd
                    | rsscript_syntax::ast::BinaryOp::LogicalOr),
                left,
                right,
                ..
            } => self.lower_logical_binary(*op, left, right),
            checked::HirExpr::Binary {
                op, left, right, ..
            } => {
                let left = self.lower_expression(left)?;
                let right = self.lower_expression(right)?;
                let destination = self.value();
                self.emit(MirInstruction::Binary {
                    destination,
                    op: checked_binary_op(*op),
                    left,
                    right,
                });
                Ok(destination)
            }
            checked::HirExpr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.lower_expression(item))
                    .collect::<Result<Vec<_>, _>>()?;
                let destination = self.value();
                self.emit(MirInstruction::MakeList { destination, items });
                Ok(destination)
            }
            checked::HirExpr::MapLiteral { entries, .. } => {
                let entries = entries
                    .iter()
                    .map(|entry| -> Result<(ValueId, ValueId), MirLoweringError> {
                        Ok((
                            self.lower_expression(&entry.key)?,
                            self.lower_expression(&entry.value)?,
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let destination = self.value();
                self.emit(MirInstruction::MakeMap {
                    destination,
                    entries,
                });
                Ok(destination)
            }
            checked::HirExpr::ObjectLiteral { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|field| -> Result<(String, ValueId), MirLoweringError> {
                        Ok((field.name.clone(), self.lower_expression(&field.value)?))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let destination = self.value();
                self.emit(MirInstruction::MakeObject {
                    destination,
                    fields,
                });
                Ok(destination)
            }
            checked::HirExpr::Index {
                base,
                index,
                base_type,
                ..
            } if base_type
                .as_ref()
                .is_some_and(|ty| ty.root_name() == Some("List")) =>
            {
                let list = self.lower_expression(base)?;
                let index = self.lower_expression(index)?;
                let destination = self.value();
                self.emit(MirInstruction::ListGet {
                    destination,
                    list,
                    index,
                });
                Ok(destination)
            }
            checked::HirExpr::Index {
                base,
                index,
                base_type,
                ..
            } if base_type
                .as_ref()
                .is_some_and(|ty| ty.root_name() == Some("Map")) =>
            {
                let map = self.lower_expression(base)?;
                let key = self.lower_expression(index)?;
                let destination = self.value();
                self.emit(MirInstruction::MapGet {
                    destination,
                    map,
                    key,
                });
                Ok(destination)
            }
            checked::HirExpr::Index { .. } => self.unsupported("non-list checked HIR index"),
            checked::HirExpr::Call {
                callee,
                receiver,
                args,
                type_arguments,
                resolution,
                ..
            } => {
                self.lower_direct_call(callee, receiver.as_ref(), args, type_arguments, resolution)
            }
            checked::HirExpr::Effect {
                effect: checked::ParamEffect::Read,
                value,
                ..
            } => self.lower_expression(value),
            checked::HirExpr::Effect {
                effect: checked::ParamEffect::Take,
                value,
                ..
            } => self.lower_take(value),
            checked::HirExpr::Effect { .. } => self.unsupported("mutable checked HIR effect"),
            checked::HirExpr::Manage { value, .. } => self.lower_manage(value),
            checked::HirExpr::Spawn { .. } => self.unsupported("checked HIR spawn"),
            checked::HirExpr::Await { value, .. } => self.lower_await(value),
            checked::HirExpr::Try { value, .. } => {
                let source = self.lower_expression(value)?;
                let destination = self.value();
                self.emit(MirInstruction::TryResult {
                    destination,
                    source,
                    cleanup: self.resource_cleanup_places(),
                });
                Ok(destination)
            }
            checked::HirExpr::Closure {
                params,
                captures,
                ty,
                body,
                ..
            } => self.lower_closure(params, captures, ty.as_ref(), body),
            checked::HirExpr::Field { base, name, .. } => {
                let base = self.lower_expression(base)?;
                let destination = self.value();
                self.emit(MirInstruction::GetField {
                    destination,
                    base,
                    field: name.clone(),
                });
                Ok(destination)
            }
            checked::HirExpr::Match { value, arms, .. } => self.lower_match_expression(value, arms),
            checked::HirExpr::Unknown(_) => self.unsupported("unknown checked HIR expression"),
        }
    }

    /// Lower an owned checked closure into a synthetic MIR function plus an
    /// explicit verifier-visible environment. The source closure never leaks
    /// into a backend: its ABI is the structural HIR `Fn` contract and every
    /// captured local is represented by a typed ownership-mode argument.
    pub(super) fn lower_closure(
        &mut self,
        params: &[String],
        captures: &[checked::HirClosureCapture],
        ty: Option<&rsscript_semantics::ResolvedType>,
        body: &checked::HirBlock,
    ) -> Result<ValueId, MirLoweringError> {
        let ty = ty.ok_or_else(|| MirLoweringError::Unsupported {
            function: self.function_name.to_owned(),
            construct: "checked HIR closure without structural Fn contract",
        })?;
        let signature = checked_closure_signature(self.types, ty, &self.function_name)?;
        if signature.parameter_types().len() != params.len() {
            return self.unsupported("checked HIR closure parameter/contract arity mismatch");
        }

        let mut initial_places = Vec::with_capacity(captures.len() + params.len());
        let mut mir_captures = Vec::with_capacity(captures.len());
        let mut capture_arguments = Vec::with_capacity(captures.len());
        for capture in captures {
            let place = self.lookup_place(&capture.name)?;
            let ty = self.place_type(&capture.name)?;
            let mode = match capture.effect {
                checked::ParamEffect::Read => MirParameterMode::Read,
                checked::ParamEffect::Mut => MirParameterMode::Mut,
                checked::ParamEffect::Take => MirParameterMode::Take,
            };
            initial_places.push((capture.name.clone(), ty));
            mir_captures.push(rsscript_mir::MirClosureCapture::new(ty, mode));
            capture_arguments.push(match mode {
                MirParameterMode::Read => MirCallArgument::BorrowRead(place),
                MirParameterMode::Mut => MirCallArgument::BorrowMut(place),
                MirParameterMode::Take => MirCallArgument::Take(place),
            });
        }
        for (name, ty) in params
            .iter()
            .cloned()
            .zip(signature.parameter_types().iter().copied())
        {
            if initial_places.iter().any(|(existing, _)| existing == &name) {
                return self.unsupported("checked HIR closure capture shadows parameter");
            }
            initial_places.push((name, ty));
        }

        let id = self.closure_registry.allocate();
        let closure_name = format!("{}::<closure:{}>", self.function_name, id.index());
        let output = CheckedHirLowerer::new(
            CheckedHirLowererInput {
                id,
                function_name: closure_name,
                body,
                mir_signature: signature,
                initial_places,
                captures: mir_captures,
                targets: self.targets.clone(),
            },
            self.types,
            self.closure_registry,
        )
        .lower()?;
        self.closure_registry.push(output);

        let destination = self.value();
        self.emit(MirInstruction::MakeClosure {
            destination,
            function: id,
            captures: capture_arguments,
        });
        Ok(destination)
    }

    /// Lower boolean `&&`/`||` as explicit CFG rather than a binary opcode.
    ///
    /// Short-circuiting is an observable execution property: evaluating the
    /// right side before branching could invoke a Provider, allocate a
    /// resource, or fail even when the result is already known. Keeping both
    /// paths explicit also means bytecode codegen needs only ordinary branch,
    /// write, and read operations; it never has to recover source-level
    /// short-circuit behavior from a generic binary instruction.
    pub(super) fn lower_logical_binary(
        &mut self,
        op: rsscript_syntax::ast::BinaryOp,
        left: &checked::HirExpr,
        right: &checked::HirExpr,
    ) -> Result<ValueId, MirLoweringError> {
        let left = self.lower_expression(left)?;
        let right_block = self.new_block();
        let short_circuit_block = self.new_block();
        let join_block = self.new_block();
        let result_place = self.place(&format!("__rss_mir_logical_result_{}", self.next_value));
        let short_circuit_value = match op {
            rsscript_syntax::ast::BinaryOp::LogicalAnd => false,
            rsscript_syntax::ast::BinaryOp::LogicalOr => true,
            _ => return self.unsupported("non-logical checked HIR binary operation"),
        };
        let (then_target, else_target) = match op {
            rsscript_syntax::ast::BinaryOp::LogicalAnd => (right_block, short_circuit_block),
            rsscript_syntax::ast::BinaryOp::LogicalOr => (short_circuit_block, right_block),
            _ => return self.unsupported("non-logical checked HIR binary operation"),
        };
        self.terminate(MirTerminator::Branch {
            condition: left,
            then_target,
            else_target,
        });

        self.current = right_block;
        let right = self.lower_expression(right)?;
        self.emit(MirInstruction::WritePlace {
            place: result_place,
            value: right,
        });
        self.terminate(MirTerminator::Jump(join_block));

        self.current = short_circuit_block;
        let value = self.literal(MirLiteral::Bool(short_circuit_value))?;
        self.emit(MirInstruction::WritePlace {
            place: result_place,
            value,
        });
        self.terminate(MirTerminator::Jump(join_block));

        self.current = join_block;
        let destination = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination,
            place: result_place,
        });
        Ok(destination)
    }

    pub(super) fn literal(&mut self, value: MirLiteral) -> Result<ValueId, MirLoweringError> {
        let destination = self.value();
        self.emit(MirInstruction::LoadLiteral { destination, value });
        Ok(destination)
    }

    pub(super) fn literal_with_source(
        &mut self,
        value: MirLiteral,
        source: MirSourceLocation,
    ) -> Result<ValueId, MirLoweringError> {
        let destination = self.value();
        let block = self.current;
        let instruction_index = u32::try_from(self.current_block().instructions.len())
            .expect("RSScript MIR instruction count exceeds the u32 source-map address space");
        self.emit(MirInstruction::LoadLiteral { destination, value });
        self.instruction_sources
            .push(MirInstructionSource::new(block, instruction_index, source));
        Ok(destination)
    }

    pub(super) fn lower_direct_call(
        &mut self,
        callee: &rsscript_syntax::ast::Callee,
        receiver: Option<&checked::HirCallReceiver>,
        args: &[checked::HirCallArg],
        type_arguments: &[ResolvedType],
        resolution: &checked::CallResolution,
    ) -> Result<ValueId, MirLoweringError> {
        if matches!(resolution, checked::CallResolution::EnumVariant) {
            if receiver.is_some() {
                return self.unsupported("checked HIR receiver enum-variant call");
            }
            return self.lower_enum_variant_call(callee, args);
        }
        if matches!(resolution, checked::CallResolution::Unknown)
            && receiver.is_none()
            && let rsscript_syntax::ast::Callee::Name(name) = callee
            && let Some(abi) = self.closure_abis.get(name).cloned()
        {
            return self.lower_local_closure_call(name, args, abi);
        }
        let checked::CallResolution::Resolved { signature, kind } = resolution else {
            return self.unsupported("unresolved checked HIR call");
        };
        if matches!(
            kind,
            checked::ResolvedCalleeKind::Constructor {
                type_kind: checked::HirTypeKind::Struct | checked::HirTypeKind::Class,
            }
        ) {
            return self.lower_record_constructor(signature, args);
        }
        if matches!(kind, checked::ResolvedCalleeKind::Constructor { .. }) {
            return self.unsupported("non-record checked HIR constructor");
        }
        // Core interfaces mark their signatures as builtins directly. The
        // explicit async standard package exposes the same deterministic VM
        // operations as `.rssi` declarations, however, so its checked
        // signatures are interface-shaped. A catalog hit is the canonical
        // identity in both forms: route it through `BuiltinId` rather than
        // creating a fictitious Provider import for a VM-owned operation.
        if signature.is_builtin || is_catalog_builtin(signature) {
            return self.lower_builtin_call(callee, signature, receiver, args);
        }
        let target = if let Some(dispatch) = signature.namespace.as_deref().and_then(|namespace| {
            self.targets
                .dynamic_protocol_methods
                .get(&(namespace.to_owned(), signature.name.clone()))
        }) {
            MirCallTarget::Dynamic {
                dispatch: dispatch.clone(),
                parameter_modes: checked_parameter_modes(signature).into_boxed_slice(),
            }
        } else if signature.is_external {
            let symbol = checked_external_symbol(signature)?;
            self.targets
                .external_imports
                .get(symbol.as_str())
                .copied()
                .map(MirCallTarget::External)
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "direct checked HIR external call target",
                })?
        } else {
            let qualified = signature
                .namespace
                .as_ref()
                .map(|namespace| format!("{namespace}.{}", signature.name));
            let function = self
                .targets
                .functions
                .get(&signature.name)
                .or_else(|| {
                    qualified
                        .as_ref()
                        .and_then(|name| self.targets.functions.get(name))
                })
                .copied()
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "direct checked HIR call target",
                })?;
            if type_arguments.is_empty() {
                MirCallTarget::Function(function)
            } else {
                let concrete_arguments = type_arguments
                    .iter()
                    .map(|ty| checked_type_to_wire(ty, &self.function_name))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .map(|ty| self.types.intern(ty))
                    .collect::<Vec<_>>();
                let type_substitutions = signature
                    .type_params
                    .iter()
                    .zip(concrete_arguments)
                    .map(|(parameter, argument)| {
                        let parameter = self.types.intern(WireType::Named {
                            package: None,
                            name: parameter.clone(),
                            arguments: Vec::new(),
                        });
                        (parameter, argument)
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                MirCallTarget::FunctionInstance {
                    function,
                    type_substitutions,
                }
            }
        };
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let mut arguments = Vec::with_capacity(ordered.len() + usize::from(receiver.is_some()));
        let mut retained_places = Vec::new();
        if let Some(receiver) = receiver {
            let lowered = self.lower_direct_receiver_argument(receiver)?;
            if signature
                .params
                .first()
                .is_some_and(|parameter| signature.retained_params.contains(&parameter.name))
                && let MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) =
                    lowered
            {
                retained_places.push(place);
            }
            arguments.push(lowered);
        }
        for argument in ordered {
            let lowered = self.lower_direct_call_argument(&argument.value)?;
            if argument
                .parameter_index
                .and_then(|index| signature.params.get(index))
                .is_some_and(|parameter| signature.retained_params.contains(&parameter.name))
                && let MirCallArgument::BorrowRead(place) | MirCallArgument::BorrowMut(place) =
                    lowered
            {
                retained_places.push(place);
            }
            arguments.push(lowered);
        }
        let destination = self.value();
        self.emit(MirInstruction::Call {
            destination,
            target,
            arguments,
        });
        for place in retained_places {
            self.emit(MirInstruction::Retain { place });
        }
        Ok(destination)
    }

    /// Lower an invocation through a local first-class closure value. The
    /// closure's concrete synthetic function remains opaque here; its typed
    /// parameter contract was recorded when the binding was constructed.
    pub(super) fn lower_local_closure_call(
        &mut self,
        name: &str,
        args: &[checked::HirCallArg],
        abi: ClosureAbi,
    ) -> Result<ValueId, MirLoweringError> {
        let place = self.lookup_place(name)?;
        let closure = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination: closure,
            place,
        });
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let arguments = ordered
            .into_iter()
            .map(|argument| self.lower_direct_call_argument(&argument.value))
            .collect::<Result<Vec<_>, _>>()?;
        let destination = self.value();
        self.emit(MirInstruction::CallClosure {
            destination,
            closure,
            parameter_types: abi.parameter_types,
            parameter_modes: abi.parameter_modes,
            arguments,
        });
        Ok(destination)
    }

    /// Materialize a resolved struct/class constructor directly from checked
    /// signature facts. Arguments still evaluate in source order, while the
    /// resulting layout fields use declaration/parameter order.
    pub(super) fn lower_record_constructor(
        &mut self,
        signature: &checked::FunctionSig,
        args: &[checked::HirCallArg],
    ) -> Result<ValueId, MirLoweringError> {
        let wire_type = signature
            .return_ty
            .as_ref()
            .map(|ty| checked_type_to_wire(ty, &self.function_name))
            .transpose()?
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "record constructor without result type",
            })?;
        if !matches!(wire_type, WireType::Named { .. }) {
            return self.unsupported("record constructor with non-named result type");
        }
        let mut values = vec![None; signature.params.len()];
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        for argument in ordered {
            let index = argument
                .parameter_index
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "record constructor with unresolved argument binding",
                })?;
            let Some(parameter) = signature.params.get(index) else {
                return self.unsupported("record constructor argument outside signature");
            };
            if values[index].is_some() {
                return self.unsupported("record constructor duplicate argument binding");
            }
            values[index] = Some((
                parameter.name.clone(),
                self.lower_expression(&argument.value)?,
            ));
        }
        let fields = values
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "record constructor missing checked field binding",
            })?;
        let destination = self.value();
        let ty = self.types.intern(wire_type);
        self.emit(MirInstruction::MakeStruct {
            destination,
            ty,
            fields,
        });
        Ok(destination)
    }

    /// `Ok` and `Err` are language result constructors, not normal runtime
    /// intrinsics. Lower them into a typed operation so VM codegen never has
    /// to rediscover a source-level builtin name.
    pub(super) fn lower_builtin_call(
        &mut self,
        callee: &rsscript_syntax::ast::Callee,
        signature: &checked::FunctionSig,
        receiver: Option<&checked::HirCallReceiver>,
        args: &[checked::HirCallArg],
    ) -> Result<ValueId, MirLoweringError> {
        // JSON decode is the current builtin whose concrete type argument
        // changes runtime behavior. Its type operand is preserved below;
        // generic channel payloads are already fully checked and phantom to
        // the VM's channel state, so they retain the ordinary `BuiltinId`.
        let destination = self.value();
        match signature.name.as_str() {
            "Ok" | "Err" => {
                if receiver.is_some() {
                    return self.unsupported("Result constructor receiver call");
                }
                if args.len() != 1 {
                    return self.unsupported("Result constructor with non-unary arity");
                }
                let value = self.lower_expression(&args[0].value)?;
                self.emit(MirInstruction::MakeResult {
                    destination,
                    ok: signature.name == "Ok",
                    value,
                });
            }
            "Some" => {
                if receiver.is_some() {
                    return self.unsupported("Option Some receiver call");
                }
                if args.len() != 1 {
                    return self.unsupported("Option Some constructor with non-unary arity");
                }
                let value = self.lower_expression(&args[0].value)?;
                self.emit(MirInstruction::MakeOption {
                    destination,
                    value: Some(value),
                });
            }
            "None" => {
                if receiver.is_some() {
                    return self.unsupported("Option None receiver call");
                }
                if !args.is_empty() {
                    return self.unsupported("Option None constructor with non-zero arity");
                }
                self.emit(MirInstruction::MakeOption {
                    destination,
                    value: None,
                });
            }
            "concat" if signature.namespace.as_deref() == Some("String") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("String.concat with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let left = self.lower_expression(&ordered[0].value)?;
                let right = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::StringConcat {
                    destination,
                    left,
                    right,
                });
            }
            "get" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("List.get with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_expression(&ordered[0].value)?;
                let index = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::ListGet {
                    destination,
                    list,
                    index,
                });
            }
            "len" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("List.len with invalid checked call shape");
                }
                let list = self.lower_expression(&args[0].value)?;
                self.emit(MirInstruction::ListLen { destination, list });
            }
            "append" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("List.append with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (values, retained_values) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::ListAppend {
                    destination,
                    list,
                    values,
                });
                if let Some(place) = retained_values {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "clear" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("List.clear with invalid checked call shape");
                }
                let list = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::ListClear { destination, list });
            }
            "pop" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("List.pop with invalid checked call shape");
                }
                let list = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::ListPop { destination, list });
            }
            "push" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("List.push with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::ListPush {
                    destination,
                    list,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove_at" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("List.remove_at with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let index = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::ListRemoveAt {
                    destination,
                    list,
                    index,
                });
            }
            "set" if signature.namespace.as_deref() == Some("List") => {
                if receiver.is_some() || args.len() != 3 {
                    return self.unsupported("List.set with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let list = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let index = self.lower_expression(&ordered[1].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[2].value)?;
                self.emit(MirInstruction::ListSet {
                    destination,
                    list,
                    index,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "clear" if signature.namespace.as_deref() == Some("Set") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Set.clear with invalid checked call shape");
                }
                let set = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::SetClear { destination, set });
            }
            "insert" if signature.namespace.as_deref() == Some("Set") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Set.insert with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let set = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::SetInsert {
                    destination,
                    set,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove" if signature.namespace.as_deref() == Some("Set") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Set.remove with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let set = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let value = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::SetRemove {
                    destination,
                    set,
                    value,
                });
            }
            "clear" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Deque.clear with invalid checked call shape");
                }
                let deque = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::DequeClear { destination, deque });
            }
            "pop_back" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Deque.pop_back with invalid checked call shape");
                }
                let deque = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::DequePopBack { destination, deque });
            }
            "pop_front" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Deque.pop_front with invalid checked call shape");
                }
                let deque = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::DequePopFront { destination, deque });
            }
            "push_back" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Deque.push_back with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let deque = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::DequePushBack {
                    destination,
                    deque,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "push_front" if signature.namespace.as_deref() == Some("Deque") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Deque.push_front with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let deque = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::DequePushFront {
                    destination,
                    deque,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "clear" if signature.namespace.as_deref() == Some("SortedMap") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("SortedMap.clear with invalid checked call shape");
                }
                let map = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::SortedMapClear { destination, map });
            }
            "insert" if signature.namespace.as_deref() == Some("SortedMap") => {
                if receiver.is_some() || args.len() != 3 {
                    return self.unsupported("SortedMap.insert with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (key, retained_key) = self.lower_retained_builtin_value(&ordered[1].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[2].value)?;
                self.emit(MirInstruction::SortedMapInsert {
                    destination,
                    map,
                    key,
                    value,
                });
                for place in [retained_key, retained_value].into_iter().flatten() {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove" if signature.namespace.as_deref() == Some("SortedMap") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("SortedMap.remove with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let key = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::SortedMapRemove {
                    destination,
                    map,
                    key,
                });
            }
            "clear" if signature.namespace.as_deref() == Some("SortedSet") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("SortedSet.clear with invalid checked call shape");
                }
                let set = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::SortedSetClear { destination, set });
            }
            "insert" if signature.namespace.as_deref() == Some("SortedSet") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("SortedSet.insert with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let set = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[1].value)?;
                self.emit(MirInstruction::SortedSetInsert {
                    destination,
                    set,
                    value,
                });
                if let Some(place) = retained_value {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove" if signature.namespace.as_deref() == Some("SortedSet") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("SortedSet.remove with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let set = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let value = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::SortedSetRemove {
                    destination,
                    set,
                    value,
                });
            }
            "clear" if signature.namespace.as_deref() == Some("Buffer") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Buffer.clear with invalid checked call shape");
                }
                let buffer = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::BufferClear {
                    destination,
                    buffer,
                });
            }
            "push" if signature.namespace.as_deref() == Some("StringBuilder") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("StringBuilder.push with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let builder = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let value = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::StringBuilderPush {
                    destination,
                    builder,
                    value,
                });
            }
            "finish" if signature.namespace.as_deref() == Some("StringBuilder") => {
                if receiver.is_some() || args.len() != 1 {
                    return self
                        .unsupported("StringBuilder.finish with invalid checked call shape");
                }
                let checked::HirExpr::Effect {
                    effect: checked::ParamEffect::Take,
                    value,
                    ..
                } = &args[0].value
                else {
                    return self
                        .unsupported("StringBuilder.finish without checked take argument effect");
                };
                let builder = self.lower_take(value)?;
                self.emit(MirInstruction::StringBuilderFinish {
                    destination,
                    builder,
                });
            }
            "get" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Map.get with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_expression(&ordered[0].value)?;
                let key = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::MapGet {
                    destination,
                    map,
                    key,
                });
            }
            "clear" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 1 {
                    return self.unsupported("Map.clear with invalid checked call shape");
                }
                let map = self.lower_mutable_builtin_place(&args[0].value)?;
                self.emit(MirInstruction::MapClear { destination, map });
            }
            "insert" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 3 {
                    return self.unsupported("Map.insert with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (key, retained_key) = self.lower_retained_builtin_value(&ordered[1].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[2].value)?;
                self.emit(MirInstruction::MapInsert {
                    destination,
                    map,
                    key,
                    value,
                });
                for place in [retained_key, retained_value].into_iter().flatten() {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "insert_old" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 3 {
                    return self.unsupported("Map.insert_old with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let (key, retained_key) = self.lower_retained_builtin_value(&ordered[1].value)?;
                let (value, retained_value) =
                    self.lower_retained_builtin_value(&ordered[2].value)?;
                self.emit(MirInstruction::MapInsertOld {
                    destination,
                    map,
                    key,
                    value,
                });
                for place in [retained_key, retained_value].into_iter().flatten() {
                    self.emit(MirInstruction::Retain { place });
                }
            }
            "remove" if signature.namespace.as_deref() == Some("Map") => {
                if receiver.is_some() || args.len() != 2 {
                    return self.unsupported("Map.remove with invalid checked call shape");
                }
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let map = self.lower_mutable_builtin_place(&ordered[0].value)?;
                let key = self.lower_expression(&ordered[1].value)?;
                self.emit(MirInstruction::MapRemove {
                    destination,
                    map,
                    key,
                });
            }
            _ => {
                let Some(namespace) = signature.namespace.as_deref() else {
                    return self.unsupported("builtin checked HIR call without namespace");
                };
                let Some(builtin) = rsscript_mir::builtin_id(namespace, &signature.name) else {
                    return self.unsupported("unsupported checked HIR builtin call");
                };
                let type_arguments = if is_json_decode_builtin(signature) {
                    self.json_decode_type_arguments(callee)?
                } else {
                    Vec::new()
                };
                let mut ordered = args.iter().collect::<Vec<_>>();
                ordered.sort_by_key(|argument| argument.evaluation_index);
                let mut arguments =
                    Vec::with_capacity(ordered.len() + usize::from(receiver.is_some()));
                if let Some(receiver) = receiver {
                    arguments.push(self.lower_direct_receiver_argument(receiver)?);
                }
                arguments.extend(
                    ordered
                        .into_iter()
                        .map(|argument| self.lower_direct_call_argument(&argument.value))
                        .collect::<Result<Vec<_>, _>>()?,
                );
                self.emit(MirInstruction::Call {
                    destination,
                    target: MirCallTarget::Builtin {
                        id: builtin,
                        parameter_modes: signature
                            .params
                            .iter()
                            .map(|parameter| match parameter.effect {
                                Some(checked::ParamEffect::Read) | None => MirParameterMode::Read,
                                Some(checked::ParamEffect::Mut) => MirParameterMode::Mut,
                                Some(checked::ParamEffect::Take) => MirParameterMode::Take,
                            })
                            .collect(),
                        type_arguments: type_arguments.into_boxed_slice(),
                    },
                    arguments,
                });
            }
        }
        Ok(destination)
    }
}
