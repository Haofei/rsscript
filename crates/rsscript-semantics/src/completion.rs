//! Typed, scope-aware completion facts.
//!
//! Syntax owns recovery and the shape of the cursor.  This module deliberately
//! does not try to recover a second parser: given a syntax prefix and a cursor
//! offset it projects already-checked HIR signatures/types into editor-ready
//! candidates.  When the body around the cursor cannot be proven, it returns a
//! partial result instead of manufacturing receiver or ownership facts.

use std::collections::BTreeSet;

use rsscript_syntax::ast::{Block, Callee, FunctionDecl, Item, Program};
use rsscript_syntax::{
    CursorContext, PrefixParseResult, PrefixParseState, Span, SyntaxSite, parse_source_raw,
};

use crate::hir::{FunctionSig, Hir, ParamEffect};
use crate::{
    AnalysisResult, FrontendCompletion, ResolvedType, analyze_source_with_interfaces_result,
    analyze_source_with_interfaces_without_core_result,
};

mod candidates;
mod expected;
mod scope;
#[cfg(test)]
mod tests;

use candidates::{deduplicate_shadowed, retain_matching, split_top_level, unmatched_open_paren};
use expected::{
    after_keyword_on_line, assignment_target_before_cursor, bool_condition_position,
    match_pattern_expected_type, typed_let_rhs,
};
use scope::{ScopeBinding, ScopeFacts, local_availability_before_cursor, scope_at};

/// Kind of semantic completion candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticCompletionKind {
    Local,
    Param,
    Function,
    Method,
    Type,
    ArgumentName,
    Variant,
}

/// How much of a semantic completion result was proved from the prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SemanticCompletionCompleteness {
    Complete,
    Partial,
}

/// Validity of the full semantic analysis used to project completion facts.
///
/// `Valid` means the recovered source completed its semantic frontend without
/// error diagnostics. `Invalid` carries the conservative answer whenever a
/// complete semantic check found an error; `Partial` means the frontend budget
/// ended before validity could be proved.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SemanticCompletionValidity {
    Valid,
    Invalid,
    Partial,
}

/// Whether completion analysis injects the language's Core interfaces.
///
/// This deliberately has only the two choices available to completion
/// sessions. Standard-package prelude behavior is a distinct analyzer mode,
/// not an accidental third interpretation of `--no-core`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticCompletionCorePolicy {
    WithCore,
    WithoutCore,
}

/// A semantic candidate with ownership and type facts preserved separately
/// from display text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCompletion {
    pub name: String,
    pub insert_text: String,
    pub kind: SemanticCompletionKind,
    pub ty: Option<ResolvedType>,
    pub signature: Option<FunctionSig>,
    pub required_effect: Option<ParamEffect>,
    /// Number of lexical blocks between this declaration and file scope.
    pub scope_depth: usize,
    pub completeness: SemanticCompletionCompleteness,
}

/// Semantic facts for a syntax-owned cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticCompletionResult {
    pub candidates: Vec<SemanticCompletion>,
    pub expected_type: Option<ResolvedType>,
    pub completeness: SemanticCompletionCompleteness,
    /// Full semantic validity from the same recovered input as the HIR facts.
    pub validity: SemanticCompletionValidity,
}

impl SemanticCompletionResult {
    fn empty(completeness: SemanticCompletionCompleteness) -> Self {
        Self {
            candidates: Vec::new(),
            expected_type: None,
            completeness,
            validity: SemanticCompletionValidity::Partial,
        }
    }
}

/// Complete a source prefix using its syntax result.
///
/// The cursor is the end of `prefix.replace_range`, which is exactly the
/// replaceable terminal owned by [`PrefixParseResult`].  The syntax-layer
/// `CursorContext` adapter calls this primitive after it has classified the
/// cursor site; keeping this function byte-offset based makes the semantic
/// environment independently useful to incremental clients too.
pub fn semantic_completion(
    file: &str,
    source: &str,
    prefix: &PrefixParseResult,
) -> SemanticCompletionResult {
    semantic_completion_with_context(file, source, prefix, &prefix.cursor)
}

/// Complete with syntax's cursor classification. An `Unknown` site never
/// unlocks call-specific facts, so receiver/variant completions are not guessed.
pub fn semantic_completion_with_context(
    file: &str,
    source: &str,
    prefix: &PrefixParseResult,
    cursor_context: &CursorContext,
) -> SemanticCompletionResult {
    semantic_completion_at_context(
        file,
        source,
        prefix,
        cursor_context.byte_offset,
        cursor_context,
    )
}

/// Like [`semantic_completion`], with an explicit UTF-8 cursor boundary.
///
/// This is useful for clients which retain the complete document rather than a
/// source prefix.  Out-of-bounds and non-boundary offsets are treated as a
/// partial request, never rounded into a different token.
pub fn semantic_completion_at(
    file: &str,
    source: &str,
    prefix: &PrefixParseResult,
    cursor: usize,
) -> SemanticCompletionResult {
    if cursor == prefix.cursor.byte_offset {
        semantic_completion_at_context(file, source, prefix, cursor, &prefix.cursor)
    } else {
        let unknown = CursorContext {
            byte_offset: cursor,
            site: SyntaxSite::Unknown,
            function: None,
            call: None,
        };
        semantic_completion_at_context(file, source, prefix, cursor, &unknown)
    }
}

fn semantic_completion_at_context(
    file: &str,
    source: &str,
    prefix: &PrefixParseResult,
    cursor: usize,
    cursor_context: &CursorContext,
) -> SemanticCompletionResult {
    if prefix.state == PrefixParseState::Dead
        || !prefix.matches_source(source)
        || cursor > source.len()
        || !source.is_char_boundary(cursor)
    {
        return SemanticCompletionResult::empty(SemanticCompletionCompleteness::Partial);
    }

    semantic_completion_from_sources(
        file,
        source,
        &[],
        prefix,
        cursor,
        cursor_context,
        SemanticCompletionCorePolicy::WithCore,
    )
}

/// Completion equivalent which adds checked interface signatures to the HIR.
/// Interface declarations are not reparsed by this function; callers can share
/// their workspace/interface parse cache through this entrypoint.
pub fn semantic_completion_with_interfaces(
    file: &str,
    source: &str,
    interfaces: &[Program],
    prefix: &PrefixParseResult,
) -> SemanticCompletionResult {
    let cursor = prefix.cursor.byte_offset;
    if prefix.state == PrefixParseState::Dead
        || !prefix.matches_source(source)
        || cursor > source.len()
        || !source.is_char_boundary(cursor)
    {
        return SemanticCompletionResult::empty(SemanticCompletionCompleteness::Partial);
    }
    let recovered = recovered_semantic_source(source, prefix);
    let program = parse_source_raw(file, &recovered);
    let hir = Hir::from_syntax_with_interfaces(&program, interfaces);
    // This legacy API accepts already-parsed interfaces, so their source
    // diagnostics cannot be checked as one immutable input. Keep completion
    // facts compatible, but do not claim stop-validity from an incomplete
    // input boundary. New editor integrations use the source-based API below.
    complete_program(
        source,
        prefix,
        cursor,
        &prefix.cursor,
        &program,
        &hir,
        SemanticCompletionValidity::Partial,
    )
}

/// Complete using source-backed interfaces and an explicit Core policy.
///
/// The recovery string is constructed once and passed to exactly one complete
/// semantic analysis. The resulting checked Program/HIR and its validity are
/// then projected together, preventing completion and `may_stop` from seeing
/// different semantic worlds.
pub fn semantic_completion_with_interface_sources(
    file: &str,
    source: &str,
    interfaces: &[(&str, &str)],
    prefix: &PrefixParseResult,
    core_policy: SemanticCompletionCorePolicy,
) -> SemanticCompletionResult {
    let cursor = prefix.cursor.byte_offset;
    if prefix.state == PrefixParseState::Dead
        || !prefix.matches_source(source)
        || cursor > source.len()
        || !source.is_char_boundary(cursor)
    {
        return SemanticCompletionResult::empty(SemanticCompletionCompleteness::Partial);
    }
    semantic_completion_from_sources(
        file,
        source,
        interfaces,
        prefix,
        cursor,
        &prefix.cursor,
        core_policy,
    )
}

fn semantic_completion_from_sources(
    file: &str,
    source: &str,
    interfaces: &[(&str, &str)],
    prefix: &PrefixParseResult,
    cursor: usize,
    cursor_context: &CursorContext,
    core_policy: SemanticCompletionCorePolicy,
) -> SemanticCompletionResult {
    let recovered = recovered_semantic_source(source, prefix);
    let analysis = match core_policy {
        SemanticCompletionCorePolicy::WithCore => {
            analyze_source_with_interfaces_result(file, &recovered, interfaces)
        }
        SemanticCompletionCorePolicy::WithoutCore => {
            analyze_source_with_interfaces_without_core_result(file, &recovered, interfaces)
        }
    };
    let validity = semantic_validity(&analysis);
    let database = analysis.database();
    complete_program(
        source,
        prefix,
        cursor,
        cursor_context,
        database.program(),
        database.hir(),
        validity,
    )
}

fn semantic_validity(analysis: &AnalysisResult) -> SemanticCompletionValidity {
    if analysis.completion() != FrontendCompletion::Complete {
        SemanticCompletionValidity::Partial
    } else if analysis
        .diagnostics()
        .iter()
        .any(|diagnostic| diagnostic.severity.is_error())
    {
        SemanticCompletionValidity::Invalid
    } else {
        SemanticCompletionValidity::Valid
    }
}

fn complete_program(
    source: &str,
    prefix: &PrefixParseResult,
    cursor: usize,
    cursor_context: &CursorContext,
    program: &Program,
    hir: &Hir,
    validity: SemanticCompletionValidity,
) -> SemanticCompletionResult {
    // Completion projects lexical bindings, top-level declarations, resolved
    // named arguments, proven simple-receiver methods, and type-directed sum
    // variants. Complex receiver expressions and the remaining expression
    // grammar are not exhaustive, so a result set cannot honestly claim full
    // completeness yet. Individual candidates may still be Complete when their
    // local facts were proved.
    let mut completeness = SemanticCompletionCompleteness::Partial;
    if !program.unknown_top_level_spans.is_empty()
        || !program.malformed_declaration_spans.is_empty()
    {
        completeness = SemanticCompletionCompleteness::Partial;
    }

    let replace_prefix = source.get(prefix.replace_range.clone()).unwrap_or_default();
    let mut candidates = Vec::new();

    let scope = scope_at(source, cursor, program, hir);
    if scope.partial {
        completeness = SemanticCompletionCompleteness::Partial;
    }
    let availability = local_availability_before_cursor(source, cursor, hir, &scope);
    if availability.partial {
        completeness = SemanticCompletionCompleteness::Partial;
    }
    for binding in scope.bindings.values() {
        if availability.unavailable.contains(&binding.name) {
            // A checked local-flow Take event reaches an earlier source point.
            // Branch-sensitive availability is deliberately conservative: if
            // a move could have happened, do not offer an unsafe use.
            continue;
        }
        candidates.push(SemanticCompletion {
            name: binding.name.clone(),
            insert_text: binding.name.clone(),
            kind: binding.kind,
            ty: binding.ty.clone(),
            signature: None,
            // A declaration effect describes the function's contract, not a
            // use-site requirement. Only a resolved callee parameter can
            // prove `required_effect` for completion.
            required_effect: None,
            scope_depth: binding.depth,
            completeness: if scope.partial {
                SemanticCompletionCompleteness::Partial
            } else {
                SemanticCompletionCompleteness::Complete
            },
        });
    }

    let expected_type = expected_type(source, cursor, cursor_context, program, hir, &scope)
        .or_else(|| match_pattern_expected_type(source, cursor, program, hir, &scope));
    let receiver = receiver_at_cursor(source, prefix, &scope);
    let mut argument_context = call_context(source, cursor, hir, cursor_context);
    if let Some(context) = argument_context.take() {
        candidates.extend(remaining_named_arguments(&context, replace_prefix));
    } else if let Some(receiver) = receiver {
        candidates.extend(receiver_method_candidates(hir, receiver, replace_prefix));
    } else {
        candidates.extend(top_level_candidates(program, hir, replace_prefix));
        candidates.extend(sum_variant_candidates(
            hir,
            expected_type.as_ref(),
            replace_prefix,
        ));
    }

    retain_matching(&mut candidates, replace_prefix);
    deduplicate_shadowed(&mut candidates);
    candidates.sort_by(|left, right| {
        left.name
            .cmp(&right.name)
            .then(left.kind.cmp(&right.kind))
            .then(right.scope_depth.cmp(&left.scope_depth))
    });

    SemanticCompletionResult {
        candidates,
        expected_type,
        completeness,
        validity,
    }
}

/// Use only parser-owned recovery text. If the prefix oracle cannot prove a
/// suffix, the analyzer sees the original prefix and completion remains
/// conservative through its validity/completeness facts.
fn recovered_semantic_source(source: &str, prefix: &PrefixParseResult) -> String {
    let Some(suffix) = prefix.recovery_suffix() else {
        return source.to_string();
    };
    let mut recovered = String::with_capacity(source.len() + suffix.len());
    recovered.push_str(source);
    recovered.push_str(suffix);
    recovered
}

fn top_level_candidates(
    program: &Program,
    hir: &Hir,
    replace_prefix: &str,
) -> Vec<SemanticCompletion> {
    let mut candidates = Vec::new();
    for ty in hir.types() {
        candidates.push(SemanticCompletion {
            name: ty.name.clone(),
            insert_text: ty.name.clone(),
            kind: SemanticCompletionKind::Type,
            ty: Some(ResolvedType::named(ty.name.clone(), [])),
            signature: None,
            required_effect: None,
            scope_depth: 0,
            completeness: SemanticCompletionCompleteness::Complete,
        });
    }
    for alias in program.items.iter().filter_map(|item| match item {
        Item::TypeAlias(alias) => Some(alias),
        _ => None,
    }) {
        candidates.push(SemanticCompletion {
            name: alias.name.clone(),
            insert_text: alias.name.clone(),
            kind: SemanticCompletionKind::Type,
            ty: Some(ResolvedType::named(alias.name.clone(), [])),
            signature: None,
            required_effect: None,
            scope_depth: 0,
            completeness: SemanticCompletionCompleteness::Complete,
        });
    }
    for (name, signature) in hir.signatures() {
        if name.contains('.') || !name.starts_with(replace_prefix) {
            continue;
        }
        candidates.push(SemanticCompletion {
            name: name.to_string(),
            insert_text: format!("{name}()"),
            kind: SemanticCompletionKind::Function,
            ty: signature.return_ty.clone(),
            signature: Some(signature.clone()),
            required_effect: None,
            scope_depth: 0,
            completeness: SemanticCompletionCompleteness::Complete,
        });
    }
    candidates
}

fn receiver_at_cursor<'a>(
    source: &str,
    prefix: &PrefixParseResult,
    scope: &'a ScopeFacts,
) -> Option<&'a ScopeBinding> {
    let before_replace = source.get(..prefix.replace_range.start)?;
    let receiver_prefix = before_replace.strip_suffix('.')?;
    let receiver = receiver_prefix
        .rsplit(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .next()?;
    is_identifier(receiver)
        .then(|| scope.bindings.get(receiver))
        .flatten()
        .filter(|binding| binding.ty.is_some())
}

fn receiver_method_candidates(
    hir: &Hir,
    receiver: &ScopeBinding,
    replace_prefix: &str,
) -> Vec<SemanticCompletion> {
    let Some(receiver_type) = receiver.ty.as_ref() else {
        return Vec::new();
    };
    hir.receiver_methods(receiver_type)
        .into_iter()
        .filter(|signature| signature.name.starts_with(replace_prefix))
        .filter_map(|signature| {
            let receiver_effect = signature
                .params
                .first()
                .and_then(|parameter| parameter.effect)?;
            Some(SemanticCompletion {
                name: signature.name.clone(),
                insert_text: format!("{}()", signature.name),
                kind: SemanticCompletionKind::Method,
                ty: signature.return_ty.clone(),
                signature: Some(signature),
                // The implicit receiver is a call-site argument. Its effect is
                // emitted only because HIR resolution proved this signature.
                required_effect: Some(receiver_effect),
                scope_depth: receiver.depth,
                completeness: SemanticCompletionCompleteness::Complete,
            })
        })
        .collect()
}

fn sum_variant_candidates(
    hir: &Hir,
    expected: Option<&ResolvedType>,
    replace_prefix: &str,
) -> Vec<SemanticCompletion> {
    let Some(expected) = expected else {
        return Vec::new();
    };
    let Some(sum_name) = expected.root_name() else {
        return Vec::new();
    };
    if hir.type_kind(sum_name) != Some(crate::hir::HirTypeKind::Sum) {
        return Vec::new();
    }
    hir.sum_variants()
        .filter(|(variant, owner, _)| *owner == sum_name && variant.starts_with(replace_prefix))
        .map(|(variant, _, fields)| SemanticCompletion {
            name: variant.to_string(),
            insert_text: if fields.is_empty() {
                variant.to_string()
            } else {
                format!("{variant}()")
            },
            kind: SemanticCompletionKind::Variant,
            ty: Some(expected.clone()),
            signature: None,
            required_effect: None,
            scope_depth: 0,
            completeness: SemanticCompletionCompleteness::Complete,
        })
        .collect()
}

struct CallContext {
    signature: FunctionSig,
    used_names: BTreeSet<String>,
    argument_index: Option<usize>,
}

fn call_context(
    source: &str,
    cursor: usize,
    hir: &Hir,
    cursor_context: &CursorContext,
) -> Option<CallContext> {
    if cursor_context.site != SyntaxSite::CallArguments {
        return None;
    }
    let callee = cursor_context.call.as_ref()?.callee.as_deref()?;
    let resolution = hir.resolve_call(&callee_from_context(callee)?);
    let crate::hir::CallResolution::Resolved { signature, .. } = resolution else {
        return None;
    };
    let open = unmatched_open_paren(source, cursor)?;
    let mut used_names = BTreeSet::new();
    for segment in split_top_level(&source[open + 1..cursor], ',') {
        if let Some((name, _)) = segment.split_once(':') {
            let name = name.trim();
            if is_identifier(name) {
                used_names.insert(name.to_string());
            }
        }
    }
    Some(CallContext {
        signature: *signature,
        used_names,
        argument_index: cursor_context
            .call
            .as_ref()
            .and_then(|call| call.argument_index),
    })
}

fn callee_from_context(name: &str) -> Option<Callee> {
    if let Some((namespace, method)) = name.rsplit_once('.') {
        (is_identifier(namespace) && is_identifier(method)).then(|| Callee::Qualified {
            namespace: namespace.to_string(),
            name: method.to_string(),
        })
    } else {
        is_identifier(name).then(|| Callee::Name(name.to_string()))
    }
}

fn remaining_named_arguments(
    context: &CallContext,
    replace_prefix: &str,
) -> Vec<SemanticCompletion> {
    context
        .signature
        .params
        .iter()
        .filter(|parameter| {
            !context.used_names.contains(&parameter.name)
                && parameter.name.starts_with(replace_prefix)
        })
        .map(|parameter| SemanticCompletion {
            name: parameter.name.clone(),
            insert_text: format!(
                "{}: {} ",
                parameter.name,
                parameter.effect.map(ParamEffect::as_str).unwrap_or("read")
            ),
            kind: SemanticCompletionKind::ArgumentName,
            ty: Some(parameter.ty.clone()),
            signature: None,
            required_effect: parameter.effect,
            scope_depth: 0,
            completeness: SemanticCompletionCompleteness::Complete,
        })
        .collect()
}

fn expected_type(
    source: &str,
    cursor: usize,
    cursor_context: &CursorContext,
    program: &Program,
    hir: &Hir,
    scope: &ScopeFacts,
) -> Option<ResolvedType> {
    if let Some(context) = call_context(source, cursor, hir, cursor_context)
        && let Some(parameter) = context
            .signature
            .params
            .iter()
            .enumerate()
            .find_map(|(index, parameter)| {
                (context.argument_index == Some(index)
                    && !context.used_names.contains(&parameter.name))
                .then_some(parameter)
            })
            .or_else(|| {
                context
                    .signature
                    .params
                    .iter()
                    .find(|parameter| !context.used_names.contains(&parameter.name))
            })
    {
        return Some(parameter.ty.clone());
    }
    let function = program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) if function_contains(source, function, cursor) => {
                Some(function)
            }
            _ => None,
        })
        .min_by_key(|function| span_byte_length(source, &function.span));
    let function = function?;
    if after_keyword_on_line(source, cursor, "return") {
        return function.return_ty.as_ref().map(ResolvedType::from_type_ref);
    }
    if bool_condition_position(source, cursor) {
        return Some(ResolvedType::named("Bool", []));
    }
    if let Some(annotation) = typed_let_rhs(source, cursor, &function.body) {
        return Some(ResolvedType::from_type_ref(annotation));
    }
    if let Some(target) = assignment_target_before_cursor(source, cursor) {
        return scope
            .bindings
            .get(target)
            .and_then(|binding| binding.ty.clone());
    }
    None
}

fn is_identifier(text: &str) -> bool {
    !text.is_empty()
        && text.chars().enumerate().all(|(index, ch)| {
            (index == 0 && (ch == '_' || ch.is_ascii_alphabetic()))
                || (index > 0 && (ch == '_' || ch.is_ascii_alphanumeric()))
        })
}

fn span_start_byte(source: &str, span: &Span) -> Option<usize> {
    let line = source.lines().nth(span.line.checked_sub(1)?)?;
    let prefix_len = source
        .lines()
        .take(span.line.saturating_sub(1))
        .map(|line| line.len() + 1)
        .sum::<usize>();
    let column = span.column.checked_sub(1)?;
    let column_byte = line
        .char_indices()
        .nth(column)
        .map_or(line.len(), |(index, _)| index);
    Some(prefix_len + column_byte)
}

fn span_end_byte(source: &str, span: &Span) -> Option<usize> {
    let start = span_start_byte(source, span)?;
    let tail = &source[start..];
    let length_byte = tail
        .char_indices()
        .nth(span.length)
        .map_or(tail.len(), |(index, _)| index);
    Some(start + length_byte)
}

fn function_contains(source: &str, function: &FunctionDecl, cursor: usize) -> bool {
    span_start_byte(source, &function.span)
        .is_some_and(|start| brace_contains_after(source, start, cursor))
}

fn block_contains(source: &str, block: &Block, cursor: usize) -> bool {
    span_start_byte(source, &block.span)
        .is_some_and(|start| brace_contains_after(source, start, cursor))
}

/// Find the first opening brace at/after a syntax node's start and determine
/// whether it is the lexical brace enclosing `cursor`.  AST spans intentionally
/// identify starts (they cannot encode multi-line ranges), so range containment
/// must be measured from source here rather than by comparing span lengths.
fn brace_contains_after(source: &str, start: usize, cursor: usize) -> bool {
    let Some(relative_open) = source[start..].find('{') else {
        return false;
    };
    let open = start + relative_open;
    if cursor <= open {
        return false;
    }
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (relative, ch) in source[open..].char_indices() {
        if in_string {
            if !escaped && ch == '"' {
                in_string = false;
            }
            escaped = !escaped && ch == '\\';
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return open + relative >= cursor;
                }
            }
            _ => {}
        }
    }
    true
}

fn span_byte_length(source: &str, span: &Span) -> usize {
    match (span_start_byte(source, span), span_end_byte(source, span)) {
        (Some(start), Some(end)) => end.saturating_sub(start),
        _ => usize::MAX,
    }
}

fn find_name_in_span(source: &str, span: &Span, name: &str) -> Option<usize> {
    let start = span_start_byte(source, span)?;
    let line = source
        .get(start..)?
        .split_once('\n')
        .map_or_else(|| source.get(start..).unwrap_or_default(), |(line, _)| line);
    line.find(name).map(|offset| start + offset)
}
