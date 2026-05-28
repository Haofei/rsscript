use crate::checks;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::{FunctionSig, Hir, HirTypeKind};
use crate::lexer::{Token, lex};
use crate::syntax::ast::{Callee, EffectDecl, Item};
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
        self.check_signature_explicitness();
        self.check_resource_fields();
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

    pub(crate) fn resolve_callee(&self, callee: &Callee) -> Option<&FunctionSig> {
        match callee {
            Callee::Name(name) => self.hir.resolve_function(None, name),
            Callee::Qualified { namespace, name } => {
                self.hir.resolve_function(Some(namespace), name)
            }
        }
    }
}

fn effect_name(effect: &EffectDecl) -> &str {
    match effect {
        EffectDecl::Name(name) | EffectDecl::Retains(name) => name,
    }
}
