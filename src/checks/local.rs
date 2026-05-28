use std::collections::{HashMap, HashSet};

use crate::diagnostic::Span;
use crate::hir::{HirBinding, HirBindingKind, HirEffectEvent, HirEffectEventKind, HirFunctionBody};

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
}

impl LocalAnalysis {
    pub(crate) fn new(body: Option<&HirFunctionBody>) -> Self {
        Self {
            body: body.cloned(),
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

    fn effect_events(&self, span: &Span) -> &[HirEffectEvent] {
        self.body
            .as_ref()
            .and_then(|body| events_at_span(&body.effect_events, span))
            .unwrap_or(&[])
    }
}

fn events_at_span<'a>(events: &'a [HirEffectEvent], span: &Span) -> Option<&'a [HirEffectEvent]> {
    let start = events.iter().position(|event| event.span == *span)?;
    let end = events[start..]
        .iter()
        .position(|event| event.span != *span)
        .map_or(events.len(), |offset| start + offset);
    Some(&events[start..end])
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
        let body = HirFunctionBody {
            function_name: "run".to_string(),
            bindings: vec![HirBinding {
                function_name: "run".to_string(),
                name: "pool".to_string(),
                kind: HirBindingKind::Param,
                span: span(1),
                type_name: Some("ResourcePool<File>".to_string()),
            }],
            effect_events: vec![
                HirEffectEvent {
                    function_name: "run".to_string(),
                    kind: HirEffectEventKind::Retain {
                        callee: "Cache.store".to_string(),
                        param: "value".to_string(),
                    },
                    binding_name: "cached".to_string(),
                    span: span(20),
                    value_span: span(20),
                },
                HirEffectEvent {
                    function_name: "run".to_string(),
                    kind: HirEffectEventKind::Manage,
                    binding_name: "image".to_string(),
                    span: span(21),
                    value_span: span(21),
                },
            ],
            ..HirFunctionBody::default()
        };
        let local_analysis = LocalAnalysis::new(Some(&body));
        let mut state = local_analysis.initial_state();
        state.bind_local("cached");
        state.bind_local("image");

        assert_eq!(state.value_type("pool"), Some("ResourcePool<File>"));

        local_analysis.apply_retention_events(&span(20), &mut state);
        local_analysis.apply_move_events(&span(21), &mut state);

        assert!(!state.is_clean_local("cached"));
        assert_eq!(state.move_span("image").map(|span| span.line), Some(21));
    }
}
