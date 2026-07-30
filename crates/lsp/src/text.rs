//! UTF-16 position, range, and incremental text-edit utilities.

use rsscript::Span;
use tower_lsp::lsp_types::*;

/// Map a checker [`Span`] (1-based line/column counted in `char`s) to an LSP
/// [`Range`] (0-based line, UTF-16 code units).
pub(crate) fn span_to_range(source: &str, span: &Span) -> Range {
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

pub(crate) fn full_document_range(text: &str) -> Range {
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
pub(crate) fn char_position(source: &str, position: Position) -> (usize, usize) {
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
pub(crate) fn apply_change(text: &mut String, change: &TextDocumentContentChangeEvent) -> bool {
    match change.range {
        Some(range) => {
            let Some(start) = checked_byte_offset(text, range.start) else {
                return false;
            };
            let Some(end) = checked_byte_offset(text, range.end) else {
                return false;
            };
            if start > end {
                return false;
            }
            text.replace_range(start..end, &change.text);
        }
        None => *text = change.text.clone(),
    }
    true
}

pub(crate) fn checked_byte_offset(text: &str, position: Position) -> Option<usize> {
    let mut line_start = 0usize;
    for _ in 0..position.line {
        let newline = text[line_start..].find('\n')?;
        line_start += newline + 1;
    }

    let mut utf16 = 0u32;
    let mut offset = line_start;
    for character in text[line_start..].chars() {
        if utf16 == position.character {
            return Some(offset);
        }
        if character == '\n' {
            return None;
        }
        utf16 = utf16.checked_add(character.len_utf16() as u32)?;
        if utf16 > position.character {
            return None;
        }
        offset += character.len_utf8();
    }
    (utf16 == position.character).then_some(offset)
}

/// Byte offset of an LSP [`Position`] in `text` (line is 0-based, column is in
/// UTF-16 code units). Clamps past-the-end positions to the text length.
pub(crate) fn byte_offset(text: &str, position: Position) -> usize {
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

pub(crate) fn position_in_range(position: Position, range: &Range) -> bool {
    let after_start =
        (position.line, position.character) >= (range.start.line, range.start.character);
    let before_end = (position.line, position.character) <= (range.end.line, range.end.character);
    after_start && before_end
}
