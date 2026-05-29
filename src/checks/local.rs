use std::collections::{HashMap, HashSet, VecDeque};

use crate::diagnostic::Span;
use crate::hir::{
    CallResolution, HirBinding, HirBindingKind, HirBlock, HirEffectEvent, HirEffectEventKind,
    HirExpr, HirFunctionBody, HirReturnProof, HirStmt, ParamEffect,
};
use crate::syntax::ast::Callee;

use super::body::Flow;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BodyState {
    pub(crate) locals: HashSet<String>,
    pub(crate) clean_locals: HashSet<String>,
    pub(crate) managed: HashSet<String>,
    pub(crate) resources: HashSet<String>,
    pub(crate) moved: HashMap<String, Span>,
    pub(crate) moved_paths: HashMap<String, Span>,
    pub(crate) value_types: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MovedUse {
    pub(crate) name: String,
    pub(crate) use_span: Span,
    pub(crate) move_span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedToLocalUse {
    pub(crate) local_name: String,
    pub(crate) managed_name: String,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetainedLocalUse {
    pub(crate) name: String,
    pub(crate) callee: String,
    pub(crate) param: String,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RetainedClosureCapture {
    pub(crate) name: String,
    pub(crate) callee: String,
    pub(crate) param: String,
    pub(crate) capture_span: Span,
    pub(crate) closure_span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TakeHandleField {
    pub(crate) name: String,
    pub(crate) span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FreshReturnIssueKind {
    NotClean { name: String },
    UnknownIdent { name: String },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FreshReturnIssue {
    pub(crate) kind: FreshReturnIssueKind,
    pub(crate) span: Span,
}

pub(crate) struct LocalAnalysis {
    body: Option<HirFunctionBody>,
    managed_closure_uses_by_span: HashMap<Span, Vec<(String, Span)>>,
    resource_escapes_by_with_span: HashMap<Span, Vec<ResourceEscape>>,
    take_handle_fields: Vec<TakeHandleField>,
    flow_steps: Vec<LocalFlowStep>,
    flow_entry_states_by_span: HashMap<Span, BodyState>,
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
    managed_closure_captures: Vec<String>,
    binding: Option<LocalFlowBinding>,
    resource_binding: Option<LocalFlowResourceBinding>,
    events: Vec<HirEffectEvent>,
    successors: Vec<LocalFlowEdge>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalFlowBinding {
    name: String,
    kind: HirBindingKind,
    type_name: Option<String>,
    value_ident: Option<(String, Span)>,
    value_handle_field: Option<(String, Span)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalFlowResourceBinding {
    name: String,
    type_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalFlowEdge {
    to: usize,
    drop_resources: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceEscapeKind {
    Escape,
    Capture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResourceEscape {
    pub(crate) binding: String,
    pub(crate) kind: ResourceEscapeKind,
    pub(crate) span: Span,
}

impl LocalAnalysis {
    pub(crate) fn new(body: Option<&HirFunctionBody>) -> Self {
        let body = body.cloned();
        let managed_closure_uses_by_span = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(HashMap::new, index_managed_closure_uses_from_block);
        let resource_escapes_by_with_span = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(HashMap::new, index_resource_escapes_from_block);
        let take_handle_fields = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(Vec::new, collect_take_handle_fields);
        let flow_steps = body
            .as_ref()
            .and_then(|body| body.block.as_ref())
            .map_or_else(Vec::new, collect_local_flow_steps);
        let flow_entry_states_by_span =
            collect_flow_entry_states(&flow_steps, initial_state_from_body(body.as_ref()));

        Self {
            body,
            managed_closure_uses_by_span,
            resource_escapes_by_with_span,
            take_handle_fields,
            flow_steps,
            flow_entry_states_by_span,
        }
    }

    pub(crate) fn initial_state(&self) -> BodyState {
        initial_state_from_body(self.body.as_ref())
    }

    pub(crate) fn managed_closure_ident_uses(&self, span: &Span) -> Option<&[(String, Span)]> {
        self.managed_closure_uses_by_span
            .get(span)
            .map(Vec::as_slice)
    }

    pub(crate) fn resource_escapes(&self, span: &Span) -> Option<&[ResourceEscape]> {
        self.resource_escapes_by_with_span
            .get(span)
            .map(Vec::as_slice)
    }

    pub(crate) fn take_handle_fields(&self) -> &[TakeHandleField] {
        &self.take_handle_fields
    }

    pub(crate) fn flow_entry_state(&self, span: &Span) -> Option<&BodyState> {
        self.flow_entry_states_by_span.get(span)
    }

    pub(crate) fn moved_uses(&self) -> Vec<MovedUse> {
        let mut moved_uses = Vec::new();
        if let Some(block) = self.body.as_ref().and_then(|body| body.block.as_ref()) {
            collect_ordered_moved_uses_from_block(
                block,
                &self.flow_entry_states_by_span,
                &mut moved_uses,
            );
            collect_closure_local_moved_uses_from_block(block, &mut moved_uses);
        }
        moved_uses
    }

    pub(crate) fn managed_to_local_uses(&self) -> Vec<ManagedToLocalUse> {
        let mut uses = Vec::new();
        for step in &self.flow_steps {
            let Some(binding) = &step.binding else {
                continue;
            };
            if binding.kind != HirBindingKind::LocalLet {
                continue;
            }
            if let Some((managed_name, span)) = &binding.value_ident
                && self
                    .flow_entry_states_by_span
                    .get(&step.span)
                    .is_some_and(|state| state.is_managed(managed_name))
            {
                uses.push(ManagedToLocalUse {
                    local_name: binding.name.clone(),
                    managed_name: managed_name.clone(),
                    span: span.clone(),
                });
            }
            if let Some((managed_name, span)) = &binding.value_handle_field {
                uses.push(ManagedToLocalUse {
                    local_name: binding.name.clone(),
                    managed_name: managed_name.clone(),
                    span: span.clone(),
                });
            }
        }
        uses
    }

    pub(crate) fn retained_local_uses(&self) -> Vec<RetainedLocalUse> {
        let mut uses = Vec::new();
        for step in &self.flow_steps {
            let Some(state) = self.flow_entry_states_by_span.get(&step.span) else {
                continue;
            };
            for event in &step.events {
                let HirEffectEventKind::Retain { callee, param } = &event.kind else {
                    continue;
                };
                if state.is_local(&event.binding_name) {
                    let retained = RetainedLocalUse {
                        name: event.binding_name.clone(),
                        callee: callee.clone(),
                        param: param.clone(),
                        span: event.value_span.clone(),
                    };
                    if !uses.contains(&retained) {
                        uses.push(retained);
                    }
                }
            }
        }
        uses
    }

    pub(crate) fn retained_closure_captures(&self) -> Vec<RetainedClosureCapture> {
        let mut captures = Vec::new();
        if let Some(block) = self.body.as_ref().and_then(|body| body.block.as_ref()) {
            collect_retained_closure_captures_from_block(
                block,
                &self.flow_entry_states_by_span,
                &mut captures,
            );
        }
        captures
    }

    pub(crate) fn fresh_return_issues(&self) -> Vec<FreshReturnIssue> {
        let mut issues = Vec::new();
        if let Some(block) = self.body.as_ref().and_then(|body| body.block.as_ref()) {
            collect_fresh_return_issues_from_block(
                block,
                &self.flow_entry_states_by_span,
                &mut issues,
            );
        }
        issues
    }
}

fn initial_state_from_body(body: Option<&HirFunctionBody>) -> BodyState {
    let mut state = BodyState::default();
    if let Some(body) = body {
        state.seed_params(&body.bindings);
    }
    state
}

fn collect_take_handle_fields(block: &HirBlock) -> Vec<TakeHandleField> {
    let mut fields = Vec::new();
    collect_block_take_handle_fields(block, &mut fields);
    fields
}

fn collect_block_take_handle_fields(block: &HirBlock, fields: &mut Vec<TakeHandleField>) {
    for statement in &block.statements {
        collect_stmt_take_handle_fields(statement, fields);
    }
}

fn collect_stmt_take_handle_fields(statement: &HirStmt, fields: &mut Vec<TakeHandleField>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_expr_take_handle_fields(value, fields),
        HirStmt::With { resource, body, .. } => {
            collect_expr_take_handle_fields(resource, fields);
            collect_block_take_handle_fields(body, fields);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_take_handle_fields(condition, fields);
            collect_block_take_handle_fields(then_body, fields);
            if let Some(else_body) = else_body {
                collect_block_take_handle_fields(else_body, fields);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_take_handle_fields(condition, fields);
            }
            collect_block_take_handle_fields(body, fields);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expr_take_handle_fields(value, fields);
            for arm in arms {
                collect_block_take_handle_fields(&arm.body, fields);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_take_handle_fields(expr: &HirExpr, fields: &mut Vec<TakeHandleField>) {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Take,
            value,
            span,
            ..
        } => {
            if let HirExpr::Field { name, access, .. } = value.as_ref()
                && access.is_handle
            {
                push_take_handle_field(fields, name.clone(), span.clone());
            }
            collect_expr_take_handle_fields(value, fields);
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_expr_take_handle_fields(value, fields);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expr_take_handle_fields(left, fields);
            collect_expr_take_handle_fields(right, fields);
        }
        HirExpr::Field { base, .. } => collect_expr_take_handle_fields(base, fields),
        HirExpr::Index { base, index, .. } => {
            collect_expr_take_handle_fields(base, fields);
            collect_expr_take_handle_fields(index, fields);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_take_handle_fields(&arg.value, fields);
            }
        }
        HirExpr::Closure { body, .. } => collect_block_take_handle_fields(body, fields),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn push_take_handle_field(fields: &mut Vec<TakeHandleField>, name: String, span: Span) {
    let field = TakeHandleField { name, span };
    if !fields.contains(&field) {
        fields.push(field);
    }
}

fn collect_fresh_return_issues_from_block(
    block: &HirBlock,
    entry_states: &HashMap<Span, BodyState>,
    issues: &mut Vec<FreshReturnIssue>,
) {
    for statement in &block.statements {
        collect_fresh_return_issues_from_stmt(statement, entry_states, issues);
    }
}

fn collect_fresh_return_issues_from_stmt(
    statement: &HirStmt,
    entry_states: &HashMap<Span, BodyState>,
    issues: &mut Vec<FreshReturnIssue>,
) {
    match statement {
        HirStmt::Return { value, proof, span } => {
            collect_fresh_return_issue(value.as_ref(), proof, span, entry_states, issues);
        }
        HirStmt::With { body, .. } => {
            collect_fresh_return_issues_from_block(body, entry_states, issues);
        }
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => {
            collect_fresh_return_issues_from_block(then_body, entry_states, issues);
            if let Some(else_body) = else_body {
                collect_fresh_return_issues_from_block(else_body, entry_states, issues);
            }
        }
        HirStmt::Loop { body, .. } => {
            collect_fresh_return_issues_from_block(body, entry_states, issues);
        }
        HirStmt::Match { arms, .. } => {
            for arm in arms {
                collect_fresh_return_issues_from_block(&arm.body, entry_states, issues);
            }
        }
        HirStmt::Let { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Expr(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_fresh_return_issue(
    value: Option<&HirExpr>,
    proof: &HirReturnProof,
    return_span: &Span,
    entry_states: &HashMap<Span, BodyState>,
    issues: &mut Vec<FreshReturnIssue>,
) {
    match proof {
        HirReturnProof::Ident { name } => {
            let span = fresh_return_value_span(value)
                .unwrap_or(return_span)
                .clone();
            if let Some(state) = entry_states.get(return_span) {
                if state.is_managed(name) || (state.is_local(name) && !state.is_clean_local(name)) {
                    push_fresh_return_issue(
                        issues,
                        FreshReturnIssueKind::NotClean { name: name.clone() },
                        span,
                    );
                    return;
                }
                if state.is_local(name) {
                    return;
                }
            }
            push_fresh_return_issue(
                issues,
                FreshReturnIssueKind::UnknownIdent { name: name.clone() },
                span,
            );
        }
        HirReturnProof::Unknown => {
            if let Some(value) = value
                && fresh_field_access_base(value).is_some_and(|name| {
                    entry_states
                        .get(return_span)
                        .is_some_and(|state| state.is_local(name) && state.is_clean_local(name))
                })
            {
                return;
            }
            push_fresh_return_issue(
                issues,
                FreshReturnIssueKind::Unknown,
                fresh_return_value_span(value)
                    .unwrap_or(return_span)
                    .clone(),
            );
        }
        HirReturnProof::NoValue | HirReturnProof::StructConstructor | HirReturnProof::FreshCall => {
        }
    }
}

fn fresh_field_access_base(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Field { base, access, .. } if !access.is_handle => fresh_field_access_base(base),
        HirExpr::Ident { name, .. } => Some(name),
        HirExpr::Call { callee, args, .. } if fresh_wrapper_callee(callee) => args
            .first()
            .and_then(|arg| fresh_field_access_base(&arg.value)),
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => fresh_field_access_base(value),
        HirExpr::Manage { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn fresh_wrapper_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Name(name) if matches!(name.as_str(), "Ok" | "Some")
    )
}

fn fresh_return_value_span(value: Option<&HirExpr>) -> Option<&Span> {
    let mut value = value?;
    loop {
        match value {
            HirExpr::Effect { value: inner, .. } | HirExpr::Manage { value: inner, .. } => {
                value = inner;
            }
            _ => return Some(hir_expr_span(value)),
        }
    }
}

fn push_fresh_return_issue(
    issues: &mut Vec<FreshReturnIssue>,
    kind: FreshReturnIssueKind,
    span: Span,
) {
    let issue = FreshReturnIssue { kind, span };
    if !issues.contains(&issue) {
        issues.push(issue);
    }
}

fn collect_retained_closure_captures_from_block(
    block: &HirBlock,
    entry_states: &HashMap<Span, BodyState>,
    captures: &mut Vec<RetainedClosureCapture>,
) {
    for statement in &block.statements {
        collect_retained_closure_captures_from_stmt(statement, entry_states, captures);
    }
}

fn collect_ordered_moved_uses_from_block(
    block: &HirBlock,
    entry_states: &HashMap<Span, BodyState>,
    moved_uses: &mut Vec<MovedUse>,
) {
    for statement in &block.statements {
        collect_ordered_moved_uses_from_stmt(statement, entry_states, moved_uses);
    }
}

fn collect_ordered_moved_uses_from_stmt(
    statement: &HirStmt,
    entry_states: &HashMap<Span, BodyState>,
    moved_uses: &mut Vec<MovedUse>,
) {
    let entry_state = entry_states.get(hir_stmt_span(statement)).cloned();
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(value, &mut state, moved_uses);
            }
        }
        HirStmt::With { resource, body, .. } => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(resource, &mut state, moved_uses);
            }
            collect_ordered_moved_uses_from_block(body, entry_states, moved_uses);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(condition, &mut state, moved_uses);
            }
            collect_ordered_moved_uses_from_block(then_body, entry_states, moved_uses);
            if let Some(else_body) = else_body {
                collect_ordered_moved_uses_from_block(else_body, entry_states, moved_uses);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let (Some(condition), Some(mut state)) = (condition, entry_state) {
                collect_ordered_moved_uses_from_expr(condition, &mut state, moved_uses);
            }
            collect_ordered_moved_uses_from_block(body, entry_states, moved_uses);
        }
        HirStmt::Match { value, arms, .. } => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(value, &mut state, moved_uses);
            }
            for arm in arms {
                collect_ordered_moved_uses_from_block(&arm.body, entry_states, moved_uses);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_ordered_moved_uses_from_expr(
    expr: &HirExpr,
    state: &mut BodyState,
    moved_uses: &mut Vec<MovedUse>,
) {
    match expr {
        HirExpr::Ident { name, span, .. } => {
            if let Some(move_span) = state.move_span(name) {
                push_moved_use(moved_uses, name.clone(), span.clone(), move_span.clone());
            } else if let Some((moved_path, move_span)) = state.moved_subpath_span(name) {
                push_moved_use(moved_uses, moved_path, span.clone(), move_span.clone());
            }
        }
        HirExpr::Call { args, events, .. } => {
            for arg in args {
                collect_ordered_moved_uses_from_expr(&arg.value, state, moved_uses);
            }
            state.apply_move_events(events);
        }
        HirExpr::Effect { value, events, .. } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
            state.apply_move_events(events);
        }
        HirExpr::Manage {
            value,
            events,
            span,
            ..
        } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
            state.apply_move_events(events);
            if let Some((path, _)) = hir_expr_path(value) {
                state.mark_moved(&path, span.clone());
            }
        }
        HirExpr::Spawn { value, .. } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
        }
        HirExpr::Await { value, .. } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
        }
        HirExpr::Try { value, .. } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_ordered_moved_uses_from_expr(left, state, moved_uses);
            collect_ordered_moved_uses_from_expr(right, state, moved_uses);
        }
        HirExpr::Field { base, .. } => {
            if let Some((path, span)) = hir_expr_path(expr) {
                if let Some(root) = path_root(&path)
                    && let Some(move_span) = state.move_span(root)
                {
                    push_moved_use(moved_uses, root.to_string(), span, move_span.clone());
                    return;
                }
                if let Some((moved_path, move_span)) = state.moved_path_span(&path) {
                    push_moved_use(moved_uses, moved_path, span, move_span.clone());
                }
            } else {
                collect_ordered_moved_uses_from_expr(base, state, moved_uses);
            }
        }
        HirExpr::Index { base, index, .. } => {
            collect_ordered_moved_uses_from_expr(base, state, moved_uses);
            collect_ordered_moved_uses_from_expr(index, state, moved_uses);
        }
        HirExpr::Closure { body, .. } => {
            let mut uses = Vec::new();
            collect_hir_block_idents(body, &mut uses);
            for (name, span) in uses {
                if let Some(move_span) = state.move_span(&name) {
                    push_moved_use(moved_uses, name, span, move_span.clone());
                }
            }
        }
        HirExpr::Number { .. } | HirExpr::String { .. } | HirExpr::Unknown(_) => {}
    }
}

fn collect_closure_local_moved_uses_from_block(block: &HirBlock, moved_uses: &mut Vec<MovedUse>) {
    for statement in &block.statements {
        collect_closure_local_moved_uses_from_stmt(statement, moved_uses);
    }
}

fn collect_closure_local_moved_uses_from_stmt(statement: &HirStmt, moved_uses: &mut Vec<MovedUse>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_closure_local_moved_uses_from_expr(value, moved_uses),
        HirStmt::With { resource, body, .. } => {
            collect_closure_local_moved_uses_from_expr(resource, moved_uses);
            collect_closure_local_moved_uses_from_block(body, moved_uses);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_closure_local_moved_uses_from_expr(condition, moved_uses);
            collect_closure_local_moved_uses_from_block(then_body, moved_uses);
            if let Some(else_body) = else_body {
                collect_closure_local_moved_uses_from_block(else_body, moved_uses);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_closure_local_moved_uses_from_expr(condition, moved_uses);
            }
            collect_closure_local_moved_uses_from_block(body, moved_uses);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_closure_local_moved_uses_from_expr(value, moved_uses);
            for arm in arms {
                collect_closure_local_moved_uses_from_block(&arm.body, moved_uses);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_closure_local_moved_uses_from_expr(expr: &HirExpr, moved_uses: &mut Vec<MovedUse>) {
    match expr {
        HirExpr::Closure { body, .. } => {
            let steps = collect_local_flow_steps(body);
            let entry_states = collect_flow_entry_states(&steps, BodyState::default());
            collect_ordered_moved_uses_from_block(body, &entry_states, moved_uses);
            collect_closure_local_moved_uses_from_block(body, moved_uses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_closure_local_moved_uses_from_expr(&arg.value, moved_uses);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. }
        | HirExpr::Field { base: value, .. } => {
            collect_closure_local_moved_uses_from_expr(value, moved_uses);
        }
        HirExpr::Index { base, index, .. } => {
            collect_closure_local_moved_uses_from_expr(base, moved_uses);
            collect_closure_local_moved_uses_from_expr(index, moved_uses);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_closure_local_moved_uses_from_expr(left, moved_uses);
            collect_closure_local_moved_uses_from_expr(right, moved_uses);
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn push_moved_use(moved_uses: &mut Vec<MovedUse>, name: String, use_span: Span, move_span: Span) {
    let moved_use = MovedUse {
        name,
        use_span,
        move_span,
    };
    if !moved_uses.contains(&moved_use) {
        moved_uses.push(moved_use);
    }
}

fn hir_expr_path(expr: &HirExpr) -> Option<(String, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } => Some((name.clone(), span.clone())),
        HirExpr::Field {
            base, name, span, ..
        } => {
            let (mut base_path, _) = hir_expr_path(base)?;
            base_path.push('.');
            base_path.push_str(name);
            Some((base_path, span.clone()))
        }
        _ => None,
    }
}

fn path_root(path: &str) -> Option<&str> {
    path.split('.').next().filter(|root| !root.is_empty())
}

fn collect_retained_closure_captures_from_stmt(
    statement: &HirStmt,
    entry_states: &HashMap<Span, BodyState>,
    captures: &mut Vec<RetainedClosureCapture>,
) {
    let entry_state = entry_states.get(hir_stmt_span(statement));
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(value, state, captures);
            }
        }
        HirStmt::With { resource, body, .. } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(resource, state, captures);
            }
            collect_retained_closure_captures_from_block(body, entry_states, captures);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(condition, state, captures);
            }
            collect_retained_closure_captures_from_block(then_body, entry_states, captures);
            if let Some(else_body) = else_body {
                collect_retained_closure_captures_from_block(else_body, entry_states, captures);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let (Some(condition), Some(state)) = (condition, entry_state) {
                collect_retained_closure_captures_from_expr(condition, state, captures);
            }
            collect_retained_closure_captures_from_block(body, entry_states, captures);
        }
        HirStmt::Match { value, arms, .. } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(value, state, captures);
            }
            for arm in arms {
                collect_retained_closure_captures_from_block(&arm.body, entry_states, captures);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_retained_closure_captures_from_expr(
    expr: &HirExpr,
    state: &BodyState,
    captures: &mut Vec<RetainedClosureCapture>,
) {
    match expr {
        HirExpr::Call {
            callee,
            args,
            resolution,
            ..
        } => {
            if let CallResolution::Resolved { signature, .. } = resolution {
                for arg in args {
                    let Some(name) = arg.name.as_ref() else {
                        continue;
                    };
                    if !signature.retained_params.contains(name) {
                        continue;
                    }
                    let Some((body, closure_span)) = retained_closure_arg(&arg.value) else {
                        continue;
                    };
                    let mut uses = Vec::new();
                    collect_hir_block_inline_capture_uses(body, &mut uses);
                    for (used_name, capture_span) in uses {
                        if state.is_local(&used_name) {
                            push_retained_closure_capture(
                                captures,
                                RetainedClosureCapture {
                                    name: used_name,
                                    callee: callee_display(callee),
                                    param: name.clone(),
                                    capture_span,
                                    closure_span: closure_span.clone(),
                                },
                            );
                        }
                    }
                }
            }
            for arg in args {
                collect_retained_closure_captures_from_expr(&arg.value, state, captures);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_retained_closure_captures_from_expr(value, state, captures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_retained_closure_captures_from_expr(left, state, captures);
            collect_retained_closure_captures_from_expr(right, state, captures);
        }
        HirExpr::Field { base, .. } => {
            collect_retained_closure_captures_from_expr(base, state, captures);
        }
        HirExpr::Index { base, index, .. } => {
            collect_retained_closure_captures_from_expr(base, state, captures);
            collect_retained_closure_captures_from_expr(index, state, captures);
        }
        HirExpr::Closure { body, .. } => {
            collect_retained_closure_captures_from_block(body, &HashMap::new(), captures);
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn retained_closure_arg(expr: &HirExpr) -> Option<(&HirBlock, &Span)> {
    match expr {
        HirExpr::Closure { body, span, .. } => Some((body, span)),
        HirExpr::Effect {
            effect: ParamEffect::Read,
            value,
            ..
        } => retained_closure_arg(value),
        HirExpr::Call { callee, args, .. } if retained_closure_wrapper_callee(callee) => {
            args.iter().find_map(|arg| retained_closure_arg(&arg.value))
        }
        HirExpr::Effect { .. }
        | HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Try { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn retained_closure_wrapper_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Name(name) if matches!(name.as_str(), "Ok" | "Err" | "Some")
    )
}

fn push_retained_closure_capture(
    captures: &mut Vec<RetainedClosureCapture>,
    capture: RetainedClosureCapture,
) {
    if !captures.contains(&capture) {
        captures.push(capture);
    }
}

fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
    }
}

fn hir_expr_span(expr: &HirExpr) -> &Span {
    match expr {
        HirExpr::Ident { span, .. }
        | HirExpr::Number { span, .. }
        | HirExpr::String { span, .. }
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
        | HirExpr::Unknown(span) => span,
    }
}

fn index_managed_closure_uses_from_block(block: &HirBlock) -> HashMap<Span, Vec<(String, Span)>> {
    let mut closures = HashMap::new();
    collect_block_managed_closure_uses(block, &mut closures);
    closures
}

fn index_resource_escapes_from_block(block: &HirBlock) -> HashMap<Span, Vec<ResourceEscape>> {
    let mut escapes = HashMap::new();
    collect_block_resource_escapes(block, &mut escapes);
    escapes
}

fn collect_block_resource_escapes(
    block: &HirBlock,
    escapes_by_with_span: &mut HashMap<Span, Vec<ResourceEscape>>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::With {
                binding,
                body,
                span,
                ..
            } => {
                let mut escapes = Vec::new();
                collect_resource_escapes_in_block(binding, body, &mut escapes);
                escapes_by_with_span.insert(span.clone(), escapes);
                collect_block_resource_escapes(body, escapes_by_with_span);
            }
            HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value) => collect_expr_resource_escapes(value, escapes_by_with_span),
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_expr_resource_escapes(condition, escapes_by_with_span);
                collect_block_resource_escapes(then_body, escapes_by_with_span);
                if let Some(else_body) = else_body {
                    collect_block_resource_escapes(else_body, escapes_by_with_span);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_expr_resource_escapes(condition, escapes_by_with_span);
                }
                collect_block_resource_escapes(body, escapes_by_with_span);
            }
            HirStmt::Match { value, arms, .. } => {
                collect_expr_resource_escapes(value, escapes_by_with_span);
                for arm in arms {
                    collect_block_resource_escapes(&arm.body, escapes_by_with_span);
                }
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn collect_expr_resource_escapes(
    expr: &HirExpr,
    escapes_by_with_span: &mut HashMap<Span, Vec<ResourceEscape>>,
) {
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_resource_escapes(&arg.value, escapes_by_with_span);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_expr_resource_escapes(value, escapes_by_with_span);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expr_resource_escapes(left, escapes_by_with_span);
            collect_expr_resource_escapes(right, escapes_by_with_span);
        }
        HirExpr::Field { base, .. } => collect_expr_resource_escapes(base, escapes_by_with_span),
        HirExpr::Index { base, index, .. } => {
            collect_expr_resource_escapes(base, escapes_by_with_span);
            collect_expr_resource_escapes(index, escapes_by_with_span);
        }
        HirExpr::Closure { body, .. } => collect_block_resource_escapes(body, escapes_by_with_span),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn collect_resource_escapes_in_block(
    binding: &str,
    block: &HirBlock,
    escapes: &mut Vec<ResourceEscape>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Return {
                value: Some(value), ..
            } if let Some(span) = resource_escape_operand_span(value, binding) => {
                push_resource_escape(escapes, binding, ResourceEscapeKind::Escape, span);
            }
            HirStmt::Let {
                kind: HirBindingKind::ManagedLet,
                value: Some(value),
                ..
            } if let Some(span) = resource_escape_operand_span(value, binding) => {
                push_resource_escape(escapes, binding, ResourceEscapeKind::Escape, span);
            }
            HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Expr(value) => collect_resource_escapes_in_expr(binding, value, escapes),
            HirStmt::With { body, .. } => collect_resource_escapes_in_block(binding, body, escapes),
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_resource_escapes_in_expr(binding, condition, escapes);
                collect_resource_escapes_in_block(binding, then_body, escapes);
                if let Some(else_body) = else_body {
                    collect_resource_escapes_in_block(binding, else_body, escapes);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_resource_escapes_in_expr(binding, condition, escapes);
                }
                collect_resource_escapes_in_block(binding, body, escapes);
            }
            HirStmt::Match { value, arms, .. } => {
                collect_resource_escapes_in_expr(binding, value, escapes);
                for arm in arms {
                    collect_resource_escapes_in_block(binding, &arm.body, escapes);
                }
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }

        if let HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            value: Some(value),
            ..
        } = statement
            && let Some(span) = managed_binding_resource_capture_span(value, binding)
        {
            push_resource_escape(escapes, binding, ResourceEscapeKind::Capture, span);
        }
    }
}

fn managed_binding_resource_capture_span(expr: &HirExpr, binding: &str) -> Option<Span> {
    match expr {
        HirExpr::Closure { body, span, .. } if hir_block_mentions_ident(body, binding) => {
            Some(span.clone())
        }
        HirExpr::Effect {
            effect: ParamEffect::Read | ParamEffect::Mut,
            value,
            ..
        } => managed_binding_resource_capture_span(value, binding),
        HirExpr::Call { callee, args, .. } if resource_escape_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| managed_binding_resource_capture_span(&arg.value, binding)),
        _ => None,
    }
}

fn collect_resource_escapes_in_expr(
    binding: &str,
    expr: &HirExpr,
    escapes: &mut Vec<ResourceEscape>,
) {
    match expr {
        HirExpr::Manage { value, span, .. } => {
            if resource_escape_operand_span(value, binding).is_some() {
                push_resource_escape(escapes, binding, ResourceEscapeKind::Escape, span.clone());
            }
            collect_resource_escapes_in_expr(binding, value, escapes);
        }
        HirExpr::Call { args, events, .. } => {
            for event in events {
                if matches!(event.kind, HirEffectEventKind::Retain { .. })
                    && event.binding_name == binding
                {
                    push_resource_escape(
                        escapes,
                        binding,
                        ResourceEscapeKind::Escape,
                        event.value_span.clone(),
                    );
                }
            }
            for arg in args {
                collect_resource_escapes_in_expr(binding, &arg.value, escapes);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_resource_escapes_in_expr(binding, value, escapes);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_resource_escapes_in_expr(binding, left, escapes);
            collect_resource_escapes_in_expr(binding, right, escapes);
        }
        HirExpr::Field { base, .. } => collect_resource_escapes_in_expr(binding, base, escapes),
        HirExpr::Index { base, index, .. } => {
            collect_resource_escapes_in_expr(binding, base, escapes);
            collect_resource_escapes_in_expr(binding, index, escapes);
        }
        HirExpr::Closure { body, .. } => collect_resource_escapes_in_block(binding, body, escapes),
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn resource_escape_operand_span(expr: &HirExpr, binding: &str) -> Option<Span> {
    match expr {
        HirExpr::Ident { name, span, .. } if name == binding => Some(span.clone()),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            resource_escape_operand_span(value, binding)
        }
        HirExpr::Call { callee, args, .. } if resource_escape_wrapper_callee(callee) => args
            .iter()
            .find_map(|arg| resource_escape_operand_span(&arg.value, binding)),
        _ => None,
    }
}

fn resource_escape_wrapper_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Name(name) if matches!(name.as_str(), "Ok" | "Err" | "Some")
    )
}

fn hir_block_mentions_ident(block: &HirBlock, binding: &str) -> bool {
    let mut uses = Vec::new();
    collect_hir_block_idents(block, &mut uses);
    uses.iter().any(|(name, _)| name == binding)
}

fn push_resource_escape(
    escapes: &mut Vec<ResourceEscape>,
    binding: &str,
    kind: ResourceEscapeKind,
    span: Span,
) {
    let escape = ResourceEscape {
        binding: binding.to_string(),
        kind,
        span,
    };
    if !escapes.contains(&escape) {
        escapes.push(escape);
    }
}

fn collect_block_managed_closure_uses(
    block: &HirBlock,
    closures: &mut HashMap<Span, Vec<(String, Span)>>,
) {
    for statement in &block.statements {
        collect_stmt_managed_closure_uses(statement, closures);
    }
}

fn collect_stmt_managed_closure_uses(
    statement: &HirStmt,
    closures: &mut HashMap<Span, Vec<(String, Span)>>,
) {
    match statement {
        HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            value: Some(HirExpr::Closure { body, .. }),
            span,
            ..
        } => {
            let mut uses = Vec::new();
            collect_hir_block_inline_capture_uses(body, &mut uses);
            closures.insert(span.clone(), uses);
            collect_block_managed_closure_uses(body, closures);
        }
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_expr_managed_closure_uses(value, closures),
        HirStmt::With { resource, body, .. } => {
            collect_expr_managed_closure_uses(resource, closures);
            collect_block_managed_closure_uses(body, closures);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_managed_closure_uses(condition, closures);
            collect_block_managed_closure_uses(then_body, closures);
            if let Some(else_body) = else_body {
                collect_block_managed_closure_uses(else_body, closures);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_managed_closure_uses(condition, closures);
            }
            collect_block_managed_closure_uses(body, closures);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expr_managed_closure_uses(value, closures);
            for arm in arms {
                collect_block_managed_closure_uses(&arm.body, closures);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

fn collect_expr_managed_closure_uses(
    expr: &HirExpr,
    closures: &mut HashMap<Span, Vec<(String, Span)>>,
) {
    match expr {
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_managed_closure_uses(&arg.value, closures);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_expr_managed_closure_uses(value, closures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expr_managed_closure_uses(left, closures);
            collect_expr_managed_closure_uses(right, closures);
        }
        HirExpr::Field { base, .. } => collect_expr_managed_closure_uses(base, closures),
        HirExpr::Index { base, index, .. } => {
            collect_expr_managed_closure_uses(base, closures);
            collect_expr_managed_closure_uses(index, closures);
        }
        HirExpr::Closure { body, .. } => collect_block_managed_closure_uses(body, closures),
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
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_hir_block_idents(&arm.body, uses);
                }
            }
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
        HirStmt::Match { value, .. } => collect_hir_expr_idents(value, uses),
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

fn collect_hir_stmt_effect_events(statement: &HirStmt, events: &mut Vec<HirEffectEvent>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_hir_expr_effect_events(value, events),
        HirStmt::With { resource, .. } => collect_hir_expr_effect_events(resource, events),
        HirStmt::If { condition, .. } => collect_hir_expr_effect_events(condition, events),
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => collect_hir_expr_effect_events(condition, events),
        HirStmt::Match { value, .. } => collect_hir_expr_effect_events(value, events),
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

fn collect_hir_expr_effect_events(expr: &HirExpr, events: &mut Vec<HirEffectEvent>) {
    match expr {
        HirExpr::Call {
            args,
            events: expr_events,
            ..
        } => {
            events.extend(expr_events.iter().cloned());
            for arg in args {
                collect_hir_expr_effect_events(&arg.value, events);
            }
        }
        HirExpr::Effect {
            value,
            events: expr_events,
            ..
        }
        | HirExpr::Manage {
            value,
            events: expr_events,
            ..
        } => {
            events.extend(expr_events.iter().cloned());
            collect_hir_expr_effect_events(value, events);
        }
        HirExpr::Spawn { value, .. } | HirExpr::Await { value, .. } => {
            collect_hir_expr_effect_events(value, events)
        }
        HirExpr::Try { value, .. } => collect_hir_expr_effect_events(value, events),
        HirExpr::Binary { left, right, .. } => {
            collect_hir_expr_effect_events(left, events);
            collect_hir_expr_effect_events(right, events);
        }
        HirExpr::Field { base, .. } => collect_hir_expr_effect_events(base, events),
        HirExpr::Index { base, index, .. } => {
            collect_hir_expr_effect_events(base, events);
            collect_hir_expr_effect_events(index, events);
        }
        HirExpr::Closure { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn collect_hir_expr_idents(expr: &HirExpr, uses: &mut Vec<(String, Span)>) {
    match expr {
        HirExpr::Ident { name, span, .. } => uses.push((name.clone(), span.clone())),
        HirExpr::Field { base, .. } => collect_hir_expr_idents(base, uses),
        HirExpr::Index { base, index, .. } => {
            collect_hir_expr_idents(base, uses);
            collect_hir_expr_idents(index, uses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_hir_expr_idents(&arg.value, uses);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_hir_expr_idents(value, uses);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_hir_expr_idents(left, uses);
            collect_hir_expr_idents(right, uses);
        }
        HirExpr::Closure { body, .. } => collect_hir_block_idents(body, uses),
        HirExpr::Number { .. } | HirExpr::String { .. } | HirExpr::Unknown(_) => {}
    }
}

fn collect_hir_block_inline_capture_uses(block: &HirBlock, uses: &mut Vec<(String, Span)>) {
    for statement in &block.statements {
        collect_hir_stmt_inline_capture_uses(statement, uses);
        match statement {
            HirStmt::With { body, .. } => collect_hir_block_inline_capture_uses(body, uses),
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_hir_block_inline_capture_uses(then_body, uses);
                if let Some(else_body) = else_body {
                    collect_hir_block_inline_capture_uses(else_body, uses);
                }
            }
            HirStmt::Loop { body, .. } => collect_hir_block_inline_capture_uses(body, uses),
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_hir_block_inline_capture_uses(&arm.body, uses);
                }
            }
            HirStmt::Let { .. }
            | HirStmt::Return { .. }
            | HirStmt::Expr(_)
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn collect_hir_stmt_inline_capture_uses(statement: &HirStmt, uses: &mut Vec<(String, Span)>) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_hir_expr_inline_capture_uses(value, uses),
        HirStmt::With { resource, .. } => collect_hir_expr_inline_capture_uses(resource, uses),
        HirStmt::If { condition, .. } => collect_hir_expr_inline_capture_uses(condition, uses),
        HirStmt::Loop {
            condition: Some(condition),
            ..
        } => collect_hir_expr_inline_capture_uses(condition, uses),
        HirStmt::Match { value, .. } => collect_hir_expr_inline_capture_uses(value, uses),
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

fn collect_hir_expr_inline_capture_uses(expr: &HirExpr, uses: &mut Vec<(String, Span)>) {
    match expr {
        HirExpr::Ident { name, span, .. } => uses.push((name.clone(), span.clone())),
        HirExpr::Field { base, access, .. } => {
            if !access.is_handle {
                collect_hir_expr_inline_capture_uses(base, uses);
            }
        }
        HirExpr::Index { base, index, .. } => {
            collect_hir_expr_inline_capture_uses(base, uses);
            collect_hir_expr_inline_capture_uses(index, uses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_hir_expr_inline_capture_uses(&arg.value, uses);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_hir_expr_inline_capture_uses(value, uses);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_hir_expr_inline_capture_uses(left, uses);
            collect_hir_expr_inline_capture_uses(right, uses);
        }
        HirExpr::Closure { body, .. } => collect_hir_block_inline_capture_uses(body, uses),
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
    exits: Vec<LocalFlowExit>,
    breaks: Vec<LocalFlowExit>,
    continues: Vec<LocalFlowExit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalFlowExit {
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

fn collect_block_local_flow(block: &HirBlock, steps: &mut Vec<LocalFlowStep>) -> LocalFlowFragment {
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
        HirStmt::Match { arms, .. } => collect_match_local_flow(steps, node, arms),
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
        HirStmt::Let { .. } | HirStmt::Expr(_) | HirStmt::Unknown(_) => LocalFlowFragment {
            entry: Some(node),
            exits: vec![LocalFlowExit::new(node)],
            breaks: Vec::new(),
            continues: Vec::new(),
        },
    }
}

fn collect_match_local_flow(
    steps: &mut Vec<LocalFlowStep>,
    match_node: usize,
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
        if let Some(arm_entry) = arm_flow.entry {
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

fn collect_if_local_flow(
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

fn collect_loop_local_flow(
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

fn collect_scoped_body_flow(
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

fn drop_resource_on_exits(exits: Vec<LocalFlowExit>, resource: &str) -> Vec<LocalFlowExit> {
    exits
        .into_iter()
        .map(|exit| exit.with_drop(resource))
        .collect()
}

fn push_local_flow_step(steps: &mut Vec<LocalFlowStep>, statement: &HirStmt) -> usize {
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

fn local_flow_step_managed_closure_captures(statement: &HirStmt) -> Vec<String> {
    let mut captures = Vec::new();
    collect_stmt_managed_closure_capture_names(statement, &mut captures);
    captures
}

fn collect_stmt_managed_closure_capture_names(statement: &HirStmt, captures: &mut Vec<String>) {
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
        | HirStmt::Expr(value) => collect_expr_managed_closure_capture_names(value, captures),
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
        HirStmt::Match { value, .. } => {
            collect_expr_managed_closure_capture_names(value, captures);
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

fn collect_expr_managed_closure_capture_names(expr: &HirExpr, captures: &mut Vec<String>) {
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
        HirExpr::Closure { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn push_hir_block_inline_capture_names(block: &HirBlock, captures: &mut Vec<String>) {
    let mut uses = Vec::new();
    collect_hir_block_inline_capture_uses(block, &mut uses);
    for (name, _) in uses {
        if !captures.contains(&name) {
            captures.push(name);
        }
    }
}

fn add_successor(steps: &mut [LocalFlowStep], from: LocalFlowExit, to: usize) {
    let from_node = from.node;
    let edge = LocalFlowEdge {
        to,
        drop_resources: from.drop_resources,
    };
    if !steps[from_node].successors.contains(&edge) {
        steps[from_node].successors.push(edge);
    }
}

fn collect_flow_entry_states(
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

fn transfer_flow_step(step: &LocalFlowStep, mut state: BodyState) -> BodyState {
    if let Some(binding) = &step.binding {
        match binding.kind {
            HirBindingKind::ManagedLet => state.bind_managed(binding.name.clone()),
            HirBindingKind::LocalLet => state.bind_local(binding.name.clone()),
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
        if state.is_local(capture) {
            state.mark_retained(capture);
        }
    }
    state.apply_retention_events(&step.events);
    state.apply_move_events(&step.events);
    state
}

fn merge_flow_entry_state(target: &mut Option<BodyState>, incoming: &BodyState) -> bool {
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

fn merge_flow_states(left: &BodyState, right: &BodyState) -> BodyState {
    let locals = left
        .locals
        .intersection(&right.locals)
        .cloned()
        .collect::<HashSet<_>>();
    let managed = left
        .managed
        .intersection(&right.managed)
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

    let clean_locals = left
        .clean_locals
        .intersection(&right.clean_locals)
        .filter(|name| locals.contains(*name))
        .cloned()
        .collect();

    BodyState {
        locals,
        clean_locals,
        managed,
        resources,
        moved,
        moved_paths,
        value_types,
    }
}

fn local_flow_step_kind(statement: &HirStmt) -> LocalFlowStepKind {
    match statement {
        HirStmt::If { .. } => LocalFlowStepKind::Branch,
        HirStmt::Match { .. } => LocalFlowStepKind::Branch,
        HirStmt::Loop { .. } => LocalFlowStepKind::Loop,
        HirStmt::Return { .. } => LocalFlowStepKind::Return,
        HirStmt::Break(_) => LocalFlowStepKind::Break,
        HirStmt::Continue(_) => LocalFlowStepKind::Continue,
        HirStmt::Let { .. } | HirStmt::With { .. } | HirStmt::Expr(_) | HirStmt::Unknown(_) => {
            LocalFlowStepKind::Statement
        }
    }
}

fn local_flow_step_binding(statement: &HirStmt) -> Option<LocalFlowBinding> {
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
        }),
        HirStmt::Return { .. }
        | HirStmt::With { .. }
        | HirStmt::If { .. }
        | HirStmt::Match { .. }
        | HirStmt::Loop { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Expr(_)
        | HirStmt::Unknown(_) => None,
    }
}

fn local_binding_source_ident(value: &HirExpr) -> Option<(String, Span)> {
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
        | HirExpr::Unknown(_) => None,
    }
}

fn local_binding_handle_field_source(value: &HirExpr) -> Option<(String, Span)> {
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
        | HirExpr::Unknown(_) => None,
    }
}

fn local_binding_wrapper_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Name(name) if matches!(name.as_str(), "Ok" | "Err" | "Some")
    )
}

fn local_flow_step_resource_binding(statement: &HirStmt) -> Option<LocalFlowResourceBinding> {
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
        | HirStmt::Match { .. }
        | HirStmt::Loop { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Expr(_)
        | HirStmt::Unknown(_) => None,
    }
}

fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { type_name, .. }
        | HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn hir_stmt_span(statement: &HirStmt) -> &Span {
    match statement {
        HirStmt::Let { span, .. }
        | HirStmt::Return { span, .. }
        | HirStmt::With { span, .. }
        | HirStmt::If { span, .. }
        | HirStmt::Match { span, .. }
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
            if matches!(binding.effect, Some(ParamEffect::Mut | ParamEffect::Take)) {
                self.bind_local(binding.name.clone());
            }
        }
    }

    pub(crate) fn bind_managed(&mut self, name: impl Into<String>) {
        self.managed.insert(name.into());
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
            }
        } else {
            self.moved.insert(name.to_string(), span);
            self.clean_locals.remove(name);
        }
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

    pub(crate) fn is_resource(&self, name: &str) -> bool {
        self.resources.contains(name)
    }

    pub(crate) fn is_clean_local(&self, name: &str) -> bool {
        self.clean_locals.contains(name)
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
            } else if self.locals.contains(&event.binding_name) {
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
    state.managed = base.managed.clone();
    state.resources = base.resources.clone();
    state.value_types = base.value_types.clone();
    state.moved = moved;
    state.moved_paths = moved_paths;
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
    let mut moved_paths = base.moved_paths.clone();
    for (name, span) in &branch.moved {
        if base.locals.contains(name) || base.moved.contains_key(name) {
            moved.entry(name.clone()).or_insert_with(|| span.clone());
        }
    }
    merge_moved_paths_from_branch(&mut moved_paths, base, branch);

    BodyState {
        locals: base.locals.clone(),
        managed: base.managed.clone(),
        resources: base.resources.clone(),
        value_types: base.value_types.clone(),
        moved,
        moved_paths,
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
    let mut moved_paths = base.moved_paths.clone();
    for branch in [left, right] {
        for (name, span) in &branch.moved {
            if base.locals.contains(name) || base.moved.contains_key(name) {
                moved.entry(name.clone()).or_insert_with(|| span.clone());
            }
        }
        merge_moved_paths_from_branch(&mut moved_paths, base, branch);
    }

    BodyState {
        locals: base.locals.clone(),
        managed: base.managed.clone(),
        resources: base.resources.clone(),
        value_types: base.value_types.clone(),
        moved,
        moved_paths,
        clean_locals: left
            .clean_locals
            .intersection(&right.clean_locals)
            .filter(|name| base.locals.contains(*name))
            .cloned()
            .collect(),
    }
}

fn merge_moved_paths_from_branch(
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

    fn successor_ids(step: &LocalFlowStep) -> Vec<usize> {
        step.successors.iter().map(|edge| edge.to).collect()
    }

    fn successor_drop_resources(step: &LocalFlowStep, to: usize) -> Vec<String> {
        step.successors
            .iter()
            .find(|edge| edge.to == to)
            .map_or_else(Vec::new, |edge| edge.drop_resources.clone())
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
                effect: Some(ParamEffect::Mut),
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
                        value_type_name: None,
                        span: span(2),
                    },
                    HirStmt::Expr(HirExpr::Call {
                        callee: Callee::Name("store".to_string()),
                        args: Vec::new(),
                        resolution: CallResolution::Unknown,
                        events: vec![retain_event.clone()],
                        type_name: None,
                        span: span(20),
                    }),
                    HirStmt::Expr(HirExpr::Manage {
                        value: Box::new(HirExpr::Ident {
                            name: "image".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(21),
                        }),
                        events: vec![manage_event.clone()],
                        type_name: Some("Image".to_string()),
                        span: span(21),
                    }),
                    HirStmt::Expr(HirExpr::Effect {
                        effect: ParamEffect::Take,
                        value: Box::new(HirExpr::Field {
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
                                is_weak: false,
                            },
                            span: span(22),
                        }),
                        events: Vec::new(),
                        type_name: Some("Rules".to_string()),
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
                        params: Vec::new(),
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
        assert_eq!(
            local_analysis.take_handle_fields(),
            &[TakeHandleField {
                name: "rules".to_string(),
                span: span(22),
            }]
        );
        state.apply_retention_events(&[retain_event]);
        state.apply_move_events(&[manage_event]);

        assert!(!state.is_clean_local("cached"));
        assert_eq!(state.move_span("image").map(|span| span.line), Some(21));
    }

    #[test]
    fn local_analysis_reports_fresh_return_issues_from_hir_flow_state() {
        let retain_event = HirEffectEvent {
            function_name: "render".to_string(),
            kind: HirEffectEventKind::Retain {
                callee: "ImageCache.store".to_string(),
                param: "image".to_string(),
            },
            binding_name: "image".to_string(),
            span: span(2),
            value_span: span(2),
        };
        let body = HirFunctionBody {
            function_name: "render".to_string(),
            block: Some(HirBlock {
                statements: vec![
                    HirStmt::Let {
                        kind: HirBindingKind::LocalLet,
                        name: "image".to_string(),
                        value: Some(HirExpr::Call {
                            callee: Callee::Name("Image.load".to_string()),
                            args: Vec::new(),
                            resolution: CallResolution::Unknown,
                            events: Vec::new(),
                            type_name: Some("Image".to_string()),
                            span: span(1),
                        }),
                        type_name: Some("Image".to_string()),
                        value_type_name: Some("Image".to_string()),
                        span: span(1),
                    },
                    HirStmt::Expr(HirExpr::Call {
                        callee: Callee::Name("ImageCache.store".to_string()),
                        args: Vec::new(),
                        resolution: CallResolution::Unknown,
                        events: vec![retain_event],
                        type_name: None,
                        span: span(2),
                    }),
                    HirStmt::Return {
                        value: Some(HirExpr::Ident {
                            name: "image".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(3),
                        }),
                        proof: HirReturnProof::Ident {
                            name: "image".to_string(),
                        },
                        span: span(3),
                    },
                ],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };

        let local_analysis = LocalAnalysis::new(Some(&body));

        assert_eq!(
            local_analysis.fresh_return_issues(),
            vec![FreshReturnIssue {
                kind: FreshReturnIssueKind::NotClean {
                    name: "image".to_string(),
                },
                span: span(3),
            }]
        );
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
                    value_type_name: None,
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
        assert_eq!(successor_ids(&steps[0]), vec![1]);
        assert_eq!(steps[1].kind, LocalFlowStepKind::Branch);
        assert_eq!(successor_ids(&steps[1]), vec![2, 3]);
        assert_eq!(successor_ids(&steps[2]), vec![4]);
        assert_eq!(successor_ids(&steps[3]), vec![4]);
        assert!(successor_ids(&steps[4]).is_empty());
    }

    #[test]
    fn local_flow_steps_record_loop_break_and_continue_edges() {
        let block = HirBlock {
            statements: vec![
                HirStmt::Loop {
                    condition: Some(HirExpr::Ident {
                        name: "keep_going".to_string(),
                        type_name: Some("Bool".to_string()),
                        span: span(1),
                    }),
                    body: HirBlock {
                        statements: vec![HirStmt::If {
                            condition: HirExpr::Ident {
                                name: "again".to_string(),
                                type_name: Some("Bool".to_string()),
                                span: span(2),
                            },
                            then_body: HirBlock {
                                statements: vec![HirStmt::Continue(span(3))],
                                span: span(3),
                            },
                            else_body: Some(HirBlock {
                                statements: vec![HirStmt::Break(span(4))],
                                span: span(4),
                            }),
                            span: span(2),
                        }],
                        span: span(2),
                    },
                    span: span(1),
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
        assert_eq!(steps[0].kind, LocalFlowStepKind::Loop);
        assert_eq!(successor_ids(&steps[0]), vec![1, 4]);
        assert_eq!(steps[1].kind, LocalFlowStepKind::Branch);
        assert_eq!(successor_ids(&steps[1]), vec![2, 3]);
        assert_eq!(steps[2].kind, LocalFlowStepKind::Continue);
        assert_eq!(successor_ids(&steps[2]), vec![0]);
        assert_eq!(steps[3].kind, LocalFlowStepKind::Break);
        assert_eq!(successor_ids(&steps[3]), vec![4]);
        assert!(successor_ids(&steps[4]).is_empty());
    }

    #[test]
    fn local_flow_steps_do_not_route_unreachable_body_exits() {
        let block = HirBlock {
            statements: vec![
                HirStmt::Loop {
                    condition: None,
                    body: HirBlock {
                        statements: vec![
                            HirStmt::Break(span(2)),
                            HirStmt::Expr(HirExpr::Ident {
                                name: "unreachable".to_string(),
                                type_name: Some("Image".to_string()),
                                span: span(3),
                            }),
                        ],
                        span: span(2),
                    },
                    span: span(1),
                },
                HirStmt::Return {
                    value: Some(HirExpr::Ident {
                        name: "done".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(4),
                    }),
                    proof: HirReturnProof::Ident {
                        name: "done".to_string(),
                    },
                    span: span(4),
                },
            ],
            span: span(1),
        };

        let steps = collect_local_flow_steps(&block);

        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0].kind, LocalFlowStepKind::Loop);
        assert_eq!(successor_ids(&steps[0]), vec![1]);
        assert_eq!(steps[1].kind, LocalFlowStepKind::Break);
        assert_eq!(successor_ids(&steps[1]), vec![3]);
        assert!(successor_ids(&steps[2]).is_empty());
        assert!(successor_ids(&steps[3]).is_empty());
    }

    #[test]
    fn local_flow_states_scope_with_resource_bindings() {
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            block: Some(HirBlock {
                statements: vec![
                    HirStmt::With {
                        resource: HirExpr::Call {
                            callee: Callee::Name("File.open".to_string()),
                            args: Vec::new(),
                            resolution: CallResolution::Unknown,
                            events: Vec::new(),
                            type_name: Some("File".to_string()),
                            span: span(1),
                        },
                        binding: "file".to_string(),
                        body: HirBlock {
                            statements: vec![HirStmt::Expr(HirExpr::Ident {
                                name: "file".to_string(),
                                type_name: Some("File".to_string()),
                                span: span(2),
                            })],
                            span: span(2),
                        },
                        span: span(1),
                    },
                    HirStmt::Return {
                        value: Some(HirExpr::Ident {
                            name: "done".to_string(),
                            type_name: Some("Unit".to_string()),
                            span: span(3),
                        }),
                        proof: HirReturnProof::Ident {
                            name: "done".to_string(),
                        },
                        span: span(3),
                    },
                ],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));
        let steps = collect_local_flow_steps(body.block.as_ref().expect("block exists"));

        assert_eq!(successor_ids(&steps[0]), vec![1]);
        assert_eq!(successor_ids(&steps[1]), vec![2]);
        assert_eq!(
            successor_drop_resources(&steps[1], 2),
            vec!["file".to_string()]
        );
        assert!(
            local_analysis
                .flow_entry_state(&span(2))
                .is_some_and(|state| state.is_resource("file"))
        );
        assert_eq!(
            local_analysis
                .flow_entry_state(&span(2))
                .and_then(|state| state.value_type("file")),
            Some("File")
        );
        assert!(
            local_analysis
                .flow_entry_state(&span(3))
                .is_some_and(|state| !state.is_resource("file"))
        );
    }

    #[test]
    fn local_analysis_indexes_resource_escape_facts_by_with_span() {
        let retain_event = HirEffectEvent {
            function_name: "run".to_string(),
            kind: HirEffectEventKind::Retain {
                callee: "register".to_string(),
                param: "file".to_string(),
            },
            binding_name: "file".to_string(),
            span: span(3),
            value_span: span(3),
        };
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            block: Some(HirBlock {
                statements: vec![HirStmt::With {
                    resource: HirExpr::Call {
                        callee: Callee::Name("File.open".to_string()),
                        args: Vec::new(),
                        resolution: CallResolution::Unknown,
                        events: Vec::new(),
                        type_name: Some("File".to_string()),
                        span: span(1),
                    },
                    binding: "file".to_string(),
                    body: HirBlock {
                        statements: vec![
                            HirStmt::Expr(HirExpr::Call {
                                callee: Callee::Name("register".to_string()),
                                args: Vec::new(),
                                resolution: CallResolution::Unknown,
                                events: vec![retain_event],
                                type_name: None,
                                span: span(3),
                            }),
                            HirStmt::Let {
                                kind: HirBindingKind::ManagedLet,
                                name: "callback".to_string(),
                                value: Some(HirExpr::Closure {
                                    params: Vec::new(),
                                    body: HirBlock {
                                        statements: vec![HirStmt::Expr(HirExpr::Ident {
                                            name: "file".to_string(),
                                            type_name: Some("File".to_string()),
                                            span: span(5),
                                        })],
                                        span: span(4),
                                    },
                                    span: span(4),
                                }),
                                type_name: None,
                                value_type_name: None,
                                span: span(4),
                            },
                        ],
                        span: span(2),
                    },
                    span: span(1),
                }],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));
        let escapes = local_analysis
            .resource_escapes(&span(1))
            .expect("with should have resource escape facts");

        assert_eq!(
            escapes,
            &[
                ResourceEscape {
                    binding: "file".to_string(),
                    kind: ResourceEscapeKind::Escape,
                    span: span(3),
                },
                ResourceEscape {
                    binding: "file".to_string(),
                    kind: ResourceEscapeKind::Capture,
                    span: span(4),
                },
            ]
        );
    }

    #[test]
    fn local_flow_entry_states_merge_clean_local_retention() {
        let retain_event = HirEffectEvent {
            function_name: "run".to_string(),
            kind: HirEffectEventKind::Retain {
                callee: "Cache.store".to_string(),
                param: "value".to_string(),
            },
            binding_name: "image".to_string(),
            span: span(3),
            value_span: span(3),
        };
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            block: Some(HirBlock {
                statements: vec![
                    HirStmt::Let {
                        kind: HirBindingKind::LocalLet,
                        name: "image".to_string(),
                        value: None,
                        type_name: Some("Image".to_string()),
                        value_type_name: None,
                        span: span(1),
                    },
                    HirStmt::If {
                        condition: HirExpr::Ident {
                            name: "should_store".to_string(),
                            type_name: Some("Bool".to_string()),
                            span: span(2),
                        },
                        then_body: HirBlock {
                            statements: vec![HirStmt::Expr(HirExpr::Call {
                                callee: Callee::Name("Cache.store".to_string()),
                                args: Vec::new(),
                                resolution: CallResolution::Unknown,
                                events: vec![retain_event],
                                type_name: None,
                                span: span(3),
                            })],
                            span: span(3),
                        },
                        else_body: None,
                        span: span(2),
                    },
                    HirStmt::Return {
                        value: Some(HirExpr::Ident {
                            name: "image".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(4),
                        }),
                        proof: HirReturnProof::Ident {
                            name: "image".to_string(),
                        },
                        span: span(4),
                    },
                ],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));

        let return_state = local_analysis
            .flow_entry_state(&span(4))
            .expect("return should be reachable");

        assert!(return_state.is_local("image"));
        assert!(!return_state.is_clean_local("image"));
    }

    #[test]
    fn local_flow_entry_states_preserve_stable_value_types() {
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            block: Some(HirBlock {
                statements: vec![
                    HirStmt::Let {
                        kind: HirBindingKind::LocalLet,
                        name: "config".to_string(),
                        value: None,
                        type_name: Some("InlineConfig".to_string()),
                        value_type_name: None,
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
                                name: "config".to_string(),
                                type_name: Some("InlineConfig".to_string()),
                                span: span(3),
                            })],
                            span: span(3),
                        },
                        else_body: None,
                        span: span(2),
                    },
                    HirStmt::Expr(HirExpr::Ident {
                        name: "config".to_string(),
                        type_name: Some("InlineConfig".to_string()),
                        span: span(4),
                    }),
                ],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));

        let expr_state = local_analysis
            .flow_entry_state(&span(4))
            .expect("expression should be reachable");

        assert_eq!(expr_state.value_type("config"), Some("InlineConfig"));
    }

    #[test]
    fn local_analysis_indexes_managed_closure_uses_by_statement_span() {
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            block: Some(HirBlock {
                statements: vec![
                    HirStmt::Let {
                        kind: HirBindingKind::LocalLet,
                        name: "image".to_string(),
                        value: None,
                        type_name: Some("Image".to_string()),
                        value_type_name: None,
                        span: span(1),
                    },
                    HirStmt::Let {
                        kind: HirBindingKind::ManagedLet,
                        name: "callback".to_string(),
                        value: Some(HirExpr::Closure {
                            params: Vec::new(),
                            body: HirBlock {
                                statements: vec![HirStmt::Expr(HirExpr::Ident {
                                    name: "image".to_string(),
                                    type_name: Some("Image".to_string()),
                                    span: span(3),
                                })],
                                span: span(2),
                            },
                            span: span(2),
                        }),
                        type_name: None,
                        value_type_name: None,
                        span: span(2),
                    },
                ],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));

        assert_eq!(
            local_analysis.managed_closure_ident_uses(&span(2)),
            Some(&[("image".to_string(), span(3))][..])
        );
    }

    #[test]
    fn local_flow_entry_states_carry_loop_break_moves() {
        let manage_event = HirEffectEvent {
            function_name: "run".to_string(),
            kind: HirEffectEventKind::Manage,
            binding_name: "image".to_string(),
            span: span(3),
            value_span: span(3),
        };
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            block: Some(HirBlock {
                statements: vec![
                    HirStmt::Let {
                        kind: HirBindingKind::LocalLet,
                        name: "image".to_string(),
                        value: None,
                        type_name: Some("Image".to_string()),
                        value_type_name: None,
                        span: span(1),
                    },
                    HirStmt::Loop {
                        condition: None,
                        body: HirBlock {
                            statements: vec![
                                HirStmt::Expr(HirExpr::Manage {
                                    value: Box::new(HirExpr::Ident {
                                        name: "image".to_string(),
                                        type_name: Some("Image".to_string()),
                                        span: span(3),
                                    }),
                                    events: vec![manage_event],
                                    type_name: Some("Image".to_string()),
                                    span: span(3),
                                }),
                                HirStmt::Break(span(4)),
                            ],
                            span: span(3),
                        },
                        span: span(2),
                    },
                    HirStmt::Return {
                        value: Some(HirExpr::Ident {
                            name: "image".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(5),
                        }),
                        proof: HirReturnProof::Ident {
                            name: "image".to_string(),
                        },
                        span: span(5),
                    },
                ],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));

        let return_state = local_analysis
            .flow_entry_state(&span(5))
            .expect("return should be reachable");

        assert_eq!(
            return_state.move_span("image").map(|span| span.line),
            Some(3)
        );
        assert!(!return_state.is_clean_local("image"));
    }

    #[test]
    fn local_analysis_reports_moved_uses_from_flow_state() {
        let manage_event = HirEffectEvent {
            function_name: "run".to_string(),
            kind: HirEffectEventKind::Manage,
            binding_name: "image".to_string(),
            span: span(2),
            value_span: span(2),
        };
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            block: Some(HirBlock {
                statements: vec![
                    HirStmt::Let {
                        kind: HirBindingKind::LocalLet,
                        name: "image".to_string(),
                        value: None,
                        type_name: Some("Image".to_string()),
                        value_type_name: None,
                        span: span(1),
                    },
                    HirStmt::Expr(HirExpr::Manage {
                        value: Box::new(HirExpr::Ident {
                            name: "image".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(2),
                        }),
                        events: vec![manage_event],
                        type_name: Some("Image".to_string()),
                        span: span(2),
                    }),
                    HirStmt::Expr(HirExpr::Ident {
                        name: "image".to_string(),
                        type_name: Some("Image".to_string()),
                        span: span(3),
                    }),
                ],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));

        assert_eq!(
            local_analysis.moved_uses(),
            vec![MovedUse {
                name: "image".to_string(),
                use_span: span(3),
                move_span: span(2),
            }]
        );
    }

    #[test]
    fn local_analysis_reports_managed_to_local_uses_from_flow_state() {
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            block: Some(HirBlock {
                statements: vec![
                    HirStmt::Let {
                        kind: HirBindingKind::ManagedLet,
                        name: "image".to_string(),
                        value: None,
                        type_name: Some("Image".to_string()),
                        value_type_name: None,
                        span: span(1),
                    },
                    HirStmt::Let {
                        kind: HirBindingKind::LocalLet,
                        name: "working".to_string(),
                        value: Some(HirExpr::Ident {
                            name: "image".to_string(),
                            type_name: Some("Image".to_string()),
                            span: span(2),
                        }),
                        type_name: Some("Image".to_string()),
                        value_type_name: Some("Image".to_string()),
                        span: span(2),
                    },
                ],
                span: span(1),
            }),
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));

        assert_eq!(
            local_analysis.managed_to_local_uses(),
            vec![ManagedToLocalUse {
                local_name: "working".to_string(),
                managed_name: "image".to_string(),
                span: span(2),
            }]
        );
    }
}
