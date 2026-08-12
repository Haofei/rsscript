use std::collections::HashMap;

use crate::diagnostic::Span;
#[cfg(test)]
use crate::hir::HirEffectEvent;
#[cfg(test)]
use crate::hir::HirReturnProof;
use crate::hir::{HirBindingKind, HirBlock, HirEffectEventKind, HirFunctionBody};

use super::body::Flow;

mod flow;

pub(crate) use flow::*;

pub(crate) use rsscript_semantics::LocalFlowState as BodyState;
pub(crate) use rsscript_semantics::LocalFlowStep;
#[cfg(test)]
pub(crate) use rsscript_semantics::LocalFlowStepKind;

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
        let flow_entry_states_by_span = rsscript_semantics::local_flow_entry_states(
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
        self.body
            .and_then(|body| body.block.as_ref())
            .map(|block| {
                rsscript_semantics::moved_uses_from_flow(block, &self.flow_entry_states_by_span)
            })
            .unwrap_or_default()
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
        self.body
            .and_then(|body| body.block.as_ref())
            .map(|block| {
                rsscript_semantics::retained_closure_captures_from_flow(
                    block,
                    &self.flow_entry_states_by_span,
                )
            })
            .unwrap_or_default()
    }

    pub(crate) fn fresh_return_issues(&self) -> Vec<FreshReturnIssue> {
        self.body
            .and_then(|body| body.block.as_ref())
            .map(|block| {
                rsscript_semantics::fresh_return_issues_from_flow(
                    block,
                    &self.flow_entry_states_by_span,
                )
            })
            .unwrap_or_default()
    }
}
