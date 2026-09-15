//! Backend-neutral facts produced by local ownership and resource-flow analysis.

use rsscript_syntax::Span;

/// Which move took a value out of its binding.
///
/// `manage` and `take` are the only two moves the language has (§5.3), and
/// they are not interchangeable in a diagnostic: `manage x` hands the value to
/// the managed runtime, while `take x` hands it to a callee. `RS0401` names
/// the one that actually happened, so the kind travels with the fact instead
/// of being guessed at rendering time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveKind {
    /// `manage x`, including `let shared = manage x`.
    Manage,
    /// `take x`, whether at a call site or as a `match take x` scrutinee.
    Take,
}

impl MoveKind {
    /// The move as it is written in source, for a diagnostic that quotes it.
    pub fn expression(self, name: &str) -> String {
        match self {
            Self::Manage => format!("manage {name}"),
            Self::Take => format!("take {name}"),
        }
    }
}

/// Where and how a binding was moved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MoveSite {
    pub kind: MoveKind,
    pub span: Span,
}

impl MoveSite {
    pub fn new(kind: MoveKind, span: Span) -> Self {
        Self { kind, span }
    }

    pub fn manage(span: Span) -> Self {
        Self::new(MoveKind::Manage, span)
    }

    pub fn take(span: Span) -> Self {
        Self::new(MoveKind::Take, span)
    }
}

/// A use of a local after a move, paired with the source move that invalidated
/// it. The CFG engine may change without changing this semantic fact contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedUse {
    pub name: String,
    pub use_span: Span,
    pub move_site: MoveSite,
}

/// A `local` binding initialized from managed identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedToLocalUse {
    pub local_name: String,
    pub managed_name: String,
    pub span: Span,
}

/// A local value retained by a call contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedLocalUse {
    pub name: String,
    pub callee: String,
    pub param: String,
    pub span: Span,
}

/// A local captured by a closure passed to a retaining call contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetainedClosureCapture {
    pub name: String,
    pub callee: String,
    pub param: String,
    pub capture_span: Span,
    pub closure_span: Span,
}

/// A `take` operation on a handle field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TakeHandleField {
    pub name: String,
    pub span: Span,
}

/// Why a `fresh` return cannot be established from local-flow facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreshReturnIssueKind {
    NotClean { name: String },
    UnknownIdent { name: String },
    Unknown,
}

/// A failed `fresh` return proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshReturnIssue {
    pub kind: FreshReturnIssueKind,
    pub span: Span,
}

/// How a resource leaves its lexical `with` scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceEscapeKind {
    Escape,
    Capture,
}

/// A resource escape/capture fact, before diagnostics are derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEscape {
    pub binding: String,
    pub kind: ResourceEscapeKind,
    pub span: Span,
}
