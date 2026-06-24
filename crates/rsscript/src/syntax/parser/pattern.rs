use super::*;
use super::expr::*;
use super::items::*;
use super::scan::*;
use super::stmt::*;
use super::types::*;

pub(super) fn parse_match_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    // match <value> { arms... }
    // The arms block is the top-level brace group whose close ends the expression.
    let brace_start = find_match_expr_arms_open(tokens, start + 1, end)?;
    let Some(close) = find_matching(tokens, brace_start, "{", "}") else {
        return None;
    };
    // The closing brace must be the end of the expression
    if close + 1 != end {
        return None;
    }
    let (scrutinee_effect, value_start) =
        parse_match_scrutinee_effect(tokens, start + 1, brace_start);
    let value = parse_expr(tokens, value_start, brace_start)?;
    let parsed_arms = parse_match_arms(tokens, brace_start + 1, close);
    if parsed_arms.arms.is_empty() {
        return None;
    }
    Some(Expr::Match {
        value: Box::new(value),
        scrutinee_effect,
        arms: parsed_arms.arms,
        span: tokens[start].span.clone(),
    })
}

pub(super) fn parse_match_scrutinee_effect(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> (Option<DataEffect>, usize) {
    if start < end
        && let Some(effect) = parse_data_effect(tokens.get(start))
    {
        (Some(effect), start + 1)
    } else {
        (None, start)
    }
}

fn find_match_expr_arms_open(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") {
            paren_depth += 1;
        } else if token.symbol(")") {
            paren_depth = paren_depth.saturating_sub(1);
        } else if token.symbol("[") {
            bracket_depth += 1;
        } else if token.symbol("]") {
            bracket_depth = bracket_depth.saturating_sub(1);
        } else if paren_depth == 0 && bracket_depth == 0 && token.symbol("{") {
            if find_matching(tokens, index, "{", "}").is_some_and(|close| close + 1 == end) {
                return Some(index);
            }
        }
    }
    None
}

pub(super) struct ParsedMatchArms {
    pub(super) arms: Vec<MatchArm>,
    pub(super) malformed_spans: Vec<crate::diagnostic::Span>,
}

pub(super) fn parse_match_arms(tokens: &[Token], start: usize, end: usize) -> ParsedMatchArms {
    let mut arms = Vec::new();
    let mut malformed_spans = Vec::new();
    let mut index = start;
    while index < end {
        while index < end && is_trivia_boundary(&tokens[index]) {
            index += 1;
        }
        if index >= end {
            break;
        }
        let line_end = next_line_or_block_end(tokens, index, end);
        let Some(arrow) = find_top_level_symbol(tokens, index, line_end, "=>") else {
            malformed_spans.push(tokens[index].span.clone());
            index = line_end.max(index + 1);
            continue;
        };
        let (pattern_end, guard) = split_match_pattern_guard(tokens, index, arrow).map_or(
            (arrow, None),
            |(pattern_end, guard_start)| {
                (
                    pattern_end,
                    parse_expr(tokens, guard_start, arrow)
                        .or_else(|| Some(Expr::Unknown(tokens[guard_start].span.clone()))),
                )
            },
        );
        let pattern = if let Some(pattern) = parse_match_pattern(tokens, index, pattern_end) {
            pattern
        } else {
            malformed_spans.push(tokens[index].span.clone());
            MatchPattern::Wildcard(tokens[index].span.clone())
        };
        let body_start = arrow + 1;
        let (body, next) = if tokens
            .get(body_start)
            .is_some_and(|token| token.symbol("{"))
        {
            let Some(body_close) = find_matching(tokens, body_start, "{", "}") else {
                malformed_spans.push(tokens[body_start].span.clone());
                break;
            };
            (parse_block(tokens, body_start, body_close), body_close + 1)
        } else {
            if body_start >= end {
                malformed_spans.push(tokens[arrow].span.clone());
                break;
            }
            let body_end = next_line_or_block_end(tokens, body_start, end);
            let (statement, next) = parse_stmt(tokens, body_start, body_end);
            (
                Block {
                    statements: vec![statement],
                    span: tokens[body_start].span.clone(),
                },
                next,
            )
        };
        arms.push(MatchArm {
            pattern,
            guard,
            body,
            span: tokens[index].span.clone(),
        });
        index = next;
    }
    ParsedMatchArms {
        arms,
        malformed_spans,
    }
}

fn split_match_pattern_guard(tokens: &[Token], start: usize, end: usize) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") || token.symbol("{") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token.is_ident_text("if") {
            return Some((index, index + 1));
        }
    }
    None
}

/// Parse a pattern head name: a bare identifier (`ADD`) or a module-qualified
/// dotted name (`ops.ADD`, `a.b.Variant`). Returns the joined name and the token
/// index just past it.
fn parse_dotted_pattern_name(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Option<(String, usize)> {
    let mut name = ident_name(tokens.get(start)?)?.to_string();
    let mut index = start + 1;
    while index + 1 < end
        && tokens[index].symbol(".")
        && let Some(segment) = ident_name(&tokens[index + 1])
    {
        name.push('.');
        name.push_str(segment);
        index += 2;
    }
    Some((name, index))
}

pub(super) fn parse_match_pattern(tokens: &[Token], start: usize, end: usize) -> Option<MatchPattern> {
    // A tuple pattern `(p0, p1, ..)` desugars to the synthetic `__TupleN` struct
    // pattern `__TupleN { item0: p0, item1: p1, .. }`. Checked before `trim_outer`
    // strips the parens as grouping.
    if let Some(tuple) = parse_tuple_pattern(tokens, start, end) {
        return Some(tuple);
    }
    if let Some(list) = parse_list_pattern(tokens, start, end) {
        return Some(list);
    }
    let (start, end) = trim_outer(tokens, start, end);
    if start >= end {
        return None;
    }
    if start + 1 == end {
        match &tokens[start].kind {
            TokenKind::Number(value) => {
                return Some(MatchPattern::Literal {
                    value: MatchLiteral::Int(value.clone()),
                    span: tokens[start].span.clone(),
                });
            }
            TokenKind::String(value) => {
                return Some(MatchPattern::Literal {
                    value: MatchLiteral::String(value.clone()),
                    span: tokens[start].span.clone(),
                });
            }
            TokenKind::Ident(value) if matches!(value.as_str(), "true" | "false") => {
                return Some(MatchPattern::Literal {
                    value: MatchLiteral::Bool(*value == "true"),
                    span: tokens[start].span.clone(),
                });
            }
            TokenKind::Keyword(value) if matches!(*value, "true" | "false") => {
                return Some(MatchPattern::Literal {
                    value: MatchLiteral::Bool(*value == "true"),
                    span: tokens[start].span.clone(),
                });
            }
            _ => {}
        }
    }
    // The pattern head may be a bare name (`ADD`, `Some`) or a module-qualified
    // variant/type (`ops.ADD`, `ops.Pair`). `head_end` is the token index just
    // past the (possibly dotted) name.
    let (name, head_end) = parse_dotted_pattern_name(tokens, start, end)?;
    if name == "_" {
        return Some(MatchPattern::Wildcard(tokens[start].span.clone()));
    }
    if tokens.get(head_end).is_some_and(|token| token.symbol("{")) {
        let close = find_matching(tokens, head_end, "{", "}")?;
        if close + 1 != end {
            return None;
        }
        let (fields, has_rest) = parse_match_field_patterns(tokens, head_end + 1, close)?;
        return Some(MatchPattern::Struct {
            name,
            fields,
            has_rest,
            span: tokens[start].span.clone(),
        });
    }
    let binding = if tokens.get(head_end).is_some_and(|token| token.symbol("(")) {
        let close = find_matching(tokens, head_end, "(", ")")?;
        if close + 1 != end {
            return None;
        }
        if head_end + 1 == close {
            None
        } else if head_end + 2 == close && tokens[head_end + 1].is_ident_text("_") {
            Some(Box::new(MatchPattern::Wildcard(
                tokens[head_end + 1].span.clone(),
            )))
        } else if head_end + 2 == close {
            parse_single_payload_pattern(tokens, head_end + 1)
        } else {
            parse_match_pattern(tokens, head_end + 1, close).map(Box::new)
        }
    } else if head_end == end {
        None
    } else {
        return None;
    };
    Some(MatchPattern::Variant {
        name,
        binding,
        span: tokens[start].span.clone(),
    })
}

/// Parse `(p0, p1, ..)` as the synthetic tuple struct pattern. Returns `None`
/// for anything that is not a `(`...`)` wrapping at least two comma-separated
/// element patterns (a single parenthesised pattern is grouping, not a tuple).
fn parse_tuple_pattern(tokens: &[Token], start: usize, end: usize) -> Option<MatchPattern> {
    if !tokens.get(start)?.symbol("(") || !tokens.get(end.checked_sub(1)?)?.symbol(")") {
        return None;
    }
    let close = find_matching(tokens, start, "(", ")")?;
    if close + 1 != end {
        return None;
    }
    let ranges: Vec<_> = split_param_ranges(tokens, start + 1, close)
        .into_iter()
        .filter(|range| range.empty_span.is_none())
        .collect();
    if ranges.len() < 2 {
        return None;
    }
    let mut fields = Vec::with_capacity(ranges.len());
    for (index, range) in ranges.iter().enumerate() {
        // A single-token element distinguishes binding/literal/constructor exactly
        // as a struct field RHS does; multi-token elements are nested patterns.
        let element = if range.start + 1 == range.end {
            *parse_single_payload_pattern(tokens, range.start)?
        } else {
            parse_match_pattern(tokens, range.start, range.end)?
        };
        let name = format!("item{index}");
        let span = tokens[range.start].span.clone();
        let field = match element {
            MatchPattern::Binding { name: binding, .. } => MatchFieldPattern {
                name,
                binding: Some(binding),
                pattern: None,
                effect: None,
                ignored: false,
                span,
            },
            MatchPattern::Wildcard(_) => MatchFieldPattern {
                name,
                binding: None,
                pattern: None,
                effect: None,
                ignored: true,
                span,
            },
            other => MatchFieldPattern {
                name,
                binding: None,
                pattern: Some(Box::new(other)),
                effect: None,
                ignored: false,
                span,
            },
        };
        fields.push(field);
    }
    Some(MatchPattern::Struct {
        name: format!("__Tuple{}", ranges.len()),
        fields,
        has_rest: false,
        span: tokens[start].span.clone(),
    })
}

/// Parse `[p0, p1, ..]` list slice patterns: `[]`, `[a, b]`, `[first, ..rest]`,
/// `[..init, last]`, `[a, ..mid, z]`. At most one `..`/`..name` rest segment is
/// permitted; elements before it form the prefix, elements after it the suffix.
/// Returns `None` for anything not wrapped in a single matched `[` ... `]`.
fn parse_list_pattern(tokens: &[Token], start: usize, end: usize) -> Option<MatchPattern> {
    if !tokens.get(start)?.symbol("[") || !tokens.get(end.checked_sub(1)?)?.symbol("]") {
        return None;
    }
    let close = find_matching(tokens, start, "[", "]")?;
    if close + 1 != end {
        return None;
    }
    let span = tokens[start].span.clone();
    let mut prefix = Vec::new();
    let mut suffix = Vec::new();
    let mut rest: Option<Option<String>> = None;
    for range in split_param_ranges(tokens, start + 1, close) {
        if range.empty_span.is_some() {
            return None;
        }
        if let Some(rest_binding) = parse_list_rest_segment(tokens, range.start, range.end) {
            if rest.is_some() {
                return None;
            }
            rest = Some(rest_binding);
            continue;
        }
        let element = if range.start + 1 == range.end {
            *parse_single_payload_pattern(tokens, range.start)?
        } else {
            parse_match_pattern(tokens, range.start, range.end)?
        };
        if rest.is_none() {
            prefix.push(element);
        } else {
            suffix.push(element);
        }
    }
    Some(MatchPattern::List {
        prefix,
        rest,
        suffix,
        span,
    })
}

/// If `[start, end)` is a `..` / `..name` rest segment, return `Some(binding)`
/// where `binding` is `None` for an ignored `..` (or `.._`) and `Some(name)`
/// for `..name`. Returns `None` (the outer option) when the range is an ordinary
/// element pattern. `..` may tokenise as one `..` token or two `.` tokens.
fn parse_list_rest_segment(tokens: &[Token], start: usize, end: usize) -> Option<Option<String>> {
    let dots_end = if tokens.get(start)?.symbol("..") {
        start + 1
    } else if tokens.get(start)?.symbol(".") && tokens.get(start + 1).is_some_and(|t| t.symbol("."))
    {
        start + 2
    } else {
        return None;
    };
    if dots_end == end {
        return Some(None);
    }
    if dots_end + 1 == end {
        let name = ident_name(tokens.get(dots_end)?)?.to_string();
        if name == "_" {
            return Some(None);
        }
        return Some(Some(name));
    }
    None
}

fn parse_match_field_patterns(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Option<(Vec<MatchFieldPattern>, bool)> {
    let mut fields = Vec::new();
    let mut has_rest = false;
    for range in split_param_ranges(tokens, start, end) {
        if range.empty_span.is_some() {
            continue;
        }
        if (range.start + 1 == range.end && tokens[range.start].symbol(".."))
            || (range.start + 2 == range.end
                && tokens[range.start].symbol(".")
                && tokens[range.start + 1].symbol("."))
        {
            has_rest = true;
            continue;
        }
        let name = ident_name(tokens.get(range.start)?)?.to_string();
        if name == "_" {
            return None;
        }
        if let Some(colon) = find_top_level_symbol(tokens, range.start, range.end, ":") {
            if colon != range.start + 1 {
                return None;
            }
            let rhs_start = colon + 1;
            if rhs_start >= range.end {
                return None;
            }
            if rhs_start + 1 == range.end && tokens[rhs_start].is_ident_text("_") {
                fields.push(MatchFieldPattern {
                    name,
                    binding: None,
                    pattern: None,
                    effect: None,
                    ignored: true,
                    span: tokens[range.start].span.clone(),
                });
                continue;
            }
            let (effect, binding_start) =
                if let Some(effect) = parse_data_effect(tokens.get(rhs_start)) {
                    (Some(effect), rhs_start + 1)
                } else {
                    (None, rhs_start)
                };
            let (binding, pattern, ignored) = if binding_start + 1 == range.end {
                if tokens[binding_start].is_ident_text("_") {
                    (None, None, true)
                } else if let Some(pattern) =
                    parse_single_literal_or_constructor_pattern(tokens, binding_start)
                {
                    (None, Some(pattern), false)
                } else {
                    let binding = ident_name(tokens.get(binding_start)?)?.to_string();
                    (Some(binding), None, false)
                }
            } else {
                (
                    None,
                    parse_match_pattern(tokens, binding_start, range.end).map(Box::new),
                    false,
                )
            };
            if pattern.is_none() && binding.is_none() && !ignored {
                return None;
            }
            fields.push(MatchFieldPattern {
                name,
                binding,
                pattern,
                effect,
                ignored,
                span: tokens[range.start].span.clone(),
            });
        } else {
            if range.start + 1 != range.end {
                return None;
            }
            fields.push(MatchFieldPattern {
                binding: Some(name.clone()),
                pattern: None,
                name,
                effect: None,
                ignored: false,
                span: tokens[range.start].span.clone(),
            });
        }
    }
    Some((fields, has_rest))
}

fn parse_single_payload_pattern(tokens: &[Token], index: usize) -> Option<Box<MatchPattern>> {
    if tokens[index].is_ident_text("_") {
        return Some(Box::new(MatchPattern::Wildcard(tokens[index].span.clone())));
    }
    if let Some(pattern) = parse_single_literal_or_constructor_pattern(tokens, index) {
        return Some(pattern);
    }
    ident_name(&tokens[index])
        .filter(|binding| *binding != "_")
        .map(|binding| {
            Box::new(MatchPattern::Binding {
                name: binding.to_string(),
                span: tokens[index].span.clone(),
            })
        })
}

fn parse_single_literal_or_constructor_pattern(
    tokens: &[Token],
    index: usize,
) -> Option<Box<MatchPattern>> {
    match &tokens[index].kind {
        TokenKind::Number(_) | TokenKind::String(_) => {
            parse_match_pattern(tokens, index, index + 1).map(Box::new)
        }
        TokenKind::Ident(value) if matches!(value.as_str(), "true" | "false") => {
            parse_match_pattern(tokens, index, index + 1).map(Box::new)
        }
        TokenKind::Keyword(value) if matches!(*value, "true" | "false") => {
            parse_match_pattern(tokens, index, index + 1).map(Box::new)
        }
        _ => {
            let name = ident_name(&tokens[index])?;
            if starts_like_constructor(name) {
                Some(Box::new(MatchPattern::Variant {
                    name: name.to_string(),
                    binding: None,
                    span: tokens[index].span.clone(),
                }))
            } else {
                None
            }
        }
    }
}

fn starts_like_constructor(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

