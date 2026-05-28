use std::collections::{HashMap, HashSet};

use crate::ast::{Program, TypeKind, parse_program};
use crate::checks;
use crate::diagnostic::Diagnostic;
use crate::hir::{FunctionSig, Hir};
use crate::lexer::{Token, lex};
use crate::syntax::ast::{
    Block as SyntaxBlock, CallArg, Callee, DataEffect, Expr, FunctionDecl as SyntaxFunctionDecl,
    Item, LetKind, Stmt,
};
use crate::syntax::parse_source;

pub fn analyze_source(file: &str, source: &str) -> Vec<Diagnostic> {
    let tokens = lex(file, source);
    let program = parse_program(&tokens);
    let syntax_program = parse_source(file, source);
    let hir = Hir::from_syntax(&syntax_program);
    let mut analyzer = Analyzer {
        tokens: &tokens,
        program,
        syntax_program,
        hir,
        diagnostics: Vec::new(),
    };
    analyzer.run();
    analyzer.diagnostics
}

pub(crate) struct Analyzer<'a> {
    pub(crate) tokens: &'a [Token],
    pub(crate) program: Program,
    pub(crate) syntax_program: crate::syntax::ast::Program,
    pub(crate) hir: Hir,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl Analyzer<'_> {
    fn run(&mut self) {
        self.check_file_mode_present();
        self.check_signature_explicitness();
        self.check_resource_fields();
        checks::mode::check(self);
        checks::calls::check(self);
        self.check_ast_functions();
        checks::forbidden::check(self);
    }

    fn check_file_mode_present(&mut self) {
        if self.program.mode.is_none() {
            let span = self.tokens.first().map(|token| token.span.clone()).unwrap();
            self.diagnostics.push(
                Diagnostic::error(
                    "RS0001",
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

    fn check_signature_explicitness(&mut self) {
        for function in self.program.functions.values() {
            if function
                .return_type
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            {
                let span = self
                    .tokens
                    .get(function.body_start.saturating_sub(1))
                    .map(|token| token.span.clone())
                    .unwrap_or_else(|| self.tokens[0].span.clone());
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0002",
                        format!("function `{}` must declare an explicit return type.", function.name),
                        span,
                        "missing return type",
                    )
                    .with_cause("Public APIs must not rely on inference; this checker applies the canonical rule to all functions.")
                    .with_fix("add_return_type", "Add `-> Unit` or another explicit return type.", "manual"),
                );
            }

            for param in &function.params {
                if param.type_name.is_empty() {
                    let span = self
                        .tokens
                        .get(function.body_start.saturating_sub(1))
                        .map(|token| token.span.clone())
                        .unwrap_or_else(|| self.tokens[0].span.clone());
                    self.diagnostics.push(
                        Diagnostic::error(
                            "RS0003",
                            format!(
                                "parameter `{}` in `{}` must declare an explicit type.",
                                param.name, function.name
                            ),
                            span,
                            "missing parameter type",
                        )
                        .with_fix(
                            "add_parameter_type",
                            "Add an explicit parameter type.",
                            "manual",
                        ),
                    );
                }
            }

            for effect in &function.effects {
                let valid = effect == "no_panic"
                    || effect == "noalloc"
                    || effect == "no_block"
                    || effect == "pure"
                    || effect == "unsafe"
                    || effect == "native"
                    || effect.starts_with("retains(");
                if !valid {
                    let span = self
                        .tokens
                        .get(function.body_start.saturating_sub(1))
                        .map(|token| token.span.clone())
                        .unwrap_or_else(|| self.tokens[0].span.clone());
                    self.diagnostics.push(Diagnostic::warning(
                        "RS0004",
                        format!("unknown effect `{effect}` in `{}`.", function.name),
                        span,
                        "unknown effect",
                    ));
                }
            }
        }
    }

    fn check_resource_fields(&mut self) {
        let resources: HashSet<String> = self
            .program
            .types
            .values()
            .filter(|decl| decl.kind == TypeKind::Resource)
            .map(|decl| decl.name.clone())
            .collect();

        for decl in self.program.types.values() {
            if decl.kind == TypeKind::Resource {
                continue;
            }
            for field in &decl.fields {
                if resources.contains(&field.type_name) {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "RS0701",
                            format!("resource `{}` cannot be stored in `{}`.", field.type_name, decl.name),
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

    fn check_ast_functions(&mut self) {
        let functions: Vec<SyntaxFunctionDecl> = self
            .syntax_program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Function(function) => Some(function.clone()),
                Item::Type(_) => None,
            })
            .collect();

        for function in functions {
            let mut state = BodyState::default();
            self.check_ast_block(&function, &function.body, &mut state);
        }
    }

    fn check_ast_block(
        &mut self,
        function: &SyntaxFunctionDecl,
        block: &SyntaxBlock,
        state: &mut BodyState,
    ) {
        for statement in &block.statements {
            self.check_moved_uses_in_stmt(statement, state);
            self.check_stmt_semantics(function, statement, state);
            self.apply_stmt_effects(statement, state);
        }
    }

    fn check_stmt_semantics(
        &mut self,
        function: &SyntaxFunctionDecl,
        statement: &Stmt,
        state: &mut BodyState,
    ) {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.kind == LetKind::Local
                    && let Some(Expr::Ident(name, span)) = &stmt.value
                    && state.managed.contains(name)
                {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "RS0301",
                            format!(
                                "managed value cannot be converted to local binding `{}`.",
                                stmt.name
                            ),
                            span.clone(),
                            "managed value used as local",
                        )
                        .with_cause("RSScript has no managed -> local conversion.")
                        .with_fix(
                            "create_local",
                            "Create the value as `local` at its creation point.",
                            "manual",
                        ),
                    );
                }

                if stmt.kind == LetKind::Managed
                    && let Some(Expr::Closure { body, .. }) = &stmt.value
                {
                    self.check_managed_closure_captures(body, state);
                }

                if let Some(value) = &stmt.value {
                    self.check_take_of_handle_field(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    if function.returns_fresh {
                        self.check_fresh_return(function, value, state);
                    }
                    self.check_take_of_handle_field(value);
                }
            }
            Stmt::With(stmt) => {
                self.check_resource_escape_ast(&stmt.binding, &stmt.body);
                self.check_ast_block(function, &stmt.body, state);
            }
            Stmt::Expr(expr) => {
                self.check_take_of_handle_field(expr);
            }
            Stmt::Unknown(_) => {}
        }
    }

    fn apply_stmt_effects(&mut self, statement: &Stmt, state: &mut BodyState) {
        match statement {
            Stmt::Let(stmt) => {
                match stmt.kind {
                    LetKind::Managed => {
                        state.managed.insert(stmt.name.clone());
                    }
                    LetKind::Local => {
                        state.locals.insert(stmt.name.clone());
                        state.clean_locals.insert(stmt.name.clone());
                    }
                }
                if let Some(value) = &stmt.value {
                    self.apply_expr_effects(value, state);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.apply_expr_effects(value, state);
                }
            }
            Stmt::With(stmt) => {
                self.apply_expr_effects(&stmt.resource, state);
            }
            Stmt::Expr(expr) => self.apply_expr_effects(expr, state),
            Stmt::Unknown(_) => {}
        }
    }

    fn apply_expr_effects(&mut self, expr: &Expr, state: &mut BodyState) {
        match expr {
            Expr::Manage { value, span } => {
                if let Expr::Ident(name, _) = value.as_ref()
                    && state.locals.contains(name)
                {
                    state.moved.insert(name.clone(), span.clone());
                    state.clean_locals.remove(name);
                }
                self.apply_expr_effects(value, state);
            }
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                span,
            } => {
                if let Expr::Ident(name, _) = value.as_ref()
                    && state.locals.contains(name)
                {
                    state.moved.insert(name.clone(), span.clone());
                    state.clean_locals.remove(name);
                }
                self.apply_expr_effects(value, state);
            }
            Expr::Effect { value, .. } => self.apply_expr_effects(value, state),
            Expr::Call { callee, args, .. } => {
                self.apply_retention_effects(callee, args, state);
                for arg in args {
                    self.apply_expr_effects(&arg.value, state);
                }
            }
            Expr::Field { base, .. } => self.apply_expr_effects(base, state),
            Expr::Closure { .. }
            | Expr::Ident(_, _)
            | Expr::Number(_, _)
            | Expr::String(_, _)
            | Expr::Unknown(_) => {}
        }
    }

    fn apply_retention_effects(&self, callee: &Callee, args: &[CallArg], state: &mut BodyState) {
        let Some(signature) = self.resolve_callee(callee) else {
            return;
        };
        if signature.retained_params.is_empty() {
            return;
        }

        for arg in args {
            let Some(name) = &arg.name else {
                continue;
            };
            if !signature.retained_params.contains(name) {
                continue;
            }
            if let Expr::Effect {
                effect: DataEffect::Read,
                value,
                ..
            } = &arg.value
                && let Expr::Ident(name, _) = value.as_ref()
                && state.locals.contains(name)
            {
                state.clean_locals.remove(name);
            }
        }
    }

    fn check_moved_uses_in_stmt(&mut self, statement: &Stmt, state: &BodyState) {
        let mut uses = Vec::new();
        collect_stmt_idents(statement, &mut uses);
        for (name, span) in uses {
            if let Some(move_span) = state.moved.get(&name) {
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0401",
                        format!("`{name}` was moved into the managed runtime by `manage {name}`."),
                        span,
                        "used after manage",
                    )
                    .with_cause(format!(
                        "The move happened at {}:{}.",
                        move_span.line, move_span.column
                    ))
                    .with_fix(
                        "move_use_before_manage",
                        format!("Move this use before `manage {name}`."),
                        "manual",
                    ),
                );
            }
        }
    }

    fn check_managed_closure_captures(&mut self, body: &SyntaxBlock, state: &BodyState) {
        let mut uses = Vec::new();
        collect_block_idents(body, &mut uses);
        for (name, span) in uses {
            if state.locals.contains(&name) {
                self.diagnostics.push(
                    Diagnostic::error(
                        "RS0801",
                        format!("managed closure captures local value `{name}`."),
                        span,
                        "local captured here",
                    )
                    .with_cause("Closures bound with `let` are managed closures.")
                    .with_fix(
                        "use_local_closure",
                        "Bind the closure with `local` or use a noescape callback.",
                        "manual",
                    ),
                );
            }
        }
    }

    fn check_take_of_handle_field(&mut self, expr: &Expr) {
        match expr {
            Expr::Effect {
                effect: DataEffect::Take,
                value,
                span,
            } => {
                if let Expr::Field { name, .. } = value.as_ref()
                    && self.is_handle_field(name)
                {
                    self.diagnostics.push(
                        Diagnostic::error(
                            "RS0901",
                            format!("cannot `take` handle field `{name}`."),
                            span.clone(),
                            "take of handle field",
                        )
                        .with_cause("Handle fields are managed references and cannot be consumed as local inline values."),
                    );
                }
                self.check_take_of_handle_field(value);
            }
            Expr::Effect { value, .. } => self.check_take_of_handle_field(value),
            Expr::Manage { value, .. } => self.check_take_of_handle_field(value),
            Expr::Call { args, .. } => {
                for arg in args {
                    self.check_take_of_handle_field(&arg.value);
                }
            }
            Expr::Field { base, .. } => self.check_take_of_handle_field(base),
            Expr::Closure { body, .. } => {
                for statement in &body.statements {
                    self.check_take_of_handle_in_stmt(statement);
                }
            }
            Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
        }
    }

    fn check_take_of_handle_in_stmt(&mut self, statement: &Stmt) {
        match statement {
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_take_of_handle_field(value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.check_take_of_handle_field(value);
                }
            }
            Stmt::With(stmt) => {
                self.check_take_of_handle_field(&stmt.resource);
                for statement in &stmt.body.statements {
                    self.check_take_of_handle_in_stmt(statement);
                }
            }
            Stmt::Expr(expr) => self.check_take_of_handle_field(expr),
            Stmt::Unknown(_) => {}
        }
    }

    fn check_resource_escape_ast(&mut self, binding: &str, body: &SyntaxBlock) {
        for statement in &body.statements {
            match statement {
                Stmt::Return(stmt) => {
                    if let Some(Expr::Ident(name, span)) = &stmt.value
                        && name == binding
                    {
                        self.resource_escape_diagnostic(binding, span.clone());
                    }
                    if let Some(value) = &stmt.value {
                        self.check_resource_manage_escape(binding, value);
                    }
                }
                Stmt::Let(stmt) => {
                    if let Some(value) = &stmt.value {
                        self.check_resource_manage_escape(binding, value);
                    }
                }
                Stmt::Expr(expr) => self.check_resource_manage_escape(binding, expr),
                Stmt::With(stmt) => self.check_resource_escape_ast(binding, &stmt.body),
                Stmt::Unknown(_) => {}
            }
        }
    }

    fn check_resource_manage_escape(&mut self, binding: &str, expr: &Expr) {
        match expr {
            Expr::Manage { value, span } => {
                if let Expr::Ident(name, _) = value.as_ref()
                    && name == binding
                {
                    self.resource_escape_diagnostic(binding, span.clone());
                }
                self.check_resource_manage_escape(binding, value);
            }
            Expr::Effect { value, .. } => self.check_resource_manage_escape(binding, value),
            Expr::Call { args, .. } => {
                for arg in args {
                    self.check_resource_manage_escape(binding, &arg.value);
                }
            }
            Expr::Field { base, .. } => self.check_resource_manage_escape(binding, base),
            Expr::Closure { body, .. } => self.check_resource_escape_ast(binding, body),
            Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
        }
    }

    fn resource_escape_diagnostic(&mut self, binding: &str, span: crate::diagnostic::Span) {
        self.diagnostics.push(
            Diagnostic::error(
                "RS0702",
                format!("resource `{binding}` cannot escape its `with` block."),
                span,
                "resource escapes",
            )
            .with_cause("A `with` resource must be dropped when the block exits."),
        );
    }

    fn check_fresh_return(
        &mut self,
        function: &SyntaxFunctionDecl,
        value: &Expr,
        state: &BodyState,
    ) {
        match value {
            Expr::Ident(name, span) if state.managed.contains(name) => {
                self.fresh_return_diagnostic(function, name, span.clone());
            }
            Expr::Ident(name, span) if state.locals.contains(name) => {
                if !state.clean_locals.contains(name) {
                    self.fresh_return_diagnostic(function, name, span.clone());
                }
            }
            Expr::Ident(name, span)
                if !self
                    .program
                    .types
                    .get(name)
                    .is_some_and(|decl| decl.kind == TypeKind::Struct)
                    && !self
                        .hir
                        .resolve_function(None, name)
                        .is_some_and(|signature| signature.returns_fresh) =>
            {
                self.diagnostics.push(
                    Diagnostic::warning(
                        "RS0602",
                        format!("freshness of return value in `{}` could not be proven.", function.name),
                        span.clone(),
                        "freshness unknown",
                    )
                    .with_cause("This MVP checker only trusts clean locals, struct constructors, and known fresh functions."),
                );
            }
            Expr::Call { callee, span, .. } => {
                let constructor_is_struct = match callee {
                    Callee::Name(name) => self
                        .program
                        .types
                        .get(name)
                        .is_some_and(|decl| decl.kind == TypeKind::Struct),
                    Callee::Qualified { .. } => false,
                };
                let call_returns_fresh = self
                    .resolve_callee(callee)
                    .is_some_and(|signature| signature.returns_fresh);
                if !constructor_is_struct && !call_returns_fresh {
                    self.diagnostics.push(
                        Diagnostic::warning(
                            "RS0602",
                            format!("freshness of return value in `{}` could not be proven.", function.name),
                            span.clone(),
                            "freshness unknown",
                        )
                        .with_cause("This MVP checker only trusts clean locals, struct constructors, and known fresh functions."),
                    );
                }
            }
            Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
                self.check_fresh_return(function, value, state);
            }
            Expr::Field { span, .. } | Expr::Closure { span, .. } | Expr::Unknown(span) => {
                self.diagnostics.push(
                    Diagnostic::warning(
                        "RS0602",
                        format!("freshness of return value in `{}` could not be proven.", function.name),
                        span.clone(),
                        "freshness unknown",
                    )
                    .with_cause("This MVP checker only trusts clean locals, struct constructors, and known fresh functions."),
                );
            }
            Expr::Ident(_, _) => {}
            Expr::Number(_, _) | Expr::String(_, _) => {}
        }
    }

    fn fresh_return_diagnostic(
        &mut self,
        function: &SyntaxFunctionDecl,
        name: &str,
        span: crate::diagnostic::Span,
    ) {
        self.diagnostics.push(
            Diagnostic::error(
                "RS0601",
                format!(
                    "fresh function `{}` returns managed value `{name}`.",
                    function.name
                ),
                span,
                "aliased value returned",
            )
            .with_cause("A `fresh` return must be newly created or a clean local value.")
            .with_fix(
                "return_fresh_value",
                "Return a struct constructor, fresh call, or clean local binding.",
                "manual",
            ),
        );
    }

    fn is_handle_field(&self, field: &str) -> bool {
        self.program.types.values().any(|decl| {
            decl.fields
                .iter()
                .any(|decl_field| decl_field.name == field && decl_field.is_handle)
        })
    }

    pub(crate) fn resolve_callee(&self, callee: &Callee) -> Option<&FunctionSig> {
        match callee {
            Callee::Name(name) => self.hir.resolve_function(None, name),
            Callee::Qualified { namespace, name } => {
                self.hir.resolve_function(Some(namespace), name)
            }
        }
    }
}

#[derive(Debug, Default)]
struct BodyState {
    locals: HashSet<String>,
    clean_locals: HashSet<String>,
    managed: HashSet<String>,
    moved: HashMap<String, crate::diagnostic::Span>,
}

fn collect_stmt_idents(statement: &Stmt, uses: &mut Vec<(String, crate::diagnostic::Span)>) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_idents(value, uses);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_idents(value, uses);
            }
        }
        Stmt::With(stmt) => {
            collect_expr_idents(&stmt.resource, uses);
        }
        Stmt::Expr(expr) => collect_expr_idents(expr, uses),
        Stmt::Unknown(_) => {}
    }
}

fn collect_block_idents(block: &SyntaxBlock, uses: &mut Vec<(String, crate::diagnostic::Span)>) {
    for statement in &block.statements {
        collect_stmt_idents(statement, uses);
        if let Stmt::With(stmt) = statement {
            collect_block_idents(&stmt.body, uses);
        }
    }
}

fn collect_expr_idents(expr: &Expr, uses: &mut Vec<(String, crate::diagnostic::Span)>) {
    match expr {
        Expr::Ident(name, span) => uses.push((name.clone(), span.clone())),
        Expr::Field { base, .. } => collect_expr_idents(base, uses),
        Expr::Call { args, .. } => {
            for arg in args {
                collect_expr_idents(&arg.value, uses);
            }
        }
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            collect_expr_idents(value, uses);
        }
        Expr::Closure { body, .. } => collect_block_idents(body, uses),
        Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}
