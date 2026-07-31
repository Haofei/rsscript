use super::*;

mod closure_analysis;

use closure_analysis::*;

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
    /// Registers proven to hold a `Copy` scalar (`Int`/`Bool`/`Float`/`Char`/…,
    /// per [`scalar_param_type_needs_no_deep_copy`]). Such a value is inline with
    /// no interior `Rc`, so extracting it from a collection/struct/variant cannot
    /// alias the source. The DeepCopy-elision taint analysis reads this set to
    /// avoid over-tainting an extraction's source collection (see
    /// [`deepcopy_elidable_param_regs`]). Conservative by construction: a register
    /// is only inserted when the extracted static type is a known scalar; absence
    /// means "unknown", which keeps the (sound) over-tainting behavior.
    pub(crate) scalar_regs: std::collections::HashSet<Reg>,
    /// Registers that have EVER been bound to a non-scalar (heap / unknown) value.
    /// `local(name)` reuses one register per variable name, so a name bound to a
    /// scalar in one place and a heap value in another shares a register; without
    /// this poison set a scalar binding could (re-)add that shared register to
    /// [`Self::scalar_regs`] and wrongly elide the heap binding's copy (the taint
    /// analysis is flat/order-insensitive, so "last write wins" is not enough).
    /// Once poisoned, a register can never (re-)enter `scalar_regs`, so a reused
    /// register is treated as scalar only when EVERY binding of it is scalar —
    /// the sound, over-tainting default.
    pub(crate) scalar_poison_regs: std::collections::HashSet<Reg>,
}

/// The element type produced by extracting from a collection type, or `None`
/// when it cannot be determined (⇒ conservative: not marked scalar ⇒ copy kept).
/// `List<T>`/`Deque<T>` → `T`; `Map<K, V>`/`SortedMap<K, V>` → `V` (the VALUE, the
/// type `MapGet` yields). Any other root (or a missing generic arg) → `None`.
fn list_elem_type(collection_ty: &str) -> Option<&str> {
    let root = crate::text_util::type_root_name(collection_ty);
    let args = crate::text_util::type_arg_names(collection_ty)?;
    match root {
        "List" | "Deque" => args.first().copied(),
        "Map" | "SortedMap" => args.get(1).copied(),
        _ => None,
    }
}

/// The `index`-th top-level generic argument of a (possibly `None`) type spelling,
/// or `None` when the type is unknown / not generic / has too few arguments.
/// Used to derive a pattern payload type from the scrutinee's static type:
/// arg 0 of `Option<Int>` / `Result<Int, E>` is the `Some`/`Ok` payload, arg 1 of
/// `Result<T, Int>` is the `Err` payload. `None` ⇒ conservative: bind stays tainted.
fn nth_type_arg(type_name: Option<&str>, index: usize) -> Option<&str> {
    crate::text_util::type_arg_names(type_name?)?
        .get(index)
        .copied()
}

/// Strip any `Effect`/`Try` wrapper from a call-argument value to reach the
/// underlying place expression (e.g. `mut cache.items` → `cache.items`).
fn unwrap_arg_place(value: &HirExpr) -> &HirExpr {
    match value {
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => unwrap_arg_place(value),
        other => other,
    }
}

/// Plan which `mut` arguments are PLACES (field/index) that need their final
/// (written-back) value stored back into the place after the call. Returns
/// `(arg_index, arg_register)` pairs. A `mut` local argument needs nothing — its
/// argument register IS the local, so the runtime `mut`-writeback already updates
/// it. A `mut` field/index argument was lowered to a temp (a field READ), so
/// without an explicit restore the callee's mutation is silently dropped (a
/// `mut` scalar field would print the pre-call value; heap places re-store the
/// same handle, a harmless no-op that also makes a callee *reassignment* of a
/// heap `mut` param visible). Mirrors AOT's `&mut <place>` semantics.
fn mut_place_restore_plan(
    args: &[HirCallArg],
    mut_positions: &[usize],
    arg_regs: &[Reg],
) -> Vec<(usize, Reg)> {
    mut_positions
        .iter()
        .filter_map(|&pos| {
            let arg = args.get(pos)?;
            let reg = *arg_regs.get(pos)?;
            match unwrap_arg_place(&arg.value) {
                HirExpr::Field { .. } | HirExpr::Index { .. } => Some((pos, reg)),
                _ => None,
            }
        })
        .collect()
}

impl RegLowerer<'_> {
    /// Store each `mut`-place argument's written-back register value back into its
    /// place after a call (see [`mut_place_restore_plan`]).
    fn restore_mut_place_args(
        &mut self,
        args: &[HirCallArg],
        plan: Vec<(usize, Reg)>,
    ) -> Result<(), EvalError> {
        for (index, reg) in plan {
            self.lower_assign(unwrap_arg_place(&args[index].value), reg)?;
        }
        Ok(())
    }

    /// Record that `dst` holds a `Copy` scalar when `elem_ty` is a known scalar
    /// type (`Int`/`Bool`/`Float`/`Char`/…). Extracting such a value is a bit-copy
    /// that cannot alias its source, so the elision taint analysis skips it. A
    /// `None` / non-scalar `elem_ty` (e.g. `String`, unknown) leaves `dst`
    /// unmarked — the sound, over-tainting default.
    fn note_scalar(&mut self, dst: Reg, elem_ty: Option<&str>) {
        let is_scalar = elem_ty.is_some_and(|ty| {
            scalar_param_type_needs_no_deep_copy(crate::text_util::strip_fresh_type(ty))
        });
        if is_scalar {
            // Only mark scalar if this register has never held a non-scalar; see
            // `scalar_poison_regs`. Reusing `local(name)` across a scalar and a
            // heap binding of the same name must NOT elide the heap copy.
            if !self.scalar_poison_regs.contains(&dst) {
                self.scalar_regs.insert(dst);
            }
        } else {
            // Non-scalar (or unknown) binding: poison the register permanently and
            // drop any scalar mark a prior binding of the same name may have set.
            self.scalar_poison_regs.insert(dst);
            self.scalar_regs.remove(&dst);
        }
    }

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
                let elem_ty = reg_expr_type_name(iterable).and_then(list_elem_type);
                self.note_scalar(item, elem_ty);

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
            HirExpr::Char { value, .. } => {
                let dst = self.temp();
                self.emit(RegInstr::LoadChar {
                    dst,
                    value: decode_char_token(value),
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
                // `access.type_name` is the field's own static type — the value
                // this extraction yields — so it feeds `note_scalar` directly.
                self.note_scalar(dst, access.type_name.as_deref());
                Ok(dst)
            }
            HirExpr::Index { base, index, .. } => {
                let list = self.expr(base)?;
                let elem_ty = reg_expr_type_name(base).and_then(list_elem_type);
                let index = self.expr(index)?;
                let dst = self.temp();
                self.emit(RegInstr::ListGet { dst, list, index });
                self.note_scalar(dst, elem_ty);
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
                            jit_self_recursion_kind: std::cell::Cell::new(None),
                            native_status: std::cell::Cell::new(0),
                            call_count: std::cell::Cell::new(0),
                            branch_count: std::cell::Cell::new(0),
                            profile: RefCell::new(None),
                            osr_state: std::cell::Cell::new(OsrTrigger::Unknown),
                        },
                        loop_stack: Vec::new(),
                        cleanup_stack: Vec::new(),
                        closure_identity_observable: self.closure_identity_observable,
                        scalar_regs: std::collections::HashSet::new(),
                        scalar_poison_regs: std::collections::HashSet::new(),
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
                parameter_index: Some(0),
                evaluation_index: 0,
                span: crate::diagnostic::Span::default(),
            });
            synthetic_args.extend(args.iter().cloned().map(|mut arg| {
                arg.evaluation_index += 1;
                arg
            }));
            return self.call(&synthetic_callee, None, &synthetic_args);
        }

        // Evaluate in source order, then lay out operands in declared parameter
        // order. Named-call semantics require both orders simultaneously.
        let evaluated_regs = args
            .iter()
            .map(|arg| self.expr(&arg.value))
            .collect::<Result<Vec<_>, _>>()?;
        let mut order = (0..args.len()).collect::<Vec<_>>();
        order.sort_by_key(|&index| {
            args[index]
                .parameter_index
                .unwrap_or(usize::MAX.saturating_sub(args.len()) + index)
        });
        let ordered_args = order
            .iter()
            .map(|&index| args[index].clone())
            .collect::<Vec<_>>();
        let arg_regs = order
            .iter()
            .map(|&index| evaluated_regs[index])
            .collect::<Vec<_>>();
        let args = ordered_args.as_slice();
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
                    let restore = mut_place_restore_plan(args, &mut_args, &arg_regs);
                    self.emit(RegInstr::CallClosure {
                        dst,
                        closure,
                        args: arg_regs,
                        mut_args,
                    });
                    self.restore_mut_place_args(args, restore)?;
                    return Ok(dst);
                }
                // A generic call carries its type args in `name` (e.g.
                // `get_v<Int>`); functions are keyed by their bare name, so strip
                // the generics before the lookup — otherwise a generic *function*
                // call falls through and is mis-lowered as a struct construction.
                if let Some(function) = self.function_ids.get(type_root_name(name)).copied() {
                    let mut_args = self.user_mut_arg_positions(name);
                    let restore = mut_place_restore_plan(args, &mut_args, &arg_regs);
                    self.emit(RegInstr::CallKnown {
                        dst,
                        function,
                        args: arg_regs,
                        mut_args,
                    });
                    self.restore_mut_place_args(args, restore)?;
                } else if self.is_native_function(None, name) {
                    let mut_args = self.native_mut_arg_positions(None, name);
                    let restore = mut_place_restore_plan(args, &mut_args, &arg_regs);
                    self.emit(RegInstr::CallNative {
                        dst,
                        key: type_root_name(name).to_string(),
                        args: arg_regs,
                        mut_args,
                    });
                    self.restore_mut_place_args(args, restore)?;
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
                        ("CancellationToken", "is_cancelled") => {
                            RegIntrinsic::CancellationTokenIsCancelled
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
                            let elem_ty = args
                                .first()
                                .and_then(|arg| reg_expr_type_name(&arg.value))
                                .and_then(list_elem_type);
                            self.note_scalar(dst, elem_ty);
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
                            let elem_ty = args
                                .first()
                                .and_then(|arg| reg_expr_type_name(&arg.value))
                                .and_then(list_elem_type);
                            self.note_scalar(dst, elem_ty);
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
                            let elem_ty = args
                                .first()
                                .and_then(|arg| reg_expr_type_name(&arg.value))
                                .and_then(list_elem_type);
                            self.note_scalar(dst, elem_ty);
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
                            // `list_elem_type` returns the Map VALUE type (`V` of
                            // `Map<K, V>`), which is what `MapGet` yields (as a
                            // fresh `Option<V>` of a cloned value).
                            let value_ty = args
                                .first()
                                .and_then(|arg| reg_expr_type_name(&arg.value))
                                .and_then(list_elem_type);
                            self.note_scalar(dst, value_ty);
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
                        ("StringBuilder", "finish") => {
                            if arg_regs.len() != 1 {
                                return Err(EvalError::Runtime(format!(
                                    "reg VM StringBuilder.finish expected 1 arg, got {}.",
                                    arg_regs.len()
                                )));
                            }
                            self.emit(RegInstr::StringBuilderFinish {
                                dst,
                                builder: arg_regs[0],
                            });
                            return Ok(dst);
                        }
                        _ => {
                            let qualified_key = format!("{namespace_root}.{name_root}");
                            // Native declarations also appear in `function_ids` (with
                            // empty bodies), so dispatch them as native boundaries
                            // first. A user-defined qualified function (e.g.
                            // `pub fn Data.execute`) is never native, so it falls
                            // through to the `function_ids` lookup below.
                            if self.is_native_function(Some(namespace_root), name_root) {
                                let mut_args =
                                    self.native_mut_arg_positions(Some(namespace_root), name_root);
                                let restore = mut_place_restore_plan(args, &mut_args, &arg_regs);
                                self.emit(RegInstr::CallNative {
                                    dst,
                                    key: qualified_key,
                                    args: arg_regs,
                                    mut_args,
                                });
                                self.restore_mut_place_args(args, restore)?;
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
                                let restore = mut_place_restore_plan(args, &mut_args, &arg_regs);
                                self.emit(RegInstr::CallDynamic {
                                    dst,
                                    dispatch,
                                    args: arg_regs,
                                    mut_args,
                                });
                                self.restore_mut_place_args(args, restore)?;
                                return Ok(dst);
                            }
                            if let Some(function) = self.function_ids.get(&qualified_key).copied() {
                                let mut_args =
                                    self.native_mut_arg_positions(Some(namespace_root), name_root);
                                let restore = mut_place_restore_plan(args, &mut_args, &arg_regs);
                                self.emit(RegInstr::CallKnown {
                                    dst,
                                    function,
                                    args: arg_regs,
                                    mut_args,
                                });
                                self.restore_mut_place_args(args, restore)?;
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
        // Scrutinee's static type threads into pattern lowering so a scalar
        // element/field/payload extraction can be marked `note_scalar` and thus
        // not taint the (read-param) scrutinee. `None` ⇒ conservative (unmarked).
        let scrutinee_ty = reg_expr_type_name(value);
        let mut failure_patches = Vec::new();
        let mut end_jumps = Vec::new();
        for arm in arms {
            let arm_ip = self.function.code.len();
            self.patch_match_failures(failure_patches, arm_ip);
            failure_patches = self.lower_match_pattern(&arm.pattern, src, scrutinee_ty)?;
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
        let scrutinee_ty = reg_expr_type_name(value);
        let dst = self.temp();
        let mut failure_patches = Vec::new();
        let mut end_jumps = Vec::new();
        for arm in arms {
            let arm_ip = self.function.code.len();
            self.patch_match_failures(failure_patches, arm_ip);
            failure_patches = self.lower_match_pattern(&arm.pattern, src, scrutinee_ty)?;
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
        scrutinee_ty: Option<&str>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        match pattern {
            MatchPattern::Binding { name, .. } => {
                let dst = self.local(name);
                self.emit(RegInstr::Move { dst, src });
                Ok(Vec::new())
            }
            MatchPattern::Wildcard(_) => Ok(Vec::new()),
            MatchPattern::Variant { name, bindings, .. } if name == "Some" => {
                // `Some(x)` on `Option<T>` unwraps the payload `T` (arg 0).
                let payload_ty = nth_type_arg(scrutinee_ty, 0);
                self.lower_option_some_pattern(src, bindings.first(), payload_ty)
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
            MatchPattern::Variant { name, bindings, .. } if name == "Ok" || name == "Err" => {
                // `Ok`/`Err` on `Result<T, E>` unwrap arg 0 / arg 1 respectively.
                let payload_ty = nth_type_arg(scrutinee_ty, if name == "Ok" { 0 } else { 1 });
                self.lower_result_variant_pattern(src, name, bindings.first(), payload_ty)
            }
            MatchPattern::Variant { name, bindings, .. }
                if self.hir.sum_type_for_variant(name).is_some() =>
            {
                self.lower_user_variant_pattern(src, name, bindings)
            }
            MatchPattern::Struct { name, fields, .. }
                if self.hir.sum_type_for_variant(name).is_some() =>
            {
                self.lower_user_struct_variant_pattern(src, name, fields)
            }
            MatchPattern::Struct { fields, .. } => {
                self.lower_struct_field_patterns(src, fields, scrutinee_ty)
            }
            MatchPattern::List {
                prefix,
                rest,
                suffix,
                ..
            } => self.lower_list_pattern(src, prefix, rest, suffix, scrutinee_ty),
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
            MatchPattern::Variant { name, bindings, .. }
                if matches!(name.as_str(), "Some" | "None" | "Ok" | "Err") =>
            {
                bindings
                    .iter()
                    .all(|binding| self.is_supported_match_pattern(binding))
            }
            MatchPattern::Variant { name, bindings, .. } => {
                self.hir.sum_type_for_variant(name).is_some()
                    && bindings
                        .iter()
                        .all(|binding| self.is_supported_match_pattern(binding))
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
        scrutinee_ty: Option<&str>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        let mut failures = Vec::new();
        // Resolve the struct decl once (decoupled from `self` so `note_scalar`'s
        // `&mut self` borrow is free); each field's declared type gates the mark.
        let hir = self.hir;
        let owner = scrutinee_ty
            .map(|ty| crate::text_util::type_root_name(ty))
            .and_then(|root| hir.type_info(root));
        for field in fields {
            if field.ignored {
                continue;
            }
            let field_ty = owner
                .and_then(|info| info.fields.get(&field.name))
                .map(|info| info.ty.to_string());
            let field_reg = self.temp();
            self.emit(RegInstr::GetField {
                dst: field_reg,
                base: src,
                name: field.name.clone(),
            });
            // A scalar field is a bit-copy: marking `field_reg` stops the `GetField`
            // from tainting the scrutinee (the read param). Non-scalar ⇒ unmarked.
            self.note_scalar(field_reg, field_ty.as_deref());
            if let Some(pattern) = field.pattern.as_deref() {
                failures.extend(self.lower_match_pattern(
                    pattern,
                    field_reg,
                    field_ty.as_deref(),
                )?);
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
        scrutinee_ty: Option<&str>,
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        // Element type of the matched `List<T>`/`Deque<T>` (the `ListGet` result);
        // a scalar `T` lets the per-element extraction skip tainting the scrutinee.
        let elem_ty = scrutinee_ty.and_then(list_elem_type);
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
            self.note_scalar(elem, elem_ty);
            failures.extend(self.lower_match_pattern(pattern, elem, elem_ty)?);
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
                self.note_scalar(elem, elem_ty);
                failures.extend(self.lower_match_pattern(pattern, elem, elem_ty)?);
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
            MatchLiteral::Char(value) => {
                self.emit(RegInstr::LoadChar {
                    dst: expected,
                    value: decode_char_token(value),
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
        payload_ty: Option<&str>,
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
                    // A scalar `Some` payload is a bit-copy ⇒ the unwrap must not
                    // taint the scrutinee (the read-param `Option<Scalar>`).
                    self.note_scalar(dst, payload_ty);
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapSome { dst: payload, src });
                    self.note_scalar(payload, payload_ty);
                    failures.extend(self.lower_match_pattern(binding, payload, payload_ty)?);
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
        payload_ty: Option<&str>,
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
                    // Scalar `Ok`/`Err` payload ⇒ bit-copy ⇒ don't taint scrutinee.
                    self.note_scalar(dst, payload_ty);
                }
                MatchPattern::Wildcard(_) => {}
                _ => {
                    let payload = self.temp();
                    self.emit(RegInstr::UnwrapVariantValue {
                        dst: payload,
                        src,
                        expected: variant.to_string(),
                    });
                    self.note_scalar(payload, payload_ty);
                    failures.extend(self.lower_match_pattern(binding, payload, payload_ty)?);
                }
            }
        }
        Ok(failures)
    }

    fn lower_user_variant_pattern(
        &mut self,
        src: Reg,
        variant: &str,
        bindings: &[MatchPattern],
    ) -> Result<Vec<MatchFailurePatch>, EvalError> {
        // Variant field decls (name + declared type). Bound to a `&'a` slice via a
        // local copy of `self.hir` so `note_scalar`'s `&mut self` is free below.
        let hir = self.hir;
        let field_infos = hir.sum_variant_fields(variant).unwrap_or(&[]);
        let field_names: Vec<String> = field_infos.iter().map(|field| field.name.clone()).collect();
        let match_ip = self.emit(RegInstr::MatchVariant {
            src,
            expected: variant.to_string(),
            match_ip: usize::MAX,
            else_ip: usize::MAX,
        });
        let pass_ip = self.function.code.len();
        self.patch_jump(match_ip, pass_ip);
        let mut failures = vec![MatchFailurePatch::VariantOther(match_ip)];
        if bindings.len() != field_names.len() && !bindings.is_empty() {
            return Err(EvalError::Runtime(format!(
                "reg VM variant `{variant}` pattern binds {} sub-pattern(s) but declares {} field(s).",
                bindings.len(),
                field_names.len()
            )));
        }
        match bindings {
            // A bare variant name (no positional payload) only tests the tag.
            [] => {}
            // Single-payload sugar keeps the `UnwrapVariantValue` projection so the
            // native scalar-replacement pass can still dissolve the variant.
            [binding] => {
                // Single-field variant: the payload type is the sole field's type.
                let payload_ty = field_infos.first().map(|field| field.ty.to_string());
                match binding {
                    MatchPattern::Binding { name, .. } => {
                        let dst = self.local(name);
                        self.emit(RegInstr::UnwrapVariantValue {
                            dst,
                            src,
                            expected: variant.to_string(),
                        });
                        self.note_scalar(dst, payload_ty.as_deref());
                    }
                    MatchPattern::Wildcard(_) => {}
                    _ => {
                        let payload = self.temp();
                        self.emit(RegInstr::UnwrapVariantValue {
                            dst: payload,
                            src,
                            expected: variant.to_string(),
                        });
                        self.note_scalar(payload, payload_ty.as_deref());
                        failures.extend(self.lower_match_pattern(
                            binding,
                            payload,
                            payload_ty.as_deref(),
                        )?);
                    }
                }
            }
            // Positional multi-field binding routes through the same per-field
            // `GetField` projection as `lower_user_struct_variant_pattern`, so the
            // reg-VM and AOT share the struct-variant field-projection semantics.
            _ => {
                for (index, (binding, field_name)) in
                    bindings.iter().zip(field_names.iter()).enumerate()
                {
                    if matches!(binding, MatchPattern::Wildcard(_)) {
                        continue;
                    }
                    let field_ty = field_infos.get(index).map(|field| field.ty.to_string());
                    let field_reg = self.temp();
                    self.emit(RegInstr::GetField {
                        dst: field_reg,
                        base: src,
                        name: field_name.clone(),
                    });
                    self.note_scalar(field_reg, field_ty.as_deref());
                    match binding {
                        MatchPattern::Binding { name, .. } => {
                            let dst = self.local(name);
                            self.emit(RegInstr::Move {
                                dst,
                                src: field_reg,
                            });
                        }
                        _ => {
                            failures.extend(self.lower_match_pattern(
                                binding,
                                field_reg,
                                field_ty.as_deref(),
                            )?);
                        }
                    }
                }
            }
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
        // Struct-variant field decls (name → declared type), decoupled from `self`.
        let hir = self.hir;
        let field_infos = hir.sum_variant_fields(variant).unwrap_or(&[]);
        for field in fields {
            if field.ignored {
                continue;
            }
            let field_ty = field_infos
                .iter()
                .find(|info| info.name == field.name)
                .map(|info| info.ty.to_string());
            let field_reg = self.temp();
            self.emit(RegInstr::GetField {
                dst: field_reg,
                base: src,
                name: field.name.clone(),
            });
            self.note_scalar(field_reg, field_ty.as_deref());
            if let Some(pattern) = field.pattern.as_deref() {
                failures.extend(self.lower_match_pattern(
                    pattern,
                    field_reg,
                    field_ty.as_deref(),
                )?);
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
                MatchPattern::Variant { name, bindings, .. }
                    if name == "Some" && !bindings.is_empty() =>
                {
                    some_binding = bindings.iter().flat_map(MatchPattern::binding_names).next();
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

include!(concat!(env!("OUT_DIR"), "/rss-reg-intrinsic-lookup.rs"));
