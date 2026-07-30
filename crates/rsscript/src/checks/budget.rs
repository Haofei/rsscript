use std::cell::{Cell, RefCell};
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::diagnostic::{Diagnostic, Span, code};
use crate::semantic::{FrontendCompletion, FrontendStopReason};

#[derive(Debug, Clone, Copy)]
pub(crate) struct FrontendBudgetLimits {
    pub(crate) source_bytes: usize,
    pub(crate) tokens: usize,
    pub(crate) parse_depth: usize,
    pub(crate) ast_nodes: usize,
    pub(crate) nodes: usize,
    pub(crate) substitutions: usize,
    pub(crate) diagnostics: usize,
    pub(crate) semantic_recursion: usize,
}

impl Default for FrontendBudgetLimits {
    fn default() -> Self {
        Self {
            source_bytes: 16 * 1024 * 1024,
            tokens: 1_000_000,
            parse_depth: 256,
            ast_nodes: 2_000_000,
            nodes: 10_000_000,
            substitutions: 1_000_000,
            diagnostics: 10_000,
            semantic_recursion: 256,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExhaustedBudget {
    SourceBytes,
    Tokens,
    ParseDepth,
    AstNodes,
    Nodes,
    Substitutions,
    Diagnostics,
    SemanticRecursion,
    Cancelled,
}

impl ExhaustedBudget {
    fn name(self) -> &'static str {
        match self {
            Self::SourceBytes => "source bytes",
            Self::Tokens => "tokens",
            Self::ParseDepth => "parse depth",
            Self::AstNodes => "AST nodes",
            Self::Nodes => "nodes",
            Self::Substitutions => "substitutions",
            Self::Diagnostics => "diagnostics",
            Self::SemanticRecursion => "semantic recursion",
            Self::Cancelled => "cancellation",
        }
    }
}

#[derive(Debug)]
pub(crate) struct FrontendBudget {
    limits: FrontendBudgetLimits,
    source_bytes: Cell<usize>,
    tokens: Cell<usize>,
    parse_depth: Cell<usize>,
    ast_nodes: Cell<usize>,
    nodes: Cell<usize>,
    substitutions: Cell<usize>,
    diagnostics: Cell<usize>,
    semantic_recursion_depth: Cell<usize>,
    exhausted: Cell<Option<ExhaustedBudget>>,
    span: RefCell<Span>,
    cancel: Option<Arc<AtomicBool>>,
}

impl FrontendBudget {
    pub(crate) fn new(limits: FrontendBudgetLimits, span: Span) -> Rc<Self> {
        Self::with_cancellation(limits, span, None)
    }

    pub(crate) fn with_cancellation(
        limits: FrontendBudgetLimits,
        span: Span,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Rc<Self> {
        Rc::new(Self {
            limits,
            source_bytes: Cell::new(0),
            tokens: Cell::new(0),
            parse_depth: Cell::new(0),
            ast_nodes: Cell::new(0),
            nodes: Cell::new(0),
            substitutions: Cell::new(0),
            diagnostics: Cell::new(0),
            semantic_recursion_depth: Cell::new(0),
            exhausted: Cell::new(None),
            span: RefCell::new(span),
            cancel,
        })
    }

    pub(crate) fn consume_source_bytes(&self, amount: usize) -> bool {
        self.consume(
            &self.source_bytes,
            self.limits.source_bytes,
            amount,
            ExhaustedBudget::SourceBytes,
        )
    }

    pub(crate) fn consume_tokens(&self, amount: usize) -> bool {
        self.consume(
            &self.tokens,
            self.limits.tokens,
            amount,
            ExhaustedBudget::Tokens,
        )
    }

    pub(crate) fn enter_parse(self: &Rc<Self>) -> Option<ParseRecursionGuard> {
        if !self.check_active() {
            return None;
        }
        let depth = self.parse_depth.get();
        if depth >= self.limits.parse_depth {
            self.exhaust(ExhaustedBudget::ParseDepth);
            return None;
        }
        if !self.consume(
            &self.ast_nodes,
            self.limits.ast_nodes,
            1,
            ExhaustedBudget::AstNodes,
        ) {
            return None;
        }
        self.parse_depth.set(depth + 1);
        Some(ParseRecursionGuard {
            budget: self.clone(),
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
        if !self.check_active() {
            return false;
        }
        if depth > self.limits.semantic_recursion {
            self.exhaust(ExhaustedBudget::SemanticRecursion);
            return false;
        }
        true
    }

    pub(crate) fn enter_recursion(self: &Rc<Self>) -> Option<AnalysisRecursionGuard> {
        if !self.check_active() {
            return None;
        }
        let depth = self.semantic_recursion_depth.get();
        if depth >= self.limits.semantic_recursion {
            self.exhaust(ExhaustedBudget::SemanticRecursion);
            return None;
        }
        self.semantic_recursion_depth.set(depth + 1);
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
        if !self.check_active() {
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

    pub(crate) fn check_active(&self) -> bool {
        if self.exhausted.get().is_some() {
            return false;
        }
        if self
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
        {
            self.exhaust(ExhaustedBudget::Cancelled);
            return false;
        }
        true
    }

    fn exhaust(&self, exhausted_budget: ExhaustedBudget) {
        if self.exhausted.get().is_none() {
            self.exhausted.set(Some(exhausted_budget));
        }
    }

    pub(crate) fn is_exhausted(&self) -> bool {
        !self.check_active()
    }

    pub(crate) fn incomplete_diagnostic(&self) -> Option<Diagnostic> {
        let exhausted = self.exhausted.get()?;
        Some(
            Diagnostic::error(
                code::ANALYSIS_INCOMPLETE,
                "Frontend analysis stopped before completion.",
                self.span.borrow().clone(),
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

    pub(crate) fn completion(&self) -> FrontendCompletion {
        match self.exhausted.get() {
            None => FrontendCompletion::Complete,
            Some(ExhaustedBudget::SourceBytes) => {
                FrontendCompletion::Incomplete(FrontendStopReason::SourceBytes)
            }
            Some(ExhaustedBudget::Tokens) => {
                FrontendCompletion::Incomplete(FrontendStopReason::Tokens)
            }
            Some(ExhaustedBudget::ParseDepth) => {
                FrontendCompletion::Incomplete(FrontendStopReason::ParseDepth)
            }
            Some(ExhaustedBudget::AstNodes) => {
                FrontendCompletion::Incomplete(FrontendStopReason::AstNodes)
            }
            Some(ExhaustedBudget::Nodes) => {
                FrontendCompletion::Incomplete(FrontendStopReason::SemanticNodes)
            }
            Some(ExhaustedBudget::Substitutions) => {
                FrontendCompletion::Incomplete(FrontendStopReason::Substitutions)
            }
            Some(ExhaustedBudget::Diagnostics) => {
                FrontendCompletion::Incomplete(FrontendStopReason::Diagnostics)
            }
            Some(ExhaustedBudget::SemanticRecursion) => {
                FrontendCompletion::Incomplete(FrontendStopReason::SemanticRecursion)
            }
            Some(ExhaustedBudget::Cancelled) => {
                FrontendCompletion::Incomplete(FrontendStopReason::Cancelled)
            }
        }
    }
}

pub(crate) struct ParseRecursionGuard {
    budget: Rc<FrontendBudget>,
}

impl Drop for ParseRecursionGuard {
    fn drop(&mut self) {
        self.budget
            .parse_depth
            .set(self.budget.parse_depth.get().saturating_sub(1));
    }
}

pub(crate) struct AnalysisRecursionGuard {
    budget: Rc<FrontendBudget>,
}

impl Drop for AnalysisRecursionGuard {
    fn drop(&mut self) {
        self.budget
            .semantic_recursion_depth
            .set(self.budget.semantic_recursion_depth.get().saturating_sub(1));
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
        if let Some(diagnostic) = self.budget.incomplete_diagnostic() {
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
