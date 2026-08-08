use super::*;
#[cfg(feature = "native-jit")]
fn profile_closure_pic_arm_count(code: &[vm_jit::JitInstr], closure_id_ip: usize) -> u64 {
    let mut ip = closure_id_ip + 1;
    let mut arms = 0;
    while ip + 2 < code.len() {
        let arm = matches!(code[ip], vm_jit::JitInstr::LoadInt { .. })
            && matches!(code[ip + 1], vm_jit::JitInstr::Equal { .. })
            && matches!(
                code[ip + 2],
                vm_jit::JitInstr::JumpIfBool { expected: true, .. }
            );
        if !arm {
            break;
        }
        arms += 1;
        ip += 3;
    }
    arms
}

#[cfg(feature = "native-jit")]
#[derive(Debug, Default, PartialEq, Eq)]
pub(in crate::reg_vm) struct NativeCompileTelemetry {
    pub(in crate::reg_vm) direct_list_bounds_check_sites: u64,
    pub(in crate::reg_vm) memoized_runtime_helper_call_sites: u64,
    pub(in crate::reg_vm) runtime_helper_call_sites: u64,
    pub(in crate::reg_vm) fused_map_match_helper_sites: u64,
    pub(in crate::reg_vm) direct_list_store_load_forwarded_moves: u64,
    native_call_edges: u64,
    profile_closure_guard_sites: u64,
    profile_closure_id_reads: u64,
    profile_closure_pic_sites: u64,
    profile_closure_pic_arms: u64,
    profile_branch_side_exits: u64,
}

#[cfg(feature = "native-jit")]
impl NativeCompileTelemetry {
    pub(in crate::reg_vm) fn from_jit_function(jit_fn: &vm_jit::JitFunction) -> Self {
        let mut telemetry = Self::default();
        for (ip, instr) in jit_fn.code.iter().enumerate() {
            match instr {
                vm_jit::JitInstr::ListGetIntDirect { .. }
                | vm_jit::JitInstr::ListSetIntDirect { .. }
                | vm_jit::JitInstr::ListGetFloatDirect { .. }
                | vm_jit::JitInstr::ListSetFloatDirect { .. } => {
                    telemetry.direct_list_bounds_check_sites += 1;
                }
                vm_jit::JitInstr::MemoizedHostCall { .. } => {
                    telemetry.memoized_runtime_helper_call_sites += 1;
                }
                vm_jit::JitInstr::HostCall { helper, .. } => {
                    telemetry.runtime_helper_call_sites += 1;
                    if *helper == vm_jit::HostHelper::ClosureId {
                        telemetry.profile_closure_id_reads += 1;
                        telemetry.profile_closure_pic_sites += 1;
                        telemetry.profile_closure_pic_arms +=
                            profile_closure_pic_arm_count(&jit_fn.code, ip);
                    }
                }
                vm_jit::JitInstr::MatchMapGetInt { .. }
                | vm_jit::JitInstr::MatchMapGetFloat { .. }
                | vm_jit::JitInstr::MatchSortedMapGetInt { .. }
                | vm_jit::JitInstr::MatchSortedMapGetFloat { .. } => {
                    telemetry.runtime_helper_call_sites += 1;
                    telemetry.fused_map_match_helper_sites += 1;
                }
                vm_jit::JitInstr::CallNative { .. }
                | vm_jit::JitInstr::CallSelf { .. }
                | vm_jit::JitInstr::CallGroup { .. } => telemetry.native_call_edges += 1,
                vm_jit::JitInstr::GuardClosureId { .. } => {
                    telemetry.profile_closure_guard_sites += 1;
                }
                vm_jit::JitInstr::ProfiledJumpIfBool { .. }
                | vm_jit::JitInstr::ProfiledJumpIfIntCompare { .. } => {
                    telemetry.profile_branch_side_exits += 1;
                }
                vm_jit::JitInstr::Move { src, .. } => {
                    let forwarded = jit_fn.code.get(ip.wrapping_sub(1)).is_some_and(|previous| {
                        matches!(
                            previous,
                            vm_jit::JitInstr::ListSetIntDirect {
                                dst: set_dst,
                                value,
                                ..
                            } | vm_jit::JitInstr::ListSetFloatDirect {
                                dst: set_dst,
                                value,
                                ..
                            } if set_dst != value && src == value
                        )
                    });
                    telemetry.direct_list_store_load_forwarded_moves += u64::from(forwarded);
                }
                _ => {}
            }
        }
        telemetry
    }
}

#[cfg(feature = "native-jit")]
pub(super) fn native_region_is_promotion_eligible(jit_fn: &vm_jit::JitFunction) -> bool {
    !jit_fn.code.iter().any(|instr| {
        matches!(
            instr,
            vm_jit::JitInstr::CallNative { .. }
                | vm_jit::JitInstr::CallSelf { .. }
                | vm_jit::JitInstr::CallGroup { .. }
        )
    })
}

#[cfg(feature = "native-jit")]
pub(super) fn record_native_compile_stats(
    native: &mut NativeState,
    id: vm_jit::CompiledId,
    jit_fn: &vm_jit::JitFunction,
    tier: NativeCodeTier,
) {
    if !native.collect_stats {
        return;
    }
    let telemetry = NativeCompileTelemetry::from_jit_function(jit_fn);
    let module = match tier {
        NativeCodeTier::Baseline => &native.baseline_module,
        NativeCodeTier::Optimized => native
            .optimized_module
            .as_ref()
            .expect("optimized compile requires optimized module"),
    };
    native.stats.compiled += 1;
    match tier {
        NativeCodeTier::Baseline => native.stats.baseline_compiles += 1,
        NativeCodeTier::Optimized => native.stats.optimized_compiles += 1,
    }
    native.stats.compiled_ir_instrs += jit_fn.code.len() as u64;
    native.stats.compiled_code_bytes += module.code_size_bytes(id).unwrap_or(0);
    native.stats.direct_list_bounds_check_sites += telemetry.direct_list_bounds_check_sites;
    native.stats.memoized_runtime_helper_call_sites += telemetry.memoized_runtime_helper_call_sites;
    native.stats.runtime_helper_call_sites += telemetry.runtime_helper_call_sites;
    native.stats.fused_map_match_helper_sites += telemetry.fused_map_match_helper_sites;
    native.stats.direct_list_store_load_forwarded_moves +=
        telemetry.direct_list_store_load_forwarded_moves;
    native.stats.profile_branch_cold_blocks += jit_fn.cold_blocks.len() as u64;
    native.stats.profile_branch_side_exits += telemetry.profile_branch_side_exits;
    native.stats.native_call_edges += telemetry.native_call_edges;
    native.stats.native_call_depth_max = native
        .stats
        .native_call_depth_max
        .max(module.native_call_depth(id).map_or(0, u64::from));
    native.stats.profile_closure_guard_sites += telemetry.profile_closure_guard_sites;
    native.stats.profile_closure_id_reads += telemetry.profile_closure_id_reads;
    native.stats.profile_closure_pic_sites += telemetry.profile_closure_pic_sites;
    native.stats.profile_closure_pic_arms += telemetry.profile_closure_pic_arms;
    native.stats.deopt_sites += module.deopt_map(id).map_or(0, |map| map.sites.len() as u64);
}
