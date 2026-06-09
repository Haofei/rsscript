use std::collections::HashSet;

use crate::diagnostic::Span;
use crate::lexer::{Token, TokenKind, lex};
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

pub fn parse_source(file: &str, source: &str) -> Program {
    let tokens = lex(file, source);
    Parser {
        tokens: &tokens,
        index: 0,
        file,
    }
    .parse_program()
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
                if let Some(protocol_impl) = self.parse_protocol_impl_decl() {
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
        let name = self.take_ident_name()?;
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
        while self.at_symbol("#") {
            self.index += 1;
            if !self.at_ident("deprecated") {
                self.index = start;
                return None;
            }
            self.index += 1;
            if !self.at_symbol("(") {
                self.index = start;
                return None;
            }
            self.index += 1;
            let Some(TokenKind::String(reason)) = self.current().map(|token| &token.kind) else {
                self.index = start;
                return None;
            };
            deprecated_reason = Some(reason.clone());
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
        Some(UseDecl { path, span })
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
        matches!(
            self.tokens.get(self.index).map(|token| &token.kind),
            Some(TokenKind::Eof) | None
        )
    }
}

struct ParsedFields {
    fields: Vec<FieldDecl>,
    malformed_spans: Vec<crate::diagnostic::Span>,
}

struct ParsedParams {
    params: Vec<Param>,
    malformed_spans: Vec<crate::diagnostic::Span>,
}

struct ParsedGenericParams {
    params: Vec<GenericParam>,
    malformed_spans: Vec<crate::diagnostic::Span>,
}

struct ParsedEffects {
    effects: Vec<EffectDecl>,
    malformed_spans: Vec<crate::diagnostic::Span>,
}

struct ParamRange {
    start: usize,
    end: usize,
    empty_span: Option<crate::diagnostic::Span>,
}

fn parse_fields(tokens: &[Token], start: usize, end: usize) -> ParsedFields {
    let mut fields = Vec::new();
    let mut malformed_spans = Vec::new();
    let mut index = start;
    while index < end {
        if is_trivia_boundary(&tokens[index]) {
            index += 1;
            continue;
        }
        if tokens[index].is_ident_text("drop") {
            index = skip_braced_block(tokens, index).unwrap_or(end);
            continue;
        }

        let name_index = index;
        let line_end = next_line_or_block_end(tokens, index, end);
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
            } else {
                malformed_spans.push(tokens[name_index].span.clone());
            }
        } else {
            malformed_spans.push(tokens[name_index].span.clone());
        }

        index = line_end.max(index + 1);
    }
    ParsedFields {
        fields,
        malformed_spans,
    }
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

fn parse_params(tokens: &[Token], start: usize, end: usize) -> ParsedParams {
    let mut params = Vec::new();
    let mut malformed_spans = Vec::new();
    for range in split_param_ranges(tokens, start, end) {
        if let Some(span) = range.empty_span {
            malformed_spans.push(span);
            continue;
        }
        let start = range.start;
        let end = range.end;
        let Some(name) = tokens.get(start).and_then(ident_name) else {
            if let Some(token) = tokens.get(start) {
                malformed_spans.push(token.span.clone());
            }
            continue;
        };
        if parse_data_effect(tokens.get(start)).is_some() {
            malformed_spans.push(tokens[start].span.clone());
            continue;
        }

        if !tokens.get(start + 1).is_some_and(|token| token.symbol(":")) {
            params.push(Param {
                name: name.to_string(),
                effect: None,
                ty: TypeRef {
                    name: String::new(),
                    args: Vec::new(),
                    malformed_arg_spans: Vec::new(),
                    is_fresh: false,
                    is_noescape: false,
                    is_owned: false,
                    fn_params: Vec::new(),
                    fn_return: None,
                    span: tokens[start].span.clone(),
                },
                span: tokens[start].span.clone(),
            });
            continue;
        }

        let mut ty_start = start + 2;
        let effect = parse_data_effect(tokens.get(ty_start)).inspect(|_| {
            ty_start += 1;
        });
        let ty = parse_type_ref(tokens, ty_start, end).unwrap_or_else(|| TypeRef {
            name: String::new(),
            args: Vec::new(),
            malformed_arg_spans: Vec::new(),
            is_fresh: false,
            is_noescape: false,
            is_owned: false,
            fn_params: Vec::new(),
            fn_return: None,
            span: tokens[start].span.clone(),
        });
        params.push(Param {
            name: name.to_string(),
            effect,
            ty,
            span: tokens[start].span.clone(),
        });
    }

    ParsedParams {
        params,
        malformed_spans,
    }
}

fn split_param_ranges(tokens: &[Token], start: usize, end: usize) -> Vec<ParamRange> {
    let mut ranges = Vec::new();
    let mut range_start = start;
    let mut depth = 0usize;
    let mut pipe_depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if depth == 0 && token.symbol("|") {
            pipe_depth = 1usize.saturating_sub(pipe_depth);
        } else if pipe_depth == 0
            && (token.symbol("(") || token.symbol("{") || token.symbol("[") || token.symbol("<"))
        {
            depth += 1;
        } else if pipe_depth == 0
            && (token.symbol(")") || token.symbol("}") || token.symbol("]") || token.symbol(">"))
        {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && pipe_depth == 0 && token.symbol(",") {
            if range_start < index {
                ranges.push(ParamRange {
                    start: range_start,
                    end: index,
                    empty_span: None,
                });
            } else {
                ranges.push(ParamRange {
                    start: index,
                    end: index,
                    empty_span: Some(token.span.clone()),
                });
            }
            range_start = index + 1;
        }
    }
    if range_start < end {
        ranges.push(ParamRange {
            start: range_start,
            end,
            empty_span: None,
        });
    }
    ranges
}

fn parse_effects(tokens: &[Token], start: usize, end: usize) -> ParsedEffects {
    let mut effects = Vec::new();
    let mut malformed_spans = Vec::new();
    for range in split_param_ranges(tokens, start, end) {
        if let Some(span) = range.empty_span {
            malformed_spans.push(span);
            continue;
        }
        let start = range.start;
        let end = range.end;
        let Some(name) = tokens.get(start).and_then(ident_name) else {
            if let Some(token) = tokens.get(start) {
                malformed_spans.push(token.span.clone());
            }
            continue;
        };
        if name == "retains" {
            if tokens.get(start + 1).is_some_and(|token| token.symbol("("))
                && let Some(close) = find_matching(tokens, start + 1, "(", ")")
                && close + 1 == end
                && start + 3 == close
                && let Some(param) = tokens.get(start + 2).and_then(ident_name)
            {
                effects.push(EffectDecl::Retains(param.to_string()));
            } else {
                malformed_spans.push(tokens[start].span.clone());
            }
        } else if start + 1 == end {
            effects.push(EffectDecl::Name(name.to_string()));
        } else {
            malformed_spans.push(tokens[start].span.clone());
        }
    }
    ParsedEffects {
        effects,
        malformed_spans,
    }
}

fn parse_generic_params(tokens: &[Token], start: usize, end: usize) -> ParsedGenericParams {
    let mut params = Vec::new();
    let mut malformed_spans = Vec::new();
    for range in split_param_ranges(tokens, start, end) {
        if let Some(span) = range.empty_span {
            malformed_spans.push(span);
            continue;
        }
        let start = range.start;
        let end = range.end;
        let Some(name) = tokens.get(start).and_then(ident_name) else {
            if let Some(token) = tokens.get(start) {
                malformed_spans.push(token.span.clone());
            }
            continue;
        };

        let bound = if start + 1 == end {
            None
        } else if tokens.get(start + 1).is_some_and(|token| token.symbol(":")) && start + 3 == end {
            let Some(bound) = tokens.get(start + 2).and_then(parse_generic_bound) else {
                malformed_spans.push(tokens[start].span.clone());
                continue;
            };
            Some(bound)
        } else {
            malformed_spans.push(tokens[start].span.clone());
            continue;
        };

        params.push(GenericParam {
            name: name.to_string(),
            bound,
            span: tokens[start].span.clone(),
        });
    }
    ParsedGenericParams {
        params,
        malformed_spans,
    }
}

fn parse_generic_bound(token: &Token) -> Option<GenericBound> {
    if token.is_ident_text("Managed") {
        Some(GenericBound::Managed)
    } else if token.is_ident_text("Struct") {
        Some(GenericBound::Struct)
    } else if token.is_ident_text("Resource") {
        Some(GenericBound::Resource)
    } else {
        ident_name(token).map(|name| GenericBound::Protocol(name.to_string()))
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

struct IsCondition {
    value: Expr,
    effect: DataEffect,
    pattern: MatchPattern,
    body_open: usize,
    body_close: usize,
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

fn parse_match_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    // match <value> { arms... }
    // The arms block is the top-level brace group whose close ends the expression.
    let brace_start = find_match_expr_arms_open(tokens, start + 1, end)?;
    let Some(close) = find_matching(tokens, brace_start, "{", "}") else {
        return None;
    };
    // The closing brace must be the end of the expression
    if close + 1 != end {
        return None;
    }
    let (scrutinee_effect, value_start) =
        parse_match_scrutinee_effect(tokens, start + 1, brace_start);
    let value = parse_expr(tokens, value_start, brace_start)?;
    let parsed_arms = parse_match_arms(tokens, brace_start + 1, close);
    if parsed_arms.arms.is_empty() {
        return None;
    }
    Some(Expr::Match {
        value: Box::new(value),
        scrutinee_effect,
        arms: parsed_arms.arms,
        span: tokens[start].span.clone(),
    })
}

fn parse_match_scrutinee_effect(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> (Option<DataEffect>, usize) {
    if start < end
        && let Some(effect) = parse_data_effect(tokens.get(start))
    {
        (Some(effect), start + 1)
    } else {
        (None, start)
    }
}

fn find_match_expr_arms_open(tokens: &[Token], start: usize, end: usize) -> Option<usize> {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") {
            paren_depth += 1;
        } else if token.symbol(")") {
            paren_depth = paren_depth.saturating_sub(1);
        } else if token.symbol("[") {
            bracket_depth += 1;
        } else if token.symbol("]") {
            bracket_depth = bracket_depth.saturating_sub(1);
        } else if paren_depth == 0 && bracket_depth == 0 && token.symbol("{") {
            if find_matching(tokens, index, "{", "}").is_some_and(|close| close + 1 == end) {
                return Some(index);
            }
        }
    }
    None
}

struct ParsedMatchArms {
    arms: Vec<MatchArm>,
    malformed_spans: Vec<crate::diagnostic::Span>,
}

fn parse_match_arms(tokens: &[Token], start: usize, end: usize) -> ParsedMatchArms {
    let mut arms = Vec::new();
    let mut malformed_spans = Vec::new();
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
            malformed_spans.push(tokens[index].span.clone());
            index = line_end.max(index + 1);
            continue;
        };
        let (pattern_end, guard) = split_match_pattern_guard(tokens, index, arrow).map_or(
            (arrow, None),
            |(pattern_end, guard_start)| {
                (
                    pattern_end,
                    parse_expr(tokens, guard_start, arrow)
                        .or_else(|| Some(Expr::Unknown(tokens[guard_start].span.clone()))),
                )
            },
        );
        let pattern = if let Some(pattern) = parse_match_pattern(tokens, index, pattern_end) {
            pattern
        } else {
            malformed_spans.push(tokens[index].span.clone());
            MatchPattern::Wildcard(tokens[index].span.clone())
        };
        let body_start = arrow + 1;
        let (body, next) = if tokens
            .get(body_start)
            .is_some_and(|token| token.symbol("{"))
        {
            let Some(body_close) = find_matching(tokens, body_start, "{", "}") else {
                malformed_spans.push(tokens[body_start].span.clone());
                break;
            };
            (parse_block(tokens, body_start, body_close), body_close + 1)
        } else {
            if body_start >= end {
                malformed_spans.push(tokens[arrow].span.clone());
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
        arms.push(MatchArm {
            pattern,
            guard,
            body,
            span: tokens[index].span.clone(),
        });
        index = next;
    }
    ParsedMatchArms {
        arms,
        malformed_spans,
    }
}

fn split_match_pattern_guard(tokens: &[Token], start: usize, end: usize) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().take(end).skip(start) {
        if token.symbol("(") || token.symbol("{") || token.symbol("[") {
            depth += 1;
        } else if token.symbol(")") || token.symbol("}") || token.symbol("]") {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token.is_ident_text("if") {
            return Some((index, index + 1));
        }
    }
    None
}

fn parse_match_pattern(tokens: &[Token], start: usize, end: usize) -> Option<MatchPattern> {
    let (start, end) = trim_outer(tokens, start, end);
    if start >= end {
        return None;
    }
    if start + 1 == end {
        match &tokens[start].kind {
            TokenKind::Number(value) => {
                return Some(MatchPattern::Literal {
                    value: MatchLiteral::Int(value.clone()),
                    span: tokens[start].span.clone(),
                });
            }
            TokenKind::String(value) => {
                return Some(MatchPattern::Literal {
                    value: MatchLiteral::String(value.clone()),
                    span: tokens[start].span.clone(),
                });
            }
            TokenKind::Ident(value) if matches!(value.as_str(), "true" | "false") => {
                return Some(MatchPattern::Literal {
                    value: MatchLiteral::Bool(*value == "true"),
                    span: tokens[start].span.clone(),
                });
            }
            TokenKind::Keyword(value) if matches!(*value, "true" | "false") => {
                return Some(MatchPattern::Literal {
                    value: MatchLiteral::Bool(*value == "true"),
                    span: tokens[start].span.clone(),
                });
            }
            _ => {}
        }
    }
    let name = ident_name(&tokens[start])?.to_string();
    if name == "_" {
        return Some(MatchPattern::Wildcard(tokens[start].span.clone()));
    }
    if tokens.get(start + 1).is_some_and(|token| token.symbol("{")) {
        let close = find_matching(tokens, start + 1, "{", "}")?;
        if close + 1 != end {
            return None;
        }
        let (fields, has_rest) = parse_match_field_patterns(tokens, start + 2, close)?;
        return Some(MatchPattern::Struct {
            name,
            fields,
            has_rest,
            span: tokens[start].span.clone(),
        });
    }
    let binding = if tokens.get(start + 1).is_some_and(|token| token.symbol("(")) {
        let close = find_matching(tokens, start + 1, "(", ")")?;
        if close + 1 != end {
            return None;
        }
        if start + 2 == close {
            None
        } else if start + 3 == close && tokens[start + 2].is_ident_text("_") {
            Some(Box::new(MatchPattern::Wildcard(
                tokens[start + 2].span.clone(),
            )))
        } else if start + 3 == close {
            parse_single_payload_pattern(tokens, start + 2)
        } else {
            parse_match_pattern(tokens, start + 2, close).map(Box::new)
        }
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

fn parse_match_field_patterns(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Option<(Vec<MatchFieldPattern>, bool)> {
    let mut fields = Vec::new();
    let mut has_rest = false;
    for range in split_param_ranges(tokens, start, end) {
        if range.empty_span.is_some() {
            continue;
        }
        if (range.start + 1 == range.end && tokens[range.start].symbol(".."))
            || (range.start + 2 == range.end
                && tokens[range.start].symbol(".")
                && tokens[range.start + 1].symbol("."))
        {
            has_rest = true;
            continue;
        }
        let name = ident_name(tokens.get(range.start)?)?.to_string();
        if name == "_" {
            return None;
        }
        if let Some(colon) = find_top_level_symbol(tokens, range.start, range.end, ":") {
            if colon != range.start + 1 {
                return None;
            }
            let rhs_start = colon + 1;
            if rhs_start >= range.end {
                return None;
            }
            if rhs_start + 1 == range.end && tokens[rhs_start].is_ident_text("_") {
                fields.push(MatchFieldPattern {
                    name,
                    binding: None,
                    pattern: None,
                    effect: None,
                    ignored: true,
                    span: tokens[range.start].span.clone(),
                });
                continue;
            }
            let (effect, binding_start) =
                if let Some(effect) = parse_data_effect(tokens.get(rhs_start)) {
                    (Some(effect), rhs_start + 1)
                } else {
                    (None, rhs_start)
                };
            let (binding, pattern, ignored) = if binding_start + 1 == range.end {
                if tokens[binding_start].is_ident_text("_") {
                    (None, None, true)
                } else if let Some(pattern) =
                    parse_single_literal_or_constructor_pattern(tokens, binding_start)
                {
                    (None, Some(pattern), false)
                } else {
                    let binding = ident_name(tokens.get(binding_start)?)?.to_string();
                    (Some(binding), None, false)
                }
            } else {
                (
                    None,
                    parse_match_pattern(tokens, binding_start, range.end).map(Box::new),
                    false,
                )
            };
            if pattern.is_none() && binding.is_none() && !ignored {
                return None;
            }
            fields.push(MatchFieldPattern {
                name,
                binding,
                pattern,
                effect,
                ignored,
                span: tokens[range.start].span.clone(),
            });
        } else {
            if range.start + 1 != range.end {
                return None;
            }
            fields.push(MatchFieldPattern {
                binding: Some(name.clone()),
                pattern: None,
                name,
                effect: None,
                ignored: false,
                span: tokens[range.start].span.clone(),
            });
        }
    }
    Some((fields, has_rest))
}

fn parse_single_payload_pattern(tokens: &[Token], index: usize) -> Option<Box<MatchPattern>> {
    if tokens[index].is_ident_text("_") {
        return Some(Box::new(MatchPattern::Wildcard(tokens[index].span.clone())));
    }
    if let Some(pattern) = parse_single_literal_or_constructor_pattern(tokens, index) {
        return Some(pattern);
    }
    ident_name(&tokens[index])
        .filter(|binding| *binding != "_")
        .map(|binding| {
            Box::new(MatchPattern::Binding {
                name: binding.to_string(),
                span: tokens[index].span.clone(),
            })
        })
}

fn parse_single_literal_or_constructor_pattern(
    tokens: &[Token],
    index: usize,
) -> Option<Box<MatchPattern>> {
    match &tokens[index].kind {
        TokenKind::Number(_) | TokenKind::String(_) => {
            parse_match_pattern(tokens, index, index + 1).map(Box::new)
        }
        TokenKind::Ident(value) if matches!(value.as_str(), "true" | "false") => {
            parse_match_pattern(tokens, index, index + 1).map(Box::new)
        }
        TokenKind::Keyword(value) if matches!(*value, "true" | "false") => {
            parse_match_pattern(tokens, index, index + 1).map(Box::new)
        }
        _ => {
            let name = ident_name(&tokens[index])?;
            if starts_like_constructor(name) {
                Some(Box::new(MatchPattern::Variant {
                    name: name.to_string(),
                    binding: None,
                    span: tokens[index].span.clone(),
                }))
            } else {
                None
            }
        }
    }
}

fn starts_like_constructor(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

fn parse_expr(tokens: &[Token], start: usize, end: usize) -> Option<Expr> {
    let (start, end) = trim_outer(tokens, start, end);
    if start >= end {
        return None;
    }

    if let Some(number) = parse_negative_number_expr(tokens, start, end) {
        return Some(number);
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

    if let Some(unary) = parse_unary_expr(tokens, start, end) {
        return Some(unary);
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
            let Some(close) = find_matching(tokens, value_start, "(", ")") else {
                return Some(Expr::Unknown(tokens[start].span.clone()));
            };
            if close + 1 != end {
                return Some(Expr::Unknown(tokens[start].span.clone()));
            }
            parse_expr(tokens, value_start + 1, close)
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

    if let Some(receiver_call) =
        parse_receiver_call_expr_from_receiver(tokens, start, end, DataEffect::Read)
    {
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
                let expr_tokens = lex(&span.file, &expr_text);
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
                    (&["<", "<"], BinaryOp::ShiftLeft),
                    (&[">", ">"], BinaryOp::ShiftRight),
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
            declared_effects: Vec::new(),
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
        declared_effects: Vec::new(),
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

    let mut declared_effects = Vec::new();
    if tokens
        .get(cursor)
        .is_some_and(|token| token.is_ident_text("effects"))
    {
        let effects_open = cursor + 1;
        if !tokens
            .get(effects_open)
            .is_some_and(|token| token.symbol("("))
        {
            return Some(Expr::Unknown(tokens[start].span.clone()));
        }
        let Some(effects_close) = find_matching(tokens, effects_open, "(", ")") else {
            return Some(Expr::Unknown(tokens[start].span.clone()));
        };
        let Some(effects) = parse_closure_declared_effects(tokens, effects_open + 1, effects_close)
        else {
            return Some(Expr::Unknown(tokens[start].span.clone()));
        };
        declared_effects = effects;
        cursor = effects_close + 1;
    }

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
        declared_effects,
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

fn parse_closure_declared_effects(
    tokens: &[Token],
    start: usize,
    end: usize,
) -> Option<Vec<String>> {
    let mut effects = Vec::new();
    for range in split_param_ranges(tokens, start, end) {
        if range.empty_span.is_some() {
            continue;
        }
        if range.start + 1 != range.end {
            return None;
        }
        effects.push(ident_name(&tokens[range.start])?.to_string());
    }
    Some(effects)
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
    parse_receiver_call_expr_from_receiver(tokens, start + 1, end, effect)
}

fn parse_receiver_call_expr_from_receiver(
    tokens: &[Token],
    receiver_start: usize,
    end: usize,
    effect: DataEffect,
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
    let args = tokens_to_source(tokens, start + 2, close);
    if args.trim().is_empty() {
        return None;
    }
    Some(format!("{name}<{}>", args.trim()))
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

fn parse_type_ref(tokens: &[Token], start: usize, end: usize) -> Option<TypeRef> {
    let is_fresh = tokens
        .get(start)
        .and_then(ident_name)
        .is_some_and(|name| name == "fresh");
    let is_noescape = tokens
        .get(start)
        .and_then(ident_name)
        .is_some_and(|name| name == "noescape");
    let is_owned = tokens
        .get(start)
        .and_then(ident_name)
        .is_some_and(|name| name == "owned");
    let name_index = (start..end).find(|index| {
        ident_name(&tokens[*index]).is_some_and(|name| {
            !matches!(
                name,
                "read" | "mut" | "take" | "fresh" | "handle" | "weak" | "noescape" | "owned"
            )
        })
    })?;
    let (name, name_end) = parse_type_name(tokens, name_index, end)?;
    let mut args = Vec::new();
    let mut malformed_arg_spans = Vec::new();
    let mut cursor = name_end;
    if tokens.get(cursor).is_some_and(|token| token.symbol("<")) {
        if let Some(close) = find_matching(tokens, cursor, "<", ">") {
            for range in split_param_ranges(tokens, cursor + 1, close) {
                if let Some(span) = range.empty_span {
                    malformed_arg_spans.push(span);
                    continue;
                }
                let Some(arg) = parse_type_ref(tokens, range.start, range.end) else {
                    if let Some(token) = tokens.get(range.start) {
                        malformed_arg_spans.push(token.span.clone());
                    }
                    continue;
                };
                args.push(arg);
            }
            cursor = close + 1;
        } else {
            malformed_arg_spans.push(tokens[cursor].span.clone());
        }
    }
    let mut fn_params = Vec::new();
    if name == "Fn"
        && tokens.get(cursor).is_some_and(|token| token.symbol("("))
        && let Some(close) = find_matching(tokens, cursor, "(", ")")
    {
        for range in split_param_ranges(tokens, cursor + 1, close) {
            if let Some(span) = range.empty_span {
                malformed_arg_spans.push(span);
                continue;
            }
            let Some(param) = parse_type_ref(tokens, range.start, range.end) else {
                if let Some(token) = tokens.get(range.start) {
                    malformed_arg_spans.push(token.span.clone());
                }
                continue;
            };
            fn_params.push(param);
        }
        cursor = close + 1;
    }
    let fn_return = if name == "Fn" && tokens.get(cursor).is_some_and(|token| token.symbol("->")) {
        parse_type_ref(tokens, cursor + 1, end).map(Box::new)
    } else {
        None
    };
    Some(TypeRef {
        name,
        args,
        malformed_arg_spans,
        is_fresh,
        is_noescape,
        is_owned,
        fn_params,
        fn_return,
        span: tokens[name_index].span.clone(),
    })
}

fn parse_type_name(tokens: &[Token], start: usize, end: usize) -> Option<(String, usize)> {
    let mut index = start;
    let mut name = ident_name(tokens.get(index)?)?.to_string();
    index += 1;
    while index + 1 < end
        && tokens.get(index).is_some_and(|token| token.symbol("."))
        && tokens.get(index + 1).and_then(ident_name).is_some()
    {
        name.push('.');
        name.push_str(ident_name(&tokens[index + 1])?);
        index += 2;
    }
    Some((name, index))
}

fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
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
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
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
    let mut postfix_continuation_line = None;
    for (index, token) in tokens.iter().enumerate().take(limit).skip(start + 1) {
        if depth == 0 && angle_depth == 0 && token.span.line > line {
            if postfix_continuation_line == Some(token.span.line) {
                // Keep scanning the receiver-call segment that began with a
                // postfix token on this line, e.g. `}).ok()` or a next-line
                // `.map(...)` chain.
            } else if is_statement_postfix_token(token) {
                postfix_continuation_line = Some(token.span.line);
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

fn find_top_level_ident(tokens: &[Token], start: usize, end: usize, ident: &str) -> Option<usize> {
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
        .map(|open| find_matching(tokens, open, "{", "}").map_or(tokens.len(), |close| close + 1));
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

fn ident_name(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Ident(value) => Some(value),
        TokenKind::Keyword(value) => Some(value),
        _ => None,
    }
}

fn tokens_to_source(tokens: &[Token], start: usize, end: usize) -> String {
    tokens
        .iter()
        .take(end)
        .skip(start)
        .map(Token::text)
        .collect::<Vec<_>>()
        .join("")
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
}
