use crate::diagnostic::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Ident(String),
    Number(String),
    String(String),
    MultilineString(String),
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
            TokenKind::Ident(value)
            | TokenKind::Number(value)
            | TokenKind::String(value)
            | TokenKind::MultilineString(value) => value.clone(),
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
                '"' if self.peek_n(1) == Some('"') && self.peek_n(2) == Some('"') => {
                    self.lex_multiline_string()
                }
                '"' => self.lex_string(),
                ch if ch.is_ascii_digit() => self.lex_number(),
                ch if is_ident_start(ch) => self.lex_ident_or_keyword(),
                '-' if self.peek_next() == Some('>') => self.push_two("->"),
                '=' if self.peek_next() == Some('>') => self.push_two("=>"),
                ':' | ',' | '.' | '(' | ')' | '{' | '}' | '<' | '>' | '[' | ']' | '?' | '|'
                | '&' | '+' | '-' | '*' | '/' | '=' | '!' | ';' => self.push_one(),
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
        let token_start = self.index;
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
                length: self.index.saturating_sub(token_start).max(1),
            },
        });
    }

    fn lex_multiline_string(&mut self) {
        let start_line = self.line;
        let start_column = self.column;
        let token_start = self.index;
        self.bump();
        self.bump();
        self.bump();
        let start_index = self.index;
        while self.peek().is_some() {
            if self.peek() == Some('"')
                && self.peek_n(1) == Some('"')
                && self.peek_n(2) == Some('"')
            {
                break;
            }
            self.bump();
        }
        let value = self.chars[start_index..self.index.min(self.chars.len())]
            .iter()
            .collect();
        if self.peek() == Some('"') && self.peek_n(1) == Some('"') && self.peek_n(2) == Some('"') {
            self.bump();
            self.bump();
            self.bump();
        }
        self.tokens.push(Token {
            kind: TokenKind::MultilineString(value),
            span: Span {
                file: self.file.to_string(),
                line: start_line,
                column: start_column,
                length: self.index.saturating_sub(token_start).max(1),
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
            '&' => "&",
            '+' => "+",
            '-' => "-",
            '*' => "*",
            '/' => "/",
            '=' => "=",
            '!' => "!",
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

    fn peek_n(&self, offset: usize) -> Option<char> {
        self.chars.get(self.index + offset).copied()
    }
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

/// Highlight category for a keyword-like token.
///
/// This drives the generated VS Code TextMate grammar
/// (`src/editor_grammar.rs` → `editors/vscode/syntaxes/rsscript.tmLanguage.json`).
/// After changing any of the keyword tables below, regenerate the grammar with
/// `cargo run --bin gen-grammar`; the `vscode_grammar_is_up_to_date` test fails
/// until you do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeywordCategory {
    Control,
    Declaration,
    Modifier,
    Ownership,
}

/// Every reserved keyword the lexer recognizes, paired with how it should be
/// highlighted. This is the single source of truth: [`keyword`] derives the
/// lexer's reserved set from it, and the grammar generator derives its
/// highlight rules from it.
pub const KEYWORDS: &[(&str, KeywordCategory)] = &[
    // Control flow
    ("if", KeywordCategory::Control),
    ("else", KeywordCategory::Control),
    ("for", KeywordCategory::Control),
    ("in", KeywordCategory::Control),
    ("match", KeywordCategory::Control),
    ("loop", KeywordCategory::Control),
    ("while", KeywordCategory::Control),
    ("break", KeywordCategory::Control),
    ("continue", KeywordCategory::Control),
    ("return", KeywordCategory::Control),
    // Declarations
    ("features", KeywordCategory::Declaration),
    ("class", KeywordCategory::Declaration),
    ("struct", KeywordCategory::Declaration),
    ("resource", KeywordCategory::Declaration),
    ("handle", KeywordCategory::Declaration),
    ("fn", KeywordCategory::Declaration),
    ("let", KeywordCategory::Declaration),
    // Modifiers
    ("pub", KeywordCategory::Modifier),
    ("async", KeywordCategory::Modifier),
    ("effects", KeywordCategory::Modifier),
    ("drop", KeywordCategory::Modifier),
    ("with", KeywordCategory::Modifier),
    ("as", KeywordCategory::Modifier),
    // Ownership / binding modes
    ("read", KeywordCategory::Ownership),
    ("mut", KeywordCategory::Ownership),
    ("take", KeywordCategory::Ownership),
    ("fresh", KeywordCategory::Ownership),
    ("manage", KeywordCategory::Ownership),
    ("weak", KeywordCategory::Ownership),
    ("local", KeywordCategory::Ownership),
];

/// Words the lexer treats as plain identifiers but the parser interprets as
/// keywords in specific positions (see `src/syntax/parser.rs`). They are not
/// part of the reserved set, but the grammar still highlights them.
pub const CONTEXTUAL_KEYWORDS: &[(&str, KeywordCategory)] = &[
    ("await", KeywordCategory::Control),
    ("native", KeywordCategory::Modifier),
];

/// Built-in literals and result/option constructors recognized by the parser
/// (see `src/syntax/parser.rs`). Highlighted as language constants.
pub const BUILTIN_CONSTANTS: &[&str] = &["true", "false", "Ok", "Err", "Some", "None", "Unit"];

fn keyword(text: &str) -> Option<&'static str> {
    KEYWORDS
        .iter()
        .find(|(value, _)| *value == text)
        .map(|(value, _)| *value)
}
