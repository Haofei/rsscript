//! Compatibility exports for generation APIs now owned by semantics.

pub use rsscript_semantics::{
    Completeness, Completion, CompletionKind, ContinuationOptions, Continuations, Effect,
    ExpectedType, GenerateContext, GenerationCheckpoint, GenerationCoreInterfacePolicy,
    GenerationInterfaceSetSnapshot, GenerationInterfaceSnapshot, GenerationQueryIdentity,
    GenerationQuerySnapshot, GenerationRestoreError, GenerationSession, GenerationSessionStats,
    IdentifierRoleName, LiteralKindName, ParserTerminal, PrefixStatus, SemanticValidity, TextRange,
    TypeRef, valid_continuations,
};
