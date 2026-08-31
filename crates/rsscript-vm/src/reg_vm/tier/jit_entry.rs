use super::recursion::*;
use super::*;

impl RegVm {
    /// Tier-0 JIT executor for a JIT-eligible function. Runs the body via the
    /// same shared helpers (`eval_numeric_binary`, `eval_numeric_compare`, …) and
    /// register methods (`reg`/`set_reg`/`take_reg`) the interpreter uses, so its
    /// result is identical to `drive` by construction.
    ///
    /// Eligibility guarantees the function (and its whole reachable call graph) is
    /// non-suspending and non-recursive (see [`compute_jit_eligibility`]), so a
    /// `CallKnown` can be run to completion synchronously via `run_frame` without
    /// ever suspending or unbounded host-stack growth. All other instructions are
    /// pure and go through [`Self::try_exec_pure`].
    pub(in crate::reg_vm) fn run_jit(
        &mut self,
        unit: &RegUnit,
        func: &RegFunction,
        base: usize,
    ) -> Result<VmValue, EvalError> {
        let mut ip = 0usize;
        while let Some(instr) = func.code.get(ip) {
            self.tick()?;
            ip += 1;
            // Cross-function call: eligibility proved the callee cannot suspend and
            // the call graph is acyclic, so drive it to completion on a fresh frame
            // window above ours, exactly like `drive`'s `CallKnown` but synchronous.
            if let RegInstr::CallKnown {
                dst,
                function: callee_id,
                args,
                mut_args,
            } = instr
            {
                let callee_depth = self.frames.len().saturating_add(1);
                if callee_depth > self.limits.max_depth {
                    let max_depth = self.limits.max_depth;
                    return Err(EvalError::Runtime(format!(
                        "recursion depth limit exceeded ({max_depth} frames)"
                    )));
                }
                let callee = Rc::clone(&unit.functions[*callee_id]);
                let next_base = base + func.regs;
                self.prepare_frame(next_base, callee.regs)?;
                for (index, reg) in args.iter().enumerate() {
                    let value = self.reg(base + *reg).clone();
                    self.set_reg(next_base + index, value);
                }
                let result =
                    if mut_args.is_empty() && callee.code.iter().all(jit_supported_instruction) {
                        self.run_jit_pure_leaf(&callee, next_base)?
                    } else {
                        self.run_frame(unit, callee, next_base)?
                    };
                // Propagate `mut` parameters back to the caller's argument regs.
                for &pos in mut_args {
                    let value = self.reg(next_base + pos).clone();
                    self.set_reg(base + args[pos], value);
                }
                self.set_reg(base + *dst, result);
                continue;
            }
            match self.try_exec_pure(
                instr,
                base,
                &mut ip,
                None,
            )? {
                PureStep::Next => {}
                PureStep::Return(value) => return Ok(value),
                // Eligibility guarantees only pure instructions (and the
                // `CallKnown` handled above) reach here; `NotPure` is an internal
                // bug.
                PureStep::NotPure => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM JIT reached non-eligible instruction `{instr:?}`."
                    )));
                }
            }
        }
        Ok(VmValue::Unit)
    }

    fn run_jit_pure_leaf(&mut self, func: &RegFunction, base: usize) -> Result<VmValue, EvalError> {
        let mut ip = 0usize;
        while let Some(instr) = func.code.get(ip) {
            self.tick()?;
            ip += 1;
            match self.try_exec_pure(
                instr,
                base,
                &mut ip,
                None,
            )? {
                PureStep::Next => {}
                PureStep::Return(value) => return Ok(value),
                PureStep::NotPure => {
                    return Err(EvalError::Runtime(format!(
                        "reg VM JIT leaf fast path reached non-pure instruction `{instr:?}`."
                    )));
                }
            }
        }
        Ok(VmValue::Unit)
    }


    #[cfg(feature = "native-jit")]
    fn try_native_self_recursive(
        &mut self,
        _unit: &RegUnit,
        _function_id: usize,
        _func: &RegFunction,
        _caller_base: usize,
        _args: &[usize],
    ) -> Option<VmValue> {
        None
    }


    #[cfg(feature = "native-jit")]
    pub(in crate::reg_vm) fn try_native_mutual_recursive_int(
        &mut self,
        _unit: &RegUnit,
        _function_id: usize,
        _caller_base: usize,
        _args: &[usize],
    ) -> Option<VmValue> {
        None
    }

    pub(in crate::reg_vm) fn run_jit_self_recursive_int(
        &mut self,
        unit: &RegUnit,
        function_id: usize,
        caller_base: usize,
        args: &[usize],
    ) -> Result<Option<VmValue>, EvalError> {
        let func = Rc::clone(&unit.functions[function_id]);
        if args.len() != func.params {
            return Ok(None);
        }

        // General native fast path (native-call-ABI slice 3, generalized in Phase 2):
        // compile + run this function natively with self-recursive `CallSelf` via the
        // general native subset, which admits scalar Int/Bool/Float bodies (incl.
        // `match` and heap reads), not just the Int-arith whitelist. Marshalling and
        // result wrapping follow the compiled scalar parameter/return types. On a
        // clean completion return the result; otherwise fall through to the fallback
        // below (compile failure, non-scalar shape, or a depth-cap/deopt bail).
        #[cfg(feature = "native-jit")]
        if let Some(value) =
            self.try_native_self_recursive(unit, function_id, func.as_ref(), caller_base, args)
        {
            return Ok(Some(value));
        }

        // Fallback selection. The i64 tier-0 scalar executor can run only an
        // i64-representable (Int/Bool) Int-arith body — exactly what the scalar
        // candidate recognises; it handles arbitrary depth without touching the host
        // C stack. A non-i64 body (Float, or one using `match`/heap that the i64
        // machine cannot run) routes to the full interpreter (`Ok(None)`).
        let returns_bool =
            match self_recursive_scalar_jit_candidate(unit, &mut self.jit_state, function_id) {
                SelfRecursionKind::Ineligible => return Ok(None),
                SelfRecursionKind::Int => false,
                SelfRecursionKind::Bool => true,
            };

        // Marshal the scalar value args to i64 (Bool as 0/1). Both Int and Bool
        // params are i64-representable by the tier-0 machine.
        let mut stack = vec![0i64; func.regs];
        for (param, arg) in args.iter().enumerate() {
            let bits = match self.reg(caller_base + *arg) {
                VmValue::Int(value) => *value,
                VmValue::Bool(value) => i64::from(*value),
                _ => return Ok(None),
            };
            stack[param] = bits;
        }
        // Wrap an i64 result per the function's return kind.
        let wrap = |bits: i64| {
            if returns_bool {
                VmValue::Bool(bits != 0)
            } else {
                VmValue::Int(bits)
            }
        };

        #[derive(Debug, Clone, Copy)]
        struct ScalarFrame {
            ip: usize,
            base: usize,
            ret_dst: Option<usize>,
        }

        let mut frames: Vec<ScalarFrame> = Vec::new();
        if self.frames.len() + 1 > self.limits.max_depth {
            let max_depth = self.limits.max_depth;
            return Err(EvalError::Runtime(format!(
                "recursion depth limit exceeded ({max_depth} frames)"
            )));
        }
        frames.push(ScalarFrame {
            ip: 0,
            base: 0,
            ret_dst: None,
        });

        loop {
            let Some(frame) = frames.last_mut() else {
                return Ok(Some(VmValue::Unit));
            };
            let Some(instr) = func.code.get(frame.ip) else {
                let frame = frames.pop().expect("active scalar frame");
                stack.truncate(frame.base);
                if let Some(ret_dst) = frame.ret_dst {
                    stack[ret_dst] = 0;
                    continue;
                }
                return Ok(Some(VmValue::Unit));
            };
            self.tick()?;
            let base = frame.base;
            frame.ip += 1;
            match instr {
                RegInstr::LoadUnit { dst } => stack[base + *dst] = 0,
                RegInstr::LoadInt { dst, value } => stack[base + *dst] = *value,
                RegInstr::LoadBool { dst, value } => stack[base + *dst] = i64::from(*value),
                RegInstr::Move { dst, src } => {
                    stack[base + *dst] = stack[base + *src];
                }
                RegInstr::DeepCopy { .. } | RegInstr::DeepCopyElided { .. } => {
                    // Primitive scalar values are already independent; the bytecode
                    // instruction exists for heap/value isolation in the generic VM.
                }
                RegInstr::AddInt { dst, lhs, rhs } => {
                    stack[base + *dst] = stack[base + *lhs]
                        .checked_add(stack[base + *rhs])
                        .ok_or_else(|| {
                            int_overflow_error("addition", stack[base + *lhs], stack[base + *rhs])
                        })?;
                }
                RegInstr::SubInt { dst, lhs, rhs } => {
                    stack[base + *dst] = stack[base + *lhs]
                        .checked_sub(stack[base + *rhs])
                        .ok_or_else(|| {
                            int_overflow_error(
                                "subtraction",
                                stack[base + *lhs],
                                stack[base + *rhs],
                            )
                        })?;
                }
                RegInstr::MulInt { dst, lhs, rhs } => {
                    stack[base + *dst] = stack[base + *lhs]
                        .checked_mul(stack[base + *rhs])
                        .ok_or_else(|| {
                            int_overflow_error(
                                "multiplication",
                                stack[base + *lhs],
                                stack[base + *rhs],
                            )
                        })?;
                }
                RegInstr::DivInt { dst, lhs, rhs } => {
                    let rhs_value = stack[base + *rhs];
                    if rhs_value == 0 {
                        return Err(EvalError::Runtime("integer division by zero".to_string()));
                    }
                    stack[base + *dst] =
                        stack[base + *lhs].checked_div(rhs_value).ok_or_else(|| {
                            int_overflow_error("division", stack[base + *lhs], rhs_value)
                        })?;
                }
                RegInstr::ModInt { dst, lhs, rhs } => {
                    let rhs_value = stack[base + *rhs];
                    if rhs_value == 0 {
                        return Err(EvalError::Runtime("integer modulo by zero".to_string()));
                    }
                    stack[base + *dst] =
                        stack[base + *lhs].checked_rem(rhs_value).ok_or_else(|| {
                            int_overflow_error("modulo", stack[base + *lhs], rhs_value)
                        })?;
                }
                RegInstr::LessInt { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] < stack[base + *rhs]);
                }
                RegInstr::LessEqualInt { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] <= stack[base + *rhs]);
                }
                RegInstr::GreaterInt { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] > stack[base + *rhs]);
                }
                RegInstr::GreaterEqualInt { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] >= stack[base + *rhs]);
                }
                RegInstr::Equal { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] == stack[base + *rhs]);
                }
                RegInstr::NotEqual { dst, lhs, rhs } => {
                    stack[base + *dst] = i64::from(stack[base + *lhs] != stack[base + *rhs]);
                }
                RegInstr::Jump { target } => {
                    frames.last_mut().expect("active scalar frame").ip = *target;
                }
                RegInstr::JumpIfBool {
                    cond,
                    expected,
                    target,
                } => {
                    let taken = (stack[base + *cond] != 0) == *expected;
                    if taken {
                        frames.last_mut().expect("active scalar frame").ip = *target;
                    }
                }
                RegInstr::JumpIfIntCompare {
                    lhs,
                    rhs,
                    op,
                    expected,
                    target,
                } => {
                    let taken =
                        eval_int_compare(*op, stack[base + *lhs], stack[base + *rhs]) == *expected;
                    if taken {
                        frames.last_mut().expect("active scalar frame").ip = *target;
                    }
                }
                RegInstr::CallKnown {
                    dst,
                    function,
                    args,
                    mut_args,
                } if *function == function_id
                    && mut_args.is_empty()
                    && args.len() == func.params =>
                {
                    if self.frames.len() + frames.len() + 1 > self.limits.max_depth {
                        let max_depth = self.limits.max_depth;
                        return Err(EvalError::Runtime(format!(
                            "recursion depth limit exceeded ({max_depth} frames)"
                        )));
                    }
                    let ret_dst = base + *dst;
                    let callee_base = stack.len();
                    stack.resize(callee_base + func.regs, 0);
                    for (param, arg) in args.iter().enumerate() {
                        stack[callee_base + param] = stack[base + *arg];
                    }
                    frames.push(ScalarFrame {
                        ip: 0,
                        base: callee_base,
                        ret_dst: Some(ret_dst),
                    });
                }
                RegInstr::Return { src } => {
                    let value = stack[base + *src];
                    let frame = frames.pop().expect("active scalar frame");
                    stack.truncate(frame.base);
                    if let Some(ret_dst) = frame.ret_dst {
                        stack[ret_dst] = value;
                    } else {
                        return Ok(Some(wrap(value)));
                    }
                }
                _ => return Ok(None),
            }
        }
    }
}
