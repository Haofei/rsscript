use crate::diagnostic::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident(String),
    Number(String),
    String(String),
    Keyword(&'static str),
    Symbol(&'static str),
    Eof,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn text(&self) -> String {
        match &self.kind {
            TokenKind::Ident(value) | TokenKind::Number(value) | TokenKind::String(value) => {
                value.clone()
            }
            TokenKind::Keyword(value) | TokenKind::Symbol(value) => (*value).to_string(),
            TokenKind::Eof => "<eof>".to_string(),
        }
    }

    pub fn is_ident_text(&self, text: &str) -> bool {
        matches!(&self.kind, TokenKind::Ident(value) if value == text)
            || matches!(&self.kind, TokenKind::Keyword(value) if *value == text)
    }

    pub fn symbol(&self, symbol: &str) -> bool {
        matches!(&self.kind, TokenKind::Symbol(value) if *value == symbol)
    }
}

pub fn lex(file: &str, source: &str) -> Vec<Token> {
    let mut lexer = Lexer {
        file,
        chars: source.chars().collect(),
        index: 0,
        line: 1,
        column: 1,
        tokens: Vec::new(),
    };
    lexer.run();
    lexer.tokens
}

struct Lexer<'a> {
    file: &'a str,
    chars: Vec<char>,
    index: usize,
    line: usize,
    column: usize,
    tokens: Vec<Token>,
}

impl Lexer<'_> {
    fn run(&mut self) {
        while let Some(ch) = self.peek() {
            match ch {
                ch if ch.is_whitespace() => self.bump_whitespace(),
                '/' if self.peek_next() == Some('/') => self.bump_line_comment(),
                '"' => self.lex_string(),
                ch if ch.is_ascii_digit() => self.lex_number(),
                ch if is_ident_start(ch) => self.lex_ident_or_keyword(),
                '-' if self.peek_next() == Some('>') => self.push_two("->"),
                '=' if self.peek_next() == Some('>') => self.push_two("=>"),
                ':' | ',' | '.' | '(' | ')' | '{' | '}' | '<' | '>' | '[' | ']' | '?' | '|'
                | '+' | '-' | '*' | '/' | '=' | ';' => self.push_one(),
                _ => self.push_one(),
            }
        }
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: self.span(0),
        });
    }

    fn lex_ident_or_keyword(&mut self) {
        let start_line = self.line;
        let start_column = self.column;
        let start_index = self.index;
        while self.peek().is_some_and(is_ident_continue) {
            self.bump();
        }
        let text: String = self.chars[start_index..self.index].iter().collect();
        let kind = keyword(&text)
            .map(TokenKind::Keyword)
            .unwrap_or(TokenKind::Ident(text));
        self.tokens.push(Token {
            kind,
            span: Span {
                file: self.file.to_string(),
                line: start_line,
                column: start_column,
                length: self.index - start_index,
            },
        });
    }

    fn lex_number(&mut self) {
        let start_line = self.line;
        let start_column = self.column;
        let start_index = self.index;
        while self
            .peek()
            .is_some_and(|ch| ch.is_ascii_digit() || ch == '.')
        {
            self.bump();
        }
        let value = self.chars[start_index..self.index].iter().collect();
        self.tokens.push(Token {
            kind: TokenKind::Number(value),
            span: Span {
                file: self.file.to_string(),
                line: start_line,
                column: start_column,
                length: self.index - start_index,
            },
        });
    }

    fn lex_string(&mut self) {
        let start_line = self.line;
        let start_column = self.column;
        self.bump();
        let start_index = self.index;
        while let Some(ch) = self.peek() {
            if ch == '"' {
                break;
            }
            if ch == '\\' {
                self.bump();
            }
            self.bump();
        }
        let value = self.chars[start_index..self.index.min(self.chars.len())]
            .iter()
            .collect();
        if self.peek() == Some('"') {
            self.bump();
        }
        self.tokens.push(Token {
            kind: TokenKind::String(value),
            span: Span {
                file: self.file.to_string(),
                line: start_line,
                column: start_column,
                length: self.column.saturating_sub(start_column).max(1),
            },
        });
    }

    fn bump_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }

    fn bump_line_comment(&mut self) {
        while let Some(ch) = self.peek() {
            self.bump();
            if ch == '\n' {
                break;
            }
        }
    }

    fn push_one(&mut self) {
        let span = self.span(1);
        let symbol = match self.bump().unwrap() {
            ':' => ":",
            ',' => ",",
            '.' => ".",
            '(' => "(",
            ')' => ")",
            '{' => "{",
            '}' => "}",
            '<' => "<",
            '>' => ">",
            '[' => "[",
            ']' => "]",
            '?' => "?",
            '|' => "|",
            '+' => "+",
            '-' => "-",
            '*' => "*",
            '/' => "/",
            '=' => "=",
            ';' => ";",
            _ => "?",
        };
        self.tokens.push(Token {
            kind: TokenKind::Symbol(symbol),
            span,
        });
    }

    fn push_two(&mut self, symbol: &'static str) {
        let span = self.span(2);
        self.bump();
        self.bump();
        self.tokens.push(Token {
            kind: TokenKind::Symbol(symbol),
            span,
        });
    }

    fn span(&self, length: usize) -> Span {
        Span {
            file: self.file.to_string(),
            line: self.line,
            column: self.column,
            length,
        }
    }

    fn bump(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.index += 1;
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.index).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.index + 1).copied()
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn keyword(text: &str) -> Option<&'static str> {
    Some(match text {
        "mode" => "mode",
        "managed" => "managed",
        "uses-local" => "uses-local",
        "class" => "class",
        "struct" => "struct",
        "resource" => "resource",
        "handle" => "handle",
        "drop" => "drop",
        "let" => "let",
        "local" => "local",
        "with" => "with",
        "as" => "as",
        "fn" => "fn",
        "pub" => "pub",
        "async" => "async",
        "return" => "return",
        "read" => "read",
        "mut" => "mut",
        "take" => "take",
        "fresh" => "fresh",
        "manage" => "manage",
        "effects" => "effects",
        "if" => "if",
        "else" => "else",
        "for" => "for",
        "in" => "in",
        "match" => "match",
        "loop" => "loop",
        "while" => "while",
        "break" => "break",
        "continue" => "continue",
        _ => return None,
    })
}
