use std::collections::{HashMap, HashSet};

use crate::checks;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::{
    CallResolution, DuplicateSymbolKind, FunctionSig, Hir, HirBlock, HirExpr, HirStmt, HirTypeKind,
    ParamSig, ResolvedCalleeKind,
};
use crate::interfaces::CORE_INTERFACES;
use crate::lexer::{Token, lex};
use crate::syntax::ast::merge_programs;
use crate::syntax::ast::{
    Block, Callee, DataEffect, EffectDecl, Expr, FunctionDecl, GenericBound, GenericParam, Item,
    MatchPattern, Param, Stmt, TypeKind, TypeRef,
};
use crate::syntax::parse_source;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceGenericContext {
    Ordinary,
    Return,
}

fn resource_result_return_arg_allowed(
    ty: &TypeRef,
    index: usize,
    context: ResourceGenericContext,
) -> bool {
    context == ResourceGenericContext::Return && ty.name == "Result" && index == 0
}

pub fn analyze_source(file: &str, source: &str) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let syntax_program = parse_source(file, source);
    let hir = Hir::from_syntax(&syntax_program);
    analyze_program(tokens, syntax_program, hir)
}

pub fn core_interfaces() -> &'static [(&'static str, &'static str)] {
    CORE_INTERFACES
}

pub fn analyze_source_with_core(file: &str, source: &str) -> Vec<Diagnostic> {
    analyze_source_with_interfaces(file, source, CORE_INTERFACES)
}

pub fn analyze_source_with_interfaces(
    file: &str,
    source: &str,
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let syntax_program = parse_source(file, source);
    let interface_programs = interfaces
        .iter()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&syntax_program, &interface_programs);
    analyze_program(tokens, syntax_program, hir)
}

pub fn analyze_sources_with_interfaces(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    let tokens = sources
        .iter()
        .flat_map(|(file, source)| lex(file, source))
        .collect::<Vec<_>>();
    let syntax_program = merge_programs(
        sources
            .iter()
            .map(|(file, source)| parse_source(file, source)),
    );
    let interface_programs = interfaces
        .iter()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&syntax_program, &interface_programs);
    analyze_program(tokens, syntax_program, hir)
}

fn analyze_program(
    tokens: Vec<Token>,
    syntax_program: crate::syntax::ast::Program,
    hir: Hir,
) -> Vec<Diagnostic> {
    let mut analyzer = Analyzer {
        tokens: &tokens,
        syntax_program,
        hir,
        diagnostics: Vec::new(),
    };
    analyzer.run();
    analyzer.diagnostics
}

pub(crate) struct Analyzer<'a> {
    pub(crate) tokens: &'a [Token],
    pub(crate) syntax_program: crate::syntax::ast::Program,
    pub(crate) hir: Hir,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeGuarantee {
    Noalloc,
    Pure,
    NoBlock,
    NoPanic,
}

impl RuntimeGuarantee {
    const ALL: [Self; 4] = [Self::Noalloc, Self::Pure, Self::NoBlock, Self::NoPanic];

    fn effect_name(self) -> &'static str {
        match self {
            Self::Noalloc => "noalloc",
            Self::Pure => "pure",
            Self::NoBlock => "no_block",
            Self::NoPanic => "no_panic",
        }
    }
}

impl Analyzer<'_> {
    fn run(&mut self) {
        self.check_single_feature_declaration();
        self.check_unknown_file_features();
        self.check_duplicate_file_features();
        self.check_removed_profile_declarations();
        self.check_unsupported_syntax();
        self.check_match_exhaustiveness();
        self.check_duplicate_declarations();
        self.check_protocol_contracts();
        self.check_signature_explicitness();
        self.check_unknown_types();
        self.check_unknown_fields();
        self.check_unknown_bindings();
        self.check_fd_surface();
        self.check_generic_constraints();
        self.check_runtime_guarantee_bodies();
        self.check_try_operator_result_returns();
        self.check_resource_fields();
        self.check_weak_fields();
        self.check_resource_pool_type_arguments();
        self.check_resource_generic_arguments();
        checks::features::check(self);
        checks::calls::check(self);
        checks::body::check(self);
        checks::forbidden::check(self);
    }

    fn check_single_feature_declaration(&mut self) {
        for span in self.syntax_program.feature_spans.iter().skip(1) {
            self.diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_FEATURE_DECLARATION,
                    "RSScript files may declare at most one explicit feature header.",
                    span.clone(),
                    "duplicate features",
                )
                .with_cause("Only one top-level `features:` declaration is allowed.")
                .with_fix(
                    "remove_duplicate_features",
                    "Merge the feature list into one `features:` declaration.",
                    "manual",
                ),
            );
        }
    }

    fn check_unknown_file_features(&mut self) {
        for feature in &self.syntax_program.unknown_features {
            self.diagnostics.push(
                Diagnostic::error(
                    code::UNKNOWN_FILE_FEATURE,
                    format!("Unknown file feature `{}`.", feature.name),
                    feature.span.clone(),
                    "unknown feature",
                )
                .with_cause(
                    "File features must be review-relevant capabilities recognized by this compiler.",
                )
                .with_fix(
                    "remove_or_correct_feature",
                    "Remove the feature name or replace it with a supported feature such as `local`.",
                    "manual",
                ),
            );
        }
    }

    fn check_duplicate_file_features(&mut self) {
        for feature in &self.syntax_program.duplicate_features {
            self.diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_FILE_FEATURE,
                    format!("Duplicate file feature `{}`.", feature.name),
                    feature.span.clone(),
                    "duplicate feature",
                )
                .with_cause(
                    "File features are capability declarations; repeating one makes the review boundary noisier without changing semantics.",
                )
                .with_fix(
                    "remove_duplicate_feature",
                    format!("Remove the repeated `{}` feature.", feature.name),
                    "machine-applicable",
                ),
            );
        }
    }

    fn check_removed_profile_declarations(&mut self) {
        for span in &self.syntax_program.profile_spans {
            self.diagnostics.push(
                Diagnostic::error(
                    code::REMOVED_PROFILE_DECLARATION,
                    "`profile:` declarations are not part of RSScript v0.5.",
                    span.clone(),
                    "removed profile declaration",
                )
                .with_cause("v0.5 uses `features:` for file-level advanced capabilities; omitted features means managed-only.")
                .with_fix(
                    "remove_profile",
                    "Remove `profile:` and add `features: local` only if the file uses local ownership features.",
                    "manual",
                ),
            );
        }
    }

    fn check_unsupported_syntax(&mut self) {
        for span in self.syntax_program.unknown_top_level_spans.clone() {
            self.unsupported_syntax(
                span,
                "unsupported top-level item",
                "This top-level construct is outside the current RSScript parser surface.",
            );
        }
        for span in self.syntax_program.malformed_declaration_spans.clone() {
            self.unsupported_syntax(
                span,
                "malformed declaration",
                "This declaration starts like RSScript syntax but does not match the supported declaration grammar.",
            );
        }
        let items = self.syntax_program.items.clone();
        for item in &items {
            self.check_unsupported_syntax_item(item);
        }
    }

    fn check_unsupported_syntax_item(&mut self, item: &Item) {
        match item {
            Item::Function(function) => {
                for span in &function.malformed_generic_param_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed generic parameter declaration",
                        "Generic parameters must use `T`, `T: Managed`, `T: Struct`, `T: Resource`, or a single protocol bound such as `T: Writer`.",
                    );
                }
                if function.is_async && !function.body.statements.is_empty() {
                    self.unsupported_syntax(
                        function.span.clone(),
                        "unsupported async function body",
                        "`async fn` is currently supported only in interface and review metadata; executable async lowering is not part of the v0.5 runtime yet.",
                    );
                }
                if function.is_native && !function.body.statements.is_empty() {
                    self.unsupported_syntax(
                        function.span.clone(),
                        "unsupported native function body",
                        "`native fn` declares an external/native boundary in v0.5. Provide a bodyless declaration and bind the implementation through the native wrapper path.",
                    );
                }
                for span in &function.malformed_effect_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed effect declaration",
                        "Effects must use a bare effect name or `retains(parameter)`.",
                    );
                }
                for span in &function.malformed_param_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed parameter declaration",
                        "Function parameters must use `name: Type`, `name: read Type`, `name: mut Type`, or `name: take Type`.",
                    );
                }
                for param in &function.params {
                    self.check_unsupported_syntax_type_ref(&param.ty, true);
                }
                if let Some(return_ty) = &function.return_ty {
                    self.check_unsupported_syntax_type_ref(return_ty, false);
                }
                self.check_unsupported_syntax_block(&function.body);
            }
            Item::Type(type_decl) => {
                for span in &type_decl.malformed_generic_param_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed generic parameter declaration",
                        "Generic parameters must use `T`, `T: Managed`, `T: Struct`, `T: Resource`, or a single protocol bound such as `T: Writer`.",
                    );
                }
                for span in &type_decl.malformed_field_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed field declaration",
                        "Type fields must use `name: Type`, `name: handle Type`, or `name: weak Type`.",
                    );
                }
                if type_decl.is_opaque && !type_decl.fields.is_empty() {
                    self.unsupported_syntax(
                        type_decl.span.clone(),
                        "unsupported opaque type body",
                        "Opaque interface types hide their representation. Declare `opaque struct Name`, `opaque class Name`, or `opaque resource Name` without fields.",
                    );
                }
                if type_decl.is_opaque && type_decl.drop_body.is_some() {
                    self.unsupported_syntax(
                        type_decl.span.clone(),
                        "unsupported opaque type body",
                        "Opaque resource contracts hide their implementation details, including drop bodies. Resource cleanup belongs to the implementation, not the `.rssi` contract.",
                    );
                }
                for field in &type_decl.fields {
                    self.check_unsupported_syntax_type_ref(&field.ty, false);
                }
                if type_decl.kind != TypeKind::Resource
                    && let Some(drop_body) = &type_decl.drop_body
                {
                    self.unsupported_syntax(
                        drop_body.span.clone(),
                        "unsupported managed drop",
                        "Managed class and struct values do not have user-observable destructors in v0.5. Use `resource` with `with` or `ResourcePool` for deterministic cleanup.",
                    );
                }
            }
        }
    }

    fn check_unsupported_syntax_type_ref(&mut self, ty: &TypeRef, allow_noescape_param: bool) {
        if ty.is_noescape && (!allow_noescape_param || ty.name != "Fn") {
            self.unsupported_syntax(
                ty.span.clone(),
                "unsupported noescape position",
                "`noescape Fn(...)` is only supported as a direct function parameter type.",
            );
        }
        for span in &ty.malformed_arg_spans {
            self.unsupported_syntax(
                span.clone(),
                "malformed type argument",
                "Type arguments must be valid type references; empty or unsupported type argument slots are not allowed.",
            );
        }
        for arg in &ty.args {
            self.check_unsupported_syntax_type_ref(arg, false);
        }
    }

    fn check_unsupported_syntax_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.check_unsupported_syntax_stmt(statement);
        }
    }

    fn check_unsupported_syntax_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.malformed {
                    self.unsupported_syntax(
                        stmt.span.clone(),
                        "malformed statement",
                        "`let` and `local` bindings need a binding name, and an `=` must be followed by an expression.",
                    );
                }
                if let Some(value) = &stmt.value {
                    self.check_unsupported_syntax_expr(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_unsupported_syntax_expr(value);
                }
            }
            Stmt::With(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.resource);
                self.check_unsupported_syntax_block(&stmt.body);
            }
            Stmt::MalformedWith(span) => self.unsupported_syntax(
                span.clone(),
                "malformed with statement",
                "`with` statements must use `with resource as name { ... }`.",
            ),
            Stmt::If(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.condition);
                self.check_unsupported_syntax_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_unsupported_syntax_block(else_body);
                }
            }
            Stmt::MalformedIf(span) => self.unsupported_syntax(
                span.clone(),
                "malformed if statement",
                "`if` statements must use `if condition { ... }` with optional `else { ... }` or `else if ...`.",
            ),
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_unsupported_syntax_expr(condition);
                }
                self.check_unsupported_syntax_block(&stmt.body);
            }
            Stmt::MalformedLoop(span) => self.unsupported_syntax(
                span.clone(),
                "malformed loop statement",
                "`loop` statements must use `loop { ... }`; `while` statements must use `while condition { ... }`.",
            ),
            Stmt::For(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.iterable);
                self.check_unsupported_syntax_block(&stmt.body);
            }
            Stmt::MalformedFor(span) => self.unsupported_syntax(
                span.clone(),
                "malformed for statement",
                "`for` statements must use `for name in iterable { ... }`.",
            ),
            Stmt::Match(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.value);
                for span in &stmt.malformed_arm_spans {
                    self.unsupported_syntax(
                        span.clone(),
                        "malformed match arm",
                        "Match arms must use `pattern => statement` or `pattern => { ... }`.",
                    );
                }
                for arm in &stmt.arms {
                    self.check_unsupported_syntax_block(&arm.body);
                }
            }
            Stmt::MalformedMatch(span) => self.unsupported_syntax(
                span.clone(),
                "malformed match statement",
                "`match` statements must use `match value { pattern => ... }`.",
            ),
            Stmt::Expr(expr) => self.check_unsupported_syntax_expr(expr),
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Unknown(span) => self.unsupported_syntax(
                span.clone(),
                "unsupported statement",
                "This statement is outside the current RSScript parser surface.",
            ),
        }
    }

    fn check_unsupported_syntax_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Binary { left, right, .. } => {
                self.check_unsupported_syntax_expr(left);
                self.check_unsupported_syntax_expr(right);
            }
            Expr::Field { base, .. } => self.check_unsupported_syntax_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_unsupported_syntax_expr(base);
                self.check_unsupported_syntax_expr(index);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    if arg.malformed {
                        self.unsupported_syntax(
                            arg.span.clone(),
                            "malformed call argument",
                            "Call arguments cannot contain empty argument slots.",
                        );
                    } else {
                        self.check_unsupported_syntax_expr(&arg.value);
                    }
                }
            }
            Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
                self.check_unsupported_syntax_expr(value);
            }
            Expr::Spawn { value, span } => {
                self.unsupported_syntax(
                    span.clone(),
                    "unsupported spawn expression",
                    "`spawn` is currently review metadata only; executable async task lowering is not part of the v0.5 runtime yet.",
                );
                self.check_unsupported_syntax_expr(value);
            }
            Expr::Await { value, span } => {
                self.unsupported_syntax(
                    span.clone(),
                    "unsupported await expression",
                    "`await` is future executable async syntax; executable async lowering is not part of the v0.5 runtime yet.",
                );
                self.check_unsupported_syntax_expr(value);
            }
            Expr::Closure { body, .. } => self.check_unsupported_syntax_block(body),
            Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) => {}
            Expr::Unknown(span) => self.unsupported_syntax(
                span.clone(),
                "unsupported expression",
                "This expression is outside the current RSScript parser surface.",
            ),
        }
    }

    fn unsupported_syntax(&mut self, span: crate::diagnostic::Span, label: &str, cause: &str) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNSUPPORTED_SYNTAX,
                "unsupported RSScript syntax.",
                span,
                label,
            )
            .with_cause(cause)
            .with_fix(
                "rewrite_supported_syntax",
                "Rewrite this construct using the currently supported RSScript syntax.",
                "manual",
            ),
        );
    }

    fn check_match_exhaustiveness(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            self.check_match_exhaustiveness_block(&function.body);
        }
    }

    fn check_match_exhaustiveness_block(&mut self, block: &Block) {
        for statement in &block.statements {
            self.check_match_exhaustiveness_stmt(statement);
        }
    }

    fn check_match_exhaustiveness_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_match_exhaustiveness_expr(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_match_exhaustiveness_expr(value);
                }
            }
            Stmt::With(stmt) => {
                self.check_match_exhaustiveness_expr(&stmt.resource);
                self.check_match_exhaustiveness_block(&stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_match_exhaustiveness_expr(&stmt.condition);
                self.check_match_exhaustiveness_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_match_exhaustiveness_block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_match_exhaustiveness_expr(condition);
                }
                self.check_match_exhaustiveness_block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_match_exhaustiveness_expr(&stmt.iterable);
                self.check_match_exhaustiveness_block(&stmt.body);
            }
            Stmt::Match(stmt) => {
                self.check_match_exhaustiveness_expr(&stmt.value);
                for arm in &stmt.arms {
                    self.check_match_exhaustiveness_block(&arm.body);
                }
                if !match_is_exhaustive(&stmt.arms) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::NON_EXHAUSTIVE_MATCH,
                            "match statement is not exhaustive.",
                            stmt.span.clone(),
                            "non-exhaustive match",
                        )
                        .with_cause(
                            "Supported match statements must cover `Some`/`None`, `Ok`/`Err`, or include `_`.",
                        )
                        .with_fix(
                            "add_missing_arm",
                            "Add the missing variant arm or a final `_` fallback.",
                            "manual",
                        ),
                    );
                }
            }
            Stmt::Expr(expr) => self.check_match_exhaustiveness_expr(expr),
            Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => {}
        }
    }

    fn check_match_exhaustiveness_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Binary { left, right, .. } => {
                self.check_match_exhaustiveness_expr(left);
                self.check_match_exhaustiveness_expr(right);
            }
            Expr::Field { base, .. } => self.check_match_exhaustiveness_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_match_exhaustiveness_expr(base);
                self.check_match_exhaustiveness_expr(index);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    self.check_match_exhaustiveness_expr(&arg.value);
                }
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_match_exhaustiveness_expr(value);
            }
            Expr::Closure { body, .. } => self.check_match_exhaustiveness_block(body),
            Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
        }
    }

    fn check_signature_explicitness(&mut self) {
        let items = self.syntax_program.items.clone();
        let protocol_names = self
            .syntax_program
            .protocols
            .iter()
            .map(|protocol| protocol.name.clone())
            .collect::<HashSet<_>>();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            if function.return_ty.is_none() {
                self.diagnostics.push(
                    Diagnostic::error(
                        code::MISSING_RETURN_TYPE,
                        format!("function `{}` must declare an explicit return type.", function.name),
                        function.span.clone(),
                        "missing return type",
                    )
                    .with_cause("Public APIs must not rely on inference; this checker applies the canonical rule to all functions.")
                    .with_fix("add_return_type", "Add `-> Unit` or another explicit return type.", "manual"),
                );
            }

            let is_qualified_method = function.name.contains('.');
            let is_protocol_method = function
                .name
                .split_once('.')
                .is_some_and(|(namespace, _)| protocol_names.contains(namespace));
            for (index, param) in function.params.iter().enumerate() {
                if param.name == "self" && (!is_qualified_method || index != 0) {
                    self.invalid_self_parameter_diagnostic(
                        function,
                        param,
                        "`self` may only be the first parameter of a qualified method signature.",
                    );
                }
                if param.ty.name.is_empty() {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::MISSING_PARAMETER_TYPE,
                            format!(
                                "parameter `{}` in `{}` must declare an explicit type.",
                                param.name, function.name
                            ),
                            param.span.clone(),
                            "missing parameter type",
                        )
                        .with_fix(
                            "add_parameter_type",
                            "Add an explicit parameter type.",
                            "manual",
                        ),
                    );
                }
                if param.effect.is_none() && param.ty.name == "share" {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::REMOVED_SHARE_EFFECT,
                            format!(
                                "parameter `{}` in `{}` uses removed `share` data effect.",
                                param.name, function.name
                            ),
                            param.ty.span.clone(),
                            "removed share data effect",
                        )
                        .with_cause("RSScript v0.5 has exactly three data effects: `read`, `mut`, and `take`.")
                        .with_fix(
                            "replace_share_effect",
                            format!(
                                "Use `{}: read T` and add `effects(retains({}))` if the function retains it.",
                                param.name, param.name
                            ),
                            "manual",
                        ),
                    );
                }
                if param.effect.is_none()
                    && !param.ty.name.is_empty()
                    && param.ty.name != "share"
                    && !type_ref_is_noescape(&param.ty)
                    && !type_ref_is_closure_effect_exempt(&param.ty)
                    && !type_ref_has_surface_reference(&param.ty, self.tokens)
                    && !type_ref_contains_name(&param.ty, "Fd")
                    && !is_copy_type(&param.ty)
                {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::MISSING_PARAMETER_EFFECT,
                            format!(
                                "parameter `{}` in `{}` must declare `read`, `mut`, or `take`.",
                                param.name, function.name
                            ),
                            param.span.clone(),
                            "missing parameter effect",
                        )
                        .with_cause("Non-Copy parameters must expose their data effect in the function signature.")
                        .with_fix(
                            "add_parameter_effect",
                            format!("Write `{}: read {}` or another explicit effect.", param.name, type_ref_name(&param.ty)),
                            "manual",
                        ),
                    );
                }
            }

            if is_protocol_method {
                match function.params.first() {
                    Some(param)
                        if param.name == "self"
                            && param.ty.name == "Self"
                            && param.effect.is_some() => {}
                    Some(param) => self.invalid_self_parameter_diagnostic(
                        function,
                        param,
                        "Protocol methods must declare `self: read|mut|take Self` as their first parameter.",
                    ),
                    None => self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_SELF_PARAMETER,
                            format!(
                                "protocol method `{}` must declare an explicit `self` parameter.",
                                function.name
                            ),
                            function.span.clone(),
                            "missing protocol self parameter",
                        )
                        .with_cause("Protocol calls are explicit capability calls, so the receiver must be review-visible as `self: read|mut|take Self`.")
                        .with_fix(
                            "add_protocol_self",
                            "Add `self: read Self`, `self: mut Self`, or `self: take Self` as the first parameter.",
                            "manual",
                        ),
                    ),
                }
            }

            for effect in &function.effects {
                let effect_name = effect_name(effect);
                if effect_name == "fresh" {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::UNKNOWN_EFFECT,
                            format!(
                                "`fresh` is not a valid `effects(...)` item in `{}`.",
                                function.name
                            ),
                            function.span.clone(),
                            "fresh is a return marker",
                        )
                        .with_cause(
                            "`fresh` is a return contract, not a side effect or runtime guarantee.",
                        )
                        .with_fix(
                            "move_fresh_to_return_type",
                            "Write `-> fresh T` or `-> Result<fresh T, E>` instead.",
                            "manual",
                        ),
                    );
                    continue;
                }
                if let Some(replacement) = removed_runtime_effect_replacement(effect_name) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::REMOVED_RUNTIME_EFFECT,
                            format!(
                                "`{effect_name}` is not a valid RSScript v0.5 effect in `{}`.",
                                function.name
                            ),
                            function.span.clone(),
                            "removed runtime effect",
                        )
                        .with_cause("v0.5 uses reductive guarantees such as `no_panic`, `noalloc`, `no_block`, and `pure`.")
                        .with_fix(
                            "replace_removed_effect",
                            replacement,
                            "manual",
                        ),
                    );
                    continue;
                }
                let valid = effect_name == "no_panic"
                    || effect_name == "noalloc"
                    || effect_name == "no_block"
                    || effect_name == "pure"
                    || effect_name == "unsafe"
                    || effect_name == "native"
                    || effect_name == "parallel"
                    || matches!(effect, EffectDecl::Retains(_));
                if !valid {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::UNKNOWN_EFFECT,
                            format!("unknown effect `{effect_name}` in `{}`.", function.name),
                            function.span.clone(),
                            "unknown effect",
                        )
                        .with_cause("Effects are review-visible semantic contracts; unknown effect names cannot be interpreted safely.")
                        .with_fix(
                            "fix_unknown_effect",
                            "Use a recognized effect such as `pure`, `no_panic`, `noalloc`, `no_block`, `native`, `unsafe`, `parallel`, or `retains(param)`.",
                            "manual",
                        ),
                    );
                }
            }

            if function.is_native && !function_has_effect(function, "native") {
                self.diagnostics.push(
                    Diagnostic::error(
                        code::FEATURE_VIOLATION,
                        format!(
                            "`native fn {}` must declare `effects(native)`.",
                            function.name
                        ),
                        function.span.clone(),
                        "native boundary missing effect",
                    )
                    .with_cause(
                        "`native fn` is an implementation boundary, and v0.5 requires that boundary to appear in the effect list for review maps and semantic diffs.",
                    )
                    .with_fix(
                        "add_native_effect",
                        "Add `effects(native)` to the native function declaration.",
                        "manual",
                    ),
                );
            }

            let param_names: HashSet<&str> = function
                .params
                .iter()
                .map(|param| param.name.as_str())
                .collect();
            for effect in &function.effects {
                let EffectDecl::Retains(param) = effect else {
                    continue;
                };
                if !param_names.contains(param.as_str()) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::UNKNOWN_RETAINED_PARAMETER,
                            format!(
                                "`{}` declares `effects(retains({param}))`, but `{param}` is not a parameter.",
                                function.name
                            ),
                            function.span.clone(),
                            "unknown retained parameter",
                        )
                        .with_cause("Retention effects must name a parameter from the same function signature.")
                        .with_fix(
                            "fix_retains_parameter",
                            "Rename the retained parameter or remove this retention effect.",
                            "manual",
                        ),
                    );
                    continue;
                }
                if let Some(function_param) = function
                    .params
                    .iter()
                    .find(|function_param| function_param.name == *param)
                    && type_ref_is_copy(&function_param.ty)
                {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::UNKNOWN_RETAINED_PARAMETER,
                            format!(
                                "`{}` declares `effects(retains({param}))`, but `{param}` is Copy.",
                                function.name
                            ),
                            function_param.span.clone(),
                            "Copy parameter cannot be retained",
                        )
                        .with_cause(
                            "`retains(x)` marks a managed retention boundary. Copy values have no managed handle to retain.",
                        )
                        .with_fix(
                            "remove_copy_retains",
                            format!("Remove `effects(retains({param}))`."),
                            "manual",
                        ),
                    );
                    continue;
                }
                if function.params.iter().any(|function_param| {
                    function_param.name == *param && type_ref_is_noescape(&function_param.ty)
                }) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::NOESCAPE_CALLBACK_ESCAPE,
                            format!(
                                "`{}` cannot retain noescape callback parameter `{param}`.",
                                function.name
                            ),
                            function.span.clone(),
                            "noescape callback escapes",
                        )
                        .with_cause(
                            "`noescape Fn()` parameters may be called or forwarded to another noescape parameter, but they cannot be retained after return.",
                        )
                        .with_fix(
                            "remove_noescape_retention",
                            format!("Remove `effects(retains({param}))`, or use an ordinary managed callback type."),
                            "manual",
                        ),
                    );
                }
            }

            if function
                .effects
                .iter()
                .any(|effect| matches!(effect, EffectDecl::Name(name) if name == "pure"))
            {
                if let Some(return_ty) = &function.return_ty
                    && let Some(resource_name) = self
                        .resource_return_type_name(return_ty)
                        .map(str::to_string)
                {
                    self.pure_resource_return_diagnostic(
                        &function.name,
                        return_ty.span.clone(),
                        &resource_name,
                    );
                }

                for param in function.params.iter().filter(|param| {
                    matches!(param.effect, Some(DataEffect::Mut | DataEffect::Take))
                }) {
                    let (label, cause, fix) = match param.effect {
                        Some(DataEffect::Take) => (
                            "taking parameter in pure function",
                            "A `pure` function must not consume values or change ownership boundaries.",
                            "Remove `pure`, or change the parameter to `read` if the function only observes it.",
                        ),
                        _ => (
                            "mutable parameter in pure function",
                            "A `pure` function must not mutate reachable managed state.",
                            "Remove `pure`, or change the parameter to `read` if the function does not mutate it.",
                        ),
                    };
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_PURE_EFFECT,
                            format!(
                                "`{}` is declared pure but parameter `{}` uses `{}`.",
                                function.name,
                                param.name,
                                param.effect.map_or("unknown", data_effect_name)
                            ),
                            param.span.clone(),
                            label,
                        )
                        .with_cause(cause)
                        .with_fix("remove_pure_or_use_read", fix, "manual"),
                    );
                }

                for effect in function
                    .effects
                    .iter()
                    .filter(|effect| matches!(effect, EffectDecl::Retains(_)))
                {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_PURE_EFFECT,
                            format!(
                                "`{}` is declared pure but also retains a parameter.",
                                function.name
                            ),
                            function.span.clone(),
                            "retention in pure function",
                        )
                        .with_cause("A `pure` function must not retain parameters after returning.")
                        .with_fix(
                            "remove_pure_or_retains",
                            format!("Remove `pure` or remove `{}`.", effect_display(effect)),
                            "manual",
                        ),
                    );
                }
            }
        }
    }

    fn invalid_self_parameter_diagnostic(
        &mut self,
        function: &FunctionDecl,
        param: &Param,
        cause: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_SELF_PARAMETER,
                format!("invalid `self` parameter in `{}`.", function.name),
                param.span.clone(),
                "invalid self parameter",
            )
            .with_cause(cause)
            .with_fix(
                "fix_self_parameter",
                "Use a different parameter name, or make this the first parameter of an explicit method/protocol signature.",
                "manual",
            ),
        );
    }

    fn check_generic_constraints(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    let bounds = generic_bounds(&decl.type_params);
                    if decl.kind == TypeKind::Resource {
                        for param in &decl.type_params {
                            if param.bound.is_none() {
                                self.generic_resource_argument_diagnostic(
                                    &param.name,
                                    &param.name,
                                    &param.span,
                                    "resource type parameters must declare an explicit bound.",
                                );
                            }
                        }
                        for field in &decl.fields {
                            self.check_resource_type_param_field(&field.ty, &bounds, false);
                        }
                    }
                    for field in &decl.fields {
                        self.check_generic_resource_pool_type_ref(&field.ty, &bounds);
                    }
                }
                Item::Function(function) => {
                    let bounds = generic_bounds(&function.type_params);
                    for param in &function.params {
                        self.check_generic_resource_pool_type_ref(&param.ty, &bounds);
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_generic_resource_pool_type_ref(return_ty, &bounds);
                        if function.returns_fresh {
                            self.check_fresh_generic_return_bound(
                                &function.name,
                                return_ty,
                                &bounds,
                            );
                        }
                    }
                }
            }
        }
    }

    fn check_protocol_contracts(&mut self) {
        let protocol_names = self
            .syntax_program
            .protocols
            .iter()
            .map(|protocol| protocol.name.clone())
            .collect::<HashSet<_>>();
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    for param in &decl.type_params {
                        self.check_protocol_bound(param, &protocol_names);
                    }
                }
                Item::Function(function) => {
                    if function_body_belongs_to_protocol(function, &protocol_names) {
                        self.unsupported_syntax(
                            function.span.clone(),
                            "unsupported protocol method body",
                            "Protocols are effect-carrying capability contracts in v0.5. Protocol methods are bodyless signatures; default method bodies are not part of the RSScript protocol model.",
                        );
                    }
                    for param in &function.type_params {
                        self.check_protocol_bound(param, &protocol_names);
                    }
                }
            }
        }

        let protocol_impls = self.syntax_program.protocol_impls.clone();
        for protocol_impl in &protocol_impls {
            if !protocol_names.contains(&protocol_impl.protocol) {
                self.unknown_protocol_diagnostic(&protocol_impl.protocol, &protocol_impl.span);
                continue;
            }
            if self.hir.type_info(&protocol_impl.type_name).is_none()
                && !is_builtin_type_name(&protocol_impl.type_name)
            {
                self.unknown_type_name_diagnostic(&protocol_impl.type_name, &protocol_impl.span);
            }
            let protocol_methods = protocol_method_names(&items, &protocol_impl.protocol);
            let mapped_methods = protocol_impl
                .mappings
                .iter()
                .map(|mapping| mapping.method.clone())
                .collect::<HashSet<_>>();
            for method in &protocol_methods {
                if !mapped_methods.contains(method) {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        method,
                        &protocol_impl.span,
                        "missing protocol method mapping",
                        format!(
                            "`{}` must map protocol method `{method}` to a concrete function.",
                            protocol_impl.type_name
                        ),
                    );
                }
            }

            for mapping in &protocol_impl.mappings {
                let Some(protocol_signature) = self
                    .hir
                    .resolve_function(Some(&protocol_impl.protocol), &mapping.method)
                else {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        &mapping.method,
                        &mapping.span,
                        "unknown protocol method",
                        format!(
                            "`{}` does not declare method `{}`.",
                            protocol_impl.protocol, mapping.method
                        ),
                    );
                    continue;
                };
                let (target_namespace, target_name) = split_qualified_name(&mapping.target);
                let Some(target_signature) = self
                    .hir
                    .resolve_function(target_namespace.as_deref(), target_name)
                else {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        &mapping.method,
                        &mapping.span,
                        "unknown protocol implementation target",
                        format!(
                            "Mapped target `{}` must resolve to a concrete function.",
                            mapping.target
                        ),
                    );
                    continue;
                };
                if let Some(reason) = protocol_signature_mismatch(
                    protocol_signature,
                    target_signature,
                    &protocol_impl.type_name,
                ) {
                    self.protocol_impl_mismatch_diagnostic(
                        &protocol_impl.protocol,
                        &protocol_impl.type_name,
                        &mapping.method,
                        &mapping.span,
                        "protocol implementation signature mismatch",
                        reason,
                    );
                }
            }
        }
    }

    fn check_protocol_bound(&mut self, param: &GenericParam, protocol_names: &HashSet<String>) {
        let Some(GenericBound::Protocol(protocol)) = &param.bound else {
            return;
        };
        if !protocol_names.contains(protocol) {
            self.unknown_protocol_diagnostic(protocol, &param.span);
        }
    }

    fn check_unknown_types(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    let generic_params = decl
                        .type_params
                        .iter()
                        .map(|param| param.name.as_str())
                        .collect::<HashSet<_>>();
                    for field in &decl.fields {
                        self.check_unknown_type_ref(&field.ty, &generic_params);
                    }
                }
                Item::Function(function) => {
                    let generic_params = function
                        .type_params
                        .iter()
                        .map(|param| param.name.as_str())
                        .collect::<HashSet<_>>();
                    for param in &function.params {
                        self.check_unknown_type_ref(&param.ty, &generic_params);
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_unknown_type_ref(return_ty, &generic_params);
                    }
                }
            }
        }
    }

    fn check_unknown_type_ref(&mut self, ty: &TypeRef, generic_params: &HashSet<&str>) {
        if !known_type_ref(ty, generic_params, &self.hir) {
            self.unknown_type_diagnostic(ty);
        }
        for arg in &ty.args {
            self.check_unknown_type_ref(arg, generic_params);
        }
        for param in &ty.fn_params {
            self.check_unknown_type_ref(param, generic_params);
        }
        if let Some(return_ty) = &ty.fn_return {
            self.check_unknown_type_ref(return_ty, generic_params);
        }
    }

    fn check_unknown_fields(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            let Some(body) = self.hir.function_body(&function.name).cloned() else {
                continue;
            };
            for access in &body.field_accesses {
                let Some(base_type) = &access.base_type else {
                    continue;
                };
                let Some(type_info) = self.hir.type_info(base_type) else {
                    continue;
                };
                if !type_info.fields.contains_key(&access.name) {
                    self.unknown_field_diagnostic(&access.name, base_type, &access.span);
                }
            }
        }
    }

    fn check_unknown_bindings(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            let Some(block) = self
                .hir
                .function_body(&function.name)
                .and_then(|body| body.block.clone())
            else {
                continue;
            };
            let mut visible = function
                .params
                .iter()
                .map(|param| param.name.clone())
                .collect::<HashSet<_>>();
            self.check_unknown_bindings_in_block(&block, &mut visible);
        }
    }

    fn check_unknown_bindings_in_block(&mut self, block: &HirBlock, visible: &mut HashSet<String>) {
        for statement in &block.statements {
            self.check_unknown_bindings_in_stmt(statement, visible);
        }
    }

    fn check_unknown_bindings_in_stmt(
        &mut self,
        statement: &HirStmt,
        visible: &mut HashSet<String>,
    ) {
        match statement {
            HirStmt::Let { name, value, .. } => {
                if let Some(value) = value {
                    self.check_unknown_bindings_in_expr(value, visible);
                }
                visible.insert(name.clone());
            }
            HirStmt::Return { value, .. } => {
                if let Some(value) = value {
                    self.check_unknown_bindings_in_expr(value, visible);
                }
            }
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                self.check_unknown_bindings_in_expr(resource, visible);
                let mut body_visible = visible.clone();
                body_visible.insert(binding.clone());
                self.check_unknown_bindings_in_block(body, &mut body_visible);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                self.check_unknown_bindings_in_expr(condition, visible);
                let mut then_visible = visible.clone();
                self.check_unknown_bindings_in_block(then_body, &mut then_visible);
                if let Some(else_body) = else_body {
                    let mut else_visible = visible.clone();
                    self.check_unknown_bindings_in_block(else_body, &mut else_visible);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    self.check_unknown_bindings_in_expr(condition, visible);
                }
                let mut body_visible = visible.clone();
                self.check_unknown_bindings_in_block(body, &mut body_visible);
            }
            HirStmt::For {
                binding,
                iterable,
                body,
                ..
            } => {
                self.check_unknown_bindings_in_expr(iterable, visible);
                let mut body_visible = visible.clone();
                body_visible.insert(binding.clone());
                self.check_unknown_bindings_in_block(body, &mut body_visible);
            }
            HirStmt::Match { value, arms, .. } => {
                self.check_unknown_bindings_in_expr(value, visible);
                for arm in arms {
                    let mut arm_visible = visible.clone();
                    if let MatchPattern::Variant {
                        binding: Some(binding),
                        ..
                    } = &arm.pattern
                    {
                        arm_visible.insert(binding.clone());
                    }
                    self.check_unknown_bindings_in_block(&arm.body, &mut arm_visible);
                }
            }
            HirStmt::Expr(value) => self.check_unknown_bindings_in_expr(value, visible),
            HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
        }
    }

    fn check_unknown_bindings_in_expr(&mut self, expr: &HirExpr, visible: &HashSet<String>) {
        match expr {
            HirExpr::Ident { name, span, .. } => {
                if !visible.contains(name) && !builtin_value_ident(name) {
                    self.unknown_binding_diagnostic(name, span);
                }
            }
            HirExpr::Binary { left, right, .. } => {
                self.check_unknown_bindings_in_expr(left, visible);
                self.check_unknown_bindings_in_expr(right, visible);
            }
            HirExpr::Field { base, .. } => self.check_unknown_bindings_in_expr(base, visible),
            HirExpr::Index { base, index, .. } => {
                self.check_unknown_bindings_in_expr(base, visible);
                self.check_unknown_bindings_in_expr(index, visible);
            }
            HirExpr::Call { args, .. } => {
                for arg in args {
                    self.check_unknown_bindings_in_expr(&arg.value, visible);
                }
            }
            HirExpr::Effect { value, .. }
            | HirExpr::Manage { value, .. }
            | HirExpr::Spawn { value, .. }
            | HirExpr::Await { value, .. }
            | HirExpr::Try { value, .. } => self.check_unknown_bindings_in_expr(value, visible),
            HirExpr::Closure { params, body, .. } => {
                let mut closure_visible = visible.clone();
                closure_visible.extend(params.iter().cloned());
                self.check_unknown_bindings_in_block(body, &mut closure_visible);
            }
            HirExpr::Number { .. } | HirExpr::String { .. } | HirExpr::Unknown(_) => {}
        }
    }

    fn check_fresh_generic_return_bound(
        &mut self,
        function_name: &str,
        return_ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        let target = fresh_return_target_type(return_ty);
        if bounds.contains_key(&target.name)
            && bounds.get(&target.name).and_then(Option::as_ref) != Some(&GenericBound::Struct)
        {
            self.diagnostics.push(
                Diagnostic::error(
                    code::INVALID_FRESH_RETURN_TYPE,
                    format!(
                        "function `{function_name}` returns `fresh {}` but `{}` is not bounded by `Struct`.",
                        target.name, target.name
                    ),
                    target.span.clone(),
                    "invalid fresh generic type",
                )
                .with_cause("A generic `fresh T` return must require `T: Struct` so freshness is valid for every instantiation.")
                .with_fix(
                    "add_struct_bound",
                    format!("Declare `{}` with `{}: Struct`, or remove `fresh`.", target.name, target.name),
                    "manual",
                ),
            );
        }
    }

    fn check_generic_resource_pool_type_ref(
        &mut self,
        ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        if ty.name == "ResourcePool"
            && let Some(arg) = ty.args.first()
            && let Some(bound) = bounds.get(&arg.name)
            && bound.as_ref() != Some(&GenericBound::Resource)
        {
            self.invalid_resource_pool_type_diagnostic(
                format!(
                    "ResourcePool<{}> requires `{}` to be explicitly bounded by Resource.",
                    arg.name, arg.name
                ),
                arg.span.clone(),
            );
        }
        for arg in &ty.args {
            self.check_generic_resource_pool_type_ref(arg, bounds);
        }
    }

    fn check_resource_type_param_field(
        &mut self,
        ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
        in_resource_pool: bool,
    ) {
        let next_in_resource_pool = in_resource_pool || ty.name == "ResourcePool";
        if !next_in_resource_pool
            && bounds.get(&ty.name).and_then(Option::as_ref) == Some(&GenericBound::Resource)
        {
            self.generic_resource_argument_diagnostic(
                &ty.name,
                &ty.name,
                &ty.span,
                "generic resources cannot directly contain `T: Resource`; use an approved resource container.",
            );
        }
        for arg in &ty.args {
            self.check_resource_type_param_field(arg, bounds, next_in_resource_pool);
        }
    }

    fn check_try_operator_result_returns(&mut self) {
        for (index, item) in self.syntax_program.items.iter().enumerate() {
            let Item::Function(function) = item else {
                continue;
            };
            if function
                .return_ty
                .as_ref()
                .is_some_and(|return_ty| return_ty.name == "Result")
            {
                continue;
            }

            let start = self
                .tokens
                .iter()
                .position(|token| token.span == function.span)
                .unwrap_or(0);
            let end = self
                .syntax_program
                .items
                .iter()
                .skip(index + 1)
                .map(item_span)
                .find_map(|span| self.tokens.iter().position(|token| token.span == *span))
                .unwrap_or(self.tokens.len());

            for token in &self.tokens[start..end] {
                if token.symbol("?") {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_TRY_OPERATOR,
                            format!(
                                "`?` in `{}` requires the function to return `Result<T, E>`.",
                                function.name
                            ),
                            token.span.clone(),
                            "invalid try operator",
                        )
                        .with_cause("RSScript represents recoverable failure in explicit `Result` return types.")
                        .with_fix(
                            "return_result_or_handle_error",
                            "Change the return type to `Result<..., E>` or handle the error explicitly.",
                            "manual",
                        ),
                    );
                }
            }
        }
    }

    fn check_runtime_guarantee_bodies(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            for guarantee in RuntimeGuarantee::ALL {
                if function_has_effect(function, guarantee.effect_name()) {
                    self.check_runtime_guarantee_block(guarantee, &function.name, &function.body);
                }
            }
        }
    }

    fn check_runtime_guarantee_block(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        block: &Block,
    ) {
        for statement in &block.statements {
            self.check_runtime_guarantee_stmt(guarantee, function_name, statement);
        }
    }

    fn check_runtime_guarantee_stmt(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        statement: &Stmt,
    ) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_runtime_guarantee_expr(guarantee, function_name, value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_runtime_guarantee_expr(guarantee, function_name, value);
                }
            }
            Stmt::Expr(value) => self.check_runtime_guarantee_expr(guarantee, function_name, value),
            Stmt::With(stmt) => {
                if guarantee == RuntimeGuarantee::Pure {
                    self.pure_with_resource_diagnostic(function_name, &stmt.span);
                }
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.resource);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.condition);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_runtime_guarantee_block(guarantee, function_name, else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_runtime_guarantee_expr(guarantee, function_name, condition);
                }
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.iterable);
                self.check_runtime_guarantee_block(guarantee, function_name, &stmt.body);
            }
            Stmt::Match(stmt) => {
                self.check_runtime_guarantee_expr(guarantee, function_name, &stmt.value);
                for arm in &stmt.arms {
                    self.check_runtime_guarantee_block(guarantee, function_name, &arm.body);
                }
            }
            Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => {}
        }
    }

    fn check_runtime_guarantee_expr(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        expr: &Expr,
    ) {
        match expr {
            Expr::Call {
                callee, args, span, ..
            } => {
                match self.hir.resolve_call(callee) {
                    CallResolution::Resolved { signature, kind } => {
                        if matches!(kind, ResolvedCalleeKind::Constructor { .. }) {
                            if guarantee == RuntimeGuarantee::Noalloc {
                                self.noalloc_allocation_diagnostic(
                                    function_name,
                                    span,
                                    format!(
                                        "constructor `{}` creates a new value.",
                                        callee_display(callee)
                                    ),
                                );
                            }
                        } else if !signature
                            .effects
                            .iter()
                            .any(|effect| effect == guarantee.effect_name())
                        {
                            self.runtime_guarantee_call_diagnostic(
                                guarantee,
                                function_name,
                                callee,
                                span,
                            );
                        }
                    }
                    CallResolution::EnumVariant | CallResolution::Unknown => {}
                }
                for arg in args {
                    self.check_runtime_guarantee_expr(guarantee, function_name, &arg.value);
                }
            }
            Expr::Manage { value, span } => {
                if guarantee == RuntimeGuarantee::Noalloc {
                    self.noalloc_allocation_diagnostic(
                        function_name,
                        span,
                        "`manage` may allocate while migrating a local graph.".to_string(),
                    );
                }
                if guarantee == RuntimeGuarantee::Pure {
                    self.pure_manage_diagnostic(function_name, span);
                }
                self.check_runtime_guarantee_expr(guarantee, function_name, value);
            }
            Expr::Effect { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, value);
            }
            Expr::Binary { left, right, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, left);
                self.check_runtime_guarantee_expr(guarantee, function_name, right);
            }
            Expr::Field { base, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, base);
            }
            Expr::Index { base, index, .. } => {
                self.check_runtime_guarantee_expr(guarantee, function_name, base);
                self.check_runtime_guarantee_expr(guarantee, function_name, index);
            }
            Expr::Closure { body, .. } => {
                self.check_runtime_guarantee_block(guarantee, function_name, body);
            }
            Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
        }
    }

    fn check_duplicate_declarations(&mut self) {
        for duplicate in self.hir.duplicate_symbols() {
            self.diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_DECLARATION,
                    format!(
                        "{} `{}` is declared more than once.",
                        duplicate_symbol_label(duplicate.kind),
                        duplicate.name
                    ),
                    duplicate.duplicate_span.clone(),
                    "duplicate declaration",
                )
                .with_cause(format!(
                    "The first declaration is at {}:{}.",
                    duplicate.first_span.line, duplicate.first_span.column
                ))
                .with_fix(
                    "rename_declaration",
                    "Rename or remove one declaration so the symbol table is unambiguous.",
                    "manual",
                ),
            );
        }
    }

    fn check_resource_fields(&mut self) {
        for item in &self.syntax_program.items {
            let Item::Type(decl) = item else {
                continue;
            };
            if self.hir.type_kind(&decl.name) == Some(HirTypeKind::Resource) {
                continue;
            }
            for field in &decl.fields {
                if self.hir.type_kind(&field.ty.name) == Some(HirTypeKind::Resource) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::RESOURCE_FIELD,
                            format!("resource `{}` cannot be stored in `{}`.", field.ty.name, decl.name),
                            field.span.clone(),
                            "resource field",
                        )
                        .with_cause("Resources must be used through `with` or approved resource containers.")
                        .with_fix("use_with", "Use `with` or `ResourcePool<T: Resource>` instead.", "manual"),
                    );
                }
            }
        }
    }

    fn check_fd_surface(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Function(function) => {
                    if function.is_native {
                        continue;
                    }
                    for param in &function.params {
                        if type_ref_contains_name(&param.ty, "Fd") {
                            self.fd_surface_diagnostic(
                                param.ty.span.clone(),
                                "`Fd` parameter outside native boundary",
                                "Use a `resource` type such as `File` instead of exposing raw descriptor handles.",
                            );
                        }
                    }
                    if let Some(return_ty) = &function.return_ty
                        && type_ref_contains_name(return_ty, "Fd")
                    {
                        self.fd_surface_diagnostic(
                            return_ty.span.clone(),
                            "`Fd` return outside native boundary",
                            "Return a `resource` type such as `File` instead of exposing raw descriptor handles.",
                        );
                    }
                }
                Item::Type(decl) => {
                    if decl.kind == TypeKind::Resource {
                        continue;
                    }
                    for field in &decl.fields {
                        if type_ref_contains_name(&field.ty, "Fd") {
                            self.fd_surface_diagnostic(
                                field.ty.span.clone(),
                                "`Fd` field outside resource internals",
                                "Use a `resource` field wrapper or a non-Fd public value type.",
                            );
                        }
                    }
                }
            }
        }
    }

    fn check_weak_fields(&mut self) {
        for item in &self.syntax_program.items {
            let Item::Type(decl) = item else {
                continue;
            };
            for field in &decl.fields {
                if !field.is_weak {
                    continue;
                }
                if self.hir.type_kind(&field.ty.name) != Some(HirTypeKind::Class) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_WEAK_FIELD,
                            format!(
                                "weak field `{}` must point to a class, but `{}` is not a class.",
                                field.name, field.ty.name
                            ),
                            field.span.clone(),
                            "invalid weak field",
                        )
                        .with_cause(
                            "`weak` is only for breaking managed identity-object cycles in the MVP.",
                        )
                        .with_fix(
                            "use_class_or_remove_weak",
                            "Use a class type for the weak field, or remove `weak`.",
                            "manual",
                        ),
                    );
                }
            }
        }
    }

    fn check_resource_pool_type_arguments(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    for field in &decl.fields {
                        self.check_resource_pool_type_ref(&field.ty);
                    }
                }
                Item::Function(function) => {
                    for param in &function.params {
                        self.check_resource_pool_type_ref(&param.ty);
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_resource_pool_type_ref(return_ty);
                    }
                    self.check_resource_pool_calls_in_block(&function.body);
                }
            }
        }
    }

    fn check_resource_generic_arguments(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            match item {
                Item::Type(decl) => {
                    for field in &decl.fields {
                        self.check_resource_generic_type_ref(
                            &field.ty,
                            ResourceGenericContext::Ordinary,
                        );
                    }
                }
                Item::Function(function) => {
                    for param in &function.params {
                        self.check_resource_generic_type_ref(
                            &param.ty,
                            ResourceGenericContext::Ordinary,
                        );
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_resource_generic_type_ref(
                            return_ty,
                            ResourceGenericContext::Return,
                        );
                    }
                    self.check_resource_generic_calls_in_block(&function.body);
                }
            }
        }
    }

    fn check_resource_pool_type_ref(&mut self, ty: &TypeRef) {
        if ty.name == "ResourcePool" {
            match ty.args.first() {
                Some(arg) => self.check_resource_pool_arg(&arg.name, &arg.span),
                None => self.invalid_resource_pool_type_diagnostic(
                    "ResourcePool must declare a resource type argument.",
                    ty.span.clone(),
                ),
            }
        }
        for arg in &ty.args {
            self.check_resource_pool_type_ref(arg);
        }
    }

    fn check_resource_pool_calls_in_block(&mut self, block: &crate::syntax::ast::Block) {
        for statement in &block.statements {
            self.check_resource_pool_calls_in_stmt(statement);
        }
    }

    fn check_resource_generic_type_ref(&mut self, ty: &TypeRef, context: ResourceGenericContext) {
        if ty.name != "ResourcePool" {
            for (index, arg) in ty.args.iter().enumerate() {
                if self.hir.type_kind(&arg.name) == Some(HirTypeKind::Resource)
                    && !resource_result_return_arg_allowed(ty, index, context)
                {
                    self.resource_generic_argument_diagnostic(&ty.name, &arg.name, &arg.span);
                }
            }
        }
        for (index, arg) in ty.args.iter().enumerate() {
            if resource_result_return_arg_allowed(ty, index, context) {
                continue;
            }
            self.check_resource_generic_type_ref(arg, ResourceGenericContext::Ordinary);
        }
    }

    fn check_resource_pool_calls_in_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_pool_calls_in_expr(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_pool_calls_in_expr(value);
                }
            }
            Stmt::Expr(value) => self.check_resource_pool_calls_in_expr(value),
            Stmt::With(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.resource);
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.condition);
                self.check_resource_pool_calls_in_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_resource_pool_calls_in_block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_resource_pool_calls_in_expr(condition);
                }
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.iterable);
                self.check_resource_pool_calls_in_block(&stmt.body);
            }
            Stmt::Match(stmt) => {
                self.check_resource_pool_calls_in_expr(&stmt.value);
                for arm in &stmt.arms {
                    self.check_resource_pool_calls_in_block(&arm.body);
                }
            }
            Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => {}
        }
    }

    fn check_resource_pool_calls_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call { callee, args, span } => {
                if let Callee::Qualified { namespace, name } = callee
                    && namespace == "ResourcePool"
                    && name == "new"
                {
                    self.invalid_resource_pool_type_diagnostic(
                        "ResourcePool.new must be called as ResourcePool<T>.new with resource T.",
                        span.clone(),
                    );
                } else if let Callee::Qualified { namespace, .. } = callee
                    && let Some(arg) = resource_pool_namespace_arg(namespace)
                {
                    self.check_resource_pool_arg(arg, span);
                }
                for arg in args {
                    self.check_resource_pool_calls_in_expr(&arg.value);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_resource_pool_calls_in_expr(left);
                self.check_resource_pool_calls_in_expr(right);
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_resource_pool_calls_in_expr(value);
            }
            Expr::Field { base, .. } => self.check_resource_pool_calls_in_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_resource_pool_calls_in_expr(base);
                self.check_resource_pool_calls_in_expr(index);
            }
            Expr::Closure { body, .. } => self.check_resource_pool_calls_in_block(body),
            Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
        }
    }

    fn check_resource_generic_calls_in_block(&mut self, block: &crate::syntax::ast::Block) {
        for statement in &block.statements {
            self.check_resource_generic_calls_in_stmt(statement);
        }
    }

    fn check_resource_generic_calls_in_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_generic_calls_in_expr(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_resource_generic_calls_in_expr(value);
                }
            }
            Stmt::Expr(value) => self.check_resource_generic_calls_in_expr(value),
            Stmt::With(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.resource);
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.condition);
                self.check_resource_generic_calls_in_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_resource_generic_calls_in_block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_resource_generic_calls_in_expr(condition);
                }
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::For(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.iterable);
                self.check_resource_generic_calls_in_block(&stmt.body);
            }
            Stmt::Match(stmt) => {
                self.check_resource_generic_calls_in_expr(&stmt.value);
                for arm in &stmt.arms {
                    self.check_resource_generic_calls_in_block(&arm.body);
                }
            }
            Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::MalformedWith(_)
            | Stmt::MalformedIf(_)
            | Stmt::MalformedLoop(_)
            | Stmt::MalformedFor(_)
            | Stmt::MalformedMatch(_)
            | Stmt::Unknown(_) => {}
        }
    }

    fn check_resource_generic_calls_in_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call { callee, args, span } => {
                if let Callee::Qualified { namespace, .. } = callee
                    && let Some((root, args)) = generic_namespace_args(namespace)
                    && root != "ResourcePool"
                {
                    for arg in args {
                        if self.hir.type_kind(arg) == Some(HirTypeKind::Resource) {
                            self.resource_generic_argument_diagnostic(root, arg, span);
                        }
                    }
                }
                for arg in args {
                    self.check_resource_generic_calls_in_expr(&arg.value);
                }
            }
            Expr::Binary { left, right, .. } => {
                self.check_resource_generic_calls_in_expr(left);
                self.check_resource_generic_calls_in_expr(right);
            }
            Expr::Effect { value, .. }
            | Expr::Manage { value, .. }
            | Expr::Spawn { value, .. }
            | Expr::Await { value, .. }
            | Expr::Try { value, .. } => {
                self.check_resource_generic_calls_in_expr(value);
            }
            Expr::Field { base, .. } => self.check_resource_generic_calls_in_expr(base),
            Expr::Index { base, index, .. } => {
                self.check_resource_generic_calls_in_expr(base);
                self.check_resource_generic_calls_in_expr(index);
            }
            Expr::Closure { body, .. } => self.check_resource_generic_calls_in_block(body),
            Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
        }
    }

    fn check_resource_pool_arg(&mut self, type_name: &str, span: &crate::diagnostic::Span) {
        match self.hir.type_kind(type_name) {
            Some(HirTypeKind::Resource) | None => {}
            Some(HirTypeKind::Class) | Some(HirTypeKind::Struct) => {
                self.invalid_resource_pool_type_diagnostic(
                    format!(
                        "ResourcePool can only hold resources, but `{type_name}` is not a resource."
                    ),
                    span.clone(),
                );
            }
        }
    }

    fn invalid_resource_pool_type_diagnostic(
        &mut self,
        summary: impl Into<String>,
        span: crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_RESOURCE_POOL_TYPE,
                summary,
                span,
                "invalid ResourcePool type",
            )
            .with_cause("`ResourcePool<T>` is the privileged container for long-lived resource values, so `T` must be a resource.")
            .with_fix(
                "use_resource_type",
                "Use a resource type argument or a non-resource container for ordinary values.",
                "manual",
            ),
        );
    }

    fn fd_surface_diagnostic(
        &mut self,
        span: crate::diagnostic::Span,
        summary: impl Into<String>,
        fix: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::FD_OUTSIDE_INTERNAL_BOUNDARY,
                summary,
                span,
                "Fd outside native/resource internals",
            )
            .with_cause(
                "`Fd` is a trusted native/resource-internal descriptor handle, not an ordinary RSScript value type.",
            )
            .with_fix("use_resource_wrapper", fix, "manual"),
        );
    }

    fn unknown_type_diagnostic(&mut self, ty: &TypeRef) {
        self.unknown_type_name_diagnostic(&type_ref_name(ty), &ty.span);
    }

    fn unknown_type_name_diagnostic(&mut self, name: &str, span: &crate::diagnostic::Span) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_TYPE,
                format!("unknown type `{name}`."),
                span.clone(),
                "unknown type",
            )
            .with_cause("RSScript type checking must resolve source-level types before Rust lowering.")
            .with_fix(
                "declare_or_import_type",
                format!(
                    "Declare `{}`, import an `.rssi` contract that declares it, or use a known core/runtime type.",
                    name
                ),
                "manual",
            ),
        );
    }

    fn unknown_protocol_diagnostic(&mut self, name: &str, span: &crate::diagnostic::Span) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_PROTOCOL,
                format!("unknown protocol `{name}`."),
                span.clone(),
                "unknown protocol",
            )
            .with_cause(
                "Protocol bounds and implementations must name an explicit `protocol` declaration.",
            )
            .with_fix(
                "declare_protocol",
                format!("Declare `protocol {name} {{ ... }}` or use a declared protocol name."),
                "manual",
            ),
        );
    }

    fn protocol_impl_mismatch_diagnostic(
        &mut self,
        protocol: &str,
        type_name: &str,
        method: &str,
        span: &crate::diagnostic::Span,
        label: impl Into<String>,
        cause: impl Into<String>,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::PACKAGE_INTERFACE_MISMATCH,
                format!("`{type_name}` does not satisfy protocol `{protocol}` method `{method}`."),
                span.clone(),
                label,
            )
            .with_cause(cause)
            .with_fix(
                "fix_protocol_impl_mapping",
                "Update the protocol impl mapping or concrete function signature to match the protocol contract exactly.",
                "manual",
            ),
        );
    }

    fn unknown_field_diagnostic(
        &mut self,
        field_name: &str,
        base_type: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_FIELD,
                format!("unknown field `{field_name}` on type `{base_type}`."),
                span.clone(),
                "unknown field",
            )
            .with_cause("RSScript field accesses must resolve before Rust lowering.")
            .with_fix(
                "use_declared_field",
                format!("Use a field declared on `{base_type}` or update the type declaration."),
                "manual",
            ),
        );
    }

    fn unknown_binding_diagnostic(&mut self, name: &str, span: &crate::diagnostic::Span) {
        self.diagnostics.push(
            Diagnostic::error(
                code::UNKNOWN_BINDING,
                format!("unknown value binding `{name}`."),
                span.clone(),
                "unknown binding",
            )
            .with_cause("RSScript values must resolve before Rust lowering.")
            .with_fix(
                "declare_binding",
                format!("Declare `{name}` before using it or pass it as a parameter."),
                "manual",
            ),
        );
    }

    fn resource_generic_argument_diagnostic(
        &mut self,
        generic_name: &str,
        resource_name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::RESOURCE_GENERIC_ARGUMENT,
                format!(
                    "generic type `{generic_name}` cannot be instantiated with resource `{resource_name}`."
                ),
                span.clone(),
                "resource generic argument",
            )
            .with_cause("Only explicit resource APIs such as `ResourcePool<T: Resource>` may hold resources.")
            .with_fix(
                "use_resource_api",
                "Use `with`, `ResourcePool<T: Resource>`, or a non-resource value type.",
                "manual",
            ),
        );
    }

    fn generic_resource_argument_diagnostic(
        &mut self,
        generic_name: &str,
        resource_name: &str,
        span: &crate::diagnostic::Span,
        cause: &str,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::RESOURCE_GENERIC_ARGUMENT,
                format!(
                    "generic type `{generic_name}` cannot be used with resource `{resource_name}`."
                ),
                span.clone(),
                "resource generic misuse",
            )
            .with_cause(cause)
            .with_fix(
                "add_or_change_resource_bound",
                "Use explicit `T: Resource` only with approved resource APIs such as `ResourcePool<T>`.",
                "manual",
            ),
        );
    }

    fn noalloc_allocation_diagnostic(
        &mut self,
        function_name: &str,
        span: &crate::diagnostic::Span,
        cause: String,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NOALLOC_ALLOCATION,
                format!("`{function_name}` is declared noalloc but contains an allocation site."),
                span.clone(),
                "allocation in noalloc function",
            )
            .with_cause(cause)
            .with_fix(
                "remove_allocation_or_noalloc",
                "Remove the allocation site, or remove `noalloc` from the function effects.",
                "manual",
            ),
        );
    }

    fn allocating_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NOALLOC_CALL,
                format!(
                    "`{function_name}` is declared noalloc but calls possibly allocating function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "possibly allocating call in noalloc function",
            )
            .with_cause(
                "A `noalloc` function may only call enum variants or functions also declared `effects(noalloc)`.",
            )
            .with_fix(
                "remove_noalloc_or_call_noalloc",
                "Remove `noalloc`, or call only APIs whose signatures are declared `effects(noalloc)`.",
                "manual",
            ),
        );
    }

    fn runtime_guarantee_call_diagnostic(
        &mut self,
        guarantee: RuntimeGuarantee,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        match guarantee {
            RuntimeGuarantee::Noalloc => {
                self.allocating_call_diagnostic(function_name, callee, span)
            }
            RuntimeGuarantee::Pure => self.non_pure_call_diagnostic(function_name, callee, span),
            RuntimeGuarantee::NoBlock => self.blocking_call_diagnostic(function_name, callee, span),
            RuntimeGuarantee::NoPanic => self.panic_call_diagnostic(function_name, callee, span),
        }
    }

    fn non_pure_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_PURE_EFFECT,
                format!(
                    "`{function_name}` is declared pure but calls non-pure function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "non-pure call in pure function",
            )
            .with_cause(
                "A `pure` function may only call constructors, enum variants, or functions also declared `effects(pure)`.",
            )
            .with_fix(
                "remove_pure_or_call_pure",
                "Remove `pure`, or call only APIs whose signatures are declared `effects(pure)`.",
                "manual",
            ),
        );
    }

    fn pure_manage_diagnostic(&mut self, function_name: &str, span: &crate::diagnostic::Span) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_PURE_EFFECT,
                format!("`{function_name}` is declared pure but uses `manage`."),
                span.clone(),
                "manage in pure function",
            )
            .with_cause(
                "`manage` consumes a local value and changes its ownership boundary; `pure` functions may observe inputs but must not consume local values.",
            )
            .with_fix(
                "remove_manage_or_pure",
                "Move the `manage` operation outside the pure function, or remove `pure`.",
                "manual",
            ),
        );
    }

    fn pure_with_resource_diagnostic(
        &mut self,
        function_name: &str,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_PURE_EFFECT,
                format!("`{function_name}` is declared pure but opens a resource scope."),
                span.clone(),
                "resource scope in pure function",
            )
            .with_cause(
                "`with` introduces deterministic resource lifetime behavior; `pure` functions may observe inputs but must not open resource scopes.",
            )
            .with_fix(
                "remove_with_or_pure",
                "Move resource handling outside the pure function, or remove `pure`.",
                "manual",
            ),
        );
    }

    fn pure_resource_return_diagnostic(
        &mut self,
        function_name: &str,
        span: crate::diagnostic::Span,
        resource_name: &str,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_PURE_EFFECT,
                format!(
                    "`{function_name}` is declared pure but returns resource `{resource_name}`."
                ),
                span,
                "resource return in pure function",
            )
            .with_cause(
                "Returning a resource creates a lifetime boundary; `pure` functions must not open or return resources.",
            )
            .with_fix(
                "remove_resource_return_or_pure",
                "Return an ordinary value, or remove `pure` from the resource-producing function.",
                "manual",
            ),
        );
    }

    fn resource_return_type_name<'a>(&self, ty: &'a TypeRef) -> Option<&'a str> {
        let target = if matches!(ty.name.as_str(), "Result" | "Option") {
            ty.args.first().unwrap_or(ty)
        } else {
            ty
        };
        if self.hir.type_kind(&target.name) == Some(HirTypeKind::Resource) {
            Some(target.name.as_str())
        } else {
            None
        }
    }

    fn blocking_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NO_BLOCK_CALL,
                format!(
                    "`{function_name}` is declared no_block but calls possibly blocking function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "possibly blocking call in no_block function",
            )
            .with_cause(
                "A `no_block` function may only call constructors, enum variants, or functions also declared `effects(no_block)`.",
            )
            .with_fix(
                "remove_no_block_or_call_no_block",
                "Remove `no_block`, or call only APIs whose signatures are declared `effects(no_block)`.",
                "manual",
            ),
        );
    }

    fn panic_call_diagnostic(
        &mut self,
        function_name: &str,
        callee: &Callee,
        span: &crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                code::INVALID_NO_PANIC_CALL,
                format!(
                    "`{function_name}` is declared no_panic but calls possibly panicking function `{}`.",
                    callee_display(callee)
                ),
                span.clone(),
                "possibly panicking call in no_panic function",
            )
            .with_cause(
                "A `no_panic` function may only call constructors, enum variants, or functions also declared `effects(no_panic)`.",
            )
            .with_fix(
                "remove_no_panic_or_call_no_panic",
                "Remove `no_panic`, or call only APIs whose signatures are declared `effects(no_panic)`.",
                "manual",
            ),
        );
    }
}

fn resource_pool_namespace_arg(namespace: &str) -> Option<&str> {
    namespace
        .strip_prefix("ResourcePool<")
        .and_then(|rest| rest.strip_suffix('>'))
}

fn generic_namespace_args(namespace: &str) -> Option<(&str, Vec<&str>)> {
    let (root, rest) = namespace.split_once('<')?;
    let args = rest.strip_suffix('>')?;
    Some((root, split_top_level_type_args(args)))
}

fn split_top_level_type_args(args: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(args[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    result.push(args[start..].trim());
    result
}

fn effect_name(effect: &EffectDecl) -> &str {
    match effect {
        EffectDecl::Name(name) | EffectDecl::Retains(name) => name,
    }
}

fn data_effect_name(effect: DataEffect) -> &'static str {
    match effect {
        DataEffect::Read => "read",
        DataEffect::Mut => "mut",
        DataEffect::Take => "take",
    }
}

fn effect_display(effect: &EffectDecl) -> String {
    match effect {
        EffectDecl::Name(name) => name.clone(),
        EffectDecl::Retains(param) => format!("retains({param})"),
    }
}

fn generic_bounds(params: &[GenericParam]) -> HashMap<String, Option<GenericBound>> {
    params
        .iter()
        .map(|param| (param.name.clone(), param.bound.clone()))
        .collect()
}

fn protocol_method_names(items: &[Item], protocol: &str) -> HashSet<String> {
    items
        .iter()
        .filter_map(|item| {
            let Item::Function(function) = item else {
                return None;
            };
            let (namespace, method) = split_qualified_name(&function.name);
            (namespace.as_deref() == Some(protocol)).then(|| method.to_string())
        })
        .collect()
}

fn function_body_belongs_to_protocol(
    function: &FunctionDecl,
    protocol_names: &HashSet<String>,
) -> bool {
    !function.body.statements.is_empty()
        && split_qualified_name(&function.name)
            .0
            .is_some_and(|namespace| protocol_names.contains(&namespace))
}

fn protocol_signature_mismatch(
    protocol: &FunctionSig,
    target: &FunctionSig,
    concrete_type: &str,
) -> Option<String> {
    if protocol.is_async != target.is_async {
        return Some("async/sync kind must match the protocol method exactly.".to_string());
    }
    if protocol.params.len() != target.params.len() {
        return Some(format!(
            "parameter count mismatch: protocol has {}, implementation has {}.",
            protocol.params.len(),
            target.params.len()
        ));
    }
    for (protocol_param, target_param) in protocol.params.iter().zip(&target.params) {
        if let Some(reason) = protocol_param_mismatch(protocol_param, target_param, concrete_type) {
            return Some(reason);
        }
    }
    let protocol_return = protocol
        .return_type
        .as_deref()
        .map(|return_type| substitute_protocol_self(return_type, concrete_type));
    if protocol_return.as_deref() != target.return_type.as_deref() {
        return Some(format!(
            "return type mismatch: protocol expects `{}`, implementation returns `{}`.",
            protocol_return.as_deref().unwrap_or("Unit"),
            target.return_type.as_deref().unwrap_or("Unit")
        ));
    }
    if protocol.returns_fresh != target.returns_fresh {
        return Some("fresh return mode must match the protocol method exactly.".to_string());
    }
    let protocol_effects = protocol.effects.iter().collect::<HashSet<_>>();
    let target_effects = target.effects.iter().collect::<HashSet<_>>();
    if protocol_effects != target_effects {
        return Some(
            "guarantee and boundary effects must match the protocol method exactly.".to_string(),
        );
    }
    if protocol.retained_params != target.retained_params {
        return Some("retains(...) effects must match the protocol method exactly.".to_string());
    }
    None
}

fn protocol_param_mismatch(
    protocol: &ParamSig,
    target: &ParamSig,
    concrete_type: &str,
) -> Option<String> {
    if protocol.name != target.name {
        return Some(format!(
            "parameter name mismatch: protocol expects `{}`, implementation has `{}`.",
            protocol.name, target.name
        ));
    }
    if protocol.effect != target.effect {
        return Some(format!(
            "parameter effect mismatch for `{}`: protocol expects `{}`, implementation has `{}`.",
            protocol.name,
            protocol
                .effect
                .map(|effect| effect.as_str())
                .unwrap_or("none"),
            target
                .effect
                .map(|effect| effect.as_str())
                .unwrap_or("none")
        ));
    }
    let expected_type = substitute_protocol_self(&protocol.type_name, concrete_type);
    if expected_type != target.type_name {
        return Some(format!(
            "parameter type mismatch for `{}`: protocol expects `{expected_type}`, implementation has `{}`.",
            protocol.name, target.type_name
        ));
    }
    None
}

fn substitute_protocol_self(type_name: &str, concrete_type: &str) -> String {
    if type_name == "Self" {
        return concrete_type.to_string();
    }
    type_name.replace("Self", concrete_type)
}

fn split_qualified_name(name: &str) -> (Option<String>, &str) {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        (Some(namespace.to_string()), name)
    } else {
        (None, name)
    }
}

fn fresh_return_target_type(return_ty: &TypeRef) -> &TypeRef {
    if matches!(return_ty.name.as_str(), "Result" | "Option")
        && let Some(first_arg) = return_ty.args.first()
    {
        return first_arg;
    }
    return_ty
}

fn function_has_effect(function: &crate::syntax::ast::FunctionDecl, effect_name: &str) -> bool {
    function
        .effects
        .iter()
        .any(|effect| matches!(effect, EffectDecl::Name(name) if name == effect_name))
}

fn match_is_exhaustive(arms: &[crate::syntax::ast::MatchArm]) -> bool {
    let mut variants = HashSet::new();
    for arm in arms {
        match &arm.pattern {
            MatchPattern::Wildcard(_) => return true,
            MatchPattern::Variant { name, .. } => {
                variants.insert(name.as_str());
            }
        }
    }
    (variants.contains("Some") && variants.contains("None"))
        || (variants.contains("Ok") && variants.contains("Err"))
}

fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
    }
}

fn removed_runtime_effect_replacement(effect_name: &str) -> Option<&'static str> {
    match effect_name {
        "io" => Some(
            "Remove `io`; I/O is allowed by default unless a guarantee such as `pure` or `no_block` forbids it.",
        ),
        "allocates" => Some(
            "Remove `allocates`; allocation is allowed by default. Use `noalloc` only when the function guarantees no allocation.",
        ),
        "may_panic" => Some(
            "Remove `may_panic`; panic is allowed by default. Use `no_panic` only when the function guarantees no panic.",
        ),
        "may_fail" => Some(
            "Remove `may_fail`; represent failure in the return type, for example `Result<T, E>`.",
        ),
        "async" => Some(
            "Remove `async` from `effects(...)`; write `async fn` when the function itself is async.",
        ),
        _ => None,
    }
}

fn item_span(item: &Item) -> &crate::diagnostic::Span {
    match item {
        Item::Type(decl) => &decl.span,
        Item::Function(function) => &function.span,
    }
}

fn duplicate_symbol_label(kind: DuplicateSymbolKind) -> &'static str {
    match kind {
        DuplicateSymbolKind::Function => "function",
        DuplicateSymbolKind::Type => "type",
        DuplicateSymbolKind::Constructor => "callable",
        DuplicateSymbolKind::Field => "field",
    }
}

fn is_copy_type(ty: &TypeRef) -> bool {
    ty.args.is_empty()
        && matches!(
            ty.name.as_str(),
            "Bool"
                | "Byte"
                | "Char"
                | "Float"
                | "Float32"
                | "Float64"
                | "Int"
                | "Int8"
                | "Int16"
                | "Int32"
                | "Int64"
                | "UInt"
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
                | "Unit"
        )
}

fn known_type_ref(ty: &TypeRef, generic_params: &HashSet<&str>, hir: &Hir) -> bool {
    if ty.name.is_empty() {
        return true;
    }
    if ty.is_noescape {
        return ty.name == "Fn";
    }
    generic_params.contains(ty.name.as_str())
        || is_builtin_type_name(&ty.name)
        || hir.type_info(&ty.name).is_some()
}

fn is_builtin_type_name(name: &str) -> bool {
    matches!(
        name,
        "Unit"
            | "Bool"
            | "Byte"
            | "Char"
            | "Int"
            | "Int8"
            | "Int16"
            | "Int32"
            | "Int64"
            | "UInt"
            | "UInt8"
            | "UInt16"
            | "UInt32"
            | "UInt64"
            | "Float"
            | "Float32"
            | "Float64"
            | "String"
            | "Url"
            | "Fd"
            | "Bytes"
            | "Buffer"
            | "Path"
            | "Result"
            | "Option"
            | "List"
            | "Map"
            | "Set"
            | "Fn"
            | "Closure"
            | "Cache"
            | "FileError"
            | "IOError"
            | "HttpError"
            | "ConfigError"
            | "DbError"
            | "ImageError"
            | "JsonError"
            | "CsvError"
            | "NetworkError"
    )
}

fn builtin_value_ident(name: &str) -> bool {
    matches!(name, "true" | "false" | "Unit" | "None")
}

fn type_ref_contains_name(ty: &TypeRef, name: &str) -> bool {
    ty.name == name
        || ty.args.iter().any(|arg| type_ref_contains_name(arg, name))
        || ty
            .fn_params
            .iter()
            .any(|param| type_ref_contains_name(param, name))
        || ty
            .fn_return
            .as_deref()
            .is_some_and(|return_ty| type_ref_contains_name(return_ty, name))
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
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

fn type_ref_is_noescape(ty: &TypeRef) -> bool {
    ty.is_noescape || ty.args.iter().any(type_ref_is_noescape)
}

fn type_ref_is_copy(ty: &TypeRef) -> bool {
    !ty.is_fresh
        && !ty.is_noescape
        && ty.args.is_empty()
        && ty.fn_params.is_empty()
        && ty.fn_return.is_none()
        && matches!(
            ty.name.as_str(),
            "Bool"
                | "Byte"
                | "Char"
                | "Float"
                | "Float32"
                | "Float64"
                | "Int"
                | "Int8"
                | "Int16"
                | "Int32"
                | "Int64"
                | "UInt"
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
                | "Unit"
        )
}

fn type_ref_is_closure_effect_exempt(ty: &TypeRef) -> bool {
    ty.args.is_empty() && !ty.is_noescape && ty.name == "Closure"
}

fn type_ref_has_surface_reference(ty: &TypeRef, tokens: &[Token]) -> bool {
    tokens.iter().any(|token| {
        token.symbol("&")
            && token.span.file == ty.span.file
            && token.span.line == ty.span.line
            && token.span.column + token.span.length <= ty.span.column
    })
}
