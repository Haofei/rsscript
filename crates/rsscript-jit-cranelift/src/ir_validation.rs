use crate::validated::ValidationFacts;

fn check_zero_init_reg(program: &JitFunction, reg: u32) -> Result<(), JitError> {
    if reg >= program.n_regs {
        return Err(JitError::invalid_ir(format!(
            "zero-initialized register {reg} is out of range (n_regs {})",
            program.n_regs
        )));
    }
    if reg < program.n_params {
        return Err(JitError::invalid_ir(format!(
            "parameter register {reg} cannot be declared zero-initialized"
        )));
    }
    if matches!(
        program.reg_types[reg as usize],
        JitValueType::Handle
            | JitValueType::FlatInt
            | JitValueType::FlatIntMut
            | JitValueType::FlatFloat
            | JitValueType::FlatFloatMut
    ) {
        return Err(JitError::invalid_ir(format!(
            "zero-initialized register {reg} must have a scalar type"
        )));
    }
    Ok(())
}

fn validate_memo_scopes(
    program: &JitFunction,
    memo_slot_owners: &[Option<usize>],
) -> Result<(), JitError> {
    let n = program.code.len();
    let mut slot_scopes = vec![None; memo_slot_owners.len()];

    for (scope_index, scope) in program.memo_scopes.iter().enumerate() {
        let header = scope.header as usize;
        let exit = scope.exit as usize;
        if header >= exit || exit >= n {
            return Err(JitError::invalid_ir(format!(
                "memo scope {scope_index}: expected 0 <= header < exit < code length, got [{header}, {exit}) for {n} instructions"
            )));
        }
        if scope.memo_slots.is_empty() {
            return Err(JitError::invalid_ir(format!(
                "memo scope {scope_index}: memo_slots cannot be empty"
            )));
        }

        let mut has_backedge = false;
        for (source, _) in program.code.iter().enumerate() {
            let source_in_scope = source >= header && source < exit;
            for target in successors(program, source) {
                let target_in_scope = target >= header && target < exit;
                if !source_in_scope && target_in_scope && target != header {
                    return Err(JitError::invalid_ir(format!(
                        "memo scope {scope_index}: external edge {source} -> {target} enters scope interior"
                    )));
                }
                if source_in_scope && !target_in_scope && target != exit {
                    return Err(JitError::invalid_ir(format!(
                        "memo scope {scope_index}: edge {source} -> {target} leaves scope anywhere other than exit {exit}"
                    )));
                }
                if source_in_scope && target == header {
                    if !matches!(
                        program.code[source],
                        JitInstr::Jump { target } if target as usize == header
                    ) {
                        return Err(JitError::invalid_ir(format!(
                            "memo scope {scope_index}: backedge {source} -> {header} must be an unconditional Jump"
                        )));
                    }
                    has_backedge = true;
                }
            }
        }
        if !has_backedge {
            return Err(JitError::invalid_ir(format!(
                "memo scope {scope_index}: no backedge targets header {header}"
            )));
        }

        for &slot in &scope.memo_slots {
            let Some(owner) = memo_slot_owners.get(slot as usize).and_then(|owner| *owner) else {
                return Err(JitError::invalid_ir(format!(
                    "memo scope {scope_index}: memo_slot {slot} has no memoized call site"
                )));
            };
            let Some(previous_scope) = slot_scopes.get_mut(slot as usize) else {
                return Err(JitError::invalid_ir(format!(
                    "memo scope {scope_index}: memo_slot {slot} is out of range"
                )));
            };
            if let Some(previous_scope) = previous_scope {
                return Err(JitError::invalid_ir(format!(
                    "memo_slot {slot} belongs to both memo scopes {previous_scope} and {scope_index}"
                )));
            }
            if owner < header || owner >= exit {
                return Err(JitError::invalid_ir(format!(
                    "memo scope {scope_index}: memo_slot {slot} site {owner} is outside [{header}, {exit})"
                )));
            }
            *previous_scope = Some(scope_index);
        }
    }

    for (slot, scope) in slot_scopes.iter().enumerate() {
        if scope.is_none() {
            return Err(JitError::invalid_ir(format!(
                "MemoizedHostCall memo_slot {slot} does not belong to a memo scope"
            )));
        }
    }
    // Validate the interval family in O(S log S), not with an O(S²) pairwise
    // comparison. Sorting outer scopes before inner scopes makes a simple stack
    // sufficient to reject crossing intervals and equal-header pseudo-nesting.
    let mut ordered: Vec<usize> = (0..program.memo_scopes.len()).collect();
    ordered.sort_unstable_by_key(|&index| {
        let scope = &program.memo_scopes[index];
        (scope.header, std::cmp::Reverse(scope.exit))
    });
    let mut stack: Vec<usize> = Vec::new();
    for index in ordered {
        let scope = &program.memo_scopes[index];
        while stack
            .last()
            .is_some_and(|&parent| program.memo_scopes[parent].exit <= scope.header)
        {
            stack.pop();
        }
        if let Some(&parent_index) = stack.last() {
            let parent = &program.memo_scopes[parent_index];
            if scope.header <= parent.header || scope.exit > parent.exit {
                return Err(JitError::invalid_ir(format!(
                    "memo scopes {parent_index} [{}, {}) and {index} [{}, {}) overlap without strict nesting",
                    parent.header, parent.exit, scope.header, scope.exit
                )));
            }
        }
        stack.push(index);
    }
    Ok(())
}

/// Validate public IR before codegen. `build_function` assumes well-formed input
/// (it indexes `reg_types`/`code` directly and relies on Cranelift register types
/// matching each opcode); this turns every such assumption into a clean
/// [`JitError`] so a buggy producer can never reach codegen with input that would
/// panic or generate invalid assumptions.
///
/// Storage-class rules mirror `build_function`'s lowering: arithmetic preserves the
/// operand class (and forbids `Handle`), the int-only ops (`Mod`, bit/shift) require
/// `Int`, comparisons yield a logical `Bool`, and `Handle` registers are only valid
/// as the `base` of a heap read.
#[cfg(test)]
pub(crate) fn validate(program: &JitFunction, osr: bool) -> Result<(), JitError> {
    validate_with_limits(program, osr, &JitLimits::default()).map(|_| ())
}

pub(crate) fn validate_with_limits(
    program: &JitFunction,
    osr: bool,
    limits: &JitLimits,
) -> Result<ValidationFacts, JitError> {
    let n_regs = program.n_regs as usize;
    let n = program.code.len();

    if program.reg_types.len() != n_regs {
        return Err(JitError::invalid_ir(format!(
            "reg_types length {} does not match n_regs {n_regs}",
            program.reg_types.len()
        )));
    }
    if program.n_params > program.n_regs {
        return Err(JitError::invalid_ir(format!(
            "n_params {} exceeds n_regs {n_regs}",
            program.n_params
        )));
    }
    if n_regs > limits.max_registers {
        return Err(JitError::invalid_ir(format!(
            "n_regs {n_regs} exceeds the JIT limit {}",
            limits.max_registers
        )));
    }
    if program.n_params as usize > limits.max_parameters {
        return Err(JitError::invalid_ir(format!(
            "n_params {} exceeds the JIT limit {}",
            program.n_params, limits.max_parameters
        )));
    }
    if n > limits.max_instructions {
        return Err(JitError::invalid_ir(format!(
            "code length {n} exceeds the JIT limit {}",
            limits.max_instructions
        )));
    }
    let analysis_cells = n_regs.checked_mul(n).ok_or_else(|| {
        JitError::invalid_ir(format!(
            "JIT analysis dimensions overflow: {n} instructions x {n_regs} registers"
        ))
    })?;
    if analysis_cells > limits.max_analysis_cells {
        return Err(JitError::invalid_ir(format!(
            "JIT analysis size {analysis_cells} cells exceeds the limit {} \
             ({n} instructions x {n_regs} registers)",
            limits.max_analysis_cells
        )));
    }
    let mut cfg_edges = 0usize;
    let mut total_operands = 0usize;
    for (ip, instr) in program.code.iter().enumerate() {
        cfg_edges = cfg_edges
            .checked_add(successors(program, ip).len())
            .ok_or_else(|| JitError::invalid_ir("JIT CFG edge count overflow"))?;
        instr.visit_used_registers(|_| total_operands = total_operands.saturating_add(1));
    }
    if cfg_edges > limits.max_cfg_edges {
        return Err(JitError::invalid_ir(format!(
            "JIT CFG has {cfg_edges} edges, exceeding the limit {}",
            limits.max_cfg_edges
        )));
    }
    if total_operands > limits.max_total_operands {
        return Err(JitError::invalid_ir(format!(
            "JIT IR has {total_operands} register operands, exceeding the limit {}",
            limits.max_total_operands
        )));
    }
    if program.memo_scopes.len() > limits.max_memo_scopes {
        return Err(JitError::invalid_ir(format!(
            "JIT IR has {} memo scopes, exceeding the limit {}",
            program.memo_scopes.len(),
            limits.max_memo_scopes
        )));
    }
    let work = limits
        .checked_work(
            n,
            n_regs,
            cfg_edges,
            total_operands,
            program.memo_scopes.len(),
        )
        .ok_or_else(|| JitError::invalid_ir("JIT compilation work estimate overflow"))?;
    if work > limits.max_ir_work_units {
        return Err(JitError::invalid_ir(format!(
            "JIT compilation work estimate {work} exceeds the limit {}",
            limits.max_ir_work_units
        )));
    }
    for &reg in &program.zero_init_regs {
        check_zero_init_reg(program, reg)?;
    }
    for &cold_ip in &program.cold_blocks {
        if (cold_ip as usize) >= n {
            return Err(JitError::invalid_ir(format!(
                "cold block instruction {cold_ip} is out of range for {n} instructions"
            )));
        }
    }

    let check_reg = |r: u32| -> Result<(), JitError> {
        if (r as usize) < n_regs {
            Ok(())
        } else {
            Err(JitError::invalid_ir(format!(
                "register {r} out of range (n_regs {n_regs})"
            )))
        }
    };
    let class = |r: u32| program.reg_types[r as usize];
    // Non-scalar register classes (opaque handle or flat-array pointer): valid only
    // as the `base` of a heap/direct read, never in scalar/arith/move/return.
    let is_nonscalar = |r: u32| {
        matches!(
            class(r),
            JitValueType::Handle
                | JitValueType::FlatInt
                | JitValueType::FlatIntMut
                | JitValueType::FlatFloat
                | JitValueType::FlatFloatMut
        )
    };
    let check_target = |t: u32| -> Result<(), JitError> {
        if (t as usize) < n {
            Ok(())
        } else {
            Err(JitError::invalid_ir(format!(
                "jump target {t} out of range (code length {n})"
            )))
        }
    };

    // Two operands of the same scalar (non-`Handle`) class: the shape every
    // arithmetic/comparison opcode requires.
    let scalar_pair = |lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        check_reg(lhs)?;
        check_reg(rhs)?;
        if is_nonscalar(lhs) || is_nonscalar(rhs) {
            return Err(JitError::invalid_ir(format!(
                "{op}: operand is a non-scalar (Handle/flat) register"
            )));
        }
        if class(lhs) != class(rhs) {
            return Err(JitError::invalid_ir(format!(
                "{op}: operand classes differ ({:?} vs {:?})",
                class(lhs),
                class(rhs)
            )));
        }
        Ok(())
    };
    let numeric_pair = |lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        scalar_pair(lhs, rhs, op)?;
        if !matches!(class(lhs), JitValueType::Int | JitValueType::Float) {
            return Err(JitError::invalid_ir(format!(
                "{op}: operands must be Int or Float"
            )));
        }
        Ok(())
    };
    // Arithmetic: result register has the operands' class.
    let arith = |dst: u32, lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        numeric_pair(lhs, rhs, op)?;
        check_reg(dst)?;
        if class(dst) != class(lhs) {
            return Err(JitError::invalid_ir(format!(
                "{op}: result {:?} does not match operands {:?}",
                class(dst),
                class(lhs)
            )));
        }
        Ok(())
    };
    // Integer-only ternary (Mod, bitwise, shift): every register must be `Int`.
    let int_op = |dst: u32, lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        check_reg(dst)?;
        check_reg(lhs)?;
        check_reg(rhs)?;
        for r in [dst, lhs, rhs] {
            if class(r) != JitValueType::Int {
                return Err(JitError::invalid_ir(format!(
                    "{op}: register {r} must be Int"
                )));
            }
        }
        Ok(())
    };
    // Comparison: operands share a scalar class, result is a logical Bool.
    let compare = |dst: u32, lhs: u32, rhs: u32, op: &str| -> Result<(), JitError> {
        numeric_pair(lhs, rhs, op)?;
        check_reg(dst)?;
        if class(dst) != JitValueType::Bool {
            return Err(JitError::invalid_ir(format!(
                "{op}: boolean result must be Bool"
            )));
        }
        Ok(())
    };
    let require_class = |r: u32, want: JitValueType, op: &str| -> Result<(), JitError> {
        check_reg(r)?;
        if class(r) != want {
            return Err(JitError::invalid_ir(format!(
                "{op}: register {r} is {:?}, expected {want:?}",
                class(r)
            )));
        }
        Ok(())
    };
    // A flat-array base must be the expected flat class *and* enter through the
    // caller's args/lens window (never produced internally). For a normal compile the
    // window is the packed params (`0..n_params`); for an OSR-entry the window is the
    // full `n_regs`-wide register file (args/lens are both `n_regs`-wide and loaded by
    // register index — see the OSR `load_set`), so any window register may carry a
    // flat pointer the host marshalled and bounds-checked against the parallel `lens`
    // slot. The host's borrow protocol pins the buffer for the call's duration either
    // way.
    let require_flat_param = |r: u32, want: JitValueType, op: &str| -> Result<(), JitError> {
        check_reg(r)?;
        let actual = class(r);
        let type_matches = actual == want
            || matches!(
                (want, actual),
                (JitValueType::FlatInt, JitValueType::FlatIntMut)
                    | (JitValueType::FlatFloat, JitValueType::FlatFloatMut)
            );
        if !type_matches {
            return Err(JitError::invalid_ir(format!(
                "{op}: register {r} is {actual:?}, expected {want:?}"
            )));
        }
        let window = if osr {
            program.n_regs as usize
        } else {
            program.n_params as usize
        };
        if (r as usize) >= window {
            return Err(JitError::invalid_ir(format!(
                "{op}: register {r} is not a parameter"
            )));
        }
        Ok(())
    };

    let mut returns = Vec::new();
    let memo_count = program
        .code
        .iter()
        .filter(|instr| matches!(instr, JitInstr::MemoizedHostCall { .. }))
        .count();
    if memo_count > limits.max_memo_slots {
        return Err(JitError::invalid_ir(format!(
            "JIT IR has {memo_count} memo slots, exceeding the limit {}",
            limits.max_memo_slots
        )));
    }
    let mut memo_slot_owners = vec![None; memo_count];
    for (ip, instr) in program.code.iter().enumerate() {
        let JitInstr::MemoizedHostCall { memo_slot, .. } = instr else {
            continue;
        };
        let Some(owner) = memo_slot_owners.get_mut(*memo_slot as usize) else {
            return Err(JitError::invalid_ir(format!(
                "MemoizedHostCall at instruction {ip}: memo_slot {memo_slot} is out of range for {memo_count} memoization sites"
            )));
        };
        if let Some(owner) = owner {
            return Err(JitError::invalid_ir(format!(
                "MemoizedHostCall memo_slot {memo_slot} is shared by instructions {owner} and {ip}"
            )));
        }
        *owner = Some(ip);
    }
    for (i, instr) in program.code.iter().enumerate() {
        // Conditional branches fall through to `i + 1` (`build_function` indexes
        // `block_for[i + 1]`), so the instruction must not be the last one.
        let check_fallthrough = || -> Result<(), JitError> {
            if i + 1 < n {
                Ok(())
            } else {
                Err(JitError::invalid_ir(format!(
                    "conditional branch at {i} has no fall-through instruction"
                )))
            }
        };
        match instr {
            JitInstr::Nop | JitInstr::TailCallGuard { .. } | JitInstr::Bail => {}
            JitInstr::LoadInt { dst, .. } => require_class(*dst, JitValueType::Int, "LoadInt")?,
            JitInstr::LoadFloat { dst, .. } => {
                require_class(*dst, JitValueType::Float, "LoadFloat")?
            }
            JitInstr::LoadBool { dst, .. } => require_class(*dst, JitValueType::Bool, "LoadBool")?,
            JitInstr::Move { dst, src } => {
                check_reg(*dst)?;
                check_reg(*src)?;
                // A **Handle** register is a plain `i64` (a heap-table index), so a
                // Move just copies it — sound for a stored closure handle threaded
                // through a temporary (Pending #1). A **flat-array** register (pointer
                // + length, keyed by reg in the `lens` slice) genuinely cannot be
                // moved, so those stay rejected.
                let is_flat = |r: u32| is_flat_type(class(r));
                if is_flat(*src) || is_flat(*dst) {
                    return Err(JitError::invalid_ir(
                        "Move: flat-array registers cannot be moved",
                    ));
                }
                if class(*dst) != class(*src) {
                    return Err(JitError::invalid_ir(format!(
                        "Move: classes differ ({:?} vs {:?})",
                        class(*dst),
                        class(*src)
                    )));
                }
            }
            JitInstr::Add { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Add")?,
            JitInstr::Sub { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Sub")?,
            JitInstr::Mul { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Mul")?,
            JitInstr::Div { dst, lhs, rhs } => arith(*dst, *lhs, *rhs, "Div")?,
            JitInstr::Mod { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "Mod")?,
            JitInstr::IntToFloat { dst, src } => {
                require_class(*src, JitValueType::Int, "IntToFloat src")?;
                require_class(*dst, JitValueType::Float, "IntToFloat result")?;
            }
            JitInstr::FloatToInt { dst, src, .. } => {
                require_class(*src, JitValueType::Float, "FloatToInt src")?;
                require_class(*dst, JitValueType::Int, "FloatToInt result")?;
            }
            JitInstr::HostCall { helper, dst, args } => {
                let sig = helper.signature();
                if sig.found_out {
                    return Err(JitError::invalid_ir(format!(
                        "HostCall {helper:?}: helper has a private found output"
                    )));
                }
                if args.len() != sig.args.len() {
                    return Err(JitError::invalid_ir(format!(
                        "HostCall {helper:?}: got {} args, expected {}",
                        args.len(),
                        sig.args.len()
                    )));
                }
                for (i, (arg, expected)) in args.iter().zip(sig.args.iter()).enumerate() {
                    match arg {
                        HostArg::Reg(reg) => {
                            require_class(*reg, *expected, &format!("HostCall {helper:?} arg {i}"))?
                        }
                        HostArg::ImmI64(_) => {
                            if *expected != JitValueType::Int {
                                return Err(JitError::invalid_ir(format!(
                                    "HostCall {helper:?} arg {i}: immediate used for {expected:?}"
                                )));
                            }
                        }
                    }
                }
                match sig.result {
                    HostResult::Exact(result) => {
                        check_reg(*dst)?;
                        if class(*dst) != result {
                            return Err(JitError::invalid_ir(format!(
                                "HostCall {helper:?} result: register {dst} is {:?}, expected {result:?}",
                                class(*dst)
                            )));
                        }
                    }
                    HostResult::IntOrFloatBits => {
                        check_reg(*dst)?;
                        if !matches!(class(*dst), JitValueType::Int | JitValueType::Float) {
                            return Err(JitError::invalid_ir(format!(
                                "HostCall {helper:?} result: register {dst} is {:?}, expected Int or Float",
                                class(*dst)
                            )));
                        }
                    }
                }
            }
            JitInstr::MemoizedHostCall {
                helper, dst, args, ..
            } => {
                let sig = helper.signature();
                if sig.found_out {
                    return Err(JitError::invalid_ir(format!(
                        "MemoizedHostCall {helper:?}: helper has a private found output"
                    )));
                }
                if helper.heap_effect().writes_existing_heap() {
                    return Err(JitError::invalid_ir(format!(
                        "MemoizedHostCall {helper:?}: heap-writing helpers cannot be memoized"
                    )));
                }
                if args.len() != sig.args.len() {
                    return Err(JitError::invalid_ir(format!(
                        "MemoizedHostCall {helper:?}: got {} args, expected {}",
                        args.len(),
                        sig.args.len()
                    )));
                }
                for (i, (arg, expected)) in args.iter().zip(sig.args.iter()).enumerate() {
                    match arg {
                        HostArg::Reg(reg) => require_class(
                            *reg,
                            *expected,
                            &format!("MemoizedHostCall {helper:?} arg {i}"),
                        )?,
                        HostArg::ImmI64(_) => {
                            if *expected != JitValueType::Int {
                                return Err(JitError::invalid_ir(format!(
                                    "MemoizedHostCall {helper:?} arg {i}: immediate used for {expected:?}"
                                )));
                            }
                        }
                    }
                }
                match sig.result {
                    HostResult::Exact(result) => {
                        if !matches!(
                            result,
                            JitValueType::Int | JitValueType::Bool | JitValueType::Float
                        ) {
                            return Err(JitError::invalid_ir(format!(
                                "MemoizedHostCall {helper:?}: result must be a scalar"
                            )));
                        }
                        require_class(
                            *dst,
                            result,
                            &format!("MemoizedHostCall {helper:?} result"),
                        )?;
                    }
                    HostResult::IntOrFloatBits => {
                        check_reg(*dst)?;
                        if !matches!(class(*dst), JitValueType::Int | JitValueType::Float) {
                            return Err(JitError::invalid_ir(format!(
                                "MemoizedHostCall {helper:?} result: register {dst} is {:?}, expected Int or Float",
                                class(*dst)
                            )));
                        }
                    }
                }
            }
            JitInstr::CallNative { dst, args, .. } => {
                check_reg(*dst)?;
                if is_flat_type(class(*dst)) {
                    return Err(JitError::invalid_ir(format!(
                        "CallNative result register {dst} is a flat-array register"
                    )));
                }
                for arg in args {
                    check_reg(*arg)?;
                }
            }
            JitInstr::CallSelf { dst, args } => {
                check_reg(*dst)?;
                if is_flat_type(class(*dst)) {
                    return Err(JitError::invalid_ir(format!(
                        "CallSelf result register {dst} is a flat-array register"
                    )));
                }
                // A self-call invokes THIS function: arity and arg/result classes must
                // match its own signature (params are regs `0..n_params`).
                if args.len() != program.n_params as usize {
                    return Err(JitError::invalid_ir(format!(
                        "CallSelf got {} args, function expects {}",
                        args.len(),
                        program.n_params
                    )));
                }
                for (i, arg) in args.iter().enumerate() {
                    check_reg(*arg)?;
                    let expected = program.reg_types[i];
                    if class(*arg) != expected {
                        return Err(JitError::invalid_ir(format!(
                            "CallSelf arg {i}: register {arg} is {:?}, function param is {expected:?}",
                            class(*arg)
                        )));
                    }
                }
            }
            JitInstr::CallGroup { dst, args, .. } => {
                // Group index, arity, and arg/result classes are checked against the
                // co-compiled group in `compile_recursive_group`/`build_function`,
                // where the group's signatures are known. Here only the local
                // register references are validated.
                check_reg(*dst)?;
                if is_flat_type(class(*dst)) {
                    return Err(JitError::invalid_ir(format!(
                        "CallGroup result register {dst} is a flat-array register"
                    )));
                }
                for arg in args {
                    check_reg(*arg)?;
                }
            }
            JitInstr::MatchMapGetInt {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require_class(*map, JitValueType::Handle, "MatchMapGetInt map")?;
                require_class(*key, JitValueType::Int, "MatchMapGetInt key")?;
                require_class(*value_dst, JitValueType::Int, "MatchMapGetInt value")?;
                check_target(*some_ip)?;
                check_target(*none_ip)?;
            }
            JitInstr::MatchMapGetFloat {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require_class(*map, JitValueType::Handle, "MatchMapGetFloat map")?;
                require_class(*key, JitValueType::Int, "MatchMapGetFloat key")?;
                require_class(*value_dst, JitValueType::Float, "MatchMapGetFloat value")?;
                check_target(*some_ip)?;
                check_target(*none_ip)?;
            }
            JitInstr::MatchSortedMapGetInt {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require_class(*map, JitValueType::Handle, "MatchSortedMapGetInt map")?;
                require_class(*key, JitValueType::Int, "MatchSortedMapGetInt key")?;
                require_class(*value_dst, JitValueType::Int, "MatchSortedMapGetInt value")?;
                check_target(*some_ip)?;
                check_target(*none_ip)?;
            }
            JitInstr::MatchSortedMapGetFloat {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                require_class(*map, JitValueType::Handle, "MatchSortedMapGetFloat map")?;
                require_class(*key, JitValueType::Int, "MatchSortedMapGetFloat key")?;
                require_class(
                    *value_dst,
                    JitValueType::Float,
                    "MatchSortedMapGetFloat value",
                )?;
                check_target(*some_ip)?;
                check_target(*none_ip)?;
            }
            JitInstr::BitAnd { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "BitAnd")?,
            JitInstr::BitOr { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "BitOr")?,
            JitInstr::BitXor { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "BitXor")?,
            JitInstr::Shl { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "Shl")?,
            JitInstr::Shr { dst, lhs, rhs } => int_op(*dst, *lhs, *rhs, "Shr")?,
            JitInstr::Compare { dst, lhs, rhs, .. } => compare(*dst, *lhs, *rhs, "Compare")?,
            JitInstr::Equal { dst, lhs, rhs } => {
                scalar_pair(*lhs, *rhs, "Equal")?;
                require_class(*dst, JitValueType::Bool, "Equal result")?;
            }
            JitInstr::NotEqual { dst, lhs, rhs } => {
                scalar_pair(*lhs, *rhs, "NotEqual")?;
                require_class(*dst, JitValueType::Bool, "NotEqual result")?;
            }
            JitInstr::Jump { target } => check_target(*target)?,
            JitInstr::JumpIfBool { cond, target, .. } => {
                require_class(*cond, JitValueType::Bool, "JumpIfBool")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::ProfiledJumpIfBool { cond, target, .. } => {
                require_class(*cond, JitValueType::Bool, "ProfiledJumpIfBool")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::JumpIfIntCompare {
                lhs, rhs, target, ..
            } => {
                numeric_pair(*lhs, *rhs, "JumpIfIntCompare")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::ProfiledJumpIfIntCompare {
                lhs, rhs, target, ..
            } => {
                numeric_pair(*lhs, *rhs, "ProfiledJumpIfIntCompare")?;
                check_target(*target)?;
                check_fallthrough()?;
            }
            JitInstr::Return { src } => {
                if osr {
                    return Err(JitError::invalid_ir(
                        "Return: OSR functions must exit through OsrExit",
                    ));
                }
                check_reg(*src)?;
                // Heap-result return ABI: a scalar (`Int`/`Float`) or a
                // `Handle` register may be returned. A `Handle` return's i64 is an
                // opaque output-table handle (the host materializes a heap value from
                // it); see [`NativeOutcome::CompletedHandle`]. A FLAT-array register is
                // still rejected — it is a (pointer, length) pair, not a single
                // returnable word.
                if is_flat_type(class(*src)) {
                    return Err(JitError::invalid_ir(
                        "Return: cannot return a flat-array register",
                    ));
                }
                returns.push((i, class(*src)));
            }
            JitInstr::ListGetIntDirect { dst, base, index } => {
                require_flat_param(*base, JitValueType::FlatInt, "ListGetIntDirect base")?;
                require_class(*index, JitValueType::Int, "ListGetIntDirect index")?;
                require_class(*dst, JitValueType::Int, "ListGetIntDirect result")?;
            }
            JitInstr::ListSetIntDirect {
                dst,
                base,
                index,
                value,
            } => {
                require_flat_param(*base, JitValueType::FlatIntMut, "ListSetIntDirect base")?;
                require_class(*index, JitValueType::Int, "ListSetIntDirect index")?;
                require_class(*value, JitValueType::Int, "ListSetIntDirect value")?;
                require_class(*dst, JitValueType::Int, "ListSetIntDirect result")?;
            }
            JitInstr::ListGetFloatDirect { dst, base, index } => {
                require_flat_param(*base, JitValueType::FlatFloat, "ListGetFloatDirect base")?;
                require_class(*index, JitValueType::Int, "ListGetFloatDirect index")?;
                require_class(*dst, JitValueType::Float, "ListGetFloatDirect result")?;
            }
            JitInstr::ListSetFloatDirect {
                dst,
                base,
                index,
                value,
            } => {
                require_flat_param(*base, JitValueType::FlatFloatMut, "ListSetFloatDirect base")?;
                require_class(*index, JitValueType::Int, "ListSetFloatDirect index")?;
                require_class(*value, JitValueType::Float, "ListSetFloatDirect value")?;
                require_class(*dst, JitValueType::Int, "ListSetFloatDirect result")?;
            }
            JitInstr::ListLenDirect { dst, base } => {
                check_reg(*base)?;
                if !matches!(
                    class(*base),
                    JitValueType::FlatInt
                        | JitValueType::FlatIntMut
                        | JitValueType::FlatFloat
                        | JitValueType::FlatFloatMut
                ) {
                    return Err(JitError::invalid_ir(format!(
                        "ListLenDirect base: register {base} is {:?}, expected a flat-array param",
                        class(*base)
                    )));
                }
                let window = if osr {
                    program.n_regs as usize
                } else {
                    program.n_params as usize
                };
                if (*base as usize) >= window {
                    return Err(JitError::invalid_ir(format!(
                        "ListLenDirect base: register {base} is not a parameter"
                    )));
                }
                require_class(*dst, JitValueType::Int, "ListLenDirect result")?;
            }
            JitInstr::ListIsEmptyDirect { dst, base } => {
                check_reg(*base)?;
                if !matches!(
                    class(*base),
                    JitValueType::FlatInt
                        | JitValueType::FlatIntMut
                        | JitValueType::FlatFloat
                        | JitValueType::FlatFloatMut
                ) {
                    return Err(JitError::invalid_ir(format!(
                        "ListIsEmptyDirect base: register {base} is {:?}, expected a flat-array param",
                        class(*base)
                    )));
                }
                let window = if osr {
                    program.n_regs as usize
                } else {
                    program.n_params as usize
                };
                if (*base as usize) >= window {
                    return Err(JitError::invalid_ir(format!(
                        "ListIsEmptyDirect base: register {base} is not a parameter"
                    )));
                }
                require_class(*dst, JitValueType::Bool, "ListIsEmptyDirect result")?;
            }
            JitInstr::GuardClosureId { base, expected } => {
                require_class(*base, JitValueType::Handle, "GuardClosureId base")?;
                if *expected < 0 {
                    return Err(JitError::invalid_ir(format!(
                        "GuardClosureId expected: {expected} is not a valid function id"
                    )));
                }
            }
            // OSR-exit is a parameterless terminator (an unconditional deopt at its
            // own ip); its live set is computed from definite-assignment, so it
            // carries no operands to validate.
            JitInstr::OsrExit => {
                if !osr {
                    return Err(JitError::invalid_ir(
                        "OsrExit: normal functions must exit through Return",
                    ));
                }
            }
        }
    }
    validate_memo_scopes(program, &memo_slot_owners)?;
    let reachable = reachable_jit_instrs(program);
    let mut reachable_return_type = None;
    for (ip, ty) in returns {
        if !reachable[ip] {
            continue;
        }
        match reachable_return_type {
            Some(expected) if expected != ty => {
                return Err(JitError::invalid_ir(format!(
                    "Return: inconsistent result types ({expected:?} vs {ty:?})"
                )));
            }
            None => reachable_return_type = Some(ty),
            Some(_) => {}
        }
    }

    let assigned_in = definite_assignment(program, osr);
    for (ip, instr) in program.code.iter().enumerate() {
        if !reachable[ip] {
            continue;
        }
        for used in instr_uses(instr) {
            if !assigned_in[ip][used as usize] {
                return Err(JitError::invalid_ir(format!(
                    "instruction {ip} reads register {used} before it is definitely assigned"
                )));
            }
        }
    }

    let has_call_self = program
        .code
        .iter()
        .enumerate()
        .any(|(ip, instr)| reachable[ip] && matches!(instr, JitInstr::CallSelf { .. }));
    if has_call_self {
        let Some(return_type) = reachable_return_type else {
            return Err(JitError::invalid_ir(
                "CallSelf requires a reachable function Return",
            ));
        };
        if program.reg_types[..program.n_params as usize]
            .iter()
            .any(|ty| is_flat_type(*ty))
        {
            return Err(JitError::invalid_ir(
                "CallSelf does not support flat-array parameters",
            ));
        }
        for (ip, instr) in program.code.iter().enumerate() {
            let JitInstr::CallSelf { dst, .. } = instr else {
                continue;
            };
            if reachable[ip] && program.reg_types[*dst as usize] != return_type {
                return Err(JitError::invalid_ir(format!(
                    "CallSelf result register {dst} is {:?}, function returns {return_type:?}",
                    program.reg_types[*dst as usize]
                )));
            }
        }
    }

    if (has_call_self
        || program
            .code
            .iter()
            .any(|instr| matches!(instr, JitInstr::CallGroup { .. })))
        && native_recursion_frame_bytes_estimate(program) > NATIVE_RECURSION_STACK_BUDGET_BYTES
    {
        return Err(JitError::invalid_ir(format!(
            "recursive native frame estimate {} bytes exceeds the {} byte stack budget",
            native_recursion_frame_bytes_estimate(program),
            NATIVE_RECURSION_STACK_BUDGET_BYTES
        )));
    }
    Ok(ValidationFacts {
        assigned_in,
        return_type: reachable_return_type,
    })
}

pub(crate) fn reachable_jit_instrs(program: &JitFunction) -> Vec<bool> {
    let mut reachable = vec![false; program.code.len()];
    if program.code.is_empty() {
        return reachable;
    }
    let mut pending = vec![0usize];
    while let Some(ip) = pending.pop() {
        if reachable[ip] {
            continue;
        }
        reachable[ip] = true;
        pending.extend(successors(program, ip));
    }
    reachable
}

/// The register an instruction definitely writes (its `dst`), if any. Control
/// instructions (`Return`/`Jump`/`JumpIf*`/`Bail`) and `Nop` write nothing.
pub(crate) fn instr_def(instr: &JitInstr) -> Option<u32> {
    instr.defined_register()
}

/// Registers whose current values are semantically consumed by `instr`.
/// Scratch/cache destinations owned by an instruction are intentionally excluded.
fn instr_uses(instr: &JitInstr) -> Vec<u32> {
    let mut uses = Vec::new();
    instr.visit_used_registers(|reg| uses.push(reg));
    uses
}

/// The control-flow successors of instruction `i` (indices into `program.code`):
/// fallthrough to `i + 1` unless `i` is an unconditional `Jump`; conditional
/// branches add their target; `Jump` goes only to its target; `Return`/`Bail` (and
/// running off the end) go nowhere. Out-of-range targets are dropped — `validate`
/// rejects those before codegen, and the analysis stays total regardless.
pub(crate) fn successors(program: &JitFunction, i: usize) -> Vec<usize> {
    let n = program.code.len();
    let in_range = |t: u32| (t as usize) < n;
    let next = i + 1;
    match program.code[i].effects().control_flow {
        JitControlFlow::Jump(target) => {
            if in_range(target) {
                vec![target as usize]
            } else {
                vec![]
            }
        }
        JitControlFlow::Conditional(target) => {
            let mut succ = Vec::new();
            if next < n {
                succ.push(next);
            }
            if in_range(target) {
                succ.push(target as usize);
            }
            succ
        }
        JitControlFlow::Split { first, second } => {
            let mut succ = Vec::new();
            if in_range(first) {
                succ.push(first as usize);
            }
            if in_range(second) {
                succ.push(second as usize);
            }
            succ
        }
        JitControlFlow::Terminal => vec![],
        JitControlFlow::Fallthrough => {
            if next < n {
                vec![next]
            } else {
                vec![]
            }
        }
    }
}
use super::*;
