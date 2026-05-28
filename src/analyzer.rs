use std::collections::{HashMap, HashSet};

use crate::checks;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::{DuplicateSymbolKind, Hir, HirTypeKind};
use crate::interfaces::CORE_INTERFACES;
use crate::lexer::{Token, lex};
use crate::syntax::ast::{
    Block, Callee, DataEffect, EffectDecl, Expr, GenericBound, GenericParam, Item, Stmt, TypeKind,
    TypeRef,
};
use crate::syntax::parse_source;

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

impl Analyzer<'_> {
    fn run(&mut self) {
        self.check_single_feature_declaration();
        self.check_unknown_file_features();
        self.check_duplicate_file_features();
        self.check_removed_profile_declarations();
        self.check_unsupported_syntax();
        self.check_duplicate_declarations();
        self.check_signature_explicitness();
        self.check_generic_constraints();
        self.check_noalloc_allocations();
        self.check_try_operator_result_returns();
        self.check_resource_fields();
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
        let bodies = self
            .syntax_program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Function(function) => Some(function.body.clone()),
                Item::Type(_) => None,
            })
            .collect::<Vec<_>>();
        for body in &bodies {
            self.check_unsupported_syntax_block(body);
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
            Stmt::If(stmt) => {
                self.check_unsupported_syntax_expr(&stmt.condition);
                self.check_unsupported_syntax_block(&stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_unsupported_syntax_block(else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_unsupported_syntax_expr(condition);
                }
                self.check_unsupported_syntax_block(&stmt.body);
            }
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
                    self.check_unsupported_syntax_expr(&arg.value);
                }
            }
            Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
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

    fn check_signature_explicitness(&mut self) {
        for item in &self.syntax_program.items {
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

            for param in &function.params {
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
                    && !type_ref_has_surface_reference(&param.ty, self.tokens)
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

            for effect in &function.effects {
                let effect_name = effect_name(effect);
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
                    || matches!(effect, EffectDecl::Retains(_));
                if !valid {
                    self.diagnostics.push(Diagnostic::warning(
                        code::UNKNOWN_EFFECT,
                        format!("unknown effect `{effect_name}` in `{}`.", function.name),
                        function.span.clone(),
                        "unknown effect",
                    ));
                }
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
                }
            }

            if function
                .effects
                .iter()
                .any(|effect| matches!(effect, EffectDecl::Name(name) if name == "pure"))
            {
                for param in function
                    .params
                    .iter()
                    .filter(|param| param.effect == Some(DataEffect::Mut))
                {
                    self.diagnostics.push(
                        Diagnostic::error(
                            code::INVALID_PURE_EFFECT,
                            format!(
                                "`{}` is declared pure but parameter `{}` is mutable.",
                                function.name, param.name
                            ),
                            param.span.clone(),
                            "mutable parameter in pure function",
                        )
                        .with_cause("A `pure` function must not mutate reachable managed state.")
                        .with_fix(
                            "remove_pure_or_mut",
                            "Remove `pure`, or change the parameter to `read` if the function does not mutate it.",
                            "manual",
                        ),
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

    fn check_fresh_generic_return_bound(
        &mut self,
        function_name: &str,
        return_ty: &TypeRef,
        bounds: &HashMap<String, Option<GenericBound>>,
    ) {
        let target = fresh_return_target_type(return_ty);
        if bounds.contains_key(&target.name)
            && bounds.get(&target.name).copied().flatten() != Some(GenericBound::Struct)
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
            && *bound != Some(GenericBound::Resource)
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
            && bounds.get(&ty.name).copied().flatten() == Some(GenericBound::Resource)
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

    fn check_noalloc_allocations(&mut self) {
        let items = self.syntax_program.items.clone();
        for item in &items {
            let Item::Function(function) = item else {
                continue;
            };
            if !function_has_effect(function, "noalloc") {
                continue;
            }
            self.check_noalloc_block(&function.name, &function.body);
        }
    }

    fn check_noalloc_block(&mut self, function_name: &str, block: &crate::syntax::ast::Block) {
        for statement in &block.statements {
            self.check_noalloc_stmt(function_name, statement);
        }
    }

    fn check_noalloc_stmt(&mut self, function_name: &str, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_noalloc_expr(function_name, value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_noalloc_expr(function_name, value);
                }
            }
            Stmt::Expr(value) => self.check_noalloc_expr(function_name, value),
            Stmt::With(stmt) => {
                self.check_noalloc_expr(function_name, &stmt.resource);
                self.check_noalloc_block(function_name, &stmt.body);
            }
            Stmt::If(stmt) => {
                self.check_noalloc_expr(function_name, &stmt.condition);
                self.check_noalloc_block(function_name, &stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    self.check_noalloc_block(function_name, else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    self.check_noalloc_expr(function_name, condition);
                }
                self.check_noalloc_block(function_name, &stmt.body);
            }
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
        }
    }

    fn check_noalloc_expr(&mut self, function_name: &str, expr: &Expr) {
        match expr {
            Expr::Call {
                callee, args, span, ..
            } => {
                if let Callee::Name(name) = callee
                    && self.hir.type_kind(name).is_some()
                {
                    self.noalloc_allocation_diagnostic(
                        function_name,
                        span,
                        format!("constructor `{name}` creates a new value."),
                    );
                }
                for arg in args {
                    self.check_noalloc_expr(function_name, &arg.value);
                }
            }
            Expr::Manage { value, span } => {
                self.noalloc_allocation_diagnostic(
                    function_name,
                    span,
                    "`manage` may allocate while migrating a local graph.".to_string(),
                );
                self.check_noalloc_expr(function_name, value);
            }
            Expr::Effect { value, .. } | Expr::Try { value, .. } => {
                self.check_noalloc_expr(function_name, value);
            }
            Expr::Binary { left, right, .. } => {
                self.check_noalloc_expr(function_name, left);
                self.check_noalloc_expr(function_name, right);
            }
            Expr::Field { base, .. } => self.check_noalloc_expr(function_name, base),
            Expr::Index { base, index, .. } => {
                self.check_noalloc_expr(function_name, base);
                self.check_noalloc_expr(function_name, index);
            }
            Expr::Closure { body, .. } => self.check_noalloc_block(function_name, body),
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
                        self.check_resource_generic_type_ref(&field.ty);
                    }
                }
                Item::Function(function) => {
                    for param in &function.params {
                        self.check_resource_generic_type_ref(&param.ty);
                    }
                    if let Some(return_ty) = &function.return_ty {
                        self.check_resource_generic_type_ref(return_ty);
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

    fn check_resource_generic_type_ref(&mut self, ty: &TypeRef) {
        if ty.name != "ResourcePool" {
            for arg in &ty.args {
                if self.hir.type_kind(&arg.name) == Some(HirTypeKind::Resource) {
                    self.resource_generic_argument_diagnostic(&ty.name, &arg.name, &arg.span);
                }
            }
        }
        for arg in &ty.args {
            self.check_resource_generic_type_ref(arg);
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
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
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
            Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
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
            Stmt::Break(_) | Stmt::Continue(_) | Stmt::Unknown(_) => {}
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
            Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
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

fn effect_display(effect: &EffectDecl) -> String {
    match effect {
        EffectDecl::Name(name) => name.clone(),
        EffectDecl::Retains(param) => format!("retains({param})"),
    }
}

fn generic_bounds(params: &[GenericParam]) -> HashMap<String, Option<GenericBound>> {
    params
        .iter()
        .map(|param| (param.name.clone(), param.bound))
        .collect()
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
                | "Closure"
                | "Unit"
        )
}

fn type_ref_name(ty: &TypeRef) -> String {
    if ty.args.is_empty() {
        return ty.name.clone();
    }

    let args = ty
        .args
        .iter()
        .map(type_ref_name)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}<{args}>", ty.name)
}

fn type_ref_has_surface_reference(ty: &TypeRef, tokens: &[Token]) -> bool {
    tokens.iter().any(|token| {
        token.symbol("&")
            && token.span.file == ty.span.file
            && token.span.line == ty.span.line
            && token.span.column + token.span.length <= ty.span.column
    })
}
