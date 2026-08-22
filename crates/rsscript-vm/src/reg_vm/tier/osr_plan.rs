use super::*;

#[cfg(feature = "native-jit")]
pub(super) fn osr_initial_logical_depth(physical_depth: usize, prior_tail_calls: usize) -> usize {
    physical_depth.saturating_add(prior_tail_calls)
}

#[cfg(feature = "native-jit")]
pub(super) fn osr_committed_tail_calls(final_logical_depth: usize, physical_depth: usize) -> usize {
    final_logical_depth.saturating_sub(physical_depth)
}

#[cfg(feature = "native-jit")]
pub(super) fn osr_materialize_recipe_is_supported(
    value: &OsrMaterializeValue,
    reg_types: &[NativeTy],
    depth: usize,
    nodes: &mut usize,
) -> bool {
    if depth >= MAX_OSR_MATERIALIZE_DEPTH || *nodes >= MAX_OSR_MATERIALIZE_NODES {
        return false;
    }
    *nodes += 1;
    match value {
        OsrMaterializeValue::Register(reg) => matches!(
            reg_types.get(*reg),
            Some(NativeTy::Int | NativeTy::Bool | NativeTy::Float | NativeTy::Handle)
        ),
        OsrMaterializeValue::OptionSome(payload) => {
            osr_materialize_recipe_is_supported(payload, reg_types, depth + 1, nodes)
        }
        OsrMaterializeValue::Struct { fields, .. } => fields
            .iter()
            .all(|field| osr_materialize_recipe_is_supported(field, reg_types, depth + 1, nodes)),
        OsrMaterializeValue::Variant { tag_reg, arms } => {
            !arms.is_empty()
                && tag_reg.is_none_or(|reg| {
                    matches!(reg_types.get(reg), Some(NativeTy::Int | NativeTy::Bool))
                })
                && arms.iter().all(|arm| {
                    arm.fields.iter().all(|field| {
                        osr_materialize_recipe_is_supported(field, reg_types, depth + 1, nodes)
                    })
                })
        }
    }
}

#[cfg(feature = "native-jit")]
pub(super) fn osr_materialize_value(
    value: &OsrMaterializeValue,
    live: &[vm_jit::DeoptReg],
    ctx: JitHostCallCtx,
    depth: usize,
    nodes: &mut usize,
) -> Option<VmValue> {
    if depth >= MAX_OSR_MATERIALIZE_DEPTH || *nodes >= MAX_OSR_MATERIALIZE_NODES {
        return None;
    }
    *nodes += 1;
    let deopt_value = |reg: usize| {
        live.iter()
            .find(|deopt| deopt.reg as usize == reg)
            .map(|deopt| deopt.value)
    };
    match value {
        OsrMaterializeValue::Register(reg) => match deopt_value(*reg)? {
            vm_jit::DeoptValue::Int(value) => Some(VmValue::Int(value)),
            vm_jit::DeoptValue::Bool(value) => Some(VmValue::Bool(value)),
            vm_jit::DeoptValue::Float(value) => Some(VmValue::Float(value)),
            vm_jit::DeoptValue::Handle(handle) => {
                ctx.heap_read_handle(handle, |value| Some(value.clone()))
            }
        },
        OsrMaterializeValue::OptionSome(payload) => Some(VmValue::some(osr_materialize_value(
            payload,
            live,
            ctx,
            depth + 1,
            nodes,
        )?)),
        OsrMaterializeValue::Struct { layout, fields } => {
            let fields = fields
                .iter()
                .map(|field| osr_materialize_value(field, live, ctx, depth + 1, nodes))
                .collect::<Option<Vec<_>>>()?;
            Some(VmValue::Struct(Rc::new(VmStruct::with_layout(
                Rc::clone(layout),
                fields,
            ))))
        }
        OsrMaterializeValue::Variant { tag_reg, arms } => {
            let tag = match tag_reg {
                Some(reg) => match deopt_value(*reg)? {
                    vm_jit::DeoptValue::Int(value) => value,
                    vm_jit::DeoptValue::Bool(value) => i64::from(value),
                    _ => return None,
                },
                None if arms.len() == 1 => arms[0].tag,
                None => return None,
            };
            let arm = arms.iter().find(|arm| arm.tag == tag)?;
            let fields = arm
                .fields
                .iter()
                .map(|field| osr_materialize_value(field, live, ctx, depth + 1, nodes))
                .collect::<Option<Vec<_>>>()?;
            Some(VmValue::Variant(Rc::new(VmStruct::with_layout(
                Rc::clone(&arm.layout),
                fields,
            ))))
        }
    }
}

#[cfg(feature = "native-jit")]
pub(super) fn osr_loop_region_is_native_subset(
    code: &[RegInstr],
    lp: OsrLoop,
    n_params: usize,
) -> bool {
    code.get(lp.header..lp.exit).is_some_and(|region| {
        let region_defs = native_osr_region_defined_regs(region);
        region.iter().all(|instr| {
            native_subset_instruction(instr)
                && native_osr_growth_admissible(instr, &region_defs, n_params)
        })
    })
}

/// Registers bound to a freshly-produced value INSIDE a region. An in-place
/// `ListPush` mutates the list object without rebinding the list register, so it is
/// excluded — it must not count as a fresh definition of its target list.
#[cfg(feature = "native-jit")]
pub(super) fn native_osr_region_defined_regs(
    region: &[RegInstr],
) -> std::collections::HashSet<usize> {
    let mut defs = std::collections::HashSet::new();
    for instr in region {
        if matches!(instr, RegInstr::ListPush { .. }) {
            continue;
        }
        if let RegFootprint::Some(regs) = instr_written_reg(instr) {
            defs.extend(regs);
        }
    }
    defs
}

/// Whether a growing list mutation (`ListPush`) is safe to run in an OSR loop.
///
/// Growth reallocates a list's backing buffer. The ONLY lists pinned as a raw flat
/// buffer (whose pointer a realloc would dangle) are flat `List<Int>`/`List<Float>`
/// **parameters** (`reg < n_params`; the TV2 flat classification is params-only — see
/// `flat_param_kind` in `native/translate.rs`). So a `ListPush` is admissible when its
/// target list is EITHER region-local (built natively, never pinned) OR a non-parameter
/// register (`reg >= n_params`, a function-local accumulator that is handle-accessed,
/// not flat-pinned — e.g. a `List<String>` builder). A PARAMETER list stays vetoed
/// (conservative: it may be the pinned flat buffer; growing it is UB). This unblocks
/// native heap-value list building (J0.4 #1) while preserving flat-param safety and the
/// outer-loop selection for flat-param builder/consumer shapes.
#[cfg(feature = "native-jit")]
pub(super) fn native_osr_growth_admissible(
    instr: &RegInstr,
    region_defs: &std::collections::HashSet<usize>,
    n_params: usize,
) -> bool {
    match instr {
        RegInstr::ListPush { list, .. } => region_defs.contains(list) || *list >= n_params,
        _ => true,
    }
}

#[cfg(feature = "native-jit")]
pub(super) fn osr_loop_region_needs_optimized_native_subset_path(
    code: &[RegInstr],
    lp: OsrLoop,
) -> bool {
    code.get(lp.header..lp.exit)
        .is_some_and(|region| region.iter().any(native_instruction_touches_field_slot))
}

#[cfg(feature = "native-jit")]
pub(super) fn mapped_osr_loop(
    code: &[RegInstr],
    ip_map: &[usize],
    old_header: usize,
) -> Option<OsrLoop> {
    let header = ip_map.iter().position(|&old| old == old_header)?;
    detect_natural_loop_at(code, header)
}

#[cfg(feature = "native-jit")]
pub(super) fn osr_heap_input_regs(jit_fn: &vm_jit::JitFunction) -> Vec<usize> {
    let mut regs = Vec::new();
    let mut push_reg = |reg: u32| {
        let reg = reg as usize;
        if jit_fn.reg_types.get(reg) == Some(&vm_jit::JitValueType::Handle) && !regs.contains(&reg)
        {
            regs.push(reg);
        }
    };
    for instr in &jit_fn.code {
        match instr {
            vm_jit::JitInstr::HostCall { args, .. }
            | vm_jit::JitInstr::MemoizedHostCall { args, .. } => {
                for arg in args {
                    if let vm_jit::HostArg::Reg(reg) = arg {
                        push_reg(*reg);
                    }
                }
            }
            vm_jit::JitInstr::MatchMapGetInt { map, .. }
            | vm_jit::JitInstr::MatchMapGetFloat { map, .. }
            | vm_jit::JitInstr::MatchSortedMapGetInt { map, .. }
            | vm_jit::JitInstr::MatchSortedMapGetFloat { map, .. }
            | vm_jit::JitInstr::GuardClosureId { base: map, .. } => push_reg(*map),
            _ => {}
        }
    }
    regs
}

#[cfg(all(test, feature = "native-jit"))]
mod tests {
    use super::*;

    fn map_match_function(instr: vm_jit::JitInstr) -> vm_jit::JitFunction {
        vm_jit::JitFunction {
            n_params: 2,
            n_regs: 3,
            reg_types: vec![
                vm_jit::JitValueType::Handle,
                vm_jit::JitValueType::Int,
                vm_jit::JitValueType::Float,
            ],
            zero_init_regs: Vec::new(),
            code: vec![instr],
            memo_scopes: Vec::new(),
            cold_blocks: Vec::new(),
        }
    }

    #[test]
    fn map_match_heap_inputs_are_symmetric_across_value_types() {
        let variants = [
            vm_jit::JitInstr::MatchMapGetInt {
                map: 0,
                key: 1,
                value_dst: 1,
                some_ip: 0,
                none_ip: 0,
            },
            vm_jit::JitInstr::MatchMapGetFloat {
                map: 0,
                key: 1,
                value_dst: 2,
                some_ip: 0,
                none_ip: 0,
            },
            vm_jit::JitInstr::MatchSortedMapGetInt {
                map: 0,
                key: 1,
                value_dst: 1,
                some_ip: 0,
                none_ip: 0,
            },
            vm_jit::JitInstr::MatchSortedMapGetFloat {
                map: 0,
                key: 1,
                value_dst: 2,
                some_ip: 0,
                none_ip: 0,
            },
        ];

        for instr in variants {
            assert_eq!(osr_heap_input_regs(&map_match_function(instr)), vec![0]);
        }
    }
}
