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
            #[cfg(feature = "native-jit")]
            let instr_ip = ip;
            ip += 1;
            #[cfg(feature = "native-jit")]
            self.record_native_branch_feedback(func, instr, base, instr_ip)?;
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
            match self.try_exec_pure(instr, base, &mut ip)? {
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
            #[cfg(feature = "native-jit")]
            let instr_ip = ip;
            ip += 1;
            #[cfg(feature = "native-jit")]
            self.record_native_branch_feedback(func, instr, base, instr_ip)?;
            match self.try_exec_pure(instr, base, &mut ip)? {
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

    /// Native self-recursion (native-call-ABI slice 3; generalized in Phase 2):
    /// compile `func` with `CallSelf` via the *general* native subset and run it
    /// natively for SCALAR args (Int/Bool/Float). Marshalling/wrapping is driven by
    /// the compiled parameter/return `NativeTy`s, so a Float (or match/heap-read-bodied)
    /// self-recursive function runs natively — not just the Int-arith whitelist.
    /// Returns the wrapped result on a clean completion, or `None` to fall back
    /// (compile failure, non-scalar param/return, deopt incl. the entry depth-cap
    /// bail that keeps deep recursion off the host stack). Compiled once and cached.
    #[cfg(feature = "native-jit")]
    fn try_native_self_recursive(
        &mut self,
        unit: &RegUnit,
        function_id: usize,
        func: &RegFunction,
        caller_base: usize,
        args: &[usize],
    ) -> Option<VmValue> {
        if !self.native_limits_unarmed() || self.limits.max_depth != DEFAULT_MAX_DEPTH {
            return None;
        }
        let key = func as *const RegFunction as usize;
        let (id, param_tys, ret) = {
            let native = self.native.as_mut()?;
            // Deopt-stress / forced-bail modes run the scalar/interpreter path so the
            // differential stress backends exercise the always-correct fallback.
            if native.force_bail || native.forced_safepoint.is_some() || native.force_all_safepoints
            {
                return None;
            }
            match native.self_recursive_native.get(&key) {
                Some(cached) => cached.clone()?,
                None => {
                    let self_call_sites: std::collections::HashSet<usize> = func
                        .code
                        .iter()
                        .enumerate()
                        .filter_map(|(ip, instr)| {
                            matches!(
                                instr,
                                RegInstr::CallKnown { function, mut_args, .. }
                                    if *function == function_id && mut_args.is_empty()
                            )
                            .then_some(ip)
                        })
                        .collect();
                    // Not self-recursive ⇒ this path doesn't apply (cheap negative
                    // cache; avoids attempting the heavy translate on ordinary calls).
                    let compiled = if self_call_sites.is_empty() {
                        None
                    } else {
                        translate_to_native_jit_with_calls(
                            unit,
                            func,
                            &std::collections::HashMap::new(),
                            &self_call_sites,
                            &std::collections::HashMap::new(),
                        )
                        .and_then(
                            |(jit_fn, ret, param_tys, _literals, _precise)| {
                                // Scalar-only ABI: params and return must be i64/f64
                                // scalars (Int/Bool/Float). Heap (Handle) params/returns
                                // route through the fallback — their cross-call
                                // marshalling/reconstruction is out of scope here.
                                let is_scalar = |t: &NativeTy| {
                                    matches!(t, NativeTy::Int | NativeTy::Bool | NativeTy::Float)
                                };
                                if !is_scalar(&ret) || !param_tys.iter().all(is_scalar) {
                                    return None;
                                }
                                let admission = begin_native_compile(native, 1)?;
                                match native.baseline_module.compile(&jit_fn) {
                                    Ok(id) => {
                                        if finish_native_compile(
                                            native,
                                            admission,
                                            &[id],
                                            NativeCodeTier::Baseline,
                                        ) {
                                            record_native_compile_stats(
                                                native,
                                                id,
                                                &jit_fn,
                                                NativeCodeTier::Baseline,
                                            );
                                            Some((id, param_tys, ret))
                                        } else {
                                            None
                                        }
                                    }
                                    Err(_) => {
                                        finish_native_compile_failure(native, admission);
                                        None
                                    }
                                }
                            },
                        )
                    };
                    native.self_recursive_native.insert(key, compiled.clone());
                    compiled?
                }
            }
        };
        if param_tys.len() != args.len() {
            return None;
        }
        // Marshal scalar args to i64 slots, driven by the compiled parameter type
        // (Float reinterpreted via `to_bits`; an Int value for a Float param is
        // converted first) — identical to the general native call marshalling.
        let mut int_args = Vec::with_capacity(args.len());
        for (&arg, pty) in args.iter().zip(param_tys.iter()) {
            let bits = match (pty, self.reg(caller_base + arg)) {
                (NativeTy::Float, VmValue::Float(f)) => f.to_bits() as i64,
                (NativeTy::Float, VmValue::Int(i)) => (*i as f64).to_bits() as i64,
                (_, VmValue::Int(i)) => *i,
                (_, VmValue::Bool(b)) => i64::from(*b),
                (_, VmValue::Float(f)) => f.to_bits() as i64,
                _ => return None,
            };
            int_args.push(bits);
        }
        let lens = vec![0i64; int_args.len()];
        let mut heap_tx = JitNativeCallFrame::begin();
        let initial_depth = self.frames.len().saturating_add(1);
        let outcome = {
            let native = self.native.as_ref()?;
            native.baseline_module.call_with_host_ctx_at_depth(
                id,
                &int_args,
                &lens,
                heap_tx.host_ctx(),
                &mut [],
                vm_jit::LogicalCallDepth {
                    current: initial_depth,
                    limit: self.limits.max_depth,
                },
            )
        };
        match outcome {
            vm_jit::NativeOutcome::Completed(bits) => {
                heap_tx.commit_scalar_with_writebacks(&[]);
                if let Some(native) = self.native.as_mut()
                    && native.collect_stats
                {
                    native.stats.native_calls += 1;
                    native.stats.baseline_calls += 1;
                }
                Some(match ret {
                    NativeTy::Float => VmValue::Float(f64::from_bits(bits as u64)),
                    NativeTy::Bool => VmValue::Bool(bits != 0),
                    _ => VmValue::Int(bits),
                })
            }
            _ => {
                heap_tx.abort();
                None
            }
        }
    }

    /// Native mutual recursion (native-call-ABI slice 4; generalized to scalar Float):
    /// if `function_id` is part of a mutually-recursive cycle of scalar functions,
    /// compile the whole group together (declare the cycle, then define each) and
    /// dispatch the called member natively. Arg/return marshalling follows each
    /// member's compiled scalar parameter/return `NativeTy`s (Int/Bool/Float — Float
    /// via `to_bits`/`from_bits`), exactly like `try_native_self_recursive`. Returns
    /// the result on a clean completion, or `None` to fall back to the interpreter
    /// (not eligible, non-scalar param/return, or a deopt incl. the depth-cap bail).
    /// The group is compiled once and every member cached.
    #[cfg(feature = "native-jit")]
    pub(in crate::reg_vm) fn try_native_mutual_recursive_int(
        &mut self,
        unit: &RegUnit,
        function_id: usize,
        caller_base: usize,
        args: &[usize],
    ) -> Option<VmValue> {
        if !self.native_limits_unarmed() || self.limits.max_depth != DEFAULT_MAX_DEPTH {
            return None;
        }
        let func = unit.functions.get(function_id)?;
        if args.len() != func.params {
            return None;
        }
        let key = Rc::as_ptr(func) as usize;
        // Resolve the called member's native id, its parameter types (to marshal each
        // scalar arg) and its return type (to wrap the i64 result). Cached per member;
        // compiling any member compiles the whole group.
        let (id, param_tys, ret) = {
            let native = self.native.as_mut()?;
            if native.force_bail || native.forced_safepoint.is_some() || native.force_all_safepoints
            {
                return None;
            }
            if let Some(cached) = native.mutual_recursive_native.get(&key) {
                cached.clone()?
            } else {
                let group = native_recursive_group(unit, function_id);
                let is_scalar =
                    |t: &NativeTy| matches!(t, NativeTy::Int | NativeTy::Bool | NativeTy::Float);
                let compiled = group.as_ref().and_then(|scc| {
                    let index_of: std::collections::HashMap<usize, u32> = scc
                        .iter()
                        .enumerate()
                        .map(|(i, &fid)| (fid, i as u32))
                        .collect();
                    let mut jit_funcs = Vec::with_capacity(scc.len());
                    let mut member_sigs = Vec::with_capacity(scc.len());
                    for &member in scc {
                        let mfunc = unit.functions.get(member)?;
                        let group_call_sites: std::collections::HashMap<usize, u32> = mfunc
                            .code
                            .iter()
                            .enumerate()
                            .filter_map(|(ip, instr)| match instr {
                                RegInstr::CallKnown {
                                    function, mut_args, ..
                                } if mut_args.is_empty() && index_of.contains_key(function) => {
                                    Some((ip, index_of[function]))
                                }
                                _ => None,
                            })
                            .collect();
                        let (jit_fn, ret, param_tys, _l, _pr) = translate_to_native_jit_with_calls(
                            unit,
                            mfunc,
                            &std::collections::HashMap::new(),
                            &std::collections::HashSet::new(),
                            &group_call_sites,
                        )?;
                        // Scalar-only ABI: params and return must be Int/Bool/Float
                        // (each wraps to/from an i64 slot). Heap params/returns decline.
                        if !is_scalar(&ret) || !param_tys.iter().all(is_scalar) {
                            return None;
                        }
                        member_sigs.push((param_tys, ret));
                        jit_funcs.push(jit_fn);
                    }
                    let admission = begin_native_compile(native, jit_funcs.len())?;
                    let ids = match native.baseline_module.compile_recursive_group(&jit_funcs) {
                        Ok(ids) => ids,
                        Err(_) => {
                            finish_native_compile_failure(native, admission);
                            return None;
                        }
                    };
                    if !finish_native_compile(native, admission, &ids, NativeCodeTier::Baseline) {
                        return None;
                    }
                    for (&id, jit_fn) in ids.iter().zip(&jit_funcs) {
                        record_native_compile_stats(native, id, jit_fn, NativeCodeTier::Baseline);
                    }
                    Some((ids, member_sigs))
                });
                match (group, compiled) {
                    (Some(scc), Some((ids, member_sigs))) if ids.len() == scc.len() => {
                        let mut mine = None;
                        for (i, &member) in scc.iter().enumerate() {
                            let mkey = Rc::as_ptr(&unit.functions[member]) as usize;
                            let (param_tys, ret) = member_sigs[i].clone();
                            let entry = (ids[i], param_tys, ret);
                            native
                                .mutual_recursive_native
                                .insert(mkey, Some(entry.clone()));
                            if member == function_id {
                                mine = Some(entry);
                            }
                        }
                        mine?
                    }
                    (group, _) => {
                        // Cache ineligibility (for the whole detected group, or this key).
                        match group {
                            Some(scc) => {
                                for member in scc {
                                    let mkey = Rc::as_ptr(&unit.functions[member]) as usize;
                                    native.mutual_recursive_native.insert(mkey, None);
                                }
                            }
                            None => {
                                native.mutual_recursive_native.insert(key, None);
                            }
                        }
                        return None;
                    }
                }
            }
        };
        if param_tys.len() != args.len() {
            return None;
        }
        let mut int_args = Vec::with_capacity(args.len());
        for (&arg, pty) in args.iter().zip(param_tys.iter()) {
            // Scalar value args marshal to an i64 slot, driven by the compiled
            // parameter type (Float reinterpreted via `to_bits`; an Int value for a
            // Float param converted first) — identical to the self-recursion path.
            let bits = match (pty, self.reg(caller_base + arg)) {
                (NativeTy::Float, VmValue::Float(f)) => f.to_bits() as i64,
                (NativeTy::Float, VmValue::Int(i)) => (*i as f64).to_bits() as i64,
                (_, VmValue::Int(i)) => *i,
                (_, VmValue::Bool(b)) => i64::from(*b),
                (_, VmValue::Float(f)) => f.to_bits() as i64,
                _ => return None,
            };
            int_args.push(bits);
        }
        let lens = vec![0i64; int_args.len()];
        let mut heap_tx = JitNativeCallFrame::begin();
        let initial_depth = self.frames.len().saturating_add(1);
        let outcome = {
            let native = self.native.as_ref()?;
            native.baseline_module.call_with_host_ctx_at_depth(
                id,
                &int_args,
                &lens,
                heap_tx.host_ctx(),
                &mut [],
                vm_jit::LogicalCallDepth {
                    current: initial_depth,
                    limit: self.limits.max_depth,
                },
            )
        };
        match outcome {
            vm_jit::NativeOutcome::Completed(bits) => {
                heap_tx.commit_scalar_with_writebacks(&[]);
                if let Some(native) = self.native.as_mut()
                    && native.collect_stats
                {
                    native.stats.native_calls += 1;
                    native.stats.baseline_calls += 1;
                }
                Some(match ret {
                    NativeTy::Float => VmValue::Float(f64::from_bits(bits as u64)),
                    NativeTy::Bool => VmValue::Bool(bits != 0),
                    _ => VmValue::Int(bits),
                })
            }
            _ => {
                heap_tx.abort();
                None
            }
        }
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
        let returns_bool = match self_recursive_scalar_jit_candidate(
            unit,
            &mut self.jit_state,
            function_id,
        ) {
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
                    #[cfg(feature = "native-jit")]
                    if self.native.is_some() {
                        record_branch_site(&func, frame.ip - 1, taken);
                    }
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
                    #[cfg(feature = "native-jit")]
                    if self.native.is_some() {
                        record_branch_site(&func, frame.ip - 1, taken);
                    }
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
