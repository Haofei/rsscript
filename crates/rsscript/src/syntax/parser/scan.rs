use super::expr::*;
use super::*;

pub(super) fn declaration_line_end(tokens: &[Token], start: usize) -> usize {
    if start >= tokens.len() {
        return start;
    }
    let line = tokens[start].span.line;
    (start..tokens.len())
        .find(|index| {
            tokens[*index].span.line > line || matches!(tokens[*index].kind, TokenKind::Eof)
        })
        .unwrap_or(tokens.len())
}

pub(super) fn function_signature_end(tokens: &[Token], start: usize) -> usize {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if depth == 0 && token.symbol("{") {
            return index;
        }
        if depth == 0 && token.symbol("}") {
            return index;
        }
        if index > start && depth == 0 && starts_top_level_item(tokens, index) {
            return index;
        }
        if token.symbol("(") || token.symbol("[") || token.symbol("<") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("]") || token.symbol(">") {
            depth = depth.saturating_sub(1);
        }
        if matches!(token.kind, TokenKind::Eof) {
            return index;
        }
    }
    tokens.len()
}

fn starts_top_level_item(tokens: &[Token], index: usize) -> bool {
    let Some(token) = tokens.get(index) else {
        return false;
    };
    if index > 0 && tokens[index - 1].span.line == token.span.line {
        return false;
    }
    token.is_ident_text("features")
        || token.is_ident_text("profile")
        || token.is_ident_text("class")
        || token.is_ident_text("struct")
        || token.is_ident_text("resource")
        || token.is_ident_text("fn")
        || token.is_ident_text("pub")
        || token.is_ident_text("async")
        || token.is_ident_text("native")
        || token.is_ident_text("protocol")
        || token.is_ident_text("impl")
}

pub(super) fn statement_end(tokens: &[Token], start: usize, limit: usize) -> usize {
    let mut line = tokens[start].span.line;
    let mut depth = 0usize;
    let mut angle_depth = 0usize;
    let mut postfix_continuation_line = None;
    for (index, token) in tokens.iter().enumerate().take(limit).skip(start + 1) {
        // A top-level `;` explicitly terminates the statement, letting several
        // statements share a line (`a; b; c`). `collect_statements` skips the
        // `;` itself on the next iteration since it is a trivia boundary.
        if depth == 0 && angle_depth == 0 && token.symbol(";") {
            return index;
        }
        if depth == 0 && angle_depth == 0 && token.span.line > line {
            if postfix_continuation_line == Some(token.span.line) {
                // Keep scanning the receiver-call segment that began with a
                // postfix token on this line, e.g. `}).ok()` or a next-line
                // `.map(...)` chain.
            } else if is_statement_postfix_token(token) {
                postfix_continuation_line = Some(token.span.line);
            } else if is_continuation_operator(token)
                || tokens
                    .get(index - 1)
                    .is_some_and(|prev| is_continuation_operator(prev))
            {
                // Binary-operator line continuation: a line that *begins* with a
                // binary operator (leading style) or *follows* a line ending in
                // one (trailing style) continues the current statement's
                // expression rather than starting a new statement. Absorb this
                // line by advancing the reference line. Only the unambiguous
                // operators in `is_continuation_operator` qualify (see there for
                // why `<`/`>`/`-`/`=`/`!` are excluded), so a wrapped chain can
                // never swallow the start of a genuinely new statement.
                line = token.span.line;
            } else if token.is_ident_text("else")
                && tokens
                    .get(index - 1)
                    .is_some_and(|previous| previous.symbol("}"))
            {
                // `return if condition { value } else { value }` is one
                // expression. `else` cannot start an ordinary statement, so
                // carrying the line forward here cannot merge two valid
                // statements.
                line = token.span.line;
            } else {
                return index;
            }
        }
        if token.symbol("(") || token.symbol("{") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        } else if depth == 0
            && token.symbol("<")
            && is_generic_angle_open(tokens, start, limit, index)
        {
            angle_depth += 1;
        } else if depth == 0 && angle_depth > 0 && token.symbol(">") {
            angle_depth -= 1;
        }
    }
    limit
}

fn is_statement_postfix_token(token: &Token) -> bool {
    token.symbol("?") || token.symbol(".")
}

/// Binary operators that make a statement-level expression continue across a
/// newline (SH-017): `|` `&` `+` `*` `/` `%` `^` (so `||`, `&&`, `+`, `*`, … may
/// wrap). The set is deliberately conservative so a wrapped chain can NEVER
/// absorb the start of a genuinely new statement:
/// - `<` / `>` are excluded (generic brackets / comparison).
/// - `-` is excluded (unary minus can legitimately start a statement).
/// - `=` is excluded (a dangling `let x =` must stay a *malformed statement*, not
///   silently swallow the next line — that would reintroduce the very
///   silent-merge footgun SH-017 fixes).
/// - `!` is excluded (a leading `!expr` is a valid statement start).
/// Consequently `==` / `!=` / `<=` / `>=` / `=` cannot wrap across lines — keep
/// those on one line.
fn is_continuation_operator(token: &Token) -> bool {
    token.symbol("|")
        || token.symbol("&")
        || token.symbol("+")
        || token.symbol("*")
        || token.symbol("/")
        || token.symbol("%")
        || token.symbol("^")
}

pub(super) fn find_control_body_open(
    tokens: &[Token],
    start: usize,
    limit: usize,
) -> Option<usize> {
    let mut depth = 0usize;
    let mut angle_depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(limit).skip(start + 1) {
        if depth == 0 && angle_depth == 0 && token.symbol("{") {
            return Some(index);
        }
        if token.symbol("(") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        } else if depth == 0
            && token.symbol("<")
            && is_generic_angle_open(tokens, start, limit, index)
        {
            angle_depth += 1;
        } else if depth == 0 && angle_depth > 0 && token.symbol(">") {
            angle_depth -= 1;
        }
    }
    None
}

pub(super) fn next_line_or_block_end(tokens: &[Token], start: usize, end: usize) -> usize {
    if start >= end {
        return end;
    }
    let line = tokens[start].span.line;
    (start..end)
        .find(|index| tokens[*index].span.line > line)
        .unwrap_or(end)
}

pub(super) fn find_top_level_symbol(
    tokens: &[Token],
    start: usize,
    end: usize,
    symbol: &str,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") || token.symbol("{") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token.symbol(symbol) {
            return Some(index);
        }
    }
    None
}

pub(super) fn find_top_level_ident(
    tokens: &[Token],
    start: usize,
    end: usize,
    ident: &str,
) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") || token.symbol("{") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token.is_ident_text(ident) {
            return Some(index);
        }
    }
    None
}

pub(super) fn trim_outer(tokens: &[Token], start: usize, end: usize) -> (usize, usize) {
    if start < end
        && tokens[start].symbol("(")
        && find_matching(tokens, start, "(", ")").is_some_and(|close| close + 1 == end)
    {
        (start + 1, end - 1)
    } else {
        (start, end)
    }
}

pub(super) fn skip_braced_block(tokens: &[Token], start: usize) -> Option<usize> {
    let open = (start..tokens.len()).find(|index| tokens[*index].symbol("{"))?;
    find_matching(tokens, open, "{", "}").map(|close| close + 1)
}

pub(super) fn skip_unknown_top_level(tokens: &[Token], start: usize) -> usize {
    let line_end = declaration_line_end(tokens, start);
    let block_end = (start..line_end)
        .find(|index| tokens[*index].symbol("{"))
        .map(|open| find_matching(tokens, open, "{", "}").map_or(tokens.len(), |close| close + 1));
    block_end.unwrap_or(line_end).max(start + 1)
}

pub(super) fn find_matching(
    tokens: &[Token],
    open: usize,
    open_symbol: &str,
    close_symbol: &str,
) -> Option<usize> {
    if !tokens
        .get(open)
        .is_some_and(|token| token.symbol(open_symbol))
    {
        return None;
    }
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        if token.symbol(open_symbol) {
            depth += 1;
        } else if token.symbol(close_symbol) {
            depth -= 1;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

pub(super) fn find_matching_open(
    tokens: &[Token],
    start: usize,
    close: usize,
    open_symbol: &str,
    close_symbol: &str,
) -> Option<usize> {
    let mut depth = 0usize;
    for index in (start..=close).rev() {
        let token = tokens.get(index)?;
        if token.symbol(close_symbol) {
            depth += 1;
        } else if token.symbol(open_symbol) {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(index);
            }
        }
    }
    None
}

pub(super) fn is_trivia_boundary(token: &Token) -> bool {
    token.symbol(",") || token.symbol(";")
}

pub(super) fn ident_name(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Ident(value) => Some(value),
        TokenKind::Keyword(value) => Some(value),
        _ => None,
    }
}

pub(super) fn tokens_to_source(tokens: &[Token], start: usize, end: usize) -> String {
    tokens
        .iter()
        .take(end)
        .skip(start)
        .map(Token::text)
        .collect::<Vec<_>>()
        .join("")
}
