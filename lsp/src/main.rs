//! Language server for RsScript.
//!
//! Reuses the `rsscript` checker library directly: diagnostics come from the
//! same `analyze_source_with_core` + `lint_source` path as the CLI, and
//! formatting from `format_source`, so the editor never disagrees with the
//! command line.

use std::collections::HashMap;

use rsscript::{
    Diagnostic as RsDiagnostic, Severity, Span, analyze_source_with_core, explain_diagnostic_code,
    format_source, lint_source, symbol_index,
};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

/// A document the editor has open, plus its most recent diagnostics (kept so
/// hover can explain whichever diagnostic the cursor is on).
struct Document {
    text: String,
    diagnostics: Vec<RsDiagnostic>,
}

struct Backend {
    client: Client,
    documents: tokio::sync::Mutex<HashMap<Url, Document>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            documents: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Run the checker over `text` and publish the results for `uri`.
    async fn analyze_and_publish(&self, uri: Url, text: String, version: Option<i32>) {
        let path = uri.path().to_string();
        let mut diagnostics = analyze_source_with_core(&path, &text);
        diagnostics.extend(lint_source(&path, &text));

        let lsp_diagnostics = diagnostics
            .iter()
            .map(|diagnostic| to_lsp_diagnostic(&text, diagnostic))
            .collect();

        self.client
            .publish_diagnostics(uri.clone(), lsp_diagnostics, version)
            .await;

        self.documents
            .lock()
            .await
            .insert(uri, Document { text, diagnostics });
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "rss-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                document_formatting_provider: Some(OneOf::Left(true)),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "rss-lsp ready")
            .await;
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        self.analyze_and_publish(doc.uri, doc.text, Some(doc.version))
            .await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        // Start from the last known text and apply the incremental edits in
        // order. A change with no range is a full-document replacement.
        let mut text = {
            let documents = self.documents.lock().await;
            documents
                .get(&uri)
                .map(|document| document.text.clone())
                .unwrap_or_default()
        };
        for change in &params.content_changes {
            apply_change(&mut text, change);
        }
        self.analyze_and_publish(uri, text, Some(params.text_document.version))
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if let Some(text) = params.text {
            self.analyze_and_publish(params.text_document.uri, text, None)
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        self.documents
            .lock()
            .await
            .remove(&params.text_document.uri);
        self.client
            .publish_diagnostics(params.text_document.uri, Vec::new(), None)
            .await;
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let documents = self.documents.lock().await;
        let Some(document) = documents.get(&params.text_document.uri) else {
            return Ok(None);
        };
        let path = params.text_document.uri.path();
        let formatted = format_source(path, &document.text);
        if formatted == document.text {
            return Ok(None);
        }
        Ok(Some(vec![TextEdit {
            range: full_document_range(&document.text),
            new_text: formatted,
        }]))
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        let documents = self.documents.lock().await;
        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let hovered = document.diagnostics.iter().find(|diagnostic| {
            let range = span_to_range(&document.text, &diagnostic.span);
            position_in_range(position, &range)
        });
        let Some(diagnostic) = hovered else {
            return Ok(None);
        };

        let mut markdown = format!("**{}** — {}", diagnostic.code, diagnostic.summary);
        if let Some(explanation) = explain_diagnostic_code(&diagnostic.code) {
            markdown.push_str("\n\n");
            markdown.push_str(explanation.explanation);
        }

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: markdown,
            }),
            range: Some(span_to_range(&document.text, &diagnostic.span)),
        }))
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let position = params.text_document_position_params.position;
        let uri = params.text_document_position_params.text_document.uri;
        let documents = self.documents.lock().await;
        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let (line, column) = char_position(&document.text, position);
        let index = symbol_index(uri.path(), &document.text);
        let Some(span) = index.definition_at(line, column) else {
            return Ok(None);
        };

        Ok(Some(GotoDefinitionResponse::Scalar(Location {
            uri: uri.clone(),
            range: span_to_range(&document.text, span),
        })))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let position = params.text_document_position.position;
        let uri = params.text_document_position.text_document.uri;
        let documents = self.documents.lock().await;
        let Some(document) = documents.get(&uri) else {
            return Ok(None);
        };

        let (line, column) = char_position(&document.text, position);
        let index = symbol_index(uri.path(), &document.text);
        let locations = index
            .references_at(line, column, params.context.include_declaration)
            .into_iter()
            .map(|span| Location {
                uri: uri.clone(),
                range: span_to_range(&document.text, &span),
            })
            .collect();
        Ok(Some(locations))
    }
}

fn to_lsp_diagnostic(source: &str, diagnostic: &RsDiagnostic) -> Diagnostic {
    Diagnostic {
        range: span_to_range(source, &diagnostic.span),
        severity: Some(match diagnostic.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
        }),
        code: Some(NumberOrString::String(diagnostic.code.clone())),
        source: Some("rsscript".to_string()),
        message: if diagnostic.label.is_empty() {
            diagnostic.summary.clone()
        } else {
            format!("{}\n{}", diagnostic.summary, diagnostic.label)
        },
        ..Diagnostic::default()
    }
}

/// Map a checker [`Span`] (1-based line/column counted in `char`s) to an LSP
/// [`Range`] (0-based line, UTF-16 code units).
fn span_to_range(source: &str, span: &Span) -> Range {
    let line_index = span.line.saturating_sub(1);
    let line_text = source.lines().nth(line_index).unwrap_or("");
    let start_char = span.column.saturating_sub(1);
    let end_char = start_char + span.length;

    let utf16_column = |chars: usize| -> u32 {
        line_text
            .chars()
            .take(chars)
            .map(|character| character.len_utf16())
            .sum::<usize>() as u32
    };

    let line = line_index as u32;
    Range {
        start: Position {
            line,
            character: utf16_column(start_char),
        },
        end: Position {
            line,
            character: utf16_column(end_char),
        },
    }
}

fn full_document_range(text: &str) -> Range {
    let last_line = text.lines().count().saturating_sub(1) as u32;
    let last_column = text
        .lines()
        .last()
        .map(|line| line.chars().map(|c| c.len_utf16()).sum::<usize>() as u32)
        .unwrap_or(0);
    Range {
        start: Position {
            line: 0,
            character: 0,
        },
        end: Position {
            // Cover a possible trailing newline by extending one line past the
            // last content line when the text ends with '\n'.
            line: if text.ends_with('\n') {
                last_line + 1
            } else {
                last_line
            },
            character: if text.ends_with('\n') { 0 } else { last_column },
        },
    }
}

/// Convert an LSP [`Position`] (0-based line, UTF-16 column) to the checker's
/// 1-based line / 1-based `char` column.
fn char_position(source: &str, position: Position) -> (usize, usize) {
    let line_text = source.lines().nth(position.line as usize).unwrap_or("");
    let mut utf16 = 0u32;
    let mut chars = 0usize;
    for character in line_text.chars() {
        if utf16 >= position.character {
            break;
        }
        utf16 += character.len_utf16() as u32;
        chars += 1;
    }
    (position.line as usize + 1, chars + 1)
}

/// Apply one incremental (or full) content change to `text` in place.
fn apply_change(text: &mut String, change: &TextDocumentContentChangeEvent) {
    match change.range {
        Some(range) => {
            let start = byte_offset(text, range.start);
            let end = byte_offset(text, range.end);
            text.replace_range(start..end, &change.text);
        }
        None => *text = change.text.clone(),
    }
}

/// Byte offset of an LSP [`Position`] in `text` (line is 0-based, column is in
/// UTF-16 code units). Clamps past-the-end positions to the text length.
fn byte_offset(text: &str, position: Position) -> usize {
    let mut line_start = 0usize;
    if position.line > 0 {
        let mut current_line = 0u32;
        let mut found = false;
        for (index, character) in text.char_indices() {
            if character == '\n' {
                current_line += 1;
                if current_line == position.line {
                    line_start = index + 1;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            return text.len();
        }
    }

    let mut utf16 = 0u32;
    let mut offset = line_start;
    for character in text[line_start..].chars() {
        if utf16 >= position.character || character == '\n' {
            break;
        }
        utf16 += character.len_utf16() as u32;
        offset += character.len_utf8();
    }
    offset
}

fn position_in_range(position: Position, range: &Range) -> bool {
    let after_start =
        (position.line, position.character) >= (range.start.line, range.start.character);
    let before_end = (position.line, position.character) <= (range.end.line, range.end.character);
    after_start && before_end
}

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
}
