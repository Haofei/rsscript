use std::collections::HashSet;

use crate::ast::{Program, TypeKind, parse_program};
use crate::checks;
use crate::diagnostic::Diagnostic;
use crate::hir::{FunctionSig, Hir};
use crate::lexer::{Token, lex};
use crate::syntax::ast::Callee;
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
        checks::body::check(self);
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

    pub(crate) fn resolve_callee(&self, callee: &Callee) -> Option<&FunctionSig> {
        match callee {
            Callee::Name(name) => self.hir.resolve_function(None, name),
            Callee::Qualified { namespace, name } => {
                self.hir.resolve_function(Some(namespace), name)
            }
        }
    }
}
