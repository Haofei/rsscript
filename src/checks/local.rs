use std::collections::{HashMap, HashSet};

use crate::diagnostic::Span;
use crate::hir::{
    HirBinding, HirBindingKind, HirBlock, HirEffectEvent, HirEffectEventKind, HirExpr,
    HirFieldAccess, HirFunctionBody, HirReturnProof, HirStmt,
};

use super::body::Flow;

#[derive(Debug, Clone, Default)]
pub(crate) struct BodyState {
    pub(crate) locals: HashSet<String>,
    pub(crate) clean_locals: HashSet<String>,
    pub(crate) managed: HashSet<String>,
    pub(crate) moved: HashMap<String, Span>,
    pub(crate) value_types: HashMap<String, String>,
}

pub(crate) struct LocalAnalysis {
    body: Option<HirFunctionBody>,
    events_by_span: HashMap<Span, Vec<HirEffectEvent>>,
    binding_types_by_span: HashMap<Span, String>,
    field_accesses_by_span: HashMap<Span, HirFieldAccess>,
    return_proofs_by_span: HashMap<Span, HirReturnProof>,
    closure_uses_by_span: HashMap<Span, Vec<(String, Span)>>,
    flow_steps: Vec<LocalFlowStep>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LocalFlowStepKind {
    Statement,
    Branch,
    Loop,
    Return,
    Break,
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalFlowStep {
    id: usize,
    span: Span,
    kind: LocalFlowStepKind,
    uses: Vec<(String, Span)>,
    successors: Vec<usize>,
}

impl LocalAnalysis {
    pub(crate) fn new(body: Option<&HirFunctionBody>) -> Self {
        let body = body.cloned();
        let events_by_span = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(HashMap::new, index_events_from_block);
        let binding_types_by_span = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(HashMap::new, index_binding_types_from_block);
        let field_accesses_by_span = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(HashMap::new, index_field_accesses_from_block);
        let return_proofs_by_span = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(HashMap::new, index_return_proofs_from_block);
        let closure_uses_by_span = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(HashMap::new, index_closure_uses_from_block);
        let flow_steps = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(Vec::new, collect_local_flow_steps);

        Self {
            body,
            events_by_span,
            binding_types_by_span,
            field_accesses_by_span,
            return_proofs_by_span,
            closure_uses_by_span,
            flow_steps,
        }
    }

    pub(crate) fn initial_state(&self) -> BodyState {
        let mut state = BodyState::default();
        if let Some(body) = &self.body {
            state.seed_params(&body.bindings);
        }
        state
    }

    pub(crate) fn apply_move_events(&self, span: &Span, state: &mut BodyState) {
        state.apply_move_events(self.effect_events(span));
    }

    pub(crate) fn apply_retention_events(&self, span: &Span, state: &mut BodyState) {
        state.apply_retention_events(self.effect_events(span));
    }

    pub(crate) fn retained_value_spans(&self, span: &Span, binding: &str) -> Vec<Span> {
        self.effect_events(span)
            .iter()
            .filter_map(|event| {
                if matches!(event.kind, HirEffectEventKind::Retain { .. })
                    && event.binding_name == binding
                {
                    Some(event.value_span.clone())
                } else {
                    None
                }
            })
            .collect()
    }

    pub(crate) fn binding_type(&self, span: &Span) -> Option<&str> {
        self.binding_types_by_span.get(span).map(String::as_str)
    }

    pub(crate) fn field_access(&self, span: &Span) -> Option<&HirFieldAccess> {
        self.field_accesses_by_span.get(span)
    }

    pub(crate) fn return_proof(&self, span: &Span) -> Option<&HirReturnProof> {
        self.return_proofs_by_span.get(span)
    }

    pub(crate) fn closure_ident_uses(&self, span: &Span) -> Option<&[(String, Span)]> {
        self.closure_uses_by_span.get(span).map(Vec::as_slice)
    }

    pub(crate) fn statement_ident_uses(&self, span: &Span) -> Option<&[(String, Span)]> {
        debug_assert!(self.flow_steps.iter().all(|step| {
            self.flow_steps
                .get(step.id)
                .is_some_and(|candidate| candidate.span == step.span)
                && step
                    .successors
                    .iter()
                    .all(|successor| *successor < self.flow_steps.len())
        }));

        self.flow_steps
            .iter()
            .find(|step| step.span == *span && step.kind.collects_statement_uses())
            .map(|step| step.uses.as_slice())
    }

    fn effect_events(&self, span: &Span) -> &[HirEffectEvent] {
        self.events_by_span.get(span).map_or(&[], Vec::as_slice)
    }
}

impl LocalFlowStepKind {
    fn collects_statement_uses(self) -> bool {
        matches!(
            self,
            Self::Statement | Self::Branch | Self::Loop | Self::Return
        )
    }
}

fn index_events_from_block(block: &HirBlock) -> HashMap<Span, Vec<HirEffectEvent>> {
    let mut events = HashMap::new();
    collect_block_events(block, &mut events);
    events
}

fn collect_block_events(block: &HirBlock, events: &mut HashMap<Span, Vec<HirEffectEvent>>) {
    for statement in &block.statements {
        collect_stmt_events(statement, events);
    }
}

fn collect_stmt_events(statement: &HirStmt, events: &mut HashMap<Span, Vec<HirEffectEvent>>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_expr_events(value, events),
        HirStmt::With { resource, body, .. } => {
            collect_expr_events(resource, events);
            collect_block_events(body, events);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_events(condition, events);
            collect_block_events(then_body, events);
            if let Some(else_body) = else_body {
                collect_block_events(else_body, events);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_events(condition, events);
            }
            collect_block_events(body, events);
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_events(expr: &HirExpr, events: &mut HashMap<Span, Vec<HirEffectEvent>>) {
    match expr {
        HirExpr::Call {
            args,
            events: expr_events,
            span,
            ..
        } => {
            record_expr_events(events, span, expr_events);
            for arg in args {
                collect_expr_events(&arg.value, events);
            }
        }
        HirExpr::Effect {
            value,
            events: expr_events,
            span,
            ..
        }
        | HirExpr::Manage {
            value,
            events: expr_events,
            span,
            ..
        } => {
            record_expr_events(events, span, expr_events);
            collect_expr_events(value, events);
        }
        HirExpr::Field { base, .. } => collect_expr_events(base, events),
        HirExpr::Closure { body, .. } => collect_block_events(body, events),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn record_expr_events(
    events: &mut HashMap<Span, Vec<HirEffectEvent>>,
    span: &Span,
    expr_events: &[HirEffectEvent],
) {
    if expr_events.is_empty() {
        return;
    }
    events
        .entry(span.clone())
        .or_default()
        .extend(expr_events.iter().cloned());
}

fn index_binding_types_from_block(block: &HirBlock) -> HashMap<Span, String> {
    let mut types = HashMap::new();
    collect_block_binding_types(block, &mut types);
    types
}

fn collect_block_binding_types(block: &HirBlock, types: &mut HashMap<Span, String>) {
    for statement in &block.statements {
        collect_stmt_binding_types(statement, types);
    }
}

fn collect_stmt_binding_types(statement: &HirStmt, types: &mut HashMap<Span, String>) {
    match statement {
        HirStmt::Let {
            type_name: Some(type_name),
            span,
            ..
        } => {
            types.insert(span.clone(), type_name.clone());
        }
        HirStmt::With { body, .. } => collect_block_binding_types(body, types),
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => {
            collect_block_binding_types(then_body, types);
            if let Some(else_body) = else_body {
                collect_block_binding_types(else_body, types);
            }
        }
        HirStmt::Loop { body, .. } => collect_block_binding_types(body, types),
        HirStmt::Let {
            type_name: None, ..
        }
        | HirStmt::Return { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Expr(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn index_field_accesses_from_block(block: &HirBlock) -> HashMap<Span, HirFieldAccess> {
    let mut accesses = HashMap::new();
    collect_block_field_accesses(block, &mut accesses);
    accesses
}

fn collect_block_field_accesses(block: &HirBlock, accesses: &mut HashMap<Span, HirFieldAccess>) {
    for statement in &block.statements {
        collect_stmt_field_accesses(statement, accesses);
    }
}

fn collect_stmt_field_accesses(statement: &HirStmt, accesses: &mut HashMap<Span, HirFieldAccess>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_expr_field_accesses(value, accesses),
        HirStmt::With { resource, body, .. } => {
            collect_expr_field_accesses(resource, accesses);
            collect_block_field_accesses(body, accesses);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_field_accesses(condition, accesses);
            collect_block_field_accesses(then_body, accesses);
            if let Some(else_body) = else_body {
                collect_block_field_accesses(else_body, accesses);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_field_accesses(condition, accesses);
            }
            collect_block_field_accesses(body, accesses);
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_field_accesses(expr: &HirExpr, accesses: &mut HashMap<Span, HirFieldAccess>) {
    match expr {
        HirExpr::Field {
            base, access, span, ..
        } => {
            accesses.insert(span.clone(), access.clone());
            collect_expr_field_accesses(base, accesses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_field_accesses(&arg.value, accesses);
            }
        }
        HirExpr::Effect { value, .. } | HirExpr::Manage { value, .. } => {
            collect_expr_field_accesses(value, accesses);
        }
        HirExpr::Closure { body, .. } => collect_block_field_accesses(body, accesses),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn index_return_proofs_from_block(block: &HirBlock) -> HashMap<Span, HirReturnProof> {
    let mut proofs = HashMap::new();
    collect_block_return_proofs(block, &mut proofs);
    proofs
}

fn collect_block_return_proofs(block: &HirBlock, proofs: &mut HashMap<Span, HirReturnProof>) {
    for statement in &block.statements {
        collect_stmt_return_proofs(statement, proofs);
    }
}

fn collect_stmt_return_proofs(statement: &HirStmt, proofs: &mut HashMap<Span, HirReturnProof>) {
    match statement {
        HirStmt::Return { value, proof, span } => {
            let proof_span = value.as_ref().map_or(span, hir_expr_span);
            proofs.insert(proof_span.clone(), proof.clone());
            if let Some(value) = value {
                collect_expr_return_proofs(value, proofs);
            }
        }
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_expr_return_proofs(value, proofs),
        HirStmt::With { resource, body, .. } => {
            collect_expr_return_proofs(resource, proofs);
            collect_block_return_proofs(body, proofs);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_return_proofs(condition, proofs);
            collect_block_return_proofs(then_body, proofs);
            if let Some(else_body) = else_body {
                collect_block_return_proofs(else_body, proofs);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_return_proofs(condition, proofs);
            }
            collect_block_return_proofs(body, proofs);
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_return_proofs(expr: &HirExpr, proofs: &mut HashMap<Span, HirReturnProof>) {
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_return_proofs(&arg.value, proofs);
            }
        }
        HirExpr::Effect { value, .. } | HirExpr::Manage { value, .. } => {
            collect_expr_return_proofs(value, proofs);
        }
        HirExpr::Field { base, .. } => collect_expr_return_proofs(base, proofs),
        HirExpr::Closure { body, .. } => collect_block_return_proofs(body, proofs),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn hir_expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
        | HirExpr::Field { span, .. }
        | HirExpr::Call { span, .. }
        | HirExpr::Effect { span, .. }
        | HirExpr::Manage { span, .. }
        | HirExpr::Closure { span, .. }
        | HirExpr::Unknown(span) => span,
    }
}

fn index_closure_uses_from_block(block: &HirBlock) -> HashMap<Span, Vec<(String, Span)>> {
    let mut closures = HashMap::new();
    collect_block_closure_uses(block, &mut closures);
    closures
}

fn collect_block_closure_uses(block: &HirBlock, closures: &mut HashMap<Span, Vec<(String, Span)>>) {
    for statement in &block.statements {
        collect_stmt_closure_uses(statement, closures);
    }
}

fn collect_stmt_closure_uses(
    statement: &HirStmt,
    closures: &mut HashMap<Span, Vec<(String, Span)>>,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_expr_closure_uses(value, closures),
        HirStmt::With { resource, body, .. } => {
            collect_expr_closure_uses(resource, closures);
            collect_block_closure_uses(body, closures);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_closure_uses(condition, closures);
            collect_block_closure_uses(then_body, closures);
            if let Some(else_body) = else_body {
                collect_block_closure_uses(else_body, closures);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_closure_uses(condition, closures);
            }
            collect_block_closure_uses(body, closures);
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_closure_uses(expr: &HirExpr, closures: &mut HashMap<Span, Vec<(String, Span)>>) {
    match expr {
        HirExpr::Closure { body, span } => {
            let mut uses = Vec::new();
            collect_hir_block_idents(body, &mut uses);
            closures.insert(span.clone(), uses);
            collect_block_closure_uses(body, closures);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_closure_uses(&arg.value, closures);
            }
        }
        HirExpr::Effect { value, .. } | HirExpr::Manage { value, .. } => {
            collect_expr_closure_uses(value, closures);
        }
        HirExpr::Field { base, .. } => collect_expr_closure_uses(base, closures),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn collect_hir_block_idents(block: &HirBlock, uses: &mut Vec<(String, Span)>) {
    for statement in &block.statements {
        collect_hir_stmt_idents(statement, uses);
        match statement {
            HirStmt::With { body, .. } => collect_hir_block_idents(body, uses),
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_hir_block_idents(then_body, uses);
                if let Some(else_body) = else_body {
                    collect_hir_block_idents(else_body, uses);
                }
            }
            HirStmt::Loop { body, .. } => collect_hir_block_idents(body, uses),
            HirStmt::Let { .. }
            | HirStmt::Return { .. }
            | HirStmt::Expr(_)
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn collect_hir_stmt_idents(statement: &HirStmt, uses: &mut Vec<(String, Span)>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_hir_expr_idents(value, uses),
        HirStmt::With { resource, .. } => collect_hir_expr_idents(resource, uses),
        HirStmt::If { condition, .. } => collect_hir_expr_idents(condition, uses),
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => collect_hir_expr_idents(condition, uses),
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

fn collect_hir_expr_idents(expr: &HirExpr, uses: &mut Vec<(String, Span)>) {
    match expr {
        HirExpr::Ident { name, span, .. } => uses.push((name.clone(), span.clone())),
        HirExpr::Field { base, .. } => collect_hir_expr_idents(base, uses),
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_hir_expr_idents(&arg.value, uses);
            }
        }
        HirExpr::Effect { value, .. } | HirExpr::Manage { value, .. } => {
            collect_hir_expr_idents(value, uses);
        }
        HirExpr::Closure { body, .. } => collect_hir_block_idents(body, uses),
        HirExpr::Number { .. } | HirExpr::String { .. } | HirExpr::Unknown(_) => {}
    }
}

fn collect_local_flow_steps(block: &HirBlock) -> Vec<LocalFlowStep> {
    let mut steps = Vec::new();
    collect_block_local_flow(block, &mut steps);
    steps
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalFlowFragment {
    entry: Option<usize>,
    exits: Vec<usize>,
}

fn collect_block_local_flow(block: &HirBlock, steps: &mut Vec<LocalFlowStep>) -> LocalFlowFragment {
    let mut entry = None;
    let mut pending_exits = Vec::new();

    for statement in &block.statements {
        let fragment = collect_stmt_local_flow(statement, steps);
        if entry.is_none() {
            entry = fragment.entry;
        }
        if let Some(fragment_entry) = fragment.entry {
            for exit in pending_exits.drain(..) {
                add_successor(steps, exit, fragment_entry);
            }
        }
        pending_exits = fragment.exits;
    }

    LocalFlowFragment {
        entry,
        exits: pending_exits,
    }
}

fn collect_stmt_local_flow(
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
        HirStmt::With { body, .. } => collect_scoped_body_flow(steps, node, body),
        HirStmt::Return { .. } | HirStmt::Break(_) | HirStmt::Continue(_) => LocalFlowFragment {
            entry: Some(node),
            exits: Vec::new(),
        },
        HirStmt::Let { .. } | HirStmt::Expr(_) | HirStmt::Unknown(_) => LocalFlowFragment {
            entry: Some(node),
            exits: vec![node],
        },
    }
}

fn collect_if_local_flow(
    steps: &mut Vec<LocalFlowStep>,
    branch_node: usize,
    then_body: &HirBlock,
    else_body: Option<&HirBlock>,
) -> LocalFlowFragment {
    let then_flow = collect_block_local_flow(then_body, steps);
    if let Some(then_entry) = then_flow.entry {
        add_successor(steps, branch_node, then_entry);
    }

    let mut exits = then_flow.exits;
    if let Some(else_body) = else_body {
        let else_flow = collect_block_local_flow(else_body, steps);
        if let Some(else_entry) = else_flow.entry {
            add_successor(steps, branch_node, else_entry);
        }
        exits.extend(else_flow.exits);
    } else {
        exits.push(branch_node);
    }

    LocalFlowFragment {
        entry: Some(branch_node),
        exits,
    }
}

fn collect_loop_local_flow(
    steps: &mut Vec<LocalFlowStep>,
    loop_node: usize,
    may_skip: bool,
    body: &HirBlock,
) -> LocalFlowFragment {
    let body_flow = collect_block_local_flow(body, steps);
    if let Some(body_entry) = body_flow.entry {
        add_successor(steps, loop_node, body_entry);
    }
    for exit in body_flow.exits {
        add_successor(steps, exit, loop_node);
    }

    LocalFlowFragment {
        entry: Some(loop_node),
        exits: if may_skip {
            vec![loop_node]
        } else {
            Vec::new()
        },
    }
}

fn collect_scoped_body_flow(
    steps: &mut Vec<LocalFlowStep>,
    scoped_node: usize,
    body: &HirBlock,
) -> LocalFlowFragment {
    let body_flow = collect_block_local_flow(body, steps);
    if let Some(body_entry) = body_flow.entry {
        add_successor(steps, scoped_node, body_entry);
    }
    LocalFlowFragment {
        entry: Some(scoped_node),
        exits: if body_flow.entry.is_some() {
            body_flow.exits
        } else {
            vec![scoped_node]
        },
    }
}

fn push_local_flow_step(steps: &mut Vec<LocalFlowStep>, statement: &HirStmt) -> usize {
    let mut uses = Vec::new();
    collect_hir_stmt_idents(statement, &mut uses);
    let id = steps.len();
    steps.push(LocalFlowStep {
        id,
        span: hir_stmt_span(statement).clone(),
        kind: local_flow_step_kind(statement),
        uses,
        successors: Vec::new(),
    });
    id
}

fn add_successor(steps: &mut [LocalFlowStep], from: usize, to: usize) {
    if !steps[from].successors.contains(&to) {
        steps[from].successors.push(to);
    }
}

fn local_flow_step_kind(statement: &HirStmt) -> LocalFlowStepKind {
    match statement {
        HirStmt::If { .. } => LocalFlowStepKind::Branch,
        HirStmt::Loop { .. } => LocalFlowStepKind::Loop,
        HirStmt::Return { .. } => LocalFlowStepKind::Return,
        HirStmt::Break(_) => LocalFlowStepKind::Break,
        HirStmt::Continue(_) => LocalFlowStepKind::Continue,
        HirStmt::Let { .. } | HirStmt::With { .. } | HirStmt::Expr(_) | HirStmt::Unknown(_) => {
            LocalFlowStepKind::Statement
        }
    }
}

fn hir_stmt_span(statement: &HirStmt) -> &Span {
    match statement {
        HirStmt::Let { span, .. }
        | HirStmt::Return { span, .. }
        | HirStmt::With { span, .. }
        | HirStmt::If { span, .. }
        | HirStmt::Loop { span, .. }
        | HirStmt::Break(span)
        | HirStmt::Continue(span)
        | HirStmt::Unknown(span) => span,
        HirStmt::Expr(expr) => hir_expr_span(expr),
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
        }
    }

    pub(crate) fn bind_managed(&mut self, name: impl Into<String>) {
        self.managed.insert(name.into());
    }

    pub(crate) fn bind_local(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.locals.insert(name.clone());
        self.clean_locals.insert(name);
    }

    pub(crate) fn record_type(&mut self, name: impl Into<String>, type_name: impl Into<String>) {
        self.value_types.insert(name.into(), type_name.into());
    }

    pub(crate) fn mark_moved(&mut self, name: &str, span: Span) {
        self.moved.insert(name.to_string(), span);
        self.clean_locals.remove(name);
    }

    pub(crate) fn mark_retained(&mut self, name: &str) {
        self.clean_locals.remove(name);
    }

    pub(crate) fn is_local(&self, name: &str) -> bool {
        self.locals.contains(name)
    }

    pub(crate) fn is_managed(&self, name: &str) -> bool {
        self.managed.contains(name)
    }

    pub(crate) fn is_clean_local(&self, name: &str) -> bool {
        self.clean_locals.contains(name)
    }

    pub(crate) fn move_span(&self, name: &str) -> Option<&Span> {
        self.moved.get(name)
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
            if self.locals.contains(&event.binding_name) {
                self.mark_moved(&event.binding_name, event.span.clone());
            }
        }
    }

    pub(crate) fn apply_retention_events(&mut self, events: &[HirEffectEvent]) {
        for event in events {
            if !matches!(event.kind, HirEffectEventKind::Retain { .. }) {
                continue;
            }
            if self.locals.contains(&event.binding_name) {
                self.mark_retained(&event.binding_name);
            }
        }
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
    if body_flow != Flow::Return {
        for (name, span) in &body_state.moved {
            if base.locals.contains(name) || base.moved.contains_key(name) {
                moved.entry(name.clone()).or_insert_with(|| span.clone());
            }
        }
    }

    state.locals = base.locals.clone();
    state.managed = base.managed.clone();
    state.value_types = base.value_types.clone();
    state.moved = moved;
    state.clean_locals = base
        .clean_locals
        .intersection(&body_state.clean_locals)
        .filter(|name| base.locals.contains(*name))
        .cloned()
        .collect();
    Flow::Fallthrough
}

fn merge_non_fallthrough(left: Flow, right: Flow) -> Flow {
    if left == right { left } else { Flow::Return }
}

fn fallthrough_projection(base: &BodyState, branch: &BodyState) -> BodyState {
    let mut moved = base.moved.clone();
    for (name, span) in &branch.moved {
        if base.locals.contains(name) || base.moved.contains_key(name) {
            moved.entry(name.clone()).or_insert_with(|| span.clone());
        }
    }

    BodyState {
        locals: base.locals.clone(),
        managed: base.managed.clone(),
        value_types: base.value_types.clone(),
        moved,
        clean_locals: branch
            .clean_locals
            .intersection(&base.clean_locals)
            .filter(|name| base.locals.contains(*name))
            .cloned()
            .collect(),
    }
}

fn merge_fallthrough_states(base: &BodyState, left: &BodyState, right: &BodyState) -> BodyState {
    let mut moved = base.moved.clone();
    for branch in [left, right] {
        for (name, span) in &branch.moved {
            if base.locals.contains(name) || base.moved.contains_key(name) {
                moved.entry(name.clone()).or_insert_with(|| span.clone());
            }
        }
    }

    BodyState {
        locals: base.locals.clone(),
        managed: base.managed.clone(),
        value_types: base.value_types.clone(),
        moved,
        clean_locals: left
            .clean_locals
            .intersection(&right.clean_locals)
            .filter(|name| base.locals.contains(*name))
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::{CallResolution, HirBlock, HirExpr, HirFieldAccess, HirStmt};
    use crate::syntax::ast::Callee;

    fn span(line: usize) -> Span {
        Span {
            file: "test.rss".to_string(),
            line,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn applies_move_and_retain_events_to_clean_local_state() {
        let mut state = BodyState::default();
        state.bind_local("image");
        state.bind_local("cached");

        state.apply_retention_events(&[HirEffectEvent {
            function_name: "run".to_string(),
            kind: HirEffectEventKind::Retain {
                callee: "Cache.store".to_string(),
                param: "value".to_string(),
            },
            binding_name: "cached".to_string(),
            span: span(10),
            value_span: span(10),
        }]);
        state.apply_move_events(&[HirEffectEvent {
            function_name: "run".to_string(),
            kind: HirEffectEventKind::Manage,
            binding_name: "image".to_string(),
            span: span(11),
            value_span: span(11),
        }]);

        assert!(!state.clean_locals.contains("cached"));
        assert!(!state.clean_locals.contains("image"));
        assert_eq!(state.moved["image"].line, 11);
    }

    #[test]
    fn local_analysis_seeds_params_and_applies_events_by_span() {
        let retain_event = HirEffectEvent {
            function_name: "run".to_string(),
            kind: HirEffectEventKind::Retain {
                callee: "Cache.store".to_string(),
                param: "value".to_string(),
            },
            binding_name: "cached".to_string(),
            span: span(20),
            value_span: span(20),
        };
        let manage_event = HirEffectEvent {
            function_name: "run".to_string(),
            kind: HirEffectEventKind::Manage,
            binding_name: "image".to_string(),
            span: span(21),
            value_span: span(21),
        };
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            bindings: vec![HirBinding {
                function_name: "run".to_string(),
                name: "pool".to_string(),
                kind: HirBindingKind::Param,
                span: span(1),
                type_name: Some("ResourcePool<File>".to_string()),
            }],
            block: Some(HirBlock {
                statements: vec![
                    HirStmt::Let {
                        kind: HirBindingKind::LocalLet,
                        name: "cached".to_string(),
                        value: None,
                        type_name: Some("Image".to_string()),
                        span: span(2),
                    },
                    HirStmt::Expr(HirExpr::Call {
                        callee: Callee::Name("store".to_string()),
                        args: Vec::new(),
                        resolution: CallResolution::Unknown,
                        events: vec![retain_event],
                        type_name: None,
                        span: span(20),
                    }),
                    HirStmt::Expr(HirExpr::Manage {
                        value: Box::new(HirExpr::Ident {
                            name: "image".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(21),
                        }),
                        events: vec![manage_event],
                        type_name: Some("Image".to_string()),
                        span: span(21),
                    }),
                    HirStmt::Expr(HirExpr::Field {
                        base: Box::new(HirExpr::Ident {
                            name: "config".to_string(),
                            type_name: Some("Config".to_string()),
                            span: span(22),
                        }),
                        name: "rules".to_string(),
                        access: HirFieldAccess {
                            function_name: "run".to_string(),
                            name: "rules".to_string(),
                            span: span(22),
                            base_type: Some("Config".to_string()),
                            type_name: Some("Rules".to_string()),
                            is_handle: true,
                        },
                        span: span(22),
                    }),
                    HirStmt::Return {
                        value: Some(HirExpr::Call {
                            callee: Callee::Name("Image.load".to_string()),
                            args: Vec::new(),
                            resolution: CallResolution::Unknown,
                            events: Vec::new(),
                            type_name: Some("Image".to_string()),
                            span: span(23),
                        }),
                        proof: HirReturnProof::FreshCall,
                        span: span(23),
                    },
                    HirStmt::Expr(HirExpr::Closure {
                        body: HirBlock {
                            statements: vec![HirStmt::Expr(HirExpr::Ident {
                                name: "cached".to_string(),
                                type_name: Some("Image".to_string()),
                                span: span(25),
                            })],
                            span: span(24),
                        },
                        span: span(24),
                    }),
                ],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));
        let mut state = local_analysis.initial_state();
        state.bind_local("cached");
        state.bind_local("image");

        assert_eq!(state.value_type("pool"), Some("ResourcePool<File>"));
        assert_eq!(local_analysis.binding_type(&span(2)), Some("Image"));
        assert_eq!(
            local_analysis.retained_value_spans(&span(20), "cached"),
            vec![span(20)]
        );
        assert!(
            local_analysis
                .field_access(&span(22))
                .is_some_and(|access| access.is_handle)
        );
        assert!(matches!(
            local_analysis.return_proof(&span(23)),
            Some(HirReturnProof::FreshCall)
        ));
        assert_eq!(
            local_analysis.closure_ident_uses(&span(24)),
            Some(&[("cached".to_string(), span(25))][..])
        );
        assert_eq!(
            local_analysis.statement_ident_uses(&span(24)),
            Some(&[("cached".to_string(), span(25))][..])
        );

        local_analysis.apply_retention_events(&span(20), &mut state);
        local_analysis.apply_move_events(&span(21), &mut state);

        assert!(!state.is_clean_local("cached"));
        assert_eq!(state.move_span("image").map(|span| span.line), Some(21));
    }

    #[test]
    fn local_flow_steps_record_branch_successors() {
        let block = HirBlock {
            statements: vec![
                HirStmt::Let {
                    kind: HirBindingKind::LocalLet,
                    name: "seed".to_string(),
                    value: None,
                    type_name: Some("Image".to_string()),
                    span: span(1),
                },
                HirStmt::If {
                    condition: HirExpr::Ident {
                        name: "enabled".to_string(),
                        type_name: Some("Bool".to_string()),
                        span: span(2),
                    },
                    then_body: HirBlock {
                        statements: vec![HirStmt::Expr(HirExpr::Ident {
                            name: "left".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(3),
                        })],
                        span: span(3),
                    },
                    else_body: Some(HirBlock {
                        statements: vec![HirStmt::Expr(HirExpr::Ident {
                            name: "right".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(4),
                        })],
                        span: span(4),
                    }),
                    span: span(2),
                },
                HirStmt::Return {
                    value: Some(HirExpr::Ident {
                        name: "done".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(5),
                    }),
                    proof: HirReturnProof::Ident {
                        name: "done".to_string(),
                    },
                    span: span(5),
                },
            ],
            span: span(1),
        };

        let steps = collect_local_flow_steps(&block);

        assert_eq!(steps.len(), 5);
        assert_eq!(steps[0].id, 0);
        assert_eq!(steps[0].successors, vec![1]);
        assert_eq!(steps[1].kind, LocalFlowStepKind::Branch);
        assert_eq!(steps[1].successors, vec![2, 3]);
        assert_eq!(steps[2].successors, vec![4]);
        assert_eq!(steps[3].successors, vec![4]);
        assert!(steps[4].successors.is_empty());
    }
}
