//! Symbol lookup, references, rename, signatures, and call hierarchy.

use std::collections::{HashMap, HashSet};

use rsscript::{SymbolKind as RssSymbolKind, *};
use tower_lsp::lsp_types::{SymbolKind as LspSymbolKind, *};

use crate::documents::*;
use crate::features::*;
use crate::text::*;
use crate::workspace::*;

pub(crate) struct CallContext {
    pub(crate) callee: String,
    pub(crate) active_parameter: usize,
}

pub(crate) fn workspace_definition_location(
    documents: &[WorkspaceDocument],
    lookup: &SymbolLookup,
) -> Option<Location> {
    documents.iter().find_map(|document| {
        let index = document.symbol_index();
        index
            .definitions()
            .iter()
            .find(|definition| definition_matches_lookup(definition, lookup))
            .map(|definition| Location {
                uri: document.uri.clone(),
                range: span_to_range(&document.text, &definition.span),
            })
    })
}

pub(crate) fn hover_symbol_info(
    uri: &Url,
    open_documents: &HashMap<Url, Document>,
    index: &rsscript::SymbolIndex,
    line: usize,
    column: usize,
    package_inputs: &PackageInputCache,
) -> Option<SymbolInfo> {
    let symbol = index.symbol_at(line, column)?;
    if symbol.detail.is_some() {
        return Some(symbol);
    }
    let lookup = index.lookup_at(line, column)?;
    if lookup.local_definition.is_some() {
        return Some(symbol);
    }
    let workspace_documents = workspace_documents_for_uri(uri, open_documents, package_inputs);
    workspace_symbol_info(&workspace_documents, &lookup).or(Some(symbol))
}

pub(crate) fn workspace_symbol_info(
    documents: &[WorkspaceDocument],
    lookup: &SymbolLookup,
) -> Option<SymbolInfo> {
    documents.iter().find_map(|document| {
        let index = document.symbol_index();
        index
            .definitions()
            .iter()
            .find(|definition| definition_matches_lookup(definition, lookup))
            .map(|definition| SymbolInfo {
                name: definition.name.clone(),
                kind: definition.kind,
                span: definition.span.clone(),
                detail: definition.detail.clone(),
            })
    })
}

pub(crate) fn symbol_hover_markdown(symbol: &SymbolInfo) -> String {
    let mut markdown = format!("**{}** `{}`", symbol_kind_label(symbol.kind), symbol.name);
    if let Some(detail) = &symbol.detail {
        markdown.push_str("\n\n```rss\n");
        markdown.push_str(detail);
        markdown.push_str("\n```");
    }
    markdown
}

pub(crate) fn call_context_at(source: &str, position: Position) -> Option<CallContext> {
    let cursor = byte_offset(source, position);
    let prefix = source.get(..cursor)?;
    let open = innermost_unclosed_call_open(prefix)?;
    let callee = callee_before_open(prefix, open)?;
    Some(CallContext {
        callee: normalize_callee_name(&callee),
        active_parameter: active_parameter_index(&prefix[open + 1..]),
    })
}

pub(crate) fn innermost_unclosed_call_open(prefix: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in prefix.char_indices().rev() {
        match character {
            ')' => depth += 1,
            '(' if depth == 0 => return Some(index),
            '(' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

pub(crate) fn callee_before_open(prefix: &str, open: usize) -> Option<String> {
    let before = prefix.get(..open)?.trim_end();
    let end = before.len();
    let start = before
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '<' | '>' | ',')
            {
                None
            } else {
                Some(index + character.len_utf8())
            }
        })
        .unwrap_or(0);
    let callee = before.get(start..end)?.trim();
    if callee.is_empty() {
        None
    } else {
        Some(callee.to_string())
    }
}

pub(crate) fn active_parameter_index(args_prefix: &str) -> usize {
    let mut depth = 0usize;
    let mut active = 0usize;
    for character in args_prefix.chars() {
        match character {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => active += 1,
            _ => {}
        }
    }
    active
}

pub(crate) fn normalize_callee_name(callee: &str) -> String {
    let mut normalized = String::new();
    let mut generic_depth = 0usize;
    for character in callee.chars() {
        match character {
            '<' => generic_depth += 1,
            '>' => generic_depth = generic_depth.saturating_sub(1),
            _ if generic_depth == 0 => normalized.push(character),
            _ => {}
        }
    }
    normalized
}

pub(crate) fn workspace_function_definition(
    documents: &[WorkspaceDocument],
    callee: &str,
) -> Option<Definition> {
    documents.iter().find_map(|document| {
        let index = document.symbol_index();
        index
            .definitions()
            .iter()
            .find(|definition| {
                definition.name == callee && definition.kind == RssSymbolKind::Function
            })
            .cloned()
    })
}

pub(crate) fn call_hierarchy_item_at(
    uri: &Url,
    position: Position,
    open_documents: &HashMap<Url, Document>,
    documents: &[WorkspaceDocument],
) -> Option<CallHierarchyItem> {
    let document = open_documents.get(uri)?;
    let (line, column) = char_position(&document.text, position);
    let index = document.symbol_index(uri.path());
    if let Some(symbol) = index.symbol_at(line, column)
        && symbol.kind == RssSymbolKind::Function
    {
        let definition = find_function_definition(documents, &symbol.name)?;
        return Some(to_call_hierarchy_item(&document.text, uri, &definition));
    }
    let lookup = index.lookup_at(line, column)?;
    if lookup.is_type {
        return None;
    }
    let definition = find_function_definition(documents, &lookup.name)?;
    Some(to_call_hierarchy_item(&document.text, uri, &definition))
}

pub(crate) fn incoming_call_hierarchy(
    documents: &[WorkspaceDocument],
    item: &CallHierarchyItem,
) -> Vec<CallHierarchyIncomingCall> {
    let mut calls_by_function: HashMap<(Url, String), (CallHierarchyItem, Vec<Range>)> =
        HashMap::new();
    for document in documents {
        let index = document.symbol_index();
        for reference in index
            .references()
            .iter()
            .filter(|reference| reference.name == item.name && !reference.is_type)
        {
            let Some(caller) = enclosing_function_definition(&index, reference) else {
                continue;
            };
            if caller.name == item.name && document.uri == item.uri {
                continue;
            }
            let caller_item = to_call_hierarchy_item(&document.text, &document.uri, caller);
            calls_by_function
                .entry((document.uri.clone(), caller_item.name.clone()))
                .or_insert_with(|| (caller_item, Vec::new()))
                .1
                .push(span_to_range(&document.text, &reference.span));
        }
    }
    let mut calls = calls_by_function
        .into_values()
        .map(|(from, from_ranges)| CallHierarchyIncomingCall { from, from_ranges })
        .collect::<Vec<_>>();
    calls.sort_by(|left, right| {
        left.from
            .uri
            .as_str()
            .cmp(right.from.uri.as_str())
            .then_with(|| left.from.name.cmp(&right.from.name))
    });
    calls
}

pub(crate) fn outgoing_call_hierarchy(
    documents: &[WorkspaceDocument],
    item: &CallHierarchyItem,
) -> Vec<CallHierarchyOutgoingCall> {
    let Some(document) = documents.iter().find(|document| document.uri == item.uri) else {
        return Vec::new();
    };
    let index = document.symbol_index();
    let Some(caller) = index
        .definitions()
        .iter()
        .find(|definition| {
            definition.kind == RssSymbolKind::Function
                && definition.name == item.name
                && span_to_range(&document.text, &definition.span) == item.selection_range
        })
        .or_else(|| {
            index.definitions().iter().find(|definition| {
                definition.kind == RssSymbolKind::Function && definition.name == item.name
            })
        })
    else {
        return Vec::new();
    };
    let mut calls_by_function: HashMap<(Url, String), (CallHierarchyItem, Vec<Range>)> =
        HashMap::new();
    let caller_end_line = next_function_line(&index, caller).unwrap_or(usize::MAX);
    for reference in index.references().iter().filter(|reference| {
        !reference.is_type
            && reference.name != item.name
            && reference.span.line > caller.span.line
            && reference.span.line < caller_end_line
    }) {
        let Some((callee_document, callee_definition)) =
            find_function_definition_with_document(documents, &reference.name)
        else {
            continue;
        };
        let callee_item = to_call_hierarchy_item(
            &callee_document.text,
            &callee_document.uri,
            &callee_definition,
        );
        calls_by_function
            .entry((callee_document.uri.clone(), callee_item.name.clone()))
            .or_insert_with(|| (callee_item, Vec::new()))
            .1
            .push(span_to_range(&document.text, &reference.span));
    }
    let mut calls = calls_by_function
        .into_values()
        .map(|(to, from_ranges)| CallHierarchyOutgoingCall { to, from_ranges })
        .collect::<Vec<_>>();
    calls.sort_by(|left, right| {
        left.to
            .uri
            .as_str()
            .cmp(right.to.uri.as_str())
            .then_with(|| left.to.name.cmp(&right.to.name))
    });
    calls
}

pub(crate) fn find_function_definition(
    documents: &[WorkspaceDocument],
    name: &str,
) -> Option<Definition> {
    documents.iter().find_map(|document| {
        let index = document.symbol_index();
        index
            .definitions()
            .iter()
            .find(|definition| {
                definition.kind == RssSymbolKind::Function && definition.name == name
            })
            .cloned()
    })
}

pub(crate) fn find_function_definition_with_document<'a>(
    documents: &'a [WorkspaceDocument],
    name: &str,
) -> Option<(&'a WorkspaceDocument, Definition)> {
    documents.iter().find_map(|document| {
        let index = document.symbol_index();
        index
            .definitions()
            .iter()
            .find(|definition| {
                definition.kind == RssSymbolKind::Function && definition.name == name
            })
            .cloned()
            .map(|definition| (document, definition))
    })
}

pub(crate) fn enclosing_function_definition<'a>(
    index: &'a rsscript::SymbolIndex,
    reference: &Reference,
) -> Option<&'a Definition> {
    index
        .definitions()
        .iter()
        .filter(|definition| {
            definition.kind == RssSymbolKind::Function && definition.span.line < reference.span.line
        })
        .max_by_key(|definition| definition.span.line)
}

pub(crate) fn to_call_hierarchy_item(
    source: &str,
    uri: &Url,
    definition: &Definition,
) -> CallHierarchyItem {
    let selection_range = span_to_range(source, &semantic_definition_span(source, definition));
    CallHierarchyItem {
        name: definition.name.clone(),
        kind: LspSymbolKind::FUNCTION,
        tags: None,
        detail: definition.detail.clone(),
        uri: uri.clone(),
        range: selection_range,
        selection_range,
        data: None,
    }
}

pub(crate) fn next_function_line(
    index: &rsscript::SymbolIndex,
    function: &Definition,
) -> Option<usize> {
    index
        .definitions()
        .iter()
        .filter(|definition| {
            definition.kind == RssSymbolKind::Function && definition.span.line > function.span.line
        })
        .map(|definition| definition.span.line)
        .min()
}

pub(crate) fn signature_information(
    definition: &Definition,
    active_parameter: usize,
) -> Option<SignatureInformation> {
    let label = definition.detail.as_ref()?.clone();
    let parameters = signature_parameter_labels(&label)
        .into_iter()
        .map(|parameter| ParameterInformation {
            label: ParameterLabel::Simple(parameter),
            documentation: None,
        })
        .collect::<Vec<_>>();
    Some(SignatureInformation {
        label,
        documentation: None,
        parameters: Some(parameters),
        active_parameter: Some(active_parameter as u32),
    })
}

pub(crate) fn signature_parameter_labels(label: &str) -> Vec<String> {
    let Some(open) = label.find('(') else {
        return Vec::new();
    };
    let Some(close) = find_matching_paren_in_str(label, open) else {
        return Vec::new();
    };
    split_top_level_commas(&label[open + 1..close])
}

pub(crate) fn find_matching_paren_in_str(value: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, character) in value.char_indices().skip_while(|(index, _)| *index < open) {
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

pub(crate) fn split_top_level_commas(value: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, character) in value.char_indices() {
        match character {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                let part = value[start..index].trim();
                if !part.is_empty() {
                    parts.push(part.to_string());
                }
                start = index + character.len_utf8();
            }
            _ => {}
        }
    }
    let part = value[start..].trim();
    if !part.is_empty() {
        parts.push(part.to_string());
    }
    parts
}

pub(crate) fn workspace_reference_locations(
    documents: &[WorkspaceDocument],
    lookup: &SymbolLookup,
    include_declaration: bool,
) -> Vec<Location> {
    let mut locations = Vec::new();
    for document in documents {
        let index = document.symbol_index();
        if include_declaration {
            for definition in index
                .definitions()
                .iter()
                .filter(|definition| definition_matches_lookup(definition, lookup))
            {
                locations.push(Location {
                    uri: document.uri.clone(),
                    range: span_to_range(&document.text, &definition.span),
                });
            }
        }
        for reference in index
            .references()
            .iter()
            .filter(|reference| unresolved_reference_matches_lookup(reference, lookup))
        {
            locations.push(Location {
                uri: document.uri.clone(),
                range: span_to_range(&document.text, &reference.span),
            });
        }
    }
    locations
}

pub(crate) fn reference_locations_for_position(
    uri: &Url,
    position: Position,
    open_documents: &HashMap<Url, Document>,
    include_declaration: bool,
    package_inputs: &PackageInputCache,
) -> Vec<Location> {
    let Some(document) = open_documents.get(uri) else {
        return Vec::new();
    };
    let (line, column) = char_position(&document.text, position);
    let index = document.symbol_index(uri.path());
    let Some(lookup) = index.lookup_at(line, column) else {
        return Vec::new();
    };
    if lookup.local_definition.is_some() {
        return index
            .references_at(line, column, include_declaration)
            .into_iter()
            .map(|span| Location {
                uri: uri.clone(),
                range: span_to_range(&document.text, &span),
            })
            .collect();
    }
    let workspace_documents = workspace_documents_for_uri(uri, open_documents, package_inputs);
    workspace_reference_locations(&workspace_documents, &lookup, include_declaration)
}

pub(crate) fn rename_target(
    uri: &Url,
    position: Position,
    open_documents: &HashMap<Url, Document>,
) -> Option<(Range, String)> {
    let document = open_documents.get(uri)?;
    let (line, column) = char_position(&document.text, position);
    let index = document.symbol_index(uri.path());
    index.lookup_at(line, column)?;
    let symbol = index.symbol_at(line, column)?;
    Some((span_to_range(&document.text, &symbol.span), symbol.name))
}

pub(crate) fn rename_workspace_edit(
    uri: &Url,
    position: Position,
    new_name: &str,
    open_documents: &HashMap<Url, Document>,
    package_inputs: &PackageInputCache,
) -> Option<WorkspaceEdit> {
    let document = open_documents.get(uri)?;
    let (line, column) = char_position(&document.text, position);
    let index = document.symbol_index(uri.path());
    let lookup = index.lookup_at(line, column)?;
    let symbol = index.symbol_at(line, column)?;
    let locations = if lookup.local_definition.is_some()
        && matches!(symbol.kind, RssSymbolKind::Param | RssSymbolKind::Local)
    {
        index
            .references_at(line, column, true)
            .into_iter()
            .map(|span| Location {
                uri: uri.clone(),
                range: span_to_range(&document.text, &span),
            })
            .collect::<Vec<_>>()
    } else {
        let workspace_documents = workspace_documents_for_uri(uri, open_documents, package_inputs);
        workspace_reference_locations(&workspace_documents, &lookup, true)
    };
    let changes = rename_changes(locations, new_name);
    if changes.is_empty() {
        None
    } else {
        Some(WorkspaceEdit {
            changes: Some(changes),
            document_changes: None,
            change_annotations: None,
        })
    }
}

pub(crate) fn rename_changes(
    locations: Vec<Location>,
    new_name: &str,
) -> HashMap<Url, Vec<TextEdit>> {
    let mut seen = HashSet::new();
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for location in locations {
        let key = (
            location.uri.clone(),
            location.range.start.line,
            location.range.start.character,
            location.range.end.line,
            location.range.end.character,
        );
        if !seen.insert(key) {
            continue;
        }
        changes.entry(location.uri).or_default().push(TextEdit {
            range: location.range,
            new_text: new_name.to_string(),
        });
    }
    changes
}

pub(crate) fn valid_rename_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}
