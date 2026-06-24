use super::*;
use super::expr::*;
use super::items::*;
use super::pattern::*;
use super::scan::*;
use super::types::*;

pub(super) fn parse_block(tokens: &[Token], open: usize, close: usize) -> Block {
    Block {
        statements: collect_statements(tokens, open + 1, close),
        span: tokens[open].span.clone(),
    }
}

/// Collect statements in `[start, close)`. Handles the scoped-view desugar:
/// `view name = expr` followed by the rest of the block becomes
/// `with expr as name { <rest> }`, so a borrowed view's scope ends at the
/// enclosing block (spec §20.2-1) and it reuses every `with`-lease rule
/// (no escape / no managed capture) without a separate code path.
pub(super) fn collect_statements(tokens: &[Token], mut index: usize, close: usize) -> Vec<Stmt> {
    let mut statements = Vec::new();
    while index < close {
        if tokens[index].symbol("}") {
            break;
        }
        if matches!(tokens[index].kind, TokenKind::Eof) {
            break;
        }
        if is_trivia_boundary(&tokens[index]) {
            index += 1;
            continue;
        }

        if is_view_binding(tokens, index, close) {
            let span = tokens[index].span.clone();
            let (binding, resource, after) = parse_view_header(tokens, index, close);
            // The remaining statements of the block are the view's scope.
            let body = Block {
                statements: collect_statements(tokens, after, close),
                span: span.clone(),
            };
            statements.push(Stmt::With(WithStmt {
                resource: resource.unwrap_or_else(|| Expr::Unknown(span.clone())),
                binding,
                body,
                span,
            }));
            break;
        }

        let (statement, next) = parse_stmt(tokens, index, close);
        statements.push(statement);
        index = next.max(index + 1);
    }
    statements
}

/// `view <ident> = ...` — a scoped-view binding (distinct from `view` used as an
/// ordinary identifier, which is never followed by `<ident> =`).
fn is_view_binding(tokens: &[Token], start: usize, limit: usize) -> bool {
    tokens[start].is_ident_text("view")
        && tokens
            .get(start + 1)
            .and_then(ident_name)
            .is_some_and(|name| name != "=")
        && tokens.get(start + 2).is_some_and(|token| token.symbol("="))
        && start + 2 < limit
}

/// Parse the `view <name> = <expr>` header, returning the binding name, the
/// resource expression, and the index just past the statement.
fn parse_view_header(
    tokens: &[Token],
    start: usize,
    limit: usize,
) -> (String, Option<Expr>, usize) {
    let name = tokens
        .get(start + 1)
        .and_then(ident_name)
        .unwrap_or("")
        .to_string();
    let end = statement_end(tokens, start, limit);
    let equals = (start + 2..end).find(|index| tokens[*index].symbol("="));
    let resource = equals.and_then(|equals| parse_expr(tokens, equals + 1, end));
    (name, resource, end)
}

pub(super) fn parse_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    // async let x = expr
    if tokens[start].is_ident_text("async")
        && tokens
            .get(start + 1)
            .is_some_and(|t| t.is_ident_text("let"))
    {
        return parse_async_let_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("let") || tokens[start].is_ident_text("local") {
        return parse_let_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("return") {
        return parse_return_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("with") {
        return parse_with_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("if") {
        return parse_if_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("while") || tokens[start].is_ident_text("loop") {
        return parse_loop_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("match") {
        return parse_match_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("await")
        && tokens
            .get(start + 1)
            .is_some_and(|token| token.is_ident_text("for"))
    {
        return parse_for_stmt(tokens, start + 1, limit, true);
    }
    if tokens[start].is_ident_text("for") {
        return parse_for_stmt(tokens, start, limit, false);
    }
    if tokens[start].is_ident_text("task_group") {
        return parse_task_group_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("select") {
        return parse_select_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("break") {
        return (
            Stmt::Break(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    }
    if tokens[start].is_ident_text("continue") {
        return (
            Stmt::Continue(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    }

    let end = statement_end(tokens, start, limit);
    if let Some(statement) = try_parse_assign_stmt(tokens, start, end) {
        return (statement, end);
    }
    let statement = parse_expr(tokens, start, end)
        .map_or_else(|| Stmt::Unknown(tokens[start].span.clone()), Stmt::Expr);
    (statement, end)
}

/// Parse a controlled assignment `<place> = <expr>`. Recognizes a standalone
/// `=` at bracket depth 0 — one that is not part of `==`, `!=`, `<=`, or `>=`
/// (the lexer emits those as separate `=` tokens) — and splits the statement
/// into a place target and a value. Returns `None` when there is no such `=`,
/// so ordinary expression statements fall through unchanged.
fn try_parse_assign_stmt(tokens: &[Token], start: usize, end: usize) -> Option<Stmt> {
    let mut depth = 0i32;
    let mut assign_index = None;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") || token.symbol("[") || token.symbol("{") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("]") || token.symbol("}") {
            depth -= 1;
        } else if depth == 0 && token.symbol("=") {
            let prev_joins = index
                .checked_sub(1)
                .and_then(|i| tokens.get(i))
                .is_some_and(|t| t.symbol("=") || t.symbol("!") || t.symbol("<") || t.symbol(">"));
            let next_joins = tokens.get(index + 1).is_some_and(|t| t.symbol("="));
            if !prev_joins && !next_joins {
                assign_index = Some(index);
                break;
            }
        }
    }
    let equals = assign_index?;
    let target = parse_expr(tokens, start, equals)?;
    let value = parse_expr(tokens, equals + 1, end)?;
    Some(Stmt::Assign(AssignStmt {
        target,
        value,
        span: tokens[start].span.clone(),
    }))
}

// let Some(binding) = expr else { diverging-block }
// let Ok(binding) = expr else { diverging-block }
fn try_parse_let_else(tokens: &[Token], start: usize, limit: usize) -> Option<(Stmt, usize)> {
    // start should be at `let` or `local`
    if !tokens[start].is_ident_text("let") && !tokens[start].is_ident_text("local") {
        return None;
    }
    let pattern_start = start + 1;
    // Check for variant pattern: Some(...) or Ok(...) or None or Err(...)
    let variant_name = ident_name(tokens.get(pattern_start)?)?;
    if !matches!(variant_name, "Some" | "Ok" | "None" | "Err") {
        return None;
    }
    // Parse binding inside parens (if any)
    let binding_name;
    let after_pattern;
    if tokens.get(pattern_start + 1)?.symbol("(") {
        let close = find_matching(tokens, pattern_start + 1, "(", ")")?;
        binding_name = ident_name(tokens.get(pattern_start + 2)?)
            .unwrap_or("")
            .to_string();
        after_pattern = close + 1;
    } else {
        binding_name = String::new();
        after_pattern = pattern_start + 1;
    }
    // Expect `=`
    if !tokens.get(after_pattern)?.symbol("=") {
        return None;
    }
    // Find `else` keyword before a `{`
    let mut else_pos = None;
    let mut i = after_pattern + 1;
    while i < limit {
        if tokens[i].is_ident_text("else") {
            else_pos = Some(i);
            break;
        }
        if tokens[i].symbol("{") {
            // Could be the else block's opening brace - check if preceded by else
            break;
        }
        i += 1;
    }
    let else_pos = else_pos?;
    // Parse expression between `=` and `else`
    let value = parse_expr(tokens, after_pattern + 1, else_pos)?;
    // Parse else block
    let open = (else_pos + 1..limit).find(|idx| tokens[*idx].symbol("{"))?;
    let close = find_matching(tokens, open, "{", "}")?;
    let else_body = parse_block(tokens, open + 1, close);
    let pattern = MatchPattern::Variant {
        name: variant_name.to_string(),
        binding: if binding_name.is_empty() {
            None
        } else {
            Some(Box::new(MatchPattern::Binding {
                name: binding_name.clone(),
                span: tokens[pattern_start + 2].span.clone(),
            }))
        },
        span: tokens[pattern_start].span.clone(),
    };
    Some((
        Stmt::LetElse(LetElseStmt {
            pattern,
            value,
            else_body,
            binding_name,
            span: tokens[start].span.clone(),
        }),
        close + 1,
    ))
}

fn parse_let_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    // Try to parse `let Some(binding) = expr else { ... }` or `let Ok(binding) = expr else { ... }`
    if let Some(result) = try_parse_let_else(tokens, start, limit) {
        return result;
    }

    let kind = if tokens[start].is_ident_text("local") {
        LetKind::Local
    } else {
        LetKind::Managed
    };
    // `let mut name = expr` declares a reassignable local. `mut` is only a
    // modifier on `let`; it shifts the name/annotation/value one token right.
    let is_mut = kind == LetKind::Managed
        && tokens
            .get(start + 1)
            .is_some_and(|t| t.is_ident_text("mut"));
    let name_index = if is_mut { start + 2 } else { start + 1 };
    // `let (a, b) = expr` destructures a tuple. The element names are recorded on
    // the binding and expanded into a temporary plus per-element `let`s by the
    // tuple desugar.
    if tokens
        .get(name_index)
        .is_some_and(|token| token.symbol("("))
    {
        return parse_let_tuple_stmt(tokens, start, kind, name_index, limit);
    }
    let parsed_name = tokens.get(name_index).and_then(ident_name);
    let name = parsed_name.unwrap_or("").to_string();
    let end = statement_end(tokens, start, limit);
    let equals = (name_index + 1..end).find(|index| tokens[*index].symbol("="));
    let annotation_end = equals.unwrap_or(end);
    let colon = (name_index + 1..annotation_end).find(|index| tokens[*index].symbol(":"));
    let type_annotation = colon.and_then(|colon| parse_type_ref(tokens, colon + 1, annotation_end));
    let value = equals.and_then(|equals| parse_expr(tokens, equals + 1, end));
    let malformed = parsed_name.is_none()
        || (equals.is_some() && value.is_none())
        || (colon.is_some() && type_annotation.is_none());

    (
        Stmt::Let(LetStmt {
            kind,
            name,
            type_annotation,
            value,
            is_async: false,
            is_mut,
            destructure: None,
            malformed,
            span: tokens[start].span.clone(),
        }),
        end,
    )
}

/// Parse `let (a, b) = expr` / `local (a, b) = expr`. `name_index` points at the
/// opening `(`. Records the element names in `LetStmt::destructure`; the value is
/// the right-hand expression.
fn parse_let_tuple_stmt(
    tokens: &[Token],
    start: usize,
    kind: LetKind,
    name_index: usize,
    limit: usize,
) -> (Stmt, usize) {
    let end = statement_end(tokens, start, limit);
    let close = find_matching(tokens, name_index, "(", ")");
    let names = close.and_then(|close| {
        let ranges = split_param_ranges(tokens, name_index + 1, close);
        let mut names = Vec::new();
        for range in ranges {
            if range.empty_span.is_some() {
                continue;
            }
            // Each element must be a single binding identifier (or `_`).
            if range.start + 1 != range.end {
                return None;
            }
            names.push(ident_name(&tokens[range.start])?.to_string());
        }
        (names.len() >= 2).then_some(names)
    });
    let equals = close.and_then(|close| (close + 1..end).find(|index| tokens[*index].symbol("=")));
    let value = equals.and_then(|equals| parse_expr(tokens, equals + 1, end));
    let malformed = names.is_none() || value.is_none();

    (
        Stmt::Let(LetStmt {
            kind,
            name: String::new(),
            type_annotation: None,
            value,
            is_async: false,
            is_mut: false,
            destructure: names,
            malformed,
            span: tokens[start].span.clone(),
        }),
        end,
    )
}

fn parse_async_let_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    // async let name = expr
    // start is at "async", start+1 is "let"
    let let_start = start + 1;
    let parsed_name = tokens.get(let_start + 1).and_then(ident_name);
    let name = parsed_name.unwrap_or("").to_string();
    let end = statement_end(tokens, start, limit);
    let equals = (let_start + 2..end).find(|index| tokens[*index].symbol("="));
    let value = equals.and_then(|equals| parse_expr(tokens, equals + 1, end));
    let malformed = parsed_name.is_none() || equals.is_none() || value.is_none();

    (
        Stmt::Let(LetStmt {
            kind: LetKind::Managed,
            name,
            type_annotation: None,
            value,
            is_async: true,
            is_mut: false,
            destructure: None,
            malformed,
            span: tokens[start].span.clone(),
        }),
        end,
    )
}

fn parse_return_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    let end = statement_end(tokens, start, limit);
    let value = parse_expr(tokens, start + 1, end);
    (
        Stmt::Return(ReturnStmt {
            value,
            span: tokens[start].span.clone(),
        }),
        end,
    )
}

fn parse_with_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    let Some(as_index) = (start + 1..limit).find(|index| tokens[*index].is_ident_text("as")) else {
        return (
            Stmt::MalformedWith(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let Some(open) = (as_index + 1..limit).find(|index| tokens[*index].symbol("{")) else {
        return (
            Stmt::MalformedWith(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let Some(close) = find_matching(tokens, open, "{", "}") else {
        return (Stmt::MalformedWith(tokens[start].span.clone()), limit);
    };
    let resource = parse_expr(tokens, start + 1, as_index)
        .unwrap_or_else(|| Expr::Unknown(tokens[start].span.clone()));
    let Some(binding) = tokens.get(as_index + 1).and_then(ident_name) else {
        return (Stmt::MalformedWith(tokens[start].span.clone()), close + 1);
    };
    if as_index + 2 != open {
        return (Stmt::MalformedWith(tokens[start].span.clone()), close + 1);
    }
    let body = parse_block(tokens, open, close);

    (
        Stmt::With(WithStmt {
            resource,
            binding: binding.to_string(),
            body,
            span: tokens[start].span.clone(),
        }),
        close + 1,
    )
}

fn parse_if_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    if let Some(parsed) = parse_if_is_stmt(tokens, start, limit) {
        return parsed;
    }
    let Some(open) = find_control_body_open(tokens, start, limit) else {
        return (
            Stmt::MalformedIf(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let Some(close) = find_matching(tokens, open, "{", "}") else {
        return (Stmt::MalformedIf(tokens[start].span.clone()), limit);
    };
    if tokens
        .get(start + 1)
        .is_some_and(|token| token.is_ident_text("let"))
    {
        return parse_if_let_stmt(tokens, start, open, close, limit);
    }
    let condition = parse_expr(tokens, start + 1, open)
        .unwrap_or_else(|| Expr::Unknown(tokens[start].span.clone()));
    let then_body = parse_block(tokens, open, close);
    let mut next = close + 1;
    let else_body = if tokens
        .get(next)
        .is_some_and(|token| token.is_ident_text("else"))
    {
        if tokens.get(next + 1).is_some_and(|token| token.symbol("{")) {
            let else_open = next + 1;
            let Some(else_close) = find_matching(tokens, else_open, "{", "}") else {
                return (Stmt::MalformedIf(tokens[start].span.clone()), limit);
            };
            next = else_close + 1;
            Some(parse_block(tokens, else_open, else_close))
        } else if tokens
            .get(next + 1)
            .is_some_and(|token| token.is_ident_text("if"))
        {
            let span = tokens[next + 1].span.clone();
            let (else_if, else_next) = parse_if_stmt(tokens, next + 1, limit);
            next = else_next;
            Some(Block {
                statements: vec![else_if],
                span,
            })
        } else {
            return (
                Stmt::MalformedIf(tokens[start].span.clone()),
                statement_end(tokens, next, limit),
            );
        }
    } else {
        None
    };

    (
        Stmt::If(IfStmt {
            condition,
            then_body,
            else_body,
            span: tokens[start].span.clone(),
        }),
        next,
    )
}

pub(super) struct IsCondition {
    pub(super) value: Expr,
    pub(super) effect: DataEffect,
    pub(super) pattern: MatchPattern,
    pub(super) body_open: usize,
    pub(super) body_close: usize,
}

fn parse_is_condition(tokens: &[Token], start: usize, limit: usize) -> Option<IsCondition> {
    let effect = parse_data_effect(tokens.get(start))?;
    let is_index = find_top_level_ident(tokens, start + 1, limit, "is")?;
    if is_index <= start + 1 {
        return None;
    }
    for body_open in top_level_body_open_candidates(tokens, is_index + 1, limit) {
        let Some(body_close) = find_matching(tokens, body_open, "{", "}") else {
            continue;
        };
        if !control_body_close_can_end(tokens, body_close, limit) {
            continue;
        }
        let Some(pattern) = parse_match_pattern(tokens, is_index + 1, body_open) else {
            continue;
        };
        let value = parse_expr(tokens, start + 1, is_index)
            .unwrap_or_else(|| Expr::Unknown(tokens[start].span.clone()));
        return Some(IsCondition {
            value,
            effect,
            pattern,
            body_open,
            body_close,
        });
    }
    None
}

fn top_level_body_open_candidates(
    tokens: &[Token],
    start: usize,
    limit: usize,
) -> impl Iterator<Item = usize> + '_ {
    let mut depth = 0usize;
    (start..limit).filter(move |index| {
        let token = &tokens[*index];
        if token.symbol("{") {
            if depth == 0 {
                return true;
            }
            depth += 1;
            return false;
        }
        if token.symbol("(") || token.symbol("[") {
            depth += 1;
        } else if token.symbol("}") || token.symbol(")") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        }
        false
    })
}

fn control_body_close_can_end(tokens: &[Token], body_close: usize, limit: usize) -> bool {
    let next = body_close + 1;
    next >= limit
        || matches!(
            tokens.get(next).map(|token| &token.kind),
            Some(TokenKind::Eof)
        )
        || tokens
            .get(next)
            .is_some_and(|token| token.is_ident_text("else") || token.symbol("}"))
        || tokens
            .get(next)
            .is_some_and(|token| token.span.line > tokens[body_close].span.line)
}

fn parse_if_is_stmt(tokens: &[Token], start: usize, limit: usize) -> Option<(Stmt, usize)> {
    let IsCondition {
        value,
        effect,
        pattern,
        body_open: open,
        body_close: close,
    } = parse_is_condition(tokens, start + 1, limit)?;
    let then_body = parse_block(tokens, open, close);
    let mut next = close + 1;
    let else_body = if tokens
        .get(next)
        .is_some_and(|token| token.is_ident_text("else"))
    {
        if tokens.get(next + 1).is_some_and(|token| token.symbol("{")) {
            let else_open = next + 1;
            let else_close = find_matching(tokens, else_open, "{", "}")?;
            next = else_close + 1;
            parse_block(tokens, else_open, else_close)
        } else if tokens
            .get(next + 1)
            .is_some_and(|token| token.is_ident_text("if"))
        {
            let span = tokens[next + 1].span.clone();
            let (else_if, else_next) = parse_if_stmt(tokens, next + 1, limit);
            next = else_next;
            Block {
                statements: vec![else_if],
                span,
            }
        } else {
            return Some((
                Stmt::MalformedIf(tokens[start].span.clone()),
                statement_end(tokens, next, limit),
            ));
        }
    } else {
        Block {
            statements: Vec::new(),
            span: tokens[start].span.clone(),
        }
    };

    Some((
        Stmt::Match(MatchStmt {
            value,
            scrutinee_effect: Some(effect),
            arms: vec![
                MatchArm {
                    pattern,
                    guard: None,
                    body: then_body,
                    span: tokens[start].span.clone(),
                },
                MatchArm {
                    pattern: MatchPattern::Wildcard(tokens[start].span.clone()),
                    guard: None,
                    body: else_body,
                    span: tokens[start].span.clone(),
                },
            ],
            malformed_arm_spans: Vec::new(),
            span: tokens[start].span.clone(),
        }),
        next,
    ))
}

fn parse_if_let_stmt(
    tokens: &[Token],
    start: usize,
    open: usize,
    close: usize,
    limit: usize,
) -> (Stmt, usize) {
    let Some(equals) = find_top_level_symbol(tokens, start + 2, open, "=") else {
        return (Stmt::MalformedIf(tokens[start].span.clone()), close + 1);
    };
    let Some(pattern) = parse_match_pattern(tokens, start + 2, equals) else {
        return (Stmt::MalformedIf(tokens[start].span.clone()), close + 1);
    };
    let value = parse_expr(tokens, equals + 1, open)
        .unwrap_or_else(|| Expr::Unknown(tokens[start].span.clone()));
    let then_body = parse_block(tokens, open, close);
    let mut next = close + 1;
    let else_body = if tokens
        .get(next)
        .is_some_and(|token| token.is_ident_text("else"))
    {
        if tokens.get(next + 1).is_some_and(|token| token.symbol("{")) {
            let else_open = next + 1;
            let Some(else_close) = find_matching(tokens, else_open, "{", "}") else {
                return (Stmt::MalformedIf(tokens[start].span.clone()), limit);
            };
            next = else_close + 1;
            parse_block(tokens, else_open, else_close)
        } else if tokens
            .get(next + 1)
            .is_some_and(|token| token.is_ident_text("if"))
        {
            let span = tokens[next + 1].span.clone();
            let (else_if, else_next) = parse_if_stmt(tokens, next + 1, limit);
            next = else_next;
            Block {
                statements: vec![else_if],
                span,
            }
        } else {
            return (
                Stmt::MalformedIf(tokens[start].span.clone()),
                statement_end(tokens, next, limit),
            );
        }
    } else {
        Block {
            statements: Vec::new(),
            span: tokens[start].span.clone(),
        }
    };
    (
        Stmt::Match(MatchStmt {
            value,
            scrutinee_effect: None,
            arms: vec![
                MatchArm {
                    pattern,
                    guard: None,
                    body: then_body,
                    span: tokens[start].span.clone(),
                },
                MatchArm {
                    pattern: MatchPattern::Wildcard(tokens[start].span.clone()),
                    guard: None,
                    body: else_body,
                    span: tokens[start].span.clone(),
                },
            ],
            malformed_arm_spans: Vec::new(),
            span: tokens[start].span.clone(),
        }),
        next,
    )
}

fn parse_loop_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    if tokens[start].is_ident_text("while")
        && let Some(parsed) = parse_while_is_stmt(tokens, start, limit)
    {
        return parsed;
    }
    let Some(open) = find_control_body_open(tokens, start, limit) else {
        return (
            Stmt::MalformedLoop(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let Some(close) = find_matching(tokens, open, "{", "}") else {
        return (Stmt::MalformedLoop(tokens[start].span.clone()), limit);
    };
    let condition = if tokens[start].is_ident_text("while") {
        let Some(condition) = parse_expr(tokens, start + 1, open) else {
            return (Stmt::MalformedLoop(tokens[start].span.clone()), close + 1);
        };
        Some(condition)
    } else {
        if open != start + 1 {
            return (Stmt::MalformedLoop(tokens[start].span.clone()), close + 1);
        }
        None
    };

    (
        Stmt::Loop(LoopStmt {
            condition,
            body: parse_block(tokens, open, close),
            span: tokens[start].span.clone(),
        }),
        close + 1,
    )
}

fn parse_while_is_stmt(tokens: &[Token], start: usize, limit: usize) -> Option<(Stmt, usize)> {
    let IsCondition {
        value,
        effect,
        pattern,
        body_open: open,
        body_close: close,
    } = parse_is_condition(tokens, start + 1, limit)?;
    let body = parse_block(tokens, open, close);
    Some((
        Stmt::Loop(LoopStmt {
            condition: None,
            body: Block {
                statements: vec![Stmt::Match(MatchStmt {
                    value,
                    scrutinee_effect: Some(effect),
                    arms: vec![
                        MatchArm {
                            pattern,
                            guard: None,
                            body,
                            span: tokens[start].span.clone(),
                        },
                        MatchArm {
                            pattern: MatchPattern::Wildcard(tokens[start].span.clone()),
                            guard: None,
                            body: Block {
                                statements: vec![Stmt::Break(tokens[start].span.clone())],
                                span: tokens[start].span.clone(),
                            },
                            span: tokens[start].span.clone(),
                        },
                    ],
                    malformed_arm_spans: Vec::new(),
                    span: tokens[start].span.clone(),
                })],
                span: tokens[start].span.clone(),
            },
            span: tokens[start].span.clone(),
        }),
        close + 1,
    ))
}

fn parse_for_stmt(tokens: &[Token], start: usize, limit: usize, is_async: bool) -> (Stmt, usize) {
    let Some(open) = find_control_body_open(tokens, start, limit) else {
        return (
            Stmt::MalformedFor(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let Some(close) = find_matching(tokens, open, "{", "}") else {
        return (Stmt::MalformedFor(tokens[start].span.clone()), limit);
    };
    let Some(binding) = tokens.get(start + 1).and_then(ident_name) else {
        return (Stmt::MalformedFor(tokens[start].span.clone()), close + 1);
    };
    if !tokens
        .get(start + 2)
        .is_some_and(|token| token.is_ident_text("in"))
    {
        return (Stmt::MalformedFor(tokens[start].span.clone()), close + 1);
    }
    let Some(iterable) = parse_expr(tokens, start + 3, open) else {
        return (Stmt::MalformedFor(tokens[start].span.clone()), close + 1);
    };

    (
        Stmt::For(ForStmt {
            binding: binding.to_string(),
            iterable,
            body: parse_block(tokens, open, close),
            is_async,
            span: tokens[start].span.clone(),
        }),
        close + 1,
    )
}

fn parse_task_group_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    // task_group { body }
    let open = start + 1;
    if open >= limit || !tokens[open].symbol("{") {
        return (
            Stmt::Unknown(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    }
    let Some(close) = find_matching(tokens, open, "{", "}") else {
        return (Stmt::Unknown(tokens[start].span.clone()), limit);
    };
    (
        Stmt::TaskGroup(TaskGroupStmt {
            body: parse_block(tokens, open, close),
            span: tokens[start].span.clone(),
        }),
        close + 1,
    )
}

fn parse_select_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    // select { arms }
    let open = start + 1;
    if open >= limit || !tokens[open].symbol("{") {
        return (
            Stmt::Unknown(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    }
    let Some(close) = find_matching(tokens, open, "{", "}") else {
        return (Stmt::Unknown(tokens[start].span.clone()), limit);
    };
    (
        Stmt::Select(SelectStmt {
            arms: parse_select_arms(tokens, open + 1, close),
            span: tokens[start].span.clone(),
        }),
        close + 1,
    )
}

fn parse_select_arms(tokens: &[Token], start: usize, end: usize) -> Vec<SelectArm> {
    // Each arm is `<binding> = <await-operation> => { body }`, where the body is
    // a brace block or a single statement (mirroring `match` arms).
    let mut arms = Vec::new();
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
            index = line_end.max(index + 1);
            continue;
        };
        let arm_start = index;
        let Some(eq) = find_top_level_symbol(tokens, index, arrow, "=") else {
            index = arrow + 1;
            continue;
        };
        let binding = ident_name(&tokens[index])
            .map(str::to_string)
            .unwrap_or_else(|| "_".to_string());
        let Some(operation) = parse_expr(tokens, eq + 1, arrow) else {
            index = arrow + 1;
            continue;
        };
        let body_start = arrow + 1;
        let (body, next) = if tokens
            .get(body_start)
            .is_some_and(|token| token.symbol("{"))
        {
            let Some(body_close) = find_matching(tokens, body_start, "{", "}") else {
                break;
            };
            (parse_block(tokens, body_start, body_close), body_close + 1)
        } else {
            if body_start >= end {
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
        arms.push(SelectArm {
            binding,
            operation,
            body,
            span: tokens[arm_start].span.clone(),
        });
        index = next;
    }
    arms
}

fn parse_match_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    let Some(open) = find_control_body_open(tokens, start, limit) else {
        return (
            Stmt::MalformedMatch(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let Some(close) = find_matching(tokens, open, "{", "}") else {
        return (Stmt::MalformedMatch(tokens[start].span.clone()), limit);
    };
    let (scrutinee_effect, value_start) = parse_match_scrutinee_effect(tokens, start + 1, open);
    let Some(value) = parse_expr(tokens, value_start, open) else {
        return (Stmt::MalformedMatch(tokens[start].span.clone()), close + 1);
    };

    let parsed_arms = parse_match_arms(tokens, open + 1, close);
    (
        Stmt::Match(MatchStmt {
            value,
            scrutinee_effect,
            arms: parsed_arms.arms,
            malformed_arm_spans: parsed_arms.malformed_spans,
            span: tokens[start].span.clone(),
        }),
        close + 1,
    )
}

