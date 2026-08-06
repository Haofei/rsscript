use std::ops::Deref;
use std::rc::Rc;

use crate::diagnostic::{Diagnostic, code};
use crate::semantic::{FrontendCompletion, FrontendStopReason};

pub(crate) use rsscript_work_budget::{BudgetExhaustion, FrontendBudget, FrontendBudgetLimits};

pub(crate) fn incomplete_diagnostic(budget: &FrontendBudget) -> Option<Diagnostic> {
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

pub(crate) fn budget_completion(budget: &FrontendBudget) -> FrontendCompletion {
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

pub(crate) struct AnalysisDiagnostics {
    values: Vec<Diagnostic>,
    budget: Rc<FrontendBudget>,
}

impl AnalysisDiagnostics {
    pub(crate) fn new(budget: Rc<FrontendBudget>) -> Self {
        Self {
            values: Vec::new(),
            budget,
        }
    }

    pub(crate) fn push(&mut self, diagnostic: Diagnostic) {
        if self.budget.consume_diagnostic() {
            self.values.push(diagnostic);
        }
    }

    pub(crate) fn extend<I>(&mut self, diagnostics: I)
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

    pub(crate) fn push_incomplete(&mut self) {
        if let Some(diagnostic) = incomplete_diagnostic(&self.budget) {
            self.values.push(diagnostic);
        }
    }

    pub(crate) fn into_vec(self) -> Vec<Diagnostic> {
        self.values
    }

    pub(crate) fn as_mut_slice(&mut self) -> &mut [Diagnostic] {
        &mut self.values
    }
}

impl Deref for AnalysisDiagnostics {
    type Target = Vec<Diagnostic>;

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

pub(crate) type AnalysisBudget = FrontendBudget;
#[cfg(test)]
pub(crate) type AnalysisBudgetLimits = FrontendBudgetLimits;
