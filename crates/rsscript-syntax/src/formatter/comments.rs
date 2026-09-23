//! Line comments for `rss fmt`.
//!
//! The formatter prints the AST, and the AST has no comments, so the formatter
//! reads them from the source separately and puts each back at the point of
//! the output that corresponds to where it was written. A comment is never
//! dropped: every one is emitted exactly once, and one the formatter found no
//! better place for is emitted at the next line boundary after its position.
//!
//! Placement rests on two facts per comment, both read from the token stream:
//!
//! * whether it is *trailing* — some token precedes it on its own line, so it
//!   comments that line (`let x = 1 // why`) — or *own-line*;
//! * its position relative to the start of the node the formatter is about to
//!   print. Before printing a statement, argument, arm, field, or item on a
//!   fresh line, the formatter flushes every pending comment written before
//!   that node's start: a trailing one goes back to the end of the previous
//!   output line, an own-line one on its own line at the current indent.
//!
//! The formatter prints top-level items out of source order (protocols first,
//! impls last), so each comment is also *owned* by one top-level unit — the
//! one it is written inside, the one it trails, or the one it leads — and is
//! only flushed while that unit is printed.

use std::collections::HashMap;

use crate::Span;
use crate::lexer::{Token, TokenKind};

/// A source position: 1-based line and column.
pub(super) type Pos = (usize, usize);

pub(super) const END: Pos = (usize::MAX, usize::MAX);

pub(super) fn pos(span: &Span) -> Pos {
    (span.line, span.column)
}

#[derive(Debug)]
struct Comment {
    pos: Pos,
    text: String,
    /// A token precedes the comment on its line.
    trailing: bool,
    /// The source has a blank line directly above this own-line comment.
    blank_before: bool,
    /// Index of the top-level unit this comment belongs to; `None` for a
    /// comment after the last unit's last token.
    owner: Option<usize>,
    emitted: bool,
}

#[derive(Debug, Default)]
pub(super) struct SourceComments {
    comments: Vec<Comment>,
    /// The matching closer of every `(`, `[`, and `{` token, by position.
    closers: HashMap<Pos, Pos>,
    /// Start positions of `(`, `[`, and `{` tokens, in source order.
    openers: Vec<Pos>,
    /// Top-level unit starts, in source order.
    units: Vec<Pos>,
}

impl SourceComments {
    pub(super) fn new(
        tokens: &[Token],
        comments: Vec<crate::lexer::LineComment>,
        unit_starts: &[Pos],
    ) -> Self {
        let mut token_positions = Vec::with_capacity(tokens.len());
        let mut token_lines = Vec::with_capacity(tokens.len());
        let mut modifiers = Vec::with_capacity(tokens.len());
        let mut significant = Vec::with_capacity(tokens.len());
        let mut closers = HashMap::new();
        let mut openers = Vec::new();
        let mut stack: Vec<(Pos, &str)> = Vec::new();
        for token in tokens {
            if matches!(token.kind, TokenKind::Eof) {
                continue;
            }
            let position = pos(&token.span);
            significant.push(token);
            token_positions.push(position);
            token_lines.push(position.0);
            modifiers.push(match &token.kind {
                TokenKind::Keyword(word) => matches!(*word, "pub" | "async" | "opaque"),
                TokenKind::Ident(word) => matches!(word.as_str(), "pub" | "async" | "opaque"),
                _ => false,
            });
            match &token.kind {
                TokenKind::Symbol(open @ ("(" | "[" | "{")) => {
                    openers.push(position);
                    stack.push((position, open));
                }
                TokenKind::Symbol(close @ (")" | "]" | "}")) => {
                    let expected = match *close {
                        ")" => "(",
                        "]" => "[",
                        _ => "{",
                    };
                    if let Some(index) = stack.iter().rposition(|(_, open)| *open == expected) {
                        let (open, _) = stack[index];
                        stack.truncate(index);
                        closers.insert(open, position);
                    }
                }
                _ => {}
            }
        }

        let mut units = unit_starts
            .iter()
            .map(|start| unit_start(&token_positions, &significant, &modifiers, *start))
            .collect::<Vec<_>>();
        units.sort_unstable();
        units.dedup();

        let mut previous_comment_line = 0;
        let comments = comments
            .into_iter()
            .map(|comment| {
                let position = pos(&comment.span);
                let before = token_positions.partition_point(|token| *token < position);
                let previous_token_line = before.checked_sub(1).map(|index| token_lines[index]);
                let trailing = previous_token_line == Some(position.0);
                let occupied = previous_token_line.unwrap_or(0).max(previous_comment_line);
                let blank_before = occupied > 0 && position.0 > occupied + 1;
                previous_comment_line = position.0;
                Comment {
                    pos: position,
                    text: comment.text,
                    trailing,
                    blank_before,
                    owner: owner(&units, &token_positions, &token_lines, position, trailing),
                    emitted: false,
                }
            })
            .collect();

        Self {
            comments,
            closers,
            openers,
            units,
        }
    }

    /// The index of the top-level unit whose AST span starts at `start`: the
    /// last unit starting at or before it (a unit's recorded start may be
    /// moved back over its modifiers, never past another unit).
    pub(super) fn unit_index(&self, start: Pos) -> Option<usize> {
        self.units
            .partition_point(|unit| *unit <= start)
            .checked_sub(1)
    }

    /// The position of the bracket that closes the opener at `open`, if `open`
    /// is a `(`, `[`, or `{` token.
    pub(super) fn closer(&self, open: Pos) -> Option<Pos> {
        self.closers.get(&open).copied()
    }

    /// The nearest opener of any kind before `position`.
    pub(super) fn opener_before(&self, position: Pos) -> Option<Pos> {
        let index = self.openers.partition_point(|open| *open < position);
        index.checked_sub(1).map(|index| self.openers[index])
    }

    /// Whether an unemitted comment lies strictly between `start` and `end`.
    pub(super) fn any_between(&self, start: Pos, end: Pos) -> bool {
        self.comments
            .iter()
            .any(|comment| !comment.emitted && comment.pos > start && comment.pos < end)
    }

    /// Take, in source order, every unemitted comment before `before` that the
    /// unit `owner` owns (every one, when `owner` is `None`).
    pub(super) fn take_before(
        &mut self,
        before: Pos,
        owner: Option<usize>,
    ) -> Vec<(String, bool, bool)> {
        let mut taken = Vec::new();
        for comment in &mut self.comments {
            if comment.emitted || comment.pos >= before {
                continue;
            }
            if owner.is_some() && comment.owner != owner {
                continue;
            }
            comment.emitted = true;
            taken.push((comment.text.clone(), comment.trailing, comment.blank_before));
        }
        taken
    }

    pub(super) fn is_empty(&self) -> bool {
        self.comments.iter().all(|comment| comment.emitted)
    }
}

/// Move a top-level unit's recorded start back over what the AST span may not
/// include — modifiers (`pub fn`, `pub async fn`, `pub opaque struct`) and a
/// `#lower_name("...")` attribute — so a comment above them leads the unit
/// rather than trailing the one before.
fn unit_start(
    token_positions: &[Pos],
    significant: &[&Token],
    modifiers: &[bool],
    start: Pos,
) -> Pos {
    let mut index = token_positions.partition_point(|token| *token < start);
    while let Some(previous) = index.checked_sub(1) {
        if modifiers[previous] {
            index = previous;
            continue;
        }
        // `# name ( ... )`
        if significant[previous].symbol(")")
            && let Some(open) = (0..previous).rev().find(|at| significant[*at].symbol("("))
            && open >= 2
            && matches!(significant[open - 1].kind, TokenKind::Ident(_))
            && significant[open - 2].symbol("#")
        {
            index = open - 2;
            continue;
        }
        break;
    }
    token_positions.get(index).copied().unwrap_or(start)
}

/// The unit a comment belongs to.
///
/// A comment between two unit starts is inside the earlier unit if a token of
/// that unit follows it, or if it trails that unit's last token; otherwise it
/// leads the later unit. Before the first unit it leads the first; after the
/// last unit's last token it belongs to no unit and is printed at the end.
fn owner(
    units: &[Pos],
    tokens: &[Pos],
    token_lines: &[usize],
    position: Pos,
    trailing: bool,
) -> Option<usize> {
    if units.is_empty() {
        return None;
    }
    let after = units.partition_point(|unit| *unit <= position);
    let Some(unit) = after.checked_sub(1) else {
        return Some(0);
    };
    let boundary = units.get(unit + 1).copied();
    let last_token_index = match boundary {
        Some(next) => tokens.partition_point(|token| *token < next),
        None => tokens.len(),
    }
    .checked_sub(1);
    let Some(last_token_index) = last_token_index else {
        return Some(unit);
    };
    let last_token = tokens[last_token_index];
    if position < last_token || (trailing && token_lines[last_token_index] == position.0) {
        return Some(unit);
    }
    // Own-line after the unit's last token: it leads the next unit, or,
    // after the last unit, belongs to none and is printed at the end.
    boundary.map(|_| unit + 1)
}
