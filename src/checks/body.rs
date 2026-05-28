use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::Diagnostic;
use crate::syntax::ast::{
    Block, CallArg, Callee, DataEffect, Expr, FunctionDecl, Item, LetKind, Stmt,
};

pub(crate) fn check(analyzer: &mut Analyzer<'_>) {
    let functions: Vec<FunctionDecl> = analyzer
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
        check_block(analyzer, &function, &function.body, &mut state);
    }
}

#[derive(Debug, Default)]
struct BodyState {
    locals: HashSet<String>,
    clean_locals: HashSet<String>,
    managed: HashSet<String>,
    moved: HashMap<String, crate::diagnostic::Span>,
}

fn check_block(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    block: &Block,
    state: &mut BodyState,
) {
    for statement in &block.statements {
        check_moved_uses_in_stmt(analyzer, statement, state);
        check_stmt_semantics(analyzer, function, statement, state);
        apply_stmt_effects(analyzer, statement, state);
    }
}

fn check_stmt_semantics(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    statement: &Stmt,
    state: &mut BodyState,
) {
    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local
                && let Some(Expr::Ident(name, span)) = &stmt.value
                && state.managed.contains(name)
            {
                analyzer.diagnostics.push(
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
                check_managed_closure_captures(analyzer, body, state);
            }

            if let Some(value) = &stmt.value {
                check_take_of_handle_field(analyzer, value);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                if function.returns_fresh {
                    check_fresh_return(analyzer, function, value, state);
                }
                check_take_of_handle_field(analyzer, value);
            }
        }
        Stmt::With(stmt) => {
            check_resource_escape(analyzer, &stmt.binding, &stmt.body);
            check_block(analyzer, function, &stmt.body, state);
        }
        Stmt::Expr(expr) => {
            check_take_of_handle_field(analyzer, expr);
        }
        Stmt::Unknown(_) => {}
    }
}

fn apply_stmt_effects(analyzer: &mut Analyzer<'_>, statement: &Stmt, state: &mut BodyState) {
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
                apply_expr_effects(analyzer, value, state);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                apply_expr_effects(analyzer, value, state);
            }
        }
        Stmt::With(stmt) => {
            apply_expr_effects(analyzer, &stmt.resource, state);
        }
        Stmt::Expr(expr) => apply_expr_effects(analyzer, expr, state),
        Stmt::Unknown(_) => {}
    }
}

fn apply_expr_effects(analyzer: &mut Analyzer<'_>, expr: &Expr, state: &mut BodyState) {
    match expr {
        Expr::Manage { value, span } => {
            if let Expr::Ident(name, _) = value.as_ref()
                && state.locals.contains(name)
            {
                state.moved.insert(name.clone(), span.clone());
                state.clean_locals.remove(name);
            }
            apply_expr_effects(analyzer, value, state);
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
            apply_expr_effects(analyzer, value, state);
        }
        Expr::Effect { value, .. } => apply_expr_effects(analyzer, value, state),
        Expr::Call { callee, args, .. } => {
            apply_retention_effects(analyzer, callee, args, state);
            for arg in args {
                apply_expr_effects(analyzer, &arg.value, state);
            }
        }
        Expr::Field { base, .. } => apply_expr_effects(analyzer, base, state),
        Expr::Closure { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn apply_retention_effects(
    analyzer: &Analyzer<'_>,
    callee: &Callee,
    args: &[CallArg],
    state: &mut BodyState,
) {
    let Some(signature) = analyzer.resolve_callee(callee) else {
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

fn check_moved_uses_in_stmt(analyzer: &mut Analyzer<'_>, statement: &Stmt, state: &BodyState) {
    let mut uses = Vec::new();
    collect_stmt_idents(statement, &mut uses);
    for (name, span) in uses {
        if let Some(move_span) = state.moved.get(&name) {
            analyzer.diagnostics.push(
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

fn check_managed_closure_captures(analyzer: &mut Analyzer<'_>, body: &Block, state: &BodyState) {
    let mut uses = Vec::new();
    collect_block_idents(body, &mut uses);
    for (name, span) in uses {
        if state.locals.contains(&name) {
            analyzer.diagnostics.push(
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

fn check_take_of_handle_field(analyzer: &mut Analyzer<'_>, expr: &Expr) {
    match expr {
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            span,
        } => {
            if let Expr::Field { name, .. } = value.as_ref()
                && is_handle_field(analyzer, name)
            {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        "RS0901",
                        format!("cannot `take` handle field `{name}`."),
                        span.clone(),
                        "take of handle field",
                    )
                    .with_cause("Handle fields are managed references and cannot be consumed as local inline values."),
                );
            }
            check_take_of_handle_field(analyzer, value);
        }
        Expr::Effect { value, .. } => check_take_of_handle_field(analyzer, value),
        Expr::Manage { value, .. } => check_take_of_handle_field(analyzer, value),
        Expr::Call { args, .. } => {
            for arg in args {
                check_take_of_handle_field(analyzer, &arg.value);
            }
        }
        Expr::Field { base, .. } => check_take_of_handle_field(analyzer, base),
        Expr::Closure { body, .. } => {
            for statement in &body.statements {
                check_take_of_handle_in_stmt(analyzer, statement);
            }
        }
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn check_take_of_handle_in_stmt(analyzer: &mut Analyzer<'_>, statement: &Stmt) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                check_take_of_handle_field(analyzer, value);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                check_take_of_handle_field(analyzer, value);
            }
        }
        Stmt::With(stmt) => {
            check_take_of_handle_field(analyzer, &stmt.resource);
            for statement in &stmt.body.statements {
                check_take_of_handle_in_stmt(analyzer, statement);
            }
        }
        Stmt::Expr(expr) => check_take_of_handle_field(analyzer, expr),
        Stmt::Unknown(_) => {}
    }
}

fn check_resource_escape(analyzer: &mut Analyzer<'_>, binding: &str, body: &Block) {
    for statement in &body.statements {
        match statement {
            Stmt::Return(stmt) => {
                if let Some(Expr::Ident(name, span)) = &stmt.value
                    && name == binding
                {
                    resource_escape_diagnostic(analyzer, binding, span.clone());
                }
                if let Some(value) = &stmt.value {
                    check_resource_manage_escape(analyzer, binding, value);
                }
            }
            Stmt::Let(stmt) => {
                if let Some(value) = &stmt.value {
                    check_resource_manage_escape(analyzer, binding, value);
                }
            }
            Stmt::Expr(expr) => check_resource_manage_escape(analyzer, binding, expr),
            Stmt::With(stmt) => check_resource_escape(analyzer, binding, &stmt.body),
            Stmt::Unknown(_) => {}
        }
    }
}

fn check_resource_manage_escape(analyzer: &mut Analyzer<'_>, binding: &str, expr: &Expr) {
    match expr {
        Expr::Manage { value, span } => {
            if let Expr::Ident(name, _) = value.as_ref()
                && name == binding
            {
                resource_escape_diagnostic(analyzer, binding, span.clone());
            }
            check_resource_manage_escape(analyzer, binding, value);
        }
        Expr::Effect { value, .. } => check_resource_manage_escape(analyzer, binding, value),
        Expr::Call { args, .. } => {
            for arg in args {
                check_resource_manage_escape(analyzer, binding, &arg.value);
            }
        }
        Expr::Field { base, .. } => check_resource_manage_escape(analyzer, binding, base),
        Expr::Closure { body, .. } => check_resource_escape(analyzer, binding, body),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn resource_escape_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
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
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    value: &Expr,
    state: &BodyState,
) {
    match value {
        Expr::Ident(name, span) if state.managed.contains(name) => {
            fresh_return_diagnostic(analyzer, function, name, span.clone());
        }
        Expr::Ident(name, span) if state.locals.contains(name) => {
            if !state.clean_locals.contains(name) {
                fresh_return_diagnostic(analyzer, function, name, span.clone());
            }
        }
        Expr::Ident(name, span)
            if !analyzer
                .program
                .types
                .get(name)
                .is_some_and(|decl| decl.kind == crate::ast::TypeKind::Struct)
                && !analyzer
                    .hir
                    .resolve_function(None, name)
                    .is_some_and(|signature| signature.returns_fresh) =>
        {
            analyzer.diagnostics.push(
                Diagnostic::warning(
                    "RS0602",
                    format!(
                        "freshness of return value in `{}` could not be proven.",
                        function.name
                    ),
                    span.clone(),
                    "freshness unknown",
                )
                .with_cause("This MVP checker only trusts clean locals, struct constructors, and known fresh functions."),
            );
        }
        Expr::Call { callee, span, .. } => {
            let constructor_is_struct = match callee {
                Callee::Name(name) => analyzer
                    .program
                    .types
                    .get(name)
                    .is_some_and(|decl| decl.kind == crate::ast::TypeKind::Struct),
                Callee::Qualified { .. } => false,
            };
            let call_returns_fresh = analyzer
                .resolve_callee(callee)
                .is_some_and(|signature| signature.returns_fresh);
            if !constructor_is_struct && !call_returns_fresh {
                analyzer.diagnostics.push(
                    Diagnostic::warning(
                        "RS0602",
                        format!(
                            "freshness of return value in `{}` could not be proven.",
                            function.name
                        ),
                        span.clone(),
                        "freshness unknown",
                    )
                    .with_cause("This MVP checker only trusts clean locals, struct constructors, and known fresh functions."),
                );
            }
        }
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            check_fresh_return(analyzer, function, value, state);
        }
        Expr::Field { span, .. } | Expr::Closure { span, .. } | Expr::Unknown(span) => {
            analyzer.diagnostics.push(
                Diagnostic::warning(
                    "RS0602",
                    format!(
                        "freshness of return value in `{}` could not be proven.",
                        function.name
                    ),
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
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    name: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
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

fn is_handle_field(analyzer: &Analyzer<'_>, field: &str) -> bool {
    analyzer.hir.is_handle_field_name(field)
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

fn collect_block_idents(block: &Block, uses: &mut Vec<(String, crate::diagnostic::Span)>) {
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
