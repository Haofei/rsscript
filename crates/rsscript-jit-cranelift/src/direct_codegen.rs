//! Crate-private native-to-native ABI for infallible scalar leaves.
//!
//! The public VM entry remains the versioned `JitCallFrame` ABI. A compact
//! scalar body is emitted only for a leaf whose reachable instructions cannot deopt,
//! allocate, call a helper, suspend, or recurse. Its parameters and result use
//! their Cranelift scalar types directly, so a native caller needs no child frame,
//! payload, safepoint, or temporary argument window. The frame ABI is a small
//! adapter around that canonical body, not a second lowering of the function.

use crate::*;

fn scalar_type(ty: JitValueType) -> Option<cranelift_codegen::ir::Type> {
    match ty {
        JitValueType::Int | JitValueType::Bool => Some(types::I64),
        JitValueType::Float => Some(types::F64),
        _ => None,
    }
}

/// Whether `function` can use a canonical frame-free scalar body.
///
/// Checked integer arithmetic, shifts, host calls and nested native calls are
/// intentionally excluded: those operations need the full deopt contract. Float
/// arithmetic is infallible under RSScript's IEEE-754 semantics.
pub(crate) fn direct_scalar_callable(
    function: &JitFunction,
    osr: bool,
    return_type: Option<JitValueType>,
) -> bool {
    if osr || function.n_params > 8 || return_type.and_then(scalar_type).is_none() {
        return false;
    }
    if function
        .reg_types
        .iter()
        .copied()
        .any(|ty| scalar_type(ty).is_none())
    {
        return false;
    }
    let reachable = reachable_jit_instrs(function);
    function.code.iter().enumerate().all(|(ip, instr)| {
        if !reachable[ip] {
            return true;
        }
        matches!(
            instr,
            JitInstr::Nop
                | JitInstr::LoadInt { .. }
                | JitInstr::LoadFloat { .. }
                | JitInstr::LoadBool { .. }
                | JitInstr::Move { .. }
                | JitInstr::IntToFloat { .. }
                | JitInstr::FloatToInt { .. }
                | JitInstr::BitAnd { .. }
                | JitInstr::BitOr { .. }
                | JitInstr::BitXor { .. }
                | JitInstr::Compare { .. }
                | JitInstr::Equal { .. }
                | JitInstr::NotEqual { .. }
                | JitInstr::Jump { .. }
                | JitInstr::JumpIfBool { .. }
                | JitInstr::JumpIfIntCompare { .. }
                | JitInstr::Return { .. }
        ) || matches!(instr, JitInstr::Add { lhs, .. } | JitInstr::Sub { lhs, .. } | JitInstr::Mul { lhs, .. } | JitInstr::Div { lhs, .. }
            if function.is_float(*lhs))
    })
}

pub(crate) fn push_direct_scalar_signature(
    func: &mut cranelift_codegen::ir::Function,
    function: &JitFunction,
    return_type: JitValueType,
) {
    for ty in &function.reg_types[..function.n_params as usize] {
        func.signature.params.push(AbiParam::new(
            scalar_type(*ty).expect("direct ABI eligibility"),
        ));
    }
    func.signature.returns.push(AbiParam::new(
        scalar_type(return_type).expect("direct ABI eligibility"),
    ));
}

/// Build the stable frame-ABI adapter for an infallible scalar leaf whose only
/// real body is `direct`. The adapter validates the versioned prefix, loads the
/// scalar arguments, calls the compact native ABI, stores the result, and
/// reports completion. Keeping this adapter tiny avoids emitting a second full
/// copy of the function merely to support top-level VM entry.
pub(crate) fn build_direct_scalar_frame_wrapper(
    func: &mut cranelift_codegen::ir::Function,
    fbctx: &mut FunctionBuilderContext,
    direct: cranelift_codegen::ir::FuncRef,
    program: &JitFunction,
    return_type: JitValueType,
) {
    let ptr_ty = func.signature.params[0].value_type;
    let mut bcx = FunctionBuilder::new(func, fbctx);
    let entry = bcx.create_block();
    let compatible = bcx.create_block();
    let incompatible = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);

    let frame = bcx.block_params(entry)[0];
    // The prefix is the only part that may be read before compatibility has
    // been established. This is the same contract as the general codegen path.
    let abi_version = bcx
        .ins()
        .load(types::I32, MemFlags::trusted(), frame, FRAME_ABI_VERSION);
    let frame_size = bcx
        .ins()
        .load(types::I32, MemFlags::trusted(), frame, FRAME_SIZE);
    let expected_version = bcx
        .ins()
        .iconst(types::I32, i64::from(JIT_CALL_ABI_VERSION));
    let required_size = bcx.ins().iconst(types::I32, i64::from(CALL_FRAME_SIZE));
    let version_ok = bcx.ins().icmp(IntCC::Equal, abi_version, expected_version);
    let size_ok = bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, frame_size, required_size);
    let abi_ok = bcx.ins().band(version_ok, size_ok);
    bcx.ins().brif(abi_ok, compatible, &[], incompatible, &[]);

    bcx.switch_to_block(incompatible);
    let mismatch = bcx.ins().iconst(types::I8, JitStatus::AbiMismatch as i64);
    bcx.ins().return_(&[mismatch]);

    bcx.switch_to_block(compatible);
    let args_ptr = bcx
        .ins()
        .load(ptr_ty, MemFlags::trusted(), frame, FRAME_ARGS);
    let result_ptr = bcx
        .ins()
        .load(ptr_ty, MemFlags::trusted(), frame, FRAME_RESULT);
    let mut args = Vec::with_capacity(program.n_params as usize);
    for (index, ty) in program.reg_types[..program.n_params as usize]
        .iter()
        .copied()
        .enumerate()
    {
        let ty = scalar_type(ty).expect("direct wrapper accepts scalar parameters only");
        args.push(bcx.ins().load(
            ty,
            MemFlags::trusted(),
            args_ptr,
            i32::try_from(index * 8).expect("bounded JIT parameter offset fits i32"),
        ));
    }
    let call = bcx.ins().call(direct, &args);
    let result = bcx.inst_results(call)[0];
    debug_assert_eq!(
        bcx.func.dfg.value_type(result),
        scalar_type(return_type).unwrap()
    );
    bcx.ins().store(MemFlags::trusted(), result, result_ptr, 0);
    let completed = bcx.ins().iconst(types::I8, JitStatus::Completed as i64);
    bcx.ins().return_(&[completed]);

    bcx.seal_all_blocks();
    bcx.finalize();
}

pub(crate) fn build_direct_scalar_function(
    func: &mut cranelift_codegen::ir::Function,
    fbctx: &mut FunctionBuilderContext,
    program: &JitFunction,
) -> Result<(), JitError> {
    let reachable = reachable_jit_instrs(program);
    let mut bcx = FunctionBuilder::new(func, fbctx);
    let var_ty = |reg: usize| {
        scalar_type(program.reg_types[reg]).expect("direct ABI accepts scalar registers only")
    };
    let vars: Vec<_> = (0..program.n_regs as usize)
        .map(|reg| bcx.declare_var(var_ty(reg)))
        .collect();
    // Build blocks only for actual CFG leaders. The former one-block-per-
    // instruction lowering inflated Cranelift IR and compile/finalize work for
    // long scalar leaves even though most instructions are straight-line.
    let mut leaders = vec![false; program.code.len()];
    if !leaders.is_empty() {
        leaders[0] = true;
    }
    for (ip, instr) in program.code.iter().enumerate() {
        let mut mark = |target: u32| {
            if let Some(leader) = leaders.get_mut(target as usize) {
                *leader = true;
            }
        };
        match instr {
            JitInstr::Jump { target } => mark(*target),
            JitInstr::JumpIfBool { target, .. } | JitInstr::JumpIfIntCompare { target, .. } => {
                mark(*target);
                if let Some(leader) = leaders.get_mut(ip + 1) {
                    *leader = true;
                }
            }
            JitInstr::Return { .. } => {
                if let Some(leader) = leaders.get_mut(ip + 1) {
                    *leader = true;
                }
            }
            _ => {}
        }
    }
    let blocks: Vec<_> = leaders
        .iter()
        .map(|leader| leader.then(|| bcx.create_block()))
        .collect();
    let entry = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    let params = bcx.block_params(entry).to_vec();
    for reg in 0..program.n_params as usize {
        bcx.def_var(vars[reg], params[reg]);
    }
    bcx.ins()
        .jump(blocks[0].expect("instruction zero is a CFG leader"), &[]);

    let reg = |index: u32| vars[index as usize];
    for (ip, instr) in program.code.iter().enumerate() {
        if !reachable[ip] {
            continue;
        }
        if let Some(block) = blocks[ip] {
            bcx.switch_to_block(block);
        }
        let mut terminated = false;
        match instr {
            JitInstr::Nop => {}
            JitInstr::LoadInt { dst, value } => {
                let value = bcx.ins().iconst(types::I64, *value);
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::LoadFloat { dst, value } => {
                let value = bcx.ins().f64const(*value);
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::LoadBool { dst, value } => {
                let value = bcx.ins().iconst(types::I64, i64::from(*value));
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::Move { dst, src } => {
                let value = bcx.use_var(reg(*src));
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::Add { dst, lhs, rhs }
            | JitInstr::Sub { dst, lhs, rhs }
            | JitInstr::Mul { dst, lhs, rhs }
            | JitInstr::Div { dst, lhs, rhs } => {
                debug_assert!(program.is_float(*lhs));
                let lhs_value = bcx.use_var(reg(*lhs));
                let rhs_value = bcx.use_var(reg(*rhs));
                let value = match instr {
                    JitInstr::Add { .. } => bcx.ins().fadd(lhs_value, rhs_value),
                    JitInstr::Sub { .. } => bcx.ins().fsub(lhs_value, rhs_value),
                    JitInstr::Mul { .. } => bcx.ins().fmul(lhs_value, rhs_value),
                    JitInstr::Div { .. } => bcx.ins().fdiv(lhs_value, rhs_value),
                    _ => unreachable!(),
                };
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::IntToFloat { dst, src } => {
                let value = bcx.use_var(reg(*src));
                let value = bcx.ins().fcvt_from_sint(types::F64, value);
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::FloatToInt { dst, src, rounding } => {
                let value = bcx.use_var(reg(*src));
                let rounded = match rounding {
                    FloatRounding::Floor => bcx.ins().floor(value),
                    FloatRounding::Ceil => bcx.ins().ceil(value),
                };
                let value = bcx.ins().fcvt_to_sint_sat(types::I64, rounded);
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::BitAnd { dst, lhs, rhs }
            | JitInstr::BitOr { dst, lhs, rhs }
            | JitInstr::BitXor { dst, lhs, rhs } => {
                let lhs_value = bcx.use_var(reg(*lhs));
                let rhs_value = bcx.use_var(reg(*rhs));
                let value = match instr {
                    JitInstr::BitAnd { .. } => bcx.ins().band(lhs_value, rhs_value),
                    JitInstr::BitOr { .. } => bcx.ins().bor(lhs_value, rhs_value),
                    JitInstr::BitXor { .. } => bcx.ins().bxor(lhs_value, rhs_value),
                    _ => unreachable!(),
                };
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::Compare { dst, op, lhs, rhs } => {
                let lhs_value = bcx.use_var(reg(*lhs));
                let rhs_value = bcx.use_var(reg(*rhs));
                let value = if program.is_float(*lhs) {
                    bcx.ins().fcmp(op.fcc(), lhs_value, rhs_value)
                } else {
                    bcx.ins().icmp(op.cc(), lhs_value, rhs_value)
                };
                let value = bcx.ins().uextend(types::I64, value);
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::Equal { dst, lhs, rhs } | JitInstr::NotEqual { dst, lhs, rhs } => {
                let lhs_value = bcx.use_var(reg(*lhs));
                let rhs_value = bcx.use_var(reg(*rhs));
                let equal = matches!(instr, JitInstr::Equal { .. });
                let value = if program.is_float(*lhs) {
                    bcx.ins().fcmp(
                        if equal {
                            FloatCC::Equal
                        } else {
                            FloatCC::NotEqual
                        },
                        lhs_value,
                        rhs_value,
                    )
                } else {
                    bcx.ins().icmp(
                        if equal { IntCC::Equal } else { IntCC::NotEqual },
                        lhs_value,
                        rhs_value,
                    )
                };
                let value = bcx.ins().uextend(types::I64, value);
                bcx.def_var(reg(*dst), value);
            }
            JitInstr::Jump { target } => {
                bcx.ins().jump(
                    blocks[*target as usize].expect("validated jump target is a leader"),
                    &[],
                );
                terminated = true;
            }
            JitInstr::JumpIfBool {
                cond,
                expected,
                target,
            } => {
                let cond = bcx.use_var(reg(*cond));
                let target =
                    blocks[*target as usize].expect("validated conditional target is a leader");
                let fallthrough = blocks[ip + 1].expect("conditional fallthrough is a leader");
                if *expected {
                    bcx.ins().brif(cond, target, &[], fallthrough, &[]);
                } else {
                    bcx.ins().brif(cond, fallthrough, &[], target, &[]);
                }
                terminated = true;
            }
            JitInstr::JumpIfIntCompare {
                lhs,
                rhs,
                op,
                expected,
                target,
            } => {
                let lhs_value = bcx.use_var(reg(*lhs));
                let rhs_value = bcx.use_var(reg(*rhs));
                let cond = if program.is_float(*lhs) {
                    bcx.ins().fcmp(op.fcc(), lhs_value, rhs_value)
                } else {
                    bcx.ins().icmp(op.cc(), lhs_value, rhs_value)
                };
                let target =
                    blocks[*target as usize].expect("validated conditional target is a leader");
                let fallthrough = blocks[ip + 1].expect("conditional fallthrough is a leader");
                if *expected {
                    bcx.ins().brif(cond, target, &[], fallthrough, &[]);
                } else {
                    bcx.ins().brif(cond, fallthrough, &[], target, &[]);
                }
                terminated = true;
            }
            JitInstr::Return { src } => {
                let value = bcx.use_var(reg(*src));
                bcx.ins().return_(&[value]);
                terminated = true;
            }
            _ => {
                return Err(JitError::invalid_ir(
                    "instruction is not direct-scalar callable",
                ));
            }
        }
        if !terminated {
            let next = ip + 1;
            if next >= program.code.len() || !reachable[next] {
                return Err(JitError::invalid_ir(
                    "direct scalar function falls through without a Return",
                ));
            }
            if let Some(next_block) = blocks[next] {
                bcx.ins().jump(next_block, &[]);
            }
        }
    }
    bcx.seal_all_blocks();
    bcx.finalize();
    Ok(())
}
