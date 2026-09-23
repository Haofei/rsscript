use std::cell::RefCell;
use std::rc::Rc;

use crate::ast::{
    AssignStmt, BinaryOp, Block, CallArg, Callee, ConstDecl, DataEffect, Expr, FieldDecl, ForStmt,
    FunctionDecl, GenericBound, GenericParam, IfStmt, Item, LetElseStmt, LetKind, LetStmt,
    LoopStmt, MapLiteralEntry, MatchArm, MatchFieldPattern, MatchLiteral, MatchPattern, MatchStmt,
    ModuleDecl, ObjectLiteralField, Param, Program, ProtocolDecl, ProtocolImpl,
    ProtocolImplMapping, ReturnStmt, SelectArm, SelectStmt, Stmt, SumTypeDecl, SumVariant,
    TaskGroupStmt, TypeAliasDecl, TypeDecl, TypeKind, TypeRef, UseDecl, WithStmt,
};
use crate::lexer::{KeywordCategory, Token, TokenKind, lex_with_budget};
use crate::{FrontendBudget, FrontendBudgetLimits, ParseRecursionGuard, Span};

mod expr;
pub use expr::interpolation_item_spans;
mod items;
mod pattern;
mod scan;
mod stmt;
mod types;

use expr::*;
use items::*;
use scan::*;
use stmt::*;
use types::*;

/// Words the parser matches as keywords in specific positions, which the lexer
/// deliberately leaves as plain identifiers and which are therefore absent from
/// [`crate::lexer::KEYWORDS`].
///
/// The generated grammar surface reads this table. It is not a second,
/// documentation-only catalog: `parser_keywords_are_matched_by_a_parser_
/// production` asserts every entry is really matched by a production in this
/// module, so a word cannot survive here after the parser stops recognizing it.
pub const PARSER_KEYWORDS: &[(&str, KeywordCategory)] = &[
    // Declarations and bindings
    ("sum", KeywordCategory::Declaration),
    ("protocol", KeywordCategory::Declaration),
    ("impl", KeywordCategory::Declaration),
    ("type", KeywordCategory::Declaration),
    ("const", KeywordCategory::Declaration),
    ("opaque", KeywordCategory::Declaration),
    ("module", KeywordCategory::Declaration),
    ("use", KeywordCategory::Declaration),
    ("view", KeywordCategory::Declaration),
    // Declaration clauses
    ("derives", KeywordCategory::Modifier),
    ("retains", KeywordCategory::Modifier),
    ("captures", KeywordCategory::Modifier),
    // Ownership annotations on a type
    ("noescape", KeywordCategory::Ownership),
    ("owned", KeywordCategory::Ownership),
    // Structured concurrency statements
    ("task_group", KeywordCategory::Control),
    ("select", KeywordCategory::Control),
    ("spawn", KeywordCategory::Control),
];

/// The parser's closed top-level dispatch table. Prefix completion reads this
/// table directly; it is intentionally not a second, completion-only catalog.
/// Retired source spellings such as `features` and `native` are absent.
pub(crate) const TOP_LEVEL_STARTERS: &[TopLevelStarter] = &[
    TopLevelStarter::Type("class"),
    TopLevelStarter::Type("struct"),
    TopLevelStarter::Type("resource"),
    TopLevelStarter::Type("opaque"),
    TopLevelStarter::Sum("sum"),
    TopLevelStarter::Protocol("protocol"),
    TopLevelStarter::Impl("impl"),
    TopLevelStarter::TypeAlias("type"),
    TopLevelStarter::Const("const"),
    TopLevelStarter::Module("module"),
    TopLevelStarter::Use("use"),
    TopLevelStarter::Public("pub"),
    TopLevelStarter::Function("async"),
    TopLevelStarter::Function("fn"),
    TopLevelStarter::Function("#"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TopLevelStarter {
    Type(&'static str),
    Sum(&'static str),
    Protocol(&'static str),
    Impl(&'static str),
    TypeAlias(&'static str),
    Const(&'static str),
    Module(&'static str),
    Use(&'static str),
    Public(&'static str),
    Function(&'static str),
}

impl TopLevelStarter {
    pub(crate) const fn text(self) -> &'static str {
        match self {
            Self::Type(text)
            | Self::Sum(text)
            | Self::Protocol(text)
            | Self::Impl(text)
            | Self::TypeAlias(text)
            | Self::Const(text)
            | Self::Module(text)
            | Self::Use(text)
            | Self::Public(text)
            | Self::Function(text) => text,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParserExpectationTerminal {
    Fixed(&'static str),
    Identifier(ParserIdentifierRole),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParserIdentifierRole {
    Function,
    Parameter,
    Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParserExpectationSite {
    TopLevel,
    FunctionHeader,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParserPrefixOracle {
    pub expected: Vec<ParserExpectationTerminal>,
    pub site: ParserExpectationSite,
    /// Whether every expected terminal came from an instrumented parser
    /// failpoint. A false value must be surfaced as Partial by callers.
    pub instrumented: bool,
}

#[derive(Debug, Clone)]
struct ExpectationSink {
    farthest: usize,
    expected: Vec<ParserExpectationTerminal>,
    site: ParserExpectationSite,
    instrumented: bool,
}

impl Default for ExpectationSink {
    fn default() -> Self {
        Self {
            farthest: 0,
            expected: Vec::new(),
            site: ParserExpectationSite::Unknown,
            instrumented: false,
        }
    }
}

impl ExpectationSink {
    fn record(
        &mut self,
        index: usize,
        terminal: ParserExpectationTerminal,
        site: ParserExpectationSite,
    ) {
        if index > self.farthest {
            self.farthest = index;
            self.expected.clear();
        }
        if index == self.farthest && !self.expected.contains(&terminal) {
            self.expected.push(terminal);
            self.site = site;
            self.instrumented = true;
        }
    }

    fn finish(self) -> Option<ParserPrefixOracle> {
        (!self.expected.is_empty()).then_some(ParserPrefixOracle {
            expected: self.expected,
            site: self.site,
            instrumented: self.instrumented,
        })
    }
}

/// Parse `source`, then apply source-preserving desugarings (currently:
/// associated-constant references). This is what every *semantic* consumer
/// (checker, HIR, lowering) uses. Tools that must preserve the exact source
/// surface (formatter, symbol index) use [`parse_source_raw`] instead.
pub fn parse_source(file: &str, source: &str) -> Program {
    let budget = FrontendBudget::new(
        FrontendBudgetLimits::default(),
        source_start_span(file, source.len()),
    );
    let tokens = lex_with_budget(file, source, budget.clone());
    parse_source_tokens(file, &tokens, budget)
}

pub fn parse_source_tokens(file: &str, tokens: &[Token], budget: Rc<FrontendBudget>) -> Program {
    let mut program = parse_source_tokens_raw(file, tokens, budget.clone());
    if budget.check_active() {
        super::desugar::desugar_associated_consts(&mut program);
        super::desugar::expand_tuple_destructuring(&mut program);
        super::desugar::inject_tuple_structs(&mut program);
    }
    program
}

/// Parse `source` without desugaring — the AST mirrors the written surface.
pub fn parse_source_raw(file: &str, source: &str) -> Program {
    let budget = FrontendBudget::new(
        FrontendBudgetLimits::default(),
        source_start_span(file, source.len()),
    );
    let tokens = lex_with_budget(file, source, budget.clone());
    parse_source_tokens_raw(file, &tokens, budget)
}

fn parse_source_tokens_raw(_file: &str, tokens: &[Token], budget: Rc<FrontendBudget>) -> Program {
    let _active_budget = ActiveParseBudget::set(budget);
    Parser::new(tokens).parse_program()
}

/// Run the ordinary parser with its prefix failpoint sink enabled. This is not
/// an incremental parser: callers still own source revision and caching.
pub(crate) fn parse_prefix_oracle(file: &str, source: &str) -> Option<ParserPrefixOracle> {
    let budget = FrontendBudget::new(
        FrontendBudgetLimits::default(),
        source_start_span(file, source.len()),
    );
    let tokens = lex_with_budget(file, source, budget.clone());
    let sink = Rc::new(RefCell::new(ExpectationSink::default()));
    let _active_budget = ActiveParseBudget::set(budget);
    Parser::with_expectations(&tokens, sink.clone()).parse_program();
    sink.borrow().clone().finish()
}

fn source_start_span(file: &str, length: usize) -> Span {
    Span {
        file: file.to_string(),
        line: 1,
        column: 1,
        length,
    }
}

thread_local! {
    static ACTIVE_PARSE_BUDGET: RefCell<Option<Rc<FrontendBudget>>> =
        const { RefCell::new(None) };
}

struct ActiveParseBudget {
    previous: Option<Rc<FrontendBudget>>,
}

impl ActiveParseBudget {
    fn set(budget: Rc<FrontendBudget>) -> Self {
        let previous = ACTIVE_PARSE_BUDGET.with(|active| active.replace(Some(budget)));
        Self { previous }
    }
}

impl Drop for ActiveParseBudget {
    fn drop(&mut self) {
        ACTIVE_PARSE_BUDGET.with(|active| {
            active.replace(self.previous.take());
        });
    }
}

pub(super) fn enter_parse() -> Option<ParseRecursionGuard> {
    ACTIVE_PARSE_BUDGET.with(|active| active.borrow().as_ref()?.enter_parse())
}

pub(super) fn current_parse_budget() -> Option<Rc<FrontendBudget>> {
    ACTIVE_PARSE_BUDGET.with(|active| active.borrow().clone())
}

pub(super) fn parse_is_active() -> bool {
    ACTIVE_PARSE_BUDGET.with(|active| {
        active
            .borrow()
            .as_ref()
            .is_none_or(|budget| budget.check_active())
    })
}

struct Parser<'a> {
    tokens: &'a [Token],
    index: usize,
    expectations: Option<Rc<RefCell<ExpectationSink>>>,
}

impl Parser<'_> {
    fn new(tokens: &[Token]) -> Parser<'_> {
        Parser {
            tokens,
            index: 0,
            expectations: None,
        }
    }

    fn with_expectations(
        tokens: &[Token],
        expectations: Rc<RefCell<ExpectationSink>>,
    ) -> Parser<'_> {
        Parser {
            tokens,
            index: 0,
            expectations: Some(expectations),
        }
    }

    fn parse_program(&mut self) -> Program {
        let _parse = enter_parse();
        let mut unknown_top_level_spans = Vec::new();
        let mut malformed_declaration_spans = Vec::new();
        let mut protocols = Vec::new();
        let mut protocol_impls = Vec::new();
        let mut items = Vec::new();

        while parse_is_active() && !self.is_eof() {
            match self.top_level_starter() {
                Some(TopLevelStarter::Type(_)) => {
                    let start = self.index;
                    if let Some(item) = self.parse_type_decl() {
                        items.push(Item::Type(item));
                    } else {
                        malformed_declaration_spans.push(self.tokens[start].span.clone());
                        self.index = skip_unknown_top_level(self.tokens, start);
                    }
                }
                Some(TopLevelStarter::Sum(_)) => {
                    let start = self.index;
                    if let Some(item) = self.parse_sum_type_decl() {
                        items.push(Item::SumType(item));
                    } else {
                        malformed_declaration_spans.push(self.tokens[start].span.clone());
                        self.index = skip_unknown_top_level(self.tokens, start);
                    }
                }
                Some(TopLevelStarter::Protocol(_)) => {
                    let start = self.index;
                    if let Some((protocol, functions)) = self.parse_protocol_decl() {
                        protocols.push(protocol);
                        items.extend(functions.into_iter().map(Item::Function));
                    } else {
                        malformed_declaration_spans.push(self.tokens[start].span.clone());
                        self.index = skip_unknown_top_level(self.tokens, start);
                    }
                }
                Some(TopLevelStarter::Impl(_)) => {
                    let start = self.index;
                    if self.impl_is_inherent() {
                        if let Some(functions) = self.parse_inherent_impl_decl() {
                            items.extend(functions.into_iter().map(Item::Function));
                        } else {
                            malformed_declaration_spans.push(self.tokens[start].span.clone());
                            self.index = skip_unknown_top_level(self.tokens, start);
                        }
                    } else if let Some(protocol_impl) = self.parse_protocol_impl_decl() {
                        protocol_impls.push(protocol_impl);
                    } else {
                        malformed_declaration_spans.push(self.tokens[start].span.clone());
                        self.index = skip_unknown_top_level(self.tokens, start);
                    }
                }
                Some(TopLevelStarter::TypeAlias(_)) => {
                    let start = self.index;
                    if let Some(alias) = self.parse_type_alias_decl() {
                        items.push(Item::TypeAlias(alias));
                    } else {
                        malformed_declaration_spans.push(self.tokens[start].span.clone());
                        self.index = skip_unknown_top_level(self.tokens, start);
                    }
                }
                Some(TopLevelStarter::Const(_)) => {
                    let start = self.index;
                    if let Some(decl) = self.parse_const_decl() {
                        items.push(Item::Const(decl));
                    } else {
                        malformed_declaration_spans.push(self.tokens[start].span.clone());
                        self.index = skip_unknown_top_level(self.tokens, start);
                    }
                }
                Some(TopLevelStarter::Module(_)) if !self.peek_ident(1, "{") => {
                    if let Some(decl) = self.parse_module_decl() {
                        items.push(Item::Module(decl));
                    } else {
                        self.index += 1;
                    }
                }
                Some(TopLevelStarter::Module(_)) => {
                    self.record_top_level_expectations();
                    unknown_top_level_spans.push(self.tokens[self.index].span.clone());
                    self.index = skip_unknown_top_level(self.tokens, self.index);
                }
                Some(TopLevelStarter::Use(_)) => {
                    if let Some(decl) = self.parse_use_decl() {
                        items.push(Item::Use(decl));
                    } else {
                        self.index += 1;
                    }
                }
                Some(TopLevelStarter::Function(_)) | Some(TopLevelStarter::Public(_)) => {
                    let start = self.index;
                    if let Some(item) = self.parse_function_decl() {
                        items.push(Item::Function(item));
                    } else {
                        malformed_declaration_spans.push(self.tokens[start].span.clone());
                        self.index = skip_unknown_top_level(self.tokens, start);
                    }
                }
                None => {
                    self.record_top_level_expectations();
                    unknown_top_level_spans.push(self.tokens[self.index].span.clone());
                    self.index = skip_unknown_top_level(self.tokens, self.index);
                }
            }
        }

        if items.is_empty()
            && protocols.is_empty()
            && protocol_impls.is_empty()
            && unknown_top_level_spans.is_empty()
            && malformed_declaration_spans.is_empty()
        {
            self.record_top_level_expectations();
        }

        Program {
            unknown_top_level_spans,
            malformed_declaration_spans,
            protocols,
            protocol_impls,
            items,
        }
    }

    // type Name = TargetType
    // type Name<T> = Result<T, Error>
    // pub type Name = TargetType
    fn parse_type_alias_decl(&mut self) -> Option<TypeAliasDecl> {
        let span = self.current()?.span.clone();
        let is_public = self.at_ident("pub");
        if is_public {
            self.index += 1;
        }
        if !self.at_ident("type") {
            return None;
        }
        self.index += 1;
        let name = self.take_ident_name()?;
        let parsed_generics = self.parse_generic_params();
        if !self.at_symbol("=") {
            return None;
        }
        self.index += 1;
        let ty_start = self.index;
        let ty_end = next_line_or_block_end(self.tokens, ty_start, self.tokens.len());
        let target = parse_type_ref(self.tokens, ty_start, ty_end)?;
        self.index = ty_end;
        Some(TypeAliasDecl {
            name,
            type_params: parsed_generics.params,
            target,
            is_public,
            span,
        })
    }

    // sum PaymentState { Pending, Authorized(receipt: Receipt), Failed(reason: String) }
    // pub sum Name<T> { ... }
    fn parse_sum_type_decl(&mut self) -> Option<SumTypeDecl> {
        let span = self.current()?.span.clone();
        let is_public = self.at_ident("pub");
        if is_public {
            self.index += 1;
        }
        if !self.at_ident("sum") {
            return None;
        }
        self.index += 1;
        let name = self.take_ident_name()?;
        let parsed_generics = self.parse_generic_params();
        let derives = self.parse_derives();
        if !self.at_symbol("{") {
            return None;
        }
        let open = self.index;
        let close = find_matching(self.tokens, open, "{", "}")?;
        self.index = open + 1;
        let mut variants = Vec::new();
        while self.index < close {
            if let Some(variant) = self.parse_sum_variant(close) {
                variants.push(variant);
            } else {
                self.index += 1;
            }
        }
        self.index = close + 1;
        Some(SumTypeDecl {
            name,
            type_params: parsed_generics.params,
            derives,
            variants,
            is_public,
            span,
        })
    }

    fn parse_sum_variant(&mut self, limit: usize) -> Option<SumVariant> {
        if self.index >= limit {
            return None;
        }
        let span = self.current()?.span.clone();
        let name = self.take_ident_name()?;
        let mut fields = Vec::new();
        if self.at_symbol("(") {
            let open = self.index;
            let close = find_matching(self.tokens, open, "(", ")")?;
            // Parse fields inside parens: name: Type, name: Type, ...
            let mut pos = open + 1;
            while pos < close {
                let field_name = ident_name(self.tokens.get(pos)?)?;
                if pos + 1 < close && self.tokens[pos + 1].symbol(":") {
                    let ty_start = pos + 2;
                    let ty_end = (ty_start..close)
                        .find(|i| self.tokens[*i].symbol(","))
                        .unwrap_or(close);
                    if let Some(ty) = parse_type_ref(self.tokens, ty_start, ty_end) {
                        fields.push(FieldDecl {
                            name: field_name.to_string(),
                            ty,
                            is_handle: false,
                            is_weak: false,
                            default: None,
                            span: self.tokens[pos].span.clone(),
                        });
                    }
                    pos = if ty_end < close { ty_end + 1 } else { close };
                } else {
                    pos += 1;
                }
            }
            self.index = close + 1;
        }
        Some(SumVariant { name, fields, span })
    }

    // const NAME: Type = value
    // pub const NAME: Type = value
    // const NAME = value  (type inferred)
    fn parse_const_decl(&mut self) -> Option<ConstDecl> {
        let span = self.current()?.span.clone();
        let is_public = self.at_ident("pub");
        if is_public {
            self.index += 1;
        }
        if !self.at_ident("const") {
            return None;
        }
        self.index += 1;
        // A dotted, type-associated name (`const Device.DEFAULT: ...`) or a plain
        // one (`const MAX_RETRIES: ...`). Associated names are flattened to an
        // ordinary const by the `desugar_associated_consts` pass.
        let name = self.take_function_name()?;
        let type_annotation = if self.at_symbol(":") {
            self.index += 1;
            let ty_start = self.index;
            // scan forward to '=' to find the type end
            let mut eq_pos = ty_start;
            while eq_pos < self.tokens.len() && !self.tokens[eq_pos].symbol("=") {
                eq_pos += 1;
            }
            let ty = parse_type_ref(self.tokens, ty_start, eq_pos)?;
            self.index = eq_pos;
            Some(ty)
        } else {
            None
        };
        if !self.at_symbol("=") {
            return None;
        }
        self.index += 1;
        // Parse const value expression: scan to end of line
        let expr_start = self.index;
        let expr_end = next_line_or_block_end(self.tokens, expr_start, self.tokens.len());
        let value = parse_expr(self.tokens, expr_start, expr_end)?;
        self.index = expr_end;
        Some(ConstDecl {
            name,
            type_annotation,
            value,
            is_public,
            span,
        })
    }

    fn parse_type_decl(&mut self) -> Option<TypeDecl> {
        let span = self.current()?.span.clone();
        let is_public = self.at_ident("pub");
        if is_public {
            self.index += 1;
        }
        let is_opaque = self.at_ident("opaque");
        if is_opaque {
            self.index += 1;
        }
        let kind = if self.at_ident("class") {
            TypeKind::Class
        } else if self.at_ident("struct") {
            TypeKind::Struct
        } else if self.at_ident("resource") {
            TypeKind::Resource
        } else {
            return None;
        };
        self.index += 1;
        let name = self.take_function_name()?;
        let parsed_type_params = self.parse_generic_params();
        let type_params = parsed_type_params.params;
        let malformed_generic_param_spans = parsed_type_params.malformed_spans;
        let derives = self.parse_derives();
        let (fields, malformed_field_spans, drop_body) = if self.at_symbol("{") {
            let open = self.index;
            let close = find_matching(self.tokens, open, "{", "}")?;
            self.index = close + 1;
            let parsed_fields = parse_fields(self.tokens, open + 1, close);
            (
                parsed_fields.fields,
                parsed_fields.malformed_spans,
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
            (Vec::new(), Vec::new(), None)
        };

        Some(TypeDecl {
            kind,
            name,
            is_public,
            is_opaque,
            type_params,
            malformed_generic_param_spans,
            derives,
            fields,
            malformed_field_spans,
            drop_body,
            span,
        })
    }

    fn peek_ident(&self, offset: usize, text: &str) -> bool {
        self.tokens
            .get(self.index + offset)
            .is_some_and(|token| token.is_ident_text(text))
    }

    fn parse_function_decl(&mut self) -> Option<FunctionDecl> {
        let start = self.index;
        let span = self.current()?.span.clone();
        let mut deprecated_reason = None;
        let mut lower_name = None;
        while self.at_symbol("#") {
            self.index += 1;
            let attribute = if self.at_ident("deprecated") {
                "deprecated"
            } else if self.at_ident("lower_name") {
                "lower_name"
            } else {
                self.index = start;
                return None;
            };
            self.index += 1;
            if !self.at_symbol("(") {
                self.index = start;
                return None;
            }
            self.index += 1;
            let Some(TokenKind::String(value)) = self.current().map(|token| &token.kind) else {
                self.index = start;
                return None;
            };
            match attribute {
                "deprecated" => deprecated_reason = Some(value.clone()),
                "lower_name" => lower_name = Some(value.clone()),
                _ => unreachable!(),
            }
            self.index += 1;
            if !self.at_symbol(")") {
                self.index = start;
                return None;
            }
            self.index += 1;
        }
        let mut is_public = false;
        let mut is_async = false;
        while self.at_ident("pub") || self.at_ident("async") {
            if self.at_ident("pub") {
                is_public = true;
            }
            if self.at_ident("async") {
                is_async = true;
            }
            self.index += 1;
        }
        if !self.at_ident("fn") {
            self.record_expectation(
                ParserExpectationTerminal::Fixed("fn"),
                ParserExpectationSite::FunctionHeader,
            );
            return None;
        }
        self.index += 1;
        let name = self.expect_identifier(
            ParserExpectationSite::FunctionHeader,
            ParserIdentifierRole::Function,
        )?;
        let mut name = name;
        while self.at_symbol(".") {
            self.index += 1;
            let segment = self.expect_identifier(
                ParserExpectationSite::FunctionHeader,
                ParserIdentifierRole::Function,
            )?;
            name.push('.');
            name.push_str(&segment);
        }
        let parsed_type_params = self.parse_generic_params();
        let type_params = parsed_type_params.params;
        let malformed_generic_param_spans = parsed_type_params.malformed_spans;

        let mut params = Vec::new();
        let mut malformed_param_spans = Vec::new();
        if self.expect_symbol("(", ParserExpectationSite::FunctionHeader) {
            let open = self.index - 1;
            let Some(close) = find_matching(self.tokens, open, "(", ")") else {
                self.record_expectation_at_eof(
                    ParserExpectationTerminal::Identifier(ParserIdentifierRole::Parameter),
                    ParserExpectationSite::FunctionHeader,
                );
                self.record_expectation_at_eof(
                    ParserExpectationTerminal::Fixed(")"),
                    ParserExpectationSite::FunctionHeader,
                );
                // The parameter slice has not been parsed. These are recovery
                // hints, not an exhaustive set for its current type/effect slot.
                if let Some(expectations) = &self.expectations {
                    expectations.borrow_mut().instrumented = false;
                }
                return None;
            };
            let parsed_params = parse_params(self.tokens, open + 1, close);
            params = parsed_params.params;
            malformed_param_spans = parsed_params.malformed_spans;
            self.index = close + 1;
        } else {
            self.record_expectation(
                ParserExpectationTerminal::Fixed("("),
                ParserExpectationSite::FunctionHeader,
            );
            if self.expectations.is_some() {
                return None;
            }
        }

        let signature_end = function_signature_end(self.tokens, self.index);
        let mut return_ty = None;
        let mut returns_fresh = false;
        if self.index < signature_end && self.at_symbol("->") {
            self.index += 1;
            if self.index < signature_end && self.at_ident("fresh") {
                returns_fresh = true;
                self.index += 1;
            }
            let return_start = self.index;
            while self.index < signature_end
                && !self.at_ident("retains")
                && !self.at_symbol("{")
                && !self.at_symbol("=")
            {
                if self.at_ident("fresh") {
                    returns_fresh = true;
                }
                self.index += 1;
            }
            if return_start == self.index {
                self.record_expectation(
                    ParserExpectationTerminal::Identifier(ParserIdentifierRole::Type),
                    ParserExpectationSite::FunctionHeader,
                );
            }
            return_ty = parse_type_ref(self.tokens, return_start, self.index);
        }

        let mut retained_params = Vec::new();
        while self.index < signature_end && self.at_ident("retains") && self.peek_symbol(1, "(") {
            let open = self.index + 1;
            let close = find_matching(self.tokens, open, "(", ")")?;
            if close != open + 2 {
                return None;
            }
            retained_params.push(self.tokens.get(open + 1)?.text());
            self.index = close + 1;
        }
        let default_impl_marker =
            self.index + 1 < signature_end && self.at_symbol("=") && self.peek_ident(1, "_");
        if default_impl_marker {
            self.index += 2;
        }
        let (has_body, body) = if self.at_symbol("{") {
            let open = self.index;
            let Some(close) = find_matching(self.tokens, open, "{", "}") else {
                self.record_expectation_at_eof(
                    ParserExpectationTerminal::Fixed("}"),
                    ParserExpectationSite::Unknown,
                );
                return None;
            };
            self.index = close + 1;
            (true, parse_block(self.tokens, open, close))
        } else {
            self.index = signature_end;
            (
                false,
                Block {
                    statements: Vec::new(),
                    span: self
                        .tokens
                        .get(self.index)
                        .map_or(span.clone(), |token| token.span.clone()),
                },
            )
        };

        Some(FunctionDecl {
            name,
            is_public,
            is_async,
            has_body,
            default_impl_marker,
            deprecated_reason,
            lower_name,
            type_params,
            malformed_generic_param_spans,
            params,
            malformed_param_spans,
            return_ty,
            returns_fresh,
            retained_params,
            body,
            span,
        })
    }

    fn parse_protocol_decl(&mut self) -> Option<(ProtocolDecl, Vec<FunctionDecl>)> {
        let span = self.current()?.span.clone();
        self.index += 1;
        let protocol = self.take_ident_name()?;
        let decl = ProtocolDecl {
            name: protocol.clone(),
            span,
        };
        if !self.at_symbol("{") {
            return None;
        }
        let open = self.index;
        let close = find_matching(self.tokens, open, "{", "}")?;
        self.index = open + 1;
        let mut methods = Vec::new();
        while self.index < close {
            if is_trivia_boundary(self.current()?) {
                self.index += 1;
                continue;
            }
            let start = self.index;
            let Some(mut method) = self.parse_function_decl() else {
                self.index = skip_unknown_top_level(self.tokens, start).min(close);
                continue;
            };
            method.name = if method.name.contains('.') {
                method.name
            } else {
                format!("{protocol}.{}", method.name)
            };
            method.is_public = true;
            if !method.type_params.iter().any(|param| param.name == "Self") {
                method.type_params.insert(
                    0,
                    GenericParam {
                        name: "Self".to_string(),
                        bound: Some(GenericBound::Managed),
                        span: method.span.clone(),
                    },
                );
            }
            methods.push(method);
        }
        self.index = close + 1;
        Some((decl, methods))
    }

    fn parse_protocol_impl_decl(&mut self) -> Option<ProtocolImpl> {
        let span = self.current()?.span.clone();
        self.index += 1;
        let protocol = self.take_ident_name()?;
        if !self.at_ident("for") {
            return None;
        }
        self.index += 1;
        let type_name = self.take_function_name()?;
        if !self.at_symbol("{") {
            return None;
        }
        let open = self.index;
        let close = find_matching(self.tokens, open, "{", "}")?;
        self.index = open + 1;
        let mut mappings = Vec::new();
        while self.index < close {
            if is_trivia_boundary(self.current()?) {
                self.index += 1;
                continue;
            }
            let mapping_span = self.current()?.span.clone();
            let Some(method) = self.take_ident_name() else {
                self.index = skip_unknown_top_level(self.tokens, self.index).min(close);
                continue;
            };
            if !self.at_symbol("=") {
                self.index = skip_unknown_top_level(self.tokens, self.index).min(close);
                continue;
            }
            self.index += 1;
            let Some(target) = self.take_function_name() else {
                self.index = skip_unknown_top_level(self.tokens, self.index).min(close);
                continue;
            };
            mappings.push(ProtocolImplMapping {
                method,
                target,
                span: mapping_span,
            });
        }
        self.index = close + 1;
        Some(ProtocolImpl {
            protocol,
            type_name,
            mappings,
            span,
        })
    }

    /// Distinguish an inherent-method block `impl Type { ... }` from a protocol
    /// implementation `impl Protocol for Type { ... }`: the former reaches its
    /// opening brace with no `for` keyword in between.
    fn impl_is_inherent(&self) -> bool {
        let mut i = self.index + 1;
        while let Some(token) = self.tokens.get(i) {
            if token.is_ident_text("for") {
                return false;
            }
            if token.symbol("{") {
                return true;
            }
            i += 1;
        }
        false
    }

    // impl Type {
    //     fn method(mut self, ...) -> R { ... }   // `<effect> self` or `self`
    //     fn other(self: read Type, ...) { ... }  // explicit form also allowed
    // }
    //
    // An inherent-method block: pure parse-time sugar for a set of top-level
    // qualified functions `fn Type.method(self: <effect> Type, ...)`. The block
    // only supplies the `Type.` qualifier and the `self` receiver type, so every
    // downstream stage (checker, HIR, receiver-call resolution, lowering) sees
    // exactly what the flat spelling produces — no new external_binding, grouping only.
    fn parse_inherent_impl_decl(&mut self) -> Option<Vec<FunctionDecl>> {
        self.index += 1;
        let type_name = self.take_ident_name()?;
        if !self.at_symbol("{") {
            return None;
        }
        let open = self.index;
        let close = find_matching(self.tokens, open, "{", "}")?;
        self.index = open + 1;
        let mut functions = Vec::new();
        while self.index < close {
            if is_trivia_boundary(self.current()?) {
                self.index += 1;
                continue;
            }
            let start = self.index;
            let Some(mut function) = self.parse_function_decl() else {
                self.index = skip_unknown_top_level(self.tokens, start).min(close);
                continue;
            };
            if !function.name.contains('.') {
                function.name = format!("{type_name}.{}", function.name);
            }
            for param in &mut function.params {
                if param.name == "self" && param.ty.name.is_empty() {
                    param.ty.name = type_name.clone();
                }
            }
            functions.push(function);
        }
        self.index = close + 1;
        Some(functions)
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

    fn expect_symbol(&mut self, symbol: &'static str, site: ParserExpectationSite) -> bool {
        if self.at_symbol(symbol) {
            self.index += 1;
            true
        } else {
            self.record_expectation(ParserExpectationTerminal::Fixed(symbol), site);
            false
        }
    }

    fn expect_identifier(
        &mut self,
        site: ParserExpectationSite,
        role: ParserIdentifierRole,
    ) -> Option<String> {
        if let Some(name) = self.take_ident_name() {
            Some(name)
        } else {
            self.record_expectation(ParserExpectationTerminal::Identifier(role), site);
            None
        }
    }

    fn record_expectation(&self, terminal: ParserExpectationTerminal, site: ParserExpectationSite) {
        if let Some(expectations) = &self.expectations {
            expectations.borrow_mut().record(self.index, terminal, site);
        }
    }

    fn record_expectation_at_eof(
        &self,
        terminal: ParserExpectationTerminal,
        site: ParserExpectationSite,
    ) {
        if let Some(expectations) = &self.expectations {
            expectations
                .borrow_mut()
                .record(self.tokens.len().saturating_sub(1), terminal, site);
        }
    }

    fn record_top_level_expectations(&self) {
        for starter in TOP_LEVEL_STARTERS {
            self.record_expectation(
                ParserExpectationTerminal::Fixed(starter.text()),
                ParserExpectationSite::TopLevel,
            );
        }
    }

    fn peek_symbol(&self, offset: usize, symbol: &str) -> bool {
        self.tokens
            .get(self.index + offset)
            .is_some_and(|token| token.symbol(symbol))
    }

    fn top_level_starter(&self) -> Option<TopLevelStarter> {
        let token = self.current()?;
        let text = if token.symbol("#") {
            "#"
        } else {
            ident_name(token)?
        };
        let starter = TOP_LEVEL_STARTERS
            .iter()
            .copied()
            .find(|starter| starter.text() == text)?;
        if !matches!(starter, TopLevelStarter::Public(_)) {
            return Some(starter);
        }

        let next = self.tokens.get(self.index + 1).and_then(ident_name);
        TOP_LEVEL_STARTERS
            .iter()
            .copied()
            .find(|candidate| candidate.text() == next.unwrap_or_default())
            .filter(|candidate| {
                matches!(
                    candidate,
                    TopLevelStarter::Type(_)
                        | TopLevelStarter::Sum(_)
                        | TopLevelStarter::TypeAlias(_)
                        | TopLevelStarter::Const(_)
                )
            })
            .or(Some(TopLevelStarter::Function("fn")))
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

    fn parse_generic_params(&mut self) -> ParsedGenericParams {
        if !self.at_symbol("<") {
            return ParsedGenericParams {
                params: Vec::new(),
                malformed_spans: Vec::new(),
            };
        }
        let open = self.index;
        let Some(close) = find_matching(self.tokens, open, "<", ">") else {
            return ParsedGenericParams {
                params: Vec::new(),
                malformed_spans: vec![self.tokens[open].span.clone()],
            };
        };
        let params = parse_generic_params(self.tokens, open + 1, close);
        self.index = close + 1;
        params
    }

    /// Parse `derives(Debug, Clone, Eq)` annotation.
    /// Returns empty vec if no derives annotation present.
    fn parse_derives(&mut self) -> Vec<String> {
        if !self.at_ident("derives") {
            return Vec::new();
        }
        self.index += 1;
        if !self.at_symbol("(") {
            return Vec::new();
        }
        let open = self.index;
        let Some(close) = find_matching(self.tokens, open, "(", ")") else {
            return Vec::new();
        };
        self.index = open + 1;
        let mut derives = Vec::new();
        while self.index < close {
            if let Some(name) = self.take_ident_name() {
                derives.push(name);
            }
            // skip commas
            if self.at_symbol(",") {
                self.index += 1;
            }
        }
        self.index = close + 1;
        derives
    }

    /// Parse `module package.review` declaration.
    fn parse_module_decl(&mut self) -> Option<ModuleDecl> {
        let span = self.current()?.span.clone();
        if !self.at_ident("module") {
            return None;
        }
        self.index += 1;
        let path = self.parse_dotted_path()?;
        Some(ModuleDecl { path, span })
    }

    /// Parse `use package.contract.PackageContract` declaration.
    fn parse_use_decl(&mut self) -> Option<UseDecl> {
        let span = self.current()?.span.clone();
        if !self.at_ident("use") {
            return None;
        }
        self.index += 1;
        let path = self.parse_dotted_path()?;
        // `use module.*` glob: `parse_dotted_path` consumes the trailing `.` and
        // stops at `*` (not an identifier), leaving the cursor on it.
        let glob = self.at_symbol("*");
        if glob {
            self.index += 1;
        }
        // Optional `as <alias>` renames the import locally so a file can pull two
        // same-leaf symbols from different modules without collision. A glob has
        // no single local name, so it takes no alias.
        let alias = if !glob && self.at_ident("as") {
            self.index += 1;
            self.take_ident_name()
        } else {
            None
        };
        Some(UseDecl {
            path,
            alias,
            glob,
            span,
        })
    }

    /// Parse a dot-separated path like `package.contract.PackageContract`.
    fn parse_dotted_path(&mut self) -> Option<Vec<String>> {
        let mut path = Vec::new();
        let first = self.take_ident_name()?;
        path.push(first);
        while self.at_symbol(".") {
            self.index += 1;
            if let Some(segment) = self.take_ident_name() {
                path.push(segment);
            } else {
                break;
            }
        }
        Some(path)
    }

    fn is_eof(&self) -> bool {
        !parse_is_active()
            || matches!(
                self.tokens.get(self.index).map(|token| &token.kind),
                Some(TokenKind::Eof) | None
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_explicit_generic_function_call_arguments() {
        let program = parse_source(
            "test.rss",
            r#"
fn run() -> Unit {
    Json.array_fold<RemapFacts>(
        value: read diagnostics,
        initial: read empty_remap_facts(),
        folder: |facts, item| {
            return diagnose_remap_item(item: read item, facts: read facts)
        },
    )
}
"#,
        );
        let Item::Function(function) = &program.items[0] else {
            panic!("expected function");
        };
        let Stmt::Expr(Expr::Call { callee, args, .. }) = &function.body.statements[0] else {
            panic!("expected call");
        };
        assert_eq!(
            callee,
            &Callee::Qualified {
                namespace: "Json".to_string(),
                name: "array_fold<RemapFacts>".to_string(),
            }
        );
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn let_else_keeps_every_statement_of_its_else_block() {
        let source = "fn unwrap(value: Option<String>) -> String {\n    let Some(inner) = value else {\n        return \"default\"\n    }\n    return inner\n}\n";
        let program = parse_source("test.rss", source);
        let Item::Function(function) = &program.items[0] else {
            panic!("expected function");
        };
        let Stmt::LetElse(let_else) = &function.body.statements[0] else {
            panic!("expected let-else, got {:?}", function.body.statements[0]);
        };
        assert_eq!(
            let_else.else_body.statements.len(),
            1,
            "the else block's first statement must survive: {:?}",
            let_else.else_body.statements
        );
        assert!(
            matches!(&let_else.else_body.statements[0], Stmt::Return(_)),
            "the diverging `return` must be parsed as a return, got {:?}",
            let_else.else_body.statements[0]
        );
    }

    #[test]
    fn canonicalizes_omitted_function_type_effects_as_read() {
        let program = parse_source("test.rss", "fn apply(f: Fn(Int) -> Int) -> Unit {}");
        let Item::Function(function) = &program.items[0] else {
            panic!("expected function");
        };

        assert_eq!(
            types::type_ref_name(&function.params[0].ty),
            "Fn(read Int) -> Int"
        );
    }

    #[test]
    fn parses_multiline_if_expression() {
        let source = "fn choose(flag: Bool) -> Int {\n    return if flag {\n        1\n    } else {\n        2\n    }\n}\n";
        let program = parse_source("test.rss", source);
        let Item::Function(function) = &program.items[0] else {
            panic!("expected function");
        };
        let Stmt::Return(return_stmt) = &function.body.statements[0] else {
            panic!("expected return");
        };
        let Some(Expr::Match {
            value,
            arms,
            from_if_expression,
            ..
        }) = &return_stmt.value
        else {
            panic!("expected if expression, got {:?}", return_stmt.value);
        };
        assert!(matches!(value.as_ref(), Expr::Ident(name, _) if name == "flag"));
        assert!(*from_if_expression);
        assert!(matches!(
            arms.as_slice(),
            [
                MatchArm {
                    pattern: MatchPattern::Literal {
                        value: MatchLiteral::Bool(true),
                        ..
                    },
                    ..
                },
                MatchArm {
                    pattern: MatchPattern::Literal {
                        value: MatchLiteral::Bool(false),
                        ..
                    },
                    ..
                },
            ]
        ));
    }

    /// Every parser keyword must really be matched by a production in this
    /// module. The parser recognizes a positional keyword through exactly three
    /// forms, so scanning its own sources for them ties the published grammar
    /// surface to the parser's behavior instead of to a hand-kept list.
    #[test]
    fn parser_keywords_are_matched_by_a_parser_production() {
        const SOURCES: &[&str] = &[
            include_str!("mod.rs"),
            include_str!("expr.rs"),
            include_str!("items.rs"),
            include_str!("pattern.rs"),
            include_str!("scan.rs"),
            include_str!("stmt.rs"),
            include_str!("types.rs"),
        ];

        for (word, _) in PARSER_KEYWORDS {
            let matched = SOURCES.iter().any(|source| {
                source.contains(&format!("is_ident_text(\"{word}\")"))
                    || source.contains(&format!("at_ident(\"{word}\")"))
                    || source.contains(&format!("name == \"{word}\""))
                    || source.contains(&format!("(\"{word}\")"))
            });
            assert!(
                matched,
                "`{word}` is published as a parser keyword but no production matches it"
            );
        }
    }

    /// A parser keyword is by definition one the lexer does *not* reserve, so
    /// the three published tables stay disjoint.
    #[test]
    fn parser_keywords_are_disjoint_from_the_lexer_tables() {
        for (word, _) in PARSER_KEYWORDS {
            assert!(
                !crate::lexer::KEYWORDS.iter().any(|(kw, _)| kw == word),
                "`{word}` is already a reserved keyword"
            );
            assert!(
                !crate::lexer::CONTEXTUAL_KEYWORDS
                    .iter()
                    .any(|(kw, _)| kw == word),
                "`{word}` is already a contextual keyword"
            );
        }
    }

    /// The single statement of `function`'s body, by index.
    fn body_statement(program: &Program, index: usize) -> &Stmt {
        let Item::Function(function) = &program.items[index] else {
            panic!("expected a function at item {index}");
        };
        &function.body.statements[0]
    }

    #[test]
    fn brace_struct_literal_parses_as_the_canonical_constructor_call() {
        let program = parse_source(
            "test.rss",
            "fn build(title: take String) -> Report {\n    return Report { title: take title, count: 0 }\n}\n",
        );
        let Stmt::Return(ReturnStmt {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        }) = body_statement(&program, 0)
        else {
            panic!("expected a constructor call, got {:?}", program.items[0]);
        };

        assert_eq!(callee, &Callee::Name("Report".to_string()));
        assert_eq!(
            args.iter()
                .map(|arg| arg.name.clone())
                .collect::<Vec<_>>()
                .as_slice(),
            [Some("title".to_string()), Some("count".to_string())]
        );
        assert!(args.iter().all(|arg| !arg.malformed));
        assert!(matches!(
            &args[0].value,
            Expr::Effect {
                effect: DataEffect::Take,
                ..
            }
        ));
    }

    #[test]
    fn brace_struct_literal_accepts_a_trailing_comma_and_a_qualified_head() {
        let program = parse_source(
            "test.rss",
            "fn build() -> Report {\n    return shapes.Report {\n        title: \"t\",\n        count: 0,\n    }\n}\n",
        );
        let Stmt::Return(ReturnStmt {
            value: Some(Expr::Call { callee, args, .. }),
            ..
        }) = body_statement(&program, 0)
        else {
            panic!("expected a constructor call");
        };

        assert_eq!(
            callee,
            &Callee::Qualified {
                namespace: "shapes".to_string(),
                name: "Report".to_string(),
            }
        );
        assert_eq!(args.len(), 2);
    }

    /// A brace group that is not `Head { field: value, ... }` must keep falling
    /// through to the other productions, so `select`/`task_group` in value
    /// position still report unsupported syntax instead of becoming a call.
    #[test]
    fn brace_struct_literal_does_not_swallow_statement_only_blocks() {
        let program = parse_source(
            "test.rss",
            "fn run() -> Unit {\n    let handle = select { ready = await Stream.next(stream: mut stream) => { return Unit } }\n}\n",
        );
        let Stmt::Let(LetStmt { value, .. }) = body_statement(&program, 0) else {
            panic!("expected a let statement");
        };
        assert!(
            !matches!(value, Some(Expr::Call { .. })),
            "select in value position must not parse as a constructor call: {value:?}"
        );
    }

    #[test]
    fn expression_match_arm_with_a_trailing_comma_parses_as_the_canonical_block_arm() {
        let sugar = parse_source(
            "test.rss",
            "fn classify(value: Int) -> Int {\n    return match value {\n        0 => 10,\n        _ => 20,\n    }\n}\n",
        );
        let canonical = parse_source(
            "test.rss",
            "fn classify(value: Int) -> Int {\n    return match value {\n        0 => { 10 }\n        _ => { 20 }\n    }\n}\n",
        );

        let arm_shapes = |program: &Program| {
            let Stmt::Return(ReturnStmt {
                value:
                    Some(Expr::Match {
                        arms,
                        malformed_arm_spans,
                        ..
                    }),
                ..
            }) = body_statement(program, 0)
            else {
                panic!("expected a match expression");
            };
            assert!(malformed_arm_spans.is_empty(), "arm failed to parse");
            arms.iter()
                .map(|arm| {
                    assert_eq!(arm.body.statements.len(), 1);
                    let Stmt::Expr(Expr::Number(literal, _)) = &arm.body.statements[0] else {
                        panic!("expected a single expression statement, got {:?}", arm.body);
                    };
                    (
                        format!("{:?}", arm.pattern.binding_names()),
                        literal.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(arm_shapes(&sugar), arm_shapes(&canonical));
        assert_eq!(arm_shapes(&sugar).len(), 2);
    }

    #[test]
    fn block_match_arm_accepts_a_trailing_comma() {
        let program = parse_source(
            "test.rss",
            "fn classify(value: Int) -> Int {\n    match value {\n        0 => { return 1 },\n        _ => { return 2 },\n    }\n}\n",
        );
        let Stmt::Match(MatchStmt {
            arms,
            malformed_arm_spans,
            ..
        }) = body_statement(&program, 0)
        else {
            panic!("expected a match statement");
        };
        assert!(malformed_arm_spans.is_empty());
        assert_eq!(arms.len(), 2);
    }

    /// The sugar must not merge two comma-free arms: an arm without a trailing
    /// comma still ends at its own line.
    #[test]
    fn comma_free_expression_arms_stay_separate() {
        let program = parse_source(
            "test.rss",
            "fn classify(value: Int) -> Int {\n    match value {\n        0 => return 1\n        _ => return 2\n    }\n}\n",
        );
        let Stmt::Match(MatchStmt { arms, .. }) = body_statement(&program, 0) else {
            panic!("expected a match statement");
        };
        assert_eq!(arms.len(), 2);
        assert!(arms.iter().all(|arm| arm.body.statements.len() == 1));
    }

    /// Tuple arity has no alphabet-sized ceiling: the synthetic `__TupleN`
    /// struct's type parameters are generated from the element index, so at
    /// any arity they stay unique, stay identifiers, and stay inside the
    /// `__rss_` namespace reserved for compiler-generated symbols — a user
    /// type or type parameter can never capture one.
    #[test]
    fn wide_tuple_structs_declare_unique_reserved_type_parameters() {
        const ARITY: usize = 30;
        let elements = std::iter::repeat_n("Int", ARITY)
            .collect::<Vec<_>>()
            .join(", ");
        let program = parse_source(
            "test.rss",
            &format!("fn wide(value: ({elements})) -> Unit {{}}\n"),
        );

        let expected_name = format!("__Tuple{ARITY}");
        let Some(Item::Type(tuple)) = program
            .items
            .iter()
            .find(|item| matches!(item, Item::Type(decl) if decl.name == expected_name))
        else {
            panic!("an arity-{ARITY} tuple injects its synthetic struct");
        };
        assert_eq!(tuple.type_params.len(), ARITY);
        assert_eq!(tuple.fields.len(), ARITY);

        let names = tuple
            .type_params
            .iter()
            .map(|parameter| parameter.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), ARITY, "every element needs its own parameter");
        for name in &names {
            assert!(
                name.starts_with("__rss_"),
                "`{name}` must stay in the reserved generated namespace"
            );
            assert!(
                name.chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_'),
                "`{name}` must stay an identifier"
            );
        }
        // Field `itemI` is typed by the parameter declared at position `I`.
        for (index, field) in tuple.fields.iter().enumerate() {
            assert_eq!(field.name, format!("item{index}"));
            assert_eq!(field.ty.name, tuple.type_params[index].name);
        }
    }

    fn select_statement(source: &str) -> SelectStmt {
        let program = parse_source("test.rss", source);
        let Stmt::Select(select) = body_statement(&program, 0) else {
            panic!("expected a select statement");
        };
        select.clone()
    }

    /// An arm written without its binding used to be skipped outright, so
    /// `select { await op => { ... } }` parsed to a `select` with zero arms and
    /// checked clean. The arm must be recorded as malformed instead.
    #[test]
    fn select_arm_without_a_binding_is_recorded_as_malformed() {
        let select = select_statement(
            "fn run() -> Unit {\n    select {\n        await Receiver.recv(receiver: rx) => {\n            return Unit\n        }\n    }\n}\n",
        );

        assert!(
            select.arms.is_empty(),
            "an arm with no binding is not a parsed arm: {:?}",
            select.arms
        );
        assert_eq!(
            select.malformed_arm_spans.len(),
            1,
            "the dropped arm must leave a malformed span behind"
        );
    }

    /// The malformed arm must not swallow the arms written after it: recovery
    /// steps past the arm body, so a well-formed neighbour still parses.
    #[test]
    fn select_recovers_from_a_binding_less_arm_and_keeps_the_next_arm() {
        let select = select_statement(
            "fn run() -> Unit {\n    select {\n        await Receiver.recv(receiver: left) => {\n            return Unit\n        }\n        right = await Receiver.recv(receiver: right_rx) => {\n            return Unit\n        }\n    }\n}\n",
        );

        assert_eq!(select.malformed_arm_spans.len(), 1);
        assert_eq!(select.arms.len(), 1, "the well-formed arm must survive");
        assert_eq!(select.arms[0].binding, "right");
    }

    /// `_` is the spelling for an arm whose value the body does not use, and it
    /// stays a real arm.
    #[test]
    fn select_arm_bound_to_underscore_is_a_well_formed_arm() {
        let select = select_statement(
            "fn run() -> Unit {\n    select {\n        _ = await Receiver.recv(receiver: rx) => {\n            return Unit\n        }\n    }\n}\n",
        );

        assert!(select.malformed_arm_spans.is_empty());
        assert_eq!(select.arms.len(), 1);
        assert_eq!(select.arms[0].binding, "_");
    }

    /// Text inside `select { ... }` that has no `=>` at all is also kept as a
    /// malformed span rather than skipped.
    #[test]
    fn select_body_text_without_an_arrow_is_recorded_as_malformed() {
        let select =
            select_statement("fn run() -> Unit {\n    select {\n        let x = 1\n    }\n}\n");

        assert!(select.arms.is_empty());
        assert!(!select.malformed_arm_spans.is_empty());
    }

    /// An empty `select` parses cleanly here; the "nothing to wait on" rule is
    /// the checker's, and it needs the arm list to be honestly empty.
    #[test]
    fn select_with_no_arms_parses_to_an_empty_arm_list() {
        let select = select_statement("fn run() -> Unit {\n    select {\n    }\n}\n");

        assert!(select.arms.is_empty());
        assert!(select.malformed_arm_spans.is_empty());
    }
}
