//! Parser- and semantics-owned source generation.
//!
//! This adapter composes the syntax prefix oracle with typed semantic
//! completion. It deliberately lives in `rsscript-semantics`: no compiler,
//! lowering, VM, or provider capability is required to answer a query.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rsscript_syntax::{
    ExpectedTerminal, IdentifierRole, LiteralKind, PrefixParseState, TerminalCompleteness,
    parse_source_prefix,
};

use crate::{
    SemanticCompletion, SemanticCompletionCompleteness, SemanticCompletionCorePolicy,
    SemanticCompletionKind, SemanticCompletionValidity, semantic_completion_with_interface_sources,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ContinuationOptions {
    pub max_names: usize,
}

impl Default for ContinuationOptions {
    fn default() -> Self {
        Self { max_names: 50 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    Complete,
    Partial,
}

/// Syntax prefix status captured by the same parser invocation as a
/// continuation response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PrefixStatus {
    Complete,
    Incomplete,
    Dead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticValidity {
    Valid,
    Invalid,
    Partial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationCoreInterfacePolicy {
    WithCore,
    WithoutCore,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ParserTerminal {
    Fixed {
        text: String,
        completeness: Completeness,
    },
    Identifier {
        role: IdentifierRoleName,
        completeness: Completeness,
    },
    Literal {
        literal: LiteralKindName,
        completeness: Completeness,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentifierRoleName {
    ItemName,
    FunctionName,
    ParameterName,
    TypeName,
    Expression,
    FieldName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LiteralKindName {
    Number,
    String,
    Char,
    InterpolatedString,
    MultilineString,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Completion {
    pub text: String,
    pub insert_text: String,
    pub replace: TextRange,
    pub kind: CompletionKind,
    pub signature: Option<String>,
    pub result_type: Option<TypeRef>,
    pub required_effect: Option<Effect>,
    pub completeness: Completeness,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ExpectedType {
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TypeRef {
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Read,
    Mut,
    Take,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    Local,
    Param,
    Type,
    Function,
    Method,
    ArgName,
    Variant,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GenerationInterfaceSnapshot {
    pub path: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GenerationInterfaceSetSnapshot {
    pub revision: u64,
    pub interfaces: Vec<GenerationInterfaceSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GenerationQuerySnapshot {
    pub file: String,
    pub source: String,
    pub revision: u64,
    pub interfaces: GenerationInterfaceSetSnapshot,
    /// Stable identity for the immutable input used by one query.  This is
    /// deliberately data-only so editor clients can log or serialize it
    /// without retaining session internals.
    pub identity: GenerationQueryIdentity,
}

/// Identity of a generation query's exact semantic input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GenerationQueryIdentity {
    pub session_id: u64,
    pub revision: u64,
    pub interface_revision: u64,
    pub source_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct GenerationCheckpoint {
    session_id: u64,
    snapshot: GenerationQuerySnapshot,
    core_interface_policy: GenerationCoreInterfacePolicy,
}

impl GenerationCheckpoint {
    pub fn snapshot(&self) -> &GenerationQuerySnapshot {
        &self.snapshot
    }
}

/// A checkpoint belongs to a single mutable generation session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationRestoreError {
    DifferentSession,
}

impl std::fmt::Display for GenerationRestoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DifferentSession => {
                formatter.write_str("checkpoint belongs to a different generation session")
            }
        }
    }
}

impl std::error::Error for GenerationRestoreError {}

/// Observable cache counters for focused editor integration tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct GenerationSessionStats {
    /// Number of complete prefix+semantic analyses performed by this session.
    pub semantic_analyses: u64,
    /// Number of queries answered by a cached full fact set or projection.
    pub cache_hits: u64,
    /// Number of `max_names` projections built from cached or fresh facts.
    pub projections: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Continuations {
    /// Exact immutable input identity used for this parser+semantic response.
    /// Consumers must not combine candidates with a different source revision.
    pub identity: GenerationQueryIdentity,
    /// Syntax status from the prefix parse used to build this response.
    pub status: PrefixStatus,
    /// Whole-token replacement range from that same prefix parse.
    pub replace: TextRange,
    pub terminals: Vec<ParserTerminal>,
    pub names: Vec<Completion>,
    pub expected_type: Option<ExpectedType>,
    pub current_terminal_completeness: Completeness,
    pub terminal_completeness: Completeness,
    pub name_completeness: Completeness,
    pub semantic_validity: SemanticValidity,
    pub total_discovered_names: usize,
    pub truncated: bool,
    pub may_stop: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QueryKey {
    identity: GenerationQueryIdentity,
    core_interface_policy: GenerationCoreInterfacePolicy,
}

#[derive(Debug, Clone)]
struct CachedFacts {
    key: QueryKey,
    facts: Arc<GenerationFacts>,
}

#[derive(Debug, Clone)]
struct CachedProjection {
    key: QueryKey,
    max_names: usize,
    response: Arc<Continuations>,
}

#[derive(Debug, Clone)]
struct GenerationFacts {
    identity: GenerationQueryIdentity,
    prefix: rsscript_syntax::PrefixParseResult,
    semantic: crate::SemanticCompletionResult,
}

/// Mutable editor source with a bounded, revisioned query cache.
///
/// Changed input is fully reparsed and rechecked; this is deliberately not an
/// incremental parser. Repeating the active revision/options returns the same
/// response allocation.
#[derive(Debug)]
pub struct GenerationSession {
    session_id: u64,
    file: String,
    source: String,
    interfaces: BTreeMap<String, String>,
    revision: u64,
    interface_revision: u64,
    core_interface_policy: GenerationCoreInterfacePolicy,
    facts_cache: Option<CachedFacts>,
    projection_cache: Option<CachedProjection>,
    stats: GenerationSessionStats,
}

impl Clone for GenerationSession {
    fn clone(&self) -> Self {
        // A clone can diverge immediately, so it deliberately receives a new
        // identity and cannot accept a checkpoint from its origin session.
        Self {
            session_id: next_session_id(),
            file: self.file.clone(),
            source: self.source.clone(),
            interfaces: self.interfaces.clone(),
            revision: self.revision,
            interface_revision: self.interface_revision,
            core_interface_policy: self.core_interface_policy,
            facts_cache: None,
            projection_cache: None,
            stats: GenerationSessionStats::default(),
        }
    }
}

fn next_session_id() -> u64 {
    static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
}

impl GenerationSession {
    pub fn new(file: impl Into<String>) -> Self {
        Self::with_source(file, "")
    }

    pub fn with_source(file: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            session_id: next_session_id(),
            file: file.into(),
            source: source.into(),
            interfaces: BTreeMap::new(),
            revision: 0,
            interface_revision: 0,
            core_interface_policy: GenerationCoreInterfacePolicy::WithCore,
            facts_cache: None,
            projection_cache: None,
            stats: GenerationSessionStats::default(),
        }
    }

    pub fn file(&self) -> &str {
        &self.file
    }
    pub fn source(&self) -> &str {
        &self.source
    }
    pub const fn core_interface_policy(&self) -> GenerationCoreInterfacePolicy {
        self.core_interface_policy
    }
    pub const fn session_id(&self) -> u64 {
        self.session_id
    }
    pub const fn stats(&self) -> GenerationSessionStats {
        self.stats
    }

    pub fn set_core_interface_policy(&mut self, policy: GenerationCoreInterfacePolicy) -> bool {
        if self.core_interface_policy == policy {
            return false;
        }
        self.core_interface_policy = policy;
        self.invalidate();
        true
    }

    pub fn append(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.source.push_str(text);
        self.invalidate();
    }

    pub fn checkpoint(&self) -> GenerationCheckpoint {
        GenerationCheckpoint {
            session_id: self.session_id,
            snapshot: self.query_snapshot(),
            core_interface_policy: self.core_interface_policy,
        }
    }

    pub fn restore(
        &mut self,
        checkpoint: &GenerationCheckpoint,
    ) -> Result<(), GenerationRestoreError> {
        if checkpoint.session_id != self.session_id {
            return Err(GenerationRestoreError::DifferentSession);
        }
        let snapshot = checkpoint.snapshot();
        self.file.clone_from(&snapshot.file);
        self.source.clone_from(&snapshot.source);
        self.interfaces = snapshot
            .interfaces
            .interfaces
            .iter()
            .map(|interface| (interface.path.clone(), interface.source.clone()))
            .collect();
        // Restoring content creates a new revision. Rewinding counters would
        // let a divergent edit reuse the identity of an earlier query.
        self.interface_revision = self.interface_revision.saturating_add(1);
        self.core_interface_policy = checkpoint.core_interface_policy;
        self.invalidate();
        Ok(())
    }

    pub fn set_interface(&mut self, path: impl Into<String>, source: impl Into<String>) -> bool {
        let path = path.into();
        let source = source.into();
        if self.interfaces.get(&path) == Some(&source) {
            return false;
        }
        self.interfaces.insert(path, source);
        self.interface_revision = self.interface_revision.saturating_add(1);
        self.invalidate();
        true
    }

    pub fn remove_interface(&mut self, path: &str) -> bool {
        if self.interfaces.remove(path).is_none() {
            return false;
        }
        self.interface_revision = self.interface_revision.saturating_add(1);
        self.invalidate();
        true
    }

    pub fn interface_snapshot(&self) -> GenerationInterfaceSetSnapshot {
        GenerationInterfaceSetSnapshot {
            revision: self.interface_revision,
            interfaces: self
                .interfaces
                .iter()
                .map(|(path, source)| GenerationInterfaceSnapshot {
                    path: path.clone(),
                    source: source.clone(),
                })
                .collect(),
        }
    }

    pub fn query_snapshot(&self) -> GenerationQuerySnapshot {
        GenerationQuerySnapshot {
            file: self.file.clone(),
            source: self.source.clone(),
            revision: self.revision,
            interfaces: self.interface_snapshot(),
            identity: self.query_identity(),
        }
    }

    pub fn query_identity(&self) -> GenerationQueryIdentity {
        GenerationQueryIdentity {
            session_id: self.session_id,
            revision: self.revision,
            interface_revision: self.interface_revision,
            source_bytes: u64::try_from(self.source.len()).unwrap_or(u64::MAX),
        }
    }

    pub fn query(&mut self, options: ContinuationOptions) -> Arc<Continuations> {
        let key = QueryKey {
            identity: self.query_identity(),
            core_interface_policy: self.core_interface_policy,
        };
        if let Some(cached) = self
            .projection_cache
            .as_ref()
            .filter(|cached| cached.key == key && cached.max_names == options.max_names)
        {
            self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
            return Arc::clone(&cached.response);
        }
        let facts =
            if let Some(cached) = self.facts_cache.as_ref().filter(|cached| cached.key == key) {
                self.stats.cache_hits = self.stats.cache_hits.saturating_add(1);
                Arc::clone(&cached.facts)
            } else {
                // `query` deliberately borrows the active session input. The
                // explicit `query_snapshot` API is the only query path that
                // clones source/interface text for an external owner.
                let facts = Arc::new(generate_facts(
                    self.query_identity(),
                    &self.file,
                    &self.source,
                    &self.interfaces,
                    self.core_interface_policy,
                ));
                self.stats.semantic_analyses = self.stats.semantic_analyses.saturating_add(1);
                self.facts_cache = Some(CachedFacts {
                    key,
                    facts: Arc::clone(&facts),
                });
                facts
            };
        let response = Arc::new(project_facts(&facts, options));
        self.stats.projections = self.stats.projections.saturating_add(1);
        self.projection_cache = Some(CachedProjection {
            key,
            max_names: options.max_names,
            response: Arc::clone(&response),
        });
        response
    }

    fn invalidate(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.clear_cache();
    }

    fn clear_cache(&mut self) {
        self.facts_cache = None;
        self.projection_cache = None;
    }
}

fn generate_facts(
    identity: GenerationQueryIdentity,
    file: &str,
    source: &str,
    interfaces: &BTreeMap<String, String>,
    core_policy: GenerationCoreInterfacePolicy,
) -> GenerationFacts {
    let prefix = parse_source_prefix(file, source);
    let interfaces = interfaces
        .iter()
        .map(|(path, source)| (path.as_str(), source.as_str()))
        .collect::<Vec<_>>();
    let semantic = semantic_completion_with_interface_sources(
        file,
        source,
        &interfaces,
        &prefix,
        semantic_core_policy(core_policy),
    );
    GenerationFacts {
        identity,
        prefix,
        semantic,
    }
}

fn project_facts(facts: &GenerationFacts, options: ContinuationOptions) -> Continuations {
    let prefix = &facts.prefix;
    let semantic = &facts.semantic;
    let total_discovered_names = semantic.candidates.len();
    let truncated = total_discovered_names > options.max_names;
    let names = semantic
        .candidates
        .iter()
        .take(options.max_names)
        .map(|candidate| completion_from_semantic(candidate, &prefix.replace_range))
        .collect();
    Continuations {
        identity: facts.identity,
        status: prefix_status(prefix.state),
        replace: TextRange {
            start: prefix.replace_range.start,
            end: prefix.replace_range.end,
        },
        terminals: prefix
            .expected_terminals
            .iter()
            .map(parser_terminal_from_syntax)
            .collect(),
        names,
        expected_type: semantic.expected_type.as_ref().map(|ty| ExpectedType {
            display: ty.to_string(),
        }),
        current_terminal_completeness: terminal_completeness(prefix.current_terminal_completeness),
        terminal_completeness: terminal_completeness(prefix.expected_terminals_completeness),
        name_completeness: semantic_completeness(semantic.completeness),
        semantic_validity: semantic_validity(semantic.validity),
        total_discovered_names,
        truncated,
        may_stop: prefix.state == PrefixParseState::Complete
            && semantic.validity == SemanticCompletionValidity::Valid,
    }
}

fn parser_terminal_from_syntax(terminal: &ExpectedTerminal) -> ParserTerminal {
    match terminal {
        ExpectedTerminal::Fixed { text, completeness } => ParserTerminal::Fixed {
            text: (*text).to_string(),
            completeness: terminal_completeness(*completeness),
        },
        ExpectedTerminal::Identifier { role, completeness } => ParserTerminal::Identifier {
            role: identifier_role(*role),
            completeness: terminal_completeness(*completeness),
        },
        ExpectedTerminal::Literal { kind, completeness } => ParserTerminal::Literal {
            literal: literal_kind(*kind),
            completeness: terminal_completeness(*completeness),
        },
    }
}

fn completion_from_semantic(
    candidate: &SemanticCompletion,
    replace: &std::ops::Range<usize>,
) -> Completion {
    let kind = completion_kind(candidate.kind);
    let insert_text = match (kind, candidate.required_effect) {
        (CompletionKind::ArgName, Some(crate::hir::ParamEffect::Read) | None) => {
            format!("{}: ", candidate.name)
        }
        (CompletionKind::ArgName, Some(effect)) => {
            format!("{}: {} ", candidate.name, effect.as_str())
        }
        _ => candidate.insert_text.clone(),
    };
    Completion {
        text: candidate.name.clone(),
        insert_text,
        replace: TextRange {
            start: replace.start,
            end: replace.end,
        },
        kind,
        signature: candidate.signature.as_ref().map(format_signature),
        result_type: candidate.ty.as_ref().map(|ty| TypeRef {
            display: ty.to_string(),
        }),
        required_effect: matches!(kind, CompletionKind::ArgName | CompletionKind::Method)
            .then(|| candidate.required_effect.map(effect))
            .flatten(),
        completeness: semantic_completeness(candidate.completeness),
    }
}

fn format_signature(signature: &crate::hir::FunctionSig) -> String {
    let name = signature
        .namespace
        .as_ref()
        .map(|namespace| format!("{namespace}.{}", signature.name))
        .unwrap_or_else(|| signature.name.clone());
    let parameters = signature
        .params
        .iter()
        .map(|parameter| match parameter.effect {
            Some(effect) => format!("{}: {} {}", parameter.name, effect.as_str(), parameter.ty),
            None => format!("{}: {}", parameter.name, parameter.ty),
        })
        .collect::<Vec<_>>()
        .join(", ");
    signature
        .return_ty
        .as_ref()
        .map(|ty| format!("{name}({parameters}) -> {ty}"))
        .unwrap_or_else(|| format!("{name}({parameters})"))
}

fn terminal_completeness(value: TerminalCompleteness) -> Completeness {
    match value {
        TerminalCompleteness::Complete => Completeness::Complete,
        TerminalCompleteness::Partial => Completeness::Partial,
    }
}
fn prefix_status(value: PrefixParseState) -> PrefixStatus {
    match value {
        PrefixParseState::Complete => PrefixStatus::Complete,
        PrefixParseState::Incomplete => PrefixStatus::Incomplete,
        PrefixParseState::Dead => PrefixStatus::Dead,
    }
}
fn semantic_completeness(value: SemanticCompletionCompleteness) -> Completeness {
    match value {
        SemanticCompletionCompleteness::Complete => Completeness::Complete,
        SemanticCompletionCompleteness::Partial => Completeness::Partial,
    }
}
fn semantic_validity(value: SemanticCompletionValidity) -> SemanticValidity {
    match value {
        SemanticCompletionValidity::Valid => SemanticValidity::Valid,
        SemanticCompletionValidity::Invalid => SemanticValidity::Invalid,
        SemanticCompletionValidity::Partial => SemanticValidity::Partial,
    }
}
fn semantic_core_policy(value: GenerationCoreInterfacePolicy) -> SemanticCompletionCorePolicy {
    match value {
        GenerationCoreInterfacePolicy::WithCore => SemanticCompletionCorePolicy::WithCore,
        GenerationCoreInterfacePolicy::WithoutCore => SemanticCompletionCorePolicy::WithoutCore,
    }
}
fn effect(value: crate::hir::ParamEffect) -> Effect {
    match value {
        crate::hir::ParamEffect::Read => Effect::Read,
        crate::hir::ParamEffect::Mut => Effect::Mut,
        crate::hir::ParamEffect::Take => Effect::Take,
    }
}
fn identifier_role(value: IdentifierRole) -> IdentifierRoleName {
    match value {
        IdentifierRole::ItemName => IdentifierRoleName::ItemName,
        IdentifierRole::FunctionName => IdentifierRoleName::FunctionName,
        IdentifierRole::ParameterName => IdentifierRoleName::ParameterName,
        IdentifierRole::TypeName => IdentifierRoleName::TypeName,
        IdentifierRole::Expression => IdentifierRoleName::Expression,
        IdentifierRole::FieldName => IdentifierRoleName::FieldName,
    }
}
fn literal_kind(value: LiteralKind) -> LiteralKindName {
    match value {
        LiteralKind::Number => LiteralKindName::Number,
        LiteralKind::String => LiteralKindName::String,
        LiteralKind::Char => LiteralKindName::Char,
        LiteralKind::InterpolatedString => LiteralKindName::InterpolatedString,
        LiteralKind::MultilineString => LiteralKindName::MultilineString,
    }
}
fn completion_kind(value: SemanticCompletionKind) -> CompletionKind {
    match value {
        SemanticCompletionKind::Local => CompletionKind::Local,
        SemanticCompletionKind::Param => CompletionKind::Param,
        SemanticCompletionKind::Function => CompletionKind::Function,
        SemanticCompletionKind::Method => CompletionKind::Method,
        SemanticCompletionKind::Type => CompletionKind::Type,
        SemanticCompletionKind::ArgumentName => CompletionKind::ArgName,
        SemanticCompletionKind::Variant => CompletionKind::Variant,
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GenerateContext<'a> {
    pub file: &'a str,
    pub partial_source: &'a str,
}

pub fn valid_continuations(
    context: &GenerateContext<'_>,
    options: ContinuationOptions,
) -> Continuations {
    let mut session = GenerationSession::with_source(context.file, context.partial_source);
    (*session.query(options)).clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_serializes_prefix_status_and_replace_from_its_query() {
        let source = "fn main() -> Unit {\n";
        let mut session = GenerationSession::with_source("query.rss", source);
        let response = session.query(ContinuationOptions::default());

        // This independent parse is only a test oracle. Production consumers
        // receive these fields from `response`, not from another syntax query.
        let prefix = parse_source_prefix("query.rss", source);
        assert_eq!(response.status, prefix_status(prefix.state));
        assert_eq!(response.replace.start, prefix.replace_range.start);
        assert_eq!(response.replace.end, prefix.replace_range.end);

        let json = serde_json::to_value(&*response).expect("response serializes");
        assert_eq!(json["status"], "incomplete");
        assert_eq!(
            json["replace"]["start"],
            serde_json::json!(prefix.replace_range.start)
        );
        assert_eq!(
            json["replace"]["end"],
            serde_json::json!(prefix.replace_range.end)
        );
    }
}
