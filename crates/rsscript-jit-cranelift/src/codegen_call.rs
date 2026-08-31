use super::*;

pub(crate) struct ChildCallFrameValues {
    pub(crate) args: Value,
    pub(crate) lens: Value,
    pub(crate) arg_count: Value,
    pub(crate) host_ctx: Value,
    pub(crate) limits: Value,
    pub(crate) result: Value,
    pub(crate) bail: Value,
    pub(crate) safepoint: Value,
    pub(crate) deopt: Value,
    pub(crate) native_depth: Value,
    pub(crate) logical_depth: Value,
    pub(crate) logical_depth_limit: Value,
}

pub(crate) fn build_child_call_frame(
    bcx: &mut FunctionBuilder<'_>,
    ptr_ty: cranelift_codegen::ir::Type,
    values: ChildCallFrameValues,
) -> Value {
    let ChildCallFrameValues {
        args,
        lens,
        arg_count,
        host_ctx,
        limits,
        result,
        bail,
        safepoint,
        deopt,
        native_depth,
        logical_depth,
        logical_depth_limit,
    } = values;
    let slot = bcx.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        CALL_FRAME_SIZE,
        3,
    ));
    let abi = bcx
        .ins()
        .iconst(types::I32, i64::from(JIT_CALL_ABI_VERSION));
    let frame_size = bcx.ins().iconst(types::I32, i64::from(CALL_FRAME_SIZE));
    let flags = bcx.ins().iconst(types::I64, 0);
    bcx.ins().stack_store(abi, slot, FRAME_ABI_VERSION);
    bcx.ins().stack_store(frame_size, slot, FRAME_SIZE);
    bcx.ins().stack_store(flags, slot, FRAME_FLAGS);
    for (value, offset) in [
        (args, FRAME_ARGS),
        (lens, FRAME_LENS),
        (arg_count, FRAME_ARG_COUNT),
        (host_ctx, FRAME_HOST_CTX),
        (limits, FRAME_LIMITS),
        (result, FRAME_RESULT),
        (bail, FRAME_BAIL),
        (safepoint, FRAME_SAFEPOINT),
        (deopt, FRAME_DEOPT),
        (native_depth, FRAME_NATIVE_DEPTH),
        (logical_depth, FRAME_LOGICAL_DEPTH),
        (logical_depth_limit, FRAME_LOGICAL_DEPTH_LIMIT),
    ] {
        bcx.ins().stack_store(value, slot, offset);
    }
    bcx.ins().stack_addr(ptr_ty, slot, 0)
}

/// Heuristic host stack budget used to decline research-only native recursion.
pub(crate) const NATIVE_RECURSION_STACK_BUDGET_BYTES: i64 = 1 << 20;

/// Upper bound for the frame-size-derived native recursion cap.
pub(crate) const NATIVE_RECURSION_DEPTH_CAP_MAX: i64 = 250;

/// Conservatively estimate one generated recursive frame's stack footprint.
pub(crate) fn native_recursion_frame_bytes_estimate(program: &JitFunction) -> i64 {
    const SLOT_BYTES: i64 = 8;
    const FIXED_OVERHEAD_BYTES: i64 = 4096;
    let regs = program.n_regs as i64;
    let explicit_slots = program.code.iter().fold(0_i64, |total, instr| {
        let words: i64 = match instr {
            _ => 0,
        };
        total.saturating_add(words.saturating_mul(SLOT_BYTES))
    });
    FIXED_OVERHEAD_BYTES
        .saturating_add(SLOT_BYTES.saturating_mul(regs).saturating_mul(4))
        .saturating_add(explicit_slots)
}

/// Derive a frame-size-aware recursive entry cap from the research budget.
pub(crate) fn native_recursion_depth_cap(program: &JitFunction) -> i64 {
    let frame = native_recursion_frame_bytes_estimate(program).max(1);
    (NATIVE_RECURSION_STACK_BUDGET_BYTES / frame).min(NATIVE_RECURSION_DEPTH_CAP_MAX)
}
