use crate::lexer::{Token, TokenKind, lex};
use crate::syntax::ast::{
    Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FileMode, FunctionDecl,
    IfStmt, Item, LetKind, LetStmt, LoopStmt, Param, Program, ReturnStmt, Stmt, TypeDecl, TypeKind,
    TypeRef, WithStmt,
};

pub fn parse_source(file: &str, source: &str) -> Program {
    let tokens = lex(file, source);
    Parser {
        tokens: &tokens,
        index: 0,
    }
    .parse_program()
}

struct Parser<'a> {
    tokens: &'a [Token],
    index: usize,
}

impl Parser<'_> {
    fn parse_program(&mut self) -> Program {
        let mut mode = None;
        let mut items = Vec::new();

        while !self.is_eof() {
            if self.at_ident("mode") && self.peek_symbol(1, ":") {
                mode = self.parse_mode();
            } else if self.at_ident("class") || self.at_ident("struct") || self.at_ident("resource")
            {
                if let Some(item) = self.parse_type_decl() {
                    items.push(Item::Type(item));
                }
            } else if self.at_ident("pub") || self.at_ident("async") || self.at_ident("fn") {
                if let Some(item) = self.parse_function_decl() {
                    items.push(Item::Function(item));
                }
            } else {
                self.index += 1;
            }
        }

        Program { mode, items }
    }

    fn parse_mode(&mut self) -> Option<FileMode> {
        self.index += 2;
        let mode = if self.at_ident("managed") {
            Some(FileMode::Managed)
        } else if self.at_ident("uses") && self.peek_symbol(1, "-") && self.peek_ident(2, "local") {
            self.index += 2;
            Some(FileMode::UsesLocal)
        } else if self.at_ident("uses-local") {
            Some(FileMode::UsesLocal)
        } else {
            None
        };
        self.index += 1;
        mode
    }

    fn parse_type_decl(&mut self) -> Option<TypeDecl> {
        let span = self.current()?.span.clone();
        let kind = if self.at_ident("class") {
            TypeKind::Class
        } else if self.at_ident("struct") {
            TypeKind::Struct
        } else {
            TypeKind::Resource
        };
        self.index += 1;
        let name = self.take_ident_name()?;
        let open = self.expect_symbol("{")?;
        let close = find_matching(self.tokens, open, "{", "}").unwrap_or(open);
        let fields = parse_fields(self.tokens, open + 1, close);
        self.index = close + 1;

        Some(TypeDecl {
            kind,
            name,
            fields,
            span,
        })
    }

    fn parse_function_decl(&mut self) -> Option<FunctionDecl> {
        let span = self.current()?.span.clone();
        while self.at_ident("pub") || self.at_ident("async") {
            self.index += 1;
        }
        if !self.at_ident("fn") {
            return None;
        }
        self.index += 1;
        let name = self.take_ident_name()?;

        let mut params = Vec::new();
        if self.at_symbol("(") {
            let open = self.index;
            let close = find_matching(self.tokens, open, "(", ")").unwrap_or(open);
            params = parse_params(self.tokens, open + 1, close);
            self.index = close + 1;
        }

        let mut return_ty = None;
        let mut returns_fresh = false;
        if self.at_symbol("->") {
            self.index += 1;
            let return_start = self.index;
            while !self.is_eof() && !self.at_ident("effects") && !self.at_symbol("{") {
                if self.at_ident("fresh") {
                    returns_fresh = true;
                }
                self.index += 1;
            }
            return_ty = parse_type_ref(self.tokens, return_start, self.index);
        }

        let mut effects = Vec::new();
        if self.at_ident("effects") && self.peek_symbol(1, "(") {
            let open = self.index + 1;
            let close = find_matching(self.tokens, open, "(", ")").unwrap_or(open);
            effects = parse_effects(self.tokens, open + 1, close);
            self.index = close + 1;
        }

        let body = if self.at_symbol("{") {
            let open = self.index;
            let close = find_matching(self.tokens, open, "{", "}").unwrap_or(open);
            self.index = close + 1;
            parse_block(self.tokens, open, close)
        } else {
            Block {
                statements: Vec::new(),
                span: self
                    .tokens
                    .get(self.index)
                    .map_or(span.clone(), |token| token.span.clone()),
            }
        };

        Some(FunctionDecl {
            name,
            params,
            return_ty,
            returns_fresh,
            effects,
            body,
            span,
        })
    }

    fn current(&self) -> Option<&Token> {
        self.tokens.get(self.index)
    }

    fn at_ident(&self, text: &str) -> bool {
        self.tokens
            .get(self.index)
            .is_some_and(|token| token.is_ident_text(text))
    }

    fn peek_ident(&self, offset: usize, text: &str) -> bool {
        self.tokens
            .get(self.index + offset)
            .is_some_and(|token| token.is_ident_text(text))
    }

    fn at_symbol(&self, symbol: &str) -> bool {
        self.tokens
            .get(self.index)
            .is_some_and(|token| token.symbol(symbol))
    }

    fn peek_symbol(&self, offset: usize, symbol: &str) -> bool {
        self.tokens
            .get(self.index + offset)
            .is_some_and(|token| token.symbol(symbol))
    }

    fn expect_symbol(&mut self, symbol: &str) -> Option<usize> {
        if self.at_symbol(symbol) {
            let index = self.index;
            self.index += 1;
            Some(index)
        } else {
            None
        }
    }

    fn take_ident_name(&mut self) -> Option<String> {
        let name = ident_name(self.tokens.get(self.index)?)?.to_string();
        self.index += 1;
        Some(name)
    }

    fn is_eof(&self) -> bool {
        matches!(
            self.tokens.get(self.index).map(|token| &token.kind),
            Some(TokenKind::Eof) | None
        )
    }
}

fn parse_fields(tokens: &[Token], start: usize, end: usize) -> Vec<FieldDecl> {
    let mut fields = Vec::new();
    let mut index = start;
    while index < end {
        if tokens[index].is_ident_text("drop") {
            index = skip_braced_block(tokens, index).unwrap_or(end);
            continue;
        }

        let name_index = index;
        if let Some(name) = tokens.get(name_index).and_then(ident_name)
            && tokens
                .get(name_index + 1)
                .is_some_and(|token| token.symbol(":"))
        {
            let mut ty_start = name_index + 2;
            let is_handle = tokens
                .get(ty_start)
                .is_some_and(|token| token.is_ident_text("handle"));
            if is_handle {
                ty_start += 1;
            }
            let ty_end = next_line_or_block_end(tokens, ty_start, end);
            if let Some(ty) = parse_type_ref(tokens, ty_start, ty_end) {
                fields.push(FieldDecl {
                    name: name.to_string(),
                    ty,
                    is_handle,
                    span: tokens[name_index].span.clone(),
                });
            }
        }

        index += 1;
    }
    fields
}

fn parse_params(tokens: &[Token], start: usize, end: usize) -> Vec<Param> {
    split_top_level(tokens, start, end, ",")
        .into_iter()
        .filter_map(|(start, end)| {
            let name = tokens.get(start).and_then(ident_name)?;
            if !tokens.get(start + 1).is_some_and(|token| token.symbol(":")) {
                return None;
            }

            let mut ty_start = start + 2;
            let effect = parse_data_effect(tokens.get(ty_start)).inspect(|_| {
                ty_start += 1;
            });
            let ty = parse_type_ref(tokens, ty_start, end)?;
            Some(Param {
                name: name.to_string(),
                effect,
                ty,
                span: tokens[start].span.clone(),
            })
        })
        .collect()
}

fn parse_effects(tokens: &[Token], start: usize, end: usize) -> Vec<EffectDecl> {
    let mut effects = Vec::new();
    let mut index = start;
    while index < end {
        if let Some(name) = tokens.get(index).and_then(ident_name) {
            if name == "retains" && tokens.get(index + 1).is_some_and(|token| token.symbol("(")) {
                let open = index + 1;
                if let Some(close) = find_matching(tokens, open, "(", ")") {
                    if let Some(param) = tokens.get(open + 1).and_then(ident_name) {
                        effects.push(EffectDecl::Retains(param.to_string()));
                    }
                    index = close + 1;
                    continue;
                }
            }
            effects.push(EffectDecl::Name(name.to_string()));
        }
        index += 1;
    }
    effects
}

fn parse_block(tokens: &[Token], open: usize, close: usize) -> Block {
    let mut statements = Vec::new();
    let mut index = open + 1;
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

        let (statement, next) = parse_stmt(tokens, index, close);
        statements.push(statement);
        index = next.max(index + 1);
    }

    Block {
        statements,
        span: tokens[open].span.clone(),
    }
}

fn parse_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
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
    let statement = parse_expr(tokens, start, end)
        .map_or_else(|| Stmt::Unknown(tokens[start].span.clone()), Stmt::Expr);
    (statement, end)
}

fn parse_let_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    let kind = if tokens[start].is_ident_text("local") {
        LetKind::Local
    } else {
        LetKind::Managed
    };
    let name = tokens
        .get(start + 1)
        .and_then(ident_name)
        .unwrap_or("")
        .to_string();
    let end = statement_end(tokens, start, limit);
    let value = (start + 2..end)
        .find(|index| tokens[*index].symbol("="))
        .and_then(|equals| parse_expr(tokens, equals + 1, end));

    (
        Stmt::Let(LetStmt {
            kind,
            name,
            value,
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
            Stmt::Unknown(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let Some(open) = (as_index + 1..limit).find(|index| tokens[*index].symbol("{")) else {
        return (
            Stmt::Unknown(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let close = find_matching(tokens, open, "{", "}").unwrap_or(open);
    let resource = parse_expr(tokens, start + 1, as_index)
        .unwrap_or_else(|| Expr::Unknown(tokens[start].span.clone()));
    let binding = tokens
        .get(as_index + 1)
        .and_then(ident_name)
        .unwrap_or("")
        .to_string();
    let body = parse_block(tokens, open, close);

    (
        Stmt::With(WithStmt {
            resource,
            binding,
            body,
            span: tokens[start].span.clone(),
        }),
        close + 1,
    )
}

fn parse_if_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    let Some(open) = find_control_body_open(tokens, start, limit) else {
        return (
            Stmt::Unknown(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let close = find_matching(tokens, open, "{", "}").unwrap_or(open);
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
            let else_close = find_matching(tokens, else_open, "{", "}").unwrap_or(else_open);
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
            None
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

fn parse_loop_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    let Some(open) = find_control_body_open(tokens, start, limit) else {
        return (
            Stmt::Unknown(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let close = find_matching(tokens, open, "{", "}").unwrap_or(open);
    let condition = if tokens[start].is_ident_text("while") {
        parse_expr(tokens, start + 1, open)
    } else {
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

fn parse_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let start = trim_outer(tokens, start, end).0;
    let end = trim_outer(tokens, start, end).1;
    if start >= end {
        return None;
    }

    if tokens[start].symbol("|") && tokens.get(start + 1).is_some_and(|token| token.symbol("|")) {
        let Some(open) = (start + 2..end).find(|index| tokens[*index].symbol("{")) else {
            return Some(Expr::Unknown(tokens[start].span.clone()));
        };
        let close = find_matching(tokens, open, "{", "}").unwrap_or(open);
        return Some(Expr::Closure {
            body: parse_block(tokens, open, close),
            span: tokens[start].span.clone(),
        });
    }

    if let Some(effect) = parse_data_effect(tokens.get(start)) {
        let value_start = start + 1;
        let value = if tokens
            .get(value_start)
            .is_some_and(|token| token.symbol("("))
        {
            let close =
                find_matching(tokens, value_start, "(", ")").unwrap_or(end.saturating_sub(1));
            parse_expr(tokens, value_start + 1, close)
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
            let close =
                find_matching(tokens, value_start, "(", ")").unwrap_or(end.saturating_sub(1));
            parse_expr(tokens, value_start + 1, close)
        } else {
            parse_expr(tokens, value_start, end)
        }?;
        return Some(Expr::Manage {
            value: Box::new(value),
            span: tokens[start].span.clone(),
        });
    }

    if let Some(call) = parse_call_expr(tokens, start, end) {
        return Some(call);
    }

    if let Some(field) = parse_field_expr(tokens, start, end) {
        return Some(field);
    }

    match tokens.get(start).map(|token| &token.kind)? {
        TokenKind::Ident(value) => Some(Expr::Ident(value.to_string(), tokens[start].span.clone())),
        TokenKind::Keyword(value) => Some(Expr::Ident(
            (*value).to_string(),
            tokens[start].span.clone(),
        )),
        TokenKind::Number(value) => Some(Expr::Number(value.clone(), tokens[start].span.clone())),
        TokenKind::String(value) => Some(Expr::String(value.clone(), tokens[start].span.clone())),
        _ => Some(Expr::Unknown(tokens[start].span.clone())),
    }
}

fn parse_call_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let (callee, open) = if tokens.get(start + 1).is_some_and(|token| token.symbol("(")) {
        (
            Callee::Name(ident_name(tokens.get(start)?)?.to_string()),
            start + 1,
        )
    } else if tokens.get(start + 1).is_some_and(|token| token.symbol("."))
        && tokens.get(start + 3).is_some_and(|token| token.symbol("("))
    {
        (
            Callee::Qualified {
                namespace: ident_name(tokens.get(start)?)?.to_string(),
                name: ident_name(tokens.get(start + 2)?)?.to_string(),
            },
            start + 3,
        )
    } else {
        return None;
    };
    let close = find_matching(tokens, open, "(", ")")?;
    if close >= end {
        return None;
    }
    let args = parse_call_args(tokens, open + 1, close);
    Some(Expr::Call {
        callee,
        args,
        span: tokens[start].span.clone(),
    })
}

fn parse_field_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    if start + 2 >= end || !tokens.get(start + 1).is_some_and(|token| token.symbol(".")) {
        return None;
    }
    Some(Expr::Field {
        base: Box::new(parse_expr(tokens, start, start + 1)?),
        name: ident_name(tokens.get(start + 2)?)?.to_string(),
        span: tokens[start].span.clone(),
    })
}

fn parse_call_args(tokens: &[Token], start: usize, end: usize) -> Vec<CallArg> {
    split_top_level(tokens, start, end, ",")
        .into_iter()
        .filter_map(|(start, end)| {
            if let Some(name) = tokens.get(start).and_then(ident_name)
                && tokens.get(start + 1).is_some_and(|token| token.symbol(":"))
            {
                let value = parse_expr(tokens, start + 2, end)?;
                Some(CallArg {
                    name: Some(name.to_string()),
                    value,
                    span: tokens[start].span.clone(),
                })
            } else {
                let value = parse_expr(tokens, start, end)?;
                Some(CallArg {
                    name: None,
                    value,
                    span: tokens[start].span.clone(),
                })
            }
        })
        .collect()
}

fn parse_type_ref(tokens: &[Token], start: usize, end: usize) -> Option<TypeRef> {
    let name_index = (start..end).find(|index| {
        ident_name(&tokens[*index])
            .is_some_and(|name| !matches!(name, "read" | "mut" | "take" | "fresh" | "handle"))
    })?;
    let name = ident_name(&tokens[name_index])?.to_string();
    let mut args = Vec::new();
    if tokens
        .get(name_index + 1)
        .is_some_and(|token| token.symbol("<"))
        && let Some(close) = find_matching(tokens, name_index + 1, "<", ">")
    {
        args = split_top_level(tokens, name_index + 2, close, ",")
            .into_iter()
            .filter_map(|(start, end)| parse_type_ref(tokens, start, end))
            .collect();
    }
    Some(TypeRef {
        name,
        args,
        span: tokens[name_index].span.clone(),
    })
}

fn parse_data_effect(token: Option<&Token>) -> Option<DataEffect> {
    let token = token?;
    if token.is_ident_text("read") {
        Some(DataEffect::Read)
    } else if token.is_ident_text("mut") {
        Some(DataEffect::Mut)
    } else if token.is_ident_text("take") {
        Some(DataEffect::Take)
    } else {
        None
    }
}

fn statement_end(tokens: &[Token], start: usize, limit: usize) -> usize {
    let line = tokens[start].span.line;
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(limit).skip(start + 1) {
        if depth == 0 && token.span.line > line {
            return index;
        }
        if token.symbol("(") || token.symbol("{") || token.symbol("[") || token.symbol("<") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") || token.symbol(">") {
            depth = depth.saturating_sub(1);
        }
    }
    limit
}

fn find_control_body_open(tokens: &[Token], start: usize, limit: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(limit).skip(start + 1) {
        if depth == 0 && token.symbol("{") {
            return Some(index);
        }
        if token.symbol("(") || token.symbol("[") || token.symbol("<") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("]") || token.symbol(">") {
            depth = depth.saturating_sub(1);
        }
    }
    None
}

fn next_line_or_block_end(tokens: &[Token], start: usize, end: usize) -> usize {
    if start >= end {
        return end;
    }
    let line = tokens[start].span.line;
    (start..end)
        .find(|index| tokens[*index].span.line > line)
        .unwrap_or(end)
}

fn split_top_level(
    tokens: &[Token],
    start: usize,
    end: usize,
    delimiter: &str,
) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut range_start = start;
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") || token.symbol("{") || token.symbol("[") || token.symbol("<") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") || token.symbol(">") {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token.symbol(delimiter) {
            if range_start < index {
                ranges.push((range_start, index));
            }
            range_start = index + 1;
        }
    }
    if range_start < end {
        ranges.push((range_start, end));
    }
    ranges
}

fn trim_outer(tokens: &[Token], start: usize, end: usize) -> (usize, usize) {
    if start < end
        && tokens[start].symbol("(")
        && find_matching(tokens, start, "(", ")").is_some_and(|close| close + 1 == end)
    {
        (start + 1, end - 1)
    } else {
        (start, end)
    }
}

fn skip_braced_block(tokens: &[Token], start: usize) -> Option<usize> {
    let open = (start..tokens.len()).find(|index| tokens[*index].symbol("{"))?;
    find_matching(tokens, open, "{", "}").map(|close| close + 1)
}

fn find_matching(
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

fn is_trivia_boundary(token: &Token) -> bool {
    token.symbol(",") || token.symbol(";")
}

fn ident_name(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Ident(value) => Some(value),
        TokenKind::Keyword(value) => Some(value),
        _ => None,
    }
}
