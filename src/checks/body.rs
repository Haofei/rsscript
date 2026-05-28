use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::{CallResolution, HirReturnProof, HirTypeKind, ResolvedCalleeKind};
use crate::syntax::ast::{Block, Callee, DataEffect, Expr, FunctionDecl, Item, LetKind, Stmt};

use super::local::{BodyState, LocalAnalysis, merge_if_state, merge_loop_state};

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
        let local_analysis = LocalAnalysis::new(analyzer.hir.function_body(&function.name));
        let mut state = local_analysis.initial_state();
        check_block(
            analyzer,
            &local_analysis,
            &function,
            &function.body,
            &mut state,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flow {
    Fallthrough,
    Return,
    Break,
    Continue,
}

fn check_block(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    function: &FunctionDecl,
    block: &Block,
    state: &mut BodyState,
) -> Flow {
    for statement in &block.statements {
        check_moved_uses_in_stmt(analyzer, local_analysis, statement, state);
        let flow = check_stmt_semantics(analyzer, local_analysis, function, statement, state);
        apply_stmt_effects(analyzer, local_analysis, statement, state);
        if flow != Flow::Fallthrough {
            return flow;
        }
    }
    Flow::Fallthrough
}

fn check_stmt_semantics(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    function: &FunctionDecl,
    statement: &Stmt,
    state: &mut BodyState,
) -> Flow {
    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local
                && let Some(Expr::Ident(name, span)) = &stmt.value
                && state.is_managed(name)
            {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        code::MANAGED_TO_LOCAL,
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
                && let Some(Expr::Closure { body, span }) = &stmt.value
            {
                check_managed_closure_captures(analyzer, local_analysis, span, body, state);
            }

            if let Some(value) = &stmt.value {
                check_take_of_handle_field(analyzer, local_analysis, value, state);
            }
            Flow::Fallthrough
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                if function.returns_fresh {
                    check_fresh_return(analyzer, local_analysis, function, value, state);
                }
                check_take_of_handle_field(analyzer, local_analysis, value, state);
            }
            Flow::Return
        }
        Stmt::With(stmt) => {
            check_resource_escape(analyzer, local_analysis, &stmt.binding, &stmt.body);
            check_block(analyzer, local_analysis, function, &stmt.body, state)
        }
        Stmt::If(stmt) => {
            check_take_of_handle_field(analyzer, local_analysis, &stmt.condition, state);
            apply_expr_effects(local_analysis, &stmt.condition, state);

            let base_state = state.clone();
            let mut then_state = base_state.clone();
            let then_flow = check_block(
                analyzer,
                local_analysis,
                function,
                &stmt.then_body,
                &mut then_state,
            );

            let else_branch = stmt.else_body.as_ref().map(|else_body| {
                let mut else_state = base_state.clone();
                let else_flow = check_block(
                    analyzer,
                    local_analysis,
                    function,
                    else_body,
                    &mut else_state,
                );
                (else_state, else_flow)
            });

            merge_if_state(state, &base_state, then_state, then_flow, else_branch)
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                check_take_of_handle_field(analyzer, local_analysis, condition, state);
                apply_expr_effects(local_analysis, condition, state);
            }

            let base_state = state.clone();
            let mut body_state = base_state.clone();
            let body_flow = check_block(
                analyzer,
                local_analysis,
                function,
                &stmt.body,
                &mut body_state,
            );

            merge_loop_state(
                state,
                &base_state,
                body_state,
                body_flow,
                stmt.condition.is_some(),
            )
        }
        Stmt::Expr(expr) => {
            check_take_of_handle_field(analyzer, local_analysis, expr, state);
            Flow::Fallthrough
        }
        Stmt::Break(_) => Flow::Break,
        Stmt::Continue(_) => Flow::Continue,
        Stmt::Unknown(_) => Flow::Fallthrough,
    }
}

fn apply_stmt_effects(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    statement: &Stmt,
    state: &mut BodyState,
) {
    match statement {
        Stmt::Let(stmt) => {
            match stmt.kind {
                LetKind::Managed => {
                    state.bind_managed(stmt.name.clone());
                }
                LetKind::Local => {
                    state.bind_local(stmt.name.clone());
                }
            }
            if let Some(value) = &stmt.value {
                if let Some(type_name) = local_analysis
                    .binding_type(&stmt.span)
                    .map(str::to_string)
                    .or_else(|| infer_expr_type(analyzer, value, state))
                {
                    state.record_type(stmt.name.clone(), type_name);
                }
                apply_expr_effects(local_analysis, value, state);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                apply_expr_effects(local_analysis, value, state);
            }
        }
        Stmt::With(stmt) => {
            apply_expr_effects(local_analysis, &stmt.resource, state);
        }
        Stmt::If(_) => {}
        Stmt::Loop(_) => {}
        Stmt::Expr(expr) => apply_expr_effects(local_analysis, expr, state),
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Unknown(_) => {}
    }
}

fn apply_expr_effects(local_analysis: &LocalAnalysis, expr: &Expr, state: &mut BodyState) {
    match expr {
        Expr::Manage { value, span } => {
            local_analysis.apply_move_events(span, state);
            apply_expr_effects(local_analysis, value, state);
        }
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            span,
        } => {
            local_analysis.apply_move_events(span, state);
            apply_expr_effects(local_analysis, value, state);
        }
        Expr::Effect { value, .. } => apply_expr_effects(local_analysis, value, state),
        Expr::Call { args, span, .. } => {
            local_analysis.apply_retention_events(span, state);
            for arg in args {
                apply_expr_effects(local_analysis, &arg.value, state);
            }
        }
        Expr::Field { base, .. } => apply_expr_effects(local_analysis, base, state),
        Expr::Closure { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn check_moved_uses_in_stmt(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    statement: &Stmt,
    state: &BodyState,
) {
    let state = local_analysis
        .flow_entry_state(stmt_span(statement))
        .unwrap_or(state);
    let fallback_uses;
    let uses = if let Some(uses) = local_analysis.statement_ident_uses(stmt_span(statement)) {
        uses
    } else {
        fallback_uses = {
            let mut uses = Vec::new();
            collect_stmt_idents(statement, &mut uses);
            uses
        };
        fallback_uses.as_slice()
    };
    for (name, span) in uses {
        if let Some(move_span) = state.move_span(name) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::USE_AFTER_MANAGE,
                    format!("`{name}` was moved into the managed runtime by `manage {name}`."),
                    span.clone(),
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

fn check_managed_closure_captures(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    closure_span: &crate::diagnostic::Span,
    body: &Block,
    state: &BodyState,
) {
    let fallback_uses;
    let uses = if let Some(uses) = local_analysis.closure_ident_uses(closure_span) {
        uses
    } else {
        fallback_uses = {
            let mut uses = Vec::new();
            collect_block_idents(body, &mut uses);
            uses
        };
        fallback_uses.as_slice()
    };
    for (name, span) in uses {
        if state.is_local(name) {
            analyzer.diagnostics.push(
                Diagnostic::error(
                    code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE,
                    format!("managed closure captures local value `{name}`."),
                    span.clone(),
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

fn check_take_of_handle_field(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    expr: &Expr,
    state: &BodyState,
) {
    match expr {
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            span,
        } => {
            if let Expr::Field {
                base,
                name,
                span: field_span,
            } = value.as_ref()
                && is_handle_field(analyzer, local_analysis, state, base, name, field_span)
            {
                analyzer.diagnostics.push(
                    Diagnostic::error(
                        code::TAKE_HANDLE_FIELD,
                        format!("cannot `take` handle field `{name}`."),
                        span.clone(),
                        "take of handle field",
                    )
                    .with_cause("Handle fields are managed references and cannot be consumed as local inline values."),
                );
            }
            check_take_of_handle_field(analyzer, local_analysis, value, state);
        }
        Expr::Effect { value, .. } => {
            check_take_of_handle_field(analyzer, local_analysis, value, state);
        }
        Expr::Manage { value, .. } => {
            check_take_of_handle_field(analyzer, local_analysis, value, state);
        }
        Expr::Call { args, .. } => {
            for arg in args {
                check_take_of_handle_field(analyzer, local_analysis, &arg.value, state);
            }
        }
        Expr::Field { base, .. } => {
            check_take_of_handle_field(analyzer, local_analysis, base, state);
        }
        Expr::Closure { body, .. } => {
            for statement in &body.statements {
                check_take_of_handle_in_stmt(analyzer, local_analysis, statement, state);
            }
        }
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn check_take_of_handle_in_stmt(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    statement: &Stmt,
    state: &BodyState,
) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                check_take_of_handle_field(analyzer, local_analysis, value, state);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                check_take_of_handle_field(analyzer, local_analysis, value, state);
            }
        }
        Stmt::With(stmt) => {
            check_take_of_handle_field(analyzer, local_analysis, &stmt.resource, state);
            for statement in &stmt.body.statements {
                check_take_of_handle_in_stmt(analyzer, local_analysis, statement, state);
            }
        }
        Stmt::If(stmt) => {
            check_take_of_handle_field(analyzer, local_analysis, &stmt.condition, state);
            for statement in &stmt.then_body.statements {
                check_take_of_handle_in_stmt(analyzer, local_analysis, statement, state);
            }
            if let Some(else_body) = &stmt.else_body {
                for statement in &else_body.statements {
                    check_take_of_handle_in_stmt(analyzer, local_analysis, statement, state);
                }
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                check_take_of_handle_field(analyzer, local_analysis, condition, state);
            }
            for statement in &stmt.body.statements {
                check_take_of_handle_in_stmt(analyzer, local_analysis, statement, state);
            }
        }
        Stmt::Expr(expr) => check_take_of_handle_field(analyzer, local_analysis, expr, state),
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Unknown(_) => {}
    }
}

fn check_resource_escape(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    binding: &str,
    body: &Block,
) {
    for statement in &body.statements {
        match statement {
            Stmt::Return(stmt) => {
                if let Some(Expr::Ident(name, span)) = &stmt.value
                    && name == binding
                {
                    resource_escape_diagnostic(analyzer, binding, span.clone());
                }
                if let Some(value) = &stmt.value {
                    check_resource_escape_expr(analyzer, local_analysis, binding, value);
                }
            }
            Stmt::Let(stmt) => {
                if stmt.kind == LetKind::Managed
                    && let Some(Expr::Closure { body, span }) = &stmt.value
                    && block_mentions_ident(body, binding)
                {
                    resource_capture_diagnostic(analyzer, binding, span.clone());
                }
                if let Some(value) = &stmt.value {
                    check_resource_escape_expr(analyzer, local_analysis, binding, value);
                }
            }
            Stmt::Expr(expr) => check_resource_escape_expr(analyzer, local_analysis, binding, expr),
            Stmt::With(stmt) => {
                check_resource_escape(analyzer, local_analysis, binding, &stmt.body)
            }
            Stmt::If(stmt) => {
                check_resource_escape_expr(analyzer, local_analysis, binding, &stmt.condition);
                check_resource_escape(analyzer, local_analysis, binding, &stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    check_resource_escape(analyzer, local_analysis, binding, else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    check_resource_escape_expr(analyzer, local_analysis, binding, condition);
                }
                check_resource_escape(analyzer, local_analysis, binding, &stmt.body);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Unknown(_) => {}
        }
    }
}

fn check_resource_escape_expr(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    binding: &str,
    expr: &Expr,
) {
    match expr {
        Expr::Manage { value, span } => {
            if let Expr::Ident(name, _) = value.as_ref()
                && name == binding
            {
                resource_escape_diagnostic(analyzer, binding, span.clone());
            }
            check_resource_escape_expr(analyzer, local_analysis, binding, value);
        }
        Expr::Effect { value, .. } => {
            check_resource_escape_expr(analyzer, local_analysis, binding, value);
        }
        Expr::Call { args, span, .. } => {
            check_resource_retained_by_call(analyzer, local_analysis, binding, span);
            for arg in args {
                check_resource_escape_expr(analyzer, local_analysis, binding, &arg.value);
            }
        }
        Expr::Field { base, .. } => {
            check_resource_escape_expr(analyzer, local_analysis, binding, base);
        }
        Expr::Closure { body, .. } => {
            check_resource_escape(analyzer, local_analysis, binding, body)
        }
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn check_resource_retained_by_call(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    binding: &str,
    span: &crate::diagnostic::Span,
) {
    for escaping_span in local_analysis.retained_value_spans(span, binding) {
        resource_escape_diagnostic(analyzer, binding, escaping_span);
    }
}

fn block_mentions_ident(block: &Block, binding: &str) -> bool {
    let mut uses = Vec::new();
    collect_block_idents(block, &mut uses);
    uses.iter().any(|(name, _)| name == binding)
}

fn resource_escape_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_ESCAPE,
            format!("resource `{binding}` cannot escape its `with` block."),
            span,
            "resource escapes",
        )
        .with_cause("A `with` resource must be dropped when the block exits."),
    );
}

fn resource_capture_diagnostic(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::RESOURCE_ESCAPE,
            format!("resource `{binding}` cannot be captured by a managed closure."),
            span,
            "resource captured",
        )
        .with_cause("Managed closures may outlive the `with` block that owns the resource."),
    );
}

fn check_fresh_return(
    analyzer: &mut Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    function: &FunctionDecl,
    value: &Expr,
    state: &BodyState,
) {
    match value {
        Expr::Ident(name, span) if state.is_managed(name) => {
            fresh_return_diagnostic(analyzer, function, name, span.clone());
        }
        Expr::Ident(name, span) if state.is_local(name) => {
            if !state.is_clean_local(name) {
                fresh_return_diagnostic(analyzer, function, name, span.clone());
            }
        }
        Expr::Ident(name, span)
            if !is_struct_type(analyzer, name)
                && !analyzer
                    .hir
                    .resolve_function(None, name)
                    .is_some_and(|signature| signature.returns_fresh) =>
        {
            analyzer.diagnostics.push(
                Diagnostic::warning(
                    code::FRESHNESS_UNKNOWN,
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
            if !hir_return_proves_fresh_call(analyzer, local_analysis, callee, span) {
                analyzer.diagnostics.push(
                    Diagnostic::warning(
                        code::FRESHNESS_UNKNOWN,
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
            check_fresh_return(analyzer, local_analysis, function, value, state);
        }
        Expr::Field { span, .. } | Expr::Closure { span, .. } | Expr::Unknown(span) => {
            analyzer.diagnostics.push(
                Diagnostic::warning(
                    code::FRESHNESS_UNKNOWN,
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

fn hir_return_proves_fresh_call(
    analyzer: &Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    callee: &Callee,
    span: &crate::diagnostic::Span,
) -> bool {
    if let Some(return_proof) = local_analysis.return_proof(span) {
        return matches!(
            return_proof,
            HirReturnProof::StructConstructor | HirReturnProof::FreshCall
        );
    }

    let resolution = analyzer.resolve_call_site(callee, span);
    let constructor_is_struct = matches!(
        resolution,
        CallResolution::Resolved {
            kind: ResolvedCalleeKind::Constructor {
                type_kind: HirTypeKind::Struct
            },
            ..
        }
    );
    let call_returns_fresh = match resolution {
        CallResolution::Resolved { signature, .. } => signature.returns_fresh,
        CallResolution::EnumVariant | CallResolution::Unknown => false,
    };
    constructor_is_struct || call_returns_fresh
}

fn fresh_return_diagnostic(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    name: &str,
    span: crate::diagnostic::Span,
) {
    analyzer.diagnostics.push(
        Diagnostic::error(
            code::FRESH_RETURN_NOT_CLEAN,
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

fn is_struct_type(analyzer: &Analyzer<'_>, name: &str) -> bool {
    analyzer.hir.type_kind(name) == Some(crate::hir::HirTypeKind::Struct)
}

fn infer_expr_type(analyzer: &Analyzer<'_>, expr: &Expr, state: &BodyState) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => state.value_type(name).map(str::to_string),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            infer_expr_type(analyzer, value, state)
        }
        Expr::Call { callee, span, .. } => infer_call_type(analyzer, callee, span),
        Expr::Field { base, name, .. } => {
            let base_type = infer_expr_type(analyzer, base, state)?;
            let field = analyzer.hir.type_info(&base_type)?.fields.get(name)?;
            Some(field.type_name.clone())
        }
        Expr::Closure { .. } | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => None,
    }
}

fn infer_call_type(
    analyzer: &Analyzer<'_>,
    callee: &Callee,
    span: &crate::diagnostic::Span,
) -> Option<String> {
    match analyzer.resolve_call_site(callee, span) {
        CallResolution::Resolved { signature, .. } => signature.return_type,
        CallResolution::EnumVariant | CallResolution::Unknown => None,
    }
}

fn is_handle_field(
    analyzer: &Analyzer<'_>,
    local_analysis: &LocalAnalysis,
    state: &BodyState,
    base: &Expr,
    field_name: &str,
    field_span: &crate::diagnostic::Span,
) -> bool {
    if let Some(field) = local_analysis.field_access(field_span) {
        return field.is_handle;
    }

    if let Some(base_type) = infer_expr_type(analyzer, base, state) {
        return analyzer
            .hir
            .type_info(&base_type)
            .and_then(|type_info| type_info.fields.get(field_name))
            .is_some_and(|field| field.is_handle);
    }

    !matches!(base, Expr::Ident(_, _)) && analyzer.hir.is_handle_field_name(field_name)
}

fn stmt_span(statement: &Stmt) -> &crate::diagnostic::Span {
    match statement {
        Stmt::Let(stmt) => &stmt.span,
        Stmt::Return(stmt) => &stmt.span,
        Stmt::With(stmt) => &stmt.span,
        Stmt::If(stmt) => &stmt.span,
        Stmt::Loop(stmt) => &stmt.span,
        Stmt::Break(span) | Stmt::Continue(span) | Stmt::Unknown(span) => span,
        Stmt::Expr(expr) => expr.span(),
    }
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
        Stmt::If(stmt) => collect_expr_idents(&stmt.condition, uses),
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_expr_idents(condition, uses);
            }
        }
        Stmt::Expr(expr) => collect_expr_idents(expr, uses),
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Unknown(_) => {}
    }
}

fn collect_block_idents(block: &Block, uses: &mut Vec<(String, crate::diagnostic::Span)>) {
    for statement in &block.statements {
        collect_stmt_idents(statement, uses);
        match statement {
            Stmt::With(stmt) => collect_block_idents(&stmt.body, uses),
            Stmt::If(stmt) => {
                collect_block_idents(&stmt.then_body, uses);
                if let Some(else_body) = &stmt.else_body {
                    collect_block_idents(else_body, uses);
                }
            }
            Stmt::Loop(stmt) => collect_block_idents(&stmt.body, uses),
            Stmt::Let(_)
            | Stmt::Return(_)
            | Stmt::Expr(_)
            | Stmt::Break(_)
            | Stmt::Continue(_)
            | Stmt::Unknown(_) => {}
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
