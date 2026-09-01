//! `CheckedHirLowerer` lowering methods (part 2 of 2): call/enum/take/JSON
//! lowering. Split from `lowerer.rs` for module-size partitioning.

use super::*;

impl<'source, 'types, 'closures> CheckedHirLowerer<'source, 'types, 'closures> {
    /// JSON decode has a semantic type argument which changes the VM's
    /// decoding contract. Preserve it as a typed MIR operand instead of
    /// letting the compatibility executable IR recover it from the callee
    /// spelling at backend time. The checked HIR still carries this spelling
    /// during the transition; it is converted to canonical `WireType` before
    /// it enters MIR.
    pub(super) fn json_decode_type_arguments(
        &mut self,
        callee: &rsscript_syntax::ast::Callee,
    ) -> Result<Vec<TypeId>, MirLoweringError> {
        let type_argument = callee_type_arguments(callee)
            .and_then(|arguments| arguments.first().copied())
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "JSON decode without concrete type argument",
            })?;
        if callee_type_arguments(callee).is_some_and(|arguments| arguments.len() != 1) {
            return self.unsupported("JSON decode with invalid type argument arity");
        }
        Ok(vec![self.types.intern(WireType::parse(type_argument))])
    }

    pub(super) fn lower_enum_variant_call(
        &mut self,
        callee: &rsscript_syntax::ast::Callee,
        args: &[checked::HirCallArg],
    ) -> Result<ValueId, MirLoweringError> {
        let name = match callee {
            rsscript_syntax::ast::Callee::Name(name)
            | rsscript_syntax::ast::Callee::Qualified { name, .. } => {
                name.split('<').next().unwrap_or(name).trim()
            }
            rsscript_syntax::ast::Callee::ReceiverCall { .. } => {
                return self.unsupported("checked HIR receiver enum variant call");
            }
        };
        if let Some(ok) = match name {
            "Ok" => Some(true),
            "Err" => Some(false),
            _ => None,
        } {
            if args.len() != 1 {
                return self.unsupported("Result enum variant with non-unary arity");
            }
            let value = self.lower_expression(&args[0].value)?;
            let destination = self.value();
            self.emit(MirInstruction::MakeResult {
                destination,
                ok,
                value,
            });
            return Ok(destination);
        }
        if let Some(some) = option_variant_tag(name) {
            let destination = self.value();
            match some {
                true => {
                    if args.len() != 1 {
                        return self.unsupported("Option Some variant with non-unary arity");
                    }
                    let value = self.lower_expression(&args[0].value)?;
                    self.emit(MirInstruction::MakeOption {
                        destination,
                        value: Some(value),
                    });
                }
                false => {
                    if !args.is_empty() {
                        return self.unsupported("Option None variant with non-zero arity");
                    }
                    self.emit(MirInstruction::MakeOption {
                        destination,
                        value: None,
                    });
                }
            }
            return Ok(destination);
        }
        let Some(layout) = self.targets.variants.get(name).cloned() else {
            return self.unsupported("unknown checked HIR enum variant");
        };
        let mut values = vec![None; layout.fields.len()];
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        for argument in ordered {
            let index = argument
                .parameter_index
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "enum variant with unresolved argument binding",
                })?;
            if index >= values.len() || values[index].is_some() {
                return self.unsupported("enum variant with invalid argument binding");
            }
            values[index] = Some(self.lower_expression(&argument.value)?);
        }
        let fields = values
            .into_iter()
            .enumerate()
            .map(|(index, value)| value.map(|value| (layout.fields[index].clone(), value)))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "enum variant missing checked field binding",
            })?;
        let destination = self.value();
        let ty = self.types.intern(WireType::Named {
            package: None,
            name: layout.owner,
            arguments: Vec::new(),
        });
        self.emit(MirInstruction::MakeVariant {
            destination,
            ty,
            variant: name.to_owned(),
            fields,
        });
        Ok(destination)
    }

    pub(super) fn lower_take(
        &mut self,
        value: &checked::HirExpr,
    ) -> Result<ValueId, MirLoweringError> {
        let checked::HirExpr::Ident { name, .. } = value else {
            return self.unsupported("take checked HIR effect on non-local value");
        };
        let place = self.lookup_place(name)?;
        let destination = self.value();
        self.emit(MirInstruction::TakePlace { destination, place });
        Ok(destination)
    }

    /// A mutating builtin must carry the checked mutable place directly. This
    /// is intentionally narrower than general mutable argument lowering: each
    /// new in-place MIR operation decides its own runtime contract instead of
    /// erasing `mut` into an ordinary value before codegen.
    pub(super) fn lower_mutable_builtin_place(
        &mut self,
        value: &checked::HirExpr,
    ) -> Result<PlaceId, MirLoweringError> {
        let checked::HirExpr::Effect {
            effect: checked::ParamEffect::Mut,
            value,
            ..
        } = value
        else {
            return self.unsupported("mutating builtin argument without checked mut effect");
        };
        self.lower_mutable_place(value)
    }

    /// A checked `mut` argument is usually a local place. Struct fields are
    /// also valid mutable collection locations: materialize the field value in
    /// a compiler-private place so the existing collection MIR instructions
    /// retain their explicit mutation operand while the runtime continues to
    /// mutate the shared collection identity.
    pub(super) fn lower_mutable_place(
        &mut self,
        value: &checked::HirExpr,
    ) -> Result<PlaceId, MirLoweringError> {
        match value {
            checked::HirExpr::Ident { name, .. } => self.lookup_place(name),
            checked::HirExpr::Field { .. } => {
                let source = self.lower_expression(value)?;
                let place = self.place(&format!("$mir_mut_field_{}", self.place_names.len()));
                self.emit(MirInstruction::WritePlace {
                    place,
                    value: source,
                });
                Ok(place)
            }
            _ => self.unsupported("mutating builtin argument on non-place value"),
        }
    }

    /// Materialize a value that the resolved builtin retains and preserve the
    /// semantic retention fact when the caller supplied a `read local`.
    ///
    /// The resulting runtime operand is still a normal `ValueId`; the
    /// separate `Retain` makes the ownership contract visible to the MIR
    /// verifier instead of leaving it encoded only in the `.rssi` signature.
    pub(super) fn lower_retained_builtin_value(
        &mut self,
        argument: &checked::HirExpr,
    ) -> Result<(ValueId, Option<PlaceId>), MirLoweringError> {
        let retained_place = match argument {
            checked::HirExpr::Effect {
                effect: checked::ParamEffect::Read,
                value,
                ..
            } => match value.as_ref() {
                checked::HirExpr::Ident { name, .. } if !is_checked_literal_ident(value) => {
                    Some(self.lookup_place(name)?)
                }
                _ => None,
            },
            _ => None,
        };
        let value = self.lower_expression(argument)?;
        Ok((value, retained_place))
    }

    /// `manage local` consumes the local graph and creates a stable managed
    /// identity. Keep both operations visible instead of treating `manage` as
    /// a transparent read, otherwise a later local use could bypass the
    /// ownership transition represented by semantic HIR.
    pub(super) fn lower_manage(
        &mut self,
        value: &checked::HirExpr,
    ) -> Result<ValueId, MirLoweringError> {
        let source = self.lower_take(value)?;
        let destination = self.value();
        self.emit(MirInstruction::Manage {
            destination,
            source,
        });
        Ok(destination)
    }

    pub(super) fn lower_direct_call_argument(
        &mut self,
        argument: &checked::HirExpr,
    ) -> Result<MirCallArgument, MirLoweringError> {
        if let checked::HirExpr::Manage { value, .. } = argument {
            let checked::HirExpr::Ident { name, .. } = value.as_ref() else {
                return self.unsupported("manage checked HIR call argument on non-local value");
            };
            return self.lookup_place(name).map(MirCallArgument::BorrowRead);
        }
        let checked::HirExpr::Effect { effect, value, .. } = argument else {
            return self.lower_expression(argument).map(MirCallArgument::Value);
        };
        // `read` is an observation-only qualifier. It may be attached to an
        // rvalue such as a string literal, where there is no caller-owned
        // place to borrow. Preserve that distinction in MIR as an ordinary
        // value argument; only local `read` values use `BorrowRead` so the
        // verifier can track the place lifetime.
        if *effect == checked::ParamEffect::Read {
            match value.as_ref() {
                checked::HirExpr::Ident { name, .. } if !is_checked_literal_ident(value) => {
                    return self.lookup_place(name).map(MirCallArgument::BorrowRead);
                }
                checked::HirExpr::Manage { value, .. } => {
                    let checked::HirExpr::Ident { name, .. } = value.as_ref() else {
                        return self
                            .unsupported("manage checked HIR call argument on non-local value");
                    };
                    return self.lookup_place(name).map(MirCallArgument::BorrowRead);
                }
                _ => return self.lower_expression(value).map(MirCallArgument::Value),
            }
        }
        if *effect == checked::ParamEffect::Mut {
            return self
                .lower_mutable_place(value)
                .map(MirCallArgument::BorrowMut);
        }
        let value = match value.as_ref() {
            checked::HirExpr::Ident { .. } => value.as_ref(),
            _ => return self.unsupported("checked HIR data effect on non-local value"),
        };
        let checked::HirExpr::Ident { name, .. } = value else {
            return self.unsupported("checked HIR data effect on non-local value");
        };
        let place = self.lookup_place(name)?;
        Ok(match effect {
            checked::ParamEffect::Read | checked::ParamEffect::Mut => {
                unreachable!("read and mut effects returned above")
            }
            checked::ParamEffect::Take => MirCallArgument::Take(place),
        })
    }

    /// Receiver calls are semantically bound as parameter zero. Preserve their
    /// declared effect just like an explicitly named argument, while allowing
    /// a read-qualified rvalue receiver to remain an ordinary owned value.
    pub(super) fn lower_direct_receiver_argument(
        &mut self,
        receiver: &checked::HirCallReceiver,
    ) -> Result<MirCallArgument, MirLoweringError> {
        let value = receiver.value.as_ref();
        match receiver.effect {
            checked::ParamEffect::Read => {
                if let checked::HirExpr::Ident { name, .. } = value
                    && !is_checked_literal_ident(value)
                {
                    return self.lookup_place(name).map(MirCallArgument::BorrowRead);
                }
                self.lower_expression(value).map(MirCallArgument::Value)
            }
            checked::ParamEffect::Mut => {
                let place = self.lower_mutable_place(value)?;
                Ok(MirCallArgument::BorrowMut(place))
            }
            checked::ParamEffect::Take => {
                let checked::HirExpr::Ident { name, .. } = value else {
                    return self.unsupported("checked HIR take receiver on non-local value");
                };
                let place = self.lookup_place(name)?;
                Ok(match receiver.effect {
                    checked::ParamEffect::Take => MirCallArgument::Take(place),
                    checked::ParamEffect::Read => unreachable!("matched above"),
                    checked::ParamEffect::Mut => unreachable!("matched above"),
                })
            }
        }
    }

    pub(super) fn lower_async_binding(
        &mut self,
        name: &str,
        value: Option<&checked::HirExpr>,
    ) -> Result<(), MirLoweringError> {
        if self.tasks.contains_key(name) {
            return self.unsupported("duplicate async checked HIR binding");
        }
        let Some(value) = value else {
            return self.unsupported("async checked HIR binding without direct call");
        };
        let task = self.lower_spawn_call(value)?;
        self.tasks.insert(name.to_owned(), task);
        Ok(())
    }

    /// Lower one resolved async call into an owned child task. Both `async let`
    /// and `select` use this exact path so they share target resolution,
    /// Provider-wrapper construction, argument ownership checks, and the
    /// lexical task group.
    pub(super) fn lower_spawn_call(
        &mut self,
        value: &checked::HirExpr,
    ) -> Result<TaskId, MirLoweringError> {
        let checked::HirExpr::Call {
            receiver,
            args,
            resolution,
            ..
        } = value
        else {
            return self.unsupported("async checked HIR binding without direct call");
        };
        let checked::CallResolution::Resolved { signature, .. } = resolution else {
            return self.unsupported("unresolved async checked HIR call");
        };
        if signature.is_builtin && !is_catalog_builtin(signature) {
            return self.unsupported("async builtin checked HIR call");
        }
        let qualified = signature
            .namespace
            .as_ref()
            .map(|namespace| format!("{namespace}.{}", signature.name));
        let target = if is_catalog_builtin(signature) {
            self.targets
                .async_builtin_wrappers
                .get(
                    qualified
                        .as_deref()
                        .ok_or_else(|| MirLoweringError::Unsupported {
                            function: self.function_name.to_owned(),
                            construct: "async catalog builtin without qualified identity",
                        })?,
                )
                .copied()
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "async catalog builtin checked HIR wrapper",
                })?
        } else if signature.is_external {
            let symbol = checked_external_symbol(signature)?;
            self.targets
                .async_external_wrappers
                .get(symbol.as_str())
                .copied()
                .ok_or_else(|| MirLoweringError::Unsupported {
                    function: self.function_name.to_owned(),
                    construct: "async external checked HIR wrapper",
                })?
        } else {
            self.targets
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
                    construct: "direct async checked HIR call target",
                })?
        };
        let mut ordered = args.iter().collect::<Vec<_>>();
        ordered.sort_by_key(|argument| argument.evaluation_index);
        let mut arguments = Vec::with_capacity(ordered.len() + usize::from(receiver.is_some()));
        if let Some(receiver) = receiver {
            arguments.push(self.lower_direct_receiver_argument(receiver)?);
        }
        arguments.extend(
            ordered
                .into_iter()
                .map(|argument| self.lower_direct_call_argument(&argument.value))
                .collect::<Result<Vec<_>, _>>()?,
        );
        let task = self.task();
        self.emit(MirInstruction::Spawn {
            task,
            group: TaskGroupId::new(0),
            target,
            arguments,
        });
        Ok(task)
    }

    pub(super) fn lower_await(
        &mut self,
        value: &checked::HirExpr,
    ) -> Result<ValueId, MirLoweringError> {
        // A checked async external call is already a suspension-capable MIR
        // `CallExternal`: the VM parks the current task while its linked
        // Provider future is pending, then writes the result to the call
        // destination. Do not fabricate an internal task merely to represent
        // `await Host.call()`.
        if let checked::HirExpr::Call {
            callee,
            receiver,
            args,
            type_arguments,
            resolution,
            ..
        } = value
        {
            let checked::CallResolution::Resolved { signature, .. } = resolution else {
                return self.unsupported("unresolved awaited checked HIR call");
            };
            if signature.is_external && signature.is_async {
                return self.lower_direct_call(
                    callee,
                    receiver.as_ref(),
                    args,
                    type_arguments,
                    resolution,
                );
            }
        }
        let checked::HirExpr::Ident { name, .. } = value else {
            return self.unsupported("await of non-task checked HIR local");
        };
        let Some(task) = self.tasks.get(name).copied() else {
            return self.unsupported("await of unknown checked HIR task");
        };
        let destination = self.value();
        self.emit(MirInstruction::Await { destination, task });
        Ok(destination)
    }

    /// Lower `select` as explicit task creation, one verifier-visible first
    /// ready wait, and ordinary CFG dispatch into the selected arm. The select
    /// instruction consumes every arm task: the VM transfers the winner value
    /// and cancels/reaps all losers before the branch ladder executes.
    pub(super) fn lower_select(
        &mut self,
        arms: &[checked::HirSelectArm],
    ) -> Result<(), MirLoweringError> {
        if arms.is_empty() {
            return Ok(());
        }

        let mut tasks = Vec::with_capacity(arms.len());
        let mut arm_has_try = Vec::with_capacity(arms.len());
        for arm in arms {
            let (operation, has_try) = peel_checked_select_operation(&arm.operation);
            tasks.push(self.lower_spawn_call(operation)?);
            arm_has_try.push(has_try);
        }

        let winner = self.value();
        let value = self.value();
        self.emit(MirInstruction::Select {
            tasks,
            winner,
            value,
        });

        let join = self.new_block();
        for (index, arm) in arms.iter().enumerate() {
            let arm_block = self.new_block();
            let next = self.new_block();
            let arm_index = self.literal(MirLiteral::Int(index as i64))?;
            let matches_arm = self.value();
            self.emit(MirInstruction::Binary {
                destination: matches_arm,
                op: MirBinaryOp::Equal,
                left: winner,
                right: arm_index,
            });
            self.terminate(MirTerminator::Branch {
                condition: matches_arm,
                then_target: arm_block,
                else_target: next,
            });

            self.current = arm_block;
            let bound = if arm_has_try[index] {
                let destination = self.value();
                self.emit(MirInstruction::TryResult {
                    destination,
                    source: value,
                    cleanup: self.resource_cleanup_places(),
                });
                destination
            } else {
                value
            };
            if arm.binding != "_" {
                let binding = self.place(&arm.binding);
                self.emit(MirInstruction::WritePlace {
                    place: binding,
                    value: bound,
                });
            }
            self.lower_checked_block(&arm.body)?;
            if self.current_block().terminator.is_none() {
                self.terminate(MirTerminator::Jump(join));
            }

            self.current = next;
        }
        self.terminate(MirTerminator::Unreachable);
        self.current = join;
        Ok(())
    }

    pub(super) fn lower_if(
        &mut self,
        condition: &checked::HirExpr,
        then_body: &checked::HirBlock,
        else_body: Option<&checked::HirBlock>,
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
        self.lower_checked_block(then_body)?;
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(join_block));
        }

        self.current = else_block;
        if let Some(else_body) = else_body {
            self.lower_checked_block(else_body)?;
        }
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(join_block));
        }

        self.current = join_block;
        Ok(())
    }

    pub(super) fn lower_match(
        &mut self,
        value: &checked::HirExpr,
        arms: &[checked::HirMatchArm],
    ) -> Result<(), MirLoweringError> {
        let value = self.lower_expression(value)?;
        let join = self.new_block();
        for arm in arms {
            if arm.guard.is_some() {
                return self.unsupported("checked HIR match guard");
            }
            let arm_block = self.new_block();
            let next = self.new_block();
            let variant_bindings = match &arm.pattern {
                rsscript_syntax::ast::MatchPattern::Wildcard(_) => {
                    self.terminate(MirTerminator::Jump(arm_block));
                    None
                }
                rsscript_syntax::ast::MatchPattern::Literal { value: literal, .. } => {
                    let literal = match_literal(literal, &self.function_name)?;
                    let expected = self.literal(literal)?;
                    let condition = self.value();
                    self.emit(MirInstruction::Binary {
                        destination: condition,
                        op: MirBinaryOp::Equal,
                        left: value,
                        right: expected,
                    });
                    self.terminate(MirTerminator::Branch {
                        condition,
                        then_target: arm_block,
                        else_target: next,
                    });
                    None
                }
                rsscript_syntax::ast::MatchPattern::Variant { name, bindings, .. } => {
                    if let Some(ok) = result_variant_tag(name) {
                        let binding = self.result_pattern_binding(bindings)?;
                        self.terminate(MirTerminator::MatchResult {
                            value,
                            ok_target: if ok { arm_block } else { next },
                            err_target: if ok { next } else { arm_block },
                        });
                        Some(MatchBindings::Result { ok, binding })
                    } else if let Some(some) = option_variant_tag(name) {
                        let binding = self.option_pattern_binding(some, bindings)?;
                        self.terminate(MirTerminator::MatchOption {
                            value,
                            some_target: if some { arm_block } else { next },
                            none_target: if some { next } else { arm_block },
                        });
                        Some(MatchBindings::Option { some, binding })
                    } else {
                        let layout = self.variant_pattern_layout(name, bindings)?;
                        self.terminate(MirTerminator::MatchVariant {
                            value,
                            expected: name.clone(),
                            match_target: arm_block,
                            else_target: next,
                        });
                        Some(MatchBindings::Variant(layout, bindings.clone()))
                    }
                }
                _ => return self.unsupported("non-literal checked HIR match pattern"),
            };

            self.current = arm_block;
            if let Some(bindings) = variant_bindings {
                self.lower_match_bindings(value, bindings)?;
            }
            self.lower_checked_block(&arm.body)?;
            if self.current_block().terminator.is_none() {
                self.terminate(MirTerminator::Jump(join));
            }
            self.current = next;
        }
        self.terminate(MirTerminator::Unreachable);
        self.current = join;
        Ok(())
    }

    pub(super) fn lower_match_expression(
        &mut self,
        value: &checked::HirExpr,
        arms: &[checked::HirMatchArm],
    ) -> Result<ValueId, MirLoweringError> {
        let value = self.lower_expression(value)?;
        let result_place = self.place(&format!("__rss_mir_match_result_{}", self.next_value));
        let join = self.new_block();
        for arm in arms {
            if arm.guard.is_some() {
                return self.unsupported("checked HIR match expression guard");
            }
            let arm_block = self.new_block();
            let next = self.new_block();
            let variant_bindings = match &arm.pattern {
                rsscript_syntax::ast::MatchPattern::Wildcard(_) => {
                    self.terminate(MirTerminator::Jump(arm_block));
                    None
                }
                rsscript_syntax::ast::MatchPattern::Literal { value: literal, .. } => {
                    let expected = self.literal(match_literal(literal, &self.function_name)?)?;
                    let condition = self.value();
                    self.emit(MirInstruction::Binary {
                        destination: condition,
                        op: MirBinaryOp::Equal,
                        left: value,
                        right: expected,
                    });
                    self.terminate(MirTerminator::Branch {
                        condition,
                        then_target: arm_block,
                        else_target: next,
                    });
                    None
                }
                rsscript_syntax::ast::MatchPattern::Variant { name, bindings, .. } => {
                    if let Some(ok) = result_variant_tag(name) {
                        let binding = self.result_pattern_binding(bindings)?;
                        self.terminate(MirTerminator::MatchResult {
                            value,
                            ok_target: if ok { arm_block } else { next },
                            err_target: if ok { next } else { arm_block },
                        });
                        Some(MatchBindings::Result { ok, binding })
                    } else if let Some(some) = option_variant_tag(name) {
                        let binding = self.option_pattern_binding(some, bindings)?;
                        self.terminate(MirTerminator::MatchOption {
                            value,
                            some_target: if some { arm_block } else { next },
                            none_target: if some { next } else { arm_block },
                        });
                        Some(MatchBindings::Option { some, binding })
                    } else {
                        let layout = self.variant_pattern_layout(name, bindings)?;
                        self.terminate(MirTerminator::MatchVariant {
                            value,
                            expected: name.clone(),
                            match_target: arm_block,
                            else_target: next,
                        });
                        Some(MatchBindings::Variant(layout, bindings.clone()))
                    }
                }
                _ => return self.unsupported("non-literal checked HIR match expression pattern"),
            };

            self.current = arm_block;
            if let Some(bindings) = variant_bindings {
                self.lower_match_bindings(value, bindings)?;
            }
            self.lower_match_expression_arm(&arm.body, result_place)?;
            if self.current_block().terminator.is_none() {
                self.terminate(MirTerminator::Jump(join));
            }
            self.current = next;
        }
        self.terminate(MirTerminator::Unreachable);
        self.current = join;
        let destination = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination,
            place: result_place,
        });
        Ok(destination)
    }

    /// Resolve the checked semantic layout before emitting a match edge. The
    /// direct MIR subset deliberately accepts only a flat positional binding
    /// or wildcard for each declared field: nested patterns require their own
    /// projection and cleanup semantics.
    pub(super) fn variant_pattern_layout(
        &self,
        name: &str,
        bindings: &[rsscript_syntax::ast::MatchPattern],
    ) -> Result<VariantLayout, MirLoweringError> {
        let Some(layout) = self.targets.variants.get(name) else {
            return self.unsupported("unresolved checked HIR variant match pattern");
        };
        if layout.fields.len() != bindings.len() {
            return self.unsupported("checked HIR variant match binding arity");
        }
        if bindings.iter().any(|binding| {
            !matches!(
                binding,
                rsscript_syntax::ast::MatchPattern::Binding { .. }
                    | rsscript_syntax::ast::MatchPattern::Wildcard(_)
            )
        }) {
            return self.unsupported("nested checked HIR variant match binding");
        }
        Ok(layout.clone())
    }

    pub(super) fn lower_variant_pattern_bindings(
        &mut self,
        value: ValueId,
        layout: &VariantLayout,
        bindings: &[rsscript_syntax::ast::MatchPattern],
    ) -> Result<(), MirLoweringError> {
        for (field, binding) in layout.fields.iter().zip(bindings) {
            let rsscript_syntax::ast::MatchPattern::Binding { name, .. } = binding else {
                continue;
            };
            let destination = self.value();
            self.emit(MirInstruction::GetField {
                destination,
                base: value,
                field: field.clone(),
            });
            let place = self.place(name);
            self.emit(MirInstruction::WritePlace {
                place,
                value: destination,
            });
        }
        Ok(())
    }

    pub(super) fn result_pattern_binding(
        &self,
        bindings: &[rsscript_syntax::ast::MatchPattern],
    ) -> Result<rsscript_syntax::ast::MatchPattern, MirLoweringError> {
        let [binding] = bindings else {
            return self.unsupported("checked HIR Result match binding arity");
        };
        if !matches!(
            binding,
            rsscript_syntax::ast::MatchPattern::Binding { .. }
                | rsscript_syntax::ast::MatchPattern::Wildcard(_)
        ) {
            return self.unsupported("nested checked HIR Result match binding");
        }
        Ok(binding.clone())
    }

    pub(super) fn lower_match_bindings(
        &mut self,
        value: ValueId,
        bindings: MatchBindings,
    ) -> Result<(), MirLoweringError> {
        match bindings {
            MatchBindings::Variant(layout, bindings) => {
                self.lower_variant_pattern_bindings(value, &layout, &bindings)
            }
            MatchBindings::Result { ok, binding } => {
                let rsscript_syntax::ast::MatchPattern::Binding { name, .. } = binding else {
                    return Ok(());
                };
                let destination = self.value();
                self.emit(MirInstruction::UnwrapResult {
                    destination,
                    source: value,
                    ok,
                });
                let place = self.place(&name);
                self.emit(MirInstruction::WritePlace {
                    place,
                    value: destination,
                });
                Ok(())
            }
            MatchBindings::Option { some, binding } => {
                let Some(rsscript_syntax::ast::MatchPattern::Binding { name, .. }) = binding else {
                    return Ok(());
                };
                let destination = self.value();
                self.emit(MirInstruction::UnwrapOption {
                    destination,
                    source: value,
                });
                let place = self.place(&name);
                self.emit(MirInstruction::WritePlace {
                    place,
                    value: destination,
                });
                debug_assert!(some, "only Some patterns bind an Option payload");
                Ok(())
            }
        }
    }

    pub(super) fn option_pattern_binding(
        &self,
        some: bool,
        bindings: &[rsscript_syntax::ast::MatchPattern],
    ) -> Result<Option<rsscript_syntax::ast::MatchPattern>, MirLoweringError> {
        if !some {
            if bindings.is_empty() {
                return Ok(None);
            }
            return self.unsupported("checked HIR None match binding arity");
        }
        let [binding] = bindings else {
            return self.unsupported("checked HIR Some match binding arity");
        };
        if !matches!(
            binding,
            rsscript_syntax::ast::MatchPattern::Binding { .. }
                | rsscript_syntax::ast::MatchPattern::Wildcard(_)
        ) {
            return self.unsupported("nested checked HIR Some match binding");
        }
        Ok(Some(binding.clone()))
    }

    pub(super) fn lower_match_expression_arm(
        &mut self,
        body: &checked::HirBlock,
        result_place: PlaceId,
    ) -> Result<(), MirLoweringError> {
        let Some((last, initial)) = body.statements.split_last() else {
            return self.unsupported("empty checked HIR match expression arm");
        };
        for statement in initial {
            self.lower_statement(statement)?;
            if self.current_block().terminator.is_some() {
                return self.unsupported("terminating statement before match expression value");
            }
        }
        match last {
            checked::HirStmt::Expr(expression) => {
                let value = self.lower_expression(expression)?;
                self.emit(MirInstruction::WritePlace {
                    place: result_place,
                    value,
                });
                Ok(())
            }
            checked::HirStmt::Return { .. } => self.lower_statement(last),
            _ => self.unsupported("checked HIR match expression arm without value"),
        }
    }

    pub(super) fn lower_loop(
        &mut self,
        condition: Option<&checked::HirExpr>,
        body: &checked::HirBlock,
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
        self.lower_checked_block(body)?;
        self.loops.pop();
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(header));
        }
        self.current = exit;
        Ok(())
    }

    /// Lower synchronous `for item in List<T>` into explicit index-based CFG.
    /// Non-list and async iterator protocols remain fail-closed until MIR owns
    /// their runtime and cancellation semantics.
    pub(super) fn lower_for(
        &mut self,
        binding: &str,
        iterable: &checked::HirExpr,
        iterable_type: Option<&rsscript_semantics::ResolvedType>,
        is_async: bool,
        body: &checked::HirBlock,
    ) -> Result<(), MirLoweringError> {
        if is_async {
            return self.unsupported("async checked HIR for loop");
        }
        let Some(iterable_type) = iterable_type else {
            return self.unsupported("checked HIR for loop without resolved iterable type");
        };
        if iterable_type.root_name() != Some("List") || iterable_type.arguments().len() != 1 {
            return self.unsupported("non-list checked HIR for loop");
        }

        let list = self.lower_expression(iterable)?;
        let index_place = self.place(&format!("$for_index_{}", self.place_names.len()));
        let zero = self.literal(MirLiteral::Int(0))?;
        self.emit(MirInstruction::WritePlace {
            place: index_place,
            value: zero,
        });
        let one = self.literal(MirLiteral::Int(1))?;
        let length = self.value();
        self.emit(MirInstruction::ListLen {
            destination: length,
            list,
        });

        let header = self.new_block();
        let body_block = self.new_block();
        let increment = self.new_block();
        let exit = self.new_block();
        self.terminate(MirTerminator::Jump(header));

        self.current = header;
        let index = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination: index,
            place: index_place,
        });
        let in_bounds = self.value();
        self.emit(MirInstruction::Binary {
            destination: in_bounds,
            op: MirBinaryOp::Less,
            left: index,
            right: length,
        });
        self.terminate(MirTerminator::Branch {
            condition: in_bounds,
            then_target: body_block,
            else_target: exit,
        });

        self.current = body_block;
        let item = self.value();
        self.emit(MirInstruction::ListGet {
            destination: item,
            list,
            index,
        });
        let binding = self.place(binding);
        self.emit(MirInstruction::WritePlace {
            place: binding,
            value: item,
        });
        self.loops.push(LoopTargets {
            continue_target: increment,
            break_target: exit,
            cleanup_depth: self.resource_scopes.len(),
        });
        self.lower_checked_block(body)?;
        self.loops.pop();
        if self.current_block().terminator.is_none() {
            self.terminate(MirTerminator::Jump(increment));
        }

        self.current = increment;
        let current = self.value();
        self.emit(MirInstruction::ReadPlace {
            destination: current,
            place: index_place,
        });
        let next = self.value();
        self.emit(MirInstruction::Binary {
            destination: next,
            op: MirBinaryOp::Add,
            left: current,
            right: one,
        });
        self.emit(MirInstruction::WritePlace {
            place: index_place,
            value: next,
        });
        self.terminate(MirTerminator::Jump(header));
        self.current = exit;
        Ok(())
    }

    pub(super) fn lower_with(
        &mut self,
        resource: &checked::HirExpr,
        resource_type: Option<&rsscript_semantics::ResolvedType>,
        binding: &str,
        body: &checked::HirBlock,
    ) -> Result<(), MirLoweringError> {
        let Some(resource_type) = resource_type else {
            return self.unsupported("checked HIR resource scope without structural type");
        };
        let wire = checked_type_to_wire(resource_type, &self.function_name)?;
        let Some(resource_type) = self.intern_resource_wire_type(wire) else {
            return self.unsupported("checked HIR resource scope is not a resource type");
        };
        let source_expression = match resource {
            checked::HirExpr::Manage { value, .. } => value.as_ref(),
            other => other,
        };
        let source = self.lower_expression(source_expression)?;
        let place = self.place(binding);
        self.emit(MirInstruction::AcquireResource {
            place,
            resource_type,
            source,
        });
        self.resource_scopes.push(place);
        self.lower_checked_block(body)?;
        if self.current_block().terminator.is_none() {
            self.emit(MirInstruction::ReleaseResource { place });
        }
        let released = self.resource_scopes.pop();
        debug_assert_eq!(released, Some(place));
        Ok(())
    }

    pub(super) fn intern_resource_wire_type(&mut self, wire: WireType) -> Option<ResourceTypeId> {
        let name = resource_type_name_from_wire(&wire)?;
        Some(ResourceTypeId::new(
            self.types.intern(WireType::Resource { name }).index() as u32,
        ))
    }

    pub(super) fn lower_checked_block(
        &mut self,
        block: &checked::HirBlock,
    ) -> Result<(), MirLoweringError> {
        for statement in &block.statements {
            if self.current_block().terminator.is_some() {
                return self.unsupported("statement after checked HIR return");
            }
            self.lower_statement(statement)?;
        }
        Ok(())
    }

    pub(super) fn new_block(&mut self) -> BlockId {
        let id = BlockId::new(self.blocks.len() as u32);
        self.blocks.push(BlockDraft::new());
        id
    }

    pub(super) fn current_block(&self) -> &BlockDraft {
        &self.blocks[self.current.index()]
    }

    pub(super) fn current_block_mut(&mut self) -> &mut BlockDraft {
        &mut self.blocks[self.current.index()]
    }

    pub(super) fn emit(&mut self, instruction: MirInstruction) {
        self.current_block_mut().instructions.push(instruction);
    }

    pub(super) fn terminate(&mut self, terminator: MirTerminator) {
        debug_assert!(self.current_block().terminator.is_none());
        self.current_block_mut().terminator = Some(terminator);
    }

    pub(super) fn start_detached_block(&mut self) {
        self.current = self.new_block();
    }

    pub(super) fn emit_resource_cleanup_from(&mut self, depth: usize) {
        let places = self.resource_cleanup_places_from(depth);
        for place in places {
            self.emit(MirInstruction::ReleaseResource { place });
        }
    }

    pub(super) fn resource_cleanup_places(&self) -> Vec<PlaceId> {
        self.resource_cleanup_places_from(0)
    }

    pub(super) fn resource_cleanup_places_from(&self, depth: usize) -> Vec<PlaceId> {
        self.resource_scopes[depth..]
            .iter()
            .rev()
            .copied()
            .collect()
    }

    pub(super) fn place(&mut self, name: &str) -> PlaceId {
        if let Some(place) = self.places.get(name) {
            return *place;
        }
        let place = PlaceId::new(self.place_names.len() as u32);
        self.places.insert(name.to_owned(), place);
        self.place_names.push(name.to_owned());
        place
    }

    pub(super) fn place_with_type(&mut self, name: &str, ty: TypeId) -> PlaceId {
        let place = self.place(name);
        self.place_types.insert(name.to_owned(), ty);
        place
    }

    pub(super) fn place_type(&self, name: &str) -> Result<TypeId, MirLoweringError> {
        self.place_types
            .get(name)
            .copied()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "checked HIR closure capture without a resolved local type",
            })
    }

    pub(super) fn lookup_place(&self, name: &str) -> Result<PlaceId, MirLoweringError> {
        self.places
            .get(name)
            .copied()
            .ok_or_else(|| MirLoweringError::Unsupported {
                function: self.function_name.to_owned(),
                construct: "unknown checked HIR local",
            })
    }

    pub(super) fn value(&mut self) -> ValueId {
        let value = ValueId::new(self.next_value);
        self.next_value += 1;
        value
    }

    pub(super) fn task(&mut self) -> TaskId {
        let task = TaskId::new(self.next_task);
        self.next_task += 1;
        task
    }

    pub(super) fn unsupported<T>(&self, construct: &'static str) -> Result<T, MirLoweringError> {
        Err(MirLoweringError::Unsupported {
            function: self.function_name.to_owned(),
            construct,
        })
    }
}
