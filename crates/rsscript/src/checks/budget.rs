use std::cell::{Cell, RefCell};
use std::ops::{Deref, DerefMut};
use std::rc::Rc;

use crate::diagnostic::{Diagnostic, Span, code};

#[derive(Debug, Clone, Copy)]
pub(crate) struct AnalysisBudgetLimits {
    pub(crate) nodes: usize,
    pub(crate) substitutions: usize,
    pub(crate) diagnostics: usize,
    pub(crate) recursion: usize,
}

impl Default for AnalysisBudgetLimits {
    fn default() -> Self {
        Self {
            nodes: 10_000_000,
            substitutions: 1_000_000,
            diagnostics: 10_000,
            recursion: 256,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExhaustedBudget {
    Nodes,
    Substitutions,
    Diagnostics,
    Recursion,
}

impl ExhaustedBudget {
    fn name(self) -> &'static str {
        match self {
            Self::Nodes => "nodes",
            Self::Substitutions => "substitutions",
            Self::Diagnostics => "diagnostics",
            Self::Recursion => "recursion",
        }
    }
}

#[derive(Debug)]
pub(crate) struct AnalysisBudget {
    limits: AnalysisBudgetLimits,
    nodes: Cell<usize>,
    substitutions: Cell<usize>,
    diagnostics: Cell<usize>,
    recursion_depth: Cell<usize>,
    exhausted: Cell<Option<ExhaustedBudget>>,
    span: RefCell<Span>,
}

impl AnalysisBudget {
    pub(crate) fn new(limits: AnalysisBudgetLimits, span: Span) -> Rc<Self> {
        Rc::new(Self {
            limits,
            nodes: Cell::new(0),
            substitutions: Cell::new(0),
            diagnostics: Cell::new(0),
            recursion_depth: Cell::new(0),
            exhausted: Cell::new(None),
            span: RefCell::new(span),
        })
    }

    pub(crate) fn consume_nodes(&self, amount: usize) -> bool {
        self.consume(
            &self.nodes,
            self.limits.nodes,
            amount,
            ExhaustedBudget::Nodes,
        )
    }

    pub(crate) fn consume_substitution(&self) -> bool {
        self.consume(
            &self.substitutions,
            self.limits.substitutions,
            1,
            ExhaustedBudget::Substitutions,
        )
    }

    pub(crate) fn check_recursion(&self, depth: usize) -> bool {
        if self.exhausted.get().is_some() {
            return false;
        }
        if depth > self.limits.recursion {
            self.exhaust(ExhaustedBudget::Recursion);
            return false;
        }
        true
    }

    pub(crate) fn enter_recursion(self: &Rc<Self>) -> Option<AnalysisRecursionGuard> {
        if self.exhausted.get().is_some() {
            return None;
        }
        let depth = self.recursion_depth.get();
        if depth >= self.limits.recursion {
            self.exhaust(ExhaustedBudget::Recursion);
            return None;
        }
        self.recursion_depth.set(depth + 1);
        Some(AnalysisRecursionGuard {
            budget: self.clone(),
        })
    }

    fn consume_diagnostic(&self) -> bool {
        self.consume(
            &self.diagnostics,
            self.limits.diagnostics,
            1,
            ExhaustedBudget::Diagnostics,
        )
    }

    fn consume(
        &self,
        used: &Cell<usize>,
        limit: usize,
        amount: usize,
        exhausted_budget: ExhaustedBudget,
    ) -> bool {
        if self.exhausted.get().is_some() {
            return false;
        }
        let current = used.get();
        let Some(next) = current.checked_add(amount) else {
            self.exhaust(exhausted_budget);
            return false;
        };
        if next > limit {
            self.exhaust(exhausted_budget);
            return false;
        }
        used.set(next);
        true
    }

    fn exhaust(&self, exhausted_budget: ExhaustedBudget) {
        if self.exhausted.get().is_none() {
            self.exhausted.set(Some(exhausted_budget));
        }
    }

    pub(crate) fn is_exhausted(&self) -> bool {
        self.exhausted.get().is_some()
    }

    pub(crate) fn incomplete_diagnostic(&self) -> Option<Diagnostic> {
        let exhausted = self.exhausted.get()?;
        Some(
            Diagnostic::error(
                code::ANALYSIS_INCOMPLETE,
                "Semantic analysis stopped before completion.",
                self.span.borrow().clone(),
                "analysis work budget exhausted",
            )
            .with_cause(format!(
                "The shared `{}` analysis budget was exhausted.",
                exhausted.name()
            ))
            .with_fix(
                "reduce_analysis_complexity",
                "Reduce generated breadth or deeply nested generic/type expressions.",
                "manual",
            ),
        )
    }
}

pub(crate) struct AnalysisRecursionGuard {
    budget: Rc<AnalysisBudget>,
}

impl Drop for AnalysisRecursionGuard {
    fn drop(&mut self) {
        self.budget
            .recursion_depth
            .set(self.budget.recursion_depth.get().saturating_sub(1));
    }
}

pub(crate) struct AnalysisDiagnostics {
    values: Vec<Diagnostic>,
    budget: Rc<AnalysisBudget>,
}

impl AnalysisDiagnostics {
    pub(crate) fn new(budget: Rc<AnalysisBudget>) -> Self {
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
        if let Some(diagnostic) = self.budget.incomplete_diagnostic() {
            self.values.push(diagnostic);
        }
    }

    pub(crate) fn into_vec(self) -> Vec<Diagnostic> {
        self.values
    }
}

impl Deref for AnalysisDiagnostics {
    type Target = Vec<Diagnostic>;

    fn deref(&self) -> &Self::Target {
        &self.values
    }
}

impl DerefMut for AnalysisDiagnostics {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.values
    }
}
