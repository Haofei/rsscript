//! Frontend budget aliases used by the migrated semantic checker.
//!
//! Frontend completion and diagnostic-budget behavior now belongs to
//! `rsscript-semantics`; compiler checks retain these aliases only while their
//! implementation is incrementally moved behind semantic query entry points.

#[cfg(test)]
pub(crate) use rsscript_semantics::incomplete_diagnostic;
pub(crate) use rsscript_semantics::{
    AnalysisDiagnostics, FrontendBudget, FrontendBudgetLimits, budget_completion,
};

pub(crate) type AnalysisBudget = FrontendBudget;
#[cfg(test)]
pub(crate) type AnalysisBudgetLimits = FrontendBudgetLimits;
