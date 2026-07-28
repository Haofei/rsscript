use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use crate::checks::budget::{FrontendBudget, FrontendBudgetLimits, ParseRecursionGuard};
use crate::diagnostic::Span;
use crate::lexer::{Token, TokenKind, lex_with_budget};
use crate::syntax::ast::{
    AssignStmt, BinaryOp, Block, CallArg, Callee, ConstDecl, DataEffect, DuplicateFileFeature,
    EffectDecl, Expr, FieldDecl, FileFeature, FileFeatureScope, ForStmt, FunctionDecl,
    GenericBound, GenericParam, IfStmt, Item, LetElseStmt, LetKind, LetStmt, LoopStmt,
    MapLiteralEntry, MatchArm, MatchFieldPattern, MatchLiteral, MatchPattern, MatchStmt,
    ModuleDecl, ObjectLiteralField, Param, Program, ProtocolDecl, ProtocolImpl,
    ProtocolImplMapping, ReturnStmt, SelectArm, SelectStmt, Stmt, SumTypeDecl, SumVariant,
    TaskGroupStmt, TypeAliasDecl, TypeDecl, TypeKind, TypeRef, UnknownFileFeature, UseDecl,
    WithStmt,
};

mod expr;
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

pub(crate) fn parse_source_tokens(
    file: &str,
    tokens: &[Token],
    budget: Rc<FrontendBudget>,
) -> Program {
    let mut program = parse_source_tokens_raw(file, tokens, budget.clone());
    if budget.check_active() {
        super::desugar::desugar_associated_consts(&mut program);
        super::desugar::expand_tuple_destructuring(&mut program);
        super::desugar::inject_tuple_structs(&mut program);
    }
    program
}

impl Program {
    pub(crate) fn parse_tokens(file: &str, tokens: &[Token], budget: Rc<FrontendBudget>) -> Self {
        parse_source_tokens(file, tokens, budget)
    }
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

fn parse_source_tokens_raw(file: &str, tokens: &[Token], budget: Rc<FrontendBudget>) -> Program {
    let _active_budget = ActiveParseBudget::set(budget);
    Parser {
        tokens,
        index: 0,
        file,
    }
    .parse_program()
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
    file: &'a str,
}

struct ParsedFeatures {
    features: Vec<FileFeature>,
    unknown_features: Vec<UnknownFileFeature>,
    duplicate_features: Vec<DuplicateFileFeature>,
}

impl Parser<'_> {
    fn parse_program(&mut self) -> Program {
        let _parse = enter_parse();
        let mut features = Vec::new();
        let mut unknown_features = Vec::new();
        let mut duplicate_features = Vec::new();
        let mut feature_spans = Vec::new();
        let mut profile_spans = Vec::new();
        let mut unknown_top_level_spans = Vec::new();
        let mut malformed_declaration_spans = Vec::new();
        let mut protocols = Vec::new();
        let mut protocol_impls = Vec::new();
        let mut items = Vec::new();

        while parse_is_active() && !self.is_eof() {
            if self.at_ident("features") && self.peek_symbol(1, ":") {
                feature_spans.push(self.tokens[self.index].span.clone());
                let parsed = self.parse_features();
                features.extend(parsed.features);
                unknown_features.extend(parsed.unknown_features);
                duplicate_features.extend(parsed.duplicate_features);
            } else if self.at_ident("profile") && self.peek_symbol(1, ":") {
                profile_spans.push(self.tokens[self.index].span.clone());
                self.index += 1;
            } else if self.at_ident("class")
                || self.at_ident("struct")
                || self.at_ident("resource")
                || self.at_ident("opaque")
                || (self.at_ident("pub")
                    && (self.peek_ident(1, "class")
                        || self.peek_ident(1, "struct")
                        || self.peek_ident(1, "resource")
                        || self.peek_ident(1, "opaque")))
            {
                let start = self.index;
                if let Some(item) = self.parse_type_decl() {
                    items.push(Item::Type(item));
                } else {
                    malformed_declaration_spans.push(self.tokens[start].span.clone());
                    self.index = skip_unknown_top_level(self.tokens, start);
                }
            } else if self.at_ident("sum") || (self.at_ident("pub") && self.peek_ident(1, "sum")) {
                let start = self.index;
                if let Some(item) = self.parse_sum_type_decl() {
                    items.push(Item::SumType(item));
                } else {
                    malformed_declaration_spans.push(self.tokens[start].span.clone());
                    self.index = skip_unknown_top_level(self.tokens, start);
                }
            } else if self.at_ident("protocol") {
                let start = self.index;
                if let Some((protocol, functions)) = self.parse_protocol_decl() {
                    protocols.push(protocol);
                    items.extend(functions.into_iter().map(Item::Function));
                } else {
                    malformed_declaration_spans.push(self.tokens[start].span.clone());
                    self.index = skip_unknown_top_level(self.tokens, start);
                }
            } else if self.at_ident("impl") {
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
            } else if self.at_ident("native") && self.peek_ident(1, "module") {
                let start = self.index;
                if let Some(functions) = self.parse_native_module_decl() {
                    items.extend(functions.into_iter().map(Item::Function));
                } else {
                    malformed_declaration_spans.push(self.tokens[start].span.clone());
                    self.index = skip_unknown_top_level(self.tokens, start);
                }
            } else if self.at_ident("type") || (self.at_ident("pub") && self.peek_ident(1, "type"))
            {
                let start = self.index;
                if let Some(alias) = self.parse_type_alias_decl() {
                    items.push(Item::TypeAlias(alias));
                } else {
                    malformed_declaration_spans.push(self.tokens[start].span.clone());
                    self.index = skip_unknown_top_level(self.tokens, start);
                }
            } else if self.at_ident("const")
                || (self.at_ident("pub") && self.peek_ident(1, "const"))
            {
                let start = self.index;
                if let Some(decl) = self.parse_const_decl() {
                    items.push(Item::Const(decl));
                } else {
                    malformed_declaration_spans.push(self.tokens[start].span.clone());
                    self.index = skip_unknown_top_level(self.tokens, start);
                }
            } else if self.at_ident("module") && !self.peek_ident(1, "{") {
                if let Some(decl) = self.parse_module_decl() {
                    items.push(Item::Module(decl));
                } else {
                    self.index += 1;
                }
            } else if self.at_ident("use") {
                if let Some(decl) = self.parse_use_decl() {
                    items.push(Item::Use(decl));
                } else {
                    self.index += 1;
                }
            } else if self.at_ident("pub")
                || self.at_ident("async")
                || self.at_ident("native")
                || self.at_ident("fn")
                || self.at_symbol("#")
            {
                let start = self.index;
                if let Some(item) = self.parse_function_decl() {
                    items.push(Item::Function(item));
                } else {
                    malformed_declaration_spans.push(self.tokens[start].span.clone());
                    self.index = skip_unknown_top_level(self.tokens, start);
                }
            } else {
                unknown_top_level_spans.push(self.tokens[self.index].span.clone());
                self.index = skip_unknown_top_level(self.tokens, self.index);
            }
        }

        Program {
            feature_scopes: vec![FileFeatureScope {
                file: self.file.to_string(),
                features: features.clone(),
            }],
            features,
            unknown_features,
            duplicate_features,
            feature_spans,
            profile_spans,
            unknown_top_level_spans,
            malformed_declaration_spans,
            protocols,
            protocol_impls,
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
        let parsed_type_params = self.parse_generic_params();
        let type_params = parsed_type_params.params;
        let malformed_generic_param_spans = parsed_type_params.malformed_spans;

        let mut params = Vec::new();
        let mut malformed_param_spans = Vec::new();
        if self.at_symbol("(") {
            let open = self.index;
            let close = find_matching(self.tokens, open, "(", ")")?;
            let parsed_params = parse_params(self.tokens, open + 1, close);
            params = parsed_params.params;
            malformed_param_spans = parsed_params.malformed_spans;
            self.index = close + 1;
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
                && !self.at_ident("effects")
                && !self.at_symbol("{")
                && !self.at_symbol("=")
            {
                if self.at_ident("fresh") {
                    returns_fresh = true;
                }
                self.index += 1;
            }
            return_ty = parse_type_ref(self.tokens, return_start, self.index);
        }

        let mut effects = Vec::new();
        let mut malformed_effect_spans = Vec::new();
        if self.index < signature_end && self.at_ident("effects") && self.peek_symbol(1, "(") {
            let open = self.index + 1;
            let close = find_matching(self.tokens, open, "(", ")")?;
            let parsed_effects = parse_effects(self.tokens, open + 1, close);
            effects = parsed_effects.effects;
            malformed_effect_spans = parsed_effects.malformed_spans;
            self.index = close + 1;
        }
        let default_impl_marker =
            self.index + 1 < signature_end && self.at_symbol("=") && self.peek_ident(1, "_");
        if default_impl_marker {
            self.index += 2;
        }
        let (has_body, body) = if self.at_symbol("{") {
            let open = self.index;
            let close = find_matching(self.tokens, open, "{", "}")?;
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
            is_native,
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
            effects,
            malformed_effect_spans,
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
    // exactly what the flat spelling produces — no new capability, grouping only.
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

    fn parse_native_module_decl(&mut self) -> Option<Vec<FunctionDecl>> {
        self.index += 2;
        let module = self.take_ident_name()?;
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
            function.name = if function.name.contains('.') {
                function.name
            } else {
                format!("{module}.{}", function.name)
            };
            function.is_public = true;
            function.is_native = true;
            if !function
                .effects
                .iter()
                .any(|effect| matches!(effect, EffectDecl::Name(name) if name == "native"))
            {
                function
                    .effects
                    .push(EffectDecl::Name("native".to_string()));
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
}
