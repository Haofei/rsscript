//! Stateless LSP symbol and semantic-token conversion.

use rsscript_language_service::{SymbolKind as RssSymbolKind, *};
use tower_lsp::lsp_types::{SymbolKind as LspSymbolKind, *};

use crate::text::*;

pub(crate) fn definition_matches_lookup(definition: &Definition, lookup: &SymbolLookup) -> bool {
    definition.name == lookup.name
        && if lookup.is_type {
            definition.kind == RssSymbolKind::Type
        } else {
            matches!(
                definition.kind,
                RssSymbolKind::Function | RssSymbolKind::Const | RssSymbolKind::Variant
            )
        }
}

pub(crate) fn unresolved_reference_matches_lookup(
    reference: &Reference,
    lookup: &SymbolLookup,
) -> bool {
    reference.definition.is_none()
        && reference.name == lookup.name
        && reference.is_type == lookup.is_type
}

#[allow(deprecated)]
pub(crate) fn to_lsp_symbol_information(
    uri: &Url,
    source: &str,
    definition: &Definition,
) -> SymbolInformation {
    SymbolInformation {
        name: definition.name.clone(),
        kind: to_lsp_symbol_kind(definition.kind),
        tags: None,
        deprecated: None,
        location: Location {
            uri: uri.clone(),
            range: span_to_range(source, &definition.span),
        },
        container_name: None,
    }
}

#[allow(deprecated)]
pub(crate) fn to_lsp_document_symbol(source: &str, symbol: RssDocumentSymbol) -> DocumentSymbol {
    DocumentSymbol {
        name: symbol.name,
        detail: symbol.detail,
        kind: to_lsp_symbol_kind(symbol.kind),
        tags: None,
        deprecated: None,
        range: span_to_range(source, &symbol.span),
        selection_range: span_to_range(source, &symbol.selection_span),
        children: if symbol.children.is_empty() {
            None
        } else {
            Some(
                symbol
                    .children
                    .into_iter()
                    .map(|child| to_lsp_document_symbol(source, child))
                    .collect(),
            )
        },
    }
}

pub(crate) const TOKEN_FUNCTION: u32 = 0;
pub(crate) const TOKEN_TYPE: u32 = 1;
pub(crate) const TOKEN_CONST: u32 = 2;
pub(crate) const TOKEN_PARAM: u32 = 3;
pub(crate) const TOKEN_LOCAL: u32 = 4;
pub(crate) const TOKEN_FIELD: u32 = 5;
pub(crate) const TOKEN_VARIANT: u32 = 6;
pub(crate) const TOKEN_RESOURCE: u32 = 7;
pub(crate) const TOKEN_KEYWORD: u32 = 8;

pub(crate) const MOD_DEFINITION: u32 = 1;
pub(crate) const MOD_READONLY: u32 = 1 << 1;
pub(crate) const MOD_ASYNC: u32 = 1 << 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct RawSemanticToken {
    pub(crate) line: u32,
    pub(crate) start: u32,
    pub(crate) length: u32,
    pub(crate) token_type: u32,
    pub(crate) modifiers: u32,
}

pub(crate) fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::FUNCTION,
            SemanticTokenType::TYPE,
            SemanticTokenType::new("const"),
            SemanticTokenType::PARAMETER,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::ENUM_MEMBER,
            SemanticTokenType::new("resource"),
            SemanticTokenType::KEYWORD,
        ],
        token_modifiers: vec![
            SemanticTokenModifier::DEFINITION,
            SemanticTokenModifier::READONLY,
            SemanticTokenModifier::ASYNC,
        ],
    }
}

#[cfg(test)]
pub(crate) fn semantic_tokens_for_source(path: &str, source: &str) -> SemanticTokens {
    let index = symbol_index(path, source);
    semantic_tokens_for_index(source, &index)
}

pub(crate) fn semantic_tokens_for_index(
    source: &str,
    index: &rsscript_language_service::SymbolIndex,
) -> SemanticTokens {
    let mut raw = Vec::new();
    for definition in index.definitions() {
        let span = semantic_definition_span(source, definition);
        push_span_token(
            source,
            &mut raw,
            &span,
            semantic_token_type_for_symbol(definition.kind),
            MOD_DEFINITION | semantic_modifiers_for_symbol(definition.kind),
        );
    }
    for reference in index.references() {
        let token_type = index
            .symbol_at(reference.span.line, reference.span.column)
            .map(|symbol| semantic_token_type_for_symbol(symbol.kind))
            .unwrap_or(if reference.is_type {
                TOKEN_TYPE
            } else {
                TOKEN_LOCAL
            });
        push_span_token(source, &mut raw, &reference.span, token_type, 0);
    }
    push_keyword_tokens(source, &mut raw);
    raw.sort();
    raw.dedup_by(|left, right| {
        left.line == right.line && left.start == right.start && left.length == right.length
    });
    SemanticTokens {
        result_id: None,
        data: encode_semantic_tokens(raw),
    }
}

pub(crate) fn semantic_definition_span(source: &str, definition: &Definition) -> Span {
    let Some(line) = source.lines().nth(definition.span.line.saturating_sub(1)) else {
        return definition.span.clone();
    };
    let start_char = definition.span.column.saturating_sub(1);
    let start_byte = byte_offset_for_char(line, start_char);
    let Some(relative_byte) = line[start_byte..].find(&definition.name) else {
        return definition.span.clone();
    };
    let before_name = &line[..start_byte + relative_byte];
    Span {
        file: definition.span.file.clone(),
        line: definition.span.line,
        column: before_name.chars().count() + 1,
        length: definition.name.chars().count(),
    }
}

pub(crate) fn byte_offset_for_char(value: &str, chars: usize) -> usize {
    value
        .char_indices()
        .nth(chars)
        .map(|(index, _)| index)
        .unwrap_or(value.len())
}

pub(crate) fn semantic_token_type_for_symbol(kind: RssSymbolKind) -> u32 {
    match kind {
        RssSymbolKind::Function => TOKEN_FUNCTION,
        RssSymbolKind::Type => TOKEN_TYPE,
        RssSymbolKind::Const => TOKEN_CONST,
        RssSymbolKind::Param => TOKEN_PARAM,
        RssSymbolKind::Local => TOKEN_LOCAL,
        RssSymbolKind::Field => TOKEN_FIELD,
        RssSymbolKind::Variant => TOKEN_VARIANT,
    }
}

pub(crate) fn semantic_modifiers_for_symbol(kind: RssSymbolKind) -> u32 {
    match kind {
        RssSymbolKind::Const => MOD_READONLY,
        _ => 0,
    }
}

pub(crate) fn push_span_token(
    source: &str,
    raw: &mut Vec<RawSemanticToken>,
    span: &Span,
    token_type: u32,
    modifiers: u32,
) {
    let range = span_to_range(source, span);
    if range.start.line != range.end.line || range.end.character <= range.start.character {
        return;
    }
    raw.push(RawSemanticToken {
        line: range.start.line,
        start: range.start.character,
        length: range.end.character - range.start.character,
        token_type,
        modifiers,
    });
}

pub(crate) fn push_keyword_tokens(source: &str, raw: &mut Vec<RawSemanticToken>) {
    for (line_index, line) in source.lines().enumerate() {
        let mut chars = line.char_indices().peekable();
        let mut in_string = false;
        while let Some((byte, character)) = chars.next() {
            if !in_string && character == '/' && chars.peek().is_some_and(|(_, next)| *next == '/')
            {
                break;
            }
            if character == '"' {
                in_string = !in_string;
                continue;
            }
            if in_string || !(character.is_ascii_alphabetic() || character == '_') {
                continue;
            }
            let start_byte = byte;
            let mut end_byte = byte + character.len_utf8();
            while let Some((next_byte, next)) = chars.peek().copied() {
                if !(next.is_ascii_alphanumeric() || next == '_') {
                    break;
                }
                chars.next();
                end_byte = next_byte + next.len_utf8();
            }
            let word = &line[start_byte..end_byte];
            let Some((token_type, modifiers)) = semantic_keyword_token(word) else {
                continue;
            };
            raw.push(RawSemanticToken {
                line: line_index as u32,
                start: utf16_len(&line[..start_byte]),
                length: utf16_len(word),
                token_type,
                modifiers,
            });
        }
    }
}

pub(crate) fn semantic_keyword_token(word: &str) -> Option<(u32, u32)> {
    match word {
        "resource" => Some((TOKEN_RESOURCE, 0)),
        "read" | "mut" | "take" | "fresh" | "owned" | "noescape" | "with" => {
            Some((TOKEN_KEYWORD, 0))
        }
        "async" => Some((TOKEN_KEYWORD, MOD_ASYNC)),
        "fn" | "struct" | "sum" | "protocol" | "const" | "let" | "return" | "if" | "else"
        | "match" | "while" | "for" | "in" | "as" | "await" | "task_group" | "impl" => {
            Some((TOKEN_KEYWORD, 0))
        }
        _ => None,
    }
}

pub(crate) fn utf16_len(value: &str) -> u32 {
    value
        .chars()
        .map(|character| character.len_utf16() as u32)
        .sum()
}

pub(crate) fn encode_semantic_tokens(raw: Vec<RawSemanticToken>) -> Vec<SemanticToken> {
    let mut encoded = Vec::with_capacity(raw.len());
    let mut previous_line = 0u32;
    let mut previous_start = 0u32;
    for token in raw {
        let delta_line = token.line.saturating_sub(previous_line);
        let delta_start = if delta_line == 0 {
            token.start.saturating_sub(previous_start)
        } else {
            token.start
        };
        encoded.push(SemanticToken {
            delta_line,
            delta_start,
            length: token.length,
            token_type: token.token_type,
            token_modifiers_bitset: token.modifiers,
        });
        previous_line = token.line;
        previous_start = token.start;
    }
    encoded
}

pub(crate) fn to_lsp_symbol_kind(kind: RssSymbolKind) -> LspSymbolKind {
    match kind {
        RssSymbolKind::Function => LspSymbolKind::FUNCTION,
        RssSymbolKind::Type => LspSymbolKind::STRUCT,
        RssSymbolKind::Const => LspSymbolKind::CONSTANT,
        RssSymbolKind::Param => LspSymbolKind::VARIABLE,
        RssSymbolKind::Local => LspSymbolKind::VARIABLE,
        RssSymbolKind::Field => LspSymbolKind::FIELD,
        RssSymbolKind::Variant => LspSymbolKind::ENUM_MEMBER,
    }
}

pub(crate) fn symbol_kind_label(kind: RssSymbolKind) -> &'static str {
    match kind {
        RssSymbolKind::Function => "function",
        RssSymbolKind::Type => "type",
        RssSymbolKind::Const => "const",
        RssSymbolKind::Param => "parameter",
        RssSymbolKind::Local => "local",
        RssSymbolKind::Field => "field",
        RssSymbolKind::Variant => "variant",
    }
}
