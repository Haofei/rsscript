//! Local move, freshness, retained capture, and handle-field ownership analysis.

use super::*;

pub(super) fn initial_state_from_body(body: Option<&HirFunctionBody>) -> BodyState {
    let mut state = BodyState::default();
    if let Some(body) = body {
        state.seed_params(&body.bindings);
    }
    state
}

pub(super) fn collect_take_handle_fields(block: &HirBlock) -> Vec<TakeHandleField> {
    let mut fields = Vec::new();
    collect_block_take_handle_fields(block, &mut fields);
    fields
}

pub(super) fn collect_block_take_handle_fields(
    block: &HirBlock,
    fields: &mut Vec<TakeHandleField>,
) {
    for statement in &block.statements {
        collect_stmt_take_handle_fields(statement, fields);
    }
}

pub(super) fn collect_stmt_take_handle_fields(
    statement: &HirStmt,
    fields: &mut Vec<TakeHandleField>,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value)
        | HirStmt::Assign { value, .. } => collect_expr_take_handle_fields(value, fields),
        HirStmt::With { resource, body, .. } => {
            collect_expr_take_handle_fields(resource, fields);
            collect_block_take_handle_fields(body, fields);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_expr_take_handle_fields(condition, fields);
            collect_block_take_handle_fields(then_body, fields);
            if let Some(else_body) = else_body {
                collect_block_take_handle_fields(else_body, fields);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_expr_take_handle_fields(condition, fields);
            }
            collect_block_take_handle_fields(body, fields);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_expr_take_handle_fields(iterable, fields);
            collect_block_take_handle_fields(body, fields);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_expr_take_handle_fields(value, fields);
            for arm in arms {
                collect_block_take_handle_fields(&arm.body, fields);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_expr_take_handle_fields(&arm.operation, fields);
                collect_block_take_handle_fields(&arm.body, fields);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_expr_take_handle_fields(expr: &HirExpr, fields: &mut Vec<TakeHandleField>) {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Take,
            value,
            span,
            ..
        } => {
            if let HirExpr::Field { name, access, .. } = value.as_ref()
                && access.is_handle
            {
                push_take_handle_field(fields, name.clone(), span.clone());
            }
            collect_expr_take_handle_fields(value, fields);
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_expr_take_handle_fields(value, fields);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_expr_take_handle_fields(left, fields);
            collect_expr_take_handle_fields(right, fields);
        }
        HirExpr::Field { base, .. } => collect_expr_take_handle_fields(base, fields),
        HirExpr::Index { base, index, .. } => {
            collect_expr_take_handle_fields(base, fields);
            collect_expr_take_handle_fields(index, fields);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_expr_take_handle_fields(&arg.value, fields);
            }
        }
        HirExpr::Closure { body, .. } => collect_block_take_handle_fields(body, fields),
        HirExpr::Match { value, arms, .. } => {
            collect_expr_take_handle_fields(value, fields);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_expr_take_handle_fields(guard, fields);
                }
                collect_block_take_handle_fields(&arm.body, fields);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_expr_take_handle_fields(&entry.key, fields);
                collect_expr_take_handle_fields(&entry.value, fields);
            }
        }
        HirExpr::ObjectLiteral {
            fields: lit_fields, ..
        } => {
            for field in lit_fields {
                collect_expr_take_handle_fields(&field.value, fields);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_expr_take_handle_fields(item, fields);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn push_take_handle_field(fields: &mut Vec<TakeHandleField>, name: String, span: Span) {
    let field = TakeHandleField { name, span };
    if !fields.contains(&field) {
        fields.push(field);
    }
}

pub(super) fn collect_fresh_return_issues_from_block(
    block: &HirBlock,
    entry_states: &HashMap<Span, BodyState>,
    issues: &mut Vec<FreshReturnIssue>,
) {
    for statement in &block.statements {
        collect_fresh_return_issues_from_stmt(statement, entry_states, issues);
    }
}

pub(super) fn collect_fresh_return_issues_from_stmt(
    statement: &HirStmt,
    entry_states: &HashMap<Span, BodyState>,
    issues: &mut Vec<FreshReturnIssue>,
) {
    match statement {
        HirStmt::Return { value, proof, span } => {
            collect_fresh_return_issue(value.as_ref(), proof, span, entry_states, issues);
        }
        HirStmt::With { body, .. } => {
            collect_fresh_return_issues_from_block(body, entry_states, issues);
        }
        HirStmt::If {
            then_body,
            else_body,
            ..
        } => {
            collect_fresh_return_issues_from_block(then_body, entry_states, issues);
            if let Some(else_body) = else_body {
                collect_fresh_return_issues_from_block(else_body, entry_states, issues);
            }
        }
        HirStmt::Loop { body, .. } => {
            collect_fresh_return_issues_from_block(body, entry_states, issues);
        }
        HirStmt::For { body, .. } => {
            collect_fresh_return_issues_from_block(body, entry_states, issues);
        }
        HirStmt::Match { arms, .. } => {
            for arm in arms {
                collect_fresh_return_issues_from_block(&arm.body, entry_states, issues);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_fresh_return_issues_from_block(&arm.body, entry_states, issues);
            }
        }
        HirStmt::Let { .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Expr(_)
        | HirStmt::Assign { .. }
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_fresh_return_issue(
    value: Option<&HirExpr>,
    proof: &HirReturnProof,
    return_span: &Span,
    entry_states: &HashMap<Span, BodyState>,
    issues: &mut Vec<FreshReturnIssue>,
) {
    match proof {
        HirReturnProof::Ident { name } => {
            let span = fresh_return_value_span(value)
                .unwrap_or(return_span)
                .clone();
            if let Some(state) = entry_states.get(return_span) {
                // A binding bound from a fresh value (a `let s = <fresh source>`)
                // stays returnable-as-fresh until it is moved, retained, or
                // captured — including a *managed* `let`, not only an exclusive
                // `local`. Those invalidations clear the fresh-returnable flag, so
                // an aliased binding falls back to NotClean.
                let returns_fresh =
                    state.is_clean_local(name) && state.is_fresh_returnable_local(name);
                if state.is_managed(name) || state.is_local(name) {
                    if returns_fresh {
                        return;
                    }
                    push_fresh_return_issue(
                        issues,
                        FreshReturnIssueKind::NotClean { name: name.clone() },
                        span,
                    );
                    return;
                }
            }
            push_fresh_return_issue(
                issues,
                FreshReturnIssueKind::UnknownIdent { name: name.clone() },
                span,
            );
        }
        HirReturnProof::Unknown => {
            if let Some(value) = value
                && let Some(path) = fresh_handle_or_weak_field_path(value)
            {
                push_fresh_return_issue(
                    issues,
                    FreshReturnIssueKind::NotClean { name: path },
                    fresh_return_value_span(Some(value))
                        .unwrap_or(return_span)
                        .clone(),
                );
                return;
            }
            if let Some(value) = value
                && fresh_field_access_base(value).is_some_and(|name| {
                    entry_states.get(return_span).is_some_and(|state| {
                        state.is_local(name)
                            && state.is_clean_local(name)
                            && state.is_fresh_returnable_local(name)
                    })
                })
            {
                return;
            }
            push_fresh_return_issue(
                issues,
                FreshReturnIssueKind::Unknown,
                fresh_return_value_span(value)
                    .unwrap_or(return_span)
                    .clone(),
            );
        }
        HirReturnProof::NoValue
        | HirReturnProof::StructConstructor
        | HirReturnProof::FreshCall
        | HirReturnProof::Literal => {}
    }
}

pub(super) fn fresh_field_access_base(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Field { base, access, .. } if !access.is_handle && !access.is_weak => {
            fresh_field_access_base(base)
        }
        HirExpr::Ident { name, .. } => Some(name),
        HirExpr::Call { callee, args, .. } if fresh_wrapper_callee(callee) => args
            .first()
            .and_then(|arg| fresh_field_access_base(&arg.value)),
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => fresh_field_access_base(value),
        HirExpr::Manage { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Match { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn fresh_handle_or_weak_field_path(expr: &HirExpr) -> Option<String> {
    match expr {
        HirExpr::Field {
            base, name, access, ..
        } if access.is_handle || access.is_weak => {
            let base = fresh_expr_path(base).unwrap_or_else(|| "<expr>".to_string());
            Some(format!("{base}.{name}"))
        }
        HirExpr::Field { base, .. } => fresh_handle_or_weak_field_path(base),
        HirExpr::Call { callee, args, .. } if fresh_wrapper_callee(callee) => args
            .first()
            .and_then(|arg| fresh_handle_or_weak_field_path(&arg.value)),
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => fresh_handle_or_weak_field_path(value),
        HirExpr::Ident { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Match { .. }
        | HirExpr::Manage { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn fresh_expr_path(expr: &HirExpr) -> Option<String> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name.clone()),
        HirExpr::Field { base, name, .. } => {
            fresh_expr_path(base).map(|base| format!("{base}.{name}"))
        }
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => fresh_expr_path(value),
        _ => None,
    }
}

pub(super) fn fresh_wrapper_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Name(name) if matches!(name.as_str(), "Ok" | "Some")
    )
}

pub(super) fn fresh_return_value_span(value: Option<&HirExpr>) -> Option<&Span> {
    let mut value = value?;
    loop {
        match value {
            HirExpr::Effect { value: inner, .. } | HirExpr::Manage { value: inner, .. } => {
                value = inner;
            }
            _ => return Some(hir_expr_span(value)),
        }
    }
}

pub(super) fn push_fresh_return_issue(
    issues: &mut Vec<FreshReturnIssue>,
    kind: FreshReturnIssueKind,
    span: Span,
) {
    let issue = FreshReturnIssue { kind, span };
    if !issues.contains(&issue) {
        issues.push(issue);
    }
}

pub(super) fn collect_retained_closure_captures_from_block(
    block: &HirBlock,
    entry_states: &HashMap<Span, BodyState>,
    captures: &mut Vec<RetainedClosureCapture>,
) {
    for statement in &block.statements {
        collect_retained_closure_captures_from_stmt(statement, entry_states, captures);
    }
}

pub(super) fn collect_ordered_moved_uses_from_block(
    block: &HirBlock,
    entry_states: &HashMap<Span, BodyState>,
    moved_uses: &mut Vec<MovedUse>,
) {
    for statement in &block.statements {
        collect_ordered_moved_uses_from_stmt(statement, entry_states, moved_uses);
    }
}

pub(super) fn collect_ordered_moved_uses_from_stmt(
    statement: &HirStmt,
    entry_states: &HashMap<Span, BodyState>,
    moved_uses: &mut Vec<MovedUse>,
) {
    let entry_state = entry_states.get(hir_stmt_span(statement)).cloned();
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(value, &mut state, moved_uses);
            }
        }
        HirStmt::Assign { target, value, .. } => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(value, &mut state, moved_uses);
                // The target is an evaluated place: a field/index base (and an
                // index expression) reads its operands, so a moved base/index is a
                // use-after-move. The write root itself is a def, not a use.
                for read in crate::hir::assign_target_reads(target) {
                    collect_ordered_moved_uses_from_expr(read, &mut state, moved_uses);
                }
            }
        }
        HirStmt::With { resource, body, .. } => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(resource, &mut state, moved_uses);
            }
            collect_ordered_moved_uses_from_block(body, entry_states, moved_uses);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(condition, &mut state, moved_uses);
            }
            collect_ordered_moved_uses_from_block(then_body, entry_states, moved_uses);
            if let Some(else_body) = else_body {
                collect_ordered_moved_uses_from_block(else_body, entry_states, moved_uses);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let (Some(condition), Some(mut state)) = (condition, entry_state) {
                collect_ordered_moved_uses_from_expr(condition, &mut state, moved_uses);
            }
            collect_ordered_moved_uses_from_block(body, entry_states, moved_uses);
        }
        HirStmt::For { iterable, body, .. } => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(iterable, &mut state, moved_uses);
            }
            collect_ordered_moved_uses_from_block(body, entry_states, moved_uses);
        }
        HirStmt::Match {
            value,
            scrutinee_effect,
            arms,
            ..
        } => {
            if let Some(mut state) = entry_state {
                collect_ordered_moved_uses_from_expr(value, &mut state, moved_uses);
                apply_match_take_move(*scrutinee_effect, value, &mut state);
            }
            for arm in arms {
                collect_ordered_moved_uses_from_block(&arm.body, entry_states, moved_uses);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                if let Some(mut state) = entry_state.clone() {
                    collect_ordered_moved_uses_from_expr(&arm.operation, &mut state, moved_uses);
                }
                collect_ordered_moved_uses_from_block(&arm.body, entry_states, moved_uses);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_ordered_moved_uses_from_expr(
    expr: &HirExpr,
    state: &mut BodyState,
    moved_uses: &mut Vec<MovedUse>,
) {
    match expr {
        HirExpr::Ident { name, span, .. } => {
            if let Some(move_span) = state.move_span(name) {
                push_moved_use(moved_uses, name.clone(), span.clone(), move_span.clone());
            } else if let Some((moved_path, move_span)) = state.moved_subpath_span(name) {
                push_moved_use(moved_uses, moved_path, span.clone(), move_span.clone());
            }
        }
        HirExpr::Call { args, events, .. } => {
            for arg in args {
                collect_ordered_moved_uses_from_expr(&arg.value, state, moved_uses);
            }
            state.apply_move_events(events);
        }
        HirExpr::Effect { value, events, .. } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
            state.apply_move_events(events);
        }
        HirExpr::Manage {
            value,
            events,
            span,
            ..
        } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
            state.apply_move_events(events);
            if let Some((path, _)) = hir_expr_path(value) {
                state.mark_moved(&path, span.clone());
            }
        }
        HirExpr::Spawn { value, .. } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
        }
        HirExpr::Await { value, .. } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
        }
        HirExpr::Try { value, .. } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_ordered_moved_uses_from_expr(left, state, moved_uses);
            collect_ordered_moved_uses_from_expr(right, state, moved_uses);
        }
        HirExpr::Field { base, .. } => {
            collect_field_move_use(expr, base, state, moved_uses);
        }
        HirExpr::Index { base, index, .. } => {
            collect_ordered_moved_uses_from_expr(base, state, moved_uses);
            collect_ordered_moved_uses_from_expr(index, state, moved_uses);
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_ordered_moved_uses_from_expr(&field.value, state, moved_uses);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_ordered_moved_uses_from_expr(item, state, moved_uses);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_ordered_moved_uses_from_expr(&entry.key, state, moved_uses);
                collect_ordered_moved_uses_from_expr(&entry.value, state, moved_uses);
            }
        }
        HirExpr::Closure { body, .. } => {
            for (name, span) in rsscript_semantics::hir_block_identifier_uses(body) {
                if let Some(move_span) = state.move_span(&name) {
                    push_moved_use(moved_uses, name, span, move_span.clone());
                }
            }
        }
        HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
        HirExpr::Match {
            value,
            scrutinee_effect,
            arms,
            ..
        } => {
            collect_ordered_moved_uses_from_expr(value, state, moved_uses);
            apply_match_take_move(*scrutinee_effect, value, state);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_ordered_moved_uses_from_expr(guard, state, moved_uses);
                }
                // Each arm body starts from the post-scrutinee state; walk it with
                // a per-arm clone so a move in one arm doesn't leak to siblings or
                // later code. (Previously analyzed with an empty state, which
                // skipped use-after-move inside match-*expression* arm bodies — the
                // match-statement path already threads real flow states.)
                collect_ordered_moved_uses_from_block_threaded(
                    &arm.body,
                    &mut state.clone(),
                    moved_uses,
                );
            }
        }
    }
}

/// Handle the `HirExpr::Field` branch of move-use collection: resolve the field
/// access to a path, recording a moved-use against the moved root or the moved
/// path, falling back to recursing into the base when no path resolves.
pub(super) fn collect_field_move_use(
    expr: &HirExpr,
    base: &HirExpr,
    state: &mut BodyState,
    moved_uses: &mut Vec<MovedUse>,
) {
    if let Some((path, span)) = hir_expr_path(expr) {
        if let Some(root) = path_root(&path)
            && let Some(move_span) = state.move_span(root)
        {
            push_moved_use(moved_uses, root.to_string(), span, move_span.clone());
            return;
        }
        if let Some((moved_path, move_span)) = state.moved_path_span(&path) {
            push_moved_use(moved_uses, moved_path, span, move_span.clone());
        }
    } else {
        collect_ordered_moved_uses_from_expr(base, state, moved_uses);
    }
}

/// Walk a block with a *threaded* move state (rather than the precomputed
/// per-statement flow map). Used for match-*expression* arm bodies, which the CFG
/// flow analysis does not weave in. It catches straight-line use-after-move within
/// the arm; nested control flow is analyzed with a per-branch clone, so it may
/// under-report across complex branches but never over-reports (no false positive
/// move errors). The expr walker mutates `state` as it applies each statement's
/// move events, so sequential moves are visible to later statements.
pub(super) fn collect_ordered_moved_uses_from_block_threaded(
    block: &HirBlock,
    state: &mut BodyState,
    moved_uses: &mut Vec<MovedUse>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value) => {
                collect_ordered_moved_uses_from_expr(value, state, moved_uses);
            }
            HirStmt::Assign { target, value, .. } => {
                collect_ordered_moved_uses_from_expr(value, state, moved_uses);
                for read in crate::hir::assign_target_reads(target) {
                    collect_ordered_moved_uses_from_expr(read, state, moved_uses);
                }
            }
            HirStmt::With { resource, body, .. } => {
                collect_ordered_moved_uses_from_expr(resource, state, moved_uses);
                collect_ordered_moved_uses_from_block_threaded(
                    body,
                    &mut state.clone(),
                    moved_uses,
                );
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_ordered_moved_uses_from_expr(condition, state, moved_uses);
                collect_ordered_moved_uses_from_block_threaded(
                    then_body,
                    &mut state.clone(),
                    moved_uses,
                );
                if let Some(else_body) = else_body {
                    collect_ordered_moved_uses_from_block_threaded(
                        else_body,
                        &mut state.clone(),
                        moved_uses,
                    );
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_ordered_moved_uses_from_expr(condition, state, moved_uses);
                }
                collect_ordered_moved_uses_from_block_threaded(
                    body,
                    &mut state.clone(),
                    moved_uses,
                );
            }
            HirStmt::For { iterable, body, .. } => {
                collect_ordered_moved_uses_from_expr(iterable, state, moved_uses);
                collect_ordered_moved_uses_from_block_threaded(
                    body,
                    &mut state.clone(),
                    moved_uses,
                );
            }
            HirStmt::Match {
                value,
                scrutinee_effect,
                arms,
                ..
            } => {
                collect_ordered_moved_uses_from_expr(value, state, moved_uses);
                apply_match_take_move(*scrutinee_effect, value, state);
                for arm in arms {
                    if let Some(guard) = &arm.guard {
                        collect_ordered_moved_uses_from_expr(guard, state, moved_uses);
                    }
                    collect_ordered_moved_uses_from_block_threaded(
                        &arm.body,
                        &mut state.clone(),
                        moved_uses,
                    );
                }
            }
            _ => {}
        }
    }
}

pub(super) fn apply_match_take_move(
    effect: Option<crate::syntax::ast::DataEffect>,
    value: &HirExpr,
    state: &mut BodyState,
) {
    if effect != Some(crate::syntax::ast::DataEffect::Take) {
        return;
    }
    if let Some((path, span)) = hir_expr_path(value) {
        state.mark_moved(&path, span);
    }
}

pub(super) fn collect_closure_local_moved_uses_from_block(
    block: &HirBlock,
    moved_uses: &mut Vec<MovedUse>,
) {
    for statement in &block.statements {
        collect_closure_local_moved_uses_from_stmt(statement, moved_uses);
    }
}

pub(super) fn collect_closure_local_moved_uses_from_stmt(
    statement: &HirStmt,
    moved_uses: &mut Vec<MovedUse>,
) {
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => collect_closure_local_moved_uses_from_expr(value, moved_uses),
        HirStmt::Assign { target, value, .. } => {
            for read in crate::hir::assign_target_reads(target) {
                collect_closure_local_moved_uses_from_expr(read, moved_uses);
            }
            collect_closure_local_moved_uses_from_expr(value, moved_uses);
        }
        HirStmt::With { resource, body, .. } => {
            collect_closure_local_moved_uses_from_expr(resource, moved_uses);
            collect_closure_local_moved_uses_from_block(body, moved_uses);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_closure_local_moved_uses_from_expr(condition, moved_uses);
            collect_closure_local_moved_uses_from_block(then_body, moved_uses);
            if let Some(else_body) = else_body {
                collect_closure_local_moved_uses_from_block(else_body, moved_uses);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_closure_local_moved_uses_from_expr(condition, moved_uses);
            }
            collect_closure_local_moved_uses_from_block(body, moved_uses);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_closure_local_moved_uses_from_expr(iterable, moved_uses);
            collect_closure_local_moved_uses_from_block(body, moved_uses);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_closure_local_moved_uses_from_expr(value, moved_uses);
            for arm in arms {
                collect_closure_local_moved_uses_from_block(&arm.body, moved_uses);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_closure_local_moved_uses_from_expr(&arm.operation, moved_uses);
                collect_closure_local_moved_uses_from_block(&arm.body, moved_uses);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_closure_local_moved_uses_from_expr(
    expr: &HirExpr,
    moved_uses: &mut Vec<MovedUse>,
) {
    match expr {
        HirExpr::Closure { body, .. } => {
            let steps = collect_local_flow_steps(body);
            let entry_states = collect_flow_entry_states(&steps, BodyState::default());
            collect_ordered_moved_uses_from_block(body, &entry_states, moved_uses);
            collect_closure_local_moved_uses_from_block(body, moved_uses);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_closure_local_moved_uses_from_expr(&arg.value, moved_uses);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. }
        | HirExpr::Field { base: value, .. } => {
            collect_closure_local_moved_uses_from_expr(value, moved_uses);
        }
        HirExpr::Index { base, index, .. } => {
            collect_closure_local_moved_uses_from_expr(base, moved_uses);
            collect_closure_local_moved_uses_from_expr(index, moved_uses);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_closure_local_moved_uses_from_expr(left, moved_uses);
            collect_closure_local_moved_uses_from_expr(right, moved_uses);
        }
        // Recurse into match-expression scrutinee, guards, and arm bodies so a
        // closure nested under a match-expression arm is still scanned for
        // captured moved locals (previously a no-op).
        HirExpr::Match { value, arms, .. } => {
            collect_closure_local_moved_uses_from_expr(value, moved_uses);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_closure_local_moved_uses_from_expr(guard, moved_uses);
                }
                collect_closure_local_moved_uses_from_block(&arm.body, moved_uses);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_closure_local_moved_uses_from_expr(&entry.key, moved_uses);
                collect_closure_local_moved_uses_from_expr(&entry.value, moved_uses);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_closure_local_moved_uses_from_expr(&field.value, moved_uses);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_closure_local_moved_uses_from_expr(item, moved_uses);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn push_moved_use(
    moved_uses: &mut Vec<MovedUse>,
    name: String,
    use_span: Span,
    move_span: Span,
) {
    let moved_use = MovedUse {
        name,
        use_span,
        move_span,
    };
    if !moved_uses.contains(&moved_use) {
        moved_uses.push(moved_use);
    }
}

pub(super) fn hir_expr_path(expr: &HirExpr) -> Option<(String, Span)> {
    match expr {
        HirExpr::Ident { name, span, .. } => Some((name.clone(), span.clone())),
        HirExpr::Field {
            base, name, span, ..
        } => {
            let (mut base_path, _) = hir_expr_path(base)?;
            base_path.push('.');
            base_path.push_str(name);
            Some((base_path, span.clone()))
        }
        _ => None,
    }
}

pub(super) fn path_root(path: &str) -> Option<&str> {
    path.split('.').next().filter(|root| !root.is_empty())
}

pub(super) fn collect_retained_closure_captures_from_stmt(
    statement: &HirStmt,
    entry_states: &HashMap<Span, BodyState>,
    captures: &mut Vec<RetainedClosureCapture>,
) {
    let entry_state = entry_states.get(hir_stmt_span(statement));
    match statement {
        HirStmt::Let {
            value: Some(value), ..
        }
        | HirStmt::Return {
            value: Some(value), ..
        }
        | HirStmt::Expr(value) => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(value, state, captures);
            }
        }
        HirStmt::Assign { target, value, .. } => {
            if let Some(state) = entry_state {
                for read in crate::hir::assign_target_reads(target) {
                    collect_retained_closure_captures_from_expr(read, state, captures);
                }
                collect_retained_closure_captures_from_expr(value, state, captures);
            }
        }
        HirStmt::With { resource, body, .. } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(resource, state, captures);
            }
            collect_retained_closure_captures_from_block(body, entry_states, captures);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(condition, state, captures);
            }
            collect_retained_closure_captures_from_block(then_body, entry_states, captures);
            if let Some(else_body) = else_body {
                collect_retained_closure_captures_from_block(else_body, entry_states, captures);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let (Some(condition), Some(state)) = (condition, entry_state) {
                collect_retained_closure_captures_from_expr(condition, state, captures);
            }
            collect_retained_closure_captures_from_block(body, entry_states, captures);
        }
        HirStmt::For { iterable, body, .. } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(iterable, state, captures);
            }
            collect_retained_closure_captures_from_block(body, entry_states, captures);
        }
        HirStmt::Match { value, arms, .. } => {
            if let Some(state) = entry_state {
                collect_retained_closure_captures_from_expr(value, state, captures);
            }
            for arm in arms {
                collect_retained_closure_captures_from_block(&arm.body, entry_states, captures);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                if let Some(state) = entry_state {
                    collect_retained_closure_captures_from_expr(&arm.operation, state, captures);
                }
                collect_retained_closure_captures_from_block(&arm.body, entry_states, captures);
            }
        }
        HirStmt::Let { value: None, .. }
        | HirStmt::Return { value: None, .. }
        | HirStmt::Break(_)
        | HirStmt::Continue(_)
        | HirStmt::Unknown(_) => {}
    }
}

pub(super) fn collect_retained_closure_captures_from_expr(
    expr: &HirExpr,
    state: &BodyState,
    captures: &mut Vec<RetainedClosureCapture>,
) {
    match expr {
        HirExpr::Call {
            callee,
            args,
            resolution,
            ..
        } => {
            if let CallResolution::Resolved { signature, .. } = resolution {
                for arg in args {
                    let Some(name) = arg.name.as_ref() else {
                        continue;
                    };
                    if !signature.retained_params.contains(name) {
                        continue;
                    }
                    let Some((body, closure_span)) = retained_closure_arg(&arg.value) else {
                        continue;
                    };
                    let mut uses = Vec::new();
                    collect_hir_block_inline_capture_uses(body, &mut uses);
                    for (used_name, capture_span) in uses {
                        if state.is_local(&used_name) {
                            push_retained_closure_capture(
                                captures,
                                RetainedClosureCapture {
                                    name: used_name,
                                    callee: callee_display(callee),
                                    param: name.clone(),
                                    capture_span,
                                    closure_span: closure_span.clone(),
                                },
                            );
                        }
                    }
                }
            }
            for arg in args {
                collect_retained_closure_captures_from_expr(&arg.value, state, captures);
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_retained_closure_captures_from_expr(value, state, captures);
        }
        HirExpr::Binary { left, right, .. } => {
            collect_retained_closure_captures_from_expr(left, state, captures);
            collect_retained_closure_captures_from_expr(right, state, captures);
        }
        HirExpr::Field { base, .. } => {
            collect_retained_closure_captures_from_expr(base, state, captures);
        }
        HirExpr::Index { base, index, .. } => {
            collect_retained_closure_captures_from_expr(base, state, captures);
            collect_retained_closure_captures_from_expr(index, state, captures);
        }
        HirExpr::Closure { body, .. } => {
            collect_retained_closure_captures_from_block(body, &HashMap::new(), captures);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_retained_closure_captures_from_expr(value, state, captures);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_retained_closure_captures_from_expr(guard, state, captures);
                }
                collect_retained_closure_captures_from_block(&arm.body, &HashMap::new(), captures);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_retained_closure_captures_from_expr(&entry.key, state, captures);
                collect_retained_closure_captures_from_expr(&entry.value, state, captures);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_retained_closure_captures_from_expr(&field.value, state, captures);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_retained_closure_captures_from_expr(item, state, captures);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn retained_closure_arg(expr: &HirExpr) -> Option<(&HirBlock, &Span)> {
    match expr {
        HirExpr::Closure { body, span, .. } => Some((body, span)),
        HirExpr::Effect {
            effect: ParamEffect::Read,
            value,
            ..
        } => retained_closure_arg(value),
        HirExpr::Call { callee, args, .. } if retained_closure_wrapper_callee(callee) => {
            args.iter().find_map(|arg| retained_closure_arg(&arg.value))
        }
        HirExpr::Effect { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Manage { .. }
        | HirExpr::Spawn { .. }
        | HirExpr::Await { .. }
        | HirExpr::Try { .. }
        | HirExpr::Match { .. }
        | HirExpr::Binary { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Field { .. }
        | HirExpr::Index { .. }
        | HirExpr::Call { .. }
        | HirExpr::Unknown(_) => None,
    }
}

pub(super) fn retained_closure_wrapper_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Name(name) if matches!(name.as_str(), "Ok" | "Err" | "Some")
    )
}

pub(super) fn push_retained_closure_capture(
    captures: &mut Vec<RetainedClosureCapture>,
    capture: RetainedClosureCapture,
) {
    if !captures.contains(&capture) {
        captures.push(capture);
    }
}

pub(super) fn callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => format!(
            "{} {}.{method}",
            (*effect).map(|e| e.as_str()).unwrap_or("read"),
            local_expr_label(receiver)
        ),
    }
}

pub(super) fn local_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::CharLiteral(value, _) | Expr::MultilineString(value, _) => {
            format!("{value:?}")
        }
        Expr::Field { base, name, .. } => format!("{}.{}", local_expr_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", local_expr_label(base)),
        Expr::Call { callee, .. } => format!("{}()", callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            local_expr_label(value)
        }
        _ => "<expr>".to_string(),
    }
}
