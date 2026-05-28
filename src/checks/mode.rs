use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::syntax::ast::{
    DataEffect, Expr, FileMode as SyntaxFileMode, Item, LetKind, Stmt, TypeRef,
};

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    let Some(mode) = analyzer.syntax_program.mode else {
        return;
    };
    if mode != SyntaxFileMode::Managed {
        return;
    }

    let items = analyzer.syntax_program.items.clone();
    for item in &items {
        match item {
            Item::Function(function) => {
                check_block(analyzer, &function.body);
                for param in &function.params {
                    if param.effect == Some(DataEffect::Take) {
                        mode_violation(
                            analyzer,
                            "`take` requires `mode: uses-local`.",
                            param.span.clone(),
                        );
                    }
                    check_type_ref(analyzer, &param.ty);
                }
                if let Some(return_ty) = &function.return_ty {
                    check_type_ref(analyzer, return_ty);
                }
            }
            Item::Type(decl) => {
                for field in &decl.fields {
                    check_type_ref(analyzer, &field.ty);
                }
            }
        }
    }
}

fn check_block(analyzer: &mut Analyzer<'_>, block: &crate::syntax::ast::Block) {
    for statement in &block.statements {
        match statement {
            Stmt::Let(stmt) => {
                if stmt.kind == LetKind::Local {
                    mode_violation(
                        analyzer,
                        "`local` requires `mode: uses-local`.",
                        stmt.span.clone(),
                    );
                }
                if let Some(value) = &stmt.value {
                    check_expr(analyzer, value);
                }
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    check_expr(analyzer, value);
                }
            }
            Stmt::With(stmt) => {
                check_expr(analyzer, &stmt.resource);
                check_block(analyzer, &stmt.body);
            }
            Stmt::If(stmt) => {
                check_expr(analyzer, &stmt.condition);
                check_block(analyzer, &stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    check_block(analyzer, else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    check_expr(analyzer, condition);
                }
                check_block(analyzer, &stmt.body);
            }
            Stmt::Expr(expr) => check_expr(analyzer, expr),
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Unknown(_) => {}
        }
    }
}

fn check_expr(analyzer: &mut Analyzer<'_>, expr: &Expr) {
    match expr {
        Expr::Manage { value, span } => {
            mode_violation(
                analyzer,
                "`manage` requires `mode: uses-local`.",
                span.clone(),
            );
            check_expr(analyzer, value);
        }
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            span,
        } => {
            mode_violation(
                analyzer,
                "`take` requires `mode: uses-local`.",
                span.clone(),
            );
            check_expr(analyzer, value);
        }
        Expr::Effect { value, .. } => check_expr(analyzer, value),
        Expr::Call { callee, args, .. } => {
            if matches!(callee, crate::syntax::ast::Callee::Qualified { namespace, .. } if namespace == "ResourcePool")
                || matches!(callee, crate::syntax::ast::Callee::Name(name) if name == "ResourcePool")
            {
                mode_violation(
                    analyzer,
                    "`ResourcePool<T>` requires `mode: uses-local`.",
                    expr.span().clone(),
                );
            }
            for arg in args {
                check_expr(analyzer, &arg.value);
            }
        }
        Expr::Field { base, .. } => check_expr(analyzer, base),
        Expr::Closure { body, .. } => check_block(analyzer, body),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn check_type_ref(analyzer: &mut Analyzer<'_>, ty: &TypeRef) {
    if ty.name == "ResourcePool" {
        mode_violation(
            analyzer,
            "`ResourcePool<T>` requires `mode: uses-local`.",
            ty.span.clone(),
        );
    }
    for arg in &ty.args {
        check_type_ref(analyzer, arg);
    }
}

fn mode_violation(analyzer: &mut Analyzer<'_>, summary: &str, span: crate::diagnostic::Span) {
    analyzer.diagnostics.push(
        Diagnostic::error(code::FILE_MODE_VIOLATION, summary, span, "mode violation").with_fix(
            "change_mode",
            "Change the file declaration to `mode: uses-local`.",
            "manual",
        ),
    );
}
