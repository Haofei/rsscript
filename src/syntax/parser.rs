use std::collections::HashSet;

use crate::lexer::{Token, TokenKind, lex};
use crate::syntax::ast::{
    BinaryOp, Block, CallArg, Callee, DataEffect, DuplicateFileFeature, EffectDecl, Expr,
    FieldDecl, FileFeature, FunctionDecl, GenericBound, GenericParam, IfStmt, Item, LetKind,
    LetStmt, LoopStmt, MatchArm, MatchPattern, MatchStmt, Param, Program, ReturnStmt, Stmt,
    TypeDecl, TypeKind, TypeRef, UnknownFileFeature, WithStmt,
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

struct ParsedFeatures {
    features: Vec<FileFeature>,
    unknown_features: Vec<UnknownFileFeature>,
    duplicate_features: Vec<DuplicateFileFeature>,
}

impl Parser<'_> {
    fn parse_program(&mut self) -> Program {
        let mut features = Vec::new();
        let mut unknown_features = Vec::new();
        let mut duplicate_features = Vec::new();
        let mut feature_spans = Vec::new();
        let mut profile_spans = Vec::new();
        let mut unknown_top_level_spans = Vec::new();
        let mut items = Vec::new();

        while !self.is_eof() {
            if self.at_ident("features") && self.peek_symbol(1, ":") {
                feature_spans.push(self.tokens[self.index].span.clone());
                let parsed = self.parse_features();
                features.extend(parsed.features);
                unknown_features.extend(parsed.unknown_features);
                duplicate_features.extend(parsed.duplicate_features);
            } else if self.at_ident("profile") && self.peek_symbol(1, ":") {
                profile_spans.push(self.tokens[self.index].span.clone());
                self.index += 1;
            } else if self.at_ident("class") || self.at_ident("struct") || self.at_ident("resource")
            {
                if let Some(item) = self.parse_type_decl() {
                    items.push(Item::Type(item));
                }
            } else if self.at_ident("pub")
                || self.at_ident("async")
                || self.at_ident("native")
                || self.at_ident("fn")
            {
                if let Some(item) = self.parse_function_decl() {
                    items.push(Item::Function(item));
                }
            } else {
                unknown_top_level_spans.push(self.tokens[self.index].span.clone());
                self.index = skip_unknown_top_level(self.tokens, self.index);
            }
        }

        Program {
            features,
            unknown_features,
            duplicate_features,
            feature_spans,
            profile_spans,
            unknown_top_level_spans,
            items,
        }
    }

    fn parse_features(&mut self) -> ParsedFeatures {
        self.index += 2;
        let end = declaration_line_end(self.tokens, self.index);
        let mut features = Vec::new();
        let mut unknown_features = Vec::new();
        let mut duplicate_features = Vec::new();
        let mut seen_features = HashSet::new();
        while self.index < end {
            if self.at_symbol(",") {
                self.index += 1;
                continue;
            }
            let token = self.tokens.get(self.index);
            if let Some(feature) = parse_file_feature(token) {
                let name = file_feature_name(feature).to_string();
                if !seen_features.insert(feature)
                    && let Some(token) = token
                {
                    duplicate_features.push(DuplicateFileFeature {
                        name,
                        span: token.span.clone(),
                    });
                }
                features.push(feature);
            } else if let Some(token) = token
                && !matches!(token.kind, TokenKind::Eof)
            {
                unknown_features.push(UnknownFileFeature {
                    name: token.text(),
                    span: token.span.clone(),
                });
            }
            self.index += 1;
        }
        ParsedFeatures {
            features,
            unknown_features,
            duplicate_features,
        }
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
        let name = self.take_function_name()?;
        let type_params = self.parse_generic_params();
        let (fields, drop_body) = if self.at_symbol("{") {
            let open = self.index;
            let close = find_matching(self.tokens, open, "{", "}").unwrap_or(open);
            self.index = close + 1;
            (
                parse_fields(self.tokens, open + 1, close),
                parse_drop_body(self.tokens, open + 1, close),
            )
        } else {
            if self
                .tokens
                .get(self.index)
                .is_some_and(|token| token.span.line == span.line)
            {
                let end = declaration_line_end(self.tokens, self.index);
                self.index = end;
            }
            (Vec::new(), None)
        };

        Some(TypeDecl {
            kind,
            name,
            type_params,
            fields,
            drop_body,
            span,
        })
    }

    fn parse_function_decl(&mut self) -> Option<FunctionDecl> {
        let span = self.current()?.span.clone();
        let mut is_public = false;
        let mut is_async = false;
        let mut is_native = false;
        while self.at_ident("pub") || self.at_ident("async") || self.at_ident("native") {
            if self.at_ident("pub") {
                is_public = true;
            }
            if self.at_ident("async") {
                is_async = true;
            }
            if self.at_ident("native") {
                is_native = true;
            }
            self.index += 1;
        }
        if !self.at_ident("fn") {
            return None;
        }
        self.index += 1;
        let name = self.take_function_name()?;
        let type_params = self.parse_generic_params();

        let mut params = Vec::new();
        if self.at_symbol("(") {
            let open = self.index;
            let close = find_matching(self.tokens, open, "(", ")").unwrap_or(open);
            params = parse_params(self.tokens, open + 1, close);
            self.index = close + 1;
        }

        let signature_end = function_signature_end(self.tokens, self.index);
        let mut return_ty = None;
        let mut returns_fresh = false;
        if self.index < signature_end && self.at_symbol("->") {
            self.index += 1;
            let return_start = self.index;
            while self.index < signature_end && !self.at_ident("effects") && !self.at_symbol("{") {
                if self.at_ident("fresh") {
                    returns_fresh = true;
                }
                self.index += 1;
            }
            return_ty = parse_type_ref(self.tokens, return_start, self.index);
        }

        let mut effects = Vec::new();
        if self.index < signature_end && self.at_ident("effects") && self.peek_symbol(1, "(") {
            let open = self.index + 1;
            let close = find_matching(self.tokens, open, "(", ")").unwrap_or(open);
            effects = parse_effects(self.tokens, open + 1, close);
            self.index = close + 1;
        }
        if is_native
            && !effects
                .iter()
                .any(|effect| matches!(effect, EffectDecl::Name(name) if name == "native"))
        {
            effects.push(EffectDecl::Name("native".to_string()));
        }

        let body = if self.at_symbol("{") {
            let open = self.index;
            let close = find_matching(self.tokens, open, "{", "}").unwrap_or(open);
            self.index = close + 1;
            parse_block(self.tokens, open, close)
        } else {
            self.index = signature_end;
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
            is_public,
            is_async,
            is_native,
            type_params,
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

    fn take_ident_name(&mut self) -> Option<String> {
        let name = ident_name(self.tokens.get(self.index)?)?.to_string();
        self.index += 1;
        Some(name)
    }

    fn take_function_name(&mut self) -> Option<String> {
        let mut name = self.take_ident_name()?;
        while self.at_symbol(".") {
            self.index += 1;
            let segment = self.take_ident_name()?;
            name.push('.');
            name.push_str(&segment);
        }
        Some(name)
    }

    fn parse_generic_params(&mut self) -> Vec<GenericParam> {
        if !self.at_symbol("<") {
            return Vec::new();
        }
        let open = self.index;
        let Some(close) = find_matching(self.tokens, open, "<", ">") else {
            return Vec::new();
        };
        let params = parse_generic_params(self.tokens, open + 1, close);
        self.index = close + 1;
        params
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
            let mut is_handle = false;
            let mut is_weak = false;
            while let Some(token) = tokens.get(ty_start) {
                if token.is_ident_text("handle") {
                    is_handle = true;
                    ty_start += 1;
                } else if token.is_ident_text("weak") {
                    is_weak = true;
                    ty_start += 1;
                } else {
                    break;
                }
            }
            let ty_end = next_line_or_block_end(tokens, ty_start, end);
            if let Some(ty) = parse_type_ref(tokens, ty_start, ty_end) {
                fields.push(FieldDecl {
                    name: name.to_string(),
                    ty,
                    is_handle,
                    is_weak,
                    span: tokens[name_index].span.clone(),
                });
            }
        }

        index += 1;
    }
    fields
}

fn parse_drop_body(tokens: &[Token], start: usize, end: usize) -> Option<Block> {
    let drop_index = (start..end).find(|index| tokens[*index].is_ident_text("drop"))?;
    let open = (drop_index + 1..end).find(|index| tokens[*index].symbol("{"))?;
    let close = find_matching(tokens, open, "{", "}")?;
    (close <= end).then(|| parse_block(tokens, open, close))
}

fn declaration_line_end(tokens: &[Token], start: usize) -> usize {
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

fn function_signature_end(tokens: &[Token], start: usize) -> usize {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(start) {
        if depth == 0 && token.symbol("{") {
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
}

fn parse_params(tokens: &[Token], start: usize, end: usize) -> Vec<Param> {
    split_top_level(tokens, start, end, ",")
        .into_iter()
        .filter_map(|(start, end)| {
            let name = tokens.get(start).and_then(ident_name)?;
            if !tokens.get(start + 1).is_some_and(|token| token.symbol(":")) {
                return Some(Param {
                    name: name.to_string(),
                    effect: None,
                    ty: TypeRef {
                        name: String::new(),
                        args: Vec::new(),
                        is_noescape: false,
                        span: tokens[start].span.clone(),
                    },
                    span: tokens[start].span.clone(),
                });
            }

            let mut ty_start = start + 2;
            let effect = parse_data_effect(tokens.get(ty_start)).inspect(|_| {
                ty_start += 1;
            });
            let ty = parse_type_ref(tokens, ty_start, end).unwrap_or_else(|| TypeRef {
                name: String::new(),
                args: Vec::new(),
                is_noescape: false,
                span: tokens[start].span.clone(),
            });
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

fn parse_generic_params(tokens: &[Token], start: usize, end: usize) -> Vec<GenericParam> {
    split_top_level(tokens, start, end, ",")
        .into_iter()
        .filter_map(|(start, end)| {
            let name = tokens.get(start).and_then(ident_name)?;
            let bound = (start + 1..end)
                .find(|index| tokens[*index].symbol(":"))
                .and_then(|colon| tokens.get(colon + 1))
                .and_then(parse_generic_bound);
            Some(GenericParam {
                name: name.to_string(),
                bound,
                span: tokens[start].span.clone(),
            })
        })
        .collect()
}

fn parse_generic_bound(token: &Token) -> Option<GenericBound> {
    if token.is_ident_text("Managed") {
        Some(GenericBound::Managed)
    } else if token.is_ident_text("Struct") {
        Some(GenericBound::Struct)
    } else if token.is_ident_text("Resource") {
        Some(GenericBound::Resource)
    } else {
        None
    }
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
    if tokens[start].is_ident_text("match") {
        return parse_match_stmt(tokens, start, limit);
    }
    if tokens[start].is_ident_text("for") {
        return parse_unsupported_control_stmt(tokens, start, limit);
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

fn parse_unsupported_control_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    let next = find_control_body_open(tokens, start, limit)
        .and_then(|open| find_matching(tokens, open, "{", "}").map(|close| close + 1))
        .unwrap_or_else(|| statement_end(tokens, start, limit));
    (Stmt::Unknown(tokens[start].span.clone()), next)
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

fn parse_match_stmt(tokens: &[Token], start: usize, limit: usize) -> (Stmt, usize) {
    let Some(open) = find_control_body_open(tokens, start, limit) else {
        return (
            Stmt::Unknown(tokens[start].span.clone()),
            statement_end(tokens, start, limit),
        );
    };
    let close = find_matching(tokens, open, "{", "}").unwrap_or(open);
    let value = parse_expr(tokens, start + 1, open)
        .unwrap_or_else(|| Expr::Unknown(tokens[start].span.clone()));

    (
        Stmt::Match(MatchStmt {
            value,
            arms: parse_match_arms(tokens, open + 1, close),
            span: tokens[start].span.clone(),
        }),
        close + 1,
    )
}

fn parse_match_arms(tokens: &[Token], start: usize, end: usize) -> Vec<MatchArm> {
    let mut arms = Vec::new();
    let mut index = start;
    while index < end {
        while index < end && is_trivia_boundary(&tokens[index]) {
            index += 1;
        }
        if index >= end {
            break;
        }
        let Some(arrow) = find_top_level_symbol(tokens, index, end, "=>") else {
            break;
        };
        let pattern = parse_match_pattern(tokens, index, arrow)
            .unwrap_or_else(|| MatchPattern::Wildcard(tokens[index].span.clone()));
        let body_start = arrow + 1;
        let (body, next) = if tokens
            .get(body_start)
            .is_some_and(|token| token.symbol("{"))
        {
            let body_close = find_matching(tokens, body_start, "{", "}").unwrap_or(body_start);
            (parse_block(tokens, body_start, body_close), body_close + 1)
        } else {
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
            body,
            span: tokens[index].span.clone(),
        });
        index = next;
    }
    arms
}

fn parse_match_pattern(tokens: &[Token], start: usize, end: usize) -> Option<MatchPattern> {
    let start = trim_outer(tokens, start, end).0;
    let end = trim_outer(tokens, start, end).1;
    if start >= end {
        return None;
    }
    let name = ident_name(&tokens[start])?.to_string();
    if name == "_" {
        return Some(MatchPattern::Wildcard(tokens[start].span.clone()));
    }
    let binding = if tokens.get(start + 1).is_some_and(|token| token.symbol("(")) {
        let close = find_matching(tokens, start + 1, "(", ")")?;
        if close + 1 != end {
            return None;
        }
        (start + 2..close)
            .find_map(|index| ident_name(&tokens[index]).map(str::to_string))
            .filter(|binding| binding != "_")
    } else if start + 1 == end {
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

fn parse_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let start = trim_outer(tokens, start, end).0;
    let end = trim_outer(tokens, start, end).1;
    if start >= end {
        return None;
    }

    if tokens[start].symbol("|") && tokens.get(start + 1).is_some_and(|token| token.symbol("|")) {
        let Some(open) = (start + 2..end).find(|index| tokens[*index].symbol("{")) else {
            let value = parse_expr(tokens, start + 2, end)
                .unwrap_or_else(|| Expr::Unknown(tokens[start].span.clone()));
            return Some(Expr::Closure {
                body: Block {
                    statements: vec![Stmt::Expr(value)],
                    span: tokens[start].span.clone(),
                },
                span: tokens[start].span.clone(),
            });
        };
        let close = find_matching(tokens, open, "{", "}").unwrap_or(open);
        return Some(Expr::Closure {
            body: parse_block(tokens, open, close),
            span: tokens[start].span.clone(),
        });
    }

    if let Some(binary) = parse_binary_expr(tokens, start, end) {
        return Some(binary);
    }

    if let Some(question) = find_trailing_top_level_question(tokens, start, end) {
        let value = parse_expr(tokens, start, question)?;
        return Some(Expr::Try {
            value: Box::new(value),
            span: tokens[question].span.clone(),
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

    if let Some(index) = parse_index_expr(tokens, start, end) {
        return Some(index);
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

fn parse_binary_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    find_top_level_operator(tokens, start, end, &[(&["|", "|"], BinaryOp::LogicalOr)])
        .or_else(|| {
            find_top_level_operator(tokens, start, end, &[(&["&", "&"], BinaryOp::LogicalAnd)])
        })
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
                &[(&["+"], BinaryOp::Add), (&["-"], BinaryOp::Subtract)],
            )
        })
        .or_else(|| {
            find_top_level_operator(
                tokens,
                start,
                end,
                &[(&["*"], BinaryOp::Multiply), (&["/"], BinaryOp::Divide)],
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

fn op_width(op: BinaryOp) -> usize {
    match op {
        BinaryOp::LogicalAnd
        | BinaryOp::LogicalOr
        | BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::LessEqual
        | BinaryOp::GreaterEqual => 2,
        BinaryOp::Add
        | BinaryOp::Subtract
        | BinaryOp::Multiply
        | BinaryOp::Divide
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
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
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
            if token.symbol(">") {
                angle_depth -= 1;
            }
            continue;
        }
        if depth == 0 {
            for (symbols, op) in operators {
                if symbols_match(tokens, index, end, symbols) {
                    if *op == BinaryOp::LogicalOr
                        && tokens.get(index + 2).is_some_and(|token| token.symbol("{"))
                    {
                        continue;
                    }
                    found = Some((index, *op));
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

fn is_generic_angle_open(tokens: &[Token], start: usize, end: usize, open: usize) -> bool {
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

fn parse_call_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let open = find_call_open(tokens, start, end)?;
    let callee = parse_callee(tokens, start, open)?;
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
    if start + 1 == end {
        return Some(Callee::Name(ident_name(tokens.get(start)?)?.to_string()));
    }

    let dot = find_top_level_dot(tokens, start, end)?;
    let namespace = type_ref_name(&parse_type_ref(tokens, start, dot)?);
    let name = ident_name(tokens.get(dot + 1)?)?.to_string();
    Some(Callee::Qualified { namespace, name })
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
    if dot + 1 >= end {
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
    let is_noescape = tokens
        .get(start)
        .and_then(ident_name)
        .is_some_and(|name| name == "noescape");
    let name_index = (start..end).find(|index| {
        ident_name(&tokens[*index]).is_some_and(|name| {
            !matches!(
                name,
                "read" | "mut" | "take" | "fresh" | "handle" | "weak" | "noescape"
            )
        })
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
        is_noescape,
        span: tokens[name_index].span.clone(),
    })
}

fn type_ref_name(ty: &TypeRef) -> String {
    let name = if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty
            .args
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    if ty.is_noescape {
        if ty.name == "Fn" && ty.args.is_empty() {
            return "noescape Fn()".to_string();
        }
        format!("noescape {name}")
    } else {
        name
    }
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

fn parse_file_feature(token: Option<&Token>) -> Option<FileFeature> {
    let token = token?;
    if token.is_ident_text("local") {
        Some(FileFeature::Local)
    } else if token.is_ident_text("native") {
        Some(FileFeature::Native)
    } else if token.is_ident_text("unsafe") {
        Some(FileFeature::Unsafe)
    } else if token.is_ident_text("async") {
        Some(FileFeature::Async)
    } else if token.is_ident_text("device") {
        Some(FileFeature::Device)
    } else if token.is_ident_text("ffi") {
        Some(FileFeature::Ffi)
    } else if token.is_ident_text("reflection") {
        Some(FileFeature::Reflection)
    } else {
        None
    }
}

fn file_feature_name(feature: FileFeature) -> &'static str {
    match feature {
        FileFeature::Local => "local",
        FileFeature::Native => "native",
        FileFeature::Unsafe => "unsafe",
        FileFeature::Async => "async",
        FileFeature::Device => "device",
        FileFeature::Ffi => "ffi",
        FileFeature::Reflection => "reflection",
    }
}

fn statement_end(tokens: &[Token], start: usize, limit: usize) -> usize {
    let line = tokens[start].span.line;
    let mut depth = 0usize;
    let mut angle_depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(limit).skip(start + 1) {
        if depth == 0 && angle_depth == 0 && token.span.line > line {
            return index;
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

fn find_control_body_open(tokens: &[Token], start: usize, limit: usize) -> Option<usize> {
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

fn next_line_or_block_end(tokens: &[Token], start: usize, end: usize) -> usize {
    if start >= end {
        return end;
    }
    let line = tokens[start].span.line;
    (start..end)
        .find(|index| tokens[*index].span.line > line)
        .unwrap_or(end)
}

fn find_top_level_symbol(
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

fn skip_unknown_top_level(tokens: &[Token], start: usize) -> usize {
    let line_end = declaration_line_end(tokens, start);
    let block_end = (start..line_end)
        .find(|index| tokens[*index].symbol("{"))
        .and_then(|open| find_matching(tokens, open, "{", "}").map(|close| close + 1));
    block_end.unwrap_or(line_end).max(start + 1)
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

fn find_matching_open(
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

fn is_trivia_boundary(token: &Token) -> bool {
    token.symbol(",") || token.symbol(";")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_generic_qualified_resource_pool_call() {
        let program = parse_source(
            "test.rss",
            r#"
features: local

fn run() -> Unit {
    local pool = ResourcePool<Image>.new(
        create: || Image.load(path: read path),
        max_size: 4,
    )
}
"#,
        );
        let Item::Function(function) = &program.items[0] else {
            panic!("expected function");
        };
        let Stmt::Let(stmt) = &function.body.statements[0] else {
            panic!("expected let");
        };
        let Some(Expr::Call { callee, .. }) = &stmt.value else {
            panic!("expected call, got {:?}", stmt.value);
        };
        assert_eq!(
            callee,
            &Callee::Qualified {
                namespace: "ResourcePool<Image>".to_string(),
                name: "new".to_string(),
            }
        );
    }
}

fn ident_name(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Ident(value) => Some(value),
        TokenKind::Keyword(value) => Some(value),
        _ => None,
    }
}
