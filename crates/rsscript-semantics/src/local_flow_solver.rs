//! Fixed-point ownership analysis for the neutral local-flow graph.

use crate::{Flow, LocalFlowState, LocalFlowStep, merge_non_fallthrough, path_root};
use rsscript_syntax::Span;
use std::collections::{HashMap, HashSet, VecDeque};

/// Compute the ownership state on entry to every reachable local-flow step.
pub fn local_flow_entry_states(
    steps: &[LocalFlowStep],
    initial_state: LocalFlowState,
) -> HashMap<Span, LocalFlowState> {
    if steps.is_empty() {
        return HashMap::new();
    }

    let mut entry_states = vec![None; steps.len()];
    entry_states[0] = Some(initial_state);
    let mut worklist = VecDeque::from([0]);

    while let Some(step_id) = worklist.pop_front() {
        let Some(entry_state) = entry_states[step_id].clone() else {
            continue;
        };
        let exit_state = transfer_local_flow_step(&steps[step_id], entry_state);
        for successor in &steps[step_id].successors {
            let mut successor_state = exit_state.clone();
            for resource in &successor.drop_resources {
                successor_state.drop_resource(resource);
            }
            if merge_local_flow_entry_state(&mut entry_states[successor.to], &successor_state) {
                worklist.push_back(successor.to);
            }
        }
    }

    steps
        .iter()
        .zip(entry_states)
        .filter_map(|(step, state)| state.map(|state| (step.span.clone(), state)))
        .collect()
}

/// Apply a single checked-HIR-derived flow step to an ownership state.
pub fn transfer_local_flow_step(step: &LocalFlowStep, mut state: LocalFlowState) -> LocalFlowState {
    if let Some(binding) = &step.binding {
        match binding.kind {
            crate::hir::HirBindingKind::ManagedLet => {
                state.bind_managed(binding.name.clone());
                if binding.fresh_from_fresh_value {
                    state.mark_fresh_returnable(binding.name.clone());
                }
            }
            crate::hir::HirBindingKind::LocalLet => {
                if binding.fresh_from_scrutinee {
                    state.bind_local(binding.name.clone());
                } else if let Some(source) = &binding.fresh_from_local_source {
                    if state.is_local(source) {
                        state.bind_local(binding.name.clone());
                    } else {
                        state.bind_managed(binding.name.clone());
                    }
                } else {
                    state.bind_local(binding.name.clone());
                }
            }
            crate::hir::HirBindingKind::Param => {}
        }
        if let Some(type_name) = &binding.type_name {
            state.record_type(binding.name.clone(), type_name.clone());
        }
    }
    if let Some(resource_binding) = &step.resource_binding {
        state.bind_resource(resource_binding.name.clone());
        if let Some(type_name) = &resource_binding.type_name {
            state.record_type(resource_binding.name.clone(), type_name.clone());
        }
    }

    for capture in &step.managed_closure_captures {
        if state.is_local(capture) || state.is_fresh_returnable_local(capture) {
            state.mark_retained(capture);
        }
    }
    state.apply_retention_events(&step.events);
    state.apply_move_events(&step.events);
    state
}

/// Merge one predecessor into a graph-entry state, returning whether it changed.
pub fn merge_local_flow_entry_state(
    target: &mut Option<LocalFlowState>,
    incoming: &LocalFlowState,
) -> bool {
    let Some(existing) = target else {
        *target = Some(incoming.clone());
        return true;
    };

    let merged = merge_local_flow_states(existing, incoming);
    if merged == *existing {
        false
    } else {
        *existing = merged;
        true
    }
}

/// Conservative lattice merge for two control-flow predecessors.
pub fn merge_local_flow_states(left: &LocalFlowState, right: &LocalFlowState) -> LocalFlowState {
    let locals = intersection(&left.locals, &right.locals);
    let field_splittable_locals = left
        .field_splittable_locals
        .intersection(&right.field_splittable_locals)
        .filter(|name| locals.contains(*name))
        .cloned()
        .collect();
    let managed = intersection(&left.managed, &right.managed);
    let read_views = intersection(&left.read_views, &right.read_views);
    let resources = intersection(&left.resources, &right.resources);
    let value_types = left
        .value_types
        .iter()
        .filter_map(|(name, left_type)| {
            right
                .value_types
                .get(name)
                .filter(|right_type| *right_type == left_type)
                .map(|_| (name.clone(), left_type.clone()))
        })
        .collect();

    let mut moved = left.moved.clone();
    for (name, span) in &right.moved {
        moved.entry(name.clone()).or_insert_with(|| span.clone());
    }
    moved.retain(|name, _| locals.contains(name));
    let mut moved_paths = left.moved_paths.clone();
    for (path, span) in &right.moved_paths {
        moved_paths
            .entry(path.clone())
            .or_insert_with(|| span.clone());
    }
    moved_paths.retain(|path, _| path_root(path).is_some_and(|root| locals.contains(root)));

    let clean_locals = left
        .clean_locals
        .intersection(&right.clean_locals)
        .filter(|name| locals.contains(*name) || managed.contains(*name))
        .cloned()
        .collect::<HashSet<_>>();
    let fresh_returnable_locals = left
        .fresh_returnable_locals
        .intersection(&right.fresh_returnable_locals)
        .filter(|name| clean_locals.contains(*name))
        .cloned()
        .collect::<HashSet<_>>();

    LocalFlowState {
        locals,
        field_splittable_locals,
        clean_locals,
        fresh_returnable_locals,
        managed,
        read_views,
        resources,
        moved,
        moved_paths,
        value_types,
    }
}

fn intersection(left: &HashSet<String>, right: &HashSet<String>) -> HashSet<String> {
    left.intersection(right).cloned().collect()
}

/// Merge the two branches of a source-shaped `if` check into its fallthrough
/// ownership state. This remains available while the legacy body walker is
/// replaced by the graph-only consumer.
pub fn merge_local_if_state(
    state: &mut LocalFlowState,
    base: &LocalFlowState,
    then_state: LocalFlowState,
    then_flow: Flow,
    else_branch: Option<(LocalFlowState, Flow)>,
) -> Flow {
    let (else_state, else_flow) = else_branch.unwrap_or_else(|| (base.clone(), Flow::Fallthrough));
    let mut fallthrough_states = Vec::new();
    if then_flow == Flow::Fallthrough {
        fallthrough_states.push(then_state);
    }
    if else_flow == Flow::Fallthrough {
        fallthrough_states.push(else_state);
    }
    match fallthrough_states.as_slice() {
        [] => {
            *state = base.clone();
            merge_non_fallthrough(then_flow, else_flow)
        }
        [only] => {
            *state = local_fallthrough_projection(base, only);
            Flow::Fallthrough
        }
        [left, right] => {
            *state = merge_local_fallthrough_states(base, left, right);
            Flow::Fallthrough
        }
        _ => unreachable!("if has at most two branches"),
    }
}

/// Merge a source-shaped loop body into its post-loop ownership state.
pub fn merge_local_loop_state(
    state: &mut LocalFlowState,
    base: &LocalFlowState,
    body_state: LocalFlowState,
    body_flow: Flow,
    may_skip: bool,
) -> Flow {
    if !may_skip && body_flow == Flow::Return {
        *state = base.clone();
        return Flow::Return;
    }
    if !may_skip && body_flow == Flow::Break {
        *state = local_fallthrough_projection(base, &body_state);
        return Flow::Fallthrough;
    }
    let mut moved = base.moved.clone();
    let mut moved_paths = base.moved_paths.clone();
    if body_flow != Flow::Return {
        for (name, span) in &body_state.moved {
            if base.locals.contains(name) || base.moved.contains_key(name) {
                moved.entry(name.clone()).or_insert_with(|| span.clone());
            }
        }
        merge_moved_paths_from_branch(&mut moved_paths, base, &body_state);
    }
    state.locals = base.locals.clone();
    state.field_splittable_locals = base.field_splittable_locals.clone();
    state.managed = base.managed.clone();
    state.read_views = base.read_views.clone();
    state.resources = base.resources.clone();
    state.value_types = base.value_types.clone();
    state.moved = moved;
    state.moved_paths = moved_paths;
    state.clean_locals = base
        .clean_locals
        .intersection(&body_state.clean_locals)
        .filter(|name| base.locals.contains(*name) || base.managed.contains(*name))
        .cloned()
        .collect();
    state.fresh_returnable_locals = base
        .fresh_returnable_locals
        .intersection(&body_state.fresh_returnable_locals)
        .filter(|name| state.clean_locals.contains(*name))
        .cloned()
        .collect();
    Flow::Fallthrough
}

fn local_fallthrough_projection(base: &LocalFlowState, branch: &LocalFlowState) -> LocalFlowState {
    let mut moved = base.moved.clone();
    let mut moved_paths = base.moved_paths.clone();
    for (name, span) in &branch.moved {
        if base.locals.contains(name) || base.moved.contains_key(name) {
            moved.entry(name.clone()).or_insert_with(|| span.clone());
        }
    }
    merge_moved_paths_from_branch(&mut moved_paths, base, branch);
    let clean_locals = branch
        .clean_locals
        .intersection(&base.clean_locals)
        .filter(|name| base.locals.contains(*name) || base.managed.contains(*name))
        .cloned()
        .collect::<HashSet<_>>();
    let fresh_returnable_locals = branch
        .fresh_returnable_locals
        .intersection(&base.fresh_returnable_locals)
        .filter(|name| clean_locals.contains(*name))
        .cloned()
        .collect::<HashSet<_>>();
    LocalFlowState {
        locals: base.locals.clone(),
        field_splittable_locals: base.field_splittable_locals.clone(),
        managed: base.managed.clone(),
        read_views: base.read_views.clone(),
        resources: base.resources.clone(),
        value_types: base.value_types.clone(),
        moved,
        moved_paths,
        clean_locals,
        fresh_returnable_locals,
    }
}

fn merge_local_fallthrough_states(
    base: &LocalFlowState,
    left: &LocalFlowState,
    right: &LocalFlowState,
) -> LocalFlowState {
    let mut moved = base.moved.clone();
    let mut moved_paths = base.moved_paths.clone();
    for branch in [left, right] {
        for (name, span) in &branch.moved {
            if base.locals.contains(name) || base.moved.contains_key(name) {
                moved.entry(name.clone()).or_insert_with(|| span.clone());
            }
        }
        merge_moved_paths_from_branch(&mut moved_paths, base, branch);
    }
    let clean_locals = left
        .clean_locals
        .intersection(&right.clean_locals)
        .filter(|name| base.locals.contains(*name) || base.managed.contains(*name))
        .cloned()
        .collect::<HashSet<_>>();
    let fresh_returnable_locals = left
        .fresh_returnable_locals
        .intersection(&right.fresh_returnable_locals)
        .filter(|name| clean_locals.contains(*name))
        .cloned()
        .collect::<HashSet<_>>();
    LocalFlowState {
        locals: base.locals.clone(),
        field_splittable_locals: base.field_splittable_locals.clone(),
        managed: base.managed.clone(),
        read_views: base.read_views.clone(),
        resources: base.resources.clone(),
        value_types: base.value_types.clone(),
        moved,
        moved_paths,
        clean_locals,
        fresh_returnable_locals,
    }
}

fn merge_moved_paths_from_branch(
    moved_paths: &mut HashMap<String, Span>,
    base: &LocalFlowState,
    branch: &LocalFlowState,
) {
    for (path, span) in &branch.moved_paths {
        if path_root(path).is_some_and(|root| base.locals.contains(root))
            || base.moved_paths.contains_key(path)
        {
            moved_paths
                .entry(path.clone())
                .or_insert_with(|| span.clone());
        }
    }
}
