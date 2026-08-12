//! Backend-neutral facts produced by local ownership and resource-flow analysis.

use rsscript_syntax::Span;

/// A use of a local after a move, paired with the source move that invalidated
/// it. The CFG engine may change without changing this semantic fact contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedUse {
    pub name: String,
    pub use_span: Span,
    pub move_span: Span,
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
