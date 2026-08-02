use super::items::*;
use super::pattern::*;
use super::scan::*;
use super::stmt::*;
use super::types::*;
use super::*;

pub(super) fn parse_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let _parse = enter_parse()?;
    // A tuple literal must be detected before `trim_outer` strips the wrapping
    // parens (which would otherwise turn `(a, b)` into the invalid `a, b`).
    if let Some(tuple) = parse_tuple_expr(tokens, start, end) {
        return Some(tuple);
    }
    let (start, end) = trim_outer(tokens, start, end);
    if start >= end {
        return None;
    }

    if let Some(number) = parse_negative_number_expr(tokens, start, end) {
        return Some(number);
    }

    if tokens[start].is_ident_text("if") {
        if let Some(if_expr) = parse_if_expr(tokens, start, end) {
            return Some(if_expr);
        }
    }

    if tokens[start].symbol("|")
        && let Some(closure) = parse_closure_expr(tokens, start, end)
    {
        return Some(closure);
    }

    if tokens[start].is_ident_text("fn")
        && let Some(closure) = parse_explicit_fn_expr(tokens, start, end)
    {
        return Some(closure);
    }

    // An effect keyword immediately followed by a closure (e.g. `read || { ... }`)
    // is an effect-annotated closure argument. Handle it before binary parsing so
    // the `||` isn't mistaken for a logical-or operator.
    if let Some(effect) = parse_data_effect(tokens.get(start))
        && tokens.get(start + 1).is_some_and(|token| token.symbol("|"))
        && let Some(closure) = parse_closure_expr(tokens, start + 1, end)
        && !matches!(closure, Expr::Unknown(_))
    {
        return Some(Expr::Effect {
            effect,
            value: Box::new(closure),
            span: tokens[start].span.clone(),
        });
    }

    if let Some(binary) = parse_binary_expr(tokens, start, end) {
        return Some(binary);
    }

    // Prefix `!`/`~` bind tighter than every binary operator but looser than the
    // postfix `?` operator, so they are parsed after `parse_binary_expr` (which
    // splits at the loosest operator first) and before the trailing-`?` check.
    if let Some(unary) = parse_unary_expr(tokens, start, end) {
        return Some(unary);
    }

    if let Some(question) = find_trailing_top_level_question(tokens, start, end) {
        let value = parse_expr(tokens, start, question)?;
        return Some(Expr::Try {
            value: Box::new(value),
            span: tokens[question].span.clone(),
        });
    }

    // Match expression: match <value> { arms }
    if tokens[start].is_ident_text("match") {
        if let Some(match_expr) = parse_match_expr(tokens, start, end) {
            return Some(match_expr);
        }
    }

    if let Some(effect) = parse_data_effect(tokens.get(start)) {
        // Check for receiver-call shorthand: <effect> receiver.method(args)
        if let Some(receiver_call) = parse_receiver_call_expr(tokens, start, end, effect) {
            return Some(receiver_call);
        }

        let value_start = start + 1;
        let value = if tokens
            .get(value_start)
            .is_some_and(|token| token.symbol("("))
        {
            let Some(close) = find_matching(tokens, value_start, "(", ")") else {
                return Some(Expr::Unknown(tokens[start].span.clone()));
            };
            if close + 1 != end {
                return Some(Expr::Unknown(tokens[start].span.clone()));
            }
            // Parse the whole parenthesised operand so a tuple literal
            // (`read (a, b)`) is recognised; a single `(expr)` is still unwrapped
            // as grouping by `parse_expr`.
            parse_expr(tokens, value_start, end)
        } else {
            parse_expr(tokens, value_start, end)
        }?;
        return Some(Expr::Effect {
            effect,
            value: Box::new(value),
            span: tokens[start].span.clone(),
        });
    }

    if tokens[start].is_ident_text("manage") {
        let value_start = start + 1;
        let value = if tokens
            .get(value_start)
            .is_some_and(|token| token.symbol("("))
        {
            let Some(close) = find_matching(tokens, value_start, "(", ")") else {
                return Some(Expr::Unknown(tokens[start].span.clone()));
            };
            if close + 1 != end {
                return Some(Expr::Unknown(tokens[start].span.clone()));
            }
            // Parse the whole parenthesised operand so a tuple literal
            // (`read (a, b)`) is recognised; a single `(expr)` is still unwrapped
            // as grouping by `parse_expr`.
            parse_expr(tokens, value_start, end)
        } else {
            parse_expr(tokens, value_start, end)
        }?;
        return Some(Expr::Manage {
            value: Box::new(value),
            span: tokens[start].span.clone(),
        });
    }

    if tokens[start].is_ident_text("spawn") {
        let value = parse_expr(tokens, start + 1, end)?;
        return Some(Expr::Spawn {
            value: Box::new(value),
            span: tokens[start].span.clone(),
        });
    }

    if tokens[start].is_ident_text("await") {
        let value = parse_expr(tokens, start + 1, end)?;
        return Some(Expr::Await {
            value: Box::new(value),
            span: tokens[start].span.clone(),
        });
    }

    if let Some(array) = parse_array_literal_expr(tokens, start, end) {
        return Some(array);
    }

    if let Some(object) = parse_object_literal_expr(tokens, start, end) {
        return Some(object);
    }

    if let Some(receiver_call) = parse_receiver_call_expr_from_receiver(tokens, start, end, None) {
        return Some(receiver_call);
    }

    if let Some(call) = parse_call_expr(tokens, start, end) {
        return Some(call);
    }

    if let Some(field) = parse_field_expr(tokens, start, end) {
        return Some(field);
    }

    if let Some(index) = parse_index_expr(tokens, start, end) {
        return Some(index);
    }

    if start + 1 != end {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    }

    match tokens.get(start).map(|token| &token.kind)? {
        TokenKind::Ident(value) => Some(Expr::Ident(value.to_string(), tokens[start].span.clone())),
        TokenKind::Keyword(value) => Some(Expr::Ident(
            (*value).to_string(),
            tokens[start].span.clone(),
        )),
        TokenKind::Number(value) => Some(Expr::Number(value.clone(), tokens[start].span.clone())),
        TokenKind::String(value) => Some(Expr::String(value.clone(), tokens[start].span.clone())),
        TokenKind::Char(value) => {
            Some(Expr::CharLiteral(value.clone(), tokens[start].span.clone()))
        }
        TokenKind::InterpolatedString(value) => {
            Some(parse_interpolated_string_expr(value, &tokens[start].span))
        }
        TokenKind::MultilineString(value) => Some(Expr::MultilineString(
            value.clone(),
            tokens[start].span.clone(),
        )),
        _ => Some(Expr::Unknown(tokens[start].span.clone())),
    }
}

fn parse_if_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let then_open = find_control_body_open(tokens, start, end)?;
    let then_close = find_matching(tokens, then_open, "{", "}")?;
    let else_index = then_close + 1;
    if !tokens.get(else_index)?.is_ident_text("else") || !tokens.get(else_index + 1)?.symbol("{") {
        return None;
    }
    let else_open = else_index + 1;
    let else_close = find_matching(tokens, else_open, "{", "}")?;
    if else_close + 1 != end {
        return None;
    }
    let condition = parse_expr(tokens, start + 1, then_open)?;
    Some(Expr::Match {
        value: Box::new(condition),
        scrutinee_effect: None,
        arms: vec![
            MatchArm {
                pattern: MatchPattern::Literal {
                    value: MatchLiteral::Bool(true),
                    span: tokens[start].span.clone(),
                },
                guard: None,
                body: parse_block(tokens, then_open, then_close),
                span: tokens[start].span.clone(),
            },
            MatchArm {
                pattern: MatchPattern::Literal {
                    value: MatchLiteral::Bool(false),
                    span: tokens[else_index].span.clone(),
                },
                guard: None,
                body: parse_block(tokens, else_open, else_close),
                span: tokens[else_index].span.clone(),
            },
        ],
        from_if_expression: true,
        malformed_arm_spans: Vec::new(),
        span: tokens[start].span.clone(),
    })
}

fn parse_interpolated_string_expr(value: &str, span: &Span) -> Expr {
    let (template, items, malformed) = parse_interpolated_string_parts(value, span);
    let template_expr = Expr::Effect {
        effect: DataEffect::Read,
        value: Box::new(Expr::String(template, span.clone())),
        span: span.clone(),
    };
    let args_expr = Expr::Effect {
        effect: DataEffect::Read,
        value: Box::new(Expr::ArrayLiteral {
            items,
            span: span.clone(),
        }),
        span: span.clone(),
    };
    Expr::Call {
        callee: Callee::Qualified {
            namespace: "String".to_string(),
            name: "format".to_string(),
        },
        args: vec![
            CallArg {
                name: Some("template".to_string()),
                value: template_expr,
                malformed,
                span: span.clone(),
            },
            CallArg {
                name: Some("args".to_string()),
                value: args_expr,
                malformed,
                span: span.clone(),
            },
        ],
        span: span.clone(),
    }
}

fn parse_interpolated_string_parts(value: &str, span: &Span) -> (String, Vec<Expr>, bool) {
    let chars: Vec<char> = value.chars().collect();
    let mut template = String::new();
    let mut items = Vec::new();
    let mut index = 0usize;
    let mut malformed = false;

    while index < chars.len() {
        match chars[index] {
            '\\' => {
                template.push(chars[index]);
                index += 1;
                if let Some(ch) = chars.get(index) {
                    template.push(*ch);
                    index += 1;
                }
            }
            '{' if chars.get(index + 1) == Some(&'{') => {
                template.push('{');
                template.push('{');
                index += 2;
            }
            '}' if chars.get(index + 1) == Some(&'}') => {
                template.push('}');
                template.push('}');
                index += 2;
            }
            '{' => {
                let expr_start = index + 1;
                let Some(expr_end) = find_interpolation_end(&chars, expr_start) else {
                    malformed = true;
                    template.push('{');
                    index += 1;
                    continue;
                };
                let expr_text = chars[expr_start..expr_end].iter().collect::<String>();
                let expr_tokens = crate::lexer::lex_embedded_with_budget(
                    &span.file,
                    &expr_text,
                    current_parse_budget().expect("interpolation parsing has an active budget"),
                );
                let token_end = expr_tokens
                    .iter()
                    .position(|token| matches!(token.kind, TokenKind::Eof))
                    .unwrap_or(expr_tokens.len());
                if let Some(expr) = parse_expr(&expr_tokens, 0, token_end) {
                    template.push('{');
                    template.push('}');
                    items.push(expr);
                } else {
                    malformed = true;
                    template.push('{');
                    template.push_str(&expr_text);
                    template.push('}');
                }
                index = expr_end + 1;
            }
            '}' => {
                template.push('}');
                template.push('}');
                index += 1;
            }
            ch => {
                template.push(ch);
                index += 1;
            }
        }
    }

    (template, items, malformed)
}

fn find_interpolation_end(chars: &[char], start: usize) -> Option<usize> {
    let mut index = start;
    let mut depth = 0usize;
    while index < chars.len() {
        match chars[index] {
            '"' => {
                index += 1;
                while index < chars.len() {
                    if chars[index] == '\\' {
                        index += 2;
                        continue;
                    }
                    if chars[index] == '"' {
                        index += 1;
                        break;
                    }
                    index += 1;
                }
            }
            '(' | '[' | '{' => {
                depth += 1;
                index += 1;
            }
            ')' | ']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            '}' if depth == 0 => return Some(index),
            '}' => {
                depth -= 1;
                index += 1;
            }
            _ => index += 1,
        }
    }
    None
}

fn parse_negative_number_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    if start + 2 != end || !tokens.get(start)?.symbol("-") {
        return None;
    }
    let TokenKind::Number(value) = &tokens.get(start + 1)?.kind else {
        return None;
    };
    Some(Expr::Number(
        format!("-{value}"),
        tokens[start].span.clone(),
    ))
}

fn parse_array_literal_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    if !tokens.get(start).is_some_and(|token| token.symbol("["))
        || !tokens
            .get(end.checked_sub(1)?)
            .is_some_and(|token| token.symbol("]"))
    {
        return None;
    }
    let close = find_matching(tokens, start, "[", "]")?;
    if close + 1 != end {
        return None;
    }
    let items = split_param_ranges(tokens, start + 1, close)
        .into_iter()
        .filter_map(|range| {
            if range.empty_span.is_some() {
                return None;
            }
            parse_expr(tokens, range.start, range.end)
        })
        .collect();
    Some(Expr::ArrayLiteral {
        items,
        span: tokens[start].span.clone(),
    })
}

/// A tuple literal `(a, b, ...)` (two or more comma-separated elements). Desugars
/// directly to a synthetic generic-struct construction `__TupleN(item0: a, ...)`;
/// `__TupleN` declarations are injected by [`super::parse_source`]. A single
/// parenthesized expression `(e)` is grouping, not a tuple, so it returns `None`
/// and the normal `trim_outer` grouping path handles it.
fn parse_tuple_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
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
    let mut args = Vec::with_capacity(ranges.len());
    for (index, range) in ranges.iter().enumerate() {
        let value = parse_expr(tokens, range.start, range.end)?;
        args.push(CallArg {
            name: Some(format!("item{index}")),
            value,
            malformed: false,
            span: tokens[range.start].span.clone(),
        });
    }
    Some(Expr::Call {
        callee: Callee::Name(format!("__Tuple{}", ranges.len())),
        args,
        span: tokens[start].span.clone(),
    })
}

fn parse_object_literal_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    if !tokens.get(start).is_some_and(|token| token.symbol("{"))
        || !tokens
            .get(end.checked_sub(1)?)
            .is_some_and(|token| token.symbol("}"))
    {
        return None;
    }
    let close = find_matching(tokens, start, "{", "}")?;
    if close + 1 != end {
        return None;
    }
    let mut fields = Vec::new();
    let mut entries = Vec::new();
    let mut saw_map_entry = false;
    let mut saw_object_field = false;
    for range in split_param_ranges(tokens, start + 1, close) {
        if range.empty_span.is_some() {
            continue;
        }
        if let Some(arrow) = find_top_level_symbol(tokens, range.start, range.end, "=>") {
            if saw_object_field {
                return None;
            }
            saw_map_entry = true;
            let key = parse_expr(tokens, range.start, arrow)
                .unwrap_or_else(|| Expr::Unknown(tokens[range.start].span.clone()));
            let value = parse_expr(tokens, arrow + 1, range.end)
                .unwrap_or_else(|| Expr::Unknown(tokens[range.start].span.clone()));
            entries.push(MapLiteralEntry {
                key,
                value,
                span: tokens[range.start].span.clone(),
            });
            continue;
        }
        if saw_map_entry {
            return None;
        }
        saw_object_field = true;
        let Some(colon) = find_top_level_symbol(tokens, range.start, range.end, ":") else {
            return None;
        };
        let Some(TokenKind::String(name)) = tokens.get(range.start).map(|token| &token.kind) else {
            return None;
        };
        if range.start + 1 != colon {
            return None;
        }
        let value = parse_expr(tokens, colon + 1, range.end)
            .unwrap_or_else(|| Expr::Unknown(tokens[range.start].span.clone()));
        fields.push(ObjectLiteralField {
            name: name.clone(),
            value,
            span: tokens[range.start].span.clone(),
        });
    }
    if saw_map_entry {
        return Some(Expr::MapLiteral {
            entries,
            span: tokens[start].span.clone(),
        });
    }
    Some(Expr::ObjectLiteral {
        fields,
        span: tokens[start].span.clone(),
    })
}

fn parse_binary_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    find_top_level_operator(tokens, start, end, &[(&["|", "|"], BinaryOp::LogicalOr)])
        .or_else(|| {
            find_top_level_operator(tokens, start, end, &[(&["&", "&"], BinaryOp::LogicalAnd)])
        })
        .or_else(|| find_top_level_operator(tokens, start, end, &[(&["|"], BinaryOp::BitOr)]))
        .or_else(|| find_top_level_operator(tokens, start, end, &[(&["^"], BinaryOp::BitXor)]))
        .or_else(|| find_top_level_operator(tokens, start, end, &[(&["&"], BinaryOp::BitAnd)]))
        .or_else(|| {
            find_top_level_operator(
                tokens,
                start,
                end,
                &[
                    (&["=", "="], BinaryOp::Equal),
                    (&["!", "="], BinaryOp::NotEqual),
                    (&["<", "="], BinaryOp::LessEqual),
                    (&[">", "="], BinaryOp::GreaterEqual),
                    (&["<"], BinaryOp::Less),
                    (&[">"], BinaryOp::Greater),
                ],
            )
        })
        .or_else(|| {
            find_top_level_operator(
                tokens,
                start,
                end,
                &[
                    (&["<", "<"], BinaryOp::ShiftLeft),
                    (&[">", ">"], BinaryOp::ShiftRight),
                ],
            )
        })
        .or_else(|| {
            find_top_level_operator(
                tokens,
                start,
                end,
                &[(&["+"], BinaryOp::Add), (&["-"], BinaryOp::Subtract)],
            )
        })
        .or_else(|| {
            find_top_level_operator(
                tokens,
                start,
                end,
                &[
                    (&["*"], BinaryOp::Multiply),
                    (&["/"], BinaryOp::Divide),
                    (&["%"], BinaryOp::Modulo),
                ],
            )
        })
        .and_then(|(operator, op)| {
            let left = parse_expr(tokens, start, operator)?;
            let right_start = operator + op_width(op);
            let right = parse_expr(tokens, right_start, end)?;
            Some(Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span: tokens[operator].span.clone(),
            })
        })
}

fn parse_unary_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    if tokens.get(start)?.symbol("!") {
        let (value_start, value_end) = unary_operand_range(tokens, start + 1, end);
        let value = parse_expr(tokens, value_start, value_end)?;
        return Some(Expr::Binary {
            op: BinaryOp::Equal,
            left: Box::new(value),
            right: Box::new(Expr::Ident("false".to_string(), tokens[start].span.clone())),
            span: tokens[start].span.clone(),
        });
    }
    if tokens.get(start)?.symbol("~") {
        let (value_start, value_end) = unary_operand_range(tokens, start + 1, end);
        let value = parse_expr(tokens, value_start, value_end)?;
        Some(Expr::Call {
            callee: Callee::Qualified {
                namespace: "Int".to_string(),
                name: "bit_not".to_string(),
            },
            args: vec![CallArg {
                name: Some("value".to_string()),
                value,
                malformed: false,
                span: tokens[start].span.clone(),
            }],
            span: tokens[start].span.clone(),
        })
    } else {
        None
    }
}

fn unary_operand_range(tokens: &[Token], start: usize, end: usize) -> (usize, usize) {
    if tokens.get(start).is_some_and(|token| token.symbol("("))
        && let Some(close) = find_matching(tokens, start, "(", ")")
        && close + 1 == end
    {
        return (start + 1, close);
    }
    (start, end)
}

fn op_width(op: BinaryOp) -> usize {
    match op {
        BinaryOp::LogicalAnd
        | BinaryOp::LogicalOr
        | BinaryOp::ShiftLeft
        | BinaryOp::ShiftRight
        | BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::LessEqual
        | BinaryOp::GreaterEqual => 2,
        BinaryOp::Add
        | BinaryOp::Subtract
        | BinaryOp::Multiply
        | BinaryOp::Divide
        | BinaryOp::Modulo
        | BinaryOp::BitAnd
        | BinaryOp::BitOr
        | BinaryOp::BitXor
        | BinaryOp::Less
        | BinaryOp::Greater => 1,
    }
}

fn find_top_level_operator(
    tokens: &[Token],
    start: usize,
    end: usize,
    operators: &[(&[&str], BinaryOp)],
) -> Option<(usize, BinaryOp)> {
    let mut depth = 0usize;
    let mut angle_depth = 0usize;
    let mut found = None;
    let mut skip_until = start;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if index < skip_until {
            continue;
        }
        if token.symbol("(") || token.symbol("{") || token.symbol("[") {
            depth += 1;
            continue;
        }
        if token.symbol(")") || token.symbol("}") || token.symbol("]") {
            depth = depth.saturating_sub(1);
            continue;
        }
        if depth == 0 && token.symbol("<") && is_generic_angle_open(tokens, start, end, index) {
            angle_depth += 1;
            continue;
        }
        if depth == 0 && angle_depth > 0 {
            // Inside a generic type-argument list, `<` always opens a nested generic
            // (never a comparison), so count it; otherwise a nested `List<Int>` would
            // desync the depth and the outer `>` would be mistaken for a comparison op.
            if token.symbol("<") {
                angle_depth += 1;
            } else if token.symbol(">") {
                angle_depth -= 1;
            }
            continue;
        }
        if depth == 0 {
            // Consume `<<`/`>>` as atomic units so a stray `<`/`>` half is not
            // mistaken for a comparison operator. The shift tier itself searches
            // for these sequences, so skip this only when they are not wanted.
            let searching_shift = operators
                .iter()
                .any(|(symbols, _)| matches!(*symbols, ["<", "<"] | [">", ">"]));
            if !searching_shift
                && (symbols_match(tokens, index, end, &["<", "<"])
                    || symbols_match(tokens, index, end, &[">", ">"]))
            {
                skip_until = index + 2;
                continue;
            }
            for (symbols, op) in operators {
                if symbols_match(tokens, index, end, symbols) {
                    if *op == BinaryOp::LogicalOr
                        && tokens.get(index + 2).is_some_and(|token| token.symbol("{"))
                    {
                        continue;
                    }
                    found = Some((index, *op));
                    skip_until = index + symbols.len();
                    break;
                }
            }
        }
    }
    found
}

fn symbols_match(tokens: &[Token], index: usize, end: usize, symbols: &[&str]) -> bool {
    index + symbols.len() <= end
        && symbols
            .iter()
            .enumerate()
            .all(|(offset, symbol)| tokens[index + offset].symbol(symbol))
}

pub(super) fn is_generic_angle_open(
    tokens: &[Token],
    start: usize,
    end: usize,
    open: usize,
) -> bool {
    if open <= start
        || tokens.get(open - 1).and_then(ident_name).is_none()
        || !tokens[open].symbol("<")
    {
        return false;
    }

    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(open) {
        if token.symbol("<") {
            depth += 1;
        } else if token.symbol(">") {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return tokens
                    .get(index + 1)
                    .is_some_and(|token| token.symbol(".") || token.symbol("("));
            }
        }
    }
    false
}

fn find_trailing_top_level_question(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    if end <= start || !tokens.get(end - 1).is_some_and(|token| token.symbol("?")) {
        return None;
    }

    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") || token.symbol("{") || token.symbol("[") || token.symbol("<") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") || token.symbol(">") {
            depth = depth.saturating_sub(1);
        } else if index == end - 1 && depth == 0 && token.symbol("?") {
            return Some(index);
        }
    }
    None
}

fn parse_closure_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let close_pipe = (start + 1..end).find(|index| tokens[*index].symbol("|"))?;
    let mut params = Vec::new();
    for range in split_param_ranges(tokens, start + 1, close_pipe) {
        if range.empty_span.is_some() {
            continue;
        }
        if range.start + 1 != range.end {
            return Some(Expr::Unknown(tokens[start].span.clone()));
        }
        let Some(name) = ident_name(&tokens[range.start]) else {
            return Some(Expr::Unknown(tokens[start].span.clone()));
        };
        params.push(name.to_string());
    }

    let body_start = close_pipe + 1;
    let Some(open) = (body_start..end).find(|index| tokens[*index].symbol("{")) else {
        let value = parse_expr(tokens, body_start, end)
            .unwrap_or_else(|| Expr::Unknown(tokens[start].span.clone()));
        return Some(Expr::Closure {
            params,
            captures: Vec::new(),
            explicit: false,
            body: Block {
                statements: vec![Stmt::Expr(value)],
                span: tokens[start].span.clone(),
            },
            span: tokens[start].span.clone(),
        });
    };
    if open != body_start {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    }
    let Some(close) = find_matching(tokens, open, "{", "}") else {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    };
    if close + 1 != end {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    }
    Some(Expr::Closure {
        params,
        captures: Vec::new(),
        explicit: false,
        body: parse_block(tokens, open, close),
        span: tokens[start].span.clone(),
    })
}

fn parse_explicit_fn_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let params_open = start + 1;
    if !tokens
        .get(params_open)
        .is_some_and(|token| token.symbol("("))
    {
        return None;
    }
    let params_close = find_matching(tokens, params_open, "(", ")")?;
    let mut params = Vec::new();
    for range in split_param_ranges(tokens, params_open + 1, params_close) {
        if range.empty_span.is_some() {
            continue;
        }
        if range.start + 1 != range.end {
            return Some(Expr::Unknown(tokens[start].span.clone()));
        }
        let Some(name) = ident_name(&tokens[range.start]) else {
            return Some(Expr::Unknown(tokens[start].span.clone()));
        };
        params.push(name.to_string());
    }

    let mut cursor = params_close + 1;
    if !tokens
        .get(cursor)
        .is_some_and(|token| token.is_ident_text("captures"))
    {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    }
    let captures_open = cursor + 1;
    if !tokens
        .get(captures_open)
        .is_some_and(|token| token.symbol("("))
    {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    }
    let Some(captures_close) = find_matching(tokens, captures_open, "(", ")") else {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    };
    let Some(captures) = parse_closure_captures(tokens, captures_open + 1, captures_close) else {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    };
    cursor = captures_close + 1;

    if !tokens.get(cursor).is_some_and(|token| token.symbol("{")) {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    }
    let Some(body_close) = find_matching(tokens, cursor, "{", "}") else {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    };
    if body_close + 1 != end {
        return Some(Expr::Unknown(tokens[start].span.clone()));
    }

    Some(Expr::Closure {
        params,
        captures,
        explicit: true,
        body: parse_block(tokens, cursor, body_close),
        span: tokens[start].span.clone(),
    })
}

fn parse_closure_captures(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Option<Vec<crate::syntax::ast::ClosureCapture>> {
    let mut captures = Vec::new();
    for range in split_param_ranges(tokens, start, end) {
        if range.empty_span.is_some() {
            continue;
        }
        if range.start + 2 != range.end {
            return None;
        }
        let effect = parse_data_effect(tokens.get(range.start))?;
        let name = ident_name(&tokens[range.start + 1])?.to_string();
        captures.push(crate::syntax::ast::ClosureCapture {
            effect,
            name,
            span: tokens[range.start].span.clone(),
        });
    }
    Some(captures)
}

/// Parse receiver-call shorthand: `<effect> receiver.method(args)`
/// The pattern is: effect keyword already consumed, then:
///   tokens[start+1] = lowercase identifier (receiver)
///   tokens[start+2] = "."
///   tokens[start+3] = identifier (method name)
///   tokens[start+4] = "(" (args open)
/// A receiver is identified by its first character being lowercase.
/// This distinguishes `mut cache.put(...)` (receiver-call) from
/// `mut Cache.put(...)` which would be parsed as effect + qualified call.
fn parse_receiver_call_expr(
    tokens: &[Token],
    start: usize,
    end: usize,
    effect: DataEffect,
) -> Option<Expr> {
    // Reached only when an effect keyword was written explicitly.
    parse_receiver_call_expr_from_receiver(tokens, start + 1, end, Some(effect))
}

fn parse_receiver_call_expr_from_receiver(
    tokens: &[Token],
    receiver_start: usize,
    end: usize,
    effect: Option<DataEffect>,
) -> Option<Expr> {
    if tokens
        .get(receiver_start)
        .is_some_and(|token| token.symbol("!"))
    {
        return None;
    }
    let dot = find_receiver_call_dot(tokens, receiver_start, end)?;
    if is_qualified_namespace_receiver(tokens, receiver_start, dot) {
        return None;
    }
    let method_pos = dot + 1;
    let method_name = ident_name(tokens.get(method_pos)?)?;

    // Find the opening paren for the call args (may have generic args like method<T>(...))
    let call_open = find_call_open(tokens, method_pos, end)?;
    let close = find_matching(tokens, call_open, "(", ")")?;
    if close + 1 != end {
        return None;
    }

    // Build the method name (may include generic args like "write<T>")
    let method = if call_open == method_pos + 1 {
        method_name.to_string()
    } else {
        // There are generic type args between method name and "("
        parse_named_callee_segment(tokens, method_pos, call_open)?
    };

    let receiver = parse_expr(tokens, receiver_start, dot)
        .unwrap_or_else(|| Expr::Unknown(tokens[receiver_start].span.clone()));
    let args = parse_call_args(tokens, call_open + 1, close);
    Some(Expr::Call {
        callee: Callee::ReceiverCall {
            receiver: Box::new(receiver),
            method,
            effect,
        },
        args,
        span: tokens[receiver_start].span.clone(),
    })
}

fn find_receiver_call_dot(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut candidates = Vec::new();
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") || token.symbol("{") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token.symbol(".") {
            candidates.push(index);
        }
    }
    candidates.into_iter().rev().find(|dot| {
        if *dot <= start || dot + 1 >= end || ident_name(&tokens[dot + 1]).is_none() {
            return false;
        }
        let Some(open) = find_call_open(tokens, dot + 1, end) else {
            return false;
        };
        find_matching(tokens, open, "(", ")").is_some_and(|close| close + 1 == end)
    })
}

fn is_qualified_namespace_receiver(tokens: &[Token], start: usize, dot: usize) -> bool {
    let Some(name) = tokens.get(start).and_then(ident_name) else {
        return false;
    };
    name.starts_with(|c: char| c.is_uppercase())
        && tokens[start + 1..dot].iter().all(|token| {
            token.symbol("<")
                || token.symbol(">")
                || token.symbol(",")
                || ident_name(token).is_some()
        })
}

fn parse_call_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let open = find_call_open(tokens, start, end)?;
    let callee = parse_callee(tokens, start, open)?;
    let close = find_matching(tokens, open, "(", ")")?;
    if close + 1 != end {
        return None;
    }
    let args = parse_call_args(tokens, open + 1, close);
    Some(Expr::Call {
        callee,
        args,
        span: tokens[start].span.clone(),
    })
}

fn find_call_open(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start + 1) {
        if depth == 0 && token.symbol("(") {
            return Some(index);
        }
        if token.symbol("<") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(">") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        }
    }
    None
}

fn parse_callee(tokens: &[Token], start: usize, end: usize) -> Option<Callee> {
    if let Some(name) = parse_named_callee_segment(tokens, start, end) {
        return Some(Callee::Name(name));
    }

    let dot = find_top_level_dot(tokens, start, end)?;
    let namespace = type_ref_name(&parse_type_ref(tokens, start, dot)?);
    let name = parse_named_callee_segment(tokens, dot + 1, end)?;
    Some(Callee::Qualified { namespace, name })
}

fn parse_named_callee_segment(tokens: &[Token], start: usize, end: usize) -> Option<String> {
    let name = ident_name(tokens.get(start)?)?;
    if start + 1 == end {
        return Some(name.to_string());
    }
    if start + 2 >= end || !tokens.get(start + 1).is_some_and(|token| token.symbol("<")) {
        return None;
    }
    let close = find_matching(tokens, start + 1, "<", ">")?;
    if close + 1 != end {
        return None;
    }
    if start + 2 >= close {
        return None;
    }
    // Canonicalize each generic type argument through `type_ref_name` so the
    // spelling matches declared types elsewhere (e.g. a struct field typed
    // `owned Fn(Int) -> Int` and `List.new<owned Fn(Int) -> Int>` must produce
    // the SAME `type_name` string for the checker's type comparison; raw token
    // concatenation would collapse spacing to `ownedFn(Int)->Int`).
    let mut canonical_args = Vec::new();
    for range in split_param_ranges(tokens, start + 2, close) {
        if range.empty_span.is_some() {
            return None;
        }
        match parse_type_ref(tokens, range.start, range.end) {
            Some(ty) => canonical_args.push(type_ref_name(&ty)),
            None => {
                let raw = tokens_to_source(tokens, range.start, range.end);
                canonical_args.push(raw.trim().to_string());
            }
        }
    }
    if canonical_args.is_empty() {
        return None;
    }
    Some(format!("{name}<{}>", canonical_args.join(", ")))
}

fn find_top_level_dot(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if depth == 0 && token.symbol(".") {
            return Some(index);
        }
        if token.symbol("<") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(">") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        }
    }
    None
}

fn parse_field_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let dot = find_last_top_level_dot(tokens, start, end)?;
    if dot + 2 != end {
        return None;
    }
    Some(Expr::Field {
        base: Box::new(parse_expr(tokens, start, dot)?),
        name: ident_name(tokens.get(dot + 1)?)?.to_string(),
        span: tokens[start].span.clone(),
    })
}

fn find_last_top_level_dot(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut dot = None;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if depth == 0 && token.symbol(".") {
            dot = Some(index);
        }
        if token.symbol("(") || token.symbol("{") || token.symbol("[") || token.symbol("<") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") || token.symbol(">") {
            depth = depth.saturating_sub(1);
        }
    }
    dot
}

fn parse_index_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    if end < start + 3 || !tokens.get(end - 1).is_some_and(|token| token.symbol("]")) {
        return None;
    }
    let open = find_matching_open(tokens, start, end - 1, "[", "]")?;
    Some(Expr::Index {
        base: Box::new(parse_expr(tokens, start, open)?),
        index: Box::new(parse_expr(tokens, open + 1, end - 1)?),
        span: tokens[start].span.clone(),
    })
}

fn parse_call_args(tokens: &[Token], start: usize, end: usize) -> Vec<CallArg> {
    split_param_ranges(tokens, start, end)
        .into_iter()
        .filter_map(|range| {
            if let Some(span) = range.empty_span {
                return Some(CallArg {
                    name: None,
                    value: Expr::Unknown(span.clone()),
                    malformed: true,
                    span,
                });
            }
            let start = range.start;
            let end = range.end;
            if let Some(name) = tokens.get(start).and_then(ident_name)
                && tokens.get(start + 1).is_some_and(|token| token.symbol(":"))
            {
                let value = parse_expr(tokens, start + 2, end)
                    .unwrap_or_else(|| Expr::Unknown(tokens[start].span.clone()));
                Some(CallArg {
                    name: Some(name.to_string()),
                    value,
                    malformed: false,
                    span: tokens[start].span.clone(),
                })
            } else {
                let span = tokens.get(start)?.span.clone();
                let value =
                    parse_expr(tokens, start, end).unwrap_or_else(|| Expr::Unknown(span.clone()));
                Some(CallArg {
                    name: None,
                    value,
                    malformed: false,
                    span,
                })
            }
        })
        .collect()
}
