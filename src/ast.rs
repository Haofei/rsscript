use std::collections::HashMap;

use crate::diagnostic::Span;
use crate::lexer::{Token, TokenKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Managed,
    UsesLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKind {
    Class,
    Struct,
    Resource,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub type_name: String,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct TypeDecl {
    pub name: String,
    pub kind: TypeKind,
    pub fields: Vec<FieldDecl>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub type_name: String,
}

#[derive(Debug, Clone)]
pub struct FunctionDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<String>,
    pub effects: Vec<String>,
    pub body_start: usize,
}

#[derive(Debug, Default)]
pub struct Program {
    pub mode: Option<(FileMode, Span)>,
    pub types: HashMap<String, TypeDecl>,
    pub functions: HashMap<String, FunctionDecl>,
}

pub fn parse_program(tokens: &[Token]) -> Program {
    let mut parser = Parser {
        tokens,
        index: 0,
        program: Program::default(),
    };
    parser.parse();
    parser.program
}

struct Parser<'a> {
    tokens: &'a [Token],
    index: usize,
    program: Program,
}

impl Parser<'_> {
    fn parse(&mut self) {
        while !self.is_eof() {
            if self.at_ident("mode") && self.peek_symbol(1, ":") {
                self.parse_mode();
            } else if self.at_ident("class") || self.at_ident("struct") || self.at_ident("resource")
            {
                self.parse_type_decl();
            } else if self.at_ident("pub") || self.at_ident("async") || self.at_ident("fn") {
                self.parse_function_decl();
            } else {
                self.index += 1;
            }
        }
    }

    fn parse_mode(&mut self) {
        let span = self.current().span.clone();
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
        if let Some(mode) = mode {
            self.program.mode = Some((mode, span));
        }
        self.index += 1;
    }

    fn parse_type_decl(&mut self) {
        let kind = if self.at_ident("class") {
            TypeKind::Class
        } else if self.at_ident("struct") {
            TypeKind::Struct
        } else {
            TypeKind::Resource
        };
        self.index += 1;
        let Some(name) = self.take_ident_name() else {
            return;
        };
        let mut fields = Vec::new();
        if !self.at_symbol("{") {
            return;
        }
        self.index += 1;
        let body_end = self
            .find_matching(self.index - 1, "{", "}")
            .unwrap_or(self.tokens.len() - 1);
        let mut i = self.index;
        while i < body_end {
            if self.tokens[i].is_ident_text("drop") {
                i = skip_block(self.tokens, i).unwrap_or(body_end);
                continue;
            }
            let field_name_index = i;
            if ident_name(&self.tokens[field_name_index]).is_some()
                && self
                    .tokens
                    .get(field_name_index + 1)
                    .is_some_and(|token| token.symbol(":"))
            {
                let mut type_start = field_name_index + 2;
                let is_handle = self
                    .tokens
                    .get(type_start)
                    .is_some_and(|token| token.is_ident_text("handle"));
                if is_handle {
                    type_start += 1;
                }
                if let Some(type_name) = first_type_name(self.tokens, type_start, body_end) {
                    fields.push(FieldDecl {
                        type_name,
                        span: self.tokens[field_name_index].span.clone(),
                    });
                }
            }
            i += 1;
        }
        self.program
            .types
            .insert(name.clone(), TypeDecl { name, kind, fields });
        self.index = body_end + 1;
    }

    fn parse_function_decl(&mut self) {
        while self.at_ident("pub") || self.at_ident("async") {
            self.index += 1;
        }
        if !self.at_ident("fn") {
            return;
        }
        self.index += 1;
        let Some(name) = self.take_ident_name() else {
            return;
        };

        let mut params = Vec::new();
        if self.at_symbol("(") {
            let close = self
                .find_matching(self.index, "(", ")")
                .unwrap_or(self.index);
            params = parse_params(self.tokens, self.index + 1, close);
            self.index = close + 1;
        }

        let mut return_type = None;
        if self.at_symbol("->") {
            self.index += 1;
            let return_start = self.index;
            let mut depth = 0usize;
            while !self.is_eof() {
                if depth == 0 && (self.at_ident("effects") || self.at_symbol("{")) {
                    break;
                }
                if self.at_symbol("<") {
                    depth += 1;
                } else if self.at_symbol(">") {
                    depth = depth.saturating_sub(1);
                }
                self.index += 1;
            }
            return_type = last_type_name(self.tokens, return_start, self.index);
        }

        let mut effects = Vec::new();
        if self.at_ident("effects") && self.peek_symbol(1, "(") {
            let open = self.index + 1;
            let close = self.find_matching(open, "(", ")").unwrap_or(open);
            effects = parse_effects(self.tokens, open + 1, close);
            self.index = close + 1;
        }

        let body_start = if self.at_symbol("{") {
            let open = self.index;
            let close = self.find_matching(open, "{", "}").unwrap_or(open);
            self.index = close + 1;
            open + 1
        } else {
            self.index
        };

        self.program.functions.insert(
            name.clone(),
            FunctionDecl {
                name,
                params,
                return_type,
                effects,
                body_start,
            },
        );
    }

    fn current(&self) -> &Token {
        &self.tokens[self.index]
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

    fn take_ident_name(&mut self) -> Option<String> {
        let name = ident_name(self.tokens.get(self.index)?)?.to_string();
        self.index += 1;
        Some(name)
    }

    fn find_matching(&self, open: usize, open_symbol: &str, close_symbol: &str) -> Option<usize> {
        find_matching(self.tokens, open, open_symbol, close_symbol)
    }

    fn is_eof(&self) -> bool {
        matches!(
            self.tokens.get(self.index).map(|token| &token.kind),
            Some(TokenKind::Eof) | None
        )
    }
}

pub fn ident_name(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Ident(value) => Some(value),
        TokenKind::Keyword(value) => Some(value),
        _ => None,
    }
}

pub fn find_matching(
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

fn skip_block(tokens: &[Token], start: usize) -> Option<usize> {
    let open = (start..tokens.len()).find(|index| tokens[*index].symbol("{"))?;
    find_matching(tokens, open, "{", "}").map(|index| index + 1)
}

fn first_type_name(tokens: &[Token], start: usize, end: usize) -> Option<String> {
    (start..end).find_map(|index| match &tokens[index].kind {
        TokenKind::Ident(value) => Some(value.clone()),
        TokenKind::Keyword(value) if *value != "handle" => Some((*value).to_string()),
        _ => None,
    })
}

fn last_type_name(tokens: &[Token], start: usize, end: usize) -> Option<String> {
    (start..end)
        .rev()
        .find_map(|index| match &tokens[index].kind {
            TokenKind::Ident(value) => Some(value.clone()),
            TokenKind::Keyword(value) if *value != "fresh" => Some((*value).to_string()),
            _ => None,
        })
}

fn parse_params(tokens: &[Token], start: usize, end: usize) -> Vec<Param> {
    let mut params = Vec::new();
    let mut i = start;
    while i < end {
        if let Some(name) = ident_name(&tokens[i])
            && tokens.get(i + 1).is_some_and(|token| token.symbol(":"))
        {
            let mut cursor = i + 2;
            if tokens.get(cursor).is_some_and(|token| {
                token.is_ident_text("read")
                    || token.is_ident_text("mut")
                    || token.is_ident_text("take")
            }) {
                cursor += 1;
            }
            let type_end = next_top_level_comma(tokens, cursor, end).unwrap_or(end);
            let type_name = first_type_name(tokens, cursor, type_end).unwrap_or_default();
            params.push(Param {
                name: name.to_string(),
                type_name,
            });
            i = type_end + 1;
        } else {
            i += 1;
        }
    }
    params
}

fn next_top_level_comma(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") || token.symbol("<") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(")") || token.symbol(">") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token.symbol(",") {
            return Some(index);
        }
    }
    None
}

fn parse_effects(tokens: &[Token], start: usize, end: usize) -> Vec<String> {
    let mut effects = Vec::new();
    let mut i = start;
    while i < end {
        if let Some(effect) = ident_name(&tokens[i]) {
            if effect == "retains" && tokens.get(i + 1).is_some_and(|token| token.symbol("(")) {
                let open = i + 1;
                if let Some(close) = find_matching(tokens, open, "(", ")") {
                    if let Some(param) = tokens.get(open + 1).and_then(ident_name) {
                        effects.push(format!("retains({param})"));
                    }
                    i = close + 1;
                    continue;
                }
            }
            effects.push(effect.to_string());
        }
        i += 1;
    }
    effects
}
