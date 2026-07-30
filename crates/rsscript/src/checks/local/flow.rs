//! Local control-flow graph construction and ownership-state transfer.

use super::*;

pub(super) fn collect_local_flow_steps(block: &HirBlock) -> Vec<LocalFlowStep> {
    let mut steps = Vec::new();
    collect_block_local_flow(block, &mut steps);
    steps
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LocalFlowFragment {
    entry: Option<usize>,
    exits: Vec<LocalFlowExit>,
    breaks: Vec<LocalFlowExit>,
    continues: Vec<LocalFlowExit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct LocalFlowExit {
    node: usize,
    drop_resources: Vec<String>,
}

impl LocalFlowExit {
    fn new(node: usize) -> Self {
        Self {
            node,
            drop_resources: Vec::new(),
        }
    }

    fn with_drop(mut self, resource: &str) -> Self {
        if !self
            .drop_resources
            .iter()
            .any(|existing| existing == resource)
        {
            self.drop_resources.push(resource.to_string());
        }
        self
    }
}

pub(super) fn collect_block_local_flow(
    block: &HirBlock,
    steps: &mut Vec<LocalFlowStep>,
) -> LocalFlowFragment {
    let mut entry = None;
    let mut pending_exits = Vec::new();
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    let mut reachable = true;

    for statement in &block.statements {
        let fragment = collect_stmt_local_flow(statement, steps);
        if entry.is_none() {
            entry = fragment.entry;
        }
        if !reachable {
            continue;
        }
        if let Some(fragment_entry) = fragment.entry {
            for exit in pending_exits.drain(..) {
                add_successor(steps, exit, fragment_entry);
            }
        }
        pending_exits = fragment.exits;
        breaks.extend(fragment.breaks);
        continues.extend(fragment.continues);
        reachable = !pending_exits.is_empty();
    }

    LocalFlowFragment {
        entry,
        exits: pending_exits,
        breaks,
        continues,
    }
}

pub(super) fn collect_stmt_local_flow(
    statement: &HirStmt,
    steps: &mut Vec<LocalFlowStep>,
) -> LocalFlowFragment {
    let node = push_local_flow_step(steps, statement);
    match statement {
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => collect_if_local_flow(steps, node, then_body, else_body.as_ref()),
        HirStmt::Loop {
            condition, body, ..
        } => collect_loop_local_flow(steps, node, condition.is_some(), body),
        HirStmt::For { body, .. } => collect_loop_local_flow(steps, node, true, body),
        HirStmt::Match { value, arms, .. } => collect_match_local_flow(steps, node, value, arms),
        HirStmt::Select { arms, .. } => collect_select_local_flow(steps, node, arms),
        HirStmt::With { binding, body, .. } => collect_scoped_body_flow(steps, node, binding, body),
        HirStmt::Return { .. } => LocalFlowFragment {
            entry: Some(node),
            exits: Vec::new(),
            breaks: Vec::new(),
            continues: Vec::new(),
        },
        HirStmt::Break(_) => LocalFlowFragment {
            entry: Some(node),
            exits: Vec::new(),
            breaks: vec![LocalFlowExit::new(node)],
            continues: Vec::new(),
        },
        HirStmt::Continue(_) => LocalFlowFragment {
            entry: Some(node),
            exits: Vec::new(),
            breaks: Vec::new(),
            continues: vec![LocalFlowExit::new(node)],
        },
        HirStmt::Let { .. } | HirStmt::Expr(_) | HirStmt::Assign { .. } | HirStmt::Unknown(_) => {
            LocalFlowFragment {
                entry: Some(node),
                exits: vec![LocalFlowExit::new(node)],
                breaks: Vec::new(),
                continues: Vec::new(),
            }
        }
    }
}

pub(super) fn collect_match_local_flow(
    steps: &mut Vec<LocalFlowStep>,
    match_node: usize,
    value: &HirExpr,
    arms: &[crate::hir::HirMatchArm],
) -> LocalFlowFragment {
    let mut exits = Vec::new();
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    if arms.is_empty() {
        exits.push(LocalFlowExit::new(match_node));
    }
    for arm in arms {
        let arm_flow = collect_block_local_flow(&arm.body, steps);
        let arm_entry = if let Some(binding) = fresh_match_pattern_binding(value, arm) {
            let binding_node = push_pattern_binding_flow_step(steps, &arm.span, binding);
            if let Some(body_entry) = arm_flow.entry {
                add_successor(steps, LocalFlowExit::new(binding_node), body_entry);
            }
            Some(binding_node)
        } else {
            arm_flow.entry
        };
        if let Some(arm_entry) = arm_entry {
            add_successor(steps, LocalFlowExit::new(match_node), arm_entry);
            exits.extend(arm_flow.exits);
            breaks.extend(arm_flow.breaks);
            continues.extend(arm_flow.continues);
        } else {
            exits.push(LocalFlowExit::new(match_node));
        }
    }
    LocalFlowFragment {
        entry: Some(match_node),
        exits,
        breaks,
        continues,
    }
}

pub(super) fn collect_select_local_flow(
    steps: &mut Vec<LocalFlowStep>,
    select_node: usize,
    arms: &[crate::hir::HirSelectArm],
) -> LocalFlowFragment {
    let mut exits = Vec::new();
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    if arms.is_empty() {
        exits.push(LocalFlowExit::new(select_node));
    }
    for arm in arms {
        let arm_flow = collect_block_local_flow(&arm.body, steps);
        let arm_entry = if arm.binding != "_" {
            let binding = LocalFlowBinding {
                name: arm.binding.clone(),
                kind: HirBindingKind::ManagedLet,
                type_name: hir_expr_type_name(&arm.operation).map(str::to_string),
                value_ident: None,
                value_handle_field: None,
                fresh_from_local_source: None,
                fresh_from_scrutinee: false,
                fresh_from_fresh_value: false,
            };
            let binding_node = push_pattern_binding_flow_step(steps, &arm.span, binding);
            if let Some(body_entry) = arm_flow.entry {
                add_successor(steps, LocalFlowExit::new(binding_node), body_entry);
            }
            Some(binding_node)
        } else {
            arm_flow.entry
        };
        if let Some(arm_entry) = arm_entry {
            add_successor(steps, LocalFlowExit::new(select_node), arm_entry);
            exits.extend(arm_flow.exits);
            breaks.extend(arm_flow.breaks);
            continues.extend(arm_flow.continues);
        } else {
            exits.push(LocalFlowExit::new(select_node));
        }
    }
    LocalFlowFragment {
        entry: Some(select_node),
        exits,
        breaks,
        continues,
    }
}

pub(super) fn push_pattern_binding_flow_step(
    steps: &mut Vec<LocalFlowStep>,
    span: &Span,
    binding: LocalFlowBinding,
) -> usize {
    let id = steps.len();
    steps.push(LocalFlowStep {
        id,
        span: span.clone(),
        kind: LocalFlowStepKind::Statement,
        uses: Vec::new(),
        managed_closure_captures: Vec::new(),
        binding: Some(binding),
        resource_binding: None,
        events: Vec::new(),
        successors: Vec::new(),
    });
    id
}

pub(super) fn fresh_match_pattern_binding(
    value: &HirExpr,
    arm: &crate::hir::HirMatchArm,
) -> Option<LocalFlowBinding> {
    let value_type = hir_expr_type_name(value)?;
    let source = hir_expr_ident_name(value);
    let fresh_from_scrutinee = is_fresh_match_scrutinee(value);
    if source.is_none() && !fresh_from_scrutinee {
        return None;
    }
    let crate::syntax::ast::MatchPattern::Variant { name, bindings, .. } = &arm.pattern else {
        return None;
    };
    // Fresh-payload tracking applies only to the single-payload sugar.
    let [binding] = bindings.as_slice() else {
        return None;
    };
    let crate::syntax::ast::MatchPattern::Binding { name: binding, .. } = binding else {
        return None;
    };
    let payload_type = fresh_payload_type_for_variant(value_type, name)?;
    Some(LocalFlowBinding {
        name: binding.clone(),
        kind: HirBindingKind::LocalLet,
        type_name: Some(strip_fresh_type(payload_type).to_string()),
        value_ident: None,
        value_handle_field: None,
        fresh_from_local_source: source.map(str::to_string),
        fresh_from_scrutinee,
        fresh_from_fresh_value: false,
    })
}

pub(super) fn fresh_payload_type_for_variant<'a>(
    value_type: &'a str,
    variant: &str,
) -> Option<&'a str> {
    let inner = value_type
        .trim()
        .strip_prefix("Option<")
        .and_then(|rest| rest.strip_suffix('>'));
    if variant == "Some" {
        let payload = inner?.trim();
        return payload.strip_prefix("fresh ").map(str::trim);
    }

    let inner = value_type
        .trim()
        .strip_prefix("Result<")
        .and_then(|rest| rest.strip_suffix('>'))?;
    let args = split_top_level_type_args(inner);
    let payload = match variant {
        "Ok" => args.first().copied()?,
        _ => return None,
    }
    .trim();
    payload.strip_prefix("fresh ").map(str::trim)
}

pub(super) fn is_fresh_match_scrutinee(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Call { resolution, .. } => {
            matches!(resolution, CallResolution::Resolved { signature, .. } if signature.returns_fresh)
        }
        HirExpr::Try { value, .. } => is_fresh_match_scrutinee(value),
        _ => false,
    }
}

pub(super) fn hir_expr_ident_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name.as_str()),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => hir_expr_ident_name(value),
        _ => None,
    }
}

pub(super) fn collect_if_local_flow(
    steps: &mut Vec<LocalFlowStep>,
    branch_node: usize,
    then_body: &HirBlock,
    else_body: Option<&HirBlock>,
) -> LocalFlowFragment {
    let then_flow = collect_block_local_flow(then_body, steps);
    if let Some(then_entry) = then_flow.entry {
        add_successor(steps, LocalFlowExit::new(branch_node), then_entry);
    }

    let mut exits = then_flow.exits;
    let mut breaks = then_flow.breaks;
    let mut continues = then_flow.continues;
    if let Some(else_body) = else_body {
        let else_flow = collect_block_local_flow(else_body, steps);
        if let Some(else_entry) = else_flow.entry {
            add_successor(steps, LocalFlowExit::new(branch_node), else_entry);
        }
        exits.extend(else_flow.exits);
        breaks.extend(else_flow.breaks);
        continues.extend(else_flow.continues);
    } else {
        exits.push(LocalFlowExit::new(branch_node));
    }

    LocalFlowFragment {
        entry: Some(branch_node),
        exits,
        breaks,
        continues,
    }
}

pub(super) fn collect_loop_local_flow(
    steps: &mut Vec<LocalFlowStep>,
    loop_node: usize,
    may_skip: bool,
    body: &HirBlock,
) -> LocalFlowFragment {
    let body_flow = collect_block_local_flow(body, steps);
    if let Some(body_entry) = body_flow.entry {
        add_successor(steps, LocalFlowExit::new(loop_node), body_entry);
    }
    for exit in body_flow.exits.iter().chain(body_flow.continues.iter()) {
        add_successor(steps, exit.clone(), loop_node);
    }

    let mut exits = if may_skip {
        vec![LocalFlowExit::new(loop_node)]
    } else {
        Vec::new()
    };
    exits.extend(body_flow.breaks);

    LocalFlowFragment {
        entry: Some(loop_node),
        exits,
        breaks: Vec::new(),
        continues: Vec::new(),
    }
}

pub(super) fn collect_scoped_body_flow(
    steps: &mut Vec<LocalFlowStep>,
    scoped_node: usize,
    binding: &str,
    body: &HirBlock,
) -> LocalFlowFragment {
    let body_flow = collect_block_local_flow(body, steps);
    if let Some(body_entry) = body_flow.entry {
        add_successor(steps, LocalFlowExit::new(scoped_node), body_entry);
    }
    let empty_body_exit = LocalFlowExit::new(scoped_node).with_drop(binding);
    LocalFlowFragment {
        entry: Some(scoped_node),
        exits: if body_flow.entry.is_some() {
            drop_resource_on_exits(body_flow.exits, binding)
        } else {
            vec![empty_body_exit]
        },
        breaks: drop_resource_on_exits(body_flow.breaks, binding),
        continues: drop_resource_on_exits(body_flow.continues, binding),
    }
}

pub(super) fn drop_resource_on_exits(
    exits: Vec<LocalFlowExit>,
    resource: &str,
) -> Vec<LocalFlowExit> {
    exits
        .into_iter()
        .map(|exit| exit.with_drop(resource))
        .collect()
}

pub(super) fn push_local_flow_step(steps: &mut Vec<LocalFlowStep>, statement: &HirStmt) -> usize {
    let mut uses = Vec::new();
    collect_hir_stmt_idents(statement, &mut uses);
    let mut events = Vec::new();
    collect_hir_stmt_effect_events(statement, &mut events);
    let id = steps.len();
    steps.push(LocalFlowStep {
        id,
        span: hir_stmt_span(statement).clone(),
        kind: local_flow_step_kind(statement),
        uses,
        managed_closure_captures: local_flow_step_managed_closure_captures(statement),
        binding: local_flow_step_binding(statement),
        resource_binding: local_flow_step_resource_binding(statement),
        events,
        successors: Vec::new(),
    });
    id
}

pub(super) fn local_flow_step_managed_closure_captures(statement: &HirStmt) -> Vec<String> {
    let mut captures = Vec::new();
    collect_stmt_managed_closure_capture_names(statement, &mut captures);
    captures
}

pub(super) fn collect_stmt_managed_closure_capture_names(
    statement: &HirStmt,
    captures: &mut Vec<String>,
) {
    match statement {
        HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            value: Some(HirExpr::Closure { body, .. }),
            ..
        } => push_hir_block_inline_capture_names(body, captures),
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => {
            collect_expr_managed_closure_capture_names(value, captures)
        }
        HirStmt::With { resource, .. } => {
            collect_expr_managed_closure_capture_names(resource, captures);
        }
        HirStmt::If { condition, .. } => {
            collect_expr_managed_closure_capture_names(condition, captures);
        }
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => {
            collect_expr_managed_closure_capture_names(condition, captures);
        }
        HirStmt::For { iterable, .. } => {
            collect_expr_managed_closure_capture_names(iterable, captures);
        }
        HirStmt::Match { value, .. } => {
            collect_expr_managed_closure_capture_names(value, captures);
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_managed_closure_capture_names(&arm.operation, captures);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Loop {
            condition: None, ..
        }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_expr_managed_closure_capture_names(
    expr: &HirExpr,
    captures: &mut Vec<String>,
) {
    match expr {
        HirExpr::Call {
            args, resolution, ..
        } => {
            if let CallResolution::Resolved { signature, .. } = resolution {
                for arg in args {
                    let Some(name) = arg.name.as_ref() else {
                        continue;
                    };
                    if !signature.retained_params.contains(name) {
                        continue;
                    }
                    if let Some((body, _)) = retained_closure_arg(&arg.value) {
                        push_hir_block_inline_capture_names(body, captures);
                    }
                }
            }
            for arg in args {
                collect_expr_managed_closure_capture_names(&arg.value, captures);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_expr_managed_closure_capture_names(value, captures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expr_managed_closure_capture_names(left, captures);
            collect_expr_managed_closure_capture_names(right, captures);
        }
        HirExpr::Field { base, .. } => collect_expr_managed_closure_capture_names(base, captures),
        HirExpr::Index { base, index, .. } => {
            collect_expr_managed_closure_capture_names(base, captures);
            collect_expr_managed_closure_capture_names(index, captures);
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_managed_closure_capture_names(&entry.key, captures);
                collect_expr_managed_closure_capture_names(&entry.value, captures);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_managed_closure_capture_names(&field.value, captures);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_managed_closure_capture_names(item, captures);
            }
        }
        HirExpr::Closure { .. }
        | HirExpr::Match { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn push_hir_block_inline_capture_names(block: &HirBlock, captures: &mut Vec<String>) {
    let mut uses = Vec::new();
    collect_hir_block_inline_capture_uses(block, &mut uses);
    for (name, _) in uses {
        if !captures.contains(&name) {
            captures.push(name);
        }
    }
}

pub(super) fn add_successor(steps: &mut [LocalFlowStep], from: LocalFlowExit, to: usize) {
    let from_node = from.node;
    let edge = LocalFlowEdge {
        to,
        drop_resources: from.drop_resources,
    };
    if !steps[from_node].successors.contains(&edge) {
        steps[from_node].successors.push(edge);
    }
}

pub(super) fn collect_flow_entry_states(
    steps: &[LocalFlowStep],
    initial_state: BodyState,
) -> HashMap<Span, BodyState> {
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
        let exit_state = transfer_flow_step(&steps[step_id], entry_state);
        for successor in &steps[step_id].successors {
            let mut successor_state = exit_state.clone();
            for resource in &successor.drop_resources {
                successor_state.drop_resource(resource);
            }
            let changed = merge_flow_entry_state(&mut entry_states[successor.to], &successor_state);
            if changed {
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

pub(super) fn transfer_flow_step(step: &LocalFlowStep, mut state: BodyState) -> BodyState {
    if let Some(binding) = &step.binding {
        match binding.kind {
            HirBindingKind::ManagedLet => {
                state.bind_managed(binding.name.clone());
                // A managed binding initialized from a fresh value is returnable
                // as fresh until aliased (move/retain/capture clear this).
                if binding.fresh_from_fresh_value {
                    state.mark_fresh_returnable(binding.name.clone());
                }
            }
            HirBindingKind::LocalLet => {
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
            HirBindingKind::Param => {}
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
        // Capturing a local, or a managed binding that is currently returnable as
        // fresh, into a managed closure aliases it — clear its fresh/clean status.
        if state.is_local(capture) || state.is_fresh_returnable_local(capture) {
            state.mark_retained(capture);
        }
    }
    state.apply_retention_events(&step.events);
    state.apply_move_events(&step.events);
    state
}

pub(super) fn merge_flow_entry_state(target: &mut Option<BodyState>, incoming: &BodyState) -> bool {
    let Some(existing) = target else {
        *target = Some(incoming.clone());
        return true;
    };

    let merged = merge_flow_states(existing, incoming);
    if merged == *existing {
        false
    } else {
        *existing = merged;
        true
    }
}

pub(super) fn merge_flow_states(left: &BodyState, right: &BodyState) -> BodyState {
    let locals = left
        .locals
        .intersection(&right.locals)
        .cloned()
        .collect::<HashSet<_>>();
    let field_splittable_locals = left
        .field_splittable_locals
        .intersection(&right.field_splittable_locals)
        .filter(|name| locals.contains(*name))
        .cloned()
        .collect::<HashSet<_>>();
    let managed = left
        .managed
        .intersection(&right.managed)
        .cloned()
        .collect::<HashSet<_>>();
    let read_views = left
        .read_views
        .intersection(&right.read_views)
        .cloned()
        .collect::<HashSet<_>>();
    let resources = left
        .resources
        .intersection(&right.resources)
        .cloned()
        .collect::<HashSet<_>>();
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
        .collect::<HashMap<_, _>>();

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

    // A fresh binding survives the merge when it is clean in both predecessors
    // and still tracked as either an exclusive `local` or a managed `let`/`let
    // mut` binding. Keeping managed bindings (not just exclusive locals) lets a
    // fresh builder pattern that runs inside a loop/branch still return cleanly.
    // This stays sound: any aliasing invalidation (manage/retain/take/capture)
    // already removes the binding from the predecessor `clean_locals`
    // intersection, so an aliased binding can never reach this filter.
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

    BodyState {
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

pub(super) fn local_flow_step_kind(statement: &HirStmt) -> LocalFlowStepKind {
    match statement {
        HirStmt::If { .. } => LocalFlowStepKind::Branch,
        HirStmt::Match { .. } => LocalFlowStepKind::Branch,
        HirStmt::Select { .. } => LocalFlowStepKind::Branch,
        HirStmt::Loop { .. } => LocalFlowStepKind::Loop,
        HirStmt::For { .. } => LocalFlowStepKind::Loop,
        HirStmt::Return { .. } => LocalFlowStepKind::Return,
        HirStmt::Break(_) => LocalFlowStepKind::Break,
        HirStmt::Continue(_) => LocalFlowStepKind::Continue,
        HirStmt::Let { .. }
        | HirStmt::With { .. }
        | HirStmt::Expr(_)
        | HirStmt::Assign { .. }
        | HirStmt::Unknown(_) => LocalFlowStepKind::Statement,
    }
}

pub(super) fn local_flow_step_binding(statement: &HirStmt) -> Option<LocalFlowBinding> {
    match statement {
        HirStmt::Let {
            kind,
            name,
            value,
            type_name,
            ..
        } => Some(LocalFlowBinding {
            name: name.clone(),
            kind: *kind,
            type_name: type_name.clone(),
            value_ident: value.as_ref().and_then(local_binding_source_ident),
            value_handle_field: value.as_ref().and_then(local_binding_handle_field_source),
            fresh_from_local_source: None,
            fresh_from_scrutinee: false,
            fresh_from_fresh_value: value.as_ref().is_some_and(hir_expr_is_fresh_value),
        }),
        HirStmt::Return { .. }
        | HirStmt::With { .. }
        | HirStmt::If { .. }
        | HirStmt::Select { .. }
        | HirStmt::Match { .. }
        | HirStmt::Loop { .. }
        | HirStmt::For { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Expr(_)
        | HirStmt::Assign { .. }
        | HirStmt::Unknown(_) => None,
    }
}

/// True if `value` is itself a fresh, unaliased value: a literal, a collection
/// literal, a struct/variant constructor, or a fresh-returning call (seen through
/// `?` and effect wrappers). A managed `let` bound to such a value can be returned
/// directly as `fresh` until it is moved, retained, or captured — exactly the
/// invalidations the flow analysis already applies to clean locals.
pub(super) fn hir_expr_is_fresh_value(value: &HirExpr) -> bool {
    match value {
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ArrayLiteral { .. } => true,
        HirExpr::Call { resolution, .. } => match resolution {
            CallResolution::EnumVariant => true,
            CallResolution::Resolved {
                kind:
                    ResolvedCalleeKind::Constructor {
                        type_kind: HirTypeKind::Struct,
                    },
                ..
            } => true,
            CallResolution::Resolved { signature, .. } => signature.returns_fresh,
            _ => false,
        },
        HirExpr::Try { value, .. } | HirExpr::Effect { value, .. } => {
            hir_expr_is_fresh_value(value)
        }
        _ => false,
    }
}

pub(super) fn local_binding_source_ident(value: &HirExpr) -> Option<(String, Span)> {
    match value {
        HirExpr::Ident { name, span, .. } => Some((name.clone(), span.clone())),
        HirExpr::Effect {
            effect: ParamEffect::Read | ParamEffect::Mut,
            value,
            ..
        } => local_binding_source_ident(value),
        HirExpr::Call { callee, args, .. } if local_binding_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| local_binding_source_ident(&arg.value)),
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Effect { .. }
        | HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Try { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Match { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn local_binding_handle_field_source(value: &HirExpr) -> Option<(String, Span)> {
    match value {
        HirExpr::Field { base, access, .. } if access.is_handle => {
            hir_expr_path(base).map(|(mut path, _)| {
                path.push('.');
                path.push_str(&access.name);
                (path, access.span.clone())
            })
        }
        HirExpr::Field { base, .. } => local_binding_handle_field_source(base),
        HirExpr::Effect {
            effect: ParamEffect::Read | ParamEffect::Mut,
            value,
            ..
        } => local_binding_handle_field_source(value),
        HirExpr::Call { callee, args, .. } if local_binding_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| local_binding_handle_field_source(&arg.value)),
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Effect { .. }
        | HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Try { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Match { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn local_binding_wrapper_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Name(name) if matches!(name.as_str(), "Ok" | "Err" | "Some")
    )
}

pub(super) fn local_flow_step_resource_binding(
    statement: &HirStmt,
) -> Option<LocalFlowResourceBinding> {
    match statement {
        HirStmt::With {
            binding, resource, ..
        } => Some(LocalFlowResourceBinding {
            name: binding.clone(),
            type_name: hir_expr_type_name(resource).map(str::to_string),
        }),
        HirStmt::Let { .. }
        | HirStmt::Return { .. }
        | HirStmt::If { .. }
        | HirStmt::Select { .. }
        | HirStmt::Match { .. }
        | HirStmt::Loop { .. }
        | HirStmt::For { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Expr(_)
        | HirStmt::Assign { .. }
        | HirStmt::Unknown(_) => None,
    }
}

pub(super) fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { type_name, .. }
        | HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn hir_stmt_span(statement: &HirStmt) -> &Span {
    match statement {
        HirStmt::Let { span, .. }
        | HirStmt::Return { span, .. }
        | HirStmt::With { span, .. }
        | HirStmt::If { span, .. }
        | HirStmt::Select { span, .. }
        | HirStmt::Match { span, .. }
        | HirStmt::Loop { span, .. }
        | HirStmt::For { span, .. }
        | HirStmt::Break(span)
        | HirStmt::Continue(span)
        | HirStmt::Unknown(span) => span,
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => hir_expr_span(expr),
    }
}

impl BodyState {
    pub(crate) fn seed_params(&mut self, bindings: &[HirBinding]) {
        for binding in bindings {
            if binding.kind != HirBindingKind::Param {
                continue;
            }
            if let Some(type_name) = &binding.type_name {
                self.record_type(binding.name.clone(), type_name.clone());
            }
            if matches!(binding.effect, Some(ParamEffect::Read))
                && binding
                    .type_name
                    .as_deref()
                    .is_none_or(|type_name| !is_copy_type_name(type_name))
            {
                self.bind_managed(binding.name.clone());
            }
            if matches!(binding.effect, Some(ParamEffect::Mut | ParamEffect::Take)) {
                if binding.effect == Some(ParamEffect::Take) {
                    self.bind_param_local(binding.name.clone(), true);
                } else {
                    self.bind_param_local(binding.name.clone(), false);
                }
            }
        }
    }

    /// Mark a (managed) binding as returnable-as-fresh: it currently holds a
    /// fresh, unaliased value. Cleared by `mark_moved`/`mark_retained` when the
    /// binding is aliased.
    pub(crate) fn mark_fresh_returnable(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.clean_locals.insert(name.clone());
        self.fresh_returnable_locals.insert(name);
    }

    pub(crate) fn bind_managed(&mut self, name: impl Into<String>) {
        self.managed.insert(name.into());
    }

    pub(crate) fn bind_read_view(&mut self, name: impl Into<String>) {
        self.read_views.insert(name.into());
    }

    pub(crate) fn bind_resource(&mut self, name: impl Into<String>) {
        self.resources.insert(name.into());
    }

    pub(crate) fn drop_resource(&mut self, name: &str) {
        self.resources.remove(name);
    }

    pub(crate) fn bind_local(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.locals.insert(name.clone());
        self.field_splittable_locals.insert(name.clone());
        self.clean_locals.insert(name.clone());
        self.fresh_returnable_locals.insert(name);
    }

    fn bind_param_local(&mut self, name: impl Into<String>, field_splittable: bool) {
        let name = name.into();
        self.locals.insert(name.clone());
        if field_splittable {
            self.field_splittable_locals.insert(name.clone());
        }
        self.clean_locals.insert(name);
    }

    pub(crate) fn record_type(&mut self, name: impl Into<String>, type_name: impl Into<String>) {
        self.value_types.insert(name.into(), type_name.into());
    }

    pub(crate) fn mark_moved(&mut self, name: &str, span: Span) {
        if name.contains('.') {
            self.moved_paths.insert(name.to_string(), span);
            if let Some(root) = path_root(name) {
                self.clean_locals.remove(root);
                self.fresh_returnable_locals.remove(root);
            }
        } else {
            self.moved.insert(name.to_string(), span);
            self.clean_locals.remove(name);
            self.fresh_returnable_locals.remove(name);
        }
    }

    pub(crate) fn mark_retained(&mut self, name: &str) {
        self.clean_locals.remove(name);
        self.fresh_returnable_locals.remove(name);
    }

    pub(crate) fn is_local(&self, name: &str) -> bool {
        self.locals.contains(name)
    }

    pub(crate) fn allows_field_split(&self, name: &str) -> bool {
        self.field_splittable_locals.contains(name)
    }

    pub(crate) fn is_managed(&self, name: &str) -> bool {
        self.managed.contains(name)
    }

    pub(crate) fn is_read_view(&self, name: &str) -> bool {
        self.read_views.contains(name)
    }

    pub(crate) fn is_resource(&self, name: &str) -> bool {
        self.resources.contains(name)
    }

    pub(crate) fn is_clean_local(&self, name: &str) -> bool {
        self.clean_locals.contains(name)
    }

    pub(crate) fn is_fresh_returnable_local(&self, name: &str) -> bool {
        self.fresh_returnable_locals.contains(name)
    }

    pub(crate) fn move_span(&self, name: &str) -> Option<&Span> {
        self.moved.get(name)
    }

    pub(crate) fn moved_path_span(&self, path: &str) -> Option<(String, &Span)> {
        self.moved_paths
            .iter()
            .find(|(moved_path, _)| {
                path == moved_path.as_str()
                    || path
                        .strip_prefix(moved_path.as_str())
                        .is_some_and(|suffix| suffix.starts_with('.'))
            })
            .map(|(path, span)| (path.clone(), span))
    }

    pub(crate) fn moved_subpath_span(&self, root: &str) -> Option<(String, &Span)> {
        self.moved_paths
            .iter()
            .find(|(path, _)| path_root(path).is_some_and(|path_root| path_root == root))
            .map(|(path, span)| (path.clone(), span))
    }

    pub(crate) fn value_type(&self, name: &str) -> Option<&str> {
        self.value_types.get(name).map(String::as_str)
    }

    pub(crate) fn apply_move_events(&mut self, events: &[HirEffectEvent]) {
        for event in events {
            if !matches!(
                event.kind,
                HirEffectEventKind::Manage | HirEffectEventKind::Take
            ) {
                continue;
            }
            if event.binding_name.contains('.') {
                if path_root(&event.binding_name).is_some_and(|root| self.locals.contains(root)) {
                    self.mark_moved(&event.binding_name, event.span.clone());
                }
            } else if self.locals.contains(&event.binding_name)
                || self.fresh_returnable_locals.contains(&event.binding_name)
            {
                self.mark_moved(&event.binding_name, event.span.clone());
            }
        }
    }

    pub(crate) fn apply_retention_events(&mut self, events: &[HirEffectEvent]) {
        for event in events {
            if !matches!(event.kind, HirEffectEventKind::Retain { .. }) {
                continue;
            }
            // Retaining a local, or a managed binding that is currently returnable
            // as fresh, aliases it into retained state — clear its fresh status so
            // it can no longer be returned as `fresh`.
            if self.locals.contains(&event.binding_name)
                || self.fresh_returnable_locals.contains(&event.binding_name)
            {
                self.mark_retained(&event.binding_name);
            }
        }
    }
}

/// Whether a value of `type_name` may be sent as a cross-isolate **message**
/// (spec §20.2-3): a self-contained value carrying no managed (`Rc`) handle, so it
/// can cross an isolate boundary without sharing mutable state. v1 allows Copy
/// scalars plus the immutable owned-data types `String` and `Bytes` (value
/// semantics; safe to transfer/share). Mutable/managed containers (`List`, `Map`,
/// `Buffer`), structs/sums, handles, closures, and generics are conservatively
/// rejected for now — broadening to data-only structs/containers is a follow-up.
pub(crate) fn is_cross_isolate_transferable(type_name: &str) -> bool {
    let type_name = type_name.trim();
    if is_copy_type_name(type_name) {
        return true;
    }
    matches!(
        type_name.strip_prefix("fresh ").unwrap_or(type_name),
        "String" | "Bytes"
    )
}

pub(crate) fn is_copy_type_name(type_name: &str) -> bool {
    let type_name = type_name.trim();
    !type_name.contains('<')
        && matches!(
            type_name.strip_prefix("fresh ").unwrap_or(type_name),
            "Bool"
                | "Byte"
                | "Char"
                | "Float"
                | "Float32"
                | "Float64"
                | "Int"
                | "Int8"
                | "Int16"
                | "Int32"
                | "Int64"
                | "UInt"
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
                | "Unit"
        )
}

pub(crate) fn merge_if_state(
    state: &mut BodyState,
    base: &BodyState,
    then_state: BodyState,
    then_flow: Flow,
    else_branch: Option<(BodyState, Flow)>,
) -> Flow {
    let (else_state, else_flow) = else_branch.unwrap_or_else(|| (base.clone(), Flow::Fallthrough));
    let mut fallthrough_states = Vec::new();
    if then_flow == Flow::Fallthrough {
        fallthrough_states.push(then_state.clone());
    }
    if else_flow == Flow::Fallthrough {
        fallthrough_states.push(else_state.clone());
    }

    match fallthrough_states.as_slice() {
        [] => {
            *state = base.clone();
            merge_non_fallthrough(then_flow, else_flow)
        }
        [only] => {
            *state = fallthrough_projection(base, only);
            Flow::Fallthrough
        }
        [left, right] => {
            *state = merge_fallthrough_states(base, left, right);
            Flow::Fallthrough
        }
        _ => unreachable!("if has at most two branches"),
    }
}

pub(crate) fn merge_loop_state(
    state: &mut BodyState,
    base: &BodyState,
    body_state: BodyState,
    body_flow: Flow,
    may_skip: bool,
) -> Flow {
    if !may_skip && body_flow == Flow::Return {
        *state = base.clone();
        return Flow::Return;
    }
    if !may_skip && body_flow == Flow::Break {
        *state = fallthrough_projection(base, &body_state);
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

#[cfg(test)]
#[path = "tests.rs"]
mod tests;

pub(super) fn merge_non_fallthrough(left: Flow, right: Flow) -> Flow {
    if left == right { left } else { Flow::Return }
}

pub(super) fn fallthrough_projection(base: &BodyState, branch: &BodyState) -> BodyState {
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

    BodyState {
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

pub(super) fn merge_fallthrough_states(
    base: &BodyState,
    left: &BodyState,
    right: &BodyState,
) -> BodyState {
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

    BodyState {
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

pub(super) fn merge_moved_paths_from_branch(
    moved_paths: &mut HashMap<String, Span>,
    base: &BodyState,
    branch: &BodyState,
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
