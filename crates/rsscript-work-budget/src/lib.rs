#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub use rsscript_source_model::Span;

#[derive(Debug, Clone, Copy)]
pub struct FrontendBudgetLimits {
    pub source_bytes: usize,
    pub tokens: usize,
    pub parse_depth: usize,
    pub ast_nodes: usize,
    pub nodes: usize,
    pub substitutions: usize,
    pub diagnostics: usize,
    pub semantic_recursion: usize,
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
pub enum BudgetExhaustion {
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

impl BudgetExhaustion {
    pub fn name(self) -> &'static str {
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
pub struct FrontendBudget {
    limits: FrontendBudgetLimits,
    source_bytes: Cell<usize>,
    tokens: Cell<usize>,
    parse_depth: Cell<usize>,
    ast_nodes: Cell<usize>,
    nodes: Cell<usize>,
    substitutions: Cell<usize>,
    diagnostics: Cell<usize>,
    semantic_recursion_depth: Cell<usize>,
    exhausted: Cell<Option<BudgetExhaustion>>,
    span: RefCell<Span>,
    cancel: Option<Arc<AtomicBool>>,
}

impl FrontendBudget {
    pub fn new(limits: FrontendBudgetLimits, span: Span) -> Rc<Self> {
        Self::with_cancellation(limits, span, None)
    }

    pub fn with_cancellation(
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

    pub fn consume_source_bytes(&self, amount: usize) -> bool {
        self.consume(
            &self.source_bytes,
            self.limits.source_bytes,
            amount,
            BudgetExhaustion::SourceBytes,
        )
    }

    pub fn consume_tokens(&self, amount: usize) -> bool {
        self.consume(
            &self.tokens,
            self.limits.tokens,
            amount,
            BudgetExhaustion::Tokens,
        )
    }

    pub fn enter_parse(self: &Rc<Self>) -> Option<ParseRecursionGuard> {
        if !self.check_active() {
            return None;
        }
        let depth = self.parse_depth.get();
        if depth >= self.limits.parse_depth {
            self.exhaust(BudgetExhaustion::ParseDepth);
            return None;
        }
        if !self.consume(
            &self.ast_nodes,
            self.limits.ast_nodes,
            1,
            BudgetExhaustion::AstNodes,
        ) {
            return None;
        }
        self.parse_depth.set(depth + 1);
        Some(ParseRecursionGuard {
            budget: self.clone(),
        })
    }

    pub fn consume_nodes(&self, amount: usize) -> bool {
        self.consume(
            &self.nodes,
            self.limits.nodes,
            amount,
            BudgetExhaustion::Nodes,
        )
    }

    pub fn consume_substitution(&self) -> bool {
        self.consume(
            &self.substitutions,
            self.limits.substitutions,
            1,
            BudgetExhaustion::Substitutions,
        )
    }

    pub fn consume_diagnostic(&self) -> bool {
        self.consume(
            &self.diagnostics,
            self.limits.diagnostics,
            1,
            BudgetExhaustion::Diagnostics,
        )
    }

    pub fn check_recursion(&self, depth: usize) -> bool {
        if !self.check_active() {
            return false;
        }
        if depth > self.limits.semantic_recursion {
            self.exhaust(BudgetExhaustion::SemanticRecursion);
            return false;
        }
        true
    }

    pub fn enter_recursion(self: &Rc<Self>) -> Option<AnalysisRecursionGuard> {
        if !self.check_active() {
            return None;
        }
        let depth = self.semantic_recursion_depth.get();
        if depth >= self.limits.semantic_recursion {
            self.exhaust(BudgetExhaustion::SemanticRecursion);
            return None;
        }
        self.semantic_recursion_depth.set(depth + 1);
        Some(AnalysisRecursionGuard {
            budget: self.clone(),
        })
    }

    pub fn check_active(&self) -> bool {
        if self.exhausted.get().is_some() {
            return false;
        }
        if self
            .cancel
            .as_ref()
            .is_some_and(|cancel| cancel.load(Ordering::Relaxed))
        {
            self.exhaust(BudgetExhaustion::Cancelled);
            return false;
        }
        true
    }

    pub fn is_exhausted(&self) -> bool {
        !self.check_active()
    }

    pub fn exhaustion(&self) -> Option<BudgetExhaustion> {
        self.exhausted.get()
    }

    pub fn span(&self) -> Span {
        self.span.borrow().clone()
    }

    fn consume(
        &self,
        used: &Cell<usize>,
        limit: usize,
        amount: usize,
        exhausted: BudgetExhaustion,
    ) -> bool {
        if !self.check_active() {
            return false;
        }
        let Some(next) = used.get().checked_add(amount) else {
            self.exhaust(exhausted);
            return false;
        };
        if next > limit {
            self.exhaust(exhausted);
            return false;
        }
        used.set(next);
        true
    }

    fn exhaust(&self, exhausted: BudgetExhaustion) {
        if self.exhausted.get().is_none() {
            self.exhausted.set(Some(exhausted));
        }
    }
}

pub struct ParseRecursionGuard {
    budget: Rc<FrontendBudget>,
}

impl Drop for ParseRecursionGuard {
    fn drop(&mut self) {
        self.budget
            .parse_depth
            .set(self.budget.parse_depth.get().saturating_sub(1));
    }
}

pub struct AnalysisRecursionGuard {
    budget: Rc<FrontendBudget>,
}

impl Drop for AnalysisRecursionGuard {
    fn drop(&mut self) {
        self.budget
            .semantic_recursion_depth
            .set(self.budget.semantic_recursion_depth.get().saturating_sub(1));
    }
}
