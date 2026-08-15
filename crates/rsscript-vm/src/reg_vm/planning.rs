use std::borrow::Borrow;

use super::*;

/// Per-program JIT eligibility: how many functions are fully covered by the
/// tier-0 JIT-supported instruction subset (and so are candidates for native
/// codegen) versus how many must fall back to the interpreter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JitPlan {
    pub total_functions: usize,
    pub eligible_functions: usize,
    pub fallback_functions: usize,
}

/// Whether `code` contains a back-edge (a jump/branch whose target is at or
/// before it) — i.e. a loop. The tier-0 JIT only pays off across loop iterations,
/// so straight-line functions are left on the interpreter.
pub(super) fn jit_function_has_loop(code: &[RegInstr]) -> bool {
    code.iter().enumerate().any(|(index, instr)| match instr {
        RegInstr::Jump { target } => *target <= index,
        RegInstr::JumpIfBool { target, .. } => *target <= index,
        RegInstr::JumpIfIntCompare { target, .. } => *target <= index,
        RegInstr::MatchOption {
            some_ip, none_ip, ..
        } => *some_ip <= index || *none_ip <= index,
        RegInstr::MatchResult { ok_ip, err_ip, .. } => *ok_ip <= index || *err_ip <= index,
        RegInstr::MatchVariant {
            match_ip, else_ip, ..
        } => *match_ip <= index || *else_ip <= index,
        RegInstr::MatchMapGet {
            some_ip, none_ip, ..
        }
        | RegInstr::MatchSortedMapGet {
            some_ip, none_ip, ..
        } => *some_ip <= index || *none_ip <= index,
        _ => false,
    })
}

/// Per-function tier-0 JIT eligibility for a whole unit, returned indexed by
/// function id.
///
/// A function is eligible when both hold:
///  1. **Non-suspending.** Every instruction is in the pure subset
///     ([`jit_supported_instruction`]) or is a `CallKnown` to another eligible
///     (non-suspending) function. A least-fixpoint computes this. Such a function
///     — and its entire reachable call graph — contains no `await`/spawn/blocking
///     op, so the tier-0 executor can run callees to completion via `run_frame`
///     without ever needing to suspend.
///  2. **Non-recursive.** The executor runs callee frames on the host stack, so
///     JIT-ing a call cycle could overflow the host stack where the stackless
///     interpreter would not — a behavioural gap. We therefore drop any function
///     that can reach a cycle in the (non-suspending) call graph; recursive
///     functions keep running on the interpreter, which is gap-free by fallback.
pub(super) fn compute_jit_eligibility<T: Borrow<RegFunction>>(functions: &[T]) -> Vec<bool> {
    let n = functions.len();

    // (1) Non-suspending least-fixpoint: start optimistic, demote any function
    // with an unsupported instruction or a call to an already-demoted function,
    // until stable (monotone, so it terminates).
    let mut non_suspending = vec![true; n];
    loop {
        let mut changed = false;
        for index in 0..n {
            if !non_suspending[index] {
                continue;
            }
            let ok = functions[index].borrow().code.iter().all(|instr| match instr {
                RegInstr::CallKnown { function, .. } => *function < n && non_suspending[*function],
                other => jit_supported_instruction(other),
            });
            if !ok {
                non_suspending[index] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // (2) Call-graph edges among non-suspending functions only.
    let edges: Vec<Vec<usize>> = (0..n)
        .map(|index| {
            if !non_suspending[index] {
                return Vec::new();
            }
            let mut targets = Vec::new();
            for instr in &functions[index].borrow().code {
                if let RegInstr::CallKnown { function, .. } = instr
                    && non_suspending[*function]
                {
                    targets.push(*function);
                }
            }
            targets
        })
        .collect();

    let reaches_cycle = functions_reaching_call_cycle(&edges);

    (0..n)
        .map(|index| non_suspending[index] && !reaches_cycle[index])
        .collect()
}

/// Classify nodes that are cyclic or can reach a cycle in `O(V + E)` space and
/// time. Repeatedly removing sinks is equivalent to pruning the acyclic portion
/// of the call graph: in a finite graph, every node left afterward has a path to
/// a cycle, and no removed node does.
pub(super) fn functions_reaching_call_cycle(edges: &[Vec<usize>]) -> Vec<bool> {
    let n = edges.len();
    let mut reverse_edges = vec![Vec::new(); n];
    let mut remaining_out_degree = vec![0usize; n];

    for (source, targets) in edges.iter().enumerate() {
        remaining_out_degree[source] = targets.len();
        for &target in targets {
            debug_assert!(target < n);
            reverse_edges[target].push(source);
        }
    }

    let mut reaches_cycle = vec![true; n];
    let mut sinks = VecDeque::new();
    for (node, &degree) in remaining_out_degree.iter().enumerate() {
        if degree == 0 {
            reaches_cycle[node] = false;
            sinks.push_back(node);
        }
    }

    while let Some(removed) = sinks.pop_front() {
        for &caller in &reverse_edges[removed] {
            if !reaches_cycle[caller] {
                continue;
            }
            remaining_out_degree[caller] -= 1;
            if remaining_out_degree[caller] == 0 {
                reaches_cycle[caller] = false;
                sinks.push_back(caller);
            }
        }
    }

    reaches_cycle
}

/// Instructions reachable from `ip == 0` along the control-flow graph
/// (sequential fallthrough, jumps, conditional branches, branch-shaped match
/// arms). Mirrors [`native_reachable_instructions`] but is always compiled (the
pub(super) fn instr_reads_register(instr: &RegInstr, reg: Reg) -> bool {
    match instr {
        RegInstr::LoadUnit { .. }
        | RegInstr::LoadInt { .. }
        | RegInstr::LoadFloat { .. }
        | RegInstr::LoadBool { .. }
        | RegInstr::LoadString { .. }
        | RegInstr::LoadChar { .. }
        | RegInstr::LoadNone { .. }
        | RegInstr::Jump { .. }
        | RegInstr::RuntimeError { .. } => false,
        RegInstr::Move { src, .. }
        | RegInstr::Manage { src, .. }
        | RegInstr::MakeSome { value: src, .. }
        | RegInstr::UnwrapSome { src, .. }
        | RegInstr::UnwrapVariantValue { src, .. }
        | RegInstr::AwaitJoin { src, .. } => *src == reg,
        RegInstr::DeepCopy { reg: r } | RegInstr::DeepCopyElided { reg: r } => *r == reg,
        RegInstr::GetField { base, .. } | RegInstr::GetFieldSlot { base, .. } => *base == reg,
        RegInstr::SetField { base, value, .. } | RegInstr::SetFieldSlot { base, value, .. } => {
            *base == reg || *value == reg
        }
        RegInstr::AddInt { lhs, rhs, .. }
        | RegInstr::SubInt { lhs, rhs, .. }
        | RegInstr::MulInt { lhs, rhs, .. }
        | RegInstr::DivInt { lhs, rhs, .. }
        | RegInstr::ModInt { lhs, rhs, .. }
        | RegInstr::BitAndInt { lhs, rhs, .. }
        | RegInstr::BitOrInt { lhs, rhs, .. }
        | RegInstr::BitXorInt { lhs, rhs, .. }
        | RegInstr::ShiftLeftInt { lhs, rhs, .. }
        | RegInstr::ShiftRightInt { lhs, rhs, .. }
        | RegInstr::LessInt { lhs, rhs, .. }
        | RegInstr::LessEqualInt { lhs, rhs, .. }
        | RegInstr::GreaterInt { lhs, rhs, .. }
        | RegInstr::GreaterEqualInt { lhs, rhs, .. }
        | RegInstr::Equal { lhs, rhs, .. }
        | RegInstr::NotEqual { lhs, rhs, .. }
        | RegInstr::JumpIfIntCompare { lhs, rhs, .. } => *lhs == reg || *rhs == reg,
        RegInstr::JumpIfBool { cond, .. } => *cond == reg,
        RegInstr::Return { src } => *src == reg,
        RegInstr::MatchOption { src, .. }
        | RegInstr::MatchResult { src, .. }
        | RegInstr::MatchVariant { src, .. } => *src == reg,
        RegInstr::MatchMapGet { map, key, .. } | RegInstr::MatchSortedMapGet { map, key, .. } => {
            *map == reg || *key == reg
        }
        RegInstr::MakeStruct { fields, .. } | RegInstr::MakeVariant { fields, .. } => {
            fields.iter().any(|(_, r)| *r == reg)
        }
        RegInstr::MakeObject { fields, .. } => fields.iter().any(|(_, r)| *r == reg),
        RegInstr::MakeMap { entries, .. } => entries.iter().any(|(k, v)| *k == reg || *v == reg),
        RegInstr::MakeList { items, .. } => items.contains(&reg),
        RegInstr::MakeClosure { captures, .. } => captures.contains(&reg),
        RegInstr::ResourceDrop { resource } => *resource == reg,
        RegInstr::CallKnown { args, .. }
        | RegInstr::CallDynamic { args, .. }
        | RegInstr::CallExternal { args, .. }
        | RegInstr::SpawnTask { args, .. } => args.contains(&reg),
        RegInstr::CallClosure { closure, args, .. } => *closure == reg || args.contains(&reg),
        RegInstr::SelectWait { handles, .. } => handles.contains(&reg),
        // Synthetic native-only ops never appear in lowered code seen by TCO, but
        // enumerate them for completeness.
        RegInstr::NativeGuardClosureId { closure, .. }
        | RegInstr::NativeClosureId { closure, .. }
        | RegInstr::NativeClosureCapture { closure, .. } => *closure == reg,
        // Any operand-bearing variant not enumerated above is treated as a use,
        // so the TCO gate conservatively bails.
        _ => true,
    }
}

/// Whether an instruction is in the tier-0 JIT-supported subset (the numeric and
/// control-flow core that native codegen targets first). Heap construction,
/// calls, async, resources, and matches fall back to the interpreter.
pub(super) fn jit_supported_instruction(instr: &RegInstr) -> bool {
    matches!(
        instr,
        RegInstr::LoadUnit { .. }
            | RegInstr::LoadInt { .. }
            | RegInstr::LoadFloat { .. }
            | RegInstr::LoadBool { .. }
            | RegInstr::LoadString { .. }
            | RegInstr::Move { .. }
            | RegInstr::DeepCopy { .. }
            | RegInstr::DeepCopyElided { .. }
            | RegInstr::Manage { .. }
            | RegInstr::GetField { .. }
            | RegInstr::SetField { .. }
            | RegInstr::GetFieldSlot { .. }
            | RegInstr::SetFieldSlot { .. }
            | RegInstr::MakeStruct { .. }
            | RegInstr::MakeVariant { .. }
            | RegInstr::MakeList { .. }
            | RegInstr::MakeObject { .. }
            | RegInstr::MakeMap { .. }
            | RegInstr::MakeSome { .. }
            | RegInstr::LoadNone { .. }
            | RegInstr::MakeClosure { .. }
            | RegInstr::MatchOption { .. }
            | RegInstr::MatchResult { .. }
            | RegInstr::MatchVariant { .. }
            | RegInstr::MatchMapGet { .. }
            | RegInstr::MatchSortedMapGet { .. }
            | RegInstr::UnwrapSome { .. }
            | RegInstr::UnwrapVariantValue { .. }
            | RegInstr::RuntimeError { .. }
            // Collection get/set/index ops (closure-free; closure-driven
            // map/filter/fold/sort_by/sort_with still fall back to the interpreter).
            | RegInstr::ListGet { .. }
            | RegInstr::ListLen { .. }
            | RegInstr::ListPush { .. }
            | RegInstr::ListSort { .. }
            | RegInstr::ListAppend { .. }
            | RegInstr::ListClear { .. }
            | RegInstr::ListPop { .. }
            | RegInstr::ListRemoveAt { .. }
            | RegInstr::ListSet { .. }
            | RegInstr::MapGet { .. }
            | RegInstr::MapClear { .. }
            | RegInstr::MapInsert { .. }
            | RegInstr::MapInsertOld { .. }
            | RegInstr::MapRemove { .. }
            | RegInstr::AddInt { .. }
            | RegInstr::SubInt { .. }
            | RegInstr::MulInt { .. }
            | RegInstr::DivInt { .. }
            | RegInstr::ModInt { .. }
            | RegInstr::BitAndInt { .. }
            | RegInstr::BitOrInt { .. }
            | RegInstr::BitXorInt { .. }
            | RegInstr::ShiftLeftInt { .. }
            | RegInstr::ShiftRightInt { .. }
            | RegInstr::LessInt { .. }
            | RegInstr::LessEqualInt { .. }
            | RegInstr::GreaterInt { .. }
            | RegInstr::GreaterEqualInt { .. }
            | RegInstr::Equal { .. }
            | RegInstr::NotEqual { .. }
            | RegInstr::Jump { .. }
            | RegInstr::JumpIfBool { .. }
            | RegInstr::JumpIfIntCompare { .. }
            | RegInstr::Return { .. }
    )
}

#[cfg(test)]
mod jit_eligibility_tests {
    use super::*;

    fn function(callees: impl IntoIterator<Item = usize>) -> RegFunction {
        let mut function = RegFunction::placeholder(String::new());
        function.code = callees
            .into_iter()
            .map(|callee| RegInstr::CallKnown {
                dst: 0,
                function: callee,
                args: Vec::new(),
                mut_args: Vec::new(),
            })
            .collect();
        function
    }

    #[test]
    fn jit_eligibility_accepts_an_acyclic_call_chain() {
        let functions = vec![function([1]), function([2]), function([3]), function([])];

        assert_eq!(
            compute_jit_eligibility(&functions),
            vec![true, true, true, true]
        );
    }

    #[test]
    fn jit_eligibility_rejects_only_star_nodes_that_reach_a_cycle() {
        let functions = vec![
            function([1, 2, 3]),
            function([]),
            function([2]),
            function([]),
        ];

        assert_eq!(
            compute_jit_eligibility(&functions),
            vec![false, true, false, true]
        );
    }

    #[test]
    fn jit_eligibility_handles_sccs_and_shared_callees() {
        let functions = vec![
            function([1]),
            function([0]),
            function([0, 3]),
            function([]),
            function([3]),
            function([3]),
        ];

        assert_eq!(
            compute_jit_eligibility(&functions),
            vec![false, false, false, true, true, true]
        );
    }

    #[test]
    fn call_cycle_classification_scales_with_a_large_chain() {
        const NODE_COUNT: usize = 100_000;
        let mut edges = Vec::with_capacity(NODE_COUNT);
        for node in 0..NODE_COUNT {
            edges.push(if node + 1 == NODE_COUNT {
                Vec::new()
            } else {
                vec![node + 1]
            });
        }

        assert!(
            functions_reaching_call_cycle(&edges)
                .into_iter()
                .all(|reaches_cycle| !reaches_cycle)
        );
    }
}
