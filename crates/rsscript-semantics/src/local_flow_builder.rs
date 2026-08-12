//! Checked-HIR to local ownership-flow graph lowering.

use crate::hir::{
    CallResolution, HirBindingKind, HirBlock, HirExpr, HirMatchArm, HirSelectArm, HirStmt,
};
use crate::{
    LocalFlowBinding, LocalFlowEdge, LocalFlowResourceBinding, LocalFlowStep, LocalFlowStepKind,
    fresh_match_binding, hir_block_inline_capture_uses, hir_expr_type_name, hir_stmt_effect_events,
    hir_stmt_identifier_uses, local_binding_value_facts, retained_closure_argument,
};
use rsscript_syntax::Span;

/// Build the neutral ownership-flow graph for a checked function block.
pub fn local_flow_graph(block: &HirBlock) -> Vec<LocalFlowStep> {
    let mut steps = Vec::new();
    collect_block(block, &mut steps);
    steps
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Fragment {
    entry: Option<usize>,
    exits: Vec<Exit>,
    breaks: Vec<Exit>,
    continues: Vec<Exit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Exit {
    node: usize,
    drop_resources: Vec<String>,
}

impl Exit {
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
            self.drop_resources.push(resource.to_owned());
        }
        self
    }
}

fn collect_block(block: &HirBlock, steps: &mut Vec<LocalFlowStep>) -> Fragment {
    let mut entry = None;
    let mut pending_exits = Vec::new();
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    let mut reachable = true;

    for statement in &block.statements {
        let fragment = collect_stmt(statement, steps);
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

    Fragment {
        entry,
        exits: pending_exits,
        breaks,
        continues,
    }
}

fn collect_stmt(statement: &HirStmt, steps: &mut Vec<LocalFlowStep>) -> Fragment {
    let node = push_step(steps, statement);
    match statement {
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => collect_if(steps, node, then_body, else_body.as_ref()),
        HirStmt::Loop {
            condition, body, ..
        } => collect_loop(steps, node, condition.is_some(), body),
        HirStmt::For { body, .. } => collect_loop(steps, node, true, body),
        HirStmt::Match { value, arms, .. } => collect_match(steps, node, value, arms),
        HirStmt::Select { arms, .. } => collect_select(steps, node, arms),
        HirStmt::With { binding, body, .. } => collect_scoped(steps, node, binding, body),
        HirStmt::Return { .. } => Fragment {
            entry: Some(node),
            exits: Vec::new(),
            breaks: Vec::new(),
            continues: Vec::new(),
        },
        HirStmt::Break(_) => Fragment {
            entry: Some(node),
            exits: Vec::new(),
            breaks: vec![Exit::new(node)],
            continues: Vec::new(),
        },
        HirStmt::Continue(_) => Fragment {
            entry: Some(node),
            exits: Vec::new(),
            breaks: Vec::new(),
            continues: vec![Exit::new(node)],
        },
        HirStmt::Let { .. } | HirStmt::Expr(_) | HirStmt::Assign { .. } | HirStmt::Unknown(_) => {
            Fragment {
                entry: Some(node),
                exits: vec![Exit::new(node)],
                breaks: Vec::new(),
                continues: Vec::new(),
            }
        }
    }
}

fn collect_match(
    steps: &mut Vec<LocalFlowStep>,
    match_node: usize,
    value: &HirExpr,
    arms: &[HirMatchArm],
) -> Fragment {
    let mut exits = Vec::new();
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    if arms.is_empty() {
        exits.push(Exit::new(match_node));
    }
    for arm in arms {
        let arm_flow = collect_block(&arm.body, steps);
        let arm_entry = if let Some(binding) = fresh_match_binding_step(value, arm) {
            let binding_node = push_pattern_binding(steps, &arm.span, binding);
            if let Some(body_entry) = arm_flow.entry {
                add_successor(steps, Exit::new(binding_node), body_entry);
            }
            Some(binding_node)
        } else {
            arm_flow.entry
        };
        if let Some(arm_entry) = arm_entry {
            add_successor(steps, Exit::new(match_node), arm_entry);
            exits.extend(arm_flow.exits);
            breaks.extend(arm_flow.breaks);
            continues.extend(arm_flow.continues);
        } else {
            exits.push(Exit::new(match_node));
        }
    }
    Fragment {
        entry: Some(match_node),
        exits,
        breaks,
        continues,
    }
}

fn collect_select(
    steps: &mut Vec<LocalFlowStep>,
    select_node: usize,
    arms: &[HirSelectArm],
) -> Fragment {
    let mut exits = Vec::new();
    let mut breaks = Vec::new();
    let mut continues = Vec::new();
    if arms.is_empty() {
        exits.push(Exit::new(select_node));
    }
    for arm in arms {
        let arm_flow = collect_block(&arm.body, steps);
        let arm_entry = if arm.binding != "_" {
            let binding = LocalFlowBinding {
                name: arm.binding.clone(),
                kind: HirBindingKind::ManagedLet,
                type_name: hir_expr_type_name(&arm.operation).map(str::to_owned),
                value_ident: None,
                value_handle_field: None,
                fresh_from_local_source: None,
                fresh_from_scrutinee: false,
                fresh_from_fresh_value: false,
            };
            let binding_node = push_pattern_binding(steps, &arm.span, binding);
            if let Some(body_entry) = arm_flow.entry {
                add_successor(steps, Exit::new(binding_node), body_entry);
            }
            Some(binding_node)
        } else {
            arm_flow.entry
        };
        if let Some(arm_entry) = arm_entry {
            add_successor(steps, Exit::new(select_node), arm_entry);
            exits.extend(arm_flow.exits);
            breaks.extend(arm_flow.breaks);
            continues.extend(arm_flow.continues);
        } else {
            exits.push(Exit::new(select_node));
        }
    }
    Fragment {
        entry: Some(select_node),
        exits,
        breaks,
        continues,
    }
}

fn push_pattern_binding(
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

fn fresh_match_binding_step(value: &HirExpr, arm: &HirMatchArm) -> Option<LocalFlowBinding> {
    let fact = fresh_match_binding(value, arm)?;
    Some(LocalFlowBinding {
        name: fact.name,
        kind: HirBindingKind::LocalLet,
        type_name: Some(strip_fresh_type(&fact.payload_type_name).to_owned()),
        value_ident: None,
        value_handle_field: None,
        fresh_from_local_source: fact.source_ident,
        fresh_from_scrutinee: fact.fresh_from_scrutinee,
        fresh_from_fresh_value: false,
    })
}

fn collect_if(
    steps: &mut Vec<LocalFlowStep>,
    branch_node: usize,
    then_body: &HirBlock,
    else_body: Option<&HirBlock>,
) -> Fragment {
    let then_flow = collect_block(then_body, steps);
    if let Some(then_entry) = then_flow.entry {
        add_successor(steps, Exit::new(branch_node), then_entry);
    }
    let mut exits = then_flow.exits;
    let mut breaks = then_flow.breaks;
    let mut continues = then_flow.continues;
    if let Some(else_body) = else_body {
        let else_flow = collect_block(else_body, steps);
        if let Some(else_entry) = else_flow.entry {
            add_successor(steps, Exit::new(branch_node), else_entry);
        }
        exits.extend(else_flow.exits);
        breaks.extend(else_flow.breaks);
        continues.extend(else_flow.continues);
    } else {
        exits.push(Exit::new(branch_node));
    }
    Fragment {
        entry: Some(branch_node),
        exits,
        breaks,
        continues,
    }
}

fn collect_loop(
    steps: &mut Vec<LocalFlowStep>,
    loop_node: usize,
    may_skip: bool,
    body: &HirBlock,
) -> Fragment {
    let body_flow = collect_block(body, steps);
    if let Some(body_entry) = body_flow.entry {
        add_successor(steps, Exit::new(loop_node), body_entry);
    }
    for exit in body_flow.exits.iter().chain(body_flow.continues.iter()) {
        add_successor(steps, exit.clone(), loop_node);
    }
    let mut exits = if may_skip {
        vec![Exit::new(loop_node)]
    } else {
        Vec::new()
    };
    exits.extend(body_flow.breaks);
    Fragment {
        entry: Some(loop_node),
        exits,
        breaks: Vec::new(),
        continues: Vec::new(),
    }
}

fn collect_scoped(
    steps: &mut Vec<LocalFlowStep>,
    scoped_node: usize,
    binding: &str,
    body: &HirBlock,
) -> Fragment {
    let body_flow = collect_block(body, steps);
    if let Some(body_entry) = body_flow.entry {
        add_successor(steps, Exit::new(scoped_node), body_entry);
    }
    let empty_body_exit = Exit::new(scoped_node).with_drop(binding);
    Fragment {
        entry: Some(scoped_node),
        exits: if body_flow.entry.is_some() {
            drop_on_exits(body_flow.exits, binding)
        } else {
            vec![empty_body_exit]
        },
        breaks: drop_on_exits(body_flow.breaks, binding),
        continues: drop_on_exits(body_flow.continues, binding),
    }
}

fn drop_on_exits(exits: Vec<Exit>, resource: &str) -> Vec<Exit> {
    exits
        .into_iter()
        .map(|exit| exit.with_drop(resource))
        .collect()
}

fn push_step(steps: &mut Vec<LocalFlowStep>, statement: &HirStmt) -> usize {
    let id = steps.len();
    steps.push(LocalFlowStep {
        id,
        span: stmt_span(statement).clone(),
        kind: step_kind(statement),
        uses: hir_stmt_identifier_uses(statement),
        managed_closure_captures: managed_closure_captures(statement),
        binding: step_binding(statement),
        resource_binding: step_resource_binding(statement),
        events: hir_stmt_effect_events(statement),
        successors: Vec::new(),
    });
    id
}

fn step_kind(statement: &HirStmt) -> LocalFlowStepKind {
    match statement {
        HirStmt::If { .. } | HirStmt::Match { .. } | HirStmt::Select { .. } => {
            LocalFlowStepKind::Branch
        }
        HirStmt::Loop { .. } | HirStmt::For { .. } => LocalFlowStepKind::Loop,
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

fn step_binding(statement: &HirStmt) -> Option<LocalFlowBinding> {
    let HirStmt::Let {
        kind,
        name,
        value,
        ty,
        ..
    } = statement
    else {
        return None;
    };
    let facts = value
        .as_ref()
        .map(local_binding_value_facts)
        .unwrap_or_default();
    Some(LocalFlowBinding {
        name: name.clone(),
        kind: *kind,
        type_name: ty.as_ref().map(ToString::to_string),
        value_ident: facts.source_ident,
        value_handle_field: facts.handle_field_source,
        fresh_from_local_source: None,
        fresh_from_scrutinee: false,
        fresh_from_fresh_value: facts.is_fresh_value,
    })
}

fn step_resource_binding(statement: &HirStmt) -> Option<LocalFlowResourceBinding> {
    let HirStmt::With {
        binding, resource, ..
    } = statement
    else {
        return None;
    };
    Some(LocalFlowResourceBinding {
        name: binding.clone(),
        type_name: hir_expr_type_name(resource).map(str::to_owned),
    })
}

fn managed_closure_captures(statement: &HirStmt) -> Vec<String> {
    let mut captures = Vec::new();
    collect_stmt_captures(statement, &mut captures);
    captures
}

fn collect_stmt_captures(statement: &HirStmt, captures: &mut Vec<String>) {
    match statement {
        HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            value: Some(HirExpr::Closure { body, .. }),
            ..
        } => push_inline_captures(body, captures),
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => collect_expr_captures(value, captures),
        HirStmt::With { resource, .. } => collect_expr_captures(resource, captures),
        HirStmt::If { condition, .. } => collect_expr_captures(condition, captures),
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => collect_expr_captures(condition, captures),
        HirStmt::For { iterable, .. } => collect_expr_captures(iterable, captures),
        HirStmt::Match { value, .. } => collect_expr_captures(value, captures),
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_captures(&arm.operation, captures);
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

fn collect_expr_captures(expr: &HirExpr, captures: &mut Vec<String>) {
    match expr {
        HirExpr::Call {
            args, resolution, ..
        } => {
            if let CallResolution::Resolved { signature, .. } = resolution {
                for arg in args {
                    if arg
                        .name
                        .as_ref()
                        .is_some_and(|name| signature.retained_params.contains(name))
                        && let Some((body, _)) = retained_closure_argument(&arg.value)
                    {
                        push_inline_captures(body, captures);
                    }
                }
            }
            for arg in args {
                collect_expr_captures(&arg.value, captures);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_expr_captures(value, captures),
        HirExpr::Binary { left, right, .. }
        | HirExpr::Index {
            base: left,
            index: right,
            ..
        } => {
            collect_expr_captures(left, captures);
            collect_expr_captures(right, captures);
        }
        HirExpr::Field { base, .. } => collect_expr_captures(base, captures),
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_captures(&entry.key, captures);
                collect_expr_captures(&entry.value, captures);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_expr_captures(&field.value, captures);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_captures(item, captures);
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

fn push_inline_captures(block: &HirBlock, captures: &mut Vec<String>) {
    for (name, _) in hir_block_inline_capture_uses(block) {
        if !captures.contains(&name) {
            captures.push(name);
        }
    }
}

fn add_successor(steps: &mut [LocalFlowStep], from: Exit, to: usize) {
    let edge = LocalFlowEdge {
        to,
        drop_resources: from.drop_resources,
    };
    if !steps[from.node].successors.contains(&edge) {
        steps[from.node].successors.push(edge);
    }
}

/// Return the source span that identifies a checked-HIR statement in flow facts.
pub fn local_flow_statement_span(statement: &HirStmt) -> &Span {
    stmt_span(statement)
}

fn stmt_span(statement: &HirStmt) -> &Span {
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
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => expr_span(expr),
    }
}

fn expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::Char { span, .. }
        | HirExpr::ObjectLiteral { span, .. }
        | HirExpr::MapLiteral { span, .. }
        | HirExpr::ArrayLiteral { span, .. }
        | HirExpr::Binary { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Index { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Effect { span, .. }
        | HirExpr::Manage { span, .. }
        | HirExpr::Spawn { span, .. }
        | HirExpr::Await { span, .. }
        | HirExpr::Try { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Match { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}

fn strip_fresh_type(type_name: &str) -> &str {
    type_name
        .trim()
        .strip_prefix("fresh ")
        .unwrap_or(type_name)
        .trim()
}
