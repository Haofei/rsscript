use std::collections::{HashMap, HashSet};

use crate::analyzer::Analyzer;
use crate::diagnostic::{Diagnostic, code};
use crate::hir::{CallResolution, HirBindingKind, HirTypeKind, ResolvedCalleeKind};
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
        seed_function_bindings(analyzer, &function, &mut state);
        check_block(analyzer, &function, &function.body, &mut state);
    }
}

#[derive(Debug, Clone, Default)]
struct BodyState {
    locals: HashSet<String>,
    clean_locals: HashSet<String>,
    managed: HashSet<String>,
    moved: HashMap<String, crate::diagnostic::Span>,
    value_types: HashMap<String, String>,
}

fn seed_function_bindings(analyzer: &Analyzer<'_>, function: &FunctionDecl, state: &mut BodyState) {
    for binding in analyzer.hir.function_bindings(&function.name) {
        if binding.kind != HirBindingKind::Param {
            continue;
        }
        if let Some(type_name) = &binding.type_name {
            state
                .value_types
                .insert(binding.name.clone(), type_name.clone());
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    Fallthrough,
    Return,
    Break,
    Continue,
}

fn check_block(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    block: &Block,
    state: &mut BodyState,
) -> Flow {
    for statement in &block.statements {
        check_moved_uses_in_stmt(analyzer, statement, state);
        let flow = check_stmt_semantics(analyzer, function, statement, state);
        apply_stmt_effects(analyzer, statement, state);
        if flow != Flow::Fallthrough {
            return flow;
        }
    }
    Flow::Fallthrough
}

fn check_stmt_semantics(
    analyzer: &mut Analyzer<'_>,
    function: &FunctionDecl,
    statement: &Stmt,
    state: &mut BodyState,
) -> Flow {
    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local
                && let Some(Expr::Ident(name, span)) = &stmt.value
                && state.managed.contains(name)
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
                && let Some(Expr::Closure { body, .. }) = &stmt.value
            {
                check_managed_closure_captures(analyzer, body, state);
            }

            if let Some(value) = &stmt.value {
                check_take_of_handle_field(analyzer, value, state);
            }
            Flow::Fallthrough
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                if function.returns_fresh {
                    check_fresh_return(analyzer, function, value, state);
                }
                check_take_of_handle_field(analyzer, value, state);
            }
            Flow::Return
        }
        Stmt::With(stmt) => {
            check_resource_escape(analyzer, &stmt.binding, &stmt.body);
            check_block(analyzer, function, &stmt.body, state)
        }
        Stmt::If(stmt) => {
            check_take_of_handle_field(analyzer, &stmt.condition, state);
            apply_expr_effects(analyzer, &stmt.condition, state);

            let base_state = state.clone();
            let mut then_state = base_state.clone();
            let then_flow = check_block(analyzer, function, &stmt.then_body, &mut then_state);

            let else_branch = stmt.else_body.as_ref().map(|else_body| {
                let mut else_state = base_state.clone();
                let else_flow = check_block(analyzer, function, else_body, &mut else_state);
                (else_state, else_flow)
            });

            merge_if_state(state, &base_state, then_state, then_flow, else_branch)
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                check_take_of_handle_field(analyzer, condition, state);
                apply_expr_effects(analyzer, condition, state);
            }

            let base_state = state.clone();
            let mut body_state = base_state.clone();
            let body_flow = check_block(analyzer, function, &stmt.body, &mut body_state);

            merge_loop_state(
                state,
                &base_state,
                body_state,
                body_flow,
                stmt.condition.is_some(),
            )
        }
        Stmt::Expr(expr) => {
            check_take_of_handle_field(analyzer, expr, state);
            Flow::Fallthrough
        }
        Stmt::Break(_) => Flow::Break,
        Stmt::Continue(_) => Flow::Continue,
        Stmt::Unknown(_) => Flow::Fallthrough,
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
                if let Some(type_name) = hir_binding_type(analyzer, &stmt.span)
                    .or_else(|| infer_expr_type(analyzer, value, state))
                {
                    state.value_types.insert(stmt.name.clone(), type_name);
                }
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
        Stmt::If(_) => {}
        Stmt::Loop(_) => {}
        Stmt::Expr(expr) => apply_expr_effects(analyzer, expr, state),
        Stmt::Break(_) | Stmt::Continue(_) => {}
        Stmt::Unknown(_) => {}
    }
}

fn hir_binding_type(analyzer: &Analyzer<'_>, span: &crate::diagnostic::Span) -> Option<String> {
    analyzer
        .hir
        .binding(span)
        .and_then(|binding| binding.type_name.clone())
}

fn merge_if_state(
    state: &mut BodyState,
    base: &BodyState,
    then_state: BodyState,
    then_flow: Flow,
    else_branch: Option<(BodyState, Flow)>,
) -> Flow {
    let (else_state, else_flow) = else_branch.unwrap_or_else(|| (base.clone(), Flow::Fallthrough));
    let mut fallthrough_states = Vec::new();
    if then_flow == Flow::Fallthrough {
        fallthrough_states.push(then_state.clone());
    }
    if else_flow == Flow::Fallthrough {
        fallthrough_states.push(else_state.clone());
    }

    match fallthrough_states.as_slice() {
        [] => {
            *state = base.clone();
            merge_non_fallthrough(then_flow, else_flow)
        }
        [only] => {
            *state = fallthrough_projection(base, only);
            Flow::Fallthrough
        }
        [left, right] => {
            *state = merge_fallthrough_states(base, left, right);
            Flow::Fallthrough
        }
        _ => unreachable!("if has at most two branches"),
    }
}

fn merge_loop_state(
    state: &mut BodyState,
    base: &BodyState,
    body_state: BodyState,
    body_flow: Flow,
    may_skip: bool,
) -> Flow {
    if !may_skip && body_flow == Flow::Return {
        *state = base.clone();
        return Flow::Return;
    }
    if !may_skip && body_flow == Flow::Break {
        *state = fallthrough_projection(base, &body_state);
        return Flow::Fallthrough;
    }

    let mut moved = base.moved.clone();
    if body_flow != Flow::Return {
        for (name, span) in &body_state.moved {
            if base.locals.contains(name) || base.moved.contains_key(name) {
                moved.entry(name.clone()).or_insert_with(|| span.clone());
            }
        }
    }

    state.locals = base.locals.clone();
    state.managed = base.managed.clone();
    state.value_types = base.value_types.clone();
    state.moved = moved;
    state.clean_locals = base
        .clean_locals
        .intersection(&body_state.clean_locals)
        .filter(|name| base.locals.contains(*name))
        .cloned()
        .collect();
    Flow::Fallthrough
}

fn merge_non_fallthrough(left: Flow, right: Flow) -> Flow {
    if left == right { left } else { Flow::Return }
}

fn fallthrough_projection(base: &BodyState, branch: &BodyState) -> BodyState {
    let mut moved = base.moved.clone();
    for (name, span) in &branch.moved {
        if base.locals.contains(name) || base.moved.contains_key(name) {
            moved.entry(name.clone()).or_insert_with(|| span.clone());
        }
    }

    BodyState {
        locals: base.locals.clone(),
        managed: base.managed.clone(),
        value_types: base.value_types.clone(),
        moved,
        clean_locals: branch
            .clean_locals
            .intersection(&base.clean_locals)
            .filter(|name| base.locals.contains(*name))
            .cloned()
            .collect(),
    }
}

fn merge_fallthrough_states(base: &BodyState, left: &BodyState, right: &BodyState) -> BodyState {
    let mut moved = base.moved.clone();
    for branch in [left, right] {
        for (name, span) in &branch.moved {
            if base.locals.contains(name) || base.moved.contains_key(name) {
                moved.entry(name.clone()).or_insert_with(|| span.clone());
            }
        }
    }

    BodyState {
        locals: base.locals.clone(),
        managed: base.managed.clone(),
        value_types: base.value_types.clone(),
        moved,
        clean_locals: left
            .clean_locals
            .intersection(&right.clean_locals)
            .filter(|name| base.locals.contains(*name))
            .cloned()
            .collect(),
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
        Expr::Call {
            callee, args, span, ..
        } => {
            apply_retention_effects(analyzer, callee, span, args, state);
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
    span: &crate::diagnostic::Span,
    args: &[CallArg],
    state: &mut BodyState,
) {
    let retained_params = match analyzer.resolve_call_site(callee, span) {
        CallResolution::Resolved { signature, .. } => signature.retained_params,
        CallResolution::EnumVariant | CallResolution::Unknown => return,
    };
    if retained_params.is_empty() {
        return;
    }

    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        if !retained_params.contains(name) {
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
                    code::USE_AFTER_MANAGE,
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
                    code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE,
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

fn check_take_of_handle_field(analyzer: &mut Analyzer<'_>, expr: &Expr, state: &BodyState) {
    match expr {
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            span,
        } => {
            if let Expr::Field { base, name, .. } = value.as_ref()
                && is_handle_field(analyzer, state, base, name)
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
            check_take_of_handle_field(analyzer, value, state);
        }
        Expr::Effect { value, .. } => check_take_of_handle_field(analyzer, value, state),
        Expr::Manage { value, .. } => check_take_of_handle_field(analyzer, value, state),
        Expr::Call { args, .. } => {
            for arg in args {
                check_take_of_handle_field(analyzer, &arg.value, state);
            }
        }
        Expr::Field { base, .. } => check_take_of_handle_field(analyzer, base, state),
        Expr::Closure { body, .. } => {
            for statement in &body.statements {
                check_take_of_handle_in_stmt(analyzer, statement, state);
            }
        }
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn check_take_of_handle_in_stmt(analyzer: &mut Analyzer<'_>, statement: &Stmt, state: &BodyState) {
    match statement {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                check_take_of_handle_field(analyzer, value, state);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                check_take_of_handle_field(analyzer, value, state);
            }
        }
        Stmt::With(stmt) => {
            check_take_of_handle_field(analyzer, &stmt.resource, state);
            for statement in &stmt.body.statements {
                check_take_of_handle_in_stmt(analyzer, statement, state);
            }
        }
        Stmt::If(stmt) => {
            check_take_of_handle_field(analyzer, &stmt.condition, state);
            for statement in &stmt.then_body.statements {
                check_take_of_handle_in_stmt(analyzer, statement, state);
            }
            if let Some(else_body) = &stmt.else_body {
                for statement in &else_body.statements {
                    check_take_of_handle_in_stmt(analyzer, statement, state);
                }
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                check_take_of_handle_field(analyzer, condition, state);
            }
            for statement in &stmt.body.statements {
                check_take_of_handle_in_stmt(analyzer, statement, state);
            }
        }
        Stmt::Expr(expr) => check_take_of_handle_field(analyzer, expr, state),
        Stmt::Break(_) | Stmt::Continue(_) => {}
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
                    check_resource_escape_expr(analyzer, binding, value);
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
                    check_resource_escape_expr(analyzer, binding, value);
                }
            }
            Stmt::Expr(expr) => check_resource_escape_expr(analyzer, binding, expr),
            Stmt::With(stmt) => check_resource_escape(analyzer, binding, &stmt.body),
            Stmt::If(stmt) => {
                check_resource_escape_expr(analyzer, binding, &stmt.condition);
                check_resource_escape(analyzer, binding, &stmt.then_body);
                if let Some(else_body) = &stmt.else_body {
                    check_resource_escape(analyzer, binding, else_body);
                }
            }
            Stmt::Loop(stmt) => {
                if let Some(condition) = &stmt.condition {
                    check_resource_escape_expr(analyzer, binding, condition);
                }
                check_resource_escape(analyzer, binding, &stmt.body);
            }
            Stmt::Break(_) | Stmt::Continue(_) => {}
            Stmt::Unknown(_) => {}
        }
    }
}

fn check_resource_escape_expr(analyzer: &mut Analyzer<'_>, binding: &str, expr: &Expr) {
    match expr {
        Expr::Manage { value, span } => {
            if let Expr::Ident(name, _) = value.as_ref()
                && name == binding
            {
                resource_escape_diagnostic(analyzer, binding, span.clone());
            }
            check_resource_escape_expr(analyzer, binding, value);
        }
        Expr::Effect { value, .. } => check_resource_escape_expr(analyzer, binding, value),
        Expr::Call {
            callee, args, span, ..
        } => {
            check_resource_retained_by_call(analyzer, binding, callee, span, args);
            for arg in args {
                check_resource_escape_expr(analyzer, binding, &arg.value);
            }
        }
        Expr::Field { base, .. } => check_resource_escape_expr(analyzer, binding, base),
        Expr::Closure { body, .. } => check_resource_escape(analyzer, binding, body),
        Expr::Ident(_, _) | Expr::Number(_, _) | Expr::String(_, _) | Expr::Unknown(_) => {}
    }
}

fn check_resource_retained_by_call(
    analyzer: &mut Analyzer<'_>,
    binding: &str,
    callee: &Callee,
    span: &crate::diagnostic::Span,
    args: &[CallArg],
) {
    let retained_params = match analyzer.resolve_call_site(callee, span) {
        CallResolution::Resolved { signature, .. } => signature.retained_params,
        CallResolution::EnumVariant | CallResolution::Unknown => return,
    };
    if retained_params.is_empty() {
        return;
    }

    for arg in args {
        let Some(name) = &arg.name else {
            continue;
        };
        if retained_params.contains(name) && expr_is_binding_value(&arg.value, binding) {
            resource_escape_diagnostic(analyzer, binding, arg.value.span().clone());
        }
    }
}

fn expr_is_binding_value(expr: &Expr, binding: &str) -> bool {
    match expr {
        Expr::Ident(name, _) => name == binding,
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            expr_is_binding_value(value, binding)
        }
        Expr::Field { base, .. } => expr_is_binding_value(base, binding),
        Expr::Call { .. }
        | Expr::Closure { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::Unknown(_) => false,
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
            if !constructor_is_struct && !call_returns_fresh {
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
            check_fresh_return(analyzer, function, value, state);
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
        Expr::Ident(name, _) => state.value_types.get(name).cloned(),
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
    state: &BodyState,
    base: &Expr,
    field_name: &str,
) -> bool {
    if let Some(base_type) = infer_expr_type(analyzer, base, state) {
        return analyzer
            .hir
            .type_info(&base_type)
            .and_then(|type_info| type_info.fields.get(field_name))
            .is_some_and(|field| field.is_handle);
    }

    !matches!(base, Expr::Ident(_, _)) && analyzer.hir.is_handle_field_name(field_name)
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
