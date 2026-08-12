//! State lattice for local ownership and structured flow analysis.

use crate::hir::{HirBinding, HirBindingKind, HirEffectEvent, HirEffectEventKind, ParamEffect};
use crate::is_copy_type_name;
use rsscript_syntax::Span;
use std::collections::{HashMap, HashSet};

/// Flow-sensitive local ownership state. Fields remain public during the
/// compiler migration because legacy checks perform explicit lattice joins;
/// ownership of the model and all state-transition operations is semantic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalFlowState {
    pub locals: HashSet<String>,
    pub field_splittable_locals: HashSet<String>,
    pub clean_locals: HashSet<String>,
    pub fresh_returnable_locals: HashSet<String>,
    pub managed: HashSet<String>,
    pub read_views: HashSet<String>,
    pub resources: HashSet<String>,
    pub moved: HashMap<String, Span>,
    pub moved_paths: HashMap<String, Span>,
    pub value_types: HashMap<String, String>,
}

impl LocalFlowState {
    /// Seed a function entry state from checked parameter bindings.
    pub fn seed_params(&mut self, bindings: &[HirBinding]) {
        for binding in bindings {
            if binding.kind != HirBindingKind::Param {
                continue;
            }
            if let Some(ty) = &binding.ty {
                self.record_type(binding.name.clone(), ty.to_string());
            }
            if matches!(binding.effect, Some(ParamEffect::Read))
                && binding
                    .ty
                    .as_ref()
                    .is_none_or(|ty| !is_copy_type_name(&ty.to_string()))
            {
                self.bind_managed(binding.name.clone());
            }
            if matches!(binding.effect, Some(ParamEffect::Mut | ParamEffect::Take)) {
                self.bind_param_local(
                    binding.name.clone(),
                    binding.effect == Some(ParamEffect::Take),
                );
            }
        }
    }

    pub fn mark_fresh_returnable(&mut self, name: impl Into<String>) {
        let name = name.into();
        self.clean_locals.insert(name.clone());
        self.fresh_returnable_locals.insert(name);
    }

    pub fn bind_managed(&mut self, name: impl Into<String>) {
        self.managed.insert(name.into());
    }
    pub fn bind_read_view(&mut self, name: impl Into<String>) {
        self.read_views.insert(name.into());
    }
    pub fn bind_resource(&mut self, name: impl Into<String>) {
        self.resources.insert(name.into());
    }
    pub fn drop_resource(&mut self, name: &str) {
        self.resources.remove(name);
    }

    pub fn bind_local(&mut self, name: impl Into<String>) {
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

    pub fn record_type(&mut self, name: impl Into<String>, type_name: impl Into<String>) {
        self.value_types.insert(name.into(), type_name.into());
    }

    pub fn mark_moved(&mut self, name: &str, span: Span) {
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

    pub fn mark_retained(&mut self, name: &str) {
        self.clean_locals.remove(name);
        self.fresh_returnable_locals.remove(name);
    }

    pub fn is_local(&self, name: &str) -> bool {
        self.locals.contains(name)
    }
    pub fn allows_field_split(&self, name: &str) -> bool {
        self.field_splittable_locals.contains(name)
    }
    pub fn is_managed(&self, name: &str) -> bool {
        self.managed.contains(name)
    }
    pub fn is_read_view(&self, name: &str) -> bool {
        self.read_views.contains(name)
    }
    pub fn is_resource(&self, name: &str) -> bool {
        self.resources.contains(name)
    }
    pub fn is_clean_local(&self, name: &str) -> bool {
        self.clean_locals.contains(name)
    }
    pub fn is_fresh_returnable_local(&self, name: &str) -> bool {
        self.fresh_returnable_locals.contains(name)
    }
    pub fn move_span(&self, name: &str) -> Option<&Span> {
        self.moved.get(name)
    }

    pub fn moved_path_span(&self, path: &str) -> Option<(String, &Span)> {
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

    pub fn moved_subpath_span(&self, root: &str) -> Option<(String, &Span)> {
        self.moved_paths
            .iter()
            .find(|(path, _)| path_root(path).is_some_and(|path_root| path_root == root))
            .map(|(path, span)| (path.clone(), span))
    }

    pub fn value_type(&self, name: &str) -> Option<&str> {
        self.value_types.get(name).map(String::as_str)
    }

    pub fn apply_move_events(&mut self, events: &[HirEffectEvent]) {
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

    pub fn apply_retention_events(&mut self, events: &[HirEffectEvent]) {
        for event in events {
            if matches!(event.kind, HirEffectEventKind::Retain { .. })
                && (self.locals.contains(&event.binding_name)
                    || self.fresh_returnable_locals.contains(&event.binding_name))
            {
                self.mark_retained(&event.binding_name);
            }
        }
    }
}

/// Return the root segment of a resolved local place path.
pub fn path_root(path: &str) -> Option<&str> {
    path.split('.').next().filter(|root| !root.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::{HirBinding, HirBindingKind, ParamEffect};

    fn span() -> Span {
        Span {
            file: "flow.rss".to_owned(),
            line: 1,
            column: 1,
            length: 1,
        }
    }

    #[test]
    fn parameter_seeding_and_move_transition_preserve_ownership_contract() {
        let mut state = LocalFlowState::default();
        state.seed_params(&[HirBinding {
            function_name: "main".to_owned(),
            name: "item".to_owned(),
            kind: HirBindingKind::Param,
            effect: Some(ParamEffect::Take),
            span: span(),
            ty: None,
            type_name: Some("Item".to_owned()),
        }]);
        assert!(state.is_local("item"));
        state.mark_moved("item.part", span());
        assert!(!state.is_clean_local("item"));
        assert_eq!(path_root("item.part"), Some("item"));
    }
}
