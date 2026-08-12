use super::*;
use crate::checks::diagnostic_helpers::error_cause_manual_fix;

pub(super) fn check_call_place_conflicts(
    analyzer: &mut Analyzer<'_>,
    args: &[HirCallArg],
    resolution: &CallResolution,
    state: &BodyState,
) {
    let mut accesses = args
        .iter()
        .filter_map(call_place_access)
        .collect::<Vec<_>>();

    // A closure literal passed to a `noescape` parameter is invoked within this
    // call, so its captured read/mut/take uses participate in the same-call
    // conflict check as synthetic accesses. Otherwise a `noescape` callback
    // becomes a back door that hides mutation/retention of a captured place that
    // also appears as a direct argument of the same call.
    let noescape_params: Vec<&str> = match resolution {
        CallResolution::Resolved { signature, .. } => signature
            .params
            .iter()
            .filter(|param| param.ty.qualifiers.noescape)
            .map(|param| param.name.as_str())
            .collect(),
        _ => Vec::new(),
    };
    if !noescape_params.is_empty() {
        for arg in args {
            let Some(name) = arg.name.as_deref() else {
                continue;
            };
            if !noescape_params.contains(&name) {
                continue;
            }
            if let HirExpr::Closure { body, .. } = &arg.value {
                let mut closure_accesses = Vec::new();
                collect_closure_capture_accesses(body, &mut closure_accesses);
                check_noescape_consuming_captured_locals(analyzer, &closure_accesses, state);
                accesses.extend(closure_accesses);
            }
        }
    }

    // Field splitting (treating distinct inline fields of one base as disjoint)
    // is a local-only external_binding. `local` bindings and `take` parameters are
    // provably exclusive; `mut` parameters are not, because a caller may pass a
    // managed-backed value.
    for access in &mut accesses {
        access.base_is_local = base_allows_field_split(analyzer, state, &access.path.base);
    }

    for left_index in 0..accesses.len() {
        for right in accesses.iter().skip(left_index + 1) {
            check_place_pair_conflict(analyzer, &accesses[left_index], right);
        }
    }
}

/// Collect the read/mut/take uses a closure body makes of captured places (free
/// variables), as synthetic call accesses. Names bound inside the closure
/// (`let`/`local`/`with`) are not captures and are excluded.
pub(super) fn collect_closure_capture_accesses(body: &HirBlock, out: &mut Vec<CallPlaceAccess>) {
    let mut bound = HashSet::new();
    collect_closure_bound_names(body, &mut bound);
    collect_closure_effect_accesses_block(body, &bound, out);
}

pub(super) fn check_noescape_consuming_captured_locals(
    analyzer: &mut Analyzer<'_>,
    accesses: &[CallPlaceAccess],
    state: &BodyState,
) {
    for access in accesses {
        if access.moves_path && state.is_local(&access.path.base) {
            analyzer
                .diagnostics
                .push(rsscript_semantics::noescape_consumes_capture_diagnostic(
                    &access.path.base,
                    access.span.clone(),
                ));
        }
    }
}

pub(super) fn collect_closure_bound_names(block: &HirBlock, bound: &mut HashSet<String>) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let { name, .. } => {
                bound.insert(name.clone());
            }
            HirStmt::With { binding, body, .. } => {
                bound.insert(binding.clone());
                collect_closure_bound_names(body, bound);
            }
            HirStmt::If {
                then_body,
                else_body,
                ..
            } => {
                collect_closure_bound_names(then_body, bound);
                if let Some(else_body) = else_body {
                    collect_closure_bound_names(else_body, bound);
                }
            }
            HirStmt::Loop { body, .. } => collect_closure_bound_names(body, bound),
            HirStmt::For { binding, body, .. } => {
                bound.insert(binding.clone());
                collect_closure_bound_names(body, bound);
            }
            HirStmt::Match { arms, .. } => {
                for arm in arms {
                    collect_closure_bound_names(&arm.body, bound);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_closure_bound_names(&arm.body, bound);
                }
            }
            HirStmt::Return { .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Expr(_)
            | HirStmt::Assign { .. }
            | HirStmt::Unknown(_) => {}
        }
    }
}

pub(super) fn collect_closure_effect_accesses_block(
    block: &HirBlock,
    bound: &HashSet<String>,
    out: &mut Vec<CallPlaceAccess>,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let {
                value: Some(value), ..
            }
            | HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                collect_closure_effect_accesses_expr(value, bound, out)
            }
            HirStmt::With { resource, body, .. } => {
                collect_closure_effect_accesses_expr(resource, bound, out);
                collect_closure_effect_accesses_block(body, bound, out);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_closure_effect_accesses_expr(condition, bound, out);
                collect_closure_effect_accesses_block(then_body, bound, out);
                if let Some(else_body) = else_body {
                    collect_closure_effect_accesses_block(else_body, bound, out);
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_closure_effect_accesses_expr(condition, bound, out);
                }
                collect_closure_effect_accesses_block(body, bound, out);
            }
            HirStmt::For { iterable, body, .. } => {
                collect_closure_effect_accesses_expr(iterable, bound, out);
                collect_closure_effect_accesses_block(body, bound, out);
            }
            HirStmt::Match { value, arms, .. } => {
                collect_closure_effect_accesses_expr(value, bound, out);
                for arm in arms {
                    collect_closure_effect_accesses_block(&arm.body, bound, out);
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_closure_effect_accesses_expr(&arm.operation, bound, out);
                    collect_closure_effect_accesses_block(&arm.body, bound, out);
                }
            }
            HirStmt::Let { value: None, .. }
            | HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

pub(super) fn collect_closure_effect_accesses_expr(
    expr: &HirExpr,
    bound: &HashSet<String>,
    out: &mut Vec<CallPlaceAccess>,
) {
    match expr {
        HirExpr::Effect { value, span, .. } => {
            if let Some(path) = place_path(value)
                && !bound.contains(&path.base)
            {
                out.push(CallPlaceAccess {
                    effect: effect_of(expr),
                    moves_path: expr_moves_path(expr),
                    path,
                    base_is_local: false,
                    span: span.clone(),
                });
            }
            collect_closure_effect_accesses_expr(value, bound, out);
        }
        HirExpr::Manage { value, span, .. } => {
            // `manage x` is a move of the captured place; it conflicts with any
            // other same-call use of that place (§8.4 manage rule).
            if let Some(path) = place_path(value)
                && !bound.contains(&path.base)
            {
                out.push(CallPlaceAccess {
                    effect: ParamEffect::Read,
                    moves_path: true,
                    path,
                    base_is_local: false,
                    span: span.clone(),
                });
            }
            collect_closure_effect_accesses_expr(value, bound, out);
        }
        HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => collect_closure_effect_accesses_expr(value, bound, out),
        HirExpr::Binary { left, right, .. } => {
            collect_closure_effect_accesses_expr(left, bound, out);
            collect_closure_effect_accesses_expr(right, bound, out);
        }
        HirExpr::Field { base, .. } => collect_closure_effect_accesses_expr(base, bound, out),
        HirExpr::Index { base, index, .. } => {
            collect_closure_effect_accesses_expr(base, bound, out);
            collect_closure_effect_accesses_expr(index, bound, out);
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_closure_effect_accesses_expr(&arg.value, bound, out);
            }
        }
        HirExpr::Closure { body, .. } => collect_closure_effect_accesses_block(body, bound, out),
        HirExpr::Match { value, arms, .. } => {
            collect_closure_effect_accesses_expr(value, bound, out);
            for arm in arms {
                if let Some(guard) = &arm.guard {
                    collect_closure_effect_accesses_expr(guard, bound, out);
                }
                collect_closure_effect_accesses_block(&arm.body, bound, out);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_closure_effect_accesses_expr(&entry.key, bound, out);
                collect_closure_effect_accesses_expr(&entry.value, bound, out);
            }
        }
        HirExpr::ObjectLiteral { fields, .. } => {
            for field in fields {
                collect_closure_effect_accesses_expr(&field.value, bound, out);
            }
        }
        HirExpr::ArrayLiteral { items, .. } => {
            for item in items {
                collect_closure_effect_accesses_expr(item, bound, out);
            }
        }
        HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => {}
    }
}

pub(super) fn effect_of(expr: &HirExpr) -> ParamEffect {
    match expr {
        HirExpr::Effect { effect, .. } => *effect,
        _ => ParamEffect::Read,
    }
}

pub(super) fn call_place_access(arg: &HirCallArg) -> Option<CallPlaceAccess> {
    let HirExpr::Effect {
        effect,
        value,
        span,
        ..
    } = &arg.value
    else {
        return None;
    };
    let path = place_path(value)?;
    Some(CallPlaceAccess {
        effect: *effect,
        moves_path: expr_moves_path(&arg.value),
        path,
        base_is_local: false,
        span: span.clone(),
    })
}

pub(super) fn expr_moves_path(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Take,
            ..
        }
        | HirExpr::Manage { .. } => true,
        HirExpr::Effect { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => expr_moves_path(value),
        HirExpr::Field { base, .. } => expr_moves_path(base),
        HirExpr::Index { base, index, .. } => expr_moves_path(base) || expr_moves_path(index),
        HirExpr::Binary { left, right, .. } => expr_moves_path(left) || expr_moves_path(right),
        HirExpr::Call { args, .. } => args.iter().any(|arg| expr_moves_path(&arg.value)),
        HirExpr::ObjectLiteral { fields, .. } => {
            fields.iter().any(|field| expr_moves_path(&field.value))
        }
        HirExpr::MapLiteral { entries, .. } => entries
            .iter()
            .any(|entry| expr_moves_path(&entry.key) || expr_moves_path(&entry.value)),
        HirExpr::ArrayLiteral { items, .. } => items.iter().any(expr_moves_path),
        HirExpr::Closure { .. }
        | HirExpr::Match { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Char { .. }
        | HirExpr::Unknown(_) => false,
    }
}

pub(super) fn place_path(expr: &HirExpr) -> Option<PlacePath> {
    match expr {
        HirExpr::Ident { name, .. } => Some(PlacePath {
            base: name.clone(),
            components: Vec::new(),
            has_index: false,
            crosses_handle: false,
        }),
        HirExpr::Field {
            base, name, access, ..
        } => {
            let mut path = place_path(base)?;
            path.components.push(name.clone());
            if access.is_handle {
                path.crosses_handle = true;
            }
            Some(path)
        }
        HirExpr::Index { base, .. } => {
            let mut path = place_path(base)?;
            path.has_index = true;
            Some(path)
        }
        HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. } => place_path(value),
        _ => None,
    }
}

pub(super) fn check_place_pair_conflict(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
) {
    if left.path.base != right.path.base {
        return;
    }

    if move_base_field_conflict(left, right) {
        move_base_field_conflict_diagnostic(analyzer, left, right);
        return;
    }

    if !pair_mutates(left, right) {
        return;
    }

    if left.path.has_index || right.path.has_index {
        indexed_place_conflict_diagnostic(analyzer, left, right);
        return;
    }

    if whole_base_or_prefix_access(&left.path, &right.path) {
        field_partial_access_conflict_diagnostic(analyzer, left, right);
        return;
    }

    if left.path.crosses_handle || right.path.crosses_handle {
        field_prefix_conflict_diagnostic(
            analyzer,
            left,
            right,
            "handle fields terminate local-inline disjointness analysis.",
        );
        return;
    }

    if path_prefix_or_equal(&left.path.components, &right.path.components) {
        field_prefix_conflict_diagnostic(
            analyzer,
            left,
            right,
            "one local field path is the same as, or a prefix of, the other.",
        );
        return;
    }

    if !left.base_is_local {
        managed_field_split_conflict_diagnostic(analyzer, left, right);
    }
}

/// A base supports field splitting (distinct inline fields treated as disjoint)
/// only when it is a locally exclusive value. `mut` parameters are not
/// splittable even when their declared type is a struct: the call site may pass
/// a managed-backed value, so the callee cannot assume field disjointness.
pub(super) fn base_allows_field_split(
    analyzer: &Analyzer<'_>,
    state: &BodyState,
    base: &str,
) -> bool {
    if !state.allows_field_split(base) {
        return false;
    }
    match state.value_type(base) {
        Some(type_name) => {
            let root = type_name
                .split_once('<')
                .map_or(type_name, |(root, _)| root);
            let is_container = matches!(root, "List" | "Map" | "Set");
            let is_class = analyzer.hir.type_kind(root) == Some(HirTypeKind::Class);
            !is_container && !is_class
        }
        None => true,
    }
}

pub(super) fn managed_field_split_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::MANAGED_FIELD_SPLIT_CONFLICT,
        format!(
            "managed object fields `{}` and `{}` cannot be split in one call.",
            place_path_display(&left.path),
            place_path_display(&right.path)
        ),
        right.span.clone(),
        "managed field split conflict",
        "Field splitting into disjoint inline paths is a local-only external_binding. A managed object is a single runtime value behind one write guard, so two mutable accesses to its inline fields conflict; the conflict root is the managed object base.",
        "split_managed_field_accesses",
        "Split the accesses into separate statements, or move the fields behind explicit `handle` fields so they become distinct managed objects.",
    ));
}

pub(super) fn move_base_field_conflict(left: &CallPlaceAccess, right: &CallPlaceAccess) -> bool {
    (left.moves_path && move_path_conflicts_with_access(&left.path, &right.path))
        || (right.moves_path && move_path_conflicts_with_access(&right.path, &left.path))
}

pub(super) fn move_path_conflicts_with_access(moved: &PlacePath, accessed: &PlacePath) -> bool {
    if moved.components.is_empty() || accessed.components.is_empty() {
        return true;
    }

    moved.has_index
        || accessed.has_index
        || moved.crosses_handle
        || accessed.crosses_handle
        || path_prefix_or_equal(&moved.components, &accessed.components)
}

pub(super) fn pair_mutates(left: &CallPlaceAccess, right: &CallPlaceAccess) -> bool {
    mutates(left.effect) || mutates(right.effect)
}

pub(super) fn mutates(effect: ParamEffect) -> bool {
    matches!(effect, ParamEffect::Mut | ParamEffect::Take)
}

pub(super) fn whole_base_or_prefix_access(left: &PlacePath, right: &PlacePath) -> bool {
    left.components.is_empty() != right.components.is_empty()
        && (is_prefix(&left.components, &right.components)
            || is_prefix(&right.components, &left.components))
}

pub(super) fn path_prefix_or_equal(left: &[String], right: &[String]) -> bool {
    is_prefix(left, right) || is_prefix(right, left)
}

pub(super) fn is_prefix(left: &[String], right: &[String]) -> bool {
    left.len() <= right.len()
        && left
            .iter()
            .zip(right.iter())
            .all(|(left, right)| left == right)
}

pub(super) fn place_path_display(path: &PlacePath) -> String {
    let mut output = path.base.clone();
    for component in &path.components {
        output.push('.');
        output.push_str(component);
    }
    if path.has_index {
        output.push_str("[...]");
    }
    output
}

pub(super) fn field_partial_access_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::FIELD_PARTIAL_ACCESS_CONFLICT,
        format!(
            "call mixes whole local access `{}` with field access `{}`.",
            place_path_display(&left.path),
            place_path_display(&right.path)
        ),
        right.span.clone(),
        "whole-base field conflict",
        "A whole local base or prefix conflicts with a mutable or taking subpath in the same call.",
        "split_call",
        "Split the whole-base read and field mutation into separate statements or pass disjoint fields explicitly.",
    ));
}

pub(super) fn field_prefix_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
    cause: &str,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::FIELD_PREFIX_CONFLICT,
        format!(
            "local field paths `{}` and `{}` are not disjoint.",
            place_path_display(&left.path),
            place_path_display(&right.path)
        ),
        right.span.clone(),
        "field path conflict",
        cause,
        "split_or_refactor_paths",
        "Split the accesses into separate calls or refactor through explicit split APIs.",
    ));
}

pub(super) fn indexed_place_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
) {
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::INDEXED_PARTIAL_ACCESS_CONFLICT,
        format!(
            "indexed local paths `{}` and `{}` cannot be proven disjoint.",
            place_path_display(&left.path),
            place_path_display(&right.path)
        ),
        right.span.clone(),
        "indexed local access conflict",
        "RSScript v0.7 treats indexed access as access to the whole local container for alias checking.",
        "use_split_api",
        "Use an explicit container split API that proves or checks disjoint element access.",
    ));
}

pub(super) fn move_base_field_conflict_diagnostic(
    analyzer: &mut Analyzer<'_>,
    left: &CallPlaceAccess,
    right: &CallPlaceAccess,
) {
    let (moved, accessed) = if left.moves_path {
        (left, right)
    } else {
        (right, left)
    };
    analyzer.diagnostics.push(error_cause_manual_fix(
        code::MOVE_BASE_FIELD_CONFLICT,
        format!(
            "call moves local path `{}` while also accessing `{}`.",
            place_path_display(&moved.path),
            place_path_display(&accessed.path)
        ),
        moved.span.clone(),
        "move-base field conflict",
        "A local base cannot be `manage`d or `take`n in the same expression where one of its fields is accessed.",
        "split_move_from_field_access",
        "Split the field access and `manage`/`take` into separate statements.",
    ));
}
