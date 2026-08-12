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
                type_name: rsscript_semantics::hir_expr_type_name(&arm.operation)
                    .map(str::to_string),
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
    let fact = rsscript_semantics::fresh_match_binding(value, arm)?;
    Some(LocalFlowBinding {
        name: fact.name,
        kind: HirBindingKind::LocalLet,
        type_name: Some(strip_fresh_type(&fact.payload_type_name).to_string()),
        value_ident: None,
        value_handle_field: None,
        fresh_from_local_source: fact.source_ident,
        fresh_from_scrutinee: fact.fresh_from_scrutinee,
        fresh_from_fresh_value: false,
    })
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
    let uses = rsscript_semantics::hir_stmt_identifier_uses(statement);
    let events = rsscript_semantics::hir_stmt_effect_events(statement);
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
                    if let Some((body, _)) =
                        rsscript_semantics::retained_closure_argument(&arg.value)
                    {
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
    for (name, _) in rsscript_semantics::hir_block_inline_capture_uses(block) {
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
            ty,
            ..
        } => {
            let value_facts = value
                .as_ref()
                .map(rsscript_semantics::local_binding_value_facts)
                .unwrap_or_default();
            Some(LocalFlowBinding {
                name: name.clone(),
                kind: *kind,
                type_name: ty.as_ref().map(ToString::to_string),
                value_ident: value_facts.source_ident,
                value_handle_field: value_facts.handle_field_source,
                fresh_from_local_source: None,
                fresh_from_scrutinee: false,
                fresh_from_fresh_value: value_facts.is_fresh_value,
            })
        }
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

pub(super) fn local_flow_step_resource_binding(
    statement: &HirStmt,
) -> Option<LocalFlowResourceBinding> {
    match statement {
        HirStmt::With {
            binding, resource, ..
        } => Some(LocalFlowResourceBinding {
            name: binding.clone(),
            type_name: rsscript_semantics::hir_expr_type_name(resource).map(str::to_string),
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
    rsscript_semantics::merge_non_fallthrough(left, right)
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
        if rsscript_semantics::path_root(path).is_some_and(|root| base.locals.contains(root))
            || base.moved_paths.contains_key(path)
        {
            moved_paths
                .entry(path.clone())
                .or_insert_with(|| span.clone());
        }
    }
}
