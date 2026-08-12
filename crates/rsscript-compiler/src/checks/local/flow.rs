//! Compatibility adapters for the semantic local-flow graph during S02.

use super::*;

pub(super) fn collect_local_flow_steps(block: &HirBlock) -> Vec<LocalFlowStep> {
    rsscript_semantics::local_flow_graph(block)
}

pub(super) fn hir_stmt_span(statement: &HirStmt) -> &Span {
    rsscript_semantics::local_flow_statement_span(statement)
}

pub(crate) fn merge_if_state(
    state: &mut BodyState,
    base: &BodyState,
    then_state: BodyState,
    then_flow: Flow,
    else_branch: Option<(BodyState, Flow)>,
) -> Flow {
    rsscript_semantics::merge_local_if_state(state, base, then_state, then_flow, else_branch)
}

pub(crate) fn merge_loop_state(
    state: &mut BodyState,
    base: &BodyState,
    body_state: BodyState,
    body_flow: Flow,
    may_skip: bool,
) -> Flow {
    rsscript_semantics::merge_local_loop_state(state, base, body_state, body_flow, may_skip)
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
