//! Local move, freshness, retained capture, and handle-field ownership analysis.

use super::*;

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
            if let Some((path, _)) = rsscript_semantics::hir_expr_path(value) {
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
    if let Some((path, span)) = rsscript_semantics::hir_expr_path(expr) {
        if let Some(root) = rsscript_semantics::path_root(&path)
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
    if let Some((path, span)) = rsscript_semantics::hir_expr_path(value) {
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
            let entry_states =
                rsscript_semantics::local_flow_entry_states(&steps, BodyState::default());
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
