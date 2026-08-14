//! Frontend work-budget completion and diagnostic collection.
//!
//! These are frontend semantic-operation concerns rather than compiler
//! composition concerns: syntax and semantic checks share one budget, and the
//! resulting incomplete-analysis evidence must be identical for CLI, editor,
//! and embedding callers.

use std::ops::Deref;
use std::rc::Rc;

use rsscript_diagnostics::{Diagnostic, code};

use crate::{FrontendCompletion, FrontendStopReason};

pub use rsscript_work_budget::{BudgetExhaustion, FrontendBudget, FrontendBudgetLimits};

/// Derive the one stable diagnostic emitted when an operation's frontend work
/// budget ends before semantic checking completes.
pub fn incomplete_diagnostic(budget: &FrontendBudget) -> Option<Diagnostic> {
    let exhausted = budget.exhaustion()?;
    Some(
        Diagnostic::error(
            code::ANALYSIS_INCOMPLETE,
            "Frontend analysis stopped before completion.",
            budget.span(),
            "frontend work budget exhausted",
        )
        .with_cause(format!(
            "The shared `{}` frontend budget was exhausted.",
            exhausted.name()
        ))
        .with_fix(
            "reduce_analysis_complexity",
            "Reduce generated breadth or deeply nested generic/type expressions.",
            "manual",
        ),
    )
}

/// Convert an exhausted frontend budget into the immutable completion fact
/// stored beside the semantic database.
pub fn budget_completion(budget: &FrontendBudget) -> FrontendCompletion {
    match budget.exhaustion() {
        None => FrontendCompletion::Complete,
        Some(BudgetExhaustion::SourceBytes) => {
            FrontendCompletion::Incomplete(FrontendStopReason::SourceBytes)
        }
        Some(BudgetExhaustion::Tokens) => {
            FrontendCompletion::Incomplete(FrontendStopReason::Tokens)
        }
        Some(BudgetExhaustion::ParseDepth) => {
            FrontendCompletion::Incomplete(FrontendStopReason::ParseDepth)
        }
        Some(BudgetExhaustion::AstNodes) => {
            FrontendCompletion::Incomplete(FrontendStopReason::AstNodes)
        }
        Some(BudgetExhaustion::Nodes) => {
            FrontendCompletion::Incomplete(FrontendStopReason::SemanticNodes)
        }
        Some(BudgetExhaustion::Substitutions) => {
            FrontendCompletion::Incomplete(FrontendStopReason::Substitutions)
        }
        Some(BudgetExhaustion::Diagnostics) => {
            FrontendCompletion::Incomplete(FrontendStopReason::Diagnostics)
        }
        Some(BudgetExhaustion::SemanticRecursion) => {
            FrontendCompletion::Incomplete(FrontendStopReason::SemanticRecursion)
        }
        Some(BudgetExhaustion::Cancelled) => {
            FrontendCompletion::Incomplete(FrontendStopReason::Cancelled)
        }
        Some(BudgetExhaustion::DeadlineExceeded) => {
            FrontendCompletion::Incomplete(FrontendStopReason::DeadlineExceeded)
        }
    }
}

/// A budget-aware semantic diagnostic sink.
///
/// It ensures a bounded operation records only the diagnostics that fit its
/// shared budget plus the single terminal incomplete-analysis fact.
pub struct AnalysisDiagnostics {
    values: Vec<Diagnostic>,
    budget: Rc<FrontendBudget>,
}

impl AnalysisDiagnostics {
    pub fn new(budget: Rc<FrontendBudget>) -> Self {
        Self {
            values: Vec::new(),
            budget,
        }
    }

    pub fn push(&mut self, diagnostic: Diagnostic) {
        if self.budget.consume_diagnostic() {
            self.values.push(diagnostic);
        }
    }

    pub fn extend<I>(&mut self, diagnostics: I)
    where
        I: IntoIterator<Item = Diagnostic>,
    {
        for diagnostic in diagnostics {
            self.push(diagnostic);
            if self.budget.is_exhausted() {
                break;
            }
        }
    }

    pub fn push_incomplete(&mut self) {
        if let Some(diagnostic) = incomplete_diagnostic(&self.budget) {
            self.values.push(diagnostic);
        }
    }

    pub fn into_vec(self) -> Vec<Diagnostic> {
        self.values
    }

    pub fn as_mut_slice(&mut self) -> &mut [Diagnostic] {
        &mut self.values
    }
}

impl Deref for AnalysisDiagnostics {
    type Target = Vec<Diagnostic>;

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsscript_source_model::Span;

    #[test]
    fn exhausted_diagnostic_budget_has_one_terminal_completion_fact() {
        let budget = FrontendBudget::new(
            FrontendBudgetLimits {
                diagnostics: 1,
                ..FrontendBudgetLimits::default()
            },
            Span::default(),
        );
        let mut diagnostics = AnalysisDiagnostics::new(Rc::clone(&budget));
        diagnostics.push(Diagnostic::error(
            "E_TEST",
            "first",
            Span::default(),
            "first",
        ));
        diagnostics.push(Diagnostic::error(
            "E_TEST",
            "second",
            Span::default(),
            "second",
        ));
        diagnostics.push_incomplete();

        let diagnostics = diagnostics.into_vec();
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[1].code, code::ANALYSIS_INCOMPLETE);
        assert_eq!(
            budget_completion(&budget),
            FrontendCompletion::Incomplete(FrontendStopReason::Diagnostics)
        );
    }
}
