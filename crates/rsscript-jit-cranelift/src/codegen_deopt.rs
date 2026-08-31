//! Deopt-guard, direct flat-buffer access, and checked-arithmetic emission
//! helpers split out of `codegen.rs` (module-size partitioning). These are the
//! leaf machine-code emitters `build_function` calls; they carry no codegen
//! state of their own.

use super::*;

pub(crate) struct DirectGet {
    pub(crate) base: Variable,
    pub(crate) index: Variable,
    pub(crate) base_param: u32,
    pub(crate) element_type: types::Type,
    pub(crate) checked: bool,
}

pub(crate) fn emit_direct_get(
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

pub(crate) struct DirectSet {
    pub(crate) base: Variable,
    pub(crate) index: Variable,
    pub(crate) value: Variable,
    pub(crate) base_param: u32,
    pub(crate) checked: bool,
}

pub(crate) fn emit_direct_set_int(
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
pub(crate) struct DeoptCtx<'a> {
    /// CFG index of the JIT instruction currently being lowered. This indexes JIT
    /// liveness only and is deliberately not an interpreter resume identity.
    pub(crate) jit_ip: u32,
    /// Explicit source/resume/accounting identity carried through native rewrites.
    pub(crate) origin: JitInstructionOrigin,
    /// Validated source-resume state sets per instruction. Detached clients use
    /// conservative definite assignment; verified-bytecode translation can add
    /// precise source liveness without dropping local JIT uses.
    pub(crate) deopt_in: &'a [Vec<bool>],
    /// Storage class per register, to type each live register.
    pub(crate) reg_types: &'a [JitValueType],
    /// Accumulated sites, in emission (= id) order.
    pub(crate) sites: &'a mut Vec<DeoptSite>,
    /// Required payload width for this function. Starts at `n_regs`; native-call
    /// deopt sites reserve extra words for copied child payloads.
    pub(crate) payload_words: &'a mut usize,
    /// Test/diagnostic hook: when set, matching safepoints bail unconditionally
    /// (see [`NativeModule::compile_forcing_bail`] and
    /// [`NativeModule::compile_forcing_all_bails`]). `None` on the default
    /// [`compile`](NativeModule::compile) path, where every site is guarded.
    pub(crate) forced: Option<ForcedDeopt>,
    /// OSR-exit (OSR): when set, the site about to be minted bails unconditionally
    /// (the loop-exit edge always deopts). Independent of `forced` (which is keyed
    /// by a specific id); this applies to whatever id this single `bail_if` mints.
    pub(crate) unconditional: bool,
    /// A normal region exit carries a planner-produced minimal state map. Guard
    /// deopts use the validation-produced live-at-resume set.
    pub(crate) live_override: Option<&'a [u32]>,
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
pub(crate) struct DeoptBuffers {
    pub(crate) fallback: Block,
    pub(crate) safepoint_ptr: Value,
    pub(crate) payload_ptr: Value,
}

pub(crate) fn bail_if(
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

pub(crate) struct ChildDeoptSource<'a> {
    pub(crate) safepoint_ptr: Value,
    pub(crate) payload_ptr: Value,
    pub(crate) metadata: &'a NativeCallee,
}

pub(crate) fn bail_if_child_native_failed(
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
pub(crate) fn bail_if_helper_failed(
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
pub(crate) struct CheckedDivRem {
    pub(crate) lhs: Variable,
    pub(crate) rhs: Variable,
    pub(crate) is_rem: bool,
}

pub(crate) fn emit_checked_divrem(
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
pub(crate) struct CheckedShift {
    pub(crate) lhs: Variable,
    pub(crate) rhs: Variable,
    pub(crate) is_right: bool,
}

pub(crate) fn emit_checked_shift(
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
