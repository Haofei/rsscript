//! Natural-loop discovery and OSR region metadata.

#![allow(clippy::doc_lazy_continuation, clippy::needless_range_loop)]

use super::*;

#[cfg(feature = "native-jit")]
const MIN_CONTINUATION_DIRECT_WORK: usize = 16;

/// Stable dispatch threshold derived from the canonical mixed-mode scorecard.
/// Region formation remains available below this value for telemetry and tests,
/// but crossing the native/VM trampoline is admitted only when enough source
/// work exists to amortize state marshalling.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) const MIN_CONTINUATION_ADMISSION_WORK: usize = 512;

/// Policy decision made from the shape-independent region plan before runtime
/// shape construction or Cranelift compilation. `Ignore` regions must not
/// suppress tier-0: they are structurally valid continuation candidates, but the
/// active execution policy cannot profitably or safely dispatch them.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) enum ContinuationDecision {
    Ignore,
    Compile,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) enum ContinuationTierDecision {
    BaselineOnly,
    Promote,
}

/// Optimized continuation code is admitted only from controlled canonical
/// evidence, expressed as basis points of end-to-end speedup. No evidence means
/// baseline-only; microbenchmarks and developer-machine timings cannot silently
/// grow a third, divergent promotion policy.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn continuation_tier_decision(
    canonical_speedup_basis_points: Option<u16>,
) -> ContinuationTierDecision {
    if canonical_speedup_basis_points.is_some_and(|speedup| speedup >= 1_150) {
        ContinuationTierDecision::Promote
    } else {
        ContinuationTierDecision::BaselineOnly
    }
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn continuation_decision(
    region: &ContinuationRegion,
    cost_model: NativeCostModel,
    step_armed: bool,
    deadline_armed: bool,
) -> ContinuationDecision {
    if region.has_backedge && (step_armed || deadline_armed) {
        return ContinuationDecision::Ignore;
    }
    if matches!(cost_model, NativeCostModel::Enforce)
        && !region.has_backedge
        && region.source_instructions < MIN_CONTINUATION_ADMISSION_WORK
    {
        return ContinuationDecision::Ignore;
    }
    ContinuationDecision::Compile
}

/// First supported helper slice for mixed-mode continuations: synchronous,
/// read-only heap queries with scalar results. Handle-producing helpers and every
/// heap mutation remain VM barriers until their transaction/accounting contracts
/// are admitted separately.
#[cfg(feature = "native-jit")]
fn continuation_readonly_helper(instr: &RegInstr) -> bool {
    let scalar_read = |spec: NativeHostIntrinsic| {
        matches!(
            spec.result_ty,
            NativeTy::Int | NativeTy::Bool | NativeTy::Float
        ) && spec.helper.heap_effect() == vm_jit::HostHeapEffect::ReadOnly
    };
    match instr {
        RegInstr::CallIntrinsic {
            intrinsic, args, ..
        } => native_host_typed_intrinsic(*intrinsic, None)
            .is_some_and(|spec| args.len() == spec.arg_tys().len() && scalar_read(spec)),
        RegInstr::CallTypedIntrinsic {
            intrinsic,
            type_arg,
            args,
            ..
        } => native_host_typed_intrinsic(*intrinsic, Some(type_arg.as_str()))
            .is_some_and(|spec| args.len() == spec.arg_tys().len() && scalar_read(spec)),
        // The translator selects a typed read-only helper from inferred value use.
        // A Handle result is rejected after inference below.
        RegInstr::ListLen { .. } | RegInstr::ListGet { .. } | RegInstr::GetFieldSlot { .. } => true,
        _ => false,
    }
}

/// A single natural loop identified for OSR (OSR): the conservative shape this
/// slice compiles. `header` is the loop's entry instruction (a conditional branch
/// that is the target of the loop's backedge); `exit` is the post-loop instruction
/// the header's branch leaves to. Native execution OSR-enters at `header` and
/// OSR-exits (deopts) at `exit`.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) struct OsrLoop {
    pub(in crate::reg_vm) header: usize,
    pub(in crate::reg_vm) exit: usize,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) struct OsrDerivedLiveIn {
    pub(in crate::reg_vm) native_reg: usize,
    pub(in crate::reg_vm) base_reg: usize,
    pub(in crate::reg_vm) field_slot: usize,
    pub(in crate::reg_vm) ty: NativeTy,
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::reg_vm) struct OsrScalarField {
    pub(in crate::reg_vm) native_reg: usize,
    pub(in crate::reg_vm) base_reg: usize,
    pub(in crate::reg_vm) field_slot: usize,
    pub(in crate::reg_vm) writeback: bool,
}

/// One conservative scalar CFG continuation. Generated code executes only the
/// marked instructions and yields normally before any entry in `exits`.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::reg_vm) struct ContinuationRegion {
    pub(in crate::reg_vm) entry: usize,
    pub(in crate::reg_vm) included: Vec<bool>,
    pub(in crate::reg_vm) exits: BTreeMap<usize, NativeBarrierReason>,
    pub(in crate::reg_vm) has_backedge: bool,
    pub(in crate::reg_vm) active_regs: Vec<bool>,
    pub(in crate::reg_vm) live_in_regs: Vec<usize>,
    pub(in crate::reg_vm) source_instructions: usize,
}

/// One dense native slot and the VM register whose value it represents.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone, Copy)]
pub(in crate::reg_vm) struct ContinuationSlot {
    pub(in crate::reg_vm) vm_reg: usize,
    pub(in crate::reg_vm) ty: NativeTy,
    pub(in crate::reg_vm) written: bool,
}

/// Static metadata for one normal continuation exit. `live_slots` indexes the
/// compact native register window, never the full VM register file.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone)]
pub(in crate::reg_vm) struct ContinuationExit {
    pub(in crate::reg_vm) reason: NativeBarrierReason,
    pub(in crate::reg_vm) live_slots: Box<[u32]>,
}

/// A compiled scalar continuation cached per function, entry, and runtime shape.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone)]
pub(in crate::reg_vm) struct ContinuationEntry {
    pub(in crate::reg_vm) id: vm_jit::CompiledId,
    pub(in crate::reg_vm) entry: usize,
    pub(in crate::reg_vm) exits: BTreeMap<usize, ContinuationExit>,
    pub(in crate::reg_vm) n_jit_regs: usize,
    pub(in crate::reg_vm) n_live_in: usize,
    pub(in crate::reg_vm) slots: Box<[ContinuationSlot]>,
}

/// CFG-derived continuation entries. A candidate is function entry or a
/// successor edge of a VM-owned barrier; textual adjacency is deliberately not
/// used because calls, matches, cleanup and scheduler resumes may target a
/// non-adjacent block.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone)]
pub(in crate::reg_vm) struct ContinuationEntrySet {
    candidates: Box<[bool]>,
}

#[cfg(feature = "native-jit")]
impl ContinuationEntrySet {
    pub(in crate::reg_vm) fn from_code(code: &[RegInstr]) -> Self {
        let mut candidates = vec![false; code.len()];
        if !code.is_empty() {
            candidates[0] = true;
        }
        for (ip, instr) in code.iter().enumerate() {
            if matches!(native_lowering_class(instr), NativeLoweringClass::Direct) {
                continue;
            }
            native_instr_successors(instr, ip, code.len(), |successor| {
                if let Some(candidate) = candidates.get_mut(successor) {
                    *candidate = true;
                }
            });
        }
        Self {
            candidates: candidates.into_boxed_slice(),
        }
    }

    pub(in crate::reg_vm) fn contains(&self, entry: usize) -> bool {
        self.candidates.get(entry).copied().unwrap_or(false)
    }

    pub(in crate::reg_vm) fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.candidates
            .iter()
            .enumerate()
            .filter_map(|(ip, candidate)| candidate.then_some(ip))
    }
}

/// Find a useful scalar CFG region beginning at `entry`.
///
/// Direct instructions, branches, and loops remain inside the region. Calls,
/// returns, synchronous helpers, and unsupported operations become normal exits.
/// Translation and dispatch still reject non-scalar frame shapes.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn detect_scalar_continuation_region(
    code: &[RegInstr],
    n_regs: usize,
    entry: usize,
) -> Option<ContinuationRegion> {
    // A region transition crosses the Rust/native ABI and materializes live
    // state. Tiny slices lose even when every contained opcode is native-capable.
    const MAX_REGION_INSTRUCTIONS: usize = 2_048;
    const MAX_REGION_EXITS: usize = 16;
    if entry >= code.len() {
        return None;
    }
    let mut included = vec![false; code.len()];
    let mut exits = BTreeMap::new();
    let mut pending = vec![entry];
    let mut direct_work = 0usize;
    let mut has_backedge = false;
    while let Some(ip) = pending.pop() {
        let instr = code.get(ip)?;
        if included[ip] || exits.contains_key(&ip) {
            continue;
        }
        let lowering = native_lowering_class(instr);
        let barrier = match (instr, lowering) {
            (RegInstr::CallKnown { mut_args, .. }, NativeLoweringClass::Yield { reason })
                if mut_args.is_empty() =>
            {
                Some(reason)
            }
            (RegInstr::Return { .. }, _) => Some(NativeBarrierReason::FunctionReturn),
            (_, NativeLoweringClass::Helper { .. }) if continuation_readonly_helper(instr) => None,
            (_, NativeLoweringClass::Helper { .. }) => {
                Some(NativeBarrierReason::UnsupportedIntrinsic)
            }
            (_, NativeLoweringClass::Yield { reason }) => Some(reason),
            (_, NativeLoweringClass::Reject) => Some(NativeBarrierReason::UnsupportedInstruction),
            _ => None,
        };
        if let Some(reason) = barrier {
            exits.insert(ip, reason);
            if exits.len() > MAX_REGION_EXITS {
                return None;
            }
            continue;
        }
        if !matches!(
            lowering,
            NativeLoweringClass::Direct | NativeLoweringClass::Helper { .. }
        ) || matches!(
            instr,
            RegInstr::RuntimeError { .. } | RegInstr::TailCallGuard
        ) {
            return None;
        }
        included[ip] = true;
        direct_work = direct_work.saturating_add(match lowering {
            NativeLoweringClass::Helper { estimated_cost } => usize::from(estimated_cost),
            _ => 1,
        });
        if direct_work > MAX_REGION_INSTRUCTIONS {
            return None;
        }
        native_instr_successors(instr, ip, code.len(), |successor| {
            has_backedge |= successor <= ip;
            pending.push(successor);
        });
    }
    // A backedge that lands on a barrier would yield once per loop iteration.
    // That native/VM ping-pong is predictably worse than keeping the loop in the
    // interpreter (or letting the existing whole-loop OSR path handle it).
    let mut barrier_backedge = false;
    for (ip, instr) in code.iter().enumerate() {
        if !included[ip] {
            continue;
        }
        native_instr_successors(instr, ip, code.len(), |successor| {
            barrier_backedge |= successor <= ip && !included[successor];
        });
    }
    // A backedge to a barrier would cross engines once per iteration and remains
    // forbidden. A closed native loop whose only exits are forward barriers is
    // safe and profitable: it yields once after the loop, not on its backedge.
    if barrier_backedge {
        return None;
    }
    if direct_work < MIN_CONTINUATION_DIRECT_WORK || exits.is_empty() {
        return None;
    }
    let max_reg = code
        .iter()
        .enumerate()
        .filter(|(ip, _)| included[*ip])
        .try_fold(0usize, |maximum, (_, instr)| {
            native_continuation_registers(instr).map(|regs| {
                regs.into_iter()
                    .max()
                    .map_or(maximum, |reg| maximum.max(reg + 1))
            })
        })?
        .max(n_regs);
    let mut active_regs = vec![false; max_reg];
    for (ip, instr) in code.iter().enumerate() {
        if !included[ip] {
            continue;
        }
        for reg in native_continuation_registers(instr)? {
            active_regs[reg] = true;
        }
    }
    let liveness = NativeRegionAnalysis::compute_prefix(code, max_reg, 0, code.len())?;
    let live_in_regs = (0..max_reg)
        .filter(|reg| {
            active_regs.get(*reg).copied().unwrap_or(false)
                && liveness.live_in(entry, *reg) == Some(true)
        })
        .collect();
    Some(ContinuationRegion {
        entry,
        included,
        exits,
        has_backedge,
        active_regs,
        live_in_regs,
        source_instructions: direct_work,
    })
}

/// Lower a conservative continuation through the mature OSR window-entry path,
/// replacing its rollback-style `OsrExit` with a commit-capable `RegionExit`.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn translate_scalar_continuation_region(
    func: &RegFunction,
    facts: &VerifiedFunctionFacts,
    region: &ContinuationRegion,
    param_native_types: &[Option<NativeTy>],
) -> Option<(
    vm_jit::JitFunction,
    Box<[ContinuationSlot]>,
    usize,
    BTreeMap<usize, ContinuationExit>,
)> {
    if region.entry >= func.code.len() || region.included.len() != func.code.len() {
        return None;
    }
    let mut synthetic = func.code.clone();
    for (ip, instr) in synthetic.iter_mut().enumerate() {
        if !region.included[ip] {
            *instr = RegInstr::RuntimeError {
                message: "native continuation boundary".to_string(),
            };
        }
    }
    let mut active_regs = region.active_regs.clone();
    active_regs.resize(func.regs, false);
    let immutable_leaf_params = vec![false; func.params];
    let (
        mut jit_fn,
        _params,
        derived_liveins,
        scalar_fields,
        reg_types,
        written_regs,
        string_literals,
    ) = translate_osr_loop_inner(
        &synthetic,
        func.regs,
        func.params,
        func.captures,
        OsrLoop {
            header: region.entry,
            exit: synthetic.len(),
        },
        Vec::new(),
        HashMap::new(),
        param_native_types,
        &immutable_leaf_params,
        Some(facts),
        false,
    )?;
    if !derived_liveins.is_empty()
        || !scalar_fields.is_empty()
        || !string_literals.is_empty()
        || reg_types.iter().any(|ty| {
            !matches!(
                ty,
                NativeTy::Int | NativeTy::Bool | NativeTy::Float | NativeTy::Handle
            )
        })
    {
        return None;
    }
    let liveness = NativeRegionAnalysis::compute_prefix(&func.code, func.regs, 0, func.code.len())?;
    if jit_fn.code.len() != func.code.len() {
        return None;
    }
    // The continuation lowering above preserves source instruction identity.
    // Attach verifier-derived source-resume liveness to every guard so checked
    // arithmetic does not capture every historical temporary in a long region.
    // Local JIT liveness is unioned by the JIT validator, while unmodified frame
    // registers remain authoritative in the VM and need no payload write-back.
    jit_fn.resume_live_regs = (0..jit_fn.code.len())
        .map(|ip| {
            (0..func.regs)
                .filter(|reg| {
                    written_regs.get(*reg).copied().unwrap_or(false)
                        && liveness.live_in(ip, *reg) == Some(true)
                })
                .map(|reg| u32::try_from(reg).ok())
                .collect::<Option<Vec<_>>>()
        })
        .collect::<Option<Vec<_>>>()?;
    for &exit in region.exits.keys() {
        let live = (0..func.regs)
            .filter(|reg| {
                written_regs.get(*reg).copied().unwrap_or(false)
                    && liveness.live_in(exit, *reg) == Some(true)
            })
            .map(|reg| u32::try_from(reg).ok())
            .collect::<Option<Vec<_>>>()?;
        *jit_fn.code.get_mut(exit)? = vm_jit::JitInstr::RegionExit {
            exit_id: u32::try_from(exit).ok()?,
            live,
        };
    }
    let mut ordered_vm_regs = region.live_in_regs.clone();
    ordered_vm_regs.extend(
        region
            .active_regs
            .iter()
            .enumerate()
            .filter_map(|(reg, active)| {
                (*active && !region.live_in_regs.contains(&reg)).then_some(reg)
            }),
    );
    let compact_slots = ordered_vm_regs
        .iter()
        .map(|reg| {
            Some(ContinuationSlot {
                vm_reg: *reg,
                ty: *reg_types.get(*reg)?,
                written: written_regs.get(*reg).copied().unwrap_or(false),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    if compact_slots
        .iter()
        .any(|slot| slot.written && slot.ty == NativeTy::Handle)
    {
        return None;
    }
    let ordered_old_regs = ordered_vm_regs
        .iter()
        .map(|reg| u32::try_from(*reg).ok())
        .collect::<Option<Vec<_>>>()?;
    jit_fn.compact_registers(
        &ordered_old_regs,
        u32::try_from(region.live_in_regs.len()).ok()?,
    )?;
    let exits = region
        .exits
        .iter()
        .map(|(ip, reason)| {
            let vm_jit::JitInstr::RegionExit { live, .. } = jit_fn.code.get(*ip)? else {
                return None;
            };
            Some((
                *ip,
                ContinuationExit {
                    reason: *reason,
                    live_slots: live.clone().into_boxed_slice(),
                },
            ))
        })
        .collect::<Option<BTreeMap<_, _>>>()?;
    Some((
        jit_fn,
        compact_slots.into_boxed_slice(),
        region.live_in_regs.len(),
        exits,
    ))
}

#[cfg(all(test, feature = "native-jit"))]
mod continuation_tests {
    use super::*;

    #[test]
    fn enforcing_policy_rejects_tiny_acyclic_regions_before_codegen() {
        let region = ContinuationRegion {
            entry: 0,
            included: vec![true; MIN_CONTINUATION_DIRECT_WORK],
            exits: BTreeMap::from([(
                MIN_CONTINUATION_DIRECT_WORK,
                NativeBarrierReason::FunctionReturn,
            )]),
            has_backedge: false,
            active_regs: vec![true],
            live_in_regs: vec![0],
            source_instructions: MIN_CONTINUATION_DIRECT_WORK,
        };
        assert_eq!(
            continuation_decision(&region, NativeCostModel::Enforce, false, false),
            ContinuationDecision::Ignore
        );
        assert_eq!(
            continuation_decision(&region, NativeCostModel::Off, false, false),
            ContinuationDecision::Compile
        );
    }

    #[test]
    fn optimized_continuations_require_controlled_retention_evidence() {
        assert_eq!(
            continuation_tier_decision(None),
            ContinuationTierDecision::BaselineOnly
        );
        assert_eq!(
            continuation_tier_decision(Some(1_149)),
            ContinuationTierDecision::BaselineOnly
        );
        assert_eq!(
            continuation_tier_decision(Some(1_150)),
            ContinuationTierDecision::Promote
        );
    }

    #[test]
    fn continuation_entries_follow_nonadjacent_cfg_successors() {
        let code = vec![
            RegInstr::MatchOption {
                src: 0,
                some_ip: 3,
                none_ip: 5,
            },
            RegInstr::RuntimeError {
                message: "unreachable".into(),
            },
            RegInstr::RuntimeError {
                message: "unreachable".into(),
            },
            RegInstr::LoadInt { dst: 1, value: 1 },
            RegInstr::Return { src: 1 },
            RegInstr::Return { src: 0 },
        ];
        let entries = ContinuationEntrySet::from_code(&code);
        assert!(entries.contains(0));
        assert!(entries.contains(3));
        assert!(entries.contains(5));
        assert!(!entries.contains(1));
    }

    #[test]
    fn armed_backedge_regions_never_suppress_bounded_execution() {
        let region = ContinuationRegion {
            entry: 0,
            included: vec![true; MIN_CONTINUATION_ADMISSION_WORK],
            exits: BTreeMap::from([(
                MIN_CONTINUATION_ADMISSION_WORK,
                NativeBarrierReason::FunctionReturn,
            )]),
            has_backedge: true,
            active_regs: vec![true],
            live_in_regs: vec![0],
            source_instructions: MIN_CONTINUATION_ADMISSION_WORK,
        };
        assert_eq!(
            continuation_decision(&region, NativeCostModel::Enforce, true, false),
            ContinuationDecision::Ignore
        );
        assert_eq!(
            continuation_decision(&region, NativeCostModel::Enforce, false, true),
            ContinuationDecision::Ignore
        );
    }

    #[test]
    fn scalar_cfg_region_keeps_branches_and_records_multiple_normal_exits() {
        let code = vec![
            RegInstr::LoadInt { dst: 0, value: 7 },
            RegInstr::LoadInt { dst: 1, value: 3 },
            RegInstr::AddInt {
                dst: 4,
                lhs: 0,
                rhs: 1,
            },
            RegInstr::AddInt {
                dst: 5,
                lhs: 4,
                rhs: 1,
            },
            RegInstr::AddInt {
                dst: 6,
                lhs: 5,
                rhs: 1,
            },
            RegInstr::AddInt {
                dst: 7,
                lhs: 6,
                rhs: 1,
            },
            RegInstr::AddInt {
                dst: 8,
                lhs: 7,
                rhs: 1,
            },
            RegInstr::AddInt {
                dst: 9,
                lhs: 8,
                rhs: 1,
            },
            RegInstr::AddInt {
                dst: 10,
                lhs: 9,
                rhs: 1,
            },
            RegInstr::AddInt {
                dst: 11,
                lhs: 10,
                rhs: 1,
            },
            RegInstr::LessInt {
                dst: 2,
                lhs: 1,
                rhs: 0,
            },
            RegInstr::JumpIfBool {
                cond: 2,
                expected: true,
                target: 15,
            },
            RegInstr::AddInt {
                dst: 3,
                lhs: 0,
                rhs: 1,
            },
            RegInstr::MulInt {
                dst: 3,
                lhs: 3,
                rhs: 1,
            },
            RegInstr::Return { src: 3 },
            RegInstr::SubInt {
                dst: 3,
                lhs: 0,
                rhs: 1,
            },
            RegInstr::MulInt {
                dst: 3,
                lhs: 3,
                rhs: 1,
            },
            RegInstr::CallKnown {
                dst: 4,
                function: 1,
                args: vec![3],
                mut_args: Vec::new(),
            },
        ];
        let region = detect_scalar_continuation_region(&code, 9, 0).expect("scalar CFG region");
        assert!(region.included[11], "conditional branch stays native");
        assert_eq!(
            region.exits.get(&14),
            Some(&NativeBarrierReason::FunctionReturn)
        );
        assert_eq!(
            region.exits.get(&17),
            Some(&NativeBarrierReason::StaticCall)
        );
    }

    #[test]
    fn continuation_entries_do_not_overlap_or_ping_pong_across_a_backedge_barrier() {
        let code = vec![
            RegInstr::CallKnown {
                dst: 0,
                function: 1,
                args: Vec::new(),
                mut_args: Vec::new(),
            },
            RegInstr::LoadInt { dst: 1, value: 1 },
            RegInstr::AddInt {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            RegInstr::AddInt {
                dst: 3,
                lhs: 2,
                rhs: 1,
            },
            RegInstr::Jump { target: 0 },
        ];
        let entries = ContinuationEntrySet::from_code(&code);
        assert!(entries.contains(0));
        assert!(entries.contains(1));
        assert!(!entries.contains(2));
        assert!(detect_scalar_continuation_region(&code, 2, 1).is_none());
    }

    #[test]
    fn closed_native_loop_can_yield_once_at_a_forward_barrier() {
        let mut code = vec![
            RegInstr::LoadInt { dst: 0, value: 0 },
            RegInstr::LoadInt { dst: 1, value: 100 },
            RegInstr::LessInt {
                dst: 2,
                lhs: 0,
                rhs: 1,
            },
            RegInstr::JumpIfBool {
                cond: 2,
                expected: false,
                target: 18,
            },
        ];
        for dst in 3..15 {
            code.push(RegInstr::AddInt {
                dst,
                lhs: 0,
                rhs: 1,
            });
        }
        code.push(RegInstr::AddInt {
            dst: 0,
            lhs: 0,
            rhs: 1,
        });
        code.push(RegInstr::Jump { target: 2 });
        code.push(RegInstr::CallKnown {
            dst: 15,
            function: 1,
            args: vec![0],
            mut_args: Vec::new(),
        });
        assert_eq!(code.len(), 19);

        let region = detect_scalar_continuation_region(&code, 3, 0).expect("closed native loop");
        assert!(region.has_backedge);
        assert_eq!(
            region.exits.get(&18),
            Some(&NativeBarrierReason::StaticCall)
        );
    }
}

/// A compiled OSR loop cached per function. The OSR loop is detected and compiled
/// on the (possibly scalar replacement-scalar-replaced) `code`, so its native `resume_ip` indexes
/// that transformed stream. The interpreter, however, executes the ORIGINAL
/// `func.code`; the two stored `orig_*` ips translate the OSR boundary back:
///   - `orig_header`: the original-code header ip the interpreter must be at for
///     the OSR to fire (the header gate). When no Option was scalar-replaced this
///     equals `trans_exit`'s loop header in original space (identity ip-map).
///   - `trans_exit`: the transformed-code exit ip (= the native `resume_ip` the
///     OSR-exit deopt reports). Used to validate the deopt resumed at the loop's
///     single exit.
///   - `orig_exit`: the ORIGINAL-code post-loop ip the interpreter resumes at —
///     `ip_map[trans_exit]`. Set the frame ip to this after an OSR-exit.
/// Loop-carried (live-in/out) registers keep their original indices (scalar replacement only adds
/// fresh tag/payload regs used strictly inside the loop and dead at both
/// boundaries), so the marshalling window and live-out restore are unchanged.
#[cfg(feature = "native-jit")]
#[derive(Debug, Clone)]
pub(in crate::reg_vm) struct OsrEntry {
    pub(in crate::reg_vm) id: vm_jit::CompiledId,
    pub(in crate::reg_vm) orig_header: usize,
    pub(in crate::reg_vm) trans_exit: usize,
    pub(in crate::reg_vm) orig_exit: usize,
    /// Width of the OSR register window the native ABI expects — the TRANSFORMED
    /// register count (`func.regs` plus any scalar replacement-added tag/payload regs). The
    /// marshalling window and `lens` slice must be exactly this wide.
    pub(in crate::reg_vm) n_jit_regs: usize,
    pub(in crate::reg_vm) param_types: Vec<NativeTy>,
    pub(in crate::reg_vm) derived_liveins: Vec<OsrDerivedLiveIn>,
    pub(in crate::reg_vm) scalar_fields: Vec<OsrScalarField>,
    pub(in crate::reg_vm) heap_input_regs: Vec<usize>,
    /// Per-register native types of the compiled OSR body. Used at OSR-exit to skip
    /// restoring **Handle**-class registers: a loop-internal handle (a stored
    /// struct/closure fetched via `FieldHandle`/`ListGetHandle`) is dead at the exit
    /// and its live-out "value" is only a heap-table index — restoring it as an Int
    /// into the interpreter slot would corrupt the register. The interpreter re-
    /// derives any still-needed heap value; a dead one is simply never read.
    pub(in crate::reg_vm) reg_types: Vec<NativeTy>,
    /// Registers written by the native OSR loop body. Live-through registers that
    /// are assigned before the loop but never written natively must not be restored
    /// from the scalar deopt payload: a heap/list live-through slot is already
    /// correct in the interpreter window, while its native payload word may be an
    /// opaque handle-table index or an untyped zero.
    pub(in crate::reg_vm) written_regs: Vec<bool>,
    pub(in crate::reg_vm) string_literals: Vec<Rc<String>>,
    /// Bounded clean-exit reconstruction trees for scalar-replaced aggregates that
    /// remain live after the OSR region. Every leaf is verified as a scalar or Handle
    /// register before this entry is cached.
    pub(in crate::reg_vm) materialize_recipes: Vec<super::passes::OsrMaterializeRecipe>,
}

/// Detect one natural loop at a specific header, allowing other disjoint loops
/// elsewhere in the same function. This is intentionally narrower than arbitrary
/// CFG loop discovery: it uses the same single-entry/single-exit validation as
/// [`detect_single_natural_loop`] for the selected `[header, exit)` region, but it
/// does not reject merely because a setup loop exists before the hot loop.
#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn detect_natural_loop_at(
    code: &[RegInstr],
    header: usize,
) -> Option<OsrLoop> {
    let n = code.len();
    if header >= n {
        return None;
    }
    let backedges = NativeRegionCfg::prefix(code, code.len())?.backedges_to(header);
    if backedges.is_empty() {
        return None;
    }
    let body_end = backedges.into_iter().max().unwrap();

    let mut cond_ip = header;
    while cond_ip < n
        && !matches!(
            code[cond_ip],
            RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. }
        )
    {
        cond_ip += 1;
    }
    if cond_ip > body_end {
        return None;
    }
    let exit = match &code[cond_ip] {
        RegInstr::JumpIfIntCompare { target, .. } | RegInstr::JumpIfBool { target, .. } => *target,
        _ => return None,
    };
    if exit <= body_end || exit > n {
        return None;
    }

    for i in header..=body_end {
        let in_region = |t: usize| t >= header && t < exit;
        match &code[i] {
            RegInstr::Jump { target } => {
                if !in_region(*target) {
                    return None;
                }
            }
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                if i == cond_ip {
                    continue;
                }
                if !in_region(*target) {
                    return None;
                }
            }
            RegInstr::Return { .. } => return None,
            RegInstr::RuntimeError { .. } => {}
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } if (!in_region(*some_ip) || !in_region(*none_ip)) => {
                return None;
            }
            _ => {}
        }
    }

    for (i, instr) in code.iter().enumerate() {
        if i >= header && i < exit {
            continue;
        }
        let enters_interior = |t: usize| t > header && t < exit;
        let bad = match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => enters_interior(*target),
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => enters_interior(*some_ip) || enters_interior(*none_ip),
            _ => false,
        };
        if bad {
            return None;
        }
    }
    Some(OsrLoop { header, exit })
}

#[cfg(feature = "native-jit")]
pub(in crate::reg_vm) fn detect_natural_loops(code: &[RegInstr]) -> Vec<OsrLoop> {
    let mut headers = Vec::new();
    for (i, instr) in code.iter().enumerate() {
        let mut push_header = |target: usize| {
            if target <= i && !headers.contains(&target) {
                headers.push(target);
            }
        };
        match instr {
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                push_header(*some_ip);
                push_header(*none_ip);
            }
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => push_header(*target),
            _ => {}
        }
    }
    headers.sort_unstable();
    headers
        .into_iter()
        .filter_map(|header| detect_natural_loop_at(code, header))
        .collect()
}

/// Identify the single natural loop OSR will compile, **conservatively** (any
/// shape we cannot analyze soundly returns `None`, so OSR does not apply).
///
/// The accepted shape is a **reducible natural loop with a single header `h`**,
/// lowered as `while cond { body }` (the body may contain internal forward control
/// flow, e.g. an `if x { ... }` reset):
///   - `header` `h`: a `JumpIfIntCompare`/`JumpIfBool` at `h` whose `target` is the
///     post-loop `exit` (the branch *leaves* the loop; fall-through stays in body),
///   - one or more **backedges** `b → h` (a `Jump`/`JumpIf*`/`MatchOption` arm whose
///     target is `≤ b`), **ALL targeting the same header `h`** (multiple backedges
///     are collapsed; backedges to two different headers ⇒ nested/multiple loops ⇒
///     reject), and
///   - the contiguous region `[h, exit)` is **single-exit**: the ONLY edge leaving
///     it is the header's exit edge to `exit`. Every other in-body branch (forward
///     `if`/`match`, or a backedge) stays within `[h, exit)`. No in-body
///     `Return` (a value-producing extra exit) is allowed.
///
/// A single header (all backedges collapsed), a single exit edge, and a contiguous
/// `[h, exit)` body make the region single-entry/single-exit — the only thing we can
/// OSR soundly. Multi-header / multi-exit / non-contiguous shapes return `None`.
#[cfg(feature = "native-jit")]
#[cfg(any(test, feature = "jit-diagnostics"))]
pub(in crate::reg_vm) fn detect_single_natural_loop(code: &[RegInstr]) -> Option<OsrLoop> {
    let n = code.len();
    // Collect backedges from the shared native CFG descriptor. This matters when
    // this runs on UNTRANSFORMED code (OSR x scalar replacement, before scalar replacement): a
    // match arm that jumps backward must be treated like any other control edge.
    let backedges = NativeRegionCfg::prefix(code, code.len())?.backedges();
    // At least one backedge ⇒ a loop exists.
    if backedges.is_empty() {
        return None;
    }
    // Collapse multiple backedges to the SAME header. Backedges to two DIFFERENT
    // headers mean nested/sibling loops — out of scope, reject. The single shared
    // header `h` is the loop entry; `body_end` is the furthest backedge source, so
    // the contiguous loop body is `h..=body_end`.
    let header = backedges[0].1;
    if backedges.iter().any(|&(_, h)| h != header) {
        return None;
    }
    if header >= n {
        return None;
    }
    let body_end = backedges.iter().map(|&(from, _)| from).max().unwrap();
    // The header BLOCK is a (possibly empty) leading run of STRAIGHT-LINE instructions
    // (no jump/branch/match/return) followed by the loop's conditional branch. This
    // admits a `while cond { body }` whose CONDITION computes a value before the
    // compare — e.g. `while i < Bytes.len(data)` lowers to `BytesLen -> t; JumpIf i <
    // t` with the backedge targeting the `BytesLen`. The Bytes length-fold then (a)
    // rewrites that in-header `Bytes.len` to a `Move` from a constant length register
    // and (b) materializes the constant length as a `LoadInt` AT the header, so the
    // value is definitely-assigned on entry to the native OSR header block (which is
    // where OSR-entry lands) and dominates the condition's read. Because the prefix is
    // straight-line, the block is single-entry (the backedge targets `header`, the
    // sole entry); if the prefix is not foldable to the native subset, `translate_osr_
    // loop` rejects it and the loop simply stays on the interpreter (safe). A loop
    // whose condition is a bare compare has `cond_ip == header`, exactly as before, so
    // ordinary loops are unaffected.
    let mut cond_ip = header;
    while cond_ip < n
        && !matches!(
            code[cond_ip],
            RegInstr::Jump { .. }
                | RegInstr::JumpIfBool { .. }
                | RegInstr::JumpIfIntCompare { .. }
                | RegInstr::MatchOption { .. }
                | RegInstr::MatchResult { .. }
                | RegInstr::MatchVariant { .. }
                | RegInstr::MatchMapGet { .. }
                | RegInstr::MatchSortedMapGet { .. }
                | RegInstr::Return { .. }
                | RegInstr::RuntimeError { .. }
        )
    {
        cond_ip += 1;
    }
    if cond_ip > body_end {
        return None;
    }
    // The condition must be a `JumpIfIntCompare`/`JumpIfBool` whose `target` is the
    // post-loop exit (the fall-through stays in the loop body). The exit must lie
    // outside the body.
    let exit = match &code[cond_ip] {
        RegInstr::JumpIfIntCompare { target, .. } | RegInstr::JumpIfBool { target, .. } => *target,
        _ => return None,
    };
    // The loop body is `header..=body_end`; the exit must be after it (the loop's
    // only way out). A header whose exit target points back inside the body is not
    // the while-shape we accept.
    if exit <= body_end || exit > n {
        return None;
    }
    // The set of backedge source indices (each must be a Jump/JumpIf* back to the
    // header; checked in-region below — a backedge to `header` is in `[header, exit)`,
    // so it is NOT an escaping edge and needs no special exemption).
    //
    // No instruction in the body `header..=body_end` may transfer control outside
    // `[header, exit)` except the header's own exit edge. (Any other escape would
    // mean multiple exits / an irreducible shape.) Internal forward branches and
    // backedges to `header` stay in-region, so they pass the same `in_region` test.
    for i in header..=body_end {
        let in_region = |t: usize| t >= header && t < exit;
        match &code[i] {
            RegInstr::Jump { target } => {
                if !in_region(*target) {
                    return None;
                }
            }
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                // The header condition's exit edge to `exit` is the sole permitted
                // escape (the condition sits at `cond_ip`, after any `LoadInt` prefix).
                if i == cond_ip {
                    continue;
                }
                if !in_region(*target) {
                    return None;
                }
            }
            // A `Return` inside the loop is a value-producing exit we do not model in
            // the single OSR-exit — bail conservatively.
            RegInstr::Return { .. } => return None,
            // A `RuntimeError` inside the loop is a trap, not a normal loop exit. It
            // is compiled to `JitInstr::Bail` (deopt to the interpreter, which then
            // re-runs the loop and raises the error itself if actually reached). The
            // exhaustive-match lowering emits a statically-reachable-but-dynamically-
            // dead `RuntimeError` after an `Option` match, so accepting it (as a bail)
            // is what lets Option-bearing loops OSR at all.
            RegInstr::RuntimeError { .. } => {}
            // `MatchOption` (untransformed-code path): both arms must stay in-region.
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } if !in_region(*some_ip) || !in_region(*none_ip) => return None,
            RegInstr::MatchOption { .. } => {}
            // Any non-straight-line call inside the body is rejected by the subset
            // check in `translate_osr_loop`; control-flow-wise it falls through,
            // which stays in-region.
            _ => {}
        }
    }
    // Single-ENTRY check: OSR enters the region only at `header`. No instruction
    // OUTSIDE `[header, exit)` may branch INTO the body interior `(header, exit)`
    // (an edge to `header` itself is the legal loop entry / fall-through). An
    // external edge into the middle would make the region multi-entry and the
    // contiguous-region/ip-map assumptions unsound, so reject. (Lowered while-loops
    // never do this; this guards an irreducible CFG defensively.)
    for (i, instr) in code.iter().enumerate() {
        if i >= header && i < exit {
            continue; // in-body edges already validated above
        }
        let enters_interior = |t: usize| t > header && t < exit;
        let bad = match instr {
            RegInstr::Jump { target }
            | RegInstr::JumpIfBool { target, .. }
            | RegInstr::JumpIfIntCompare { target, .. } => enters_interior(*target),
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            } => enters_interior(*some_ip) || enters_interior(*none_ip),
            _ => false,
        };
        if bad {
            return None;
        }
    }
    Some(OsrLoop { header, exit })
}
