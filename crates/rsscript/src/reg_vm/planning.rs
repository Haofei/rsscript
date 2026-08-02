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
pub(super) fn compute_jit_eligibility(functions: &[RegFunction]) -> Vec<bool> {
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
            let ok = functions[index].code.iter().all(|instr| match instr {
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
            for instr in &functions[index].code {
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
/// TCO pass runs regardless of the `native-jit` feature). Used to ignore the
/// lowerer's unreachable defensive `LoadUnit; Return` tail when deciding whether
/// a self-tail-recursive function has a genuine base case.
pub(super) fn tco_reachable_instructions(code: &[RegInstr]) -> Vec<bool> {
    let n = code.len();
    let mut reachable = vec![false; n];
    let mut stack = vec![0usize];
    while let Some(i) = stack.pop() {
        if i >= n || reachable[i] {
            continue;
        }
        reachable[i] = true;
        match &code[i] {
            RegInstr::Jump { target } => stack.push(*target),
            RegInstr::JumpIfBool { target, .. } | RegInstr::JumpIfIntCompare { target, .. } => {
                stack.push(*target);
                stack.push(i + 1);
            }
            RegInstr::MatchOption {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchMapGet {
                some_ip, none_ip, ..
            }
            | RegInstr::MatchSortedMapGet {
                some_ip, none_ip, ..
            } => {
                stack.push(*some_ip);
                stack.push(*none_ip);
            }
            RegInstr::MatchResult { ok_ip, err_ip, .. } => {
                stack.push(*ok_ip);
                stack.push(*err_ip);
            }
            RegInstr::MatchVariant {
                match_ip, else_ip, ..
            } => {
                stack.push(*match_ip);
                stack.push(*else_ip);
            }
            // Terminators with no fallthrough.
            RegInstr::Return { .. } | RegInstr::RuntimeError { .. } => {}
            // Everything else falls through to the next instruction.
            _ => stack.push(i + 1),
        }
    }
    reachable
}

/// Self-tail-call optimization (TCO).
///
/// Rewrites every **self-tail-call** in `function` — a `CallKnown` to this same
/// `function_id`, immediately followed by a `Return` of the call's result, with
/// no `mut`-args and no intervening observable use of the result — into an
/// argument rebind plus a backward `Jump` to the function body's entry (just
/// after the one-time parameter prologue). This turns self-tail-recursion into a
/// loop: the function loses its call-graph self-edge, so
/// [`compute_jit_eligibility`] (run *after* this pass) sees it as non-recursive
/// and it becomes native-eligible.
///
/// ## Rewrite shape (index-stable)
/// To avoid renumbering every jump target, the rewrite never deletes or inserts
/// instructions in the middle. For each self-tail-call at `(call_ip, return_ip)`
/// (the `Return` is at `call_ip + 1`):
///   * a rebind block is **appended** at the end of `code`:
///       1. `Move staging_k := args[k]`  (copy every new arg into a fresh temp
///          first, so the rebind is *simultaneous* even when an arg reads a
///          parameter register — e.g. `f(n: n - 1, acc: acc + n)` reads the old
///          `acc`/`n` while computing the new ones);
///       2. `Move param_k := staging_k`  (rebind the parameter registers);
///       3. `Jump { target: entry }`     (the backward loop edge to the body).
///   * the original `CallKnown` is replaced **in place** by `Jump { target: rebind_block }`.
///   * the original `Return` becomes unreachable (left as dead code; the
///     reachability/native passes ignore it).
///
/// Appended instructions only add *forward* edges from the existing body and a
/// single backward edge to `entry`; every pre-existing index is untouched.
///
/// `entry` is the first instruction after the prologue — the leading run of
/// `DeepCopy` instructions emitted once per non-`mut` parameter (see `lower`).
/// The back-edge must NOT re-run that prologue: a rebind value is either a fresh
/// arithmetic result or a value this frame already owns (deep-copied on the
/// original entry), so re-isolating it is unnecessary and skipping it is sound.
///
/// ## Soundness gates (any failure ⇒ the whole function is left untouched, so it
/// keeps running interpreted exactly as today — a strict improvement, never a
/// regression):
///   * **Tail position.** The instruction after the `CallKnown` must be exactly
///     `Return { src: dst }` for the call's `dst`, and that `dst` must be read by
///     no other instruction in the body (nothing observes the result between the
///     call and the return). Mutual/non-tail self-calls (`return f(..) + g(..)`)
///     therefore never qualify — their result is consumed by an arithmetic op.
///   * **Self only.** `function == function_id`.
///   * **No `mut`-args.** `mut_args` must be empty (a `mut` write-back is an
///     observable effect that must run on return; a loop would skip it).
///   * **Arity.** `args.len() == params`.
///   * **Recursion-depth-limit (`VmLimits::max_depth`) observability — the key
///     soundness condition.** An unbounded self-tail-recursion with *no* base
///     case (e.g. `fn f(n){ return f(n: n + 1) }`) today hits the depth cap and
///     returns a clean `"recursion depth limit exceeded"` error (this is an
///     asserted safety property — `hostile::deep_recursion_returns_clean_error_not_crash`).
///     Converting such a function to a loop would replace that error with an
///     infinite loop (a hang / observability change). We therefore **refuse to
///     TCO any function whose every `Return` is a self-tail-call** — i.e. one
///     with no non-tail-call return reachable, hence no base case at all. Such a
///     function stays interpreted and keeps tripping the depth cap, preserving
///     the limit's observability byte-for-byte. A function *with* a base-case
///     return (`sum_to`'s `return acc`) is TCO'd; for it the depth limit was only
///     ever reachable on a non-terminating input, which is not exercised by the
///     differential corpus (curated terminating programs, all backends share this
///     transformed bytecode) nor asserted anywhere — and native already relies on
///     `step_budget`, not `max_depth`, to bound every ordinary loop, so this does
///     not change native's safety posture.
pub(super) fn optimize_self_tail_calls(function: &mut RegFunction, function_id: usize) {
    // Locate every candidate self-tail-call site: (call_ip, dst, args, ...).
    let mut sites: Vec<(usize, Reg, Vec<Reg>)> = Vec::new();
    let code = &function.code;
    for call_ip in 0..code.len() {
        let RegInstr::CallKnown {
            dst,
            function: callee,
            args,
            mut_args,
        } = &code[call_ip]
        else {
            continue;
        };
        // Self only, no mut write-backs, matching arity.
        if *callee != function_id || !mut_args.is_empty() || args.len() != function.params {
            continue;
        }
        // Tail position: the very next instruction returns this call's result.
        let return_ip = call_ip + 1;
        let Some(RegInstr::Return { src }) = code.get(return_ip) else {
            continue;
        };
        if *src != *dst {
            continue;
        }
        // No other instruction may read the call's result (nothing observes it
        // between the call and the return). The `Return` at `return_ip` is the
        // only legitimate reader.
        let used_elsewhere = code
            .iter()
            .enumerate()
            .any(|(other_ip, instr)| other_ip != return_ip && instr_reads_register(instr, *dst));
        if used_elsewhere {
            continue;
        }
        sites.push((call_ip, *dst, args.clone()));
    }
    if sites.is_empty() {
        return;
    }
    // Recursion-depth-limit gate: bail entirely if the function has NO
    // **reachable** non-tail `Return` (no base case). A function whose every
    // reachable exit is a self-tail-call (e.g. `fn f(n){ return f(n: n + 1) }`)
    // only ever terminated by hitting the depth cap; converting it to a loop
    // would turn that clean `"recursion depth limit exceeded"` error into a hang,
    // so it must stay interpreted. Reachability matters: `lower` always appends a
    // defensive, usually-unreachable `LoadUnit; Return` tail, which must not be
    // mistaken for a real base case.
    let reachable = tco_reachable_instructions(&function.code);
    let tail_return_ips: Vec<usize> = sites.iter().map(|(call_ip, _, _)| call_ip + 1).collect();
    let has_base_case = function.code.iter().enumerate().any(|(ip, instr)| {
        reachable[ip] && matches!(instr, RegInstr::Return { .. }) && !tail_return_ips.contains(&ip)
    });
    if !has_base_case {
        return;
    }
    // Entry = first instruction past the prologue (leading `DeepCopy` run).
    let entry = function
        .code
        .iter()
        .position(|instr| {
            !matches!(
                instr,
                RegInstr::DeepCopy { .. } | RegInstr::DeepCopyElided { .. }
            )
        })
        .unwrap_or(0);

    // Apply every site. Appends rebind blocks at the tail; existing indices are
    // never disturbed, so jump targets stay valid.
    for (call_ip, _dst, args) in sites {
        // Stage each new arg into a fresh temp (simultaneous rebind), then move
        // the temps into the parameter registers, then jump to the body entry.
        let staging: Vec<Reg> = (0..args.len())
            .map(|_| {
                let reg = function.regs;
                function.regs += 1;
                reg
            })
            .collect();
        let block_start = function.code.len();
        function.code.push(RegInstr::TailCallGuard);
        for (slot, &arg) in args.iter().enumerate() {
            function.code.push(RegInstr::Move {
                dst: staging[slot],
                src: arg,
            });
        }
        for (param, &src) in staging.iter().enumerate() {
            function.code.push(RegInstr::Move { dst: param, src });
        }
        function.code.push(RegInstr::Jump { target: entry });
        // Redirect the original call site into the rebind block. The following
        // `Return` becomes unreachable dead code.
        function.code[call_ip] = RegInstr::Jump {
            target: block_start,
        };
    }
}

/// Whether `instr` reads register `reg` as a value operand. Used by the TCO pass
/// to confirm a self-tail-call's result is observed by nothing but its trailing
/// `Return`. Conservative by construction: any instruction variant whose operands
/// are not explicitly enumerated returns `true` (treated as a use), so the TCO
/// gate bails rather than risk missing a reader.
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
