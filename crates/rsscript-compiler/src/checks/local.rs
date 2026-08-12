use crate::text_util::strip_fresh_type;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::checks::shared::hir_expr_span;
use crate::diagnostic::Span;
use crate::hir::{
    CallResolution, HirBindingKind, HirBlock, HirEffectEvent, HirEffectEventKind, HirExpr,
    HirFunctionBody, HirReturnProof, HirStmt,
};
use crate::syntax::ast::{Callee, Expr};

use super::body::Flow;

mod flow;
mod ownership;

pub(crate) use flow::*;
use ownership::*;

pub(crate) use rsscript_semantics::LocalFlowState as BodyState;

pub(crate) use rsscript_semantics::{
    FreshReturnIssue, FreshReturnIssueKind, ManagedToLocalUse, MovedUse, ResourceEscape,
    ResourceEscapeKind, RetainedClosureCapture, RetainedLocalUse, TakeHandleField,
};

pub(crate) struct LocalAnalysis<'a> {
    body: Option<&'a HirFunctionBody>,
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
    fresh_from_local_source: Option<String>,
    fresh_from_scrutinee: bool,
    /// The binding's initializer is itself a fresh value (a fresh-returning call,
    /// a struct/variant constructor, or a literal). Such a binding holds a fresh,
    /// unaliased value, so returning it directly preserves freshness — until it is
    /// moved, retained, or captured (which clears its fresh-returnable status).
    fresh_from_fresh_value: bool,
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

impl<'a> LocalAnalysis<'a> {
    pub(crate) fn new(body: Option<&'a HirFunctionBody>) -> Self {
        let managed_closure_uses_by_span = body.and_then(|body| body.block.as_ref()).map_or_else(
            HashMap::new,
            rsscript_semantics::managed_closure_uses_by_statement,
        );
        let resource_escapes_by_with_span = body.and_then(|body| body.block.as_ref()).map_or_else(
            HashMap::new,
            rsscript_semantics::resource_escapes_by_with_statement,
        );
        let take_handle_fields = body
            .and_then(|body| body.block.as_ref())
            .map_or_else(Vec::new, rsscript_semantics::take_handle_fields);
        let flow_steps = body
            .and_then(|body| body.block.as_ref())
            .map_or_else(Vec::new, collect_local_flow_steps);
        let flow_entry_states_by_span = collect_flow_entry_states(
            &flow_steps,
            rsscript_semantics::initial_local_flow_state(body),
        );

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
        rsscript_semantics::initial_local_flow_state(self.body)
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
        if let Some(block) = self.body.and_then(|body| body.block.as_ref()) {
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
        if let Some(block) = self.body.and_then(|body| body.block.as_ref()) {
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
        if let Some(block) = self.body.and_then(|body| body.block.as_ref()) {
            collect_fresh_return_issues_from_block(
                block,
                &self.flow_entry_states_by_span,
                &mut issues,
            );
        }
        issues
    }
}
