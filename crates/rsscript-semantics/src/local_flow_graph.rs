//! Neutral local control-flow graph model for ownership analysis.
//!
//! This is intentionally an owned, checked-HIR-derived model. Graph
//! construction and the data-flow solver migrate independently so consumers do
//! not need to recreate the semantic shape of bindings, effects, or cleanup
//! edges.

use crate::hir::{HirBindingKind, HirEffectEvent};
use rsscript_syntax::Span;

/// The semantic role a local-flow graph step plays in structured control flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalFlowStepKind {
    Statement,
    Branch,
    Loop,
    Return,
    Break,
    Continue,
}

/// One ownership-analysis node derived from checked HIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFlowStep {
    pub id: usize,
    pub span: Span,
    pub kind: LocalFlowStepKind,
    pub uses: Vec<(String, Span)>,
    pub managed_closure_captures: Vec<String>,
    pub binding: Option<LocalFlowBinding>,
    pub resource_binding: Option<LocalFlowResourceBinding>,
    pub events: Vec<HirEffectEvent>,
    pub successors: Vec<LocalFlowEdge>,
}

/// A value binding introduced by a local-flow step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFlowBinding {
    pub name: String,
    pub kind: HirBindingKind,
    pub type_name: Option<String>,
    pub value_ident: Option<(String, Span)>,
    pub value_handle_field: Option<(String, Span)>,
    pub fresh_from_local_source: Option<String>,
    pub fresh_from_scrutinee: bool,
    pub fresh_from_fresh_value: bool,
}

/// A resource binding whose cleanup is represented by outgoing graph edges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFlowResourceBinding {
    pub name: String,
    pub type_name: Option<String>,
}

/// A control-flow edge, including resources released while traversing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFlowEdge {
    pub to: usize,
    pub drop_resources: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_edge_preserves_cleanup_metadata() {
        let edge = LocalFlowEdge {
            to: 3,
            drop_resources: vec!["file".to_owned()],
        };
        assert_eq!(edge.to, 3);
        assert_eq!(edge.drop_resources, ["file"]);
    }
}
