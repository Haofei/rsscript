use super::*;

pub(crate) struct RegLowerer<'a> {
    pub(crate) hir: &'a Hir,
    pub(crate) function_ids: &'a HashMap<String, usize>,
    pub(crate) functions: &'a mut Vec<RegFunction>,
    pub(crate) function: RegFunction,
    pub(crate) loop_stack: Vec<LoopPatch>,
    pub(crate) cleanup_stack: Vec<Reg>,
    /// Accumulator for the unit-wide closure-identity gate (see
    /// [`RegUnit::closure_identity_observable`]). OR-set whenever a user
    /// `==`/`!=` could compare a closure-containing operand. Shared across all
    /// function lowerings so it summarizes the whole program.
    pub(crate) closure_identity_observable: &'a std::cell::Cell<bool>,
}

impl RegLowerer<'_> {
    pub(crate) fn local(&mut self, name: &str) -> Reg {
        if let Some(reg) = self.function.local_regs.get(name) {
            return *reg;
        }
        let reg = self.temp();
        self.function.local_regs.insert(name.to_string(), reg);
        reg
    }

    fn lookup_local(&self, name: &str) -> Result<Reg, EvalError> {
        self.function
            .local_regs
            .get(name)
            .copied()
            .ok_or_else(|| EvalError::Runtime(format!("reg VM cannot resolve local `{name}`.")))
    }

    /// The declaration-order slot of `field` on a statically-known struct/variant
    /// type, used to emit `GetFieldSlot`/`SetFieldSlot`. `None` (→ name-based
    /// access) when the base type is unknown or not a registered type. Struct
    /// construction is canonicalized to this same order (see `MakeStruct`), so the
    /// runtime layout matches the slot.
    fn field_slot(&self, base_type: Option<&str>, field: &str) -> Option<usize> {
        let info = self.hir.type_info(base_type?)?;
        info.fields_ordered.iter().position(|f| f.name == field)
    }

    /// Reorder named constructor fields into the type's declaration order so every
    /// instance of a type shares one field layout (and matches `field_slot`).
    fn canonicalize_field_order(&self, type_name: &str, fields: &mut [(String, Reg)]) {
        if let Some(info) = self.hir.type_info(type_name) {
            fields.sort_by_key(|(name, _)| {
                info.fields_ordered
                    .iter()
                    .position(|f| &f.name == name)
                    .unwrap_or(usize::MAX)
            });
        }
    }

    /// Build a `MakeStruct` instruction with its layout interned once at lowering
    /// time (V2.0), so the runtime construction path never re-hashes
    /// `(name, field_names)`. `fields` must already be in canonical slot order.
    fn make_struct_instr(&self, dst: Reg, name: String, fields: Vec<(String, Reg)>) -> RegInstr {
        let layout = intern_struct_layout(&name, &fields);
        RegInstr::MakeStruct {
            dst,
            layout,
            fields,
        }
    }

    /// Build a `MakeVariant` instruction with its layout interned at lowering time
    /// (V2.0). See [`Self::make_struct_instr`].
    fn make_variant_instr(&self, dst: Reg, name: String, fields: Vec<(String, Reg)>) -> RegInstr {
        let layout = intern_struct_layout(&name, &fields);
        RegInstr::MakeVariant {
            dst,
            layout,
            fields,
        }
    }

    pub(crate) fn temp(&mut self) -> Reg {
        let reg = self.function.regs;
        self.function.regs += 1;
        reg
    }

    pub(crate) fn emit(&mut self, instr: RegInstr) -> usize {
        let ip = self.function.code.len();
        self.function.code.push(instr);
        ip
    }

    fn cleanup_regs_since(&self, base: usize) -> Vec<Reg> {
        self.cleanup_stack[base..].iter().rev().copied().collect()
    }

    fn all_cleanup_regs(&self) -> Vec<Reg> {
        self.cleanup_regs_since(0)
    }

    fn emit_cleanup_since(&mut self, base: usize) {
        for resource in self.cleanup_regs_since(base) {
            self.emit(RegInstr::ResourceDrop { resource });
        }
    }

    fn emit_all_cleanup(&mut self) {
        self.emit_cleanup_since(0);
    }

    fn patch_jump(&mut self, jump_ip: usize, target: usize) {
        match &mut self.function.code[jump_ip] {
            RegInstr::Jump {
                target: jump_target,
            }
            | RegInstr::JumpIfBool {
                target: jump_target,
                ..
            }
            | RegInstr::JumpIfIntCompare {
                target: jump_target,
                ..
            }
            | RegInstr::MatchOption {
                some_ip: jump_target,
                ..
            }
            | RegInstr::MatchResult {
                ok_ip: jump_target, ..
            }
            | RegInstr::MatchVariant {
                match_ip: jump_target,
                ..
            } => *jump_target = target,
            _ => {}
        }
    }

    fn patch_match_none(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchOption { none_ip, .. } = &mut self.function.code[match_ip] {
            *none_ip = target;
        }
    }

    fn patch_result_match_err(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchResult { err_ip, .. } = &mut self.function.code[match_ip] {
            *err_ip = target;
        }
    }

    fn patch_variant_match_else(&mut self, match_ip: usize, target: usize) {
        if let RegInstr::MatchVariant { else_ip, .. } = &mut self.function.code[match_ip] {
            *else_ip = target;
        }
    }

    fn patch_map_match_some(&mut self, match_ip: usize, target: usize) {
        match &mut self.function.code[match_ip] {
            RegInstr::MatchMapGet { some_ip, .. } | RegInstr::MatchSortedMapGet { some_ip, .. } => {
                *some_ip = target
            }
            _ => {}
        }
    }

    fn patch_map_match_none(&mut self, match_ip: usize, target: usize) {
        match &mut self.function.code[match_ip] {
            RegInstr::MatchMapGet { none_ip, .. } | RegInstr::MatchSortedMapGet { none_ip, .. } => {
                *none_ip = target
            }
            _ => {}
        }
    }

    fn patch_many(&mut self, jump_ips: Vec<usize>, target: usize) {
        for jump_ip in jump_ips {
            self.patch_jump(jump_ip, target);
        }
    }

    pub(crate) fn block(&mut self, block: &HirBlock) -> Result<(), EvalError> {
        for statement in &block.statements {
            self.statement(statement)?;
        }
        Ok(())
    }

    /// Assign `value` to an lvalue `target`. Locals are written directly; field
    /// and index targets read the current container, produce an updated copy
    /// (value semantics), and recurse to store that copy back into the enclosing
    /// place, so arbitrarily nested targets like `a.b.items[i]` compose.
    /// Lower `spawn f(args)` (and the call behind an `async let`): evaluate the
    /// arguments in the spawning task, then emit a `SpawnTask` that starts `f`
    /// as a new task and yields its handle. Only a direct call to a known
    /// function is supported (matching how the backend desugars `spawn`).
    fn lower_spawn(&mut self, value: &HirExpr) -> Result<Reg, EvalError> {
        let HirExpr::Call { callee, args, .. } = value else {
            return Err(EvalError::Runtime(
                "reg VM spawn/async-let expects a direct function call.".to_string(),
            ));
        };
        let Callee::Name(name) = callee else {
            return Err(EvalError::Runtime(
                "reg VM spawn/async-let supports only named function calls.".to_string(),
            ));
        };
        let function = self
            .function_ids
            .get(type_root_name(name))
            .copied()
            .ok_or_else(|| {
                EvalError::Runtime(format!(
                    "reg VM cannot resolve spawned function `{}`.",
                    type_root_name(name)
                ))
            })?;
        let arg_regs = args
            .iter()
            .map(|arg| self.expr(&arg.value))
            .collect::<Result<Vec<_>, _>>()?;
        let dst = self.temp();
        self.emit(RegInstr::SpawnTask {
            dst,
            function,
            args: arg_regs,
        });
        Ok(dst)
    }

    fn lower_assign(&mut self, target: &HirExpr, value: Reg) -> Result<(), EvalError> {
        match target {
            HirExpr::Ident { name, .. } => {
                let dst = self.lookup_local(name)?;
                self.emit(RegInstr::Move { dst, src: value });
                Ok(())
            }
            HirExpr::Field {
                base, name, access, ..
            } => {
                // Read the current container, write an updated copy back into
                // `base_value` in place, then store that copy into the enclosing
                // place (value semantics, composes for nested paths).
                let base_value = self.expr(base)?;
                let dst = self.temp();
                if let Some(slot) = self.field_slot(access.base_type.as_deref(), name) {
                    self.emit(RegInstr::SetFieldSlot {
                        dst,
                        base: base_value,
                        slot,
                        value,
                    });
                } else {
                    self.emit(RegInstr::SetField {
                        dst,
                        base: base_value,
                        name: name.clone(),
                        value,
                    });
                }
                self.lower_assign(base, base_value)
            }
            HirExpr::Index { base, index, .. } => {
                let base_value = self.expr(base)?;
                let index = self.expr(index)?;
                let dst = self.temp();
                self.emit(RegInstr::ListSet {
                    dst,
                    list: base_value,
                    index,
                    value,
                });
                self.lower_assign(base, base_value)
            }
            _ => Err(EvalError::Runtime(
                "reg VM assignment target must be a local, field, or index path.".to_string(),
            )),
        }
    }

    fn expr_block_value(&mut self, block: &HirBlock) -> Result<Reg, EvalError> {
        let Some((last, prefix)) = block.statements.split_last() else {
            return Err(EvalError::Runtime(
                "reg VM match expression arm cannot be empty.".to_string(),
            ));
        };
        for statement in prefix {
            self.statement(statement)?;
        }
        match last {
            HirStmt::Expr(value) => self.expr(value),
            HirStmt::Return { value, .. } => {
                let src = if let Some(value) = value {
                    self.expr(value)?
                } else {
                    let src = self.temp();
                    self.emit(RegInstr::LoadUnit { dst: src });
                    src
                };
                self.emit_all_cleanup();
                self.emit(RegInstr::Return { src });
                Ok(src)
            }
            other => Err(EvalError::Runtime(format!(
                "reg VM match expression arm must end with an expression, got `{other:?}`."
            ))),
        }
    }

    fn condition_jump(
        &mut self,
        condition: &HirExpr,
        expected: bool,
        target: usize,
    ) -> Result<usize, EvalError> {
        if let HirExpr::Binary {
            op, left, right, ..
        } = condition
        {
            if let Some(op) = int_compare_op(*op) {
                let lhs = self.expr(left)?;
                let rhs = self.expr(right)?;
                return Ok(self.emit(RegInstr::JumpIfIntCompare {
                    lhs,
                    rhs,
                    op,
                    expected,
                    target,
                }));
            }
        }
        let cond = self.expr(condition)?;
        Ok(self.emit(RegInstr::JumpIfBool {
            cond,
            expected,
            target,
        }))
    }

    fn statement(&mut self, statement: &HirStmt) -> Result<(), EvalError> {
        match statement {
            HirStmt::Let {
                name,
                value,
                is_async,
                ..
            } => {
                let dst = self.local(name);
                if let Some(value) = value {
                    // `async let x = f()` spawns `f` as a task and binds `x` to its
                    // handle; a plain `let` evaluates eagerly in the current task.
                    let src = if *is_async {
                        self.lower_spawn(value)?
                    } else {
                        self.expr(value)?
                    };
                    self.emit(RegInstr::Move { dst, src });
                } else {
                    self.emit(RegInstr::LoadUnit { dst });
                }
            }
            HirStmt::Assign { target, value, .. } => {
                let src = self.expr(value)?;
                self.lower_assign(target, src)?;
            }
            HirStmt::Return { value, .. } => {
                let src = if let Some(value) = value {
                    self.expr(value)?
                } else {
                    let src = self.temp();
                    self.emit(RegInstr::LoadUnit { dst: src });
                    src
                };
                self.emit_all_cleanup();
                self.emit(RegInstr::Return { src });
            }
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                let src = self.expr(resource)?;
                let dst = self.local(binding);
                self.emit(RegInstr::Move { dst, src });
                self.cleanup_stack.push(dst);
                self.block(body)?;
                self.cleanup_stack
                    .pop()
                    .expect("with cleanup stack should contain binding");
                self.emit(RegInstr::ResourceDrop { resource: dst });
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                let else_jump = self.condition_jump(condition, false, usize::MAX)?;
                self.block(then_body)?;
                let end_jump = self.emit(RegInstr::Jump { target: usize::MAX });
                let else_ip = self.function.code.len();
                self.patch_jump(else_jump, else_ip);
                if let Some(else_body) = else_body {
                    self.block(else_body)?;
                }
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                let start_ip = self.function.code.len();
                let exit_jump = if let Some(condition) = condition {
                    Some(self.condition_jump(condition, false, usize::MAX)?)
                } else {
                    None
                };
                self.loop_stack.push(LoopPatch {
                    cleanup_base: self.cleanup_stack.len(),
                    ..LoopPatch::default()
                });
                self.block(body)?;
                let loop_patch = self.loop_stack.pop().expect("loop patch should exist");
                self.emit(RegInstr::Jump { target: start_ip });
                let exit_ip = self.function.code.len();
                if let Some(exit_jump) = exit_jump {
                    self.patch_jump(exit_jump, exit_ip);
                }
                self.patch_many(loop_patch.breaks, exit_ip);
                self.patch_many(loop_patch.continues, start_ip);
            }
            HirStmt::For {
                binding,
                iterable,
                is_async,
                body,
                ..
            } => {
                if *is_async {
                    let stream = self.expr(iterable)?;
                    let start_ip = self.function.code.len();
                    let next_result = self.temp();
                    self.emit(RegInstr::CallIntrinsic {
                        dst: next_result,
                        intrinsic: RegIntrinsic::StreamNext,
                        args: vec![stream],
                    });
                    let next_option = self.temp();
                    self.emit(RegInstr::TryResult {
                        dst: next_option,
                        src: next_result,
                        cleanup: self.all_cleanup_regs(),
                    });
                    let match_ip = self.emit(RegInstr::MatchOption {
                        src: next_option,
                        some_ip: usize::MAX,
                        none_ip: usize::MAX,
                    });
                    let some_ip = self.function.code.len();
                    self.patch_jump(match_ip, some_ip);
                    let item = self.local(binding);
                    self.emit(RegInstr::UnwrapSome {
                        dst: item,
                        src: next_option,
                    });

                    self.loop_stack.push(LoopPatch {
                        cleanup_base: self.cleanup_stack.len(),
                        ..LoopPatch::default()
                    });
                    self.block(body)?;
                    let loop_patch = self.loop_stack.pop().expect("loop patch should exist");
                    self.emit(RegInstr::Jump { target: start_ip });
                    let exit_ip = self.function.code.len();
                    self.patch_match_none(match_ip, exit_ip);
                    self.patch_many(loop_patch.breaks, exit_ip);
                    self.patch_many(loop_patch.continues, start_ip);
                    return Ok(());
                }
                let list = self.expr(iterable)?;
                let index = self.temp();
                self.emit(RegInstr::LoadInt {
                    dst: index,
                    value: 0,
                });
                let len = self.temp();
                self.emit(RegInstr::ListLen { dst: len, list });
                let one = self.temp();
                self.emit(RegInstr::LoadInt { dst: one, value: 1 });

                let start_ip = self.function.code.len();
                let exit_jump = self.emit(RegInstr::JumpIfIntCompare {
                    lhs: index,
                    rhs: len,
                    op: RegIntCompare::Less,
                    expected: false,
                    target: usize::MAX,
                });
                let item = self.local(binding);
                self.emit(RegInstr::ListGet {
                    dst: item,
                    list,
                    index,
                });

                self.loop_stack.push(LoopPatch {
                    cleanup_base: self.cleanup_stack.len(),
                    ..LoopPatch::default()
                });
                self.block(body)?;
                let loop_patch = self.loop_stack.pop().expect("loop patch should exist");
                let continue_ip = self.function.code.len();
                self.emit(RegInstr::AddInt {
                    dst: index,
                    lhs: index,
                    rhs: one,
                });
                self.emit(RegInstr::Jump { target: start_ip });
                let exit_ip = self.function.code.len();
                self.patch_jump(exit_jump, exit_ip);
                self.patch_many(loop_patch.breaks, exit_ip);
                self.patch_many(loop_patch.continues, continue_ip);
            }
            HirStmt::Match { value, arms, .. } => {
                if !self.map_get_match(value, arms)?
                    && !self.variant_match(value, arms)?
                    && !self.struct_match(value, arms)?
                {
                    return Err(EvalError::Runtime(
                        "reg VM v0 does not support this match pattern.".to_string(),
                    ));
                }
            }
            HirStmt::Select { arms, .. } => {
                // First-ready select: spawn each arm's operation as a concurrent
                // task, park on whichever finishes first, then dispatch to that
                // arm's body. The scheduler's clock makes timing (e.g. differing
                // sleeps) decide the winner, matching the backend's executor.
                if arms.is_empty() {
                    return Ok(());
                }
                let mut handles = Vec::with_capacity(arms.len());
                let mut arm_has_try = Vec::with_capacity(arms.len());
                for arm in arms {
                    let (call, has_try) = peel_select_operation(&arm.operation);
                    handles.push(self.lower_spawn(call)?);
                    arm_has_try.push(has_try);
                }
                let winner = self.temp();
                let value = self.temp();
                self.emit(RegInstr::SelectWait {
                    handles,
                    winner,
                    value,
                });
                let mut end_jumps = Vec::with_capacity(arms.len());
                for (index, arm) in arms.iter().enumerate() {
                    let index_const = self.temp();
                    self.emit(RegInstr::LoadInt {
                        dst: index_const,
                        value: index as i64,
                    });
                    let is_winner = self.temp();
                    self.emit(RegInstr::Equal {
                        dst: is_winner,
                        lhs: winner,
                        rhs: index_const,
                    });
                    let skip = self.emit(RegInstr::JumpIfBool {
                        cond: is_winner,
                        expected: false,
                        target: usize::MAX,
                    });
                    // The winning arm's value is the spawned task's result; apply
                    // the arm operation's `?` (if any) before binding.
                    let bound = if arm_has_try[index] {
                        let dst = self.temp();
                        self.emit(RegInstr::TryResult {
                            dst,
                            src: value,
                            cleanup: self.all_cleanup_regs(),
                        });
                        dst
                    } else {
                        value
                    };
                    if arm.binding != "_" {
                        let binding = self.local(&arm.binding);
                        self.emit(RegInstr::Move {
                            dst: binding,
                            src: bound,
                        });
                    }
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
                    let next_arm = self.function.code.len();
                    self.patch_jump(skip, next_arm);
                }
                let end = self.function.code.len();
                for jump in end_jumps {
                    self.patch_jump(jump, end);
                }
            }
            HirStmt::Break(_) => {
                if self.loop_stack.is_empty() {
                    return Err(EvalError::Runtime(
                        "reg VM break used outside of a loop.".to_string(),
                    ));
                }
                let cleanup_base = self
                    .loop_stack
                    .last()
                    .expect("loop patch should exist")
                    .cleanup_base;
                self.emit_cleanup_since(cleanup_base);
                let jump = self.emit(RegInstr::Jump { target: usize::MAX });
                self.loop_stack
                    .last_mut()
                    .expect("loop patch should exist")
                    .breaks
                    .push(jump);
            }
            HirStmt::Continue(_) => {
                if self.loop_stack.is_empty() {
                    return Err(EvalError::Runtime(
                        "reg VM continue used outside of a loop.".to_string(),
                    ));
                }
                let cleanup_base = self
                    .loop_stack
                    .last()
                    .expect("loop patch should exist")
                    .cleanup_base;
                self.emit_cleanup_since(cleanup_base);
                let jump = self.emit(RegInstr::Jump { target: usize::MAX });
                self.loop_stack
                    .last_mut()
                    .expect("loop patch should exist")
                    .continues
                    .push(jump);
            }
            HirStmt::Expr(expr) => {
                self.expr(expr)?;
            }
            unsupported => Err(EvalError::Runtime(format!(
                "reg VM v0 does not support statement `{unsupported:?}`."
            )))?,
        }
        Ok(())
    }

    fn logical_binary(
        &mut self,
        op: BinaryOp,
        left: &HirExpr,
        right: &HirExpr,
    ) -> Result<Reg, EvalError> {
        let lhs = self.expr(left)?;
        let dst = self.temp();
        match op {
            BinaryOp::LogicalAnd => {
                let short_circuit = self.emit(RegInstr::JumpIfBool {
                    cond: lhs,
                    expected: false,
                    target: usize::MAX,
                });
                let rhs = self.expr(right)?;
                self.emit(RegInstr::Move { dst, src: rhs });
                let end_jump = self.emit(RegInstr::Jump { target: usize::MAX });
                let false_ip = self.function.code.len();
                self.patch_jump(short_circuit, false_ip);
                self.emit(RegInstr::LoadBool { dst, value: false });
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            BinaryOp::LogicalOr => {
                let short_circuit = self.emit(RegInstr::JumpIfBool {
                    cond: lhs,
                    expected: true,
                    target: usize::MAX,
                });
                let rhs = self.expr(right)?;
                self.emit(RegInstr::Move { dst, src: rhs });
                let end_jump = self.emit(RegInstr::Jump { target: usize::MAX });
                let true_ip = self.function.code.len();
                self.patch_jump(short_circuit, true_ip);
                self.emit(RegInstr::LoadBool { dst, value: true });
                let end_ip = self.function.code.len();
                self.patch_jump(end_jump, end_ip);
            }
            _ => unreachable!(),
        }
        Ok(dst)
    }

    fn expr(&mut self, expr: &HirExpr) -> Result<Reg, EvalError> {
        match expr {
            HirExpr::Ident { name, .. } if name == "Unit" => {
                let dst = self.temp();
                self.emit(RegInstr::LoadUnit { dst });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } if name == "None" => {
                let dst = self.temp();
                self.emit(RegInstr::LoadNone { dst });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } if name == "true" || name == "false" => {
                let dst = self.temp();
                self.emit(RegInstr::LoadBool {
                    dst,
                    value: name == "true",
                });
                Ok(dst)
            }
            HirExpr::Ident { name, .. } if self.hir.sum_type_for_variant(name).is_some() => {
                let fields = self.hir.sum_variant_fields(name).unwrap_or(&[]);
                if !fields.is_empty() {
                    return Err(EvalError::Runtime(format!(
                        "reg VM variant `{name}` requires {} field(s).",
                        fields.len()
                    )));
                }
                let dst = self.temp();
                let instr = self.make_variant_instr(dst, name.clone(), Vec::new());
                self.emit(instr);
                Ok(dst)
            }
            HirExpr::Ident { name, .. } => self.lookup_local(name),
            HirExpr::Number { value, .. } => {
                let dst = self.temp();
                if value.contains('.') {
                    let value = value.parse::<f64>().map_err(|error| {
                        EvalError::Runtime(format!("invalid reg VM float `{value}`: {error}"))
                    })?;
                    self.emit(RegInstr::LoadFloat { dst, value });
                } else {
                    let value = value.parse::<i64>().map_err(|error| {
                        EvalError::Runtime(format!("invalid reg VM integer `{value}`: {error}"))
                    })?;
                    self.emit(RegInstr::LoadInt { dst, value });
                }
                Ok(dst)
            }
            HirExpr::String { value, .. } => {
                let dst = self.temp();
                self.emit(RegInstr::LoadString {
                    dst,
                    value: Rc::new(decode_string_token(value)),
                });
                Ok(dst)
            }
            HirExpr::ArrayLiteral { items, .. } => {
                let items = items
                    .iter()
                    .map(|item| self.expr(item))
                    .collect::<Result<Vec<_>, _>>()?;
                let dst = self.temp();
                self.emit(RegInstr::MakeList { dst, items });
                Ok(dst)
            }
            HirExpr::ObjectLiteral { fields, .. } => {
                let fields = fields
                    .iter()
                    .map(|field| Ok((field.name.clone(), self.expr(&field.value)?)))
                    .collect::<Result<Vec<_>, EvalError>>()?;
                let dst = self.temp();
                self.emit(RegInstr::MakeObject { dst, fields });
                Ok(dst)
            }
            HirExpr::MapLiteral { entries, .. } => {
                let entries = entries
                    .iter()
                    .map(|entry| Ok((self.expr(&entry.key)?, self.expr(&entry.value)?)))
                    .collect::<Result<Vec<_>, EvalError>>()?;
                let dst = self.temp();
                self.emit(RegInstr::MakeMap { dst, entries });
                Ok(dst)
            }
            HirExpr::Binary {
                op, left, right, ..
            } => {
                if matches!(op, BinaryOp::LogicalAnd | BinaryOp::LogicalOr) {
                    return self.logical_binary(*op, left, right);
                }
                // Closure-identity gate: a user `==`/`!=` is the only observer of
                // closure pointer identity. If either operand's static type might
                // be, or transitively contain, a `Fn`, sharing cached closures
                // would make distinct allocations compare equal — so flag the
                // program as identity-observable (disabling the cache). Unknown
                // operand types are treated conservatively as observable.
                if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual)
                    && !self.closure_identity_observable.get()
                {
                    let observable = [left.as_ref(), right.as_ref()].iter().any(|operand| {
                        match reg_expr_type_name(operand) {
                            Some(type_name) => type_name_may_contain_fn(type_name, self.hir),
                            None => true,
                        }
                    });
                    if observable {
                        self.closure_identity_observable.set(true);
                    }
                }
                let lhs = self.expr(left)?;
                let rhs = self.expr(right)?;
                let dst = self.temp();
                let instr = match op {
                    BinaryOp::Add => RegInstr::AddInt { dst, lhs, rhs },
                    BinaryOp::Subtract => RegInstr::SubInt { dst, lhs, rhs },
                    BinaryOp::Multiply => RegInstr::MulInt { dst, lhs, rhs },
                    BinaryOp::Divide => RegInstr::DivInt { dst, lhs, rhs },
                    BinaryOp::Modulo => RegInstr::ModInt { dst, lhs, rhs },
                    BinaryOp::BitAnd => RegInstr::BitAndInt { dst, lhs, rhs },
                    BinaryOp::BitOr => RegInstr::BitOrInt { dst, lhs, rhs },
                    BinaryOp::BitXor => RegInstr::BitXorInt { dst, lhs, rhs },
                    BinaryOp::ShiftLeft => RegInstr::ShiftLeftInt { dst, lhs, rhs },
                    BinaryOp::ShiftRight => RegInstr::ShiftRightInt { dst, lhs, rhs },
                    BinaryOp::Less => RegInstr::LessInt { dst, lhs, rhs },
                    BinaryOp::LessEqual => RegInstr::LessEqualInt { dst, lhs, rhs },
                    BinaryOp::Greater => RegInstr::GreaterInt { dst, lhs, rhs },
                    BinaryOp::GreaterEqual => RegInstr::GreaterEqualInt { dst, lhs, rhs },
                    BinaryOp::Equal => RegInstr::Equal { dst, lhs, rhs },
                    BinaryOp::NotEqual => RegInstr::NotEqual { dst, lhs, rhs },
                    BinaryOp::LogicalAnd | BinaryOp::LogicalOr => unreachable!(),
                };
                self.emit(instr);
                Ok(dst)
            }
            HirExpr::Field {
                base, name, access, ..
            } => {
                let slot = self.field_slot(access.base_type.as_deref(), name);
                let base = self.expr(base)?;
                let dst = self.temp();
                if let Some(slot) = slot {
                    self.emit(RegInstr::GetFieldSlot { dst, base, slot });
                } else {
                    self.emit(RegInstr::GetField {
                        dst,
                        base,
                        name: name.clone(),
                    });
                }
                Ok(dst)
            }
            HirExpr::Index { base, index, .. } => {
                let list = self.expr(base)?;
                let index = self.expr(index)?;
                let dst = self.temp();
                self.emit(RegInstr::ListGet { dst, list, index });
                Ok(dst)
            }
            HirExpr::Effect { value, .. } => self.expr(value),
            HirExpr::Await { value, .. } => {
                let src = self.expr(value)?;
                let dst = self.temp();
                self.emit(RegInstr::AwaitJoin { dst, src });
                Ok(dst)
            }
            HirExpr::Spawn { value, .. } => self.lower_spawn(value),
            HirExpr::Manage { value, .. } => {
                let src = self.expr(value)?;
                let dst = self.temp();
                self.emit(RegInstr::Manage { dst, src });
                Ok(dst)
            }
            HirExpr::Try { value, .. } => {
                let src = self.expr(value)?;
                let dst = self.temp();
                self.emit(RegInstr::TryResult {
                    dst,
                    src,
                    cleanup: self.all_cleanup_regs(),
                });
                Ok(dst)
            }
            HirExpr::Call {
                callee,
                args,
                receiver,
                ..
            } => self.call(callee, receiver.as_ref(), args),
            HirExpr::Closure {
                params,
                captures,
                body,
                ..
            } => {
                let capture_names =
                    closure_capture_names(body, params, captures, &self.function.local_regs);
                let capture_regs = capture_names
                    .iter()
                    .map(|capture| self.lookup_local(capture))
                    .collect::<Result<Vec<_>, _>>()?;
                let function_id = self.functions.len();
                self.functions
                    .push(RegFunction::placeholder(format!("<closure:{function_id}>")));
                let closure_function = {
                    let mut lowerer = RegLowerer {
                        hir: self.hir,
                        function_ids: self.function_ids,
                        functions: &mut *self.functions,
                        function: RegFunction {
                            name: format!("<closure:{function_id}>"),
                            params: params.len(),
                            captures: capture_names.len(),
                            regs: 0,
                            local_regs: HashMap::new(),
                            code: Vec::new(),
                            jit_analysis: std::cell::Cell::new(None),
                            jit_self_recursive_int: std::cell::Cell::new(None),
                            native_status: std::cell::Cell::new(0),
                            call_count: std::cell::Cell::new(0),
                            branch_count: std::cell::Cell::new(0),
                            profile: RefCell::new(None),
                            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
                        },
                        loop_stack: Vec::new(),
                        cleanup_stack: Vec::new(),
                        closure_identity_observable: self.closure_identity_observable,
                    };
                    for capture in &capture_names {
                        lowerer.local(capture);
                    }
                    for param in params {
                        lowerer.local(param);
                    }
                    // A closure whose body ends in a bare expression yields that
                    // expression's value (e.g. `|x| x > 10`), matching the Rust
                    // backend's tail-expression closure rule. Bodies ending in any
                    // other statement (including an explicit `return`) fall through
                    // to an implicit `Unit` return.
                    if let Some((HirStmt::Expr(value), prefix)) = body.statements.split_last() {
                        for statement in prefix {
                            lowerer.statement(statement)?;
                        }
                        let src = lowerer.expr(value)?;
                        lowerer.emit(RegInstr::Return { src });
                    } else {
                        lowerer.block(body)?;
                        let unit = lowerer.temp();
                        lowerer.emit(RegInstr::LoadUnit { dst: unit });
                        lowerer.emit(RegInstr::Return { src: unit });
                    }
                    lowerer.function
                };
                self.functions[function_id] = closure_function;
                let dst = self.temp();
                self.emit(RegInstr::MakeClosure {
                    dst,
                    function: function_id,
                    captures: capture_regs,
                });
                Ok(dst)
            }
            HirExpr::Match { value, arms, .. } => self.match_expr(value, arms),
            unsupported => Err(EvalError::Runtime(format!(
                "reg VM v0 does not support expression `{unsupported:?}`."
            )))?,
        }
    }

    fn call(
        &mut self,
        callee: &Callee,
        receiver: Option<&HirCallReceiver>,
        args: &[HirCallArg],
    ) -> Result<Reg, EvalError> {
        if let Callee::ReceiverCall { method, .. } = callee {
            // A receiver call `x.method(args)` is sugar for `Type.method(self, args)`.
            // Rather than maintain a second (perpetually-incomplete) intrinsic table
            // here, reuse the full qualified-call lowering — stdlib intrinsics, native
            // functions, user-defined methods, and protocol dispatch — by recursing
            // with the receiver as the first argument. (The reg VM previously bailed
            // on any receiver call outside a small hand-written subset, which blocked
            // running real packages like tinygrad-rss.)
            let Some(receiver) = receiver else {
                return Err(EvalError::Runtime(format!(
                    "reg VM receiver call `{method}` is missing HIR receiver metadata."
                )));
            };
            let Some(namespace) = receiver
                .resolved_namespace
                .as_deref()
                .or(receiver.type_name.as_deref())
            else {
                return Err(EvalError::Runtime(format!(
                    "reg VM receiver call `{method}` is missing receiver type metadata."
                )));
            };
            let synthetic_callee = Callee::Qualified {
                namespace: namespace.to_string(),
                name: method.clone(),
            };
            let mut synthetic_args = Vec::with_capacity(args.len() + 1);
            synthetic_args.push(HirCallArg {
                name: None,
                value: (*receiver.value).clone(),
                span: crate::diagnostic::Span::default(),
            });
            synthetic_args.extend(args.iter().cloned());
            return self.call(&synthetic_callee, None, &synthetic_args);
        }

        let arg_regs = args
            .iter()
            .map(|arg| self.expr(&arg.value))
            .collect::<Result<Vec<_>, _>>()?;
        let dst = self.temp();
        match callee {
            Callee::Name(name) => {
                // A bare name that resolves to a LOCAL BINDING (and is not a
                // known free function) is a first-class closure value being
                // called: `let f = r.fxn; f(7)`. Calling it dispatches through
                // `CallClosure` on the stored `VmValue::Closure`. This is the VM
                // side of first-class `owned Fn` values.
                if self.function_ids.get(type_root_name(name)).is_none()
                    && let Some(&closure) = self.function.local_regs.get(name)
                {
                    // A `mut`-annotated argument at a closure call site
                    // (`f(read u, mut ctx)`) is an exclusive borrow for the
                    // call: the closure's matching `mut` parameter may mutate it,
                    // and the result is written back to the caller's binding. The
                    // call-site effect is the `HirExpr::Effect { Mut, .. }`
                    // wrapper (already type-checked against the stored `Fn`'s
                    // declared `mut` parameter), so mirror `CallKnown`'s
                    // `mut_args` from it.
                    let mut_args = call_arg_mut_positions(args);
                    self.emit(RegInstr::CallClosure {
                        dst,
                        closure,
                        args: arg_regs,
                        mut_args,
                    });
                    return Ok(dst);
                }
                // A generic call carries its type args in `name` (e.g.
                // `get_v<Int>`); functions are keyed by their bare name, so strip
                // the generics before the lookup — otherwise a generic *function*
                // call falls through and is mis-lowered as a struct construction.
                if let Some(function) = self.function_ids.get(type_root_name(name)).copied() {
                    let mut_args = self.user_mut_arg_positions(name);
                    self.emit(RegInstr::CallKnown {
                        dst,
                        function,
                        args: arg_regs,
                        mut_args,
                    });
                } else if self.is_native_function(None, name) {
                    let mut_args = self.native_mut_arg_positions(None, name);
                    self.emit(RegInstr::CallNative {
                        dst,
                        key: type_root_name(name).to_string(),
                        args: arg_regs,
                        mut_args,
                    });
                } else if type_root_name(name) == "Some" {
                    if arg_regs.len() != 1 {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Option variant `Some` expected 1 payload, got {}.",
                            arg_regs.len()
                        )));
                    }
                    self.emit(RegInstr::MakeSome {
                        dst,
                        value: arg_regs[0],
                    });
                } else if matches!(type_root_name(name), "Ok" | "Err") {
                    if arg_regs.len() != 1 {
                        return Err(EvalError::Runtime(format!(
                            "reg VM Result variant `{}` expected 1 payload, got {}.",
                            type_root_name(name),
                            arg_regs.len()
                        )));
                    }
                    let instr = self.make_variant_instr(
                        dst,
                        type_root_name(name).to_string(),
                        vec![("value".to_string(), arg_regs[0])],
                    );
                    self.emit(instr);
                } else if self
                    .hir
                    .sum_type_for_variant(type_root_name(name))
                    .is_some()
                {
                    let variant_name = type_root_name(name);
                    let fields = self.hir.sum_variant_fields(variant_name).unwrap_or(&[]);
                    match fields.len() {
                        0 if arg_regs.is_empty() => {
                            let instr =
                                self.make_variant_instr(dst, variant_name.to_string(), Vec::new());
                            self.emit(instr);
                        }
                        1 if arg_regs.len() == 1 => {
                            let instr = self.make_variant_instr(
                                dst,
                                variant_name.to_string(),
                                vec![(fields[0].name.clone(), arg_regs[0])],
                            );
                            self.emit(instr);
                        }
                        field_count if field_count == arg_regs.len() => {
                            let fields = args
                                .iter()
                                .zip(arg_regs)
                                .enumerate()
                                .map(|(index, (arg, reg))| {
                                    let name = arg
                                        .name
                                        .clone()
                                        .unwrap_or_else(|| fields[index].name.clone());
                                    (name, reg)
                                })
                                .collect::<Vec<_>>();
                            let instr =
                                self.make_variant_instr(dst, variant_name.to_string(), fields);
                            self.emit(instr);
                        }
                        field_count => {
                            return Err(EvalError::Runtime(format!(
                                "reg VM variant `{variant_name}` expected {field_count} field(s), got {}.",
                                arg_regs.len()
                            )));
                        }
                    }
                } else {
                    let mut fields = args
                        .iter()
                        .zip(arg_regs)
                        .map(|(arg, reg)| {
                            arg.name.clone().map(|name| (name, reg)).ok_or_else(|| {
                                EvalError::Runtime(
                                    "reg VM v0 struct constructors require named fields."
                                        .to_string(),
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    let type_name = type_root_name(name).to_string();
                    self.canonicalize_field_order(&type_name, &mut fields);
                    let instr = self.make_struct_instr(dst, type_name, fields);
                    self.emit(instr);
                }
            }
            Callee::Qualified { namespace, name } => {
                let namespace_root = type_root_name(namespace);
                let name_root = type_root_name(name);
                let intrinsic = if let Some(intrinsic) =
                    qualified_intrinsic(namespace_root, name_root)
                {
                    intrinsic
                } else {
                    match (namespace_root, name_root) {
                        ("Buffer", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Buffer.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::BufferClear {
                                dst,
                                buffer: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Cache", "insert") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Cache.insert expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapInsert {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("CancellationToken", "is_cancelled") => {
                            RegIntrinsic::CancellationTokenIsCancelled
                        }
                        ("ConfigStore", "replace") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM ConfigStore.replace expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ConfigStoreReplace {
                                dst,
                                store: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Counter", "add") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Counter.add expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::CounterAdd {
                                dst,
                                counter: arg_regs[0],
                                amount: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequeClear {
                                dst,
                                deque: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "pop_back") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.pop_back expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequePopBack {
                                dst,
                                deque: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "pop_front") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.pop_front expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequePopFront {
                                dst,
                                deque: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "push_back") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.push_back expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequePushBack {
                                dst,
                                deque: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Deque", "push_front") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Deque.push_front expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::DequePushFront {
                                dst,
                                deque: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("GlobalConfig", "replace") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM GlobalConfig.replace expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::GlobalConfigReplace {
                                dst,
                                global: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Http", "post_json_bearer_retry_async") => {
                            RegIntrinsic::HttpPostJsonBearerRetryAsync
                        }
                        ("Json", "array_contains_substring") => {
                            RegIntrinsic::JsonArrayContainsSubstring
                        }
                        ("Json", "bool_at_or") | ("Json", "json_bool_at_or") => {
                            RegIntrinsic::JsonBoolAtOr
                        }
                        ("Json", "string_at_or") | ("Json", "json_string_at_or") => {
                            RegIntrinsic::JsonStringAtOr
                        }
                        ("List", "append") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.append expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListAppend {
                                dst,
                                list: arg_regs[0],
                                values: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListClear {
                                dst,
                                list: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("List", "filter") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.filter expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListFilter {
                                dst,
                                list: arg_regs[0],
                                predicate: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "fold") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.fold expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListFold {
                                dst,
                                list: arg_regs[0],
                                state: arg_regs[1],
                                folder: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("List", "get") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.get expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListGet {
                                dst,
                                list: arg_regs[0],
                                index: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "len") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.len expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListLen {
                                dst,
                                list: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("List", "map") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.map expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListMap {
                                dst,
                                list: arg_regs[0],
                                mapper: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "pop") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.pop expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListPop {
                                dst,
                                list: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("List", "remove_at") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.remove_at expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListRemoveAt {
                                dst,
                                list: arg_regs[0],
                                index: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "set") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.set expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListSet {
                                dst,
                                list: arg_regs[0],
                                index: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("List", "sort") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.sort expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListSort {
                                dst,
                                list: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("List", "sort_by") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.sort_by expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListSortBy {
                                dst,
                                list: arg_regs[0],
                                key: arg_regs[1],
                                compare: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("List", "sort_with") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.sort_with expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListSortWith {
                                dst,
                                list: arg_regs[0],
                                compare: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("List", "push") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM List.push expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListPush {
                                dst,
                                list: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Map", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapClear {
                                dst,
                                map: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Map", "get") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.get expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapGet {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Map", "insert") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.insert expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapInsert {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("Map", "insert_old") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.insert_old expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapInsertOld {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("Map", "remove") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Map.remove expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::MapRemove {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Process", "run_many_stdout_timeout") => {
                            RegIntrinsic::ProcessRunManyStdoutTimeout
                        }
                        ("Process", "run_many_stdout_timeout_async") => {
                            RegIntrinsic::ProcessRunManyStdoutTimeoutAsync
                        }
                        ("Process", "run_request_cancellable_async") => {
                            RegIntrinsic::ProcessRunRequestCancellableAsync
                        }
                        ("Process", "run_stdout_timeout_async") => {
                            RegIntrinsic::ProcessRunStdoutTimeoutAsync
                        }
                        ("Pipeline", "filter") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Pipeline.filter expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListFilter {
                                dst,
                                list: arg_regs[0],
                                predicate: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Pipeline", "map") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Pipeline.map expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::ListMap {
                                dst,
                                list: arg_regs[0],
                                mapper: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("String", "concat") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM String.concat expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::StringConcat {
                                dst,
                                left: arg_regs[0],
                                right: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Set", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Set.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SetClear {
                                dst,
                                set: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("Set", "for_each") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Set.for_each expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SetForEach {
                                dst,
                                set: arg_regs[0],
                                callback: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Set", "insert") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Set.insert expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SetInsert {
                                dst,
                                set: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("Set", "remove") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM Set.remove expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SetRemove {
                                dst,
                                set: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("SortedSet", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedSet.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedSetClear {
                                dst,
                                set: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("SortedSet", "insert") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedSet.insert expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedSetInsert {
                                dst,
                                set: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("SortedSet", "remove") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedSet.remove expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedSetRemove {
                                dst,
                                set: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("SortedMap", "clear") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedMap.clear expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedMapClear {
                                dst,
                                map: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        ("SortedMap", "insert") => {
                            if arg_regs.len() != 3 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedMap.insert expected 3 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedMapInsert {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                                value: arg_regs[2],
                            });
                            return Ok(dst);
                        }
                        ("SortedMap", "remove") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM SortedMap.remove expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::SortedMapRemove {
                                dst,
                                map: arg_regs[0],
                                key: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        ("StringBuilder", "push") => {
                            if arg_regs.len() != 2 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM StringBuilder.push expected 2 args, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::StringBuilderPush {
                                dst,
                                builder: arg_regs[0],
                                value: arg_regs[1],
                            });
                            return Ok(dst);
                        }
                        _ => {
                            let qualified_key = format!("{namespace_root}.{name_root}");
                            // Native declarations also appear in `function_ids` (with
                            // empty bodies), so dispatch them as native boundaries
                            // first. A user-defined qualified function (e.g.
                            // `pub fn Sqlx.execute`) is never native, so it falls
                            // through to the `function_ids` lookup below.
                            if self.is_native_function(Some(namespace_root), name_root) {
                                let mut_args =
                                    self.native_mut_arg_positions(Some(namespace_root), name_root);
                                self.emit(RegInstr::CallNative {
                                    dst,
                                    key: qualified_key,
                                    args: arg_regs,
                                    mut_args,
                                });
                                return Ok(dst);
                            }
                            // Dynamic protocol dispatch: `Protocol.method(self: x, ...)`
                            // where `Protocol` is a protocol with impls. The concrete
                            // function is selected at runtime by `args[0]`'s struct type
                            // (capability objects + generic bounds) — the VM equivalent
                            // of the compiled backend's closed-world enum dispatch.
                            // Checked before the `function_ids` lookup because a protocol
                            // method also appears there as a bodyless stub (which would
                            // wrongly return `Unit`).
                            let dispatch: Vec<(String, usize)> = self
                                .hir
                                .protocol_method_targets(namespace_root, name_root)
                                .into_iter()
                                .filter_map(|(type_name, target)| {
                                    self.function_ids
                                        .get(type_root_name(&target))
                                        .copied()
                                        .map(|function| (type_name, function))
                                })
                                .collect();
                            if !dispatch.is_empty() {
                                let mut_args =
                                    self.native_mut_arg_positions(Some(namespace_root), name_root);
                                self.emit(RegInstr::CallDynamic {
                                    dst,
                                    dispatch,
                                    args: arg_regs,
                                    mut_args,
                                });
                                return Ok(dst);
                            }
                            if let Some(function) = self.function_ids.get(&qualified_key).copied() {
                                let mut_args =
                                    self.native_mut_arg_positions(Some(namespace_root), name_root);
                                self.emit(RegInstr::CallKnown {
                                    dst,
                                    function,
                                    args: arg_regs,
                                    mut_args,
                                });
                                return Ok(dst);
                            }
                            // `.clone()` (a derived `Clone`) deep-copies any value. A
                            // receiver call resolves its namespace to the concrete type
                            // (e.g. `Ops.clone`), not `Clone`, so map an otherwise
                            // unresolved `clone` to the deep-clone intrinsic.
                            if name_root == "clone" && arg_regs.len() == 1 {
                                self.emit(RegInstr::CallIntrinsic {
                                    dst,
                                    intrinsic: RegIntrinsic::CloneClone,
                                    args: arg_regs,
                                });
                                return Ok(dst);
                            }
                            return Err(EvalError::Runtime(format!(
                                "reg VM v0 does not support intrinsic `{namespace}.{name}`."
                            )));
                        }
                    }
                };
                match intrinsic {
                    RegIntrinsic::JsonDecode | RegIntrinsic::JsonDecodeText => {
                        let type_arg = type_arg_names(name)
                            .and_then(|args| args.first().copied())
                            .ok_or_else(|| {
                                EvalError::Runtime(format!(
                                    "reg VM {namespace}.{name} requires a concrete type argument."
                                ))
                            })?;
                        self.emit(RegInstr::CallTypedIntrinsic {
                            dst,
                            intrinsic,
                            type_arg: type_root_name(type_arg).to_string(),
                            args: arg_regs,
                        });
                    }
                    // `List<T>.new()` — TV1 construction metadata. Carry the static
                    // element type so an empty `List<Int>`/`List<Float>` starts in
                    // the matching flat typed kind. The type arg is optional (a bare
                    // `List.new()` with no annotation falls back to `Boxed`).
                    RegIntrinsic::ListNew => {
                        let type_arg = type_arg_names(namespace)
                            .or_else(|| type_arg_names(name))
                            .and_then(|args| args.first().copied())
                            .map(|arg| type_root_name(arg).to_string())
                            .unwrap_or_default();
                        self.emit(RegInstr::CallTypedIntrinsic {
                            dst,
                            intrinsic,
                            type_arg,
                            args: arg_regs,
                        });
                    }
                    _ => {
                        self.emit(RegInstr::CallIntrinsic {
                            dst,
                            intrinsic,
                            args: arg_regs,
                        });
                    }
                }
            }
            Callee::ReceiverCall { .. } => {
                unreachable!("receiver calls return before arg lowering")
            }
        }
        Ok(dst)
    }

    fn is_native_function(&self, namespace: Option<&str>, name: &str) -> bool {
        self.hir
            .resolve_function(namespace, type_root_name(name))
            .is_some_and(|signature| signature.is_native && !signature.is_builtin)
    }

    /// Parameter positions of a native function that are `mut`. These map to
    /// `CallNative` arg positions (the arg list is positional, with the receiver
    /// at index 0 for receiver calls), so the host can write mutated values back.
    fn native_mut_arg_positions(&self, namespace: Option<&str>, name: &str) -> Vec<usize> {
        self.hir
            .resolve_function(namespace, type_root_name(name))
            .map(|signature| {
                signature
                    .params
                    .iter()
                    .enumerate()
                    .filter(|(_, param)| param.effect == Some(ParamEffect::Mut))
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `mut` parameter positions of a user function, so a `CallKnown` can write
    /// the mutated arguments back to the caller (matching AOT's `&mut` params).
    fn user_mut_arg_positions(&self, name: &str) -> Vec<usize> {
        self.native_mut_arg_positions(None, name)
    }

    fn variant_match(&mut self, value: &HirExpr, arms: &[HirMatchArm]) -> Result<bool, EvalError> {
        if arms.is_empty()
            || !arms
                .iter()
                .all(|arm| self.is_supported_match_pattern(&arm.pattern))
        {
            return Ok(false);
        }

        let src = self.expr(value)?;
        let mut failure_patches = Vec::new();
        let mut end_jumps = Vec::new();
        for arm in arms {
            let arm_ip = self.function.code.len();
            self.patch_match_failures(failure_patches, arm_ip);
            failure_patches = self.lower_match_pattern(&arm.pattern, src)?;
            if let Some(guard) = &arm.guard {
                let guard_failure = self.condition_jump(guard, false, usize::MAX)?;
                failure_patches.push(MatchFailurePatch::Jump(guard_failure));
            }
            self.block(&arm.body)?;
            end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
        }

        let no_match_ip = self.function.code.len();
        self.patch_match_failures(failure_patches, no_match_ip);
        self.emit(RegInstr::RuntimeError {
            message: "reg VM match did not match any arm.".to_string(),
        });
        let end_ip = self.function.code.len();
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        Ok(true)
    }

    fn match_expr(&mut self, value: &HirExpr, arms: &[HirMatchArm]) -> Result<Reg, EvalError> {
        if arms.is_empty()
            || !arms
                .iter()
                .all(|arm| self.is_supported_match_pattern(&arm.pattern))
        {
            return Err(EvalError::Runtime(
                "reg VM v0 does not support this match expression pattern.".to_string(),
            ));
        }

        let src = self.expr(value)?;
        let dst = self.temp();
        let mut failure_patches = Vec::new();
        let mut end_jumps = Vec::new();
        for arm in arms {
            let arm_ip = self.function.code.len();
            self.patch_match_failures(failure_patches, arm_ip);
            failure_patches = self.lower_match_pattern(&arm.pattern, src)?;
            if let Some(guard) = &arm.guard {
                let guard_failure = self.condition_jump(guard, false, usize::MAX)?;
                failure_patches.push(MatchFailurePatch::Jump(guard_failure));
            }
            let value = self.expr_block_value(&arm.body)?;
            self.emit(RegInstr::Move { dst, src: value });
            end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
        }

        let no_match_ip = self.function.code.len();
        self.patch_match_failures(failure_patches, no_match_ip);
        self.emit(RegInstr::RuntimeError {
            message: "reg VM match expression did not match any arm.".to_string(),
        });
        let end_ip = self.function.code.len();
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        Ok(dst)
    }

    fn lower_match_pattern(
        &mut self,
        pattern: &MatchPattern,
        src: Reg,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        match pattern {
            MatchPattern::Binding { name, .. } => {
                let dst = self.local(name);
                self.emit(RegInstr::Move { dst, src });
                Ok(Vec::new())
            }
            MatchPattern::Wildcard(_) => Ok(Vec::new()),
            MatchPattern::Variant { name, binding, .. } if name == "Some" => {
                self.lower_option_some_pattern(src, binding.as_deref())
            }
            MatchPattern::Variant { name, .. } if name == "None" => {
                let match_ip = self.emit(RegInstr::MatchOption {
                    src,
                    some_ip: usize::MAX,
                    none_ip: usize::MAX,
                });
                let pass_ip = self.function.code.len();
                self.patch_match_none(match_ip, pass_ip);
                Ok(vec![MatchFailurePatch::OptionSome(match_ip)])
            }
            MatchPattern::Variant { name, binding, .. } if name == "Ok" || name == "Err" => {
                self.lower_result_variant_pattern(src, name, binding.as_deref())
            }
            MatchPattern::Variant { name, binding, .. }
                if self.hir.sum_type_for_variant(name).is_some() =>
            {
                self.lower_user_variant_pattern(src, name, binding.as_deref())
            }
            MatchPattern::Struct { name, fields, .. }
                if self.hir.sum_type_for_variant(name).is_some() =>
            {
                self.lower_user_struct_variant_pattern(src, name, fields)
            }
            MatchPattern::Struct { fields, .. } => self.lower_struct_field_patterns(src, fields),
            MatchPattern::List {
                prefix,
                rest,
                suffix,
                ..
            } => self.lower_list_pattern(src, prefix, rest, suffix),
            MatchPattern::Literal { value, .. } => self.lower_literal_pattern(src, value),
            _ => Err(EvalError::Runtime(
                "reg VM v0 does not support this match pattern.".to_string(),
            )),
        }
    }

    fn is_supported_match_pattern(&self, pattern: &MatchPattern) -> bool {
        match pattern {
            MatchPattern::Binding { .. }
            | MatchPattern::Literal { .. }
            | MatchPattern::Wildcard(_) => true,
            MatchPattern::Variant { name, binding, .. }
                if matches!(name.as_str(), "Some" | "None" | "Ok" | "Err") =>
            {
                binding
                    .as_deref()
                    .is_none_or(|binding| self.is_supported_match_pattern(binding))
            }
            MatchPattern::Variant { name, binding, .. } => {
                self.hir.sum_type_for_variant(name).is_some()
                    && binding
                        .as_deref()
                        .is_none_or(|binding| self.is_supported_match_pattern(binding))
            }
            MatchPattern::Struct { name, fields, .. } => {
                (self.hir.sum_type_for_variant(name).is_some()
                    || matches!(
                        self.hir.type_kind(name),
                        Some(HirTypeKind::Struct | HirTypeKind::Class)
                    ))
                    && fields.iter().all(|field| {
                        field.ignored
                            || field
                                .pattern
                                .as_deref()
                                .is_none_or(|pattern| self.is_supported_match_pattern(pattern))
                    })
            }
            MatchPattern::List { prefix, suffix, .. } => prefix
                .iter()
                .chain(suffix)
                .all(|pattern| self.is_supported_match_pattern(pattern)),
        }
    }

    /// Lower a plain (non-variant) struct pattern: there is no tag to test, so
    /// refutability comes only from nested field sub-patterns (e.g. literals).
    /// Each field is projected and either bound or recursively matched.
    fn lower_struct_field_patterns(
        &mut self,
        src: Reg,
        fields: &[MatchFieldPattern],
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let mut failures = Vec::new();
        for field in fields {
            if field.ignored {
                continue;
            }
            let field_reg = self.temp();
            self.emit(RegInstr::GetField {
                dst: field_reg,
                base: src,
                name: field.name.clone(),
            });
            if let Some(pattern) = field.pattern.as_deref() {
                failures.extend(self.lower_match_pattern(pattern, field_reg)?);
            } else if let Some(binding) = field.binding.as_ref() {
                let dst = self.local(binding);
                self.emit(RegInstr::Move {
                    dst,
                    src: field_reg,
                });
            } else {
                return Err(EvalError::Runtime(format!(
                    "reg VM struct pattern field `{}` has no binding or nested pattern.",
                    field.name
                )));
            }
        }
        Ok(failures)
    }

    /// Lower a `List<T>` slice pattern. Refutability is a length test (`==` for a
    /// fixed pattern, `>=` when a rest segment is present); elements are projected
    /// with `ListGet` and the rest segment (if bound) with `List.slice`.
    fn lower_list_pattern(
        &mut self,
        src: Reg,
        prefix: &[MatchPattern],
        rest: &Option<Option<String>>,
        suffix: &[MatchPattern],
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let mut failures = Vec::new();
        let required = (prefix.len() + suffix.len()) as i64;
        let len = self.temp();
        self.emit(RegInstr::ListLen {
            dst: len,
            list: src,
        });
        let bound = self.temp();
        self.emit(RegInstr::LoadInt {
            dst: bound,
            value: required,
        });
        // Fail (jump to the next arm) when the length constraint does not hold.
        // `RegIntCompare` has no `Equal`, so a fixed length is bracketed by `>=`
        // and `<=`; a rest pattern only needs the lower bound.
        let lower = self.emit(RegInstr::JumpIfIntCompare {
            lhs: len,
            rhs: bound,
            op: RegIntCompare::GreaterEqual,
            expected: false,
            target: usize::MAX,
        });
        failures.push(MatchFailurePatch::Jump(lower));
        if rest.is_none() {
            let upper = self.emit(RegInstr::JumpIfIntCompare {
                lhs: len,
                rhs: bound,
                op: RegIntCompare::LessEqual,
                expected: false,
                target: usize::MAX,
            });
            failures.push(MatchFailurePatch::Jump(upper));
        }
        for (index, pattern) in prefix.iter().enumerate() {
            let idx = self.temp();
            self.emit(RegInstr::LoadInt {
                dst: idx,
                value: index as i64,
            });
            let elem = self.temp();
            self.emit(RegInstr::ListGet {
                dst: elem,
                list: src,
                index: idx,
            });
            failures.extend(self.lower_match_pattern(pattern, elem)?);
        }
        if let Some(Some(rest_name)) = rest {
            let start = self.temp();
            self.emit(RegInstr::LoadInt {
                dst: start,
                value: prefix.len() as i64,
            });
            let slice_len = self.temp();
            self.emit(RegInstr::SubInt {
                dst: slice_len,
                lhs: len,
                rhs: bound,
            });
            let dst = self.local(rest_name);
            self.emit(RegInstr::CallIntrinsic {
                dst,
                intrinsic: RegIntrinsic::ListSlice,
                args: vec![src, start, slice_len],
            });
        }
        if !suffix.is_empty() {
            let suffix_count = self.temp();
            self.emit(RegInstr::LoadInt {
                dst: suffix_count,
                value: suffix.len() as i64,
            });
            let base = self.temp();
            self.emit(RegInstr::SubInt {
                dst: base,
                lhs: len,
                rhs: suffix_count,
            });
            for (offset, pattern) in suffix.iter().enumerate() {
                let offset_reg = self.temp();
                self.emit(RegInstr::LoadInt {
                    dst: offset_reg,
                    value: offset as i64,
                });
                let idx = self.temp();
                self.emit(RegInstr::AddInt {
                    dst: idx,
                    lhs: base,
                    rhs: offset_reg,
                });
                let elem = self.temp();
                self.emit(RegInstr::ListGet {
                    dst: elem,
                    list: src,
                    index: idx,
                });
                failures.extend(self.lower_match_pattern(pattern, elem)?);
            }
        }
        Ok(failures)
    }

    fn lower_literal_pattern(
        &mut self,
        src: Reg,
        literal: &MatchLiteral,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let expected = self.temp();
        match literal {
            MatchLiteral::Int(value) => {
                let value = value.parse::<i64>().map_err(|error| {
                    EvalError::Runtime(format!(
                        "reg VM could not parse match int literal `{value}`: {error}"
                    ))
                })?;
                self.emit(RegInstr::LoadInt {
                    dst: expected,
                    value,
                });
            }
            MatchLiteral::String(value) => {
                self.emit(RegInstr::LoadString {
                    dst: expected,
                    value: Rc::new(decode_string_token(value)),
                });
            }
            MatchLiteral::Bool(value) => {
                self.emit(RegInstr::LoadBool {
                    dst: expected,
                    value: *value,
                });
            }
        }
        let matches = self.temp();
        self.emit(RegInstr::Equal {
            dst: matches,
            lhs: src,
            rhs: expected,
        });
        let failure = self.emit(RegInstr::JumpIfBool {
            cond: matches,
            expected: false,
            target: usize::MAX,
        });
        Ok(vec![MatchFailurePatch::Jump(failure)])
    }

    fn lower_option_some_pattern(
        &mut self,
        src: Reg,
        binding: Option<&MatchPattern>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let match_ip = self.emit(RegInstr::MatchOption {
            src,
            some_ip: usize::MAX,
            none_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::OptionNone(match_ip)];
        if let Some(binding) = binding {
            match binding {
                MatchPattern::Binding { name, .. } => {
                    let dst = self.local(name);
                    self.emit(RegInstr::UnwrapSome { dst, src });
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapSome { dst: payload, src });
                    failures.extend(self.lower_match_pattern(binding, payload)?);
                }
            }
        }
        Ok(failures)
    }

    fn lower_result_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        binding: Option<&MatchPattern>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let match_ip = self.emit(RegInstr::MatchResult {
            src,
            ok_ip: usize::MAX,
            err_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        let mut failures = match variant {
            "Ok" => {
                self.patch_jump(match_ip, pass_ip);
                vec![MatchFailurePatch::ResultErr(match_ip)]
            }
            "Err" => {
                self.patch_result_match_err(match_ip, pass_ip);
                vec![MatchFailurePatch::ResultOk(match_ip)]
            }
            _ => unreachable!("result variant was validated before lowering"),
        };
        if let Some(binding) = binding {
            match binding {
                MatchPattern::Binding { name, .. } => {
                    let dst = self.local(name);
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst,
                        src,
                        expected: variant.to_string(),
                    });
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst: payload,
                        src,
                        expected: variant.to_string(),
                    });
                    failures.extend(self.lower_match_pattern(binding, payload)?);
                }
            }
        }
        Ok(failures)
    }

    fn lower_user_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        binding: Option<&MatchPattern>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let fields = self.hir.sum_variant_fields(variant).unwrap_or(&[]);
        let match_ip = self.emit(RegInstr::MatchVariant {
            src,
            expected: variant.to_string(),
            match_ip: usize::MAX,
            else_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::VariantOther(match_ip)];
        if let Some(binding) = binding {
            if fields.len() != 1 {
                return Err(EvalError::Runtime(format!(
                    "reg VM variant `{variant}` binding requires exactly one field, got {}.",
                    fields.len()
                )));
            }
            match binding {
                MatchPattern::Binding { name, .. } => {
                    let dst = self.local(name);
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst,
                        src,
                        expected: variant.to_string(),
                    });
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst: payload,
                        src,
                        expected: variant.to_string(),
                    });
                    failures.extend(self.lower_match_pattern(binding, payload)?);
                }
            }
        } else if !fields.is_empty() {
            return Err(EvalError::Runtime(format!(
                "reg VM variant `{variant}` pattern requires a payload binding."
            )));
        }
        Ok(failures)
    }

    fn lower_user_struct_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        fields: &[MatchFieldPattern],
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let match_ip = self.emit(RegInstr::MatchVariant {
            src,
            expected: variant.to_string(),
            match_ip: usize::MAX,
            else_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::VariantOther(match_ip)];
        for field in fields {
            if field.ignored {
                continue;
            }
            let field_reg = self.temp();
            self.emit(RegInstr::GetField {
                dst: field_reg,
                base: src,
                name: field.name.clone(),
            });
            if let Some(pattern) = field.pattern.as_deref() {
                failures.extend(self.lower_match_pattern(pattern, field_reg)?);
            } else if let Some(binding) = field.binding.as_ref() {
                let dst = self.local(binding);
                self.emit(RegInstr::Move {
                    dst,
                    src: field_reg,
                });
            } else {
                return Err(EvalError::Runtime(format!(
                    "reg VM struct variant pattern field `{}` has no binding or nested pattern.",
                    field.name
                )));
            }
        }
        Ok(failures)
    }

    fn patch_match_failures(&mut self, patches: Vec<MatchFailurePatch>, target: usize) {
        for patch in patches {
            match patch {
                MatchFailurePatch::Jump(ip) => self.patch_jump(ip, target),
                MatchFailurePatch::OptionSome(ip) | MatchFailurePatch::ResultOk(ip) => {
                    self.patch_jump(ip, target)
                }
                MatchFailurePatch::OptionNone(ip) => self.patch_match_none(ip, target),
                MatchFailurePatch::ResultErr(ip) => self.patch_result_match_err(ip, target),
                MatchFailurePatch::VariantOther(ip) => self.patch_variant_match_else(ip, target),
            }
        }
    }

    fn map_get_match(
        &mut self,
        value: &HirExpr,
        arms: &[crate::hir::HirMatchArm],
    ) -> Result<bool, EvalError> {
        let HirExpr::Call {
            callee: Callee::Qualified { namespace, name },
            args,
            receiver: None,
            ..
        } = value
        else {
            return Ok(false);
        };
        let collection = type_root_name(namespace);
        if !matches!(collection, "Map" | "SortedMap")
            || type_root_name(name) != "get"
            || args.len() != 2
        {
            return Ok(false);
        }
        if arms.len() != 2 {
            return Ok(false);
        }
        if arms.iter().any(|arm| arm.guard.is_some()) {
            return Ok(false);
        }

        let mut some_binding = None;
        let mut has_none = false;
        for arm in arms {
            match &arm.pattern {
                MatchPattern::Variant {
                    name,
                    binding: Some(binding),
                    ..
                } if name == "Some" => {
                    some_binding = binding.binding_names().into_iter().next();
                }
                MatchPattern::Variant { name, .. } if name == "None" => {
                    has_none = true;
                }
                _ => return Ok(false),
            }
        }
        let Some(some_binding) = some_binding else {
            return Ok(false);
        };
        if !has_none {
            return Ok(false);
        }

        let map = self.expr(&args[0].value)?;
        let key = self.expr(&args[1].value)?;
        let value_dst = self.local(&some_binding);
        let match_ip = if collection == "SortedMap" {
            self.emit(RegInstr::MatchSortedMapGet {
                map,
                key,
                value_dst,
                some_ip: usize::MAX,
                none_ip: usize::MAX,
            })
        } else {
            self.emit(RegInstr::MatchMapGet {
                map,
                key,
                value_dst,
                some_ip: usize::MAX,
                none_ip: usize::MAX,
            })
        };
        let mut some_ip = None;
        let mut none_ip = None;
        let mut end_jumps = Vec::new();
        for arm in arms {
            match &arm.pattern {
                MatchPattern::Variant { name, .. } if name == "Some" => {
                    let ip = self.function.code.len();
                    some_ip = Some(ip);
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
                }
                MatchPattern::Variant { name, .. } if name == "None" => {
                    let ip = self.function.code.len();
                    none_ip = Some(ip);
                    self.block(&arm.body)?;
                    end_jumps.push(self.emit(RegInstr::Jump { target: usize::MAX }));
                }
                _ => unreachable!("map get match arms were validated before lowering"),
            }
        }
        let end_ip = self.function.code.len();
        self.patch_map_match_some(
            match_ip,
            some_ip.ok_or_else(|| {
                EvalError::Runtime(format!(
                    "reg VM {collection}.get match is missing Some arm."
                ))
            })?,
        );
        self.patch_map_match_none(
            match_ip,
            none_ip.ok_or_else(|| {
                EvalError::Runtime(format!(
                    "reg VM {collection}.get match is missing None arm."
                ))
            })?,
        );
        for jump in end_jumps {
            self.patch_jump(jump, end_ip);
        }
        Ok(true)
    }

    fn struct_match(
        &mut self,
        value: &HirExpr,
        arms: &[crate::hir::HirMatchArm],
    ) -> Result<bool, EvalError> {
        let [arm] = arms else {
            return Ok(false);
        };
        if arm.guard.is_some() {
            return Ok(false);
        }
        let MatchPattern::Struct { fields, .. } = &arm.pattern else {
            return Ok(false);
        };

        let src = self.expr(value)?;
        for field in fields {
            if field.ignored {
                continue;
            }
            let Some(binding) = field.binding.as_ref() else {
                return Ok(false);
            };
            if field.pattern.is_some() {
                return Ok(false);
            }
            let dst = self.local(binding);
            self.emit(RegInstr::GetField {
                dst,
                base: src,
                name: field.name.clone(),
            });
        }
        self.block(&arm.body)?;
        Ok(true)
    }
}

/// Pure name->intrinsic mapping for qualified/receiver calls. Returns the
/// stdlib `RegIntrinsic` for the simple `Ns.method` mappings, or `None` for
/// names that need inline lowering logic or fall through to native/dynamic
/// dispatch (handled by the caller's remaining match arms).
fn qualified_intrinsic(namespace: &str, name: &str) -> Option<RegIntrinsic> {
    match (namespace, name) {
        ("Args", "all") => Some(RegIntrinsic::ArgsAll),
        ("Args", "count") => Some(RegIntrinsic::ArgsCount),
        ("Args", "get") => Some(RegIntrinsic::ArgsGet),
        ("Args", "get_or_default") => Some(RegIntrinsic::ArgsGetOrDefault),
        ("Assert", "equal") => Some(RegIntrinsic::AssertEqual),
        ("Assert", "equal_bool") => Some(RegIntrinsic::AssertEqualBool),
        ("Assert", "equal_int") => Some(RegIntrinsic::AssertEqualInt),
        ("Base64", "decode") => Some(RegIntrinsic::Base64Decode),
        ("Base64", "decode_string") => Some(RegIntrinsic::Base64DecodeString),
        ("Base64", "encode") => Some(RegIntrinsic::Base64Encode),
        ("Base64", "encode_bytes") => Some(RegIntrinsic::Base64EncodeBytes),
        ("Bytes", "concat") => Some(RegIntrinsic::BytesConcat),
        ("Bytes", "consume") => Some(RegIntrinsic::BytesConsume),
        ("Bytes", "from_buffer") => Some(RegIntrinsic::BytesViewToBytes),
        ("Bytes", "from_string") => Some(RegIntrinsic::BytesFromString),
        ("Bytes", "from_uints") => Some(RegIntrinsic::BytesFromUints),
        ("Bytes", "is_empty") => Some(RegIntrinsic::BytesIsEmpty),
        ("Bytes", "len") => Some(RegIntrinsic::BytesLen),
        ("Bytes", "slice") | ("Bytes", "view") => Some(RegIntrinsic::BytesSlice),
        ("Bytes", "to_string") => Some(RegIntrinsic::BytesToString),
        ("Bytes", "to_uints") => Some(RegIntrinsic::BytesToUints),
        ("Buffer", "consume") => Some(RegIntrinsic::BytesConsume),
        ("Buffer", "is_empty") => Some(RegIntrinsic::BytesIsEmpty),
        ("Buffer", "len") => Some(RegIntrinsic::BytesLen),
        ("Buffer", "new") => Some(RegIntrinsic::BufferNew),
        ("Buffer", "view") => Some(RegIntrinsic::BytesSlice),
        ("BufferView", "is_empty") => Some(RegIntrinsic::BytesIsEmpty),
        ("BufferView", "len") => Some(RegIntrinsic::BytesLen),
        ("BufferView", "slice") => Some(RegIntrinsic::BytesSlice),
        ("BufferView", "to_bytes") => Some(RegIntrinsic::BytesViewToBytes),
        ("BytesView", "is_empty") => Some(RegIntrinsic::BytesIsEmpty),
        ("BytesView", "len") => Some(RegIntrinsic::BytesLen),
        ("BytesView", "slice") => Some(RegIntrinsic::BytesSlice),
        ("BytesView", "starts_with") => Some(RegIntrinsic::BytesViewStartsWith),
        ("BytesView", "to_bytes") => Some(RegIntrinsic::BytesViewToBytes),
        ("Cache", "get") => Some(RegIntrinsic::CacheGet),
        ("Cache", "lookup") => Some(RegIntrinsic::CacheLookup),
        ("Cache", "new") => Some(RegIntrinsic::MapNew),
        ("CancellationSource", "cancel") => Some(RegIntrinsic::CancellationSourceCancel),
        ("CancellationSource", "new") => Some(RegIntrinsic::CancellationSourceNew),
        ("CancellationSource", "token") => Some(RegIntrinsic::CancellationSourceToken),
        ("Channel", "bounded") => Some(RegIntrinsic::ChannelBounded),
        // A message channel reuses the bounded-channel runtime; the
        // cross-isolate payload contract is enforced at check time.
        ("Channel", "message") => Some(RegIntrinsic::ChannelBounded),
        ("Channel", "receiver") => Some(RegIntrinsic::ChannelReceiver),
        ("Channel", "sender") => Some(RegIntrinsic::ChannelSender),
        ("ChannelError", "message") => Some(RegIntrinsic::ChannelErrorMessage),
        ("Tensor", "from_f32_slice") => Some(RegIntrinsic::TensorFromF32Slice),
        ("Tensor", "to_f32_slice") => Some(RegIntrinsic::TensorToF32Slice),
        ("Tensor", "shape") => Some(RegIntrinsic::TensorShape),
        ("Tensor", "rank") => Some(RegIntrinsic::TensorRank),
        ("Tensor", "f32_to_le_bytes") => Some(RegIntrinsic::TensorF32ToLeBytes),
        ("Tensor", "f32_from_le_bytes") => Some(RegIntrinsic::TensorF32FromLeBytes),
        ("Tensor", "matmul") => Some(RegIntrinsic::TensorMatmul),
        ("Tensor", "matmul_metal") => Some(RegIntrinsic::TensorMatmulMetal),
        ("Tensor", "metal_available") => Some(RegIntrinsic::TensorMetalAvailable),
        ("Tensor", "metal_device_name") => Some(RegIntrinsic::TensorMetalDeviceName),
        ("Tensor", "gpu_run_msl") => Some(RegIntrinsic::TensorGpuRunMsl),
        // Metal.* aliases (non-colliding namespace; reuse the same exec arms).
        ("Metal", "available") => Some(RegIntrinsic::TensorMetalAvailable),
        ("Metal", "device_name") => Some(RegIntrinsic::TensorMetalDeviceName),
        ("Metal", "gpu_run_msl") => Some(RegIntrinsic::TensorGpuRunMsl),
        ("Tensor", "add") => Some(RegIntrinsic::TensorAdd),
        ("Tensor", "sub") => Some(RegIntrinsic::TensorSub),
        ("Tensor", "mul") => Some(RegIntrinsic::TensorMul),
        ("Tensor", "div") => Some(RegIntrinsic::TensorDiv),
        ("Tensor", "neg") => Some(RegIntrinsic::TensorNeg),
        ("Tensor", "exp") => Some(RegIntrinsic::TensorExp),
        ("Tensor", "log") => Some(RegIntrinsic::TensorLog),
        ("Tensor", "sqrt") => Some(RegIntrinsic::TensorSqrt),
        ("Tensor", "relu") => Some(RegIntrinsic::TensorRelu),
        ("Tensor", "sum_all") => Some(RegIntrinsic::TensorSumAll),
        ("Tensor", "sum_axis") => Some(RegIntrinsic::TensorSumAxis),
        ("Tensor", "max_axis") => Some(RegIntrinsic::TensorMaxAxis),
        ("Tensor", "mean_axis") => Some(RegIntrinsic::TensorMeanAxis),
        ("Tensor", "argmax_axis") => Some(RegIntrinsic::TensorArgmaxAxis),
        ("Tensor", "reshape") => Some(RegIntrinsic::TensorReshape),
        ("Tensor", "transpose") => Some(RegIntrinsic::TensorTranspose),
        ("Tensor", "permute") => Some(RegIntrinsic::TensorPermute),
        ("Tensor", "broadcast_to") => Some(RegIntrinsic::TensorBroadcastTo),
        ("Tensor", "cmplt") => Some(RegIntrinsic::TensorCmplt),
        ("Tensor", "cmpne") => Some(RegIntrinsic::TensorCmpne),
        ("Tensor", "cmpeq") => Some(RegIntrinsic::TensorCmpeq),
        ("Tensor", "select") => Some(RegIntrinsic::TensorSelect),
        ("Tensor", "maximum") => Some(RegIntrinsic::TensorMaximum),
        ("Tensor", "minimum") => Some(RegIntrinsic::TensorMinimum),
        ("Tensor", "cast_f32") => Some(RegIntrinsic::TensorCastF32),
        ("Tensor", "cast_i32") => Some(RegIntrinsic::TensorCastI32),
        ("Tensor", "cast_bool") => Some(RegIntrinsic::TensorCastBool),
        ("Tensor", "dtype_code") => Some(RegIntrinsic::TensorDtypeCode),
        // movement+gather (ops B)
        ("Tensor", "pad") => Some(RegIntrinsic::TensorPad),
        ("Tensor", "shrink") => Some(RegIntrinsic::TensorShrink),
        ("Tensor", "flip") => Some(RegIntrinsic::TensorFlip),
        ("Tensor", "gather") => Some(RegIntrinsic::TensorGather),
        // reductions+math (ops C)
        ("Tensor", "prod_axis") => Some(RegIntrinsic::TensorProdAxis),
        ("Tensor", "min_axis") => Some(RegIntrinsic::TensorMinAxis),
        ("Tensor", "sum_axes") => Some(RegIntrinsic::TensorSumAxes),
        ("Tensor", "prod_axes") => Some(RegIntrinsic::TensorProdAxes),
        ("Tensor", "max_axes") => Some(RegIntrinsic::TensorMaxAxes),
        ("Tensor", "min_axes") => Some(RegIntrinsic::TensorMinAxes),
        ("Tensor", "mean_axes") => Some(RegIntrinsic::TensorMeanAxes),
        ("Tensor", "reciprocal") => Some(RegIntrinsic::TensorReciprocal),
        ("Tensor", "exp2") => Some(RegIntrinsic::TensorExp2),
        ("Tensor", "log2") => Some(RegIntrinsic::TensorLog2),
        ("Tensor", "rsqrt") => Some(RegIntrinsic::TensorRsqrt),
        ("Tensor", "sin") => Some(RegIntrinsic::TensorSin),
        ("Tensor", "trunc") => Some(RegIntrinsic::TensorTrunc),
        ("Tensor", "pow") => Some(RegIntrinsic::TensorPow),
        // bmm+int/bit (ops D)
        ("Tensor", "bmm") => Some(RegIntrinsic::TensorBmm),
        ("Tensor", "idiv") => Some(RegIntrinsic::TensorIdiv),
        ("Tensor", "modulo") => Some(RegIntrinsic::TensorMod),
        ("Tensor", "floordiv") => Some(RegIntrinsic::TensorFloordiv),
        ("Tensor", "floormod") => Some(RegIntrinsic::TensorFloormod),
        ("Tensor", "shl") => Some(RegIntrinsic::TensorShl),
        ("Tensor", "shr") => Some(RegIntrinsic::TensorShr),
        ("Tensor", "bit_and") => Some(RegIntrinsic::TensorAnd),
        ("Tensor", "bit_or") => Some(RegIntrinsic::TensorOr),
        ("Tensor", "bit_xor") => Some(RegIntrinsic::TensorXor),
        ("Tensor", "bitcast_f32_to_i32") => Some(RegIntrinsic::TensorBitcastF32ToI32),
        ("Tensor", "bitcast_i32_to_f32") => Some(RegIntrinsic::TensorBitcastI32ToF32),
        // rng (slice E)
        ("Tensor", "rand") => Some(RegIntrinsic::TensorRand),
        ("Tensor", "randint") => Some(RegIntrinsic::TensorRandint),
        ("Tensor", "randn") => Some(RegIntrinsic::TensorRandn),
        // nn (slice F)
        ("Tensor", "iota") => Some(RegIntrinsic::TensorIota),
        ("Tensor", "one_hot") => Some(RegIntrinsic::TensorOneHot),
        ("Tensor", "softmax") => Some(RegIntrinsic::TensorSoftmax),
        ("Tensor", "log_softmax") => Some(RegIntrinsic::TensorLogSoftmax),
        ("Tensor", "cross_entropy") => Some(RegIntrinsic::TensorCrossEntropy),
        // conv (slice G)
        ("Tensor", "conv2d") => Some(RegIntrinsic::TensorConv2d),
        ("Tensor", "max_pool2d") => Some(RegIntrinsic::TensorMaxPool2d),
        ("Tensor", "avg_pool2d") => Some(RegIntrinsic::TensorAvgPool2d),
        // scatter
        ("Tensor", "scatter_add") => Some(RegIntrinsic::TensorScatterAdd),
        ("TensorError", "message") => Some(RegIntrinsic::TensorErrorMessage),
        ("Char", "compare") => Some(RegIntrinsic::CharCompare),
        ("Char", "from_code") => Some(RegIntrinsic::CharFromCode),
        ("Char", "is_alphanumeric") => Some(RegIntrinsic::CharIsAlphanumeric),
        ("Char", "is_alpha") => Some(RegIntrinsic::CharIsAlpha),
        ("Char", "is_digit") => Some(RegIntrinsic::CharIsDigit),
        ("Char", "is_lower") => Some(RegIntrinsic::CharIsLower),
        ("Char", "is_upper") => Some(RegIntrinsic::CharIsUpper),
        ("Char", "is_whitespace") => Some(RegIntrinsic::CharIsWhitespace),
        ("Char", "to_code") => Some(RegIntrinsic::CharToCode),
        ("Char", "to_lower") => Some(RegIntrinsic::CharToLower),
        ("Char", "to_string") => Some(RegIntrinsic::CharToString),
        ("Char", "to_upper") => Some(RegIntrinsic::CharToUpper),
        ("Clock", "now") => Some(RegIntrinsic::ClockNow),
        ("Clock", "system_unix_ms") => Some(RegIntrinsic::ClockSystemUnixMs),
        ("Config", "load") => Some(RegIntrinsic::ConfigLoad),
        ("Capability", "from") => Some(RegIntrinsic::CapabilityFrom),
        ("Config", "name") => Some(RegIntrinsic::ConfigName),
        ("Config", "new") => Some(RegIntrinsic::ConfigNew),
        ("Config", "rule_count") => Some(RegIntrinsic::ConfigRuleCount),
        ("ConfigStore", "name") => Some(RegIntrinsic::ConfigStoreName),
        ("ConfigStore", "new") => Some(RegIntrinsic::ConfigStoreNew),
        ("Counter", "new") => Some(RegIntrinsic::CounterNew),
        ("Counter", "value") => Some(RegIntrinsic::CounterValue),
        ("Csv", "open_read") => Some(RegIntrinsic::CsvOpenRead),
        ("Csv", "parse_row") => Some(RegIntrinsic::CsvParseRow),
        ("Csv", "read_into") => Some(RegIntrinsic::CsvReadInto),
        ("Csv", "rows") => Some(RegIntrinsic::CsvRows),
        ("Deadline", "after") => Some(RegIntrinsic::DeadlineAfter),
        ("Deadline", "after_ms") => Some(RegIntrinsic::DeadlineAfterMs),
        ("Deadline", "is_expired") => Some(RegIntrinsic::DeadlineIsExpired),
        ("Deadline", "remaining_ms") => Some(RegIntrinsic::DeadlineRemainingMs),
        ("DecodeError", "message") => Some(RegIntrinsic::DecodeErrorMessage),
        ("Deque", "is_empty") => Some(RegIntrinsic::DequeIsEmpty),
        ("Deque", "len") => Some(RegIntrinsic::DequeLen),
        ("Deque", "new") => Some(RegIntrinsic::DequeNew),
        ("Deque", "to_list") => Some(RegIntrinsic::DequeToList),
        ("Diff", "unified") => Some(RegIntrinsic::DiffUnified),
        ("Directory", "copy_file") => Some(RegIntrinsic::DirectoryCopyFile),
        ("Directory", "create") => Some(RegIntrinsic::DirectoryCreate),
        ("Directory", "create_all") => Some(RegIntrinsic::DirectoryCreateAll),
        ("Directory", "create_dir_all") => Some(RegIntrinsic::DirectoryCreateDirAll),
        ("Directory", "exists") => Some(RegIntrinsic::DirectoryExists),
        ("Directory", "is_dir") => Some(RegIntrinsic::DirectoryIsDir),
        ("Directory", "is_file") => Some(RegIntrinsic::DirectoryIsFile),
        ("Directory", "list_files") => Some(RegIntrinsic::DirectoryListFiles),
        ("Directory", "list_paths") => Some(RegIntrinsic::DirectoryListPaths),
        ("Directory", "metadata") => Some(RegIntrinsic::DirectoryMetadata),
        ("Directory", "read_string") => Some(RegIntrinsic::DirectoryReadString),
        ("Directory", "remove_dir_all") => Some(RegIntrinsic::DirectoryRemoveDirAll),
        ("Directory", "remove_file") => Some(RegIntrinsic::DirectoryRemoveFile),
        ("Directory", "rename") => Some(RegIntrinsic::DirectoryRename),
        ("Directory", "write_string") => Some(RegIntrinsic::DirectoryWriteString),
        ("Db", "close") => Some(RegIntrinsic::DbClose),
        ("DbConnection", "open") => Some(RegIntrinsic::DbConnectionOpen),
        ("DbConnection", "query") => Some(RegIntrinsic::DbConnectionQuery),
        ("DbConnection", "try_open") => Some(RegIntrinsic::DbConnectionTryOpen),
        ("Date", "add_days") => Some(RegIntrinsic::DateAddDays),
        ("Date", "add_ms") => Some(RegIntrinsic::DateAddMs),
        ("Date", "day") => Some(RegIntrinsic::DateDay),
        ("Date", "days_between") => Some(RegIntrinsic::DateDaysBetween),
        ("Date", "days_in_month") => Some(RegIntrinsic::DateDaysInMonth),
        ("Date", "format_iso") => Some(RegIntrinsic::DateFormatIso),
        ("Date", "format_ymd") => Some(RegIntrinsic::DateFormatYmd),
        ("Date", "hour") => Some(RegIntrinsic::DateHour),
        ("Date", "is_leap_year") => Some(RegIntrinsic::DateIsLeapYear),
        ("Date", "minute") => Some(RegIntrinsic::DateMinute),
        ("Date", "month") => Some(RegIntrinsic::DateMonth),
        ("Date", "parse_iso") => Some(RegIntrinsic::DateParseIso),
        ("Date", "parse_ymd") => Some(RegIntrinsic::DateParseYmd),
        ("Date", "second") => Some(RegIntrinsic::DateSecond),
        ("Date", "start_of_day") => Some(RegIntrinsic::DateStartOfDay),
        ("Date", "weekday") => Some(RegIntrinsic::DateWeekday),
        ("Date", "year") => Some(RegIntrinsic::DateYear),
        ("Duration", "add") => Some(RegIntrinsic::DurationAdd),
        ("Duration", "as_ms") => Some(RegIntrinsic::DurationAsMs),
        ("Duration", "as_seconds") => Some(RegIntrinsic::DurationAsSeconds),
        ("Duration", "ms") => Some(RegIntrinsic::DurationMs),
        ("Duration", "seconds") => Some(RegIntrinsic::DurationSeconds),
        ("Environment", "bind_function") => Some(RegIntrinsic::EnvironmentBindFunction),
        ("Environment", "child") => Some(RegIntrinsic::EnvironmentChild),
        ("Environment", "has_function") => Some(RegIntrinsic::EnvironmentHasFunction),
        ("Environment", "has_parent") => Some(RegIntrinsic::EnvironmentHasParent),
        ("Environment", "root") => Some(RegIntrinsic::EnvironmentRoot),
        ("Env", "current_dir") => Some(RegIntrinsic::EnvCurrentDir),
        ("Env", "get") => Some(RegIntrinsic::EnvGet),
        ("Env", "get_or_default") => Some(RegIntrinsic::EnvGetOrDefault),
        ("Env", "home_dir") => Some(RegIntrinsic::EnvHomeDir),
        ("Env", "run_workspace_root") => Some(RegIntrinsic::EnvRunWorkspaceRoot),
        ("Env", "set") => Some(RegIntrinsic::EnvSet),
        ("Env", "set_current_dir") => Some(RegIntrinsic::EnvSetCurrentDir),
        ("Env", "temp_dir") => Some(RegIntrinsic::EnvTempDir),
        ("File", "append_bytes") => Some(RegIntrinsic::FileAppendBytes),
        ("File", "append_string") => Some(RegIntrinsic::FileAppendString),
        ("File", "bytes_stream") => Some(RegIntrinsic::FileBytesStream),
        ("File", "exists") => Some(RegIntrinsic::FileExists),
        ("File", "open") => Some(RegIntrinsic::FileOpen),
        ("File", "open_read") => Some(RegIntrinsic::FileOpenRead),
        ("File", "open_write") => Some(RegIntrinsic::FileOpenWrite),
        ("File", "read_all") => Some(RegIntrinsic::FileReadAll),
        ("File", "read_all_async") => Some(RegIntrinsic::FileReadAllAsync),
        ("File", "read_all_string") => Some(RegIntrinsic::FileReadAllString),
        ("File", "read_all_string_async") => Some(RegIntrinsic::FileReadAllStringAsync),
        ("File", "read_bytes") => Some(RegIntrinsic::FileReadBytes),
        ("File", "read_into") => Some(RegIntrinsic::FileReadInto),
        ("File", "read_string") => Some(RegIntrinsic::FileReadString),
        ("File", "remove") => Some(RegIntrinsic::FileRemove),
        ("File", "write") => Some(RegIntrinsic::FileWrite),
        ("File", "write_async") => Some(RegIntrinsic::FileWriteAsync),
        ("File", "write_atomic") => Some(RegIntrinsic::FileWriteAtomic),
        ("File", "write_bytes") => Some(RegIntrinsic::FileWriteBytes),
        ("File", "write_bytes_view") => Some(RegIntrinsic::FileWriteBytesView),
        ("File", "write_buffer") => Some(RegIntrinsic::FileWriteBuffer),
        ("File", "write_buffer_view") => Some(RegIntrinsic::FileWriteBufferView),
        ("File", "write_string") => Some(RegIntrinsic::FileWriteString),
        ("File", "write_string_async") => Some(RegIntrinsic::FileWriteStringAsync),
        ("File", "write_string_to_path") => Some(RegIntrinsic::FileWriteStringToPath),
        ("FalliblePipeline", "collect") => Some(RegIntrinsic::FalliblePipelineCollect),
        ("FalliblePipeline", "each") => Some(RegIntrinsic::FalliblePipelineEach),
        ("FalliblePipeline", "filter") => Some(RegIntrinsic::FalliblePipelineFilter),
        ("FalliblePipeline", "map") => Some(RegIntrinsic::FalliblePipelineMap),
        ("FalliblePipeline", "try_map") => Some(RegIntrinsic::FalliblePipelineTryMap),
        ("FileError", "message") => Some(RegIntrinsic::FileErrorMessage),
        ("FunctionObject", "has_closure") => Some(RegIntrinsic::FunctionObjectHasClosure),
        ("FunctionObject", "new") => Some(RegIntrinsic::FunctionObjectNew),
        ("Hash", "sha256_bytes") => Some(RegIntrinsic::HashSha256Bytes),
        ("Hash", "sha256_file") => Some(RegIntrinsic::HashSha256File),
        ("Hash", "sha256_string") => Some(RegIntrinsic::HashSha256String),
        ("Hash", "sha3_224_bytes") => Some(RegIntrinsic::HashSha3_224Bytes),
        ("Hash", "sha3_256_bytes") => Some(RegIntrinsic::HashSha3_256Bytes),
        ("Hash", "shake128_bytes") => Some(RegIntrinsic::HashShake128Bytes),
        ("Hmac", "sha256_bytes") => Some(RegIntrinsic::HmacSha256Bytes),
        ("Hmac", "sha256_string") => Some(RegIntrinsic::HmacSha256String),
        ("GlobalConfig", "new") => Some(RegIntrinsic::GlobalConfigNew),
        ("GlobalConfig", "rule_count") => Some(RegIntrinsic::GlobalConfigRuleCount),
        ("Gzip", "decompress_bytes") => Some(RegIntrinsic::GzipDecompressBytes),
        ("Hex", "decode") => Some(RegIntrinsic::HexDecode),
        ("Hex", "encode") => Some(RegIntrinsic::HexEncode),
        ("Hex", "encode_string") => Some(RegIntrinsic::HexEncodeString),
        ("HttpError", "message") => Some(RegIntrinsic::HttpErrorMessage),
        ("Http", "get") => Some(RegIntrinsic::HttpGet),
        ("Http", "get_async") => Some(RegIntrinsic::HttpGetAsync),
        ("Http", "get_retry_async") => Some(RegIntrinsic::HttpGetRetryAsync),
        ("Http", "get_timeout_async") => Some(RegIntrinsic::HttpGetTimeoutAsync),
        ("Http", "post_form") => Some(RegIntrinsic::HttpPostForm),
        ("Http", "post_form_async") => Some(RegIntrinsic::HttpPostFormAsync),
        ("Http", "post_json") => Some(RegIntrinsic::HttpPostJson),
        ("Http", "post_json_async") => Some(RegIntrinsic::HttpPostJsonAsync),
        ("Http", "post_json_retry_async") => Some(RegIntrinsic::HttpPostJsonRetryAsync),
        ("Http", "post_json_timeout_async") => Some(RegIntrinsic::HttpPostJsonTimeoutAsync),
        ("Http", "send_async") => Some(RegIntrinsic::HttpSendAsync),
        ("HttpRequest", "json") => Some(RegIntrinsic::HttpRequestJson),
        ("HttpRequest", "with_header") => Some(RegIntrinsic::HttpRequestWithHeader),
        ("HttpRequest", "with_retry") => Some(RegIntrinsic::HttpRequestWithRetry),
        ("HttpRequest", "with_timeout") => Some(RegIntrinsic::HttpRequestWithTimeout),
        ("HttpResponse", "bytes") => Some(RegIntrinsic::HttpResponseBytes),
        ("HttpResponse", "is_success") => Some(RegIntrinsic::HttpResponseIsSuccess),
        ("HttpResponse", "lines") => Some(RegIntrinsic::HttpResponseLines),
        ("HttpResponse", "status") => Some(RegIntrinsic::HttpResponseStatus),
        ("HttpResponse", "text") => Some(RegIntrinsic::HttpResponseText),
        ("Image", "inspect") => Some(RegIntrinsic::ImageInspect),
        ("Image", "load") => Some(RegIntrinsic::ImageLoad),
        ("Image", "normalize") => Some(RegIntrinsic::ImageNormalize),
        ("Image", "resize") => Some(RegIntrinsic::ImageResize),
        ("Image", "save") => Some(RegIntrinsic::ImageSave),
        ("Image", "sharpen") => Some(RegIntrinsic::ImageSharpen),
        ("Instant", "elapsed") => Some(RegIntrinsic::InstantElapsed),
        ("Float", "is_finite") => Some(RegIntrinsic::FloatIsFinite),
        ("Float", "is_infinite") => Some(RegIntrinsic::FloatIsInfinite),
        ("Float", "is_nan") => Some(RegIntrinsic::FloatIsNan),
        ("Float", "to_string") => Some(RegIntrinsic::FloatToString),
        ("Int", "bit_and") => Some(RegIntrinsic::IntBitAnd),
        ("Int", "bit_not") => Some(RegIntrinsic::IntBitNot),
        ("Int", "bit_or") => Some(RegIntrinsic::IntBitOr),
        ("Int", "bit_xor") => Some(RegIntrinsic::IntBitXor),
        ("Int", "shift_left") => Some(RegIntrinsic::IntShiftLeft),
        ("Int", "shift_right") => Some(RegIntrinsic::IntShiftRight),
        ("Int", "to_string") => Some(RegIntrinsic::IntToString),
        ("Int", "to_float") => Some(RegIntrinsic::IntToFloat),
        ("Math", "abs") => Some(RegIntrinsic::MathAbs),
        ("Math", "abs_float") => Some(RegIntrinsic::MathAbsFloat),
        ("Math", "ceil") => Some(RegIntrinsic::MathCeil),
        ("Math", "clamp") => Some(RegIntrinsic::MathClamp),
        ("Math", "clamp_float") => Some(RegIntrinsic::MathClampFloat),
        ("Math", "cos") => Some(RegIntrinsic::MathCos),
        ("Math", "exp") => Some(RegIntrinsic::MathExp),
        ("Math", "exp2") => Some(RegIntrinsic::MathExp2),
        ("Math", "floor") => Some(RegIntrinsic::MathFloor),
        ("Math", "log") => Some(RegIntrinsic::MathLog),
        ("Math", "log2") => Some(RegIntrinsic::MathLog2),
        ("Math", "max") => Some(RegIntrinsic::MathMax),
        ("Math", "max_float") => Some(RegIntrinsic::MathMaxFloat),
        ("Math", "min") => Some(RegIntrinsic::MathMin),
        ("Math", "min_float") => Some(RegIntrinsic::MathMinFloat),
        ("Math", "pow") => Some(RegIntrinsic::MathPow),
        ("Math", "pow_float") => Some(RegIntrinsic::MathPowFloat),
        ("Math", "round") => Some(RegIntrinsic::MathRound),
        ("Math", "saturating_add") => Some(RegIntrinsic::MathSaturatingAdd),
        ("Math", "saturating_mul") => Some(RegIntrinsic::MathSaturatingMul),
        ("Math", "saturating_sub") => Some(RegIntrinsic::MathSaturatingSub),
        ("Math", "sin") => Some(RegIntrinsic::MathSin),
        ("Math", "sqrt") => Some(RegIntrinsic::MathSqrt),
        ("Math", "tanh") => Some(RegIntrinsic::MathTanh),
        ("Math", "trunc_float") => Some(RegIntrinsic::MathTruncFloat),
        ("Math", "wrapping_add") => Some(RegIntrinsic::MathWrappingAdd),
        ("Math", "wrapping_mul") => Some(RegIntrinsic::MathWrappingMul),
        ("Math", "wrapping_sub") => Some(RegIntrinsic::MathWrappingSub),
        ("Json", "array") => Some(RegIntrinsic::JsonArray),
        ("Json", "array_bools") => Some(RegIntrinsic::JsonArrayBools),
        ("Json", "array_contains_prefix") => Some(RegIntrinsic::JsonArrayContainsPrefix),
        ("Json", "array_contains_string") => Some(RegIntrinsic::JsonArrayContainsString),
        ("Json", "array_count_where") => Some(RegIntrinsic::JsonArrayCountWhere),
        ("Json", "array_fold") => Some(RegIntrinsic::JsonArrayFold),
        ("Json", "array_get") => Some(RegIntrinsic::JsonArrayGet),
        ("Json", "array_ints") => Some(RegIntrinsic::JsonArrayInts),
        ("Json", "array_len") => Some(RegIntrinsic::JsonArrayLen),
        ("Json", "array_strings") => Some(RegIntrinsic::JsonArrayStrings),
        ("Json", "at") | ("Json", "value_at") => Some(RegIntrinsic::JsonAt),
        ("Json", "at_bool") => Some(RegIntrinsic::JsonAtBool),
        ("Json", "at_bool_or") => Some(RegIntrinsic::JsonAtBoolOr),
        ("Json", "at_int") => Some(RegIntrinsic::JsonAtInt),
        ("Json", "at_int_or") => Some(RegIntrinsic::JsonAtIntOr),
        ("Json", "at_optional") => Some(RegIntrinsic::JsonAtOptional),
        ("Json", "at_optional_bool") => Some(RegIntrinsic::JsonAtOptionalBool),
        ("Json", "at_optional_int") => Some(RegIntrinsic::JsonAtOptionalInt),
        ("Json", "at_optional_string") => Some(RegIntrinsic::JsonAtOptionalString),
        ("Json", "at_or") => Some(RegIntrinsic::JsonAtOr),
        ("Json", "at_string") => Some(RegIntrinsic::JsonAtString),
        ("Json", "at_string_or") => Some(RegIntrinsic::JsonAtStringOr),
        ("Json", "at_to_string") => Some(RegIntrinsic::JsonAtToString),
        ("Json", "at_to_string_or") => Some(RegIntrinsic::JsonAtToStringOr),
        ("Json", "as_bool") => Some(RegIntrinsic::JsonAsBool),
        ("Json", "as_int") => Some(RegIntrinsic::JsonAsInt),
        ("Json", "as_string") => Some(RegIntrinsic::JsonAsString),
        ("Json", "bool_at") => Some(RegIntrinsic::JsonBoolAt),
        ("Json", "bool_field") => Some(RegIntrinsic::JsonBoolField),
        ("Json", "clone") => Some(RegIntrinsic::JsonClone),
        ("Json", "decode") => Some(RegIntrinsic::JsonDecode),
        ("Json", "decode_text") => Some(RegIntrinsic::JsonDecodeText),
        ("Json", "encode") => Some(RegIntrinsic::JsonEncode),
        ("Json", "field") => Some(RegIntrinsic::JsonField),
        ("Json", "field_bool") => Some(RegIntrinsic::JsonFieldBool),
        ("Json", "field_int") => Some(RegIntrinsic::JsonFieldInt),
        ("Json", "field_optional") => Some(RegIntrinsic::JsonFieldOptional),
        ("Json", "field_optional_bool") => Some(RegIntrinsic::JsonFieldOptionalBool),
        ("Json", "field_optional_int") => Some(RegIntrinsic::JsonFieldOptionalInt),
        ("Json", "field_optional_string") => Some(RegIntrinsic::JsonFieldOptionalString),
        ("Json", "field_string") => Some(RegIntrinsic::JsonFieldString),
        ("Json", "int_at") => Some(RegIntrinsic::JsonIntAt),
        ("Json", "int_at_or") | ("Json", "json_int_at_or") => Some(RegIntrinsic::JsonIntAtOr),
        ("Json", "is_array") => Some(RegIntrinsic::JsonIsArray),
        ("Json", "is_null") => Some(RegIntrinsic::JsonIsNull),
        ("Json", "is_object") => Some(RegIntrinsic::JsonIsObject),
        ("Json", "int_field") => Some(RegIntrinsic::JsonIntField),
        ("Json", "kind") => Some(RegIntrinsic::JsonKind),
        ("Json", "object") => Some(RegIntrinsic::JsonObject),
        ("Json", "json_parse") | ("Json", "parse") => Some(RegIntrinsic::JsonParse),
        ("Json", "parse_file") => Some(RegIntrinsic::JsonParseFile),
        ("Json", "object_keys") => Some(RegIntrinsic::JsonObjectKeys),
        ("Json", "object_len") => Some(RegIntrinsic::JsonObjectLen),
        ("Json", "quote_string") => Some(RegIntrinsic::JsonQuoteString),
        ("Json", "raw_field") => Some(RegIntrinsic::JsonRawField),
        ("Json", "string_at") => Some(RegIntrinsic::JsonStringAt),
        ("Json", "string_array") => Some(RegIntrinsic::JsonStringArray),
        ("Json", "string_field") => Some(RegIntrinsic::JsonStringField),
        ("Json", "strings") => Some(RegIntrinsic::JsonStrings),
        ("Json", "to_string_at") => Some(RegIntrinsic::JsonToStringAt),
        ("Json", "to_string_at_or") => Some(RegIntrinsic::JsonToStringAtOr),
        ("Json", "to_string") => Some(RegIntrinsic::JsonToString),
        ("Json", "value") => Some(RegIntrinsic::JsonValue),
        ("Json", "values") => Some(RegIntrinsic::JsonValues),
        ("JsonError", "message") => Some(RegIntrinsic::JsonErrorMessage),
        ("List", "all") => Some(RegIntrinsic::ListAll),
        ("List", "any") => Some(RegIntrinsic::ListAny),
        ("List", "contains") => Some(RegIntrinsic::ListContains),
        ("List", "contains_value") => Some(RegIntrinsic::ListContainsValue),
        ("List", "count_where") => Some(RegIntrinsic::ListCountWhere),
        ("List", "consume") => Some(RegIntrinsic::ListConsume),
        ("List", "find") => Some(RegIntrinsic::ListFind),
        ("List", "flat_map") => Some(RegIntrinsic::ListFlatMap),
        ("List", "flatten") => Some(RegIntrinsic::ListFlatten),
        ("List", "first") => Some(RegIntrinsic::ListFirst),
        ("List", "is_empty") => Some(RegIntrinsic::ListIsEmpty),
        ("List", "join") => Some(RegIntrinsic::ListJoin),
        ("List", "group_by") => Some(RegIntrinsic::ListGroupBy),
        ("List", "last") => Some(RegIntrinsic::ListLast),
        ("List", "dedup") => Some(RegIntrinsic::ListDedup),
        ("List", "enumerate") => Some(RegIntrinsic::ListEnumerate),
        ("List", "max") => Some(RegIntrinsic::ListMax),
        ("List", "min") => Some(RegIntrinsic::ListMin),
        ("List", "new") => Some(RegIntrinsic::ListNew),
        ("List", "partition") => Some(RegIntrinsic::ListPartition),
        ("List", "pipeline") => Some(RegIntrinsic::ListPipeline),
        ("List", "reverse") => Some(RegIntrinsic::ListReverse),
        ("List", "skip") => Some(RegIntrinsic::ListSkip),
        ("List", "slice") => Some(RegIntrinsic::ListSlice),
        ("List", "sum") => Some(RegIntrinsic::ListSum),
        ("List", "zip") => Some(RegIntrinsic::ListZip),
        ("List", "take") => Some(RegIntrinsic::ListTake),
        ("List", "to_json_strings") => Some(RegIntrinsic::ListToJsonStrings),
        ("List", "to_json_values") => Some(RegIntrinsic::ListToJsonValues),
        ("List", "try_fold") => Some(RegIntrinsic::ListTryFold),
        ("Log", "error") => Some(RegIntrinsic::LogError),
        ("Log", "error_json") => Some(RegIntrinsic::LogErrorJson),
        ("Log", "trace") => Some(RegIntrinsic::LogTrace),
        ("Log", "write") => Some(RegIntrinsic::LogWrite),
        ("Log", "write_json") => Some(RegIntrinsic::LogWriteJson),
        ("Map", "contains_key") => Some(RegIntrinsic::MapContainsKey),
        ("Map", "filter") => Some(RegIntrinsic::MapFilter),
        ("Map", "fold") => Some(RegIntrinsic::MapFold),
        ("Map", "for_each") => Some(RegIntrinsic::MapForEach),
        ("Map", "get_or_default") => Some(RegIntrinsic::MapGetOrDefault),
        ("Map", "is_empty") => Some(RegIntrinsic::MapIsEmpty),
        ("Map", "keys") => Some(RegIntrinsic::MapKeys),
        ("Map", "len") => Some(RegIntrinsic::MapLen),
        ("Map", "map_values") => Some(RegIntrinsic::MapMapValues),
        ("Map", "merge") => Some(RegIntrinsic::MapMerge),
        ("Map", "new") => Some(RegIntrinsic::MapNew),
        ("Map", "try_fold") => Some(RegIntrinsic::MapTryFold),
        ("Map", "values") => Some(RegIntrinsic::MapValues),
        ("Option", "and_then") => Some(RegIntrinsic::OptionAndThen),
        ("Option", "filter") => Some(RegIntrinsic::OptionFilter),
        ("Option", "is_none") => Some(RegIntrinsic::OptionIsNone),
        ("Option", "is_some") => Some(RegIntrinsic::OptionIsSome),
        ("Option", "map") => Some(RegIntrinsic::OptionMap),
        ("Option", "ok_or") => Some(RegIntrinsic::OptionOkOr),
        ("Option", "or") => Some(RegIntrinsic::OptionOr),
        ("Option", "unwrap_or") => Some(RegIntrinsic::OptionUnwrapOr),
        ("Option", "unwrap_or_else") => Some(RegIntrinsic::OptionUnwrapOrElse),
        ("Clone", "clone") => Some(RegIntrinsic::CloneClone),
        ("Ord", "compare") => Some(RegIntrinsic::OrdCompare),
        ("OS", "close") => Some(RegIntrinsic::OsClose),
        ("Patch", "apply_text") => Some(RegIntrinsic::PatchApplyText),
        ("Path", "exists") => Some(RegIntrinsic::PathExists),
        ("Path", "extension") => Some(RegIntrinsic::PathExtension),
        ("Path", "file_name") => Some(RegIntrinsic::PathFileName),
        ("Path", "from_string") => Some(RegIntrinsic::PathFromString),
        ("Path", "is_absolute") => Some(RegIntrinsic::PathIsAbsolute),
        ("Path", "is_dir") => Some(RegIntrinsic::PathIsDir),
        ("Path", "is_file") => Some(RegIntrinsic::PathIsFile),
        ("Path", "join") => Some(RegIntrinsic::PathJoin),
        ("Path", "list_files") => Some(RegIntrinsic::PathListFiles),
        ("Path", "list_paths") => Some(RegIntrinsic::PathListPaths),
        ("Path", "normalize") => Some(RegIntrinsic::PathNormalize),
        ("Path", "parent") => Some(RegIntrinsic::PathParent),
        ("Path", "read_string") => Some(RegIntrinsic::PathReadString),
        ("Path", "resolve_relative") => Some(RegIntrinsic::PathResolveRelative),
        ("Path", "safe_relative") => Some(RegIntrinsic::PathSafeRelative),
        ("Path", "starts_with") => Some(RegIntrinsic::PathStartsWith),
        ("Path", "to_string") => Some(RegIntrinsic::PathToString),
        ("Path", "with_extension") => Some(RegIntrinsic::PathWithExtension),
        ("Path", "write_string") => Some(RegIntrinsic::PathWriteString),
        ("PersistentMap", "clear") => Some(RegIntrinsic::PersistentMapClear),
        ("PersistentMap", "contains_key") => Some(RegIntrinsic::PersistentMapContainsKey),
        ("PersistentMap", "get") => Some(RegIntrinsic::PersistentMapGet),
        ("PersistentMap", "insert") => Some(RegIntrinsic::PersistentMapInsert),
        ("PersistentMap", "is_empty") => Some(RegIntrinsic::PersistentMapIsEmpty),
        ("PersistentMap", "len") => Some(RegIntrinsic::PersistentMapLen),
        ("PersistentMap", "new") => Some(RegIntrinsic::PersistentMapNew),
        ("PersistentMap", "remove") => Some(RegIntrinsic::PersistentMapRemove),
        ("Pipeline", "collect") => Some(RegIntrinsic::PipelineCollect),
        ("Pipeline", "each") => Some(RegIntrinsic::PipelineEach),
        ("Pipeline", "try_map") => Some(RegIntrinsic::PipelineTryMap),
        ("PoolError", "message") => Some(RegIntrinsic::PoolErrorMessage),
        ("PoolStats", "available") => Some(RegIntrinsic::PoolStatsAvailable),
        ("PoolStats", "capacity") => Some(RegIntrinsic::PoolStatsCapacity),
        ("PoolStats", "created") => Some(RegIntrinsic::PoolStatsCreated),
        ("PoolStats", "in_use") => Some(RegIntrinsic::PoolStatsInUse),
        ("Process", "run") => Some(RegIntrinsic::ProcessRun),
        ("Process", "run_async") => Some(RegIntrinsic::ProcessRunAsync),
        ("Process", "run_many_stdout") => Some(RegIntrinsic::ProcessRunManyStdout),
        ("Process", "run_many_stdout_async") => Some(RegIntrinsic::ProcessRunManyStdoutAsync),
        ("Process", "run_request") => Some(RegIntrinsic::ProcessRunRequest),
        ("Process", "run_request_async") => Some(RegIntrinsic::ProcessRunRequestAsync),
        ("Process", "run_stdout") => Some(RegIntrinsic::ProcessRunStdout),
        ("Process", "run_stdout_async") => Some(RegIntrinsic::ProcessRunStdoutAsync),
        ("Process", "run_stdout_timeout") => Some(RegIntrinsic::ProcessRunStdoutTimeout),
        ("Process", "run_timeout") => Some(RegIntrinsic::ProcessRunTimeout),
        ("Process", "run_timeout_async") => Some(RegIntrinsic::ProcessRunTimeoutAsync),
        ("Process", "stream") => Some(RegIntrinsic::ProcessStream),
        ("Random", "bool") => Some(RegIntrinsic::RandomBool),
        ("Random", "bytes") => Some(RegIntrinsic::RandomBytes),
        ("Random", "float") => Some(RegIntrinsic::RandomFloat),
        ("Random", "int") => Some(RegIntrinsic::RandomInt),
        ("Random", "string") => Some(RegIntrinsic::RandomString),
        ("Regex", "captures") => Some(RegIntrinsic::RegexCaptures),
        ("Regex", "compile") => Some(RegIntrinsic::RegexCompile),
        ("Regex", "find") => Some(RegIntrinsic::RegexFind),
        ("Regex", "is_match") => Some(RegIntrinsic::RegexIsMatch),
        ("Regex", "replace_all") => Some(RegIntrinsic::RegexReplaceAll),
        ("Regex", "split") => Some(RegIntrinsic::RegexSplit),
        ("RegexError", "message") => Some(RegIntrinsic::RegexErrorMessage),
        ("Result", "and_then") => Some(RegIntrinsic::ResultAndThen),
        ("Result", "err") => Some(RegIntrinsic::ResultErr),
        ("Result", "err_message") => Some(RegIntrinsic::ResultErrMessage),
        ("Result", "is_err") => Some(RegIntrinsic::ResultIsErr),
        ("Result", "is_ok") => Some(RegIntrinsic::ResultIsOk),
        ("Result", "map") => Some(RegIntrinsic::ResultMap),
        ("Result", "map_error") => Some(RegIntrinsic::ResultMapError),
        ("Result", "ok") => Some(RegIntrinsic::ResultOk),
        ("Result", "unwrap_or") => Some(RegIntrinsic::ResultUnwrapOr),
        ("Result", "unwrap_or_else") => Some(RegIntrinsic::ResultUnwrapOrElse),
        ("Request", "new") => Some(RegIntrinsic::RequestNew),
        ("Request", "path") => Some(RegIntrinsic::RequestPath),
        ("Receiver", "close") => Some(RegIntrinsic::ReceiverClose),
        ("Receiver", "into_stream") => Some(RegIntrinsic::ReceiverIntoStream),
        ("Receiver", "recv") => Some(RegIntrinsic::ReceiverRecv),
        ("Receiver", "recv_cancellable") => Some(RegIntrinsic::ReceiverRecvCancellable),
        ("Response", "body") => Some(RegIntrinsic::ResponseBody),
        ("Response", "ok") => Some(RegIntrinsic::ResponseOk),
        ("Response", "status") => Some(RegIntrinsic::ResponseStatus),
        ("Row", "field_string") => Some(RegIntrinsic::RowFieldString),
        ("RowBuffer", "new") => Some(RegIntrinsic::RowBufferNew),
        ("RuleLoader", "load_rules") => Some(RegIntrinsic::RuleLoaderLoadRules),
        ("ResourcePool", "borrow") => Some(RegIntrinsic::ResourcePoolBorrow),
        ("ResourcePool", "discard") => Some(RegIntrinsic::ResourcePoolDiscard),
        ("ResourcePool", "lazy") => Some(RegIntrinsic::ResourcePoolLazy),
        ("ResourcePool", "new") => Some(RegIntrinsic::ResourcePoolNew),
        ("ResourcePool", "stats") => Some(RegIntrinsic::ResourcePoolStats),
        ("ResourcePool", "try_borrow") => Some(RegIntrinsic::ResourcePoolTryBorrow),
        ("ResourcePool", "try_lazy") => Some(RegIntrinsic::ResourcePoolTryLazy),
        ("ResourcePool", "try_new") => Some(RegIntrinsic::ResourcePoolTryNew),
        ("Set", "contains") => Some(RegIntrinsic::SetContains),
        ("Set", "difference") => Some(RegIntrinsic::SetDifference),
        ("Set", "intersection") => Some(RegIntrinsic::SetIntersection),
        ("Set", "is_empty") => Some(RegIntrinsic::SetIsEmpty),
        ("Set", "is_subset") => Some(RegIntrinsic::SetIsSubset),
        ("Set", "len") => Some(RegIntrinsic::SetLen),
        ("Set", "new") => Some(RegIntrinsic::SetNew),
        ("Set", "to_list") => Some(RegIntrinsic::SetToList),
        ("Set", "union") => Some(RegIntrinsic::SetUnion),
        ("SortedSet", "contains") => Some(RegIntrinsic::SortedSetContains),
        ("SortedSet", "is_empty") => Some(RegIntrinsic::SortedSetIsEmpty),
        ("SortedSet", "len") => Some(RegIntrinsic::SortedSetLen),
        ("SortedSet", "new") => Some(RegIntrinsic::SortedSetNew),
        ("SortedSet", "to_list") => Some(RegIntrinsic::SortedSetToList),
        ("SortedMap", "contains_key") => Some(RegIntrinsic::SortedMapContainsKey),
        ("SortedMap", "get") => Some(RegIntrinsic::SortedMapGet),
        ("SortedMap", "is_empty") => Some(RegIntrinsic::SortedMapIsEmpty),
        ("SortedMap", "keys") => Some(RegIntrinsic::SortedMapKeys),
        ("SortedMap", "len") => Some(RegIntrinsic::SortedMapLen),
        ("SortedMap", "new") => Some(RegIntrinsic::SortedMapNew),
        ("SortedMap", "values") => Some(RegIntrinsic::SortedMapValues),
        ("String", "after") => Some(RegIntrinsic::StringAfter),
        ("String", "before") => Some(RegIntrinsic::StringBefore),
        ("String", "char_at") => Some(RegIntrinsic::StringCharAt),
        ("String", "contains") => Some(RegIntrinsic::StringContains),
        ("String", "count") => Some(RegIntrinsic::StringCount),
        ("String", "copy") | ("String", "clone") => Some(RegIntrinsic::StringCopy),
        ("String", "ends_with") => Some(RegIntrinsic::StringEndsWith),
        ("String", "env") => Some(RegIntrinsic::EnvGet),
        ("String", "env_or") => Some(RegIntrinsic::EnvGetOrDefault),
        ("String", "format") => Some(RegIntrinsic::StringFormat),
        ("String", "from_bool") => Some(RegIntrinsic::StringFromBool),
        ("String", "from_float") => Some(RegIntrinsic::StringFromFloat),
        ("String", "from_int") => Some(RegIntrinsic::StringFromInt),
        ("String", "index_of") => Some(RegIntrinsic::StringIndexOf),
        ("String", "is_empty") => Some(RegIntrinsic::StringIsEmpty),
        ("String", "join") => Some(RegIntrinsic::StringJoin),
        ("String", "lines") => Some(RegIntrinsic::StringLines),
        ("String", "chars") => Some(RegIntrinsic::StringChars),
        ("String", "len") => Some(RegIntrinsic::StringLen),
        ("String", "pad_left") => Some(RegIntrinsic::StringPadLeft),
        ("String", "pad_right") => Some(RegIntrinsic::StringPadRight),
        ("String", "parse_float") => Some(RegIntrinsic::StringParseFloat),
        ("String", "parse_int") => Some(RegIntrinsic::StringParseInt),
        ("String", "repeat") => Some(RegIntrinsic::StringRepeat),
        ("String", "replace") => Some(RegIntrinsic::StringReplace),
        ("String", "replace_first") => Some(RegIntrinsic::StringReplaceFirst),
        ("String", "reverse") => Some(RegIntrinsic::StringReverse),
        ("String", "slice") | ("String", "view") => Some(RegIntrinsic::StringSlice),
        ("String", "split") => Some(RegIntrinsic::StringSplit),
        ("String", "starts_with") => Some(RegIntrinsic::StringStartsWith),
        ("String", "strip_prefix") => Some(RegIntrinsic::StringStripPrefix),
        ("String", "safe_relative") => Some(RegIntrinsic::PathSafeRelative),
        ("String", "to_path") => Some(RegIntrinsic::PathFromString),
        ("String", "to_url") => Some(RegIntrinsic::UrlFromString),
        ("String", "to_bytes") => Some(RegIntrinsic::BytesFromString),
        ("String", "to_lowercase") => Some(RegIntrinsic::StringToLowercase),
        ("String", "to_uppercase") => Some(RegIntrinsic::StringToUppercase),
        ("String", "trim") => Some(RegIntrinsic::StringTrim),
        ("String", "trim_end") => Some(RegIntrinsic::StringTrimEnd),
        ("String", "trim_start") => Some(RegIntrinsic::StringTrimStart),
        ("TcpError", "message") => Some(RegIntrinsic::TcpErrorMessage),
        ("Toml", "parse_file") => Some(RegIntrinsic::TomlParseFile),
        ("StringBuilder", "finish") => Some(RegIntrinsic::StringCopy),
        ("StringBuilder", "new") => Some(RegIntrinsic::StringBuilderNew),
        ("Stream", "collect_list") => Some(RegIntrinsic::StreamCollectList),
        ("Stream", "from_list") => Some(RegIntrinsic::StreamFromList),
        ("Stream", "next") => Some(RegIntrinsic::StreamNext),
        ("Sender", "close") => Some(RegIntrinsic::SenderClose),
        ("Sender", "send") => Some(RegIntrinsic::SenderSend),
        ("Sender", "send_cancellable") => Some(RegIntrinsic::SenderSendCancellable),
        ("StringView", "after") => Some(RegIntrinsic::StringAfter),
        ("StringView", "before") => Some(RegIntrinsic::StringBefore),
        ("StringView", "contains") => Some(RegIntrinsic::StringContains),
        ("StringView", "is_empty") => Some(RegIntrinsic::StringIsEmpty),
        ("StringView", "len") => Some(RegIntrinsic::StringLen),
        ("StringView", "slice") => Some(RegIntrinsic::StringSlice),
        ("StringView", "starts_with") => Some(RegIntrinsic::StringStartsWith),
        ("StringView", "to_string") => Some(RegIntrinsic::StringCopy),
        ("Tcp", "connect") => Some(RegIntrinsic::TcpConnect),
        ("TempDir", "keep") => Some(RegIntrinsic::TempDirKeep),
        ("TempDir", "new") => Some(RegIntrinsic::TempDirNew),
        ("TempDir", "new_in") => Some(RegIntrinsic::TempDirNewIn),
        ("TempDir", "path") => Some(RegIntrinsic::TempDirPath),
        ("TcpStream", "read") => Some(RegIntrinsic::TcpStreamRead),
        ("TcpStream", "shutdown") => Some(RegIntrinsic::TcpStreamShutdown),
        ("TcpStream", "write") => Some(RegIntrinsic::TcpStreamWrite),
        ("TcpStream", "write_all") => Some(RegIntrinsic::TcpStreamWriteAll),
        ("Timer", "sleep") => Some(RegIntrinsic::TimerSleep),
        ("Timer", "sleep_cancellable") => Some(RegIntrinsic::TimerSleepCancellable),
        ("Timer", "sleep_until") => Some(RegIntrinsic::TimerSleepUntil),
        ("Url", "decode_component") => Some(RegIntrinsic::UrlDecodeComponent),
        ("Url", "encode_component") => Some(RegIntrinsic::UrlEncodeComponent),
        ("Url", "from_string") => Some(RegIntrinsic::UrlFromString),
        ("Url", "to_string") => Some(RegIntrinsic::UrlToString),
        ("Uuid", "new_v4") => Some(RegIntrinsic::UuidNewV4),
        ("Workspace", "resolve") => Some(RegIntrinsic::PathResolveRelative),
        ("WebSocket", "close") => Some(RegIntrinsic::WebSocketClose),
        ("WebSocket", "connect") => Some(RegIntrinsic::WebSocketConnect),
        ("WebSocket", "recv_bytes") => Some(RegIntrinsic::WebSocketRecvBytes),
        ("WebSocket", "recv_text") => Some(RegIntrinsic::WebSocketRecvText),
        ("WebSocket", "send_bytes") => Some(RegIntrinsic::WebSocketSendBytes),
        ("WebSocket", "send_text") => Some(RegIntrinsic::WebSocketSendText),
        ("WebSocketError", "message") => Some(RegIntrinsic::WebSocketErrorMessage),
        ("Yaml", "parse") => Some(RegIntrinsic::YamlParse),
        ("Yaml", "parse_file") => Some(RegIntrinsic::YamlParseFile),
        ("Weak", "downgrade") => Some(RegIntrinsic::WeakDowngrade),
        ("Weak", "from") => Some(RegIntrinsic::WeakFrom),
        ("Weak", "upgrade") => Some(RegIntrinsic::WeakUpgrade),
        _ => None,
    }
}
fn closure_capture_names(
    body: &HirBlock,
    params: &[String],
    explicit_captures: &[crate::hir::HirClosureCapture],
    outer_locals: &HashMap<String, Reg>,
) -> Vec<String> {
    let mut names = explicit_captures
        .iter()
        .map(|capture| capture.name.clone())
        .collect::<Vec<_>>();
    let mut seen = names.iter().cloned().collect::<HashSet<_>>();
    let mut bound = params.iter().cloned().collect::<HashSet<_>>();
    let mut free = BTreeSet::new();
    collect_free_locals_block(body, &mut bound, &mut free);
    for name in free {
        if outer_locals.contains_key(&name) && seen.insert(name.clone()) {
            names.push(name);
        }
    }
    names
}

fn collect_free_locals_block(
    block: &HirBlock,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    for statement in &block.statements {
        collect_free_locals_stmt(statement, bound, free);
    }
}

fn collect_free_locals_stmt(
    statement: &HirStmt,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    match statement {
        HirStmt::Let { name, value, .. } => {
            if let Some(value) = value {
                collect_free_locals_expr(value, bound, free);
            }
            bound.insert(name.clone());
        }
        HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_free_locals_expr(value, bound, free);
            }
        }
        HirStmt::With {
            resource,
            binding,
            body,
            ..
        } => {
            collect_free_locals_expr(resource, bound, free);
            let mut body_bound = bound.clone();
            body_bound.insert(binding.clone());
            collect_free_locals_block(body, &mut body_bound, free);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_free_locals_expr(condition, bound, free);
            collect_free_locals_block(&then_body.clone(), &mut bound.clone(), free);
            if let Some(else_body) = else_body {
                collect_free_locals_block(&else_body.clone(), &mut bound.clone(), free);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_free_locals_expr(condition, bound, free);
            }
            collect_free_locals_block(&body.clone(), &mut bound.clone(), free);
        }
        HirStmt::For {
            binding,
            iterable,
            body,
            ..
        } => {
            collect_free_locals_expr(iterable, bound, free);
            let mut body_bound = bound.clone();
            body_bound.insert(binding.clone());
            collect_free_locals_block(body, &mut body_bound, free);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_free_locals_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                for binding in arm.pattern.binding_names() {
                    arm_bound.insert(binding.to_string());
                }
                if let Some(guard) = &arm.guard {
                    collect_free_locals_expr(guard, &mut arm_bound, free);
                }
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_free_locals_expr(&arm.operation, bound, free);
                let mut arm_bound = bound.clone();
                arm_bound.insert(arm.binding.clone());
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirStmt::Assign { target, value, .. } => {
            collect_free_locals_expr(target, bound, free);
            collect_free_locals_expr(value, bound, free);
        }
        HirStmt::Expr(value) => collect_free_locals_expr(value, bound, free),
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn collect_free_locals_expr(
    expr: &HirExpr,
    bound: &mut HashSet<String>,
    free: &mut BTreeSet<String>,
) {
    match expr {
        HirExpr::Ident { name, .. } => {
            if !bound.contains(name) {
                free.insert(name.clone());
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_free_locals_expr(&field.value, bound, free);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_free_locals_expr(&entry.key, bound, free);
                collect_free_locals_expr(&entry.value, bound, free);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_free_locals_expr(item, bound, free);
            }
        }
        HirExpr::Binary { left, right, .. } => {
            collect_free_locals_expr(left, bound, free);
            collect_free_locals_expr(right, bound, free);
        }
        HirExpr::Field { base, .. } => collect_free_locals_expr(base, bound, free),
        HirExpr::Index { base, index, .. } => {
            collect_free_locals_expr(base, bound, free);
            collect_free_locals_expr(index, bound, free);
        }
        HirExpr::Call { receiver, args, .. } => {
            if let Some(receiver) = receiver {
                collect_free_locals_expr(&receiver.value, bound, free);
            }
            for arg in args {
                collect_free_locals_expr(&arg.value, bound, free);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_free_locals_expr(value, bound, free),
        HirExpr::Closure {
            params,
            captures,
            body,
            ..
        } => {
            for capture in captures {
                if !bound.contains(&capture.name) {
                    free.insert(capture.name.clone());
                }
            }
            let mut nested_bound = bound.clone();
            for param in params {
                nested_bound.insert(param.clone());
            }
            collect_free_locals_block(body, &mut nested_bound, free);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_free_locals_expr(value, bound, free);
            for arm in arms {
                let mut arm_bound = bound.clone();
                for binding in arm.pattern.binding_names() {
                    arm_bound.insert(binding.to_string());
                }
                if let Some(guard) = &arm.guard {
                    collect_free_locals_expr(guard, &mut arm_bound, free);
                }
                collect_free_locals_block(&arm.body, &mut arm_bound, free);
            }
        }
        HirExpr::Number { .. } | HirExpr::String { .. } | HirExpr::Unknown(_) => {}
    }
}
