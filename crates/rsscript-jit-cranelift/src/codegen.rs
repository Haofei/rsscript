/// Push the versioned single-frame parameter/return signature onto `func`.
/// Shared by `compile_inner` and
/// `compile_recursive_group` so the ABI is defined in exactly one place.
///
/// `limits ptr` points at a host-owned 3-word `[i64; 3]` cell
/// `[steps, step_budget, cancel_addr]` used only by an armed OSR variant to enforce
/// `step_budget`/`cancel` in generated code; unarmed compiles ignore it, and callers
/// of unarmed functions may pass a null pointer.
pub(crate) fn push_compiled_abi_signature(
    func: &mut cranelift_codegen::ir::Function,
    ptr_ty: cranelift_codegen::ir::Type,
) {
    func.signature.params.push(AbiParam::new(ptr_ty)); // JitCallFrame ptr
    func.signature.returns.push(AbiParam::new(types::I8));
}
pub(crate) use crate::codegen_call::native_recursion_depth_cap;
use crate::codegen_call::{ChildCallFrameValues, build_child_call_frame};

/// In-generated-code `VmLimits` enforcement requested for this compile. Each flag
/// is set only when the corresponding limit is armed and the generated region can
/// enforce it. Whole-function, OSR, and continuation entries share these controls.
#[derive(Clone, Copy, Default)]
pub(crate) struct LimitChecks {
    /// Emit basic-block/guard-segment source-cost reservation and a steps write-back
    /// on every native exit. A segment is split after every possibly-deopting
    /// instruction, so a successful reservation replaces multiple per-instruction
    /// increments while an insufficient reservation returns to the interpreter at
    /// the exact first uncharged source instruction.
    pub(crate) step: bool,
    /// Emit a `cancel` poll (load the host `AtomicBool`) on every loop backedge and
    /// bail to the interpreter when set — the interpreter then re-polls and errors.
    pub(crate) cancel: bool,
    /// Poll the embedding VM's monotonic clock callback at bounded control
    /// segments and bail before the next source instruction when expired.
    pub(crate) deadline: bool,
}

/// Precompute the source cost charged at each accounting-segment entry.
///
/// Segments never cross a CFG leader and end at every instruction that is not on
/// the conservative no-deopt list. Consequently, if the last instruction bails,
/// every source instruction precharged for the segment has already reached the
/// same pre-dispatch tick boundary as the interpreter. If the whole segment does
/// not fit, generated code leaves `steps` untouched and deopts at the segment entry.
fn step_segment_costs(program: &JitFunction, is_leader: &[bool]) -> Vec<u32> {
    const MAX_CONTROL_SEGMENT_SOURCE_COST: u32 = 512;
    let n = program.code.len();
    let mut starts = is_leader.to_vec();
    for (i, instr) in program.code.iter().enumerate() {
        if !instr.descriptor().step_batch_safe && i + 1 < n {
            starts[i + 1] = true;
        }
    }
    let mut accumulated = 0_u32;
    // This loop may mark a later slot while inspecting the current slot, so an
    // indexed `while` expresses the dependency more directly than an iterator.
    let mut ip = 0;
    while ip < n {
        if starts[ip] {
            accumulated = 0;
        }
        let cost = program.instruction_origin(ip).source_cost;
        if accumulated != 0 && accumulated.saturating_add(cost) > MAX_CONTROL_SEGMENT_SOURCE_COST {
            starts[ip] = true;
            accumulated = 0;
        }
        accumulated = accumulated.saturating_add(cost);
        ip += 1;
    }
    let mut costs = vec![0_u32; n];
    let mut start = 0;
    while start < n {
        debug_assert!(starts[start]);
        let mut end = start + 1;
        while end < n && !starts[end] {
            end += 1;
        }
        costs[start] = (start..end)
            .map(|ip| program.instruction_origin(ip).source_cost)
            .try_fold(0_u32, u32::checked_add)
            .expect("validated source-step cost fits u32");
        start = end;
    }
    costs
}

impl LimitChecks {
    pub(crate) fn any(self) -> bool {
        self.step || self.cancel || self.deadline
    }
}

pub(crate) struct CodegenMetadata {
    pub(crate) deopt_map: DeoptMap,
    pub(crate) direct_list_bounds_checks_elided: u64,
}

pub(crate) struct FunctionCodegenInput<'a> {
    pub(crate) imports: HostFuncs,
    pub(crate) program: &'a JitFunction,
    pub(crate) forced: Option<ForcedDeopt>,
    pub(crate) osr_header: Option<u32>,
    pub(crate) native_callees: &'a [NativeCallee],
    pub(crate) self_func_id: FuncId,
    pub(crate) group: &'a [NativeGroupMember],
    pub(crate) limit_checks: LimitChecks,
    pub(crate) native_static_call_depth: u32,
    pub(crate) assigned_in: &'a [Vec<bool>],
    pub(crate) deopt_in: &'a [Vec<bool>],
}

pub(crate) fn build_function(
    func: &mut cranelift_codegen::ir::Function,
    fbctx: &mut FunctionBuilderContext,
    module: &mut JITModule,
    input: FunctionCodegenInput<'_>,
) -> Result<CodegenMetadata, JitError> {
    let FunctionCodegenInput {
        imports,
        program,
        forced,
        osr_header,
        native_callees,
        self_func_id,
        group,
        limit_checks,
        native_static_call_depth,
        assigned_in,
        deopt_in,
    } = input;
    let _ = (self_func_id, group, native_static_call_depth);
    // Definite-assignment facts were computed by validation and are reused here;
    // codegen never repeats the most expensive structural dataflow pass.
    // Forward integer-interval analysis (interval range analysis): per-instruction sound ranges for
    // Int registers, used purely to elide provably-non-overflowing checks. Like
    // `definite_assignment`, host-side only — it shapes no code beyond choosing the
    // checked vs unchecked arithmetic form.
    let intervals = interval_analysis(program);
    // Codegen-only direct-list bounds plan. The public IR remains checked; only
    // individually proven access IPs use the unchecked machine-code emitter.
    let list_bounds = list_bounds_plan(program, &intervals, osr_header.is_some());
    // Sites accumulate in emission order, aligned 1:1 with the `next_id` counter
    // (`sites[id - 1]` is the site for id `id`).
    let mut sites: Vec<DeoptSite> = Vec::new();
    let mut payload_words = program.n_regs as usize;

    let mut bcx = FunctionBuilder::new(func, fbctx);
    let ptr_ty = module.target_config().pointer_type();

    // Per-function references only for helpers reachable from this validated
    // instruction stream. Besides reducing import work, this lets the detached
    // scalar fuzz probe exercise validation through finalization without inventing
    // callable FFI addresses it can never legally execute.
    let mut required_helpers = Vec::new();
    for instruction in &program.code {
        if let Some(helper) = instruction.descriptor().required_host_helper
            && !required_helpers.contains(&helper)
        {
            required_helpers.push(helper);
        }
    }
    if limit_checks.deadline && !required_helpers.contains(&HostHelper::DeadlineExpired) {
        required_helpers.push(HostHelper::DeadlineExpired);
    }
    let helper_refs: Vec<_> = required_helpers
        .into_iter()
        .map(|helper| {
            (
                helper,
                module.declare_func_in_func(imports.get(helper), bcx.func),
            )
        })
        .collect();
    let native_refs: Vec<_> = native_callees
        .iter()
        .map(|callee| {
            (
                callee.handle,
                module.declare_func_in_func(
                    callee.direct_scalar_func_id.unwrap_or(callee.func_id),
                    bcx.func,
                ),
            )
        })
        .collect();
    // Native cross-function calls (native-call ABI): one Cranelift func ref per
    // resolved native callee, using its direct-scalar id when available.

    let n = program.code.len();
    let n_regs = program.n_regs as usize;

    // One Cranelift variable per VM register, typed by storage class (i64 for
    // integers/booleans, f64 for floats).
    let var_ty = |reg: usize| {
        if program.reg_types[reg] == JitValueType::Float {
            types::F64
        } else {
            types::I64
        }
    };
    let vars: Vec<Variable> = (0..n_regs).map(|i| bcx.declare_var(var_ty(i))).collect();
    #[cfg(feature = "readonly-licm")]
    let memo_count = program
        .code
        .iter()
        .filter(|instr| matches!(instr, JitInstr::MemoizedHostCall { .. }))
        .count();
    #[cfg(not(feature = "readonly-licm"))]
    let memo_count = 0;
    #[cfg(feature = "readonly-licm")]
    let memo_value_tys = {
        let mut value_types = vec![types::I64; memo_count];
        for instr in &program.code {
            if let JitInstr::MemoizedHostCall { dst, memo_slot, .. } = instr {
                value_types[*memo_slot as usize] = var_ty(*dst as usize);
            }
        }
        value_types
    };
    #[cfg(not(feature = "readonly-licm"))]
    let memo_value_tys = vec![types::I64; memo_count];
    let memo_values: Vec<Variable> = memo_value_tys
        .iter()
        .map(|&ty| bcx.declare_var(ty))
        .collect();
    let memo_flags: Vec<Variable> = (0..memo_count)
        .map(|_| bcx.declare_var(types::I64))
        .collect();
    let memo_scope_backedges: Vec<Variable> = program
        .memo_scopes
        .iter()
        .map(|_| bcx.declare_var(types::I64))
        .collect();

    // Entry: read params from the args array, zero the rest, then jump to the
    // block for instruction 0. Args are passed as raw 64-bit words; loading a
    // float register's slot as `f64` reinterprets the caller's `f64::to_bits`.
    let entry = bcx.create_block();
    bcx.append_block_params_for_function_params(entry);
    bcx.switch_to_block(entry);
    let params = bcx.block_params(entry).to_vec();
    let frame_ptr = params[0];
    // The ABI prefix is the only memory read permitted before compatibility is
    // established. A lockstep mismatch declines without touching pointer fields.
    let abi_version = bcx.ins().load(
        types::I32,
        MemFlags::trusted(),
        frame_ptr,
        FRAME_ABI_VERSION,
    );
    let frame_size = bcx
        .ins()
        .load(types::I32, MemFlags::trusted(), frame_ptr, FRAME_SIZE);
    let expected_version = bcx
        .ins()
        .iconst(types::I32, i64::from(JIT_CALL_ABI_VERSION));
    let required_size = bcx.ins().iconst(types::I32, i64::from(CALL_FRAME_SIZE));
    let version_ok = bcx.ins().icmp(IntCC::Equal, abi_version, expected_version);
    let size_ok = bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, frame_size, required_size);
    let abi_ok = bcx.ins().band(version_ok, size_ok);
    let compatible = bcx.create_block();
    let incompatible = bcx.create_block();
    bcx.ins().brif(abi_ok, compatible, &[], incompatible, &[]);
    bcx.switch_to_block(incompatible);
    let status = bcx.ins().iconst(types::I8, JitStatus::AbiMismatch as i64);
    bcx.ins().return_(&[status]);
    bcx.switch_to_block(compatible);
    let args_ptr = bcx
        .ins()
        .load(ptr_ty, MemFlags::trusted(), frame_ptr, FRAME_ARGS);
    let lens_ptr = bcx
        .ins()
        .load(ptr_ty, MemFlags::trusted(), frame_ptr, FRAME_LENS);
    let host_ctx = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), frame_ptr, FRAME_HOST_CTX);
    let out_ptr = bcx
        .ins()
        .load(ptr_ty, MemFlags::trusted(), frame_ptr, FRAME_RESULT);
    let bail_ptr = bcx
        .ins()
        .load(ptr_ty, MemFlags::trusted(), frame_ptr, FRAME_BAIL);
    let safepoint_ptr = bcx
        .ins()
        .load(ptr_ty, MemFlags::trusted(), frame_ptr, FRAME_SAFEPOINT);
    let payload_ptr = bcx
        .ins()
        .load(ptr_ty, MemFlags::trusted(), frame_ptr, FRAME_DEOPT);
    // Native call depth (native-call-ABI slice 1): the chain depth passed by the
    // caller. Forwarded as `depth + 1` to native callees so a future entry guard can
    // bail before host-stack overflow; not yet checked.
    let native_call_depth =
        bcx.ins()
            .load(ptr_ty, MemFlags::trusted(), frame_ptr, FRAME_NATIVE_DEPTH);
    let logical_call_depth =
        bcx.ins()
            .load(ptr_ty, MemFlags::trusted(), frame_ptr, FRAME_LOGICAL_DEPTH);
    let logical_depth_limit = bcx.ins().load(
        ptr_ty,
        MemFlags::trusted(),
        frame_ptr,
        FRAME_LOGICAL_DEPTH_LIMIT,
    );
    // Limits cell pointer (native limit accounting): `[steps, step_budget, cancel_addr]`. Read only by an
    // armed OSR variant; forwarded verbatim to native callees so the whole native
    // chain shares one accounting/cancel cell.
    let limits_ptr = bcx
        .ins()
        .load(ptr_ty, MemFlags::trusted(), frame_ptr, FRAME_LIMITS);
    // native limit accounting limit-tracking variables, materialized only for an armed compile so an
    // unarmed function emits byte-identical code. `steps_var` accumulates the
    // interpreter-equivalent instruction count (one tick per instruction); `limit_var`
    // holds the `step_budget`; `cancel_addr_var` holds the host `AtomicBool` address.
    let steps_var = limit_checks.step.then(|| bcx.declare_var(types::I64));
    let limit_var = limit_checks.step.then(|| bcx.declare_var(types::I64));
    let cancel_addr_var = limit_checks.cancel.then(|| bcx.declare_var(ptr_ty));
    let tail_depth_var = program
        .code
        .iter()
        .any(|instr| matches!(instr, JitInstr::TailCallGuard { .. }))
        .then(|| bcx.declare_var(ptr_ty));
    if let Some(tail_depth_var) = tail_depth_var {
        bcx.def_var(tail_depth_var, logical_call_depth);
    }
    if let (Some(steps_var), Some(limit_var)) = (steps_var, limit_var) {
        let steps0 = bcx
            .ins()
            .load(types::I64, MemFlags::trusted(), limits_ptr, 0);
        bcx.def_var(steps_var, steps0);
        let limit0 = bcx
            .ins()
            .load(types::I64, MemFlags::trusted(), limits_ptr, 8);
        bcx.def_var(limit_var, limit0);
    }
    if let Some(cancel_addr_var) = cancel_addr_var {
        let caddr = bcx.ins().load(ptr_ty, MemFlags::trusted(), limits_ptr, 16);
        bcx.def_var(cancel_addr_var, caddr);
    }
    // Running per-site bail-id counter. Starts at 1 (0 is reserved = no bail);
    // `bail_if` post-increments it so every guard/bail site gets a stable id.
    let mut next_id: i64 = 1;
    // The set of registers to load from the entry window. For a normal compile this
    // is the parameter registers (`0..n_params`, loaded by index from `args_ptr`).
    // For an OSR-entry it is the loop's *live-in* set: the registers definitely
    // assigned on entry to `header_ip`, loaded by register index from the window
    // (`args_ptr` is the interpreter's `n_regs`-wide register window, not a packed
    // arg array). Every other register is zero-initialized so the var is defined on
    // every path (SSA), exactly as on the normal entry.
    let load_set: Vec<usize> = match osr_header {
        Some(header) => {
            let header = header as usize;
            match assigned_in.get(header) {
                Some(set) => set
                    .iter()
                    .enumerate()
                    .filter(|&(_, &assigned)| assigned)
                    .map(|(r, _)| r)
                    .collect(),
                None => Vec::new(),
            }
        }
        None => (0..program.n_params as usize).collect(),
    };
    let mut is_loaded = vec![false; n_regs];
    for &r in &load_set {
        is_loaded[r] = true;
        let v = bcx
            .ins()
            .load(var_ty(r), MemFlags::trusted(), args_ptr, (r as i32) * 8);
        bcx.def_var(vars[r], v);
    }
    let zero_i = bcx.ins().iconst(types::I64, 0);
    let zero_f = bcx.ins().f64const(0.0);
    for (i, &var) in vars.iter().enumerate().take(n_regs) {
        if is_loaded[i] {
            continue;
        }
        bcx.def_var(
            var,
            if var_ty(i) == types::F64 {
                zero_f
            } else {
                zero_i
            },
        );
    }
    for (slot, (&value_var, &flag_var)) in memo_values.iter().zip(memo_flags.iter()).enumerate() {
        let initial_value = if memo_value_tys[slot] == types::F64 {
            zero_f
        } else {
            zero_i
        };
        bcx.def_var(value_var, initial_value);
        bcx.def_var(flag_var, zero_i);
    }
    for &backedge_var in &memo_scope_backedges {
        bcx.def_var(backedge_var, zero_i);
    }

    // The shared fallback block: "not completed".
    let fallback = bcx.create_block();
    let yielded = bcx.create_block();

    // Constant-modulo proofs require the target list to contain at least the
    // divisor's number of elements. Check that once at anonymous entry instead of
    // at every proven access. No safepoint id or payload is written here, so failure
    // remains the existing re-run-from-entry deopt and cannot disturb source-IP maps.
    for (&base, &minimum) in &list_bounds.entry_min_len {
        let len = bcx
            .ins()
            .load(types::I64, MemFlags::trusted(), lens_ptr, (base as i32) * 8);
        let required = bcx.ins().iconst(types::I64, minimum);
        let too_short = bcx.ins().icmp(IntCC::SignedLessThan, len, required);
        let cont = bcx.create_block();
        bcx.ins().brif(too_short, fallback, &[], cont, &[]);
        bcx.switch_to_block(cont);
        bcx.seal_block(cont);
    }

    // Block leaders: index 0, every jump target, and the instruction after any
    // control-transfer (so dead/own-block code never lands in a sealed block).
    let mut is_leader = vec![false; n];
    if n > 0 {
        is_leader[0] = true;
    }
    for (i, instr) in program.code.iter().enumerate() {
        match instr {
            JitInstr::Jump { target } => {
                is_leader[*target as usize] = true;
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
            }
            JitInstr::JumpIfBool { target, .. } | JitInstr::JumpIfIntCompare { target, .. } => {
                is_leader[*target as usize] = true;
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
            }
            JitInstr::MatchMapGetInt {
                some_ip, none_ip, ..
            }
            | JitInstr::MatchMapGetFloat {
                some_ip, none_ip, ..
            }
            | JitInstr::MatchSortedMapGetInt {
                some_ip, none_ip, ..
            }
            | JitInstr::MatchSortedMapGetFloat {
                some_ip, none_ip, ..
            } => {
                is_leader[*some_ip as usize] = true;
                is_leader[*none_ip as usize] = true;
                if i + 1 < n {
                    is_leader[i + 1] = true;
                }
            }
            JitInstr::Return { .. }
            | JitInstr::Bail
            | JitInstr::OsrExit
            | JitInstr::RegionExit { .. }
                if i + 1 < n =>
            {
                is_leader[i + 1] = true;
            }
            _ => {}
        }
    }
    for &cold_ip in &program.cold_blocks {
        if (cold_ip as usize) >= n {
            return Err(JitError::invalid_ir(format!(
                "cold block instruction {cold_ip} is out of range for {n} instructions"
            )));
        }
    }
    let block_for: Vec<Option<Block>> = (0..n)
        .map(|i| {
            if is_leader[i] {
                Some(bcx.create_block())
            } else {
                None
            }
        })
        .collect();
    // Source-step cost is reserved once per conservative accounting segment. This
    // is computed from the same CFG leaders used for codegen and does not affect an
    // unarmed compile.
    let control_cost_at = limit_checks
        .any()
        .then(|| step_segment_costs(program, &is_leader));
    for &cold_ip in &program.cold_blocks {
        if let Some(block) = block_for[cold_ip as usize] {
            bcx.set_cold_block(block);
        }
    }

    let reg = |program_reg: u32| vars[program_reg as usize];

    // Fresh per-call deopt context for the instruction currently being lowered
    // (`i`). Each `bail_if` consumes one and pushes one site, so ids and `sites`
    // indices stay in lock-step. A macro (not a closure) so the `&mut sites` borrow
    // lives only for the single `bail_if` call.
    macro_rules! deopt {
        ($ip:expr) => {
            &mut DeoptCtx {
                jit_ip: $ip as u32,
                origin: program.instruction_origin($ip),
                deopt_in,
                reg_types: &program.reg_types,
                sites: &mut sites,
                payload_words: &mut payload_words,
                forced,
                unconditional: false,
                live_override: None,
            }
        };
        ($ip:expr, unconditional) => {
            &mut DeoptCtx {
                jit_ip: $ip as u32,
                origin: program.instruction_origin($ip),
                deopt_in: &deopt_in,
                reg_types: &program.reg_types,
                sites: &mut sites,
                payload_words: &mut payload_words,
                forced,
                unconditional: true,
                live_override: None,
            }
        };
        ($ip:expr, live = $live:expr) => {
            &mut DeoptCtx {
                jit_ip: $ip as u32,
                origin: program.instruction_origin($ip),
                deopt_in: &deopt_in,
                reg_types: &program.reg_types,
                sites: &mut sites,
                payload_words: &mut payload_words,
                forced,
                unconditional: true,
                live_override: Some($live),
            }
        };
    }

    let helper_ref = |helper: HostHelper| {
        helper_refs
            .iter()
            .find_map(|(candidate, func_ref)| (*candidate == helper).then_some(*func_ref))
            .expect("host helper func ref declared")
    };
    let native_ref = |callee: CompiledId| {
        native_refs
            .iter()
            .find_map(|(candidate, func_ref)| (*candidate == callee).then_some(*func_ref))
            .expect("native callee func ref declared")
    };
    let native_callee = |callee: CompiledId| {
        native_callees
            .iter()
            .find(|candidate| candidate.handle == callee)
            .expect("native callee metadata resolved")
    };

    // Dynamic guards are needed only on recursive entries. Ordinary CallNative
    // edges are statically resolved, so their maximum depth is checked once by the
    // top-level wrapper. Reserve that known descendant depth when recursion and an
    // ordinary native edge coexist.

    match osr_header {
        // OSR-entry: begin native execution *inside* the loop at the header block.
        // The header is a backedge target, hence a leader, so its block exists; if a
        // caller passes a non-leader (or out-of-range) header we reject cleanly.
        Some(header) => {
            let header = header as usize;
            let target = block_for.get(header).copied().flatten().ok_or_else(|| {
                JitError::invalid_ir(format!(
                    "OSR header ip {header} is not a leader / jump-target block"
                ))
            })?;
            bcx.ins().jump(target, &[]);
        }
        None if n == 0 => {
            bcx.ins().jump(fallback, &[]);
        }
        None => {
            bcx.ins().jump(block_for[0].unwrap(), &[]);
        }
    }

    // native limit accounting: loop headers = any instruction that is the target of a backward control
    // transfer (`target <= source`). Each loop's header dominates its body and runs
    // once per iteration, so emitting the budget/cancel check at every header entry is
    // exactly "check on every backedge" — and it naturally covers nested/inner loops
    // (each inner header is itself a backward target). Only computed for an armed
    // compile; otherwise no checks are emitted.
    let backedge_target = if limit_checks.any() {
        let mut bt = vec![false; n];
        for src in 0..n {
            for target in successors(program, src) {
                // The fall-through successor (`src + 1`) is always forward, so a
                // `target <= src` is exactly a backward edge — its target is a header.
                if target <= src {
                    bt[target] = true;
                }
            }
        }
        bt
    } else {
        Vec::new()
    };
    let mut memo_scope_for_header = vec![None; n];
    let mut memo_scope_for_backedge = vec![None; n];
    for (scope_index, scope) in program.memo_scopes.iter().enumerate() {
        memo_scope_for_header[scope.header as usize] = Some(scope_index);
        for (source, backedge_scope) in memo_scope_for_backedge
            .iter_mut()
            .enumerate()
            .take(scope.exit as usize)
            .skip(scope.header as usize)
        {
            if matches!(
                program.code[source],
                JitInstr::Jump { target } if target == scope.header
            ) {
                *backedge_scope = Some(scope_index);
            }
        }
    }

    let mut terminated = true;
    for i in 0..n {
        if let Some(b) = block_for[i] {
            if !terminated {
                bcx.ins().jump(b, &[]);
            }
            bcx.switch_to_block(b);
            terminated = false;
        }
        if let Some(scope_index) = memo_scope_for_header[i] {
            let scope = &program.memo_scopes[scope_index];
            let backedge = bcx.use_var(memo_scope_backedges[scope_index]);
            let zero = bcx.ins().iconst(types::I64, 0);
            let preserve = bcx.ins().icmp(IntCC::NotEqual, backedge, zero);
            let preserve_block = bcx.create_block();
            let reset_block = bcx.create_block();
            let body_block = bcx.create_block();
            bcx.ins()
                .brif(preserve, preserve_block, &[], reset_block, &[]);

            bcx.switch_to_block(reset_block);
            bcx.seal_block(reset_block);
            for &slot in &scope.memo_slots {
                bcx.def_var(memo_flags[slot as usize], zero);
            }
            bcx.ins().jump(body_block, &[]);

            bcx.switch_to_block(preserve_block);
            bcx.seal_block(preserve_block);
            bcx.ins().jump(body_block, &[]);

            bcx.switch_to_block(body_block);
            bcx.seal_block(body_block);
            bcx.def_var(memo_scope_backedges[scope_index], zero);
        }
        // Poll before reserving the segment. A pre-cancelled/expired activation
        // therefore resumes at the first unpaid source instruction, where the
        // interpreter performs its canonical tick and reports the failure.
        let poll_control = control_cost_at.as_ref().is_some_and(|costs| costs[i] != 0)
            || (!backedge_target.is_empty() && backedge_target[i]);
        if poll_control {
            let mut trip: Option<Value> = None;
            if let Some(cancel_addr_var) = cancel_addr_var {
                let caddr = bcx.use_var(cancel_addr_var);
                let flag = bcx.ins().atomic_load(types::I8, MemFlags::trusted(), caddr);
                let zero = bcx.ins().iconst(types::I8, 0);
                let cancelled = bcx.ins().icmp(IntCC::NotEqual, flag, zero);
                trip = Some(cancelled);
            }
            if limit_checks.deadline {
                let call = bcx
                    .ins()
                    .call(helper_ref(HostHelper::DeadlineExpired), &[host_ctx]);
                let expired_word = bcx.inst_results(call)[0];
                let zero = bcx.ins().iconst(types::I64, 0);
                let expired = bcx.ins().icmp(IntCC::NotEqual, expired_word, zero);
                trip = Some(match trip {
                    Some(value) => bcx.ins().bor(value, expired),
                    None => expired,
                });
            }
            if let Some(trip) = trip {
                let cont = bail_if(
                    &mut bcx,
                    trip,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
            }
        }
        // Reserve a whole no-deopt accounting segment before its first source
        // instruction. If it does not fit, leave `steps` unchanged and resume the
        // interpreter at this exact IP; the interpreter's next tick is therefore
        // the first unpaid source step. When it fits, one add replaces all of the
        // segment's per-instruction increments.
        if let (Some(steps_var), Some(limit_var), Some(control_cost_at)) =
            (steps_var, limit_var, control_cost_at.as_ref())
        {
            let cost = control_cost_at[i];
            if cost != 0 {
                let steps = bcx.use_var(steps_var);
                let limit = bcx.use_var(limit_var);
                let remaining = bcx.ins().isub(limit, steps);
                let required = bcx.ins().iconst(types::I64, i64::from(cost));
                let insufficient = bcx.ins().icmp(IntCC::SignedLessThan, remaining, required);
                let cont = bail_if(
                    &mut bcx,
                    insufficient,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                let charged = bcx.ins().iadd(steps, required);
                bcx.def_var(steps_var, charged);
            }
        }
        match &program.code[i] {
            JitInstr::Nop => {}
            JitInstr::TailCallGuard { max_depth } => {
                let tail_depth_var = tail_depth_var.expect("tail guard declares depth state");
                let depth = bcx.use_var(tail_depth_var);
                let next_depth = bcx.ins().iadd_imm(depth, 1);
                bcx.def_var(tail_depth_var, next_depth);
                let built_in_limit = bcx.ins().iconst(ptr_ty, i64::from(*max_depth));
                let use_configured =
                    bcx.ins()
                        .icmp(IntCC::UnsignedLessThan, logical_depth_limit, built_in_limit);
                let limit = bcx
                    .ins()
                    .select(use_configured, logical_depth_limit, built_in_limit);
                let over = bcx
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThan, next_depth, limit);
                let cont = bcx.create_block();
                bcx.ins().brif(over, fallback, &[], cont, &[]);
                bcx.switch_to_block(cont);
            }
            JitInstr::LoadInt { dst, value } => {
                let v = bcx.ins().iconst(types::I64, *value);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::LoadFloat { dst, value } => {
                let v = bcx.ins().f64const(*value);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::LoadBool { dst, value } => {
                let v = bcx.ins().iconst(types::I64, i64::from(*value));
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::Move { dst, src } => {
                let v = bcx.use_var(reg(*src));
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::Add { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                if program.is_float(*lhs) {
                    let res = bcx.ins().fadd(a, b);
                    bcx.def_var(reg(*dst), res);
                } else if arith_cannot_overflow(&intervals[i], &program.code[i]) {
                    // Proven non-overflowing by range analysis ⇒ plain unchecked
                    // add, no overflow flag, no bail. Result is byte-identical to the
                    // checked form (which only differs by trapping on overflow).
                    let res = bcx.ins().iadd(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let (res, of) = bcx.ins().sadd_overflow(a, b);
                    let cont = bail_if(
                        &mut bcx,
                        of,
                        DeoptBuffers {
                            fallback,
                            safepoint_ptr,
                            payload_ptr,
                        },
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Sub { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                if program.is_float(*lhs) {
                    let res = bcx.ins().fsub(a, b);
                    bcx.def_var(reg(*dst), res);
                } else if arith_cannot_overflow(&intervals[i], &program.code[i]) {
                    let res = bcx.ins().isub(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let (res, of) = bcx.ins().ssub_overflow(a, b);
                    let cont = bail_if(
                        &mut bcx,
                        of,
                        DeoptBuffers {
                            fallback,
                            safepoint_ptr,
                            payload_ptr,
                        },
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Mul { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                if program.is_float(*lhs) {
                    let res = bcx.ins().fmul(a, b);
                    bcx.def_var(reg(*dst), res);
                } else if arith_cannot_overflow(&intervals[i], &program.code[i]) {
                    let res = bcx.ins().imul(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let (res, of) = bcx.ins().smul_overflow(a, b);
                    let cont = bail_if(
                        &mut bcx,
                        of,
                        DeoptBuffers {
                            fallback,
                            safepoint_ptr,
                            payload_ptr,
                        },
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Div { dst, lhs, rhs } => {
                if program.is_float(*lhs) {
                    // Float division never traps (x/0.0 = ±inf/NaN), matching the
                    // interpreter, so no bail.
                    let a = bcx.use_var(reg(*lhs));
                    let b = bcx.use_var(reg(*rhs));
                    let res = bcx.ins().fdiv(a, b);
                    bcx.def_var(reg(*dst), res);
                } else {
                    let res = emit_checked_divrem(
                        &mut bcx,
                        CheckedDivRem {
                            lhs: reg(*lhs),
                            rhs: reg(*rhs),
                            is_rem: false,
                        },
                        DeoptBuffers {
                            fallback,
                            safepoint_ptr,
                            payload_ptr,
                        },
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.def_var(reg(*dst), res);
                }
            }
            JitInstr::Mod { dst, lhs, rhs } => {
                // Float modulo is a runtime error in the VM, so only integer
                // registers reach here (eligibility rejects float `%`).
                let res = emit_checked_divrem(
                    &mut bcx,
                    CheckedDivRem {
                        lhs: reg(*lhs),
                        rhs: reg(*rhs),
                        is_rem: true,
                    },
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::IntToFloat { dst, src } => {
                // `Int.to_float`: signed-int (i64) → f64 value-preserving
                // conversion, identical to the interpreter's `i as f64`. The src
                // var is I64, the dst var F64 (per their register classes).
                let v = bcx.use_var(reg(*src));
                let res = bcx.ins().fcvt_from_sint(types::F64, v);
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::FloatToInt { dst, src, rounding } => {
                // Round the f64 to an integral f64 with the requested mode, then a
                // saturating signed cast to i64. `fcvt_to_sint_sat` mirrors Rust's
                // `as i64` saturation (NaN→0, ±∞→i64::MIN/MAX), matching the
                // interpreter's `f.floor()/.ceil() as i64`. The src var is F64, the
                // dst var I64 (per their register classes).
                let v = bcx.use_var(reg(*src));
                let rounded = match rounding {
                    FloatRounding::Floor => bcx.ins().floor(v),
                    FloatRounding::Ceil => bcx.ins().ceil(v),
                };
                let res = bcx.ins().fcvt_to_sint_sat(types::I64, rounded);
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::HostCall { helper, dst, args } => {
                let call_args: Vec<Value> = std::iter::once(host_ctx)
                    .chain(args.iter().map(|arg| match arg {
                        HostArg::Reg(arg) => bcx.use_var(reg(*arg)),
                        HostArg::ImmI64(value) => bcx.ins().iconst(types::I64, *value),
                    }))
                    .collect();
                let call = bcx.ins().call(helper_ref(*helper), &call_args);
                let result = bcx.inst_results(call)[0];
                match helper.signature().failure {
                    HostFailureMode::CannotFail => {}
                    HostFailureMode::BailFlag => {
                        let cont = bail_if_helper_failed(
                            &mut bcx,
                            bail_ptr,
                            DeoptBuffers {
                                fallback,
                                safepoint_ptr,
                                payload_ptr,
                            },
                            &vars,
                            &mut next_id,
                            deopt!(i),
                        );
                        bcx.switch_to_block(cont);
                    }
                }
                let stored = match helper.signature().result {
                    HostResult::Exact(_) => result,
                    HostResult::IntOrFloatBits if program.is_float(*dst) => {
                        bcx.ins().bitcast(types::F64, MemFlags::new(), result)
                    }
                    HostResult::IntOrFloatBits => result,
                };
                bcx.def_var(reg(*dst), stored);
            }
            #[cfg(feature = "readonly-licm")]
            JitInstr::MemoizedHostCall {
                helper,
                dst,
                args,
                memo_slot,
            } => {
                let slot = *memo_slot as usize;
                let cached_block = bcx.create_block();
                let call_block = bcx.create_block();
                let done_block = bcx.create_block();
                let flag_value = bcx.use_var(memo_flags[slot]);
                let zero = bcx.ins().iconst(types::I64, 0);
                let cached = bcx.ins().icmp(IntCC::NotEqual, flag_value, zero);
                bcx.ins().brif(cached, cached_block, &[], call_block, &[]);

                bcx.switch_to_block(cached_block);
                bcx.seal_block(cached_block);
                let cached_value = bcx.use_var(memo_values[slot]);
                bcx.def_var(reg(*dst), cached_value);
                bcx.ins().jump(done_block, &[]);

                bcx.switch_to_block(call_block);
                bcx.seal_block(call_block);
                let call_args: Vec<Value> = std::iter::once(host_ctx)
                    .chain(args.iter().map(|arg| match arg {
                        HostArg::Reg(arg) => bcx.use_var(reg(*arg)),
                        HostArg::ImmI64(value) => bcx.ins().iconst(types::I64, *value),
                    }))
                    .collect();
                let call = bcx.ins().call(helper_ref(*helper), &call_args);
                let result = bcx.inst_results(call)[0];
                match helper.signature().failure {
                    HostFailureMode::CannotFail => {}
                    HostFailureMode::BailFlag => {
                        let cont = bail_if_helper_failed(
                            &mut bcx,
                            bail_ptr,
                            DeoptBuffers {
                                fallback,
                                safepoint_ptr,
                                payload_ptr,
                            },
                            &vars,
                            &mut next_id,
                            deopt!(i),
                        );
                        bcx.switch_to_block(cont);
                    }
                }
                let stored = match helper.signature().result {
                    HostResult::Exact(_) => result,
                    HostResult::IntOrFloatBits if program.is_float(*dst) => {
                        bcx.ins().bitcast(types::F64, MemFlags::new(), result)
                    }
                    HostResult::IntOrFloatBits => result,
                };
                bcx.def_var(reg(*dst), stored);
                bcx.def_var(memo_values[slot], stored);
                let one = bcx.ins().iconst(types::I64, 1);
                bcx.def_var(memo_flags[slot], one);
                bcx.ins().jump(done_block, &[]);

                bcx.switch_to_block(done_block);
                bcx.seal_block(done_block);
            }
            JitInstr::CallNative { callee, dst, args } => {
                let meta = native_callee(*callee);
                if meta.direct_scalar_func_id.is_some() {
                    // Proven-infallible scalar leaves use their private direct
                    // signature. No child frame, args/lens windows, safepoint or
                    // deopt payload is needed on this edge.
                    let direct_args: Vec<_> =
                        args.iter().map(|arg| bcx.use_var(reg(*arg))).collect();
                    let call = bcx.ins().call(native_ref(*callee), &direct_args);
                    let result = bcx.inst_results(call)[0];
                    bcx.def_var(reg(*dst), result);
                } else {
                    let slot_bytes = |words: usize| (words.max(1) * 8) as u32;
                    let args_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        slot_bytes(meta.n_params),
                        3,
                    ));
                    // Scalar helper-free leaves cannot observe a flat-buffer length.
                    // Keep the versioned frame (and child deopt payload) for precise
                    // checked-arithmetic deopt, but do not allocate or populate an
                    // otherwise dead parallel lens window.
                    let lens_slot = (!meta.compact_scalar_frame).then(|| {
                        bcx.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            slot_bytes(meta.n_params),
                            3,
                        ))
                    });
                    let out_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let safepoint_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let payload_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        slot_bytes(meta.deopt_payload_words),
                        3,
                    ));

                    let zero_i64 = bcx.ins().iconst(types::I64, 0);
                    bcx.ins().stack_store(zero_i64, safepoint_slot, 0);
                    for (i_arg, &arg) in args.iter().enumerate() {
                        let value = bcx.use_var(reg(arg));
                        bcx.ins().stack_store(value, args_slot, (i_arg as i32) * 8);
                        let len = if matches!(
                            meta.param_types[i_arg],
                            JitValueType::FlatInt
                                | JitValueType::FlatIntMut
                                | JitValueType::FlatFloat
                                | JitValueType::FlatFloatMut
                        ) {
                            bcx.ins().load(
                                types::I64,
                                MemFlags::trusted(),
                                lens_ptr,
                                (arg as i32) * 8,
                            )
                        } else {
                            zero_i64
                        };
                        if let Some(lens_slot) = lens_slot {
                            bcx.ins().stack_store(len, lens_slot, (i_arg as i32) * 8);
                        }
                    }
                    let args_ptr_v = bcx.ins().stack_addr(ptr_ty, args_slot, 0);
                    let lens_ptr_v = match lens_slot {
                        Some(slot) => bcx.ins().stack_addr(ptr_ty, slot, 0),
                        None => bcx.ins().iconst(ptr_ty, 0),
                    };
                    let out_ptr_v = bcx.ins().stack_addr(ptr_ty, out_slot, 0);
                    let safepoint_ptr_v = bcx.ins().stack_addr(ptr_ty, safepoint_slot, 0);
                    let payload_ptr_v = bcx.ins().stack_addr(ptr_ty, payload_slot, 0);
                    let nargs_v = bcx.ins().iconst(ptr_ty, meta.n_params as i64);
                    // Forward the chain depth as `caller_depth + 1` (native-call-ABI
                    // slice 1): a native callee's depth is one deeper than its caller's.
                    let one_depth = bcx.ins().iconst(ptr_ty, 1);
                    let child_depth = bcx.ins().iadd(native_call_depth, one_depth);
                    let caller_logical_depth = tail_depth_var
                        .map(|tail_depth_var| bcx.use_var(tail_depth_var))
                        .unwrap_or(logical_call_depth);
                    let child_logical_depth = bcx.ins().iadd(caller_logical_depth, one_depth);
                    let child_frame = build_child_call_frame(
                        &mut bcx,
                        ptr_ty,
                        ChildCallFrameValues {
                            args: args_ptr_v,
                            lens: lens_ptr_v,
                            arg_count: nargs_v,
                            host_ctx,
                            limits: limits_ptr,
                            result: out_ptr_v,
                            bail: bail_ptr,
                            safepoint: safepoint_ptr_v,
                            deopt: payload_ptr_v,
                            native_depth: child_depth,
                            logical_depth: child_logical_depth,
                            logical_depth_limit,
                        },
                    );
                    let call = bcx.ins().call(native_ref(*callee), &[child_frame]);
                    let completed = bcx.inst_results(call)[0];
                    let one_i8 = bcx.ins().iconst(types::I8, 1);
                    let not_completed = bcx.ins().icmp(IntCC::NotEqual, completed, one_i8);
                    let failed = if meta.compact_scalar_frame {
                        // This callee contains neither HostCall nor a nested native
                        // call, so it cannot set the shared helper-bail byte. Its typed
                        // status is the complete failure signal.
                        not_completed
                    } else {
                        let child_bail =
                            bcx.ins().load(types::I8, MemFlags::trusted(), bail_ptr, 0);
                        let zero_i8_again = bcx.ins().iconst(types::I8, 0);
                        let child_bailed =
                            bcx.ins().icmp(IntCC::NotEqual, child_bail, zero_i8_again);
                        bcx.ins().bor(not_completed, child_bailed)
                    };
                    let cont = bail_if_child_native_failed(
                        &mut bcx,
                        failed,
                        DeoptBuffers {
                            fallback,
                            safepoint_ptr,
                            payload_ptr,
                        },
                        ChildDeoptSource {
                            safepoint_ptr: safepoint_ptr_v,
                            payload_ptr: payload_ptr_v,
                            metadata: meta,
                        },
                        &vars,
                        &mut next_id,
                        deopt!(i),
                    );
                    bcx.switch_to_block(cont);
                    let result = if meta.return_type == JitValueType::Float {
                        bcx.ins().stack_load(types::F64, out_slot, 0)
                    } else {
                        bcx.ins().stack_load(types::I64, out_slot, 0)
                    };
                    bcx.def_var(reg(*dst), result);
                }
            }
            JitInstr::MatchMapGetInt {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                let map_value = bcx.use_var(reg(*map));
                let key_value = bcx.use_var(reg(*key));
                let found_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let zero = bcx.ins().iconst(types::I64, 0);
                bcx.ins().stack_store(zero, found_slot, 0);
                let found_ptr = bcx.ins().stack_addr(ptr_ty, found_slot, 0);
                let loaded = bcx.ins().call(
                    helper_ref(HostHelper::MapGetMatchInt),
                    &[host_ctx, map_value, key_value, found_ptr],
                );
                let value = bcx.inst_results(loaded)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                let found = bcx.ins().stack_load(types::I64, found_slot, 0);

                let some_block = block_for[*some_ip as usize].unwrap();
                let none_block = block_for[*none_ip as usize].unwrap();
                let is_found = bcx.ins().icmp(IntCC::NotEqual, found, zero);
                bcx.def_var(reg(*value_dst), value);
                bcx.ins().brif(is_found, some_block, &[], none_block, &[]);
                terminated = true;
            }
            JitInstr::MatchMapGetFloat {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                // Identical control flow to MatchMapGetInt; only the payload helper
                // (f64 channel) and the Float `value_dst` differ. The lookup itself
                // is the interpreter's — the helper calls the same `map.get`.
                let map_value = bcx.use_var(reg(*map));
                let key_value = bcx.use_var(reg(*key));
                let found_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let zero = bcx.ins().iconst(types::I64, 0);
                bcx.ins().stack_store(zero, found_slot, 0);
                let found_ptr = bcx.ins().stack_addr(ptr_ty, found_slot, 0);
                let loaded = bcx.ins().call(
                    helper_ref(HostHelper::MapGetMatchFloat),
                    &[host_ctx, map_value, key_value, found_ptr],
                );
                let value = bcx.inst_results(loaded)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                let found = bcx.ins().stack_load(types::I64, found_slot, 0);
                let some_block = block_for[*some_ip as usize].unwrap();
                let none_block = block_for[*none_ip as usize].unwrap();
                let is_found = bcx.ins().icmp(IntCC::NotEqual, found, zero);
                bcx.def_var(reg(*value_dst), value);
                bcx.ins().brif(is_found, some_block, &[], none_block, &[]);
                terminated = true;
            }
            JitInstr::MatchSortedMapGetInt {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                let map_value = bcx.use_var(reg(*map));
                let key_value = bcx.use_var(reg(*key));
                let found_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let zero = bcx.ins().iconst(types::I64, 0);
                bcx.ins().stack_store(zero, found_slot, 0);
                let found_ptr = bcx.ins().stack_addr(ptr_ty, found_slot, 0);
                let loaded = bcx.ins().call(
                    helper_ref(HostHelper::SortedMapGetInt),
                    &[host_ctx, map_value, key_value, found_ptr],
                );
                let value = bcx.inst_results(loaded)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                let found = bcx.ins().stack_load(types::I64, found_slot, 0);
                let some_block = block_for[*some_ip as usize].unwrap();
                let none_block = block_for[*none_ip as usize].unwrap();
                let is_found = bcx.ins().icmp(IntCC::NotEqual, found, zero);
                bcx.def_var(reg(*value_dst), value);
                bcx.ins().brif(is_found, some_block, &[], none_block, &[]);
                terminated = true;
            }
            JitInstr::MatchSortedMapGetFloat {
                map,
                key,
                value_dst,
                some_ip,
                none_ip,
            } => {
                // Mirror of MatchSortedMapGetInt; f64 payload helper + Float value_dst.
                let map_value = bcx.use_var(reg(*map));
                let key_value = bcx.use_var(reg(*key));
                let found_slot = bcx.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    8,
                    3,
                ));
                let zero = bcx.ins().iconst(types::I64, 0);
                bcx.ins().stack_store(zero, found_slot, 0);
                let found_ptr = bcx.ins().stack_addr(ptr_ty, found_slot, 0);
                let loaded = bcx.ins().call(
                    helper_ref(HostHelper::SortedMapGetFloat),
                    &[host_ctx, map_value, key_value, found_ptr],
                );
                let value = bcx.inst_results(loaded)[0];
                let cont = bail_if_helper_failed(
                    &mut bcx,
                    bail_ptr,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.switch_to_block(cont);
                let found = bcx.ins().stack_load(types::I64, found_slot, 0);
                let some_block = block_for[*some_ip as usize].unwrap();
                let none_block = block_for[*none_ip as usize].unwrap();
                let is_found = bcx.ins().icmp(IntCC::NotEqual, found, zero);
                bcx.def_var(reg(*value_dst), value);
                bcx.ins().brif(is_found, some_block, &[], none_block, &[]);
                terminated = true;
            }
            JitInstr::BitAnd { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let v = bcx.ins().band(a, b);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::BitOr { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let v = bcx.ins().bor(a, b);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::BitXor { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let v = bcx.ins().bxor(a, b);
                bcx.def_var(reg(*dst), v);
            }
            JitInstr::Shl { dst, lhs, rhs } => {
                let res = emit_checked_shift(
                    &mut bcx,
                    CheckedShift {
                        lhs: reg(*lhs),
                        rhs: reg(*rhs),
                        is_right: false,
                    },
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Shr { dst, lhs, rhs } => {
                let res = emit_checked_shift(
                    &mut bcx,
                    CheckedShift {
                        lhs: reg(*lhs),
                        rhs: reg(*rhs),
                        is_right: true,
                    },
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                );
                bcx.def_var(reg(*dst), res);
            }
            JitInstr::Compare { dst, op, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(op.fcc(), a, b)
                } else {
                    bcx.ins().icmp(op.cc(), a, b)
                };
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::Equal { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(FloatCC::Equal, a, b)
                } else {
                    bcx.ins().icmp(IntCC::Equal, a, b)
                };
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::NotEqual { dst, lhs, rhs } => {
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(FloatCC::NotEqual, a, b)
                } else {
                    bcx.ins().icmp(IntCC::NotEqual, a, b)
                };
                let c64 = bcx.ins().uextend(types::I64, c);
                bcx.def_var(reg(*dst), c64);
            }
            JitInstr::Jump { target } => {
                if let Some(scope_index) = memo_scope_for_backedge[i] {
                    let one = bcx.ins().iconst(types::I64, 1);
                    bcx.def_var(memo_scope_backedges[scope_index], one);
                }
                bcx.ins().jump(block_for[*target as usize].unwrap(), &[]);
                terminated = true;
            }
            JitInstr::JumpIfBool {
                cond,
                expected,
                target,
            } => {
                let c = bcx.use_var(reg(*cond));
                let tb = block_for[*target as usize].unwrap();
                let fb = block_for[i + 1].unwrap();
                if *expected {
                    bcx.ins().brif(c, tb, &[], fb, &[]);
                } else {
                    bcx.ins().brif(c, fb, &[], tb, &[]);
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
                let a = bcx.use_var(reg(*lhs));
                let b = bcx.use_var(reg(*rhs));
                let c = if program.is_float(*lhs) {
                    bcx.ins().fcmp(op.fcc(), a, b)
                } else {
                    bcx.ins().icmp(op.cc(), a, b)
                };
                let tb = block_for[*target as usize].unwrap();
                let fb = block_for[i + 1].unwrap();
                if *expected {
                    bcx.ins().brif(c, tb, &[], fb, &[]);
                } else {
                    bcx.ins().brif(c, fb, &[], tb, &[]);
                }
                terminated = true;
            }
            JitInstr::Return { src } => {
                let v = bcx.use_var(reg(*src));
                bcx.ins().store(MemFlags::trusted(), v, out_ptr, 0);
                // native limit accounting: a clean native completion writes the accumulated step count
                // back to the host cell, so the interpreter continues from the exact
                // tick total native paid (no skipped/double count).
                if let Some(steps_var) = steps_var {
                    let s = bcx.use_var(steps_var);
                    bcx.ins().store(MemFlags::trusted(), s, limits_ptr, 0);
                }
                let one = bcx.ins().iconst(types::I8, 1);
                bcx.ins().return_(&[one]);
                terminated = true;
            }
            JitInstr::Bail => {
                bcx.ins().jump(fallback, &[]);
                terminated = true;
            }
            JitInstr::OsrExit => {
                // OSR-exit (OSR): the loop has exited. Deopt *unconditionally* at
                // this ip, capturing the live-out window so the host resumes the
                // interpreter here (precise-deopt). Reuse `bail_if` in its
                // unconditional mode: it mints a stable safepoint id, records the
                // `DeoptSite` (resume_ip from explicit origin metadata, live =
                // entry-assigned regs), and
                // emits the id-store + live-capture on the (unconditionally taken)
                // bail edge — the exact same machinery a guard bail uses.
                let final_logical_depth = tail_depth_var
                    .map(|tail_depth_var| bcx.use_var(tail_depth_var))
                    .unwrap_or(logical_call_depth);
                bcx.ins()
                    .store(MemFlags::trusted(), final_logical_depth, out_ptr, 0);
                let always = bcx.ins().iconst(types::I8, 1);
                let cont = bail_if(
                    &mut bcx,
                    always,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i, unconditional),
                );
                // `bail_if` switched us to the (now unreachable, since the bail is
                // unconditional) `cont` block; terminate it so it stays well-formed
                // for `seal_all_blocks` (Cranelift DCE drops the dead block).
                bcx.switch_to_block(cont);
                bcx.ins().jump(fallback, &[]);
                terminated = true;
            }
            JitInstr::RegionExit { exit_id, live } => {
                // A continuation boundary is normal, commit-capable control flow.
                // Write its statically-known live-out words directly into the
                // reusable payload. Do not mint a safepoint or a DeoptSite: normal
                // mixed-mode transfer must not pay exceptional deopt decoding costs.
                let exit = bcx.ins().iconst(types::I64, i64::from(*exit_id));
                bcx.ins().store(MemFlags::trusted(), exit, out_ptr, 0);
                for &reg in live {
                    let value = bcx.use_var(vars[reg as usize]);
                    bcx.ins()
                        .store(MemFlags::trusted(), value, payload_ptr, (reg as i32) * 8);
                }
                bcx.ins().jump(yielded, &[]);
                terminated = true;
            }
            JitInstr::ListGetIntDirect { dst, base, index } => {
                let result = emit_direct_get(
                    &mut bcx,
                    lens_ptr,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    DirectGet {
                        base: reg(*base),
                        index: reg(*index),
                        base_param: *base,
                        element_type: types::I64,
                        checked: !list_bounds.unchecked_ips.contains(&i),
                    },
                );
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListSetIntDirect {
                dst,
                base,
                index,
                value,
            } => {
                emit_direct_set_int(
                    &mut bcx,
                    lens_ptr,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    DirectSet {
                        base: reg(*base),
                        index: reg(*index),
                        value: reg(*value),
                        base_param: *base,
                        checked: !list_bounds.unchecked_ips.contains(&i),
                    },
                );
                let zero = bcx.ins().iconst(types::I64, 0);
                bcx.def_var(reg(*dst), zero);
            }
            JitInstr::ListGetFloatDirect { dst, base, index } => {
                let result = emit_direct_get(
                    &mut bcx,
                    lens_ptr,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    DirectGet {
                        base: reg(*base),
                        index: reg(*index),
                        base_param: *base,
                        element_type: types::F64,
                        checked: !list_bounds.unchecked_ips.contains(&i),
                    },
                );
                bcx.def_var(reg(*dst), result);
            }
            JitInstr::ListSetFloatDirect {
                dst,
                base,
                index,
                value,
            } => {
                // Same emitter as the Int form: the store is 8 bytes at base+index*8,
                // and `value`'s register is F64, so it stores an f64.
                emit_direct_set_int(
                    &mut bcx,
                    lens_ptr,
                    DeoptBuffers {
                        fallback,
                        safepoint_ptr,
                        payload_ptr,
                    },
                    &vars,
                    &mut next_id,
                    deopt!(i),
                    DirectSet {
                        base: reg(*base),
                        index: reg(*index),
                        value: reg(*value),
                        base_param: *base,
                        checked: !list_bounds.unchecked_ips.contains(&i),
                    },
                );
                let zero = bcx.ins().iconst(types::I64, 0);
                bcx.def_var(reg(*dst), zero);
            }
            JitInstr::ListLenDirect { dst, base } => {
                // Length lives in the `lens` slot for the base param (param index ==
                // register index for flat params). No host call, no bail.
                let len = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    lens_ptr,
                    (*base as i32) * 8,
                );
                bcx.def_var(reg(*dst), len);
            }
            JitInstr::ListIsEmptyDirect { dst, base } => {
                // Same `lens`-slot length read as `ListLenDirect`, then `== 0` as a
                // 0/1 boolean (mirrors the `Equal` lowering: icmp → uextend to i64).
                // No host call, no bail.
                let len = bcx.ins().load(
                    types::I64,
                    MemFlags::trusted(),
                    lens_ptr,
                    (*base as i32) * 8,
                );
                let zero = bcx.ins().iconst(types::I64, 0);
                let empty = bcx.ins().icmp(IntCC::Equal, len, zero);
                let empty64 = bcx.ins().uextend(types::I64, empty);
                bcx.def_var(reg(*dst), empty64);
            }
        }
    }
    if !terminated {
        // Fell off the end without an explicit return: behave like the VM's
        // defensive `Unit` return by bailing (this path is unreachable for
        // well-formed bytecode, which always ends in `Return`).
        bcx.ins().jump(fallback, &[]);
    }

    // Fallback block body: not completed.
    bcx.switch_to_block(fallback);
    // native limit accounting: every deopt edge funnels through `fallback`, so a single steps write-back
    // here covers all bails (budget/cancel/guard/OSR-exit). `steps_var` is an SSA
    // Variable, so `use_var` resolves to the accumulated count on whichever edge
    // bailed — the interpreter then resumes with the exact paid tick total.
    if let Some(steps_var) = steps_var {
        let s = bcx.use_var(steps_var);
        bcx.ins().store(MemFlags::trusted(), s, limits_ptr, 0);
    }
    let deopt_status = bcx.ins().iconst(types::I8, JitStatus::Deopt as i64);
    bcx.ins().return_(&[deopt_status]);

    bcx.switch_to_block(yielded);
    if let Some(steps_var) = steps_var {
        let steps = bcx.use_var(steps_var);
        bcx.ins().store(MemFlags::trusted(), steps, limits_ptr, 0);
    }
    let yielded_status = bcx.ins().iconst(types::I8, JitStatus::Yielded as i64);
    bcx.ins().return_(&[yielded_status]);

    bcx.seal_all_blocks();
    bcx.finalize();

    Ok(CodegenMetadata {
        deopt_map: DeoptMap {
            sites,
            payload_words,
        },
        direct_list_bounds_checks_elided: list_bounds.unchecked_ips.len() as u64,
    })
}

/// TV2 direct list read: `cont: dst = base_ptr[index]`, bounds-checked against the
/// param's `lens` slot. `base_var` holds the raw data pointer (i64); `base_param`
/// is its param/register index (used to index `lens`); `elem_ty` is `I64` for an
/// `Ints` list or `F64` for a `Floats` list. An index `< 0` or `>= len` branches to
/// `fallback` (→ the VM re-runs on the interpreter, matching the helper's OOB bail).
///
/// SAFETY (codegen contract): the generated load reads exactly one `elem_ty` at
/// `base_ptr + index * 8`, only after proving `0 <= index < len`. `base_ptr` and
/// `len` come from the caller's `args`/`lens` for the same param, which the
/// `NativeModule::call` borrow protocol guarantees point at a live, immovable,
/// unmutated buffer of `len` elements for the call's duration. So every in-bounds
/// element address is valid and the read cannot alias a concurrent mutation.
struct DirectGet {
    base: Variable,
    index: Variable,
    base_param: u32,
    element_type: types::Type,
    checked: bool,
}

fn emit_direct_get(
    bcx: &mut FunctionBuilder,
    lens_ptr: Value,
    buffers: DeoptBuffers,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
    access: DirectGet,
) -> Value {
    let index = bcx.use_var(access.index);
    if access.checked {
        let len = bcx.ins().load(
            types::I64,
            MemFlags::trusted(),
            lens_ptr,
            (access.base_param as i32) * 8,
        );
        // Single unsigned compare folds "index < 0" (huge unsigned) and
        // "index >= len" into one OOB test.
        let oob = bcx
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
        let cont = bail_if(bcx, oob, buffers, vars, next_id, deopt);
        bcx.switch_to_block(cont);
    }
    let base_ptr = bcx.use_var(access.base);
    let offset = bcx.ins().imul_imm(index, 8);
    let addr = bcx.ins().iadd(base_ptr, offset);
    bcx.ins()
        .load(access.element_type, MemFlags::trusted(), addr, 0)
}

struct DirectSet {
    base: Variable,
    index: Variable,
    value: Variable,
    base_param: u32,
    checked: bool,
}

fn emit_direct_set_int(
    bcx: &mut FunctionBuilder,
    lens_ptr: Value,
    buffers: DeoptBuffers,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
    access: DirectSet,
) {
    let index = bcx.use_var(access.index);
    if access.checked {
        let len = bcx.ins().load(
            types::I64,
            MemFlags::trusted(),
            lens_ptr,
            (access.base_param as i32) * 8,
        );
        let oob = bcx
            .ins()
            .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
        let cont = bail_if(bcx, oob, buffers, vars, next_id, deopt);
        bcx.switch_to_block(cont);
    }
    let base_ptr = bcx.use_var(access.base);
    let value = bcx.use_var(access.value);
    let offset = bcx.ins().imul_imm(index, 8);
    let addr = bcx.ins().iadd(base_ptr, offset);
    bcx.ins().store(MemFlags::trusted(), value, addr, 0);
}

/// Host-side deopt bookkeeping threaded through every guard emission. Each
/// [`bail_if`] call records one [`DeoptSite`] into `sites` for the instruction
/// currently being emitted (`ip`), keeping `sites` aligned 1:1 with the minted
/// safepoint ids. Carries no Cranelift state — it shapes no machine code.
struct DeoptCtx<'a> {
    /// CFG index of the JIT instruction currently being lowered. This indexes JIT
    /// liveness only and is deliberately not an interpreter resume identity.
    jit_ip: u32,
    /// Explicit source/resume/accounting identity carried through native rewrites.
    origin: JitInstructionOrigin,
    /// Validated source-resume state sets per instruction. Detached clients use
    /// conservative definite assignment; verified-bytecode translation can add
    /// precise source liveness without dropping local JIT uses.
    deopt_in: &'a [Vec<bool>],
    /// Storage class per register, to type each live register.
    reg_types: &'a [JitValueType],
    /// Accumulated sites, in emission (= id) order.
    sites: &'a mut Vec<DeoptSite>,
    /// Required payload width for this function. Starts at `n_regs`; native-call
    /// deopt sites reserve extra words for copied child payloads.
    payload_words: &'a mut usize,
    /// Test/diagnostic hook: when set, matching safepoints bail unconditionally
    /// (see [`NativeModule::compile_forcing_bail`] and
    /// [`NativeModule::compile_forcing_all_bails`]). `None` on the default
    /// [`compile`](NativeModule::compile) path, where every site is guarded.
    forced: Option<ForcedDeopt>,
    /// OSR-exit (OSR): when set, the site about to be minted bails unconditionally
    /// (the loop-exit edge always deopts). Independent of `forced` (which is keyed
    /// by a specific id); this applies to whatever id this single `bail_if` mints.
    unconditional: bool,
    /// A normal region exit carries a planner-produced minimal state map. Guard
    /// deopts use the validation-produced live-at-resume set.
    live_override: Option<&'a [u32]>,
}

impl DeoptCtx<'_> {
    /// Record the site for the safepoint about to be minted: resume at the current
    /// instruction with its entry-assigned (definitely-live) registers. Returns the
    /// same `live` set so the caller can emit the matching payload-capture stores.
    fn record_site(&mut self, child: Option<DeoptChildSite>) -> Vec<(u32, JitValueType)> {
        let live: Vec<(u32, JitValueType)> = match self.live_override {
            Some(regs) => regs
                .iter()
                .filter_map(|&reg| {
                    self.reg_types
                        .get(reg as usize)
                        .copied()
                        .map(|ty| (reg, ty))
                })
                .collect(),
            None => match self.deopt_in.get(self.jit_ip as usize) {
                Some(set) => set
                    .iter()
                    .enumerate()
                    .filter(|&(_, &needed)| needed)
                    .map(|(r, _)| (r as u32, self.reg_types[r]))
                    .collect(),
                None => Vec::new(),
            },
        };
        self.sites.push(DeoptSite {
            source_ip: self.origin.source_ip,
            resume_ip: self.origin.resume_ip,
            live: live.clone(),
            child,
        });
        live
    }

    fn record(&mut self) -> Vec<(u32, JitValueType)> {
        self.record_site(None)
    }

    fn record_child(
        &mut self,
        callee: &NativeCallee,
    ) -> (Vec<(u32, JitValueType)>, DeoptChildSite) {
        let safepoint_slot = *self.payload_words;
        let payload_slot = safepoint_slot + 1;
        *self.payload_words += 1 + callee.deopt_payload_words;
        let child = DeoptChildSite {
            callee: callee.handle,
            safepoint_slot: safepoint_slot as u32,
            payload_slot: payload_slot as u32,
            payload_words: callee.deopt_payload_words as u32,
        };
        let live = self.record_site(Some(child));
        (live, child)
    }
}

/// Emit a per-site guarded bail and return the `cont` block to continue in.
///
/// `safepoint_ptr` is the host's safepoint-id cell; `next_id` is the running
/// site-id counter (post-incremented to mint this site's stable id, starting from
/// 1; `0` stays reserved). On the bail edge — and *only* there — a dedicated cold
/// `site_block` stores this site's id into `safepoint_ptr` before jumping to the
/// shared `fallback`. The hot fall-through (`cont`) path executes zero extra
/// instructions, so non-bailing iterations are unaffected.
///
/// `deopt` records this site's [`DeoptSite`] (resume_ip + live regs) host-side,
/// pushed in lock-step with `next_id` so `sites[id - 1]` aligns with the id minted
/// here. This recording emits no machine code.
///
/// On the cold edge — and only there — each live register's current value is also
/// *stored* into `payload_ptr[reg]` (`vars[reg]` is its Cranelift variable; an f64
/// var stores its 8-byte bit pattern into the slot). The hot `cont` path emits no
/// capture store, so non-bailing iterations are unaffected.
#[derive(Clone, Copy)]
struct DeoptBuffers {
    fallback: Block,
    safepoint_ptr: Value,
    payload_ptr: Value,
}

fn bail_if(
    bcx: &mut FunctionBuilder,
    cond: Value,
    buffers: DeoptBuffers,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Block {
    let DeoptBuffers {
        fallback,
        safepoint_ptr,
        payload_ptr,
    } = buffers;
    let site_id = *next_id;
    *next_id += 1;
    let forced = deopt.forced.is_some_and(|forced| forced.forces(site_id)) || deopt.unconditional;
    let live = deopt.record();
    let site_block = bcx.create_block();
    let cont = bcx.create_block();
    if forced {
        // Forced-bail diagnostic path: deopt at this site unconditionally, ignoring
        // `cond`. `cont` is still created/sealed (and emitted into) below — it just
        // becomes unreachable here, which Cranelift's DCE drops. The site_block body
        // is unchanged, so the forced bail captures the same live set a natural bail
        // would. The default `compile` path never sets `forced`, so it always takes
        // the `brif` branch and emits byte-identical guards.
        bcx.ins().jump(site_block, &[]);
    } else {
        bcx.ins().brif(cond, site_block, &[], cont, &[]);
    }
    // Cold path: record this site's id, capture each live register's value into the
    // payload buffer, then fall through to the shared fallback. None of this is
    // emitted on the hot `cont` edge below.
    bcx.switch_to_block(site_block);
    let id_v = bcx.ins().iconst(types::I64, site_id);
    bcx.ins().store(MemFlags::trusted(), id_v, safepoint_ptr, 0);
    for &(reg, _) in &live {
        let v = bcx.use_var(vars[reg as usize]);
        bcx.ins()
            .store(MemFlags::trusted(), v, payload_ptr, (reg as i32) * 8);
    }
    bcx.ins().jump(fallback, &[]);
    bcx.switch_to_block(cont);
    cont
}

struct ChildDeoptSource<'a> {
    safepoint_ptr: Value,
    payload_ptr: Value,
    metadata: &'a NativeCallee,
}

fn bail_if_child_native_failed(
    bcx: &mut FunctionBuilder,
    cond: Value,
    buffers: DeoptBuffers,
    child: ChildDeoptSource<'_>,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Block {
    let DeoptBuffers {
        fallback,
        safepoint_ptr,
        payload_ptr,
    } = buffers;
    let site_id = *next_id;
    *next_id += 1;
    let forced = deopt.forced.is_some_and(|forced| forced.forces(site_id)) || deopt.unconditional;
    let (live, child_site) = deopt.record_child(child.metadata);
    let site_block = bcx.create_block();
    let cont = bcx.create_block();
    if forced {
        bcx.ins().jump(site_block, &[]);
    } else {
        bcx.ins().brif(cond, site_block, &[], cont, &[]);
    }

    bcx.switch_to_block(site_block);
    let id_v = bcx.ins().iconst(types::I64, site_id);
    bcx.ins().store(MemFlags::trusted(), id_v, safepoint_ptr, 0);
    for &(reg, _) in &live {
        let v = bcx.use_var(vars[reg as usize]);
        bcx.ins()
            .store(MemFlags::trusted(), v, payload_ptr, (reg as i32) * 8);
    }

    let child_safepoint = bcx
        .ins()
        .load(types::I64, MemFlags::trusted(), child.safepoint_ptr, 0);
    bcx.ins().store(
        MemFlags::trusted(),
        child_safepoint,
        payload_ptr,
        (child_site.safepoint_slot as i32) * 8,
    );
    for slot in 0..child_site.payload_words {
        let bits = bcx.ins().load(
            types::I64,
            MemFlags::trusted(),
            child.payload_ptr,
            (slot as i32) * 8,
        );
        bcx.ins().store(
            MemFlags::trusted(),
            bits,
            payload_ptr,
            ((child_site.payload_slot + slot) as i32) * 8,
        );
    }
    bcx.ins().jump(fallback, &[]);
    bcx.switch_to_block(cont);
    cont
}

/// Load the host-helper bail flag and branch to `fallback` if a preceding heap
/// read flagged failure — checked immediately after each helper call so a bad
/// read never keeps executing. Returns the continuation block.
fn bail_if_helper_failed(
    bcx: &mut FunctionBuilder,
    bail_ptr: Value,
    buffers: DeoptBuffers,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Block {
    let flag = bcx.ins().load(types::I8, MemFlags::trusted(), bail_ptr, 0);
    bail_if(bcx, flag, buffers, vars, next_id, deopt)
}

/// Checked division / remainder matching the interpreter: bail on divide-by-zero
/// and on `i64::MIN / -1` (the only signed-division overflow).
struct CheckedDivRem {
    lhs: Variable,
    rhs: Variable,
    is_rem: bool,
}

fn emit_checked_divrem(
    bcx: &mut FunctionBuilder,
    operation: CheckedDivRem,
    buffers: DeoptBuffers,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Value {
    let a = bcx.use_var(operation.lhs);
    let b = bcx.use_var(operation.rhs);
    let zero = bcx.ins().iconst(types::I64, 0);
    let is_zero = bcx.ins().icmp(IntCC::Equal, b, zero);
    let cont1 = bail_if(bcx, is_zero, buffers, vars, next_id, deopt);
    bcx.switch_to_block(cont1);
    let imin = bcx.ins().iconst(types::I64, i64::MIN);
    let neg1 = bcx.ins().iconst(types::I64, -1);
    let a_is_min = bcx.ins().icmp(IntCC::Equal, a, imin);
    let b_is_neg1 = bcx.ins().icmp(IntCC::Equal, b, neg1);
    let overflow = bcx.ins().band(a_is_min, b_is_neg1);
    let cont2 = bail_if(bcx, overflow, buffers, vars, next_id, deopt);
    bcx.switch_to_block(cont2);
    if operation.is_rem {
        bcx.ins().srem(a, b)
    } else {
        bcx.ins().sdiv(a, b)
    }
}

/// Checked shift: bail when the shift amount is negative or `>= 64` (so the
/// in-range case matches `wrapping_shl`/`wrapping_shr` exactly).
struct CheckedShift {
    lhs: Variable,
    rhs: Variable,
    is_right: bool,
}

fn emit_checked_shift(
    bcx: &mut FunctionBuilder,
    operation: CheckedShift,
    buffers: DeoptBuffers,
    vars: &[Variable],
    next_id: &mut i64,
    deopt: &mut DeoptCtx,
) -> Value {
    let a = bcx.use_var(operation.lhs);
    let amt = bcx.use_var(operation.rhs);
    let limit = bcx.ins().iconst(types::I64, 64);
    // Unsigned compare folds "negative" (huge unsigned) and ">= 64" into one test.
    let oob = bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, amt, limit);
    let cont = bail_if(bcx, oob, buffers, vars, next_id, deopt);
    bcx.switch_to_block(cont);
    if operation.is_right {
        bcx.ins().sshr(a, amt)
    } else {
        bcx.ins().ishl(a, amt)
    }
}
use super::*;
