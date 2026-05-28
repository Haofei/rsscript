use std::collections::HashSet;

use crate::checks;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::{DuplicateSymbolKind, Hir, HirTypeKind};
use crate::lexer::{Token, lex};
use crate::syntax::ast::{Callee, DataEffect, EffectDecl, Expr, Item, Stmt, TypeRef};
use crate::syntax::parse_source;

pub fn analyze_source(file: &str, source: &str) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let syntax_program = parse_source(file, source);
    let hir = Hir::from_syntax(&syntax_program);
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
        self.check_file_mode_present();
        self.check_single_file_mode();
        self.check_removed_profile_declarations();
        self.check_duplicate_declarations();
        self.check_signature_explicitness();
        self.check_resource_fields();
        self.check_resource_pool_type_arguments();
        self.check_resource_generic_arguments();
        checks::mode::check(self);
        checks::calls::check(self);
        checks::body::check(self);
        checks::forbidden::check(self);
    }

    fn check_file_mode_present(&mut self) {
        if self.syntax_program.mode.is_none() {
            let span = self.tokens.first().map(|token| token.span.clone()).unwrap();
            self.diagnostics.push(
                Diagnostic::error(
                    code::MISSING_FILE_MODE,
                    "RSScript files must declare exactly one file mode.",
                    span,
                    "missing mode",
                )
                .with_fix(
                    "add_mode",
                    "Add `mode: managed` or `mode: uses-local`.",
                    "manual",
                ),
            );
        }
    }

    fn check_single_file_mode(&mut self) {
        for span in self.syntax_program.mode_spans.iter().skip(1) {
            self.diagnostics.push(
                Diagnostic::error(
                    code::DUPLICATE_FILE_MODE,
                    "RSScript files must declare exactly one file mode.",
                    span.clone(),
                    "duplicate mode",
                )
                .with_cause("Only one top-level `mode:` declaration is allowed.")
                .with_fix(
                    "remove_duplicate_mode",
                    "Remove the extra `mode:` declaration.",
                    "manual",
                ),
            );
        }
    }

    fn check_removed_profile_declarations(&mut self) {
        for span in &self.syntax_program.profile_spans {
            self.diagnostics.push(
                Diagnostic::error(
                    code::REMOVED_PROFILE_DECLARATION,
                    "`profile:` declarations were removed in RSScript v0.4.1.",
                    span.clone(),
                    "removed profile declaration",
                )
                .with_cause("v0.4.1 has one canonical surface style and only `mode:` as the top-level semantic file declaration.")
                .with_fix(
                    "remove_profile",
                    "Remove `profile:` and keep exactly one `mode: managed` or `mode: uses-local` declaration.",
                    "manual",
                ),
            );
        }
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
                if param.effect.is_none() && !param.ty.name.is_empty() && !is_copy_type(&param.ty) {
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
            Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
                self.check_resource_pool_calls_in_expr(value);
            }
            Expr::Field { base, .. } => self.check_resource_pool_calls_in_expr(base),
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
            Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
                self.check_resource_generic_calls_in_expr(value);
            }
            Expr::Field { base, .. } => self.check_resource_generic_calls_in_expr(base),
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
